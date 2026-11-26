#!/usr/bin/env bash
# 导航模式（5000 kind=2）就绪度体检 —— 只读，不动任何服务、不动机器人。
#
# 背景：2026-11-14 的崩溃根因是 .234（算法机）与 .1（运控机）的
# `robots_dog_msgs` 版本不一致（0.9.8 vs 0.9.0，`NavigationState.msg` 少一个
# `uint8 current_floor`）→ CDR 错位 → `dog_task` bad_alloc / exit code -6。
# 详见 deploy/robodog-issues-2026-11-14.md §3。
#
# 用法（**必须在 192.168.41.246 上跑**，开发机连不到 192.168.234.x）：
#
#   scp deploy/robodog-body/check-nav-mode.sh root@192.168.41.246:/tmp/
#   ssh root@192.168.41.246 'bash /tmp/check-nav-mode.sh'
#
# 输出：屏幕上的 PASS/FAIL 汇总 + 原始证据存到 /tmp/nav-mode-check-<时间戳>/
#       （把那个目录 tar 回来即可；不含口令、不含地图数据）
#
# 账号按 deploy/robodog-body.md §1.1：
#   .234 robot/1（sudo 也要密码）  .1 firefly/firefly（sudo 免密）
set -uo pipefail

ORIN="${ORIN:-robot@192.168.234.234}"
CTRL="${CTRL:-firefly@192.168.234.1}"
PASS_ORIN="${PASS_ORIN:-1}"
PASS_CTRL="${PASS_CTRL:-firefly}"
STAMP="$(date +%Y%m%d-%H%M%S)"
OUT="${OUT:-/tmp/nav-mode-check-$STAMP}"
# `--analyze DIR`：跳过采集，直接分析已有证据（重跑判读 / 验证脚本行为时用）
ANALYZE_ONLY=""
while [ $# -gt 0 ]; do
  case "$1" in
    --analyze) ANALYZE_ONLY="$2"; shift 2 ;;
    --analyze=*) ANALYZE_ONLY="${1#*=}"; shift ;;
    -h|--help)
      sed -n '2,18p' "$0"; exit 0 ;;
    *) echo "未知参数: $1（支持 --analyze DIR）" >&2; exit 2 ;;
  esac
done
[ -n "$ANALYZE_ONLY" ] && OUT="$ANALYZE_ONLY"
mkdir -p "$OUT"

SSH=(ssh -q -o StrictHostKeyChecking=no -o ConnectTimeout=8
     -o PreferredAuthentications=password -o PubkeyAuthentication=no)

