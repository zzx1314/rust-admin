# 部署手册

三套环境、两个部署目标，全部走仓库里的脚本，不手工拷文件：

| 目标 | 机器 | 装什么 | 脚本 |
| --- | --- | --- | --- |
| 正式环境 | `root@39.107.55.13` · `/opt/hz/rust-admin` | 中台（`x-rust` + 前端）+ 业务平台（`robodog-ops-server` + 控制台） | `deploy/external-hz/push.sh` |
| 现场设备 | `root@192.168.41.246` · `/opt/robodog/client` | 设备客户端 `robodog-device-client`（aarch64，systemd） | `business/client/deploy/build-aarch64.sh` + `scp` |
| 开发机 | 本机（`192.168.41.227`） | 中台 3000 / 业务平台 3100 / 前端 5198 · 5273 | `backend/run.sh`、`cargo run`、`pnpm dev` |

地址细节见 [环境与地址](#环境与地址)，正式环境的历史踩坑见
[`external-hz/README.md`](external-hz/README.md)。

---

## 环境与地址

| 环境 | 中台 | 业务平台 / 设备网关 | 备注 |
| --- | --- | --- | --- |
| **正式** | <http://39.107.55.13:3005> | <http://39.107.55.13:3100> · `ws://39.107.55.13:3100/ws/device` | 前端由 Rust 进程托管，**无 nginx、无 Redis**；3000 被本机 frps 占用所以中台用 3005 |
| **开发** | <http://127.0.0.1:3000>（前端 vite 5198 / 8848） | <http://127.0.0.1:3100>（前端 vite 5273） | `business/server/server.toml` 的 `[zentai]` 指向 `127.0.0.1:3000` |
| **现场内网** | 视部署而定，如 `ws://192.168.41.10:3000` | 设备客户端连 `ws://39.107.55.13:3100`（正式） | 机器狗本体 `192.168.234.234:10010`、文件服务 `:12000`、RTSP `192.168.234.1:8554` |

现场设备机 `192.168.41.246` 跑的是设备客户端，它再连机器狗本体：

```text
192.168.41.246 (aarch64 Ubuntu 22.04, systemd: robodog-device-client)
   ├─ ws://39.107.55.13:3100/ws/device   业务平台（正式环境外网）
   └─ ws://192.168.234.234:10010        机器狗本体（control + bulkdata）
```

设备编号不用配：客户端启动时读本体 SN 分区 `/dev/mmcblk0p5`（现场实测 `0202222672400230`）。

> 本体侧（背包 `.246` 之外的 Orin NX `.234` + RK3588 `.1`）的服务树、日志路径和排障顺序见
> [`robodog-body.md`](robodog-body.md)——`sdk_node=false` / 任务被拒 / 本体状态抖动先查那一份。

---

## 一、正式环境发布

### 一键发布

```bash
cd <repo>
./deploy/external-hz/push.sh              # 全量：前端 + 两个二进制
./deploy/external-hz/push.sh --skip-web   # 只改了 Rust，跳过 pnpm build（快）
```

做的事：`build.sh` 在本机编 `x86_64-unknown-linux-musl` 静态二进制（目标机 CentOS 7 /
glibc 2.17，本机编出来直接跑会缺符号）+ 构建两份前端 → 打 tar 包 → `scp` 到
`/tmp/` → 远程跑 `install.sh`（幂等升级）。

### 前置条件（本机一次装好）

```bash
sudo apt install musl-tools                              # musl-gcc
rustup target add x86_64-unknown-linux-musl
pnpm install --dir frontend && pnpm install --dir business/web
ssh root@39.107.55.13 'hostname'                         # 免密登录
```

### install.sh 会做什么

1. 解包到 `/opt/hz/rust-admin`，旧文件保留、配置文件备份成 `*.bak.<时间戳>`；
2. **保留** `zentai.env` / `robodog.env`（不存在才生成，两个服务共用同一个
   `SERVICE_TOKEN`）；
3. 重装两个 systemd unit 并 `restart`（会话存内存，重启后用户需重新登录）；
4. 自检：打印 `:3005`、`:3100` 的 HTTP 状态和 `/api/health`。

### 发布后校验

```bash
# 服务与日志
ssh root@39.107.55.13 'systemctl is-active rust-admin-zentai rust-admin-robodog'
ssh root@39.107.55.13 'journalctl -u rust-admin-robodog -n 30 --no-pager'

# 外网可达 + 版本内容
curl -s -o /dev/null -w '%{http_code}\n' http://39.107.55.13:3005/
curl -s http://39.107.55.13:3100/api/health | python3 -m json.tool
```

`/api/health` 里要看到（正式环境）：

```json
{"status":"ok","database":"/opt/hz/rust-admin/robodog/data/robodog.db",
 "mapIngest":{"authMode":"service-token","ready":true,"zentaiUrl":"http://127.0.0.1:3005"}}
```

业务平台日志里应出现反向控制信道已连上中台：

```text
控制信道已连上中台: ws://127.0.0.1:3005/api/dogDevice/control-plane/ws
```

### 回滚

`install.sh` 只覆盖二进制和前端，数据库与密钥都保留。回滚 = 用上一版产物重跑
`install.sh`，或 `systemctl restart` + 把 `*.bak.<时间戳>` 换回来：

```bash
ssh root@39.107.55.13 'ls -lt /opt/hz/rust-admin/zentai /opt/hz/rust-admin/robodog | head'
```

---

## 二、现场设备客户端发布（192.168.41.246）

### 1. 交叉编译（本机）

目标机没有 Rust 工具链、也没有外网，必须本机编好再传。本机 glibc 比目标机
（2.35）新，所以要按 GLIBC 版本链接，否则报 `GLIBC_2.39 not found`：

```bash
cd business/client
rustup target add aarch64-unknown-linux-gnu     # 一次性
# 推荐：按 2.35 链接（脚本检测到 cargo-zigbuild + zig 会自动使用）
./deploy/build-aarch64.sh
```

脚本结尾会校验符号上限并打印产物路径 `target/aarch64-unknown-linux-gnu/release/robodog-device-client`。
看到 `✗ 产物要求 GLIBC_2.39 > 2.35` 就是没走 zigbuild，装 `cargo-zigbuild` + `zig` 后重跑。

### 2. 上传 + 重启

```bash
BIN=business/client/target/aarch64-unknown-linux-gnu/release/robodog-device-client
STAMP=$(date +%Y%m%d-%H%M%S)

# 先备份目标机上的在跑版本
ssh root@192.168.41.246 "cp -a /opt/robodog/client/robodog-device-client \
  /opt/robodog/client/robodog-device-client.bak-$STAMP"

# 注意：直接 scp 覆盖**正在运行**的二进制会 ETXTBSY（dest open: Failure），
# 必须先传成 .new 再 mv（mv 换目录项，运行中的进程仍持有旧 inode）
scp "$BIN" root@192.168.41.246:/opt/robodog/client/robodog-device-client.new
ssh root@192.168.41.246 'chmod 755 /opt/robodog/client/robodog-device-client.new &&
  mv -f /opt/robodog/client/robodog-device-client.new /opt/robodog/client/robodog-device-client &&
  systemctl restart robodog-device-client'
```

> 也可以把整个 `business/client` 目录 rsync 到目标机再跑 `sudo ./deploy/install.sh
> --skip-build`，但目标机没有 Rust，`install.sh` 的默认编译步骤要跳过；日常发布用上面的
> scp 三步即可。

### 3. 校验

```bash
# 服务状态与启动日志
ssh root@192.168.41.246 'systemctl is-active robodog-device-client'
ssh root@192.168.41.246 'journalctl -u robodog-device-client -n 40 --no-pager'

# 客户端自带自检（端口 / health / 本体 10010+12000 / 一次注册探针）
ssh root@192.168.41.246 'cd /opt/robodog/client && ./deploy/check.sh'
```

日志里要看到这几行：

```text
启动设备客户端: device_id=0202222672400230, 业务服务=ws://39.107.55.13:3100, 本体=192.168.234.234
本体连接就绪: client_id=…, 型号=…
握手成功 / bulkdata 绑定成功
设备注册成功: 0202222672400230（园区 …）
```

再到正式环境控制台 <http://39.107.55.13:3100> → 设备管理，确认该设备「在线待命」、
心跳在刷新。

### 4. 现场配置在哪里

| 文件 | 内容 | 说明 |
| --- | --- | --- |
| `/etc/robodog/client.toml` | `[server] url`、`[robot] host`、`[video]` 等 | 首次安装由 `deploy/client.toml.template` 生成，之后**保留不覆盖** |
| `/etc/robodog/client.env` | `ROBODOG_DEVICE_SECRET`、`ROBODOG_API_PASSWORD` | `chmod 600`，systemd 用 `EnvironmentFile` 加载 |
| `/opt/robodog/client/robodog-device-client` | 二进制 | 发布时先备份成 `.bak-<时间戳>` |

核对生效配置（敏感字段已脱敏）：

```bash
ssh root@192.168.41.246 '/opt/robodog/client/robodog-device-client \
  --config /etc/robodog/client.toml --print-config'
```

### 5. 回滚

```bash
ssh root@192.168.41.246 'cd /opt/robodog/client &&
  cp -a robodog-device-client.bak-<时间戳> robodog-device-client &&
  systemctl restart robodog-device-client'
```

---

## 三、`.246` 的出网路由（踩过）

这台机器有两个网卡，两条默认路由，**机器人那条会赢**：

```bash
root@ubuntu-arm64:~# ip route show default
default via 192.168.234.1 dev eth0 proto static          # ← netplan 写的，无 metric，优先
default via 192.168.40.254 dev wlp4s0 proto dhcp metric 600
```

结果：`ip route get 39.107.55.13` 走 `eth0`（机器人网关），SYN 被吞，客户端日志停在
「正在连接业务服务: ws://39.107.55.13:3100/ws/device」，平台侧一直「离线」，
而且**不会报错**（TCP 连接一直挂着）。

修复：给机器人网卡的默认路由加 metric，让 wifi 的 DHCP 路由（600）胜出。
`/etc/netplan/01-eth0.yaml`：

```yaml
      routes:
        - to: default
          via: 192.168.234.1
          metric: 1000          # ← 加这一行；不写时 metric=0，优先级高于 wifi
```

```bash
chmod 600 /etc/netplan/01-eth0.yaml    # netplan 会告警权限过宽
netplan apply
ip route get 39.107.55.13              # 应显示 via 192.168.40.254 dev wlp4s0
systemctl restart robodog-device-client
```

机器人网段（`192.168.234.0/24`）走的是链路路由，不受影响；本体 `192.168.234.234` 仍然直连。

### 元凶：`4g-connect.service`（2026-11-13 实测）

只加 netplan metric **还不够**：这台机器上厂商的 4G 兜底脚本每 30~60 秒跑一轮，
失败后 cleanup 会执行

```bash
ip route add default via "$ETH0_GW" dev eth0      # 没有 metric → metric 0
```

于是机器人网卡又被抬成最高优先级默认路由，wifi 出网被冲掉。现象是**周期性掉线**：
客户端心跳能刷一阵，然后整机连不上 `39.107.55.13`，看门狗（90s）把设备标「离线」。

修 `/usr/local/bin/4g-connect.sh` 的 cleanup 分支：

```bash
# 原来
ip route add default via "$ETH0_GW" dev eth0 2>/dev/null
# 改成（机器人网卡只做兜底，让 wifi 的 600 赢）
ip route replace default via "$ETH0_GW" dev eth0 metric 1000 2>/dev/null
```

```bash
cp -a /usr/local/bin/4g-connect.sh{,.bak-$(date +%Y%m%d-%H%M%S)}
systemctl restart 4g-connect.service      # 让新进程读补丁版
ip route del default via 192.168.234.1 dev eth0   # 清掉当前这条 metric 0
```

> 注意：`systemctl restart` 时**旧进程**的 SIGTERM cleanup 还会用老代码加一次
> metric 0 路由，所以要「先重启服务、再删那条路由」，或者删完再看一眼。

排查命令（现场直接抄）：

```bash
ip route show default                     # 有没有不带 metric 的 eth0 默认路由
ip route get 39.107.55.13                 # 走 eth0 就是错的
journalctl -t 4g-connect --since today    # "Restored eth0 as default route"
ip monitor route                          # 抓到路由被谁改的时刻，再对照 journal
```

`4g-connect.service` 本身还会重写 `/etc/resolv.conf`；4G 模块一直注册不上
（`AT+COPS: 0` / `Failed to get IP ... after 30s`）时它就是在空转，确不需要 4G 兜底可以
`systemctl disable --now 4g-connect.service`。

## 四、正式环境与设备客户端的联动检查

发布会话结束后，按这条链路验证一遍（不改数据，只看状态）：

```bash
# 1) 正式环境健康
curl -s http://39.107.55.13:3100/api/health | python3 -m json.tool

# 2) 客户端在线（.246 本机看日志）
ssh root@192.168.41.246 'journalctl -u robodog-device-client -n 5 --no-pager'

# 3) 中台台账同步（控制台 :3005 → 设备管理，或直接看库）
ssh root@39.107.55.13 'sqlite3 /opt/hz/rust-admin/zentai/data/users.db \
  "select device_id,status,device_ip,park_code,update_time from dog_device
   where is_deleted=0 order by update_time desc limit 5;"'
```

三处对不上时，排查顺序：客户端日志「注册成功/断开重试」→ 业务平台
`journalctl -u rust-admin-robodog` 的 `sent task dispatch` / 心跳看门狗 →
中台 `dog_device` 的 `update_time`。

---

## 五、常见问题

| 现象 | 原因 / 处置 |
| --- | --- |
| 目标机 `GLIBC_2.34 not found` | 必须用 musl 静态构建（`build.sh` 已默认）；不要直接 `cargo build` |
| 客户端 `GLIBC_2.39 not found` | 用 `build-aarch64.sh`（zigbuild 按 2.35 链接），别用普通交叉编译 |
| 正式环境打不开 3000 | 中台在 **3005**，3000 被 frps 占用 |
| 业务平台报「无法校验设备分配关系」 | `zentai.env` 与 `robodog.env` 的令牌不一致，或只有地址没有令牌 |
| 客户端起来了但平台看不到 | 检查 `[server] url` 是否 `ws://39.107.55.13:3100`；`journalctl` 里找「设备注册成功」 |
| 任务下发后立刻「异常」 | 设备未绑园区 / 本体离线（`body_online=false`）/ 定位未就绪，原因在任务事件里 |
| 想在没有真机时验证全链路 | 本机 `--robot mock`（见 `business/client/README.md`），能看到真实 5000/5001/5009 报文 |
| 现场 journald 里看不到报文了 | 新版客户端把周期推送（`1315/1317/1325/1327/5009`）降到 TRACE，只保留应答类报文；看全量加 `RUST_LOG=robodog_device_client=debug,robodog_device_client::zhpro::client=trace`，或 `ROBODOG_FRAME_DUMP=/var/log/robodog-frames.jsonl` |
| 改了前端没生效（正式） | 正式环境前端由 Rust 进程托管，必须 `push.sh` 重新打包；`--skip-web` 不会更新前端 |
| 客户端「正在连接业务服务」之后没有任何日志、平台一直显示离线 | **`.246` 双默认路由**：机器人网卡 `eth0`（netplan，`via 192.168.234.1`，无 metric）抢了 wifi 的 DHCP 默认路由，出网流量被丢进机器人网关。排查 `ip route get 39.107.55.13`，修复见下节 |
| `scp` 报 `dest open ... Failure` | 目标二进制**正在运行**（ETXTBSY），传成 `.new` 再 `mv` |
| 平台提示「本体 SDK 节点未就绪（sdk_node=false）」 | 本体侧问题，不要先重启算法服务：先看 `.234` 的 `robot_sdk_node.stdout`（是否刷 `Model/transport not ready`）再看 `.1` 的 `dog_task` 与 `df -h /dev/shm`，完整链路与修法见 [`robodog-body.md`](robodog-body.md) §4 |
| 任务全部回 `13897 capability busy` | 本体上还有未结束的任务（或本体栈崩了）。先按 [`robodog-body.md`](robodog-body.md) 看 `dog_task` 是否在崩溃循环，再考虑在官方 App 取消任务 / 重启本体栈 |

---

## 六、发布检查清单

- [ ] `cargo test` / `cargo clippy --all-targets -- -D warnings`（backend、business/server、business/client）
- [ ] `cd frontend && pnpm lint`、`cd business/web && pnpm typecheck`
- [ ] 正式环境：`push.sh` → 两个 unit `active` → `:3005` 200 → `/api/health` 正常 → 控制信道日志
- [ ] 客户端：`build-aarch64.sh` 符号校验通过 → 备份旧二进制 → `systemctl restart` → 「设备注册成功」
- [ ] 控制台设备管理里该设备「在线待命」、心跳刷新
