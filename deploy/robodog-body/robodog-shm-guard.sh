#!/bin/bash
# /dev/shm 守护（RK3588 运控机）
#
# 背景：rmw_zenoh 每个 session 会占一块 transport_optimization 池（默认 48MB）。
# 狗侧节点一抖（dog_task 崩溃循环），新 session 不断产生 → /dev/shm(3.9G) 被吃满 →
# dog_task 启动 40ms 内 std::bad_alloc → SDK 通道不上线 → 平台 sdk_node 反复 false。
# 这个守护在「快满」或「dog_task 已经在崩溃循环」时自动 停栈 → 清 shm → 起栈，
# 把"永久锁死、只能人工断电"变成"几十秒自愈"。
set -u
LOG=/var/log/robodog-shm-guard.log
STAMP=/run/robodog-shm-guard.last
UNIT=robot-launch

log() { echo "$(date '+%F %T') $*" >>"$LOG"; }

# 10 分钟内只补救一次，避免自己变成循环
if [ -f "$STAMP" ] && [ $(( $(date +%s) - $(stat -c %Y "$STAMP") )) -lt 600 ]; then
  exit 0
fi

used=$(df --output=pcent /dev/shm | tail -1 | tr -dc '0-9')
pid1=$(pgrep -f '/opt/ros/humble/lib/dog_task/dog_task' | head -1)
sleep 3
pid2=$(pgrep -f '/opt/ros/humble/lib/dog_task/dog_task' | head -1)

loop=0
if [ -z "$pid2" ]; then loop=1; fi
if [ -n "$pid1" ] && [ "$pid1" != "$pid2" ]; then loop=1; fi

if [ "${used:-0}" -lt 85 ] && [ "$loop" -eq 0 ]; then
  exit 0
fi

log "remediate: shm=${used}% dog_task_loop=${loop} pid1=${pid1:-none} pid2=${pid2:-none}"
systemctl stop "$UNIT"
find /dev/shm -mindepth 1 -delete
systemctl start "$UNIT"
touch "$STAMP"
sleep 10
log "after: $(df -h /dev/shm | tail -1) dog_task=$(pgrep -f '/opt/ros/humble/lib/dog_task/dog_task' | head -1)"
