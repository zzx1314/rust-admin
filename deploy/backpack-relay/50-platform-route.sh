#!/bin/sh
# 平台（39.107.55.13）的流量走哪张网卡（2026-11-18）
#
# 板子有三条出网路径：
#   wlp4s0  Wi-Fi 192.168.40.0/24   上行 ~25Mbps，但现场**一天掉 32 次**
#   enx…    USB 4G dongle          默认路由 metric 100，上行 0.25~0.6Mbps，稳定
#   eth0    狗体内网 192.168.234.x  metric 1000，出口又慢又飘，只做兜底
#
# 平台这个 IP 上绑的不只是视频：设备客户端心跳 / 控制、AI 证据上报也是它。所以这里的
# 选择直接决定「Wi-Fi 掉一下，设备会不会被标离线 + 中继会不会重连降档」。
#
# 两种模式（改下面 PIN_IFACE 那一行切换）：
#
#   ① PIN_IFACE=wlp4s0（默认，画质优先）：给平台加 /32 走 Wi-Fi，25Mbps 能推 720p15/1000k。
#      代价：Wi-Fi 一掉，平台流量回落 4G → relay 重连全部推流 + 一步压到极省流档，
#      Wi-Fi 回来再逐级升档；一天掉 32 次就是 32 次。设备心跳也跟着断（平台标「离线」）。
#
#   ② PIN_IFACE=none（稳定优先，2026-11-18 现场用）：**不加任何 /32**，平台流量跟着
#      默认路由走（就是 4G dongle）。出口恒定 → Wi-Fi 掉多少次都不影响推流和心跳。
#      代价：4G 上行只有 0.25~0.6Mbps，relay 会判成慢链路、一步压到极省流档
#      320x180@8/200k（约 2~3fps 的慢刷新），并按观看时长吃流量（≈90MB/小时，
#      没人看时平台会 video_stop，不烧）。
#      ⚠️ 该模式下平台「系统设置 → 视频中继 → 按上行带宽自动降档」必须开着；关掉就等于
#      锁在你选的档位（1000k）硬推，在 0.3Mbps 链路上必然卡。
#
# 也可以 PIN_IFACE=enx… 显式钉到某张网卡，但**不建议**：4G 重拨会换网关，/32 会变陈旧。
#
# 安装：cp 到 /etc/NetworkManager/dispatcher.d/50-backpack-relay-route && chmod 755
# 平台地址变了要同步改 PLATFORM。
IFACE="$1"
ACTION="$2"
PLATFORM="${PLATFORM:-39.107.55.13}"
UPSTREAM_IFACE="${UPSTREAM_IFACE:-wlp4s0}"
# ← 钉死 4G（稳定优先）就把这一行改成：PIN_IFACE=none
PIN_IFACE="${PIN_IFACE:-}"

[ -n "$PIN_IFACE" ] || PIN_IFACE="$UPSTREAM_IFACE"

case "$ACTION" in
  up|dhcp4-change|reapply) ;;
  *) exit 0 ;;
esac

if [ "$PIN_IFACE" = "none" ]; then
  # 不钉任何网卡：任何网卡上线 / 换 IP 都把平台 /32 清掉，让流量回到默认路由。
  #
  # 为什么要在别人的事件里干这件事：装了旧版脚本的机器一重连 Wi-Fi 就会把平台又钉回
  # Wi-Fi（那正是「Wi-Fi 一抖就卡 + 设备离线」的来源），所以这里要主动清、清完重启
  # relay —— 已经建立的 TCP 还绑在 Wi-Fi 的源地址上，不重启不会自己换到 4G。
  BEFORE=$(ip route show "$PLATFORM/32" 2>/dev/null)
  if [ -n "$BEFORE" ]; then
    while ip route del "$PLATFORM/32" 2>/dev/null; do :; done
    logger -t backpack-relay-route "已清掉平台 /32（PIN_IFACE=none，走默认路由/4G）：$BEFORE"
    systemctl try-restart backpack-relay 2>/dev/null || true
  fi
  exit 0
fi

[ "$IFACE" = "$PIN_IFACE" ] || exit 0

GW=$(ip route show default dev "$PIN_IFACE" | awk '{print $3; exit}')
[ -n "$GW" ] || exit 0

BEFORE=$(ip route show "$PLATFORM/32" 2>/dev/null)
ip route replace "$PLATFORM/32" via "$GW" dev "$PIN_IFACE"
AFTER=$(ip route show "$PLATFORM/32" 2>/dev/null)

# 路由真的变了才重启 relay：已建立的 WS/TCP 还绑在旧网卡（甚至已经死掉的旧网关上），
# 不重启不会自己切过来。Wi-Fi 断开时内核会撤掉这条接口路由、流量回落到默认路由，
# relay 自己能靠发送超时发现死连接；Wi-Fi 回来后靠这里重新接到快链路上。
if [ "$BEFORE" != "$AFTER" ]; then
  logger -t backpack-relay-route "平台 /32 → $AFTER"
  systemctl try-restart backpack-relay 2>/dev/null || true
fi
exit 0
