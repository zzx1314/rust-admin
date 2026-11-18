#!/bin/sh
# 平台中继走 Wi-Fi / 有线，而不是 4G dongle 或机器人网卡（2026-11-18）
#
# 现场板子的默认路由可能是 4G dongle（10.128.102.x，metric 100），实测上行只有
# ~0.5Mbps；也可能是机器人网卡 eth0（192.168.234.1，路由 metric 1000），那条是
# **狗体内网**，出口同样很慢/不稳定。而 Wi-Fi 上行 ~25Mbps。中继要推 ~1.2Mbps 视频，
# 走慢链路会一直背压：实际码率掉到 400kbps / 7fps，观看端 20s 连接超时后重连。
#
# 这里只给**平台 IP** 加一条 /32 路由，不动默认路由（避免影响其它服务）。
#
# 安装：cp 到 /etc/NetworkManager/dispatcher.d/50-backpack-relay-route && chmod 755
# 平台地址变了要同步改 PLATFORM。
IFACE="$1"
ACTION="$2"
PLATFORM="${PLATFORM:-39.107.55.13}"
UPSTREAM_IFACE="${UPSTREAM_IFACE:-wlp4s0}"

[ "$IFACE" = "$UPSTREAM_IFACE" ] || exit 0
case "$ACTION" in
  up|dhcp4-change|reapply) ;;
  *) exit 0 ;;
esac

GW=$(ip route show default dev "$UPSTREAM_IFACE" | awk '{print $3; exit}')
[ -n "$GW" ] || exit 0

BEFORE=$(ip route show "$PLATFORM/32" 2>/dev/null)
ip route replace "$PLATFORM/32" via "$GW" dev "$UPSTREAM_IFACE"
AFTER=$(ip route show "$PLATFORM/32" 2>/dev/null)

# 路由真的变了就重启 relay：已建立的 WS/TCP 还绑在旧网卡（甚至已经死掉的旧网关上），
# 不重启不会自己切过来。Wi-Fi 断开时内核会撤掉这条接口路由、流量回落到默认路由，
# relay 自己能靠发送超时发现死连接；Wi-Fi 回来后靠这里重新接到快链路上。
if [ "$BEFORE" != "$AFTER" ]; then
  systemctl try-restart backpack-relay 2>/dev/null || true
fi
exit 0
