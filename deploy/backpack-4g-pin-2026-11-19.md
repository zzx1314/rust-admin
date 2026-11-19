# 背包中继「平台出口钉死 4G」实施记录（2026-11-19）

> 变更目的：现场 Wi-Fi 一天掉 32 次，每次掉线都让**平台流量**回落 4G → relay 重连 7 条推流
> + 压到极省流档；Wi-Fi 回来再逐级升档。**设备客户端心跳走的是同一个平台 IP，也会一起断**
> （平台把设备标「离线」）。所以把平台流量从 Wi-Fi 上摘下来、固定在 4G，用「画质」换「稳」。
>
> 设计与取舍的完整背景：`business/docs/backpack-video-integration.md` §6.10⑤⑭⑮。
> 本文只记**这次做了什么、实测到什么、踩了哪些坑**。

## 0. 结论（TL;DR）

| 项 | 结果 |
| --- | --- |
| 平台流量出口 | `39.107.55.13 via 10.128.43.223 dev enx020c29a39b6d`（4G），**没有任何 /32 特例** |
| Wi-Fi 抖动后的影响 | 无（出口不变 → relay 不重连、不换档；心跳不受影响） |
| 4G 上行实测 | **8.5 Mbps**（8.5 MB / 8.2s WS 推流），RTT ~75 ms —— 原文档里「4G 只有 0.25~0.6M」的假设已过期 |
| 画质 | 按平台档位 `light` 起流 = **720p15 / 1000k**（只用掉 4G 的 12%），不再是 320x180 幻灯片 |
| 踩的坑 | ① `dispatcher.d` 里的 `.bak` 旧脚本**也会被执行**，一直把 /32 加回 Wi-Fi ② 4G 被判成「慢链路」一刀压到极省流档 ③ 平台下发档位会覆盖慢链路判定 |
| 回滚 | `PIN_IFACE=wlp4s0` + 注掉 `RELAY_SLOW_IFACES=`，各重启一次（§6） |

## 1. 改之前的拓扑

```bash
root@ubuntu-arm64:~# ip route show default
default via 10.128.43.223 dev enx020c29a39b6d  metric 100    # USB 4G dongle（全机默认出口）
default via 192.168.234.1 dev eth0            metric 200    # 厂商 4g-connect.sh 加的，狗体内网
default via 192.168.40.254 dev wlp4s0 proto dhcp metric 600  # 现场 Wi-Fi（一天掉 32 次）
default via 192.168.234.1 dev eth0            metric 1000    # netplan 兜底

39.107.55.13 via 192.168.40.254 dev wlp4s0                  # ← 唯一走 Wi-Fi 的特例
```

- 其它所有出网流量本来就走 4G，**只有去平台的那一条被 `50-platform-route.sh` 钉在 Wi-Fi 上**；
- 结论：所谓「固定 4G」= **取消这条特例**，不是改默认路由（默认早就是 4G）；
- 也不该反过来「给 4G 也钉一条 /32」：4G 重拨会换网关（本文写作时已经是
  `10.128.43.223`，旧文档里还是 `10.128.102.1`），钉死的 /32 会指向死网关，比走 Wi-Fi 更难查。

平台 IP 上绑的三类流量，决定了这次改动的收益面：

| 流量 | 原来掉 Wi-Fi 时 |
| --- | --- |
| 视频中继 7 条推流 WS | 出口变更 → 全部重连 + 一步压到极省流档 |
| 设备客户端心跳 / 控制 WS（`ws://39.107.55.13:3100`） | 心跳断 → 平台标「离线」 |
| AI 证据图 / 识别结果上报 | 同样断 |

## 2. 仓库改动（commit 之前的工作区）

| 文件 | 改动 |
| --- | --- |
| `deploy/backpack-relay/50-platform-route.sh` | 新增 `PIN_IFACE` 开关：`wlp4s0`（默认，钉 Wi-Fi）/ **`none`（钉死 4G：不加 /32，任何网卡上线都把残留 /32 清掉）** / `enx…`（显式钉，不建议）；加 `logger -t backpack-relay-route` |
| `deploy/backpack-relay/relay_push.py` | ① `egress_iface()` 把结果缓存到 `_last_egress_iface`；② `apply_remote_profile()` 末尾按缓存判慢链路 → `force_low_profile()`（否则平台下发的 `light/degrade` 会把「4G 该用极省流」的结论覆盖掉） |
| `deploy/backpack-relay/test_relay_push.py` | 新增 `test_platform_tier_does_not_override_a_slow_egress`（Wi-Fi 上按平台档位 / 4G 上起点直接极省流 / 平台锁档时不强压）。**36/36 通过** |
| `deploy/backpack-relay/backpack-relay.service` | 新增 `Environment=RELAY_SLOW_IFACES=`（见 §3.2），注释写清实测值与回滚方式 |
| `deploy/README.md` | 「三 → 方案 B：钉死 4G」：步骤、代价、验收、回滚、`.bak` 坑 |
| `business/docs/backpack-video-integration.md` | §6.10⑮「出口钉死 4G」+ 排障表一行 |
| `business/README.md`、`AGENTS.md` | 各补一句指向 §6.10⑮ |

