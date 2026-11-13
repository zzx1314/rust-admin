#!/bin/bash
# /dev/shm 守护（RK3588 运控机）
#
# 背景：rmw_zenoh 每个 session 会占一块 transport_optimization 池（默认 48MB）。
# 狗侧节点一抖（dog_task 崩溃循环），新 session 不断产生 → /dev/shm(3.9G) 被吃满 →
# dog_task 启动 40ms 内 std::bad_alloc → SDK 通道不上线 → 平台 sdk_node 反复 false。
# 这个守护在「快满」或「dog_task 已经在崩溃循环」时自动 停栈 → 清 shm → 起栈，
# 把"永久锁死、只能人工断电"变成"几十秒自愈"。
#
# 两个防误判：
#   1. robot-launch 启动 3 分钟内不看（那是正常启动过程，dog_task 还没起来）；
#   2. 崩溃循环要「12 秒内采样到 ≥2 个不同 pid」才算，单次没进程不算；
#   3. 10 分钟内只补救一次，避免自己变成循环。
set -u
LOG=/var/log/robodog-shm-guard.log
STAMP=/run/robodog-shm-guard.last
UNIT=robot-launch

log() { echo "$(date '+%F %T') $*" >>"$LOG"; }

if [ -f "$STAMP" ] && [ $(( $(date +%s) - $(stat -c %Y "$STAMP") )) -lt 600 ]; then
  exit 0
fi

# 1) 启动宽限期
started=$(systemctl show "$UNIT" -p ActiveEnterTimestamp --value 2>/dev/null)
start_epoch=$(date -d "$started" +%s 2>/dev/null) || exit 0
up=$(( $(date +%s) - start_epoch ))
if [ "$up" -lt 180 ]; then
  exit 0
fi

used=$(df --output=pcent /dev/shm | tail -1 | tr -dc '0-9')

# 2) 崩溃循环：12 秒采样 4 次，出现 ≥2 个不同 pid
pids=""
for _ in 1 2 3 4; do
  pids="$pids $(pgrep -f '/opt/ros/humble/lib/dog_task/dog_task' | head -1)"
  sleep 3
done
distinct=$(printf '%s\n' $pids | sort -u | grep -c .)
loop=0
if [ "${distinct:-0}" -ge 2 ]; then loop=1; fi

if [ "${used:-0}" -lt 85 ] && [ "$loop" -eq 0 ]; then
  exit 0
fi

log "remediate: shm=${used}% dog_task_restarts=${distinct} pids=$(echo $pids | tr ' ' ',')"
systemctl stop "$UNIT"
find /dev/shm -mindepth 1 -delete
systemctl start "$UNIT"
touch "$STAMP"
sleep 15
log "after: $(df -h /dev/shm | tail -1) dog_task=$(pgrep -f '/opt/ros/humble/lib/dog_task/dog_task' | head -1)"