# SSH_ASKPASS 的临时脚本：用完即删（不要留在现场）
ASK_DIR="$(mktemp -d /tmp/.navcheck.XXXXXX)"
chmod 700 "$ASK_DIR"
cleanup() { rm -rf "$ASK_DIR"; }
trap cleanup EXIT
printf '#!/bin/sh\nprintf "%%s\\n" "%s"\n' "$PASS_ORIN" > "$ASK_DIR/orin.sh"
printf '#!/bin/sh\nprintf "%%s\\n" "%s"\n' "$PASS_CTRL" > "$ASK_DIR/ctrl.sh"
chmod 700 "$ASK_DIR"/*.sh

# run_remote <askpass> <user@host> <outfile>
run_remote() {
  local ask="$1" host="$2" out="$3"
  SSH_ASKPASS="$ask" SSH_ASKPASS_REQUIRE=force setsid -w "${SSH[@]}" "$host" 'bash -s' \
    > "$out" 2>&1
}

# ── 远端采集脚本：只读命令，输出 KEY=VALUE 供本地解析 ─────────────────
read -r -d '' COLLECT <<'EOS'
echo "HOST=$(hostname 2>/dev/null)"
echo "UPTIME=$(uptime -p 2>/dev/null || uptime)"

# robots_dog_msgs：优先看 ROS 安装目录，其次 colcon/自编译目录
MSG=""
for p in /opt/ros/*/share/robots_dog_msgs/msg/NavigationState.msg \
         /opt/ros/*/install/robots_dog_msgs/share/robots_dog_msgs/msg/NavigationState.msg; do
  [ -f "$p" ] && MSG="$p" && break
done
if [ -n "$MSG" ]; then
  echo "MSG_PATH=$MSG"
  echo "MSG_MD5=$(md5sum "$MSG" | cut -d' ' -f1)"
  echo "MSG_LINES=$(wc -l < "$MSG")"
  echo "MSG_MTIME=$(date -r "$MSG" '+%Y-%m-%d %H:%M:%S' 2>/dev/null)"
  if grep -q 'current_floor' "$MSG"; then echo "MSG_FLOOR=yes"; else echo "MSG_FLOOR=no"; fi
  echo "MSG_FIELDS=$(grep -oE '^[a-zA-Z_][a-zA-Z0-9_]*[[:space:]]' "$MSG" | tr -d ' \t' | paste -sd, -)"
fi
PKG="$(dirname "$(dirname "$MSG")")/package.xml"
if [ -f "$PKG" ]; then
  echo "PKG_VERSION=$(grep -m1 -oE '<version>[^<]+' "$PKG" | cut -d'>' -f2)"
  echo "PKG_SHA=$(grep -m1 -oE '<version_control|url>[^<]+' "$PKG" | head -1)"
fi

# robot-launch 版本清单（.1 上有 robot_version_manager）
for f in /opt/robot/robot_version_manager/*/version.yaml /opt/robot/*/version.yaml; do
  [ -f "$f" ] && echo "VERSION_YAML=$f" && grep -iE 'dog_task|dog-task|msgs|robot_launch|version|commit|date' "$f" 2>/dev/null | head -20 | sed 's/^/VY: /'
done

# robot-launch 里 dog_task 的实时状态（.1 才有）
RL="$(robot-launch list 2>/dev/null)"
if [ -n "$RL" ]; then
  echo "RL_DOGTASK_LINE=$(printf '%s\n' "$RL" | grep -E 'dog_task' | head -1)"
  echo "RL_NAV_LINES=$(printf '%s\n' "$RL" | grep -E 'nav2|waypoint|patrol|localization' | head -5 | paste -sd'|' -)"
fi

# dog_task 二进制（新旧判定的直接证据）
PID="$(pgrep -o -f 'dog_task' 2>/dev/null)"
[ -n "$PID" ] && echo "DOGTASK_EXE=$(readlink -f "/proc/$PID/exe" 2>/dev/null)"
EXE="${DOGTASK_EXE:-}"
if [ -z "$EXE" ] || [ ! -e "$EXE" ]; then
  EXE="$(command -v dog_task 2>/dev/null)"
fi
if [ -n "$EXE" ] && [ -e "$EXE" ]; then
  echo "DOGTASK_BIN=$EXE"
  echo "DOGTASK_MTIME=$(date -r "$EXE" '+%Y-%m-%d %H:%M:%S' 2>/dev/null)"
  echo "DOGTASK_EPOCH=$(date -r "$EXE" '+%s' 2>/dev/null)"
  echo "DOGTASK_SIZE=$(stat -c%s "$EXE" 2>/dev/null)"
  strings "$EXE" 2>/dev/null | grep -m1 -E 'dog-task [0-9]+\.[0-9]+' | sed 's/^/DOGTASK_VERLINE=/'
  strings "$EXE" 2>/dev/null | grep -m1 'current_floor' >/dev/null && echo "DOGTASK_FLOOR_SYMBOL=yes" || echo "DOGTASK_FLOOR_SYMBOL=no"
fi

# 日志（.1 的 /home/firefly/.robot_launch_logs/dog_task.stdout）
LOG="$(ls -1 "$HOME"/.robot_launch_logs/dog_task.stdout 2>/dev/null | head -1)"
if [ -n "$LOG" ]; then
  echo "DOGTASK_LOG=$LOG"
  echo "DOGTASK_LOG_MTIME=$(date -r "$LOG" '+%Y-%m-%d %H:%M:%S' 2>/dev/null)"
  # 注意：grep -c 没命中时也会打印 0（退出码 1），所以只能 `|| true`，不能再 echo 0
  echo "DESER_COUNT=$(grep -c 'could not deserialize ROS message' "$LOG" 2>/dev/null || true)"
  echo "ALLOC_COUNT=$(grep -c 'bad_alloc' "$LOG" 2>/dev/null || true)"
  echo "EXIT6_COUNT=$(grep -c 'exit code -6' "$LOG" 2>/dev/null || true)"
  echo "NAVSTATE_COUNT=$(grep -c '/navigation_state' "$LOG" 2>/dev/null || true)"
  echo "LOG_TAIL_NAV=$(grep -E 'navigation_state|bad_alloc|exit code -6' "$LOG" 2>/dev/null | tail -3 | paste -sd'|' -)"
fi

# 共享内存占用（崩溃循环的直接副作用）
if [ -d /dev/shm ]; then
  echo "SHM=$(df -h /dev/shm 2>/dev/null | awk 'NR==2 {print $5"|"$3"/"$2}')"
  echo "SHM_SEGMENTS=$(ls -1 /dev/shm 2>/dev/null | wc -l)"
fi

# ROS 侧证据（best effort：需要能 source 环境）
ros_probe() {
  timeout 12 bash -lc 'source /opt/ros/humble/setup.bash 2>/dev/null
                       ros2 topic info /navigation_state -v 2>&1 | head -25' 2>/dev/null
}
echo "ROS_TOPIC_INFO=$(ros_probe | paste -sd'|' -)"
EOS

echo "== 采集 .234（算法机）"
if [ -n "$ANALYZE_ONLY" ]; then
  echo "   （--analyze：跳过采集，直接用 $OUT 里的证据）"
else
  run_remote "$ASK_DIR/orin.sh" "$ORIN" "$OUT/orin.txt" < <(printf '%s\n' "$COLLECT")
  echo "== 采集 .1（运控机）"
  run_remote "$ASK_DIR/ctrl.sh" "$CTRL" "$OUT/ctrl.txt" < <(printf '%s\n' "$COLLECT")
fi

get() { grep -m1 "^$2=" "$1" 2>/dev/null | cut -d= -f2-; }
PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf '  [PASS] %s\n' "$*"; }
bad()  { FAIL=$((FAIL+1)); printf '  [FAIL] %s\n' "$*"; }
warn() { printf '  [WARN] %s\n' "$*"; }
skip() { printf '  [SKIP] %s\n' "$*"; }

O_MSG="$OUT/orin.txt"; C_MSG="$OUT/ctrl.txt"

echo
echo "== 结论"
echo "-- A. 消息定义一致（根因）"
O_MD5="$(get "$O_MSG" MSG_MD5)"; C_MD5="$(get "$C_MSG" MSG_MD5)"
O_FLOOR="$(get "$O_MSG" MSG_FLOOR)"; C_FLOOR="$(get "$C_MSG" MSG_FLOOR)"
if [ -z "$O_MD5" ] || [ -z "$C_MD5" ]; then
  skip "有一台没找到 NavigationState.msg（.234=$(get "$O_MSG" MSG_PATH)，.1=$(get "$C_MSG" MSG_PATH)）"
elif [ "$O_MD5" = "$C_MD5" ]; then
  ok "NavigationState.msg md5 一致：$O_MD5（$(get "$O_MSG" MSG_LINES)/$(get "$C_MSG" MSG_LINES) 行）"
  [ "$O_FLOOR" = yes ] && [ "$C_FLOOR" = yes ] && ok "两边都有 current_floor" \
    || warn "md5 一致但字段里没看到 current_floor，确认这份 msg 是不是目标版本"
else
  bad "md5 不一致：.234=$O_MD5（$(get "$O_MSG" MSG_LINES) 行） vs .1=$C_MD5（$(get "$C_MSG" MSG_LINES) 行）→ 根因仍在"
fi
if [ "$O_FLOOR" = no ] || [ "$C_FLOOR" = no ]; then
  bad "至少一边缺 current_floor（.234=$O_FLOOR / .1=$C_FLOOR）"
fi

echo "-- B. dog_task 是不是升级后重编的"
C_MTIME="$(get "$C_MSG" DOGTASK_MTIME)"; C_VER="$(get "$C_MSG" DOGTASK_VERLINE)"
C_EPOCH="$(get "$C_MSG" DOGTASK_EPOCH)"
# 旧包（会崩的那版）构建于 2026-08-04，崩溃定格在 2026-11-14；用它当阈值
BAD_EPOCH="$(date -d '2026-11-15 00:00:00' +%s 2>/dev/null || echo 1763222400)"
if [ -z "$C_MTIME" ]; then
  skip "没取到 dog_task 二进制（需要 pstree/root 权限？）"
elif [ -n "$C_EPOCH" ] && [ "$C_EPOCH" -ge "$BAD_EPOCH" ] 2>/dev/null; then
  ok "dog_task 二进制是升级后的：$C_MTIME ${C_VER:+($C_VER)}"
else
  bad "dog_task 二进制时间戳仍是旧的：$C_MTIME ${C_VER:+($C_VER)}（旧包 2026-08-04 / f3c4374）"
fi
if [ -n "$C_MTIME" ]; then
  [ -n "$C_MTIME" ] && echo "        （判据：mtime ≥ 2026-11-15，且 / 或不再是 0.7.5-linux@f3c4374）"
  [ "$(get "$C_MSG" DOGTASK_FLOOR_SYMBOL)" = yes ] \
    && ok "二进制里出现 current_floor 符号（说明是用新 msg 编的）" \
    || warn "二进制里没找到 current_floor 字符串（可能是静态类型名被裁剪，不一定是问题；以 A 的 md5 为准）"
fi

echo "-- C. dog_task 运行状态"
C_RL="$(get "$C_MSG" RL_DOGTASK_LINE)"
if [ -z "$C_RL" ]; then skip "robot-launch list 没有 dog_task 行（.1 上跑过 robot-launch 吗？）"
elif printf '%s' "$C_RL" | grep -q 'running'; then ok "dog_task running：$C_RL"
else bad "dog_task 不是 running：$C_RL"; fi

echo "-- D. 最近有没有反序列化 / alloc 崩溃"
for k in DESER_COUNT ALLOC_COUNT EXIT6_COUNT; do
  v="$(get "$C_MSG" "$k")"
  if [ -z "$v" ]; then skip "$k 取不到（日志路径 $(get "$C_MSG" DOGTASK_LOG)）"; continue; fi
  if [ "$v" = 0 ]; then ok "$k = 0"
  else warn "$k = $v（历史累计；看下面时间戳判断是否本轮）"; fi
done
C_LOGT="$(get "$C_MSG" DOGTASK_LOG_MTIME)"
[ -n "$C_LOGT" ] && echo "        dog_task.stdout 最后写入：$C_LOGT"
[ -n "$(get "$C_MSG" LOG_TAIL_NAV)" ] && echo "        最近相关行：$(get "$C_MSG" LOG_TAIL_NAV)"

echo "-- E. /dev/shm"
C_SHM="$(get "$C_MSG" SHM)"; C_SEG="$(get "$C_MSG" SHM_SEGMENTS)"
if [ -z "$C_SHM" ]; then skip "/dev/shm 取不到"
else
  PCT="${C_SHM%%|*}"
  if [ "${PCT%\%}" -lt 50 ] 2>/dev/null; then ok "/dev/shm $C_SHM（$C_SEG 个段）"
  else bad "/dev/shm $C_SHM（$C_SEG 个段）→ 先清 shm 再谈导航，见 robodog-body.md §4.5"; fi
fi

echo "-- ROS 订阅关系（best effort，仅供参考）"
echo "        .234: $(get "$O_MSG" ROS_TOPIC_INFO | cut -c1-200)"
echo "        .1  : $(get "$C_MSG" ROS_TOPIC_INFO | cut -c1-200)"

echo
echo "== 汇总：PASS=$PASS FAIL=$FAIL"
if [ "$FAIL" -gt 0 ]; then
  echo "结论：静态条件【不满足】——先别碰 kind=2。"
else
  echo "结论：静态条件【已满足】。还差一步功能验证（崩溃是概率性的）："
  echo "      用零位移 goal 发一条 5000 kind=2，然后看 .1 的 dog_task.stdout 是否刷"
  echo "      'could not deserialize ... /navigation_state'、retry_count 是否涨。"
  echo "      命令见 deploy/robodog-body.md §3 或让 nav_probe 负责。"
fi
echo
echo "原始证据：$OUT/（orin.txt / ctrl.txt）"
