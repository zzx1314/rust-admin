#!/usr/bin/env bash
#
# 在 RK3588 运控机（192.168.234.1）上安装「本体稳定性补丁」：
#
#   1) zenoh 大消息池 48MB → 8MB（`/dev/shm` 被每个 session 吃满的根因）
#   2) `/dev/shm` 守护 timer（快满 / dog_task 已在崩溃循环 → 自动 停栈→清 shm→起栈）
#
# 用法（在 `.1` 上执行，需要 sudo）：
#   scp -r deploy/robodog-body firefly@192.168.234.1:/tmp/
#   ssh firefly@192.168.234.1 'sudo bash /tmp/robodog-body/install-body-guard.sh'
#
# 回滚：
#   sudo cp -a /opt/ros/humble/setup.bash.bak-<时间戳> /opt/ros/humble/setup.bash
#   sudo systemctl disable --now robodog-shm-guard.timer
#   sudo systemctl restart robot-launch
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

[ "$(id -u)" -eq 0 ] || {
  echo "需要 root（sudo）" >&2
  exit 1
}

echo "==> 安装 zenoh 覆盖配置"
install -m 644 "$HERE/zenoh_shm_8m.json5" /opt/app_launch/zenoh_shm_8m.json5

echo "==> 让所有 ROS 节点都带上覆盖配置（备份 setup.bash）"
if ! grep -q ZENOH_SESSION_CONFIG_URI /opt/ros/humble/setup.bash; then
  cp -a /opt/ros/humble/setup.bash "/opt/ros/humble/setup.bash.bak-$(date +%Y%m%d%H%M%S)"
  cat >>/opt/ros/humble/setup.bash <<'ENV'

# 现场补：zenoh 大消息池 48MB -> 8MB（否则每个 session 吃 48MB，把 /dev/shm 吃满）
export ZENOH_SESSION_CONFIG_URI=/opt/app_launch/zenoh_shm_8m.json5
export ZENOH_ROUTER_CONFIG_URI=/opt/app_launch/zenoh_shm_8m.json5
ENV
else
  echo "    setup.bash 已注入过，跳过"
fi

echo "==> 安装 /dev/shm 守护"
install -m 755 "$HERE/robodog-shm-guard.sh" /usr/local/bin/robodog-shm-guard.sh
install -m 644 "$HERE/robodog-shm-guard.service" /etc/systemd/system/robodog-shm-guard.service
install -m 644 "$HERE/robodog-shm-guard.timer" /etc/systemd/system/robodog-shm-guard.timer
systemctl daemon-reload
systemctl enable --now robodog-shm-guard.timer

echo "==> 重启本体栈（会掉一次运控使能，约 1 分钟）"
systemctl stop robot-launch || true
find /dev/shm -mindepth 1 -delete
systemctl start robot-launch

sleep 20
echo "==> 结果"
df -h /dev/shm | tail -1
robot-launch list 2>&1 | sed -e 's/\x1b\[[0-9;]*m//g' | grep -E 'dog_task|motion_control|zenoh_route' || true
systemctl is-active robodog-shm-guard.timer
echo "跑起来后 10 分钟内 /dev/shm 应稳定在几百 MB；超过 85% 或 dog_task 崩溃循环时守护会自动清一次。"