回归：

```bash
python3 deploy/backpack-relay/test_relay_push.py     # 36/36
sh -n deploy/backpack-relay/50-platform-route.sh
```

## 3. 板端实际改动

### 3.1 路由 + 脚本

```bash
scp deploy/backpack-relay/50-platform-route.sh root@192.168.41.246:/tmp/50-route.new
ssh root@192.168.41.246 '
  sed -i "s/^PIN_IFACE=.*/PIN_IFACE=none/" /tmp/50-route.new &&
  chmod 755 /tmp/50-route.new &&
  mv /tmp/50-route.new /etc/NetworkManager/dispatcher.d/50-backpack-relay-route &&
  ip route del 39.107.55.13/32 2>/dev/null;
  systemctl restart backpack-relay &&
  systemctl restart robodog-device-client'
```

> `PIN_IFACE=none` 之后脚本的职责变成「一有网卡上线就把残留的 /32 清掉」——装了旧版脚本的
> 机器重连 Wi-Fi 也不会再把平台钉回 Wi-Fi。

### 3.2 撤掉「4G = 慢链路」的一刀切

`/etc/systemd/system/backpack-relay.service`：

```ini
Environment=RELAY_SLOW_IFACES=      # 空 = 不按网卡名一刀切，交给 5s 窗口自适应
```

原因：`RELAY_SLOW_IFACES` 默认是 `enx,wwan,usb`，出口一旦是 4G dongle 就 `force_low_profile()`
**一步压到极省流档 320x180@8/200k**（≈2~3fps）。那个默认值是按「4G 只有 0.25~0.6Mbps」定的
（§6.10⑭），而这次实测 8.5 Mbps —— 继续压档等于把 8M 带宽扔了。

量法（在板子上跑，走的就是平台那条路，8s 窗口）：

```bash
python3 - <<\EOF
import asyncio, json, time, websockets
async def main():
    total, chunk = 0, b"\x00" * 8192
    async with websockets.connect("ws://39.107.55.13:3100/ws/device/video", max_size=None) as ws:
        await ws.send(json.dumps({"msgType": "video_register",
            "payload": {"deviceId": "bp:__speedtest", "kind": "backpack", "stream": "__speedtest"}}))
        t = time.monotonic()
        while time.monotonic() - t < 8:
            await ws.send(chunk); total += len(chunk)
        print(f"{total*8/(time.monotonic()-t)/1000:.0f} kbps")
asyncio.run(main())
EOF
# 本次输出：4G 上行：8488 KB / 8.2s = 8460 kbps
```

判定：**≥1.5 Mbps 就值得关掉压档**（配 `light`）；只有 0.3~0.6 Mbps 才保留默认的 `enx,wwan,usb`。

### 3.3 平台侧设置（确认即可，未改）

```bash
sqlite3 /opt/hz/rust-admin/robodog/data/robodog.db \
  "select setting_key, setting_value from settings where setting_key like 'video_relay%';"
# video_relay_profile|light        ← 720p15/1000k 起流
# video_relay_auto_degrade|true    ← 链路变差时自己一档一档降
```

## 4. 踩的坑（按发生顺序）

### 坑 1：`dispatcher.d` 里的 `.bak` 旧脚本也会被执行（本次最费时间）

现象：`PIN_IFACE=none` 装好了、`ip route del` 也执行了，但路由过一会儿又变成
`via 192.168.40.254 dev wlp4s0`，relay 日志里 `出口` 一会儿 `enx…` 一会儿 `wlp4s0`。

根因：`/etc/NetworkManager/dispatcher.d/50-backpack-relay-route.bak-20261118113748`（旧版本备份）
是**可执行**的，NetworkManager 每次事件照样跑它，它在我删完 `/32` 之后又加回去
（名字排序在我之后 → 我的删除被它覆盖）。

证据：每次我的脚本打印 `已清掉平台 /32` 时，`BEFORE` 里都有 `via 192.168.40.254 dev wlp4s0`
（说明上一次删完又被人加回来了）；`mv` 走 `.bak` 之后，连采样 75s 路由稳定在 4G。

处理：

```bash
mkdir -p /root/dispatcher-backup
mv /etc/NetworkManager/dispatcher.d/*.bak* /root/dispatcher-backup/
```

> 安装这类脚本时，**旧副本必须移出 `dispatcher.d`**，不要指望 `.bak` 后缀会被忽略。

### 坑 2：光删 `/32` 不够，`route_watcher` 的旧连接不会自己搬家

已经建立的 TCP 绑在旧网卡/旧源地址上，改路由对它们无效。所以装完脚本必须
`systemctl restart backpack-relay`（这也是脚本里「路由真的变了才 `try-restart`」的原因）。
设备客户端同理：`systemctl restart robodog-device-client`，让心跳立刻改走 4G。

### 坑 3：平台档位会覆盖「慢链路」结论

