#!/usr/bin/env bash
# 连接自检：在部署客户端的那台机器上跑（现场是 192.168.41.246，aarch64）。
#
# 依次检查：
#   1) 业务服务 TCP 端口是否可连
#   2) 业务服务 /api/health 是否 200
#   3) 机器狗本体 control/bulkdata 端口（默认 10010）是否可连
#   4) 机器狗本体文件服务（默认 12000）是否可连（地图回传要用）
#   5) 客户端能否真正注册到业务服务（跑一次短时注册探针）
#
# 用法：
#   ./deploy/check.sh                      # 用内置现场值
#   ./deploy/check.sh --no-probe           # 只查端口，不启动客户端
#   SERVER_URL=ws://10.0.0.5:3100 ROBOT_HOST=192.168.1.10 ./deploy/check.sh
#
# 退出码：0 = 业务服务链路正常；1 = 有检查项失败。
# 说明：ICMP（ping）被防火墙拦是常见现象，只作为参考信息，不计入失败。
set -uo pipefail

# 与 src/config.rs 的内置默认值保持一致
SERVER_URL="${SERVER_URL:-ws://39.107.55.13:3100}"
ROBOT_HOST="${ROBOT_HOST:-192.168.234.234}"
ROBOT_PORT="${ROBOT_PORT:-10010}"
FILE_PORT="${FILE_PORT:-12000}"
# 设备编号：默认按客户端同样的方式从本体 SN 分区算；也可用 DEVICE_ID 覆盖
DEVICE_ID="${DEVICE_ID:-}"
PROBE=1

for arg in "$@"; do
  case "$arg" in
    --no-probe) PROBE=0 ;;
    -h|--help)
      awk 'NR>1 && /^#/ {sub(/^# ?/, ""); print; next} NR>1 {exit}' "$0"
      exit 0
      ;;
    *)
      echo "未知参数: $arg（--help 查看用法）" >&2
      exit 2
      ;;
  esac
done

SERVER_HOST="${SERVER_URL#*://}"
SERVER_HOST="${SERVER_HOST%%/*}"
SERVER_PORT="${SERVER_HOST##*:}"
SERVER_HOST="${SERVER_HOST%%:*}"
[[ "$SERVER_PORT" == "$SERVER_HOST" ]] && SERVER_PORT=80
API_BASE="http://$SERVER_HOST:$SERVER_PORT"

FAILED=0
pass() { printf '  \033[32m✓\033[0m %s\n' "$1"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$1"; FAILED=$((FAILED + 1)); }
info() { printf '  \033[33m·\033[0m %s\n' "$1"; }

tcp_check() { # host port label
  if timeout 4 bash -c "cat < /dev/null > /dev/tcp/$1/$2" 2>/dev/null; then
    pass "$3 ($1:$2 可连)"
  else
    fail "$3 ($1:$2 连不上：端口未监听 / 防火墙 / 路由不通)"
  fi
}

if [[ -z "$DEVICE_ID" && -r /dev/mmcblk0p5 ]]; then
  DEVICE_ID="$(hexdump -C /dev/mmcblk0p5 2>/dev/null | sed -n '3s/.*|\(.*\)|.*/\1/p' | tr -d ' ')"
fi

echo "== 现场连接自检 =="
echo "业务服务: $SERVER_URL"
echo "设备编号: ${DEVICE_ID:-（自动读取本体 SN 分区 /dev/mmcblk0p5）}"
echo "机器狗  : $ROBOT_HOST (control/bulkdata $ROBOT_PORT, 文件服务 $FILE_PORT)"
echo

echo "[1/5] 网络可达性（ICMP 仅供参考，被拦不算失败）"
if ping -c 1 -W 1 "$ROBOT_HOST" >/dev/null 2>&1; then
  pass "ping $ROBOT_HOST 有响应"
else
  info "ping $ROBOT_HOST 无响应（可能只是禁 ICMP，继续看 TCP 端口）"
fi
if ping -c 1 -W 1 "$SERVER_HOST" >/dev/null 2>&1; then
  pass "ping $SERVER_HOST 有响应"
else
  info "ping $SERVER_HOST 无响应（本机部署时正常）"
fi

echo "[2/5] 业务服务端口"
tcp_check "$SERVER_HOST" "$SERVER_PORT" "业务服务"

echo "[3/5] 业务服务健康检查"
HEALTH="$(curl -s -m 5 -w '\n%{http_code}' "$API_BASE/api/health" 2>/dev/null)"
CODE="$(printf '%s' "$HEALTH" | tail -1)"
BODY="$(printf '%s' "$HEALTH" | sed '$d')"
if [[ "$CODE" == "200" ]]; then
  pass "$API_BASE/api/health → 200"
  printf '%s' "$BODY" | grep -q '"status":"ok"' && pass "服务自报 status=ok" || info "响应体: $BODY"
else
  fail "$API_BASE/api/health → ${CODE:-无响应}"
fi

echo "[4/5] 机器狗本体端口"
tcp_check "$ROBOT_HOST" "$ROBOT_PORT" "本体 control/bulkdata"
tcp_check "$ROBOT_HOST" "$FILE_PORT" "本体地图文件服务"

echo "[5/5] 客户端注册探针"
if [[ $PROBE -eq 1 ]]; then
  BIN=""
  for candidate in ./target/release/robodog-device-client ./target/debug/robodog-device-client \
                   /opt/robodog/client/robodog-device-client; do
    [[ -x "$candidate" ]] && BIN="$candidate" && break
  done
  if [[ -z "$BIN" ]]; then
    info "未找到客户端二进制，跳过（先 cargo build --release）"
  else
    PROBE_ARGS=(--server "$SERVER_URL" --robot sim)
    # 没算出来就让客户端自己探测（与正式启动同一条路径）
    [[ -n "$DEVICE_ID" ]] && PROBE_ARGS+=(--device-id "$DEVICE_ID")
    OUT="$(timeout 10 "$BIN" "${PROBE_ARGS[@]}" 2>&1 || true)"
    if printf '%s' "$OUT" | grep -q "设备注册成功"; then
      pass "注册成功：${DEVICE_ID:-（客户端自动读取的 SN）}（业务服务已接受注册）"
      if printf '%s' "$OUT" | grep -q "尚未绑定园区"; then
        info "该设备还没绑定园区：控制台「设备管理」→ 绑定园区，否则建图不会回传"
      fi
    else
      fail "注册失败，客户端输出："
      printf '%s\n' "$OUT" | sed 's/\x1b\[[0-9;]*m//g' | tail -5 | sed 's/^/      /'
    fi
  fi
else
  info "已用 --no-probe 跳过"
fi

echo
if [[ $FAILED -eq 0 ]]; then
  echo "结论：业务服务链路正常。"
  echo "若本体端口失败：确认本机与 $ROBOT_HOST 同网段或已有路由（ip route get $ROBOT_HOST），再查本体防火墙。"
  exit 0
fi

echo "结论：有 $FAILED 项失败，按上面 ✗ 逐项排查。"
cat <<EOF

常见原因：
  · 业务服务连不上：服务没起（cd business/server && cargo run）、只监听 127.0.0.1、
    或防火墙未放行 $SERVER_PORT（ufw allow $SERVER_PORT/tcp）
  · 本体 10010 连不上：本机到 $ROBOT_HOST 没有路由（ip route get $ROBOT_HOST）、
    本体未上电 / Wi-Fi 热点未连、端口被本体防火墙拦
  · 本体 12000 连不上：地图回传会失败，但任务执行不受影响
EOF
exit 1
