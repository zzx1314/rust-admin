#!/bin/sh
# 平台中继走 Wi-Fi / 有线，而不是 4G dongle（2026-11-18）
#
# 现场板子的默认路由可能是 4G dongle（10.128.102.x，metric 100），实测上行只有
# ~0.5Mbps；而 Wi-Fi 上行 ~25Mbps。中继要推 ~1.2Mbps 视频，走 dongle 会一直背压：
# 实际码率掉到 400kbps / 7fps，观看端 20s 连接超时后重连（现场感受就是「很卡」）。
#
# 这里只给**平台 IP** 加一条 /32 路由，不动默认路由（避免影响其它服务）。
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
[ -n "$GW" ] && ip route replace "$PLATFORM/32" via "$GW" dev "$UPSTREAM_IFACE"
exit 0