钉 4G 后 `route_watcher` 一启动就把档位压到极省流，但观看者进来时 `video_start` 带着
`payload.profile={"tier":"light","auto":true}`，`apply_remote_profile()` 直接把档位改回 `light`
（0.25~0.6M 的链路上 1000k 必然卡 5~10 秒才等到拥塞窗口降档）。所以补了
`apply_remote_profile()` 末尾的 `force_low_profile()` —— **只有出口确实是慢链路时才压**，
`RELAY_SLOW_IFACES=` 清空后这段就是不生效的兜底。

## 5. 验证（本次实际结果）

```bash
# 1) 路由
ip route get 39.107.55.13
# 39.107.55.13 via 10.128.43.223 dev enx020c29a39b6d src 10.128.43.32 uid 0
ip route show 39.107.55.13/32        # 空 = 没有特例

# 2) 板端 relay
journalctl -u backpack-relay -n 40 | grep -E "出口|慢速|起点档位|已连接平台"
# 平台出口网卡：enx020c29a39b6d（每 5s 巡检，变更即重建连接）
# 7 行「已连接平台，房间 bp:xxx（出口 enx020c29a39b6d）」
# 没有「慢速链路…压到极省流档」（因为 RELAY_SLOW_IFACES= 已清空）

# 3) 设备客户端
journalctl -u robodog-device-client -n 5    # 设备注册成功 / 事件正常上报

# 4) 平台侧
journalctl -u rust-admin-robodog -n 50 | grep -E "video publisher bp:|lagged"
# 7 条 publisher attached（重连沿用房间，不踢观看端）

# 5) 4G 空闲流量 ≈ 心跳量级（34 kbps），没人看不推流
```

操作时间线（2026-11-19，CST）：

| 时间 | 动作 / 结果 |
| --- | --- |
| 09:48 | 板端原来在跑：走 Wi-Fi（`出口 wlp4s0`） |
| 09:51:49 | 首次切换：装新脚本（`PIN_IFACE=none`）、删 `/32`、重启 relay + 客户端 → 7 条流出 4G，`force_low_profile` 压到 320x180 |
| 09:53:39 | Wi-Fi `dhcp4` 重连（现场本来就一天掉 32 次）→ 我的脚本清 `/32`，但 `.bak` 又加回 |
| 09:55:13 | relay 重启后又显示 `出口 wlp4s0` → 暴露坑 1 |
| 09:57 | 移走 `.bak` + 删 `/32`，15s 采样 ×5 稳定在 4G |
| 09:59:11 | 装新 unit（`RELAY_SLOW_IFACES=`）+ 新 `relay_push.py`，重启 → 7 条流全出 4G、不再压档 |
| 09:59 | 平台确认 7 条 `publisher attached`；4G 上行实测 8.5 Mbps |

期间设备被标过两次「心跳超时离线」——都是重启客户端 / 路由抖动的那几秒，属于操作中断。

## 6. 回滚

```bash
ssh root@192.168.41.246 '
  sed -i "s/^PIN_IFACE=.*/PIN_IFACE=wlp4s0/" /etc/NetworkManager/dispatcher.d/50-backpack-relay-route
  sed -i "s/^Environment=RELAY_SLOW_IFACES=/## Environment=RELAY_SLOW_IFACES=/" /etc/systemd/system/backpack-relay.service
  ip route replace 39.107.55.13/32 via 192.168.40.254 dev wlp4s0
  systemctl daemon-reload && systemctl restart backpack-relay'
```

- 回到「Wi-Fi 优先」后，`RELAY_SLOW_IFACES` 要恢复默认（4G 只是掉线兜底，一刀压到极省流档才是对的）；
- `ssh root@192.168.41.246` 走 Wi-Fi 网段的直连路由，**不受**这条 /32 影响，切换过程中不会失联。

## 7. 遗留与后续

| # | 事项 | 影响 | 建议 |
| --- | --- | --- | --- |
| 1 | `/usr/local/bin/4g-connect.sh` 每轮 `ip route del default` 后把 eth0 默认路由按 **metric 200** 加回来 | 4G 重拨的几秒里平台流量可能被送进狗体内网（黑洞）→ 推流/心跳短暂失败 | 把该行 `metric 200` 改成 `metric 5000`（让 Wi-Fi 600 兜底） |
| 2 | `publisher_attach` 不会重发 `video_start` | 板端 WS 重连后要等前端 8s 假死看门狗断一次才起流（钉死 4G 后 Wi-Fi 抖动不再触发，只剩 4G 自己掉线） | 在 `video_register` 里若房间仍有观看者就补发一次 `video_start` |
| 3 | 板端 `MAX_BACKOFF=15s` | 4G 恢复后最坏 15s 才重连 | 出口网卡变化时把 backoff 重置为 1s，或上限收到 5s |
| 4 | 画质余量 | 4G 8.5M、平台出口也 ≥8.5M，现在只推 1000k | 可试 720p20/1800k 或 1080p25/2500k；先测板子出帧（§6.10⑧ 记录 720p15 只有 13.9fps），多路同看按 1.5Mbps/路 规划 |
