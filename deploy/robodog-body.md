# 现场机器狗本体运维手册（背包 `.246` + Orin NX `.234` + RK3588 `.1`）

> 2026-11-13 事故复盘后补。`sdk_node=false`、任务下发被拒、本体状态抖动这类**本体侧**
> 问题过去只能靠猜（平台上看谁都像"算法服务没起来"），现在按本文档的顺序查，
> 十分钟内能定位到底卡在哪一层。

平台 / 服务端的排查见 [`../business/README.md`](../business/README.md) 与
[`README.md`](README.md)；本文只管**狗那边**。

---

## 1. 三台机器的角色

```text
                 ┌──────────────────────────────────────────┐
                 │ 云 / 正式环境 39.107.55.13               │
                 │ 中台 :3005 · 业务平台 + 设备网关 :3100   │
                 └───────────────▲──────────────────────────┘
                    4G 出网 ws://…:3100/ws/device
                 ┌───────────────┴──────────────────────────┐
                 │ 背包机 192.168.41.246 (aarch64)          │
                 │ robodog-device-client（我们的客户端）     │
                 │  ├→ ws://39.107.55.13:3100               │
                 │  └→ ws://192.168.234.234:10010           │
                 └───────────────▲──────────────────────────┘
                        192.168.234.0/24
        ┌────────────────────────┴─────────────────────────┐
        │ 192.168.234.234  Orin NX（算法机 ubuntu-desktop） │
        │  robot_sdk_node（1207 的 sdk_node）+ gateway(10010)│
        │  ROS2：localization / nav2 / mapping / perception-seg … │
        └────────────────────────▲─────────────────────────┘
                zsibot SDK 通道 8080/8081（非 TCP listen）
        ┌────────────────────────┴─────────────────────────┐
        │ 192.168.234.1  RK3588（运控机 hostname=firefly）  │
        │  dog_task / motion_control / spline_daemon / …    │
        │  media_server：RTSP 8554 / RTMP 1935              │
        └──────────────────────────────────────────────────┘
```

| 机器 | 登录 | 作用 | 备注 |
| --- | --- | --- | --- |
| `192.168.41.246` | `ssh root@192.168.41.246`（本机已免密） | 背包机，跑 `robodog-device-client` | 从这里能同时连到 `.234` 和 `.1` |
| `192.168.234.234` | `ssh robot@192.168.234.234` | Orin NX 16G，**算法机** | 初始密码 `1`（厂商文档 §1.1）；在 `sudo` 组 |
| `192.168.234.1` | `ssh firefly@192.168.234.1` | RK3588，**本体运控机**（"狗在这里"） | 出厂默认密码，实测 `sudo` 免密 |

`.234` / `.1` 只开了密码登录（`Permission denied (publickey,password)`），脚本里要免交互
登录时用 `SSH_ASKPASS`：

```bash
printf '#!/bin/sh\nprintf "%%s\\n" "<密码>"\n' > /tmp/.ap.sh && chmod 700 /tmp/.ap.sh
SSH_ASKPASS=/tmp/.ap.sh SSH_ASKPASS_REQUIRE=force \
  setsid -w ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no \
  robot@192.168.234.234 'hostname'
```

> 临时密码文件用完记得 `rm /tmp/.ap.sh`。

---

## 2. 本体侧的进程 / 服务树

**两台机器各有一个同名的 `robot-launch` 服务，重启前先确认在哪台机器上！**

### 2.1 Orin NX `.234`（算法机）

```text
robot-launch.service   (systemd, enabled, Restart=always, HTTP API :58787)
└─ robot-launch server (pid N)
   ├─ bash /opt/robot/nx-launch/script/launch_robot_sdk.sh → robot_sdk_node   ← 1207 的 sdk_node
   ├─ bash /opt/robot/nx-launch/script/launch_gateway.sh   → gateway         ← :10010 control/bulkdata、:12000 文件服务
   └─ bash /opt/robot/nx-launch/script/alg-manager.sh     → ros2 launch robot_alg_manager
      └─ 21 个 egg：zenoh-route / time-sync / monitor / image-decode /
         driver-gps / driver-livox / driver-realsense / driver-odin(停) /
         mapping / mapping-elevation / localization / nav2 / forward /
         perception-seg / perception(停) / app-node(停) …
```

`robot-launch` 自带 CLI 和 HTTP API（`:58787`），比 `systemctl` 更细：

```bash
robot-launch list                # 所有 egg 的 pid / status / retry_count / uptime
robot-launch restart nav2        # 只重启一个 egg
robot-launch stop|start perception
robot-launch stdout-log  <egg>   # 等价 tail /home/robot/.robot_launch_logs/<egg>.stdout
robot-launch stderr-log  <egg>
```

日志目录：`/home/robot/.robot_launch_logs/*.stdout|*.stderr`（`robot_sdk_node.*`、
`robot_gateway.*`、`localization.*` …）。

### 2.2 RK3588 `.1`（运控机）

```text
robot-launch server (pid N, 由 /usr/local/bin/robot_ap_run.sh / robot_auto_run.sh 拉起)
└─ 28 个 egg，关键几个：
   ├─ dog_task         ← 本体任务执行 + 对外 SDK 服务（zsibot 通道 8080/8081）
   ├─ motion_control   ← 运控（电机使能、步态）
   ├─ spline_daemon / imu_daemon / power_daemon / arc_platform
   ├─ media_server     ← RTSP 8554 / RTMP 1935（视频回传的拉流源）
   ├─ zenoh_route / time_sync / ecal2ros / push_image / monitor
   └─（stop 的：navigation / forward / det_inference / remote_access …）
```

日志目录：`/home/firefly/.robot_launch_logs/*.stdout|*.stderr`。

`dog_task` 日志里会打印机型（现场是 `Robot type: XGWHSPD`）。

---

## 3. 平台字段 ↔ 本体来源（排查时对号）

| 平台字段 | 本体来源 | 常见异常含义 |
| --- | --- | --- |
| `body_online` | 客户端按 1315 新鲜度算（30s 无帧即 false） | 本体整机没起来 / 算法机连不上运控机 |
| `sdk_node_available` | 1207 `modules[]` 的 `sdk_node`（`robot_sdk_node` 每 1525 心跳，网关 3s 收不到即判 false） | **`.234` 的 `robot_sdk_node` 连不上 `.1`**（见第 4 节） |
| `estop_software` | 1315 `estop.software` | 急停未解除；注意它也会"卡在 true"——因为 1302 cmd=1 要靠 SDK 通道下发 |
| `ctrl_source` | 1315 `ctrl_source`（0 未知 / 1 APP / 2 SDK / 3 其他） | 3 = 遥控器 / 第三方 App / **本体内部残留任务**，三者平台文案已区分 |
| `motion_status` | 1315 `motion_status` | `lie_down` 时摇杆不走；`unknown` 常伴随 SDK 通道断 |
| `robot_mode` | 1315 `mode` | 在 `remote ↔ unknown` 之间抖 = 1315 帧断续 |

`sdk_node=false` 时的通用判断顺序（不要一上来就重启算法服务）：

1. `.234`：`tail -f /home/robot/.robot_launch_logs/robot_sdk_node.stdout`
   —— 是否在刷 `Model/transport not ready for 192.168.234.1:8080`；
2. `.1`：`robot-launch list | grep dog_task` —— `retry_count` 是否在涨、uptime 是否只有几秒；
3. `.1`：`df -h /dev/shm` —— 是否 100%；
4. `.1`：`dog_task.stdout` 尾部 —— 是否有 `zenohc ... POSIX shm segment: OS error 12` + `bad_alloc`。

---

## 4. 事故复盘：`sdk_node` 反复 false、任务全部被拒（2026-11-13）

### 4.1 现场症状

- 18:50 / 18:56 两次下发任务 `TK-1112-11`：第一个航点前就失败，
  `5001 响应失败: code=13897 resource conflict (capability busy)`，
  事件流提示「本体上仍有未结束的任务」；
- 每次下发后约 10 秒出现 `ctrl_source 1 → 3`，平台弹「控制权不在平台」告警，20 秒后自行恢复
  （这个 `3` 不是物理遥控器，是本体内部任务/能力占用，**平台文案已按第 5 节修正**）；
- 设备抽屉提示 `本体健康检查未通过：本体 SDK 节点未就绪（sdk_node=false）`，
  遥控 / 任务全部暂停；
- 期间本体故障流：`13504 内部节点异常(级别3)`、`12672 传感器无数据`、
  `12608 RS 与 lidar 时间不同步`、`13382/13381 感知与定位延迟`，`robot_mode` 在
  `remote ↔ unknown` 之间抖；
- `.246` 客户端日志：`sdk_node` 在 true/false 之间每 10~15 秒翻一次。

### 4.2 排查路径（走过的弯路也要记）

1. 先看平台侧：服务端无异常，`AL-CTRL` / `13987` 之类都是本体上报，不是平台 bug；
2. 再怀疑"重启本体算法服务"：`.234` 的 `robot-launch` 18:54 已经自己重启过一轮，
   进程 uptime 都在，`robot_sdk_node` 也没崩 —— **说明不是它死了，而是它连不上对端**；
3. 看 `robot_sdk_node.stdout` 才定位到真正的对端：
   `target=192.168.234.1 send_port=8081 recv_port=8080`，日志一直在刷
   `Model/transport not ready` → 问题在 `.1`；
4. `.1` 上 `robot-launch list`：`dog_task` 每 6 秒重启一次（`retry_count` 持续增长），
   `dog_task.stdout` 每轮都是同一段崩溃 —— 根因找到。

### 4.3 根因

```text
.1 的 /dev/shm 被残留文件塞满（3.9G / 100%，0 可用）
  → dog_task 启动时 zenoh 建共享内存段失败：Unable to create POSIX shm segment: OS error 12 (ENOMEM)
  → 抛 RCLBadAlloc / std::bad_alloc，进程 exit -6，每 6 秒重启一次（崩溃循环）
  → 8080/8081 的 zsibot SDK 通道一直没上线
  → .234 的 robot_sdk_node 连不上，每 2s 重试，"Model/transport not ready"
  → 网关 3s 收不到 1525 心跳 → 1207 sdk_node 翻 false
  → 平台健康门禁暂停遥控/任务；1315 帧断续 → 电量=0、robot_mode 抖动、13897 能力忙
```

`/dev/shm` 塞满的直接原因是**崩溃循环自己制造的垃圾**：每次 `dog_task` 崩掉都会留下
zenoh 段文件（`.zenoh`，单个最大 48MB）和 eCAL 的 `ecal_shutdown_process_<pid>_evt`，
实测 3515 个文件里只有 1310 个被活进程占用，其余全是死进程残留。

### 4.4 证据（留档）

```text
# .1
$ df -h /dev/shm
tmpfs  3.9G  3.9G  0  100%  /dev/shm
$ ls /dev/shm | wc -l          → 3515      (358 个 *.zenoh，其余多为 ecal_shutdown_process_*)
$ sudo lsof +D /dev/shm | ...  → 1310       (仅 1310 个有人持有)
$ robot-launch list | grep dog_task
│ 3 │ 19797 │ dog_task │ running │ 1 │ 4s │      ← retry 持续增长
$ tail /home/firefly/.robot_launch_logs/dog_task.stdout
zenohc::session: Error opening session: Unable to create POSIX shm segment: OS error 12
terminate called after throwing an instance of 'rclcpp::exceptions::RCLBadAlloc'
  what():  std::bad_alloc
[ERROR] [dog_task-1]: process has died [exit code -6]
start dog task                                   ← 又一轮

# .234
$ tail /home/robot/.robot_launch_logs/robot_sdk_node.stdout
[zsibot_sdk] Vendor SDK construction: role=SDK target=192.168.234.1 send_port=8081 recv_port=8080
[zsibot_sdk] Waiting for model ready (attempts=3, timeout=1000ms)
[warning] Model/transport not ready for 192.168.234.1:8080 (attempt 1/3)
[zsibot_sdk] Disconnected → Connecting with zsibot_sdk (retry every 2s, total timeout 10s)
$ df -h /dev/shm      → 7.7G 652M 9%     （.234 的 shm 正常，别在这儿找问题）
```

### 4.5 处置步骤（在 `.1` 上执行，约 1~2 分钟）

⚠️ 前置：**狗放平稳、无人操作、不在坡道**。停机后电机会掉使能（软腿）。

```bash
sudo systemctl stop robot-launch          # 停本体栈（含运控）
sudo find /dev/shm -mindepth 1 -delete    # 只清内容，别删 /dev/shm 目录本身
sudo systemctl start robot-launch         # 起回来
```

**必须在停机状态下清**：`/dev/shm` 里还有活进程持有的段，运行中乱删会让 zenoh 会话异常；

**只 `systemctl restart` 没用**：残留段不会被自动回收，起来后 `dog_task` 照样崩。

### 4.6 修复后验证

```bash
# .1 —— shm 与模块
df -h /dev/shm                                     # 161M / 5%，百来个文件（都是活的）
robot-launch list | grep -E 'dog_task|motion_control'   # running，retry_count=0
tail /home/firefly/.robot_launch_logs/dog_task.stdout   # 不再出现 bad_alloc
# 备注：不要拿 8080/8081 是否 listen 当判据——实测链路正常时这两个端口也不在 ss 的输出里

# .234 —— SDK 链路上线（关键判据）
tail -f /home/robot/.robot_launch_logs/robot_sdk_node.stdout
#   出现 Battery / GetRechargeStatus / TaskStateInfo 的轮询日志 = 已连上
#   "Model/transport not ready" 不再新增（stderr 大小停止增长）

# .246 —— 客户端自动恢复，不用重启进程
journalctl -u robodog-device-client --since "-5 min" | grep -E 'sdk_node|急停|控制源'
#   INFO 本体软件急停已解除 / 本体 SDK 节点已就绪（sdk_node=true）
```

平台侧（业务控制台 3100 → 设备抽屉）应恢复成：
`在线待命 / body_online=1 / sdk_node=1 / estop=0 / ctrl_source=1 / robot_mode=remote / loc=100`。

本次结果对照：

| 项 | 修复前 | 修复后 |
| --- | --- | --- |
| `.1 /dev/shm` | 3.9G / **100%** / 3515 文件 | 161M / 5% / 117 文件 |
| `.1 dog_task` | 每 6s 崩一次，retry 增长 | running，retry=0 |
| `.234 robot_sdk_node` | `Model/transport not ready` 每 2s | 已连接，正常轮询本体数据 |
| `.246 客户端` | sdk_node 每 10~15s 抖 | `sdk_node=true` 稳定，急停自动解除 |
| 平台台账 | sdk=0 / estop=1 / robot_mode=unknown | sdk=1 / estop=0 / ctrl=1 / remote |

### 4.7 结论与教训

1. `sdk_node=false` 有两类：**算法机自己挂了**（看 `.234`）和**算法机连不上运控机**
   （看 `.1`）。本次是后者，重启 `.234` 无论多少次都不会好。
2. 判断"谁连谁"的最快办法是看 `robot_sdk_node.stdout` 里的 `target=…:8080`，
   不要靠猜。
3. `ctrl_source=3` 在"本体自己占着导航能力"时也会出现，别一律当成物理遥控器
   （平台告警与门禁文案已区分，见 `business/README.md`）。
4. 本体状态类告警（13504 / 12672 / 12608 / 13382）是**下游症状**，先解决
   `sdk_node` 链路，再回来看它们是否消失。

---

## 5. 一页式排障表

| 现象 | 先看哪 | 判据 | 处置 |
| --- | --- | --- | --- |
| 平台 `sdk_node=false`，遥控/任务被暂停 | `.234` `robot_sdk_node.stdout` | 在刷 `Model/transport not ready` | 转下一行 |
| 同上 | `.1` `robot-launch list` + `dog_task.stdout` | `retry_count` 涨 / `bad_alloc` | 停栈 → 清 `/dev/shm` → 起栈（§4.5） |
| 同上 | `.234` `ps -ef \| grep robot_sdk_node` | 进程真的没了 | `sudo systemctl restart robot-launch`（**在 `.234`**） |
| `body_online=false`（1315 超时） | `.234` `robot_gateway.stdout` | 网关是否在跑、`.1` 是否可达 | `nc -z 192.168.234.1 22`、看网关日志 |
| `estop_software` 一直是 true，点「解除急停」也无效 | `.234` `robot_sdk_node` 链路 | 链路断了 1302 根本发不到本体 | 先按本节第 1~2 行修链路 |
| `ctrl_source=3` | 平台告警文案 | 伴随 13897/13440 → 本体残留任务；否则 → 遥控器/第三方 App | 前者：官方 App 取消任务 / 重启算法服务；后者：关遥控器电源，必要时「强制接管」 |
| 任务回 `13897 capability busy` | `.1` `dog_task` / `robot-launch list` | 本体内是否还有未结束任务 | 官方 App 取消任务；或重启本体栈（§4.5，会清掉所有本体任务） |
| 一直报 `12608 相机无图 / 13382 障碍物检测延迟` | `.234` `robot-launch list` | `perception` egg 是 `stopped`（`perception-seg` 在跑） | 需要视觉避障时再 `robot-launch start perception`，先确认模型路径 |
| `13504 内部节点异常` | 先修 `sdk_node` 链路 | 常为上下游连带 | 链路恢复后观察是否消失 |
| 任务从**第 2 个点**起全是 `13897 capability busy` | 客户端版本 | `journalctl -u robodog-device-client \| grep 13897`；日志里每个点是独立的 `5000` | 旧版客户端逐点下发且不释放能力。换 ≥2026-11-13 的客户端（分段下发 + 建任务前等 `5009 task_state=idle`），见 §7 |
| `/dev/shm` 反复 100%，清完十几分钟又满 | `.1` `df -h /dev/shm` + `sudo lsof +L1` | 大量 48MB `*.zenoh`（被 `rmw_zenohd`/各节点持有） | zenoh 池 48MB→8MB + 装 `/dev/shm` 守护，见 §7 |

---

## 6. 防复发

1. **zenoh 池子降容 + `/dev/shm` 守护**（已脚本化，见 §7.3）：
   `sudo bash /tmp/robodog-body/install-body-guard.sh`。池子 48MB → 8MB 是根因修复，
   守护是兜底（`/dev/shm` >85% 或 `dog_task` 崩溃循环时自动清一次）。
2. **`/dev/shm` 定容 + 落 fstab**（现在是默认「50% 内存」3.9G、无 fstab 项）：
   不建议为了"扛泄漏"而调大，泄漏本身要治；需要固定值时：

   ```bash
   # /etc/fstab 追加（大小按机型内存调）
   tmpfs  /dev/shm  tmpfs  defaults,size=2G  0 0
   ```
3. **监控这两个信号**（比等平台报 `sdk_node=false` 更早）：
   - `.1` `df -h /dev/shm` 使用率 > 70%；
   - `.1` `robot-launch list` 里任一 egg 的 `retry_count` 在增长。
4. 平台侧已有：`ctrl_source=3` 告警区分「本体残留任务 / 遥控器」、急停动作 20s 回读校验、
   本体掉线时把健康字段置空（见 `business/README.md`），配合本文档一起看。

---

## 7. 第二起：`/dev/shm` 被 zenoh 池吃满（2026-11-13 晚）

### 7.1 现象

- 平台 `sdk_node=false` 与 `true` 每 5~15 秒翻一次，`estop.software` 跟着抖（刚解除又被置上）；
- 客户端日志：`指令 站立 被拒绝: 本体 SDK 节点未就绪（sdk_node=false）`、`摇杆未放行…`；
- 下发任务：**第 1 个点能建任务，第 2 个点起全是 `13897 capability busy`**；
- `.1` 上 `dog_task` 每 6 秒崩一次（`std::bad_alloc` / `OS error 12`），`retry_count` 一直涨。

### 7.2 根因（两层）

1. **zenoh 池把 `/dev/shm` 吃满**：`rmw_zenoh_cpp` 默认
   `transport.shared_memory.transport_optimization.pool_size = 48MB`（见
   `DEFAULT_RMW_ZENOH_ROUTER_CONFIG.json5:776`），**每个 session 一份**。`.1` 上有 7 个 ROS
   节点 + 视频编码，实测 39~78 个 48MB 段 ≈ 2.5~3.9G，把 3.9G 的 tmpfs 吃满；
   满 `du` 只有 52MB 而 `df` 100%，说明很多是"已删除但仍被映射"的段
   （`sudo lsof +L1` 能看到 `rmw_zenoh` 持有几十个 48MB）。
2. **本体没有释放导航能力**：`dog_task` 一崩/一重启，`5009` 的 `task_state` 与导航能力就
   对不上；旧版客户端又是**逐点下发**（每点一条 `5000`、点间不 `5002` 收尾），
   于是第 2 个点必然 `13897`。**这一条是我们这边的问题**，已改成分段下发（见 §7.4）。

### 7.3 修机器（`.1`，1 分钟停机，可一键回滚）

仓库里已备好：`deploy/robodog-body/`

```bash
# 在开发机/背包机上传
scp -r deploy/robodog-body firefly@192.168.234.1:/tmp/
# 在 .1 上安装（zenoh 池 48→8MB、注入 setup.bash、装 /dev/shm 守护、重启机器人栈）
ssh firefly@192.168.234.1 'sudo bash /tmp/robodog-body/install-body-guard.sh'
```

装了什么：

| 文件 | 作用 |
| --- | --- |
| `/opt/app_launch/zenoh_shm_8m.json5` | 覆盖 zenoh 默认配置：`transport_optimization.pool_size` 48MB → 8MB |
| `/opt/ros/humble/setup.bash`（追加两行） | `ZENOH_SESSION_CONFIG_URI` / `ZENOH_ROUTER_CONFIG_URI` 指向上面的覆盖文件（已备份 `.bak-<时间戳>`） |
| `/usr/local/bin/robodog-shm-guard.sh` + `.timer`（60s） | `/dev/shm` >85% 或 `dog_task` 在崩溃循环 → 自动 `stop → 清 shm → start`，10 分钟最多一次 |

回滚：

```bash
sudo cp -a /opt/ros/humble/setup.bash.bak-<时间戳> /opt/ros/humble/setup.bash
sudo systemctl disable --now robodog-shm-guard.timer
sudo systemctl restart robot-launch
```

验证（10 分钟）：`/dev/shm` 稳定在几百 MB、`/dev/shm` 里不再出现 48MB 文件、
`robot-launch list | grep dog_task` 的 pid 不再变。

### 7.4 修客户端（分段下发，根治 `13897`）

旧模型：**每个航点一条 `5000`**，点与点之间不释放本体导航能力 → 第 2 个点起全 busy。
新模型（`business/client/src/zhpro/route.rs`）：

- `plan_segments()` 把执行顺序切成若干段：**只在需要停点作业（`dwell>0` / 拍照 / 动作）
  的航点处断开**，纯导航点全部合并；
- 一段 = 一条 `5000 kind=2`，`params.goals` 按顺序放该段全部航点（协议 §2.5.1.4 本来就支持），
  常见 5 点纯巡检航线 = **1 个任务**，与官方 App 完全一致；
- 建任务前等 `5009 robot.task_state=idle`；失败/取消先 `5002` 再等 idle，然后才决定重试或跳过；
- 进度改用 `5009 detail.last_arrived_goal.index`（段内 1-based）驱动。

为什么官方 App 一直能跑：它就是"一条任务带全部 goals"，不存在点位之间的能力交接。

### 7.5 复盘要点

1. `sdk_node=false` 与"任务下发被拒"可能是**同一个根因**（shm 满 → `dog_task` 崩 → SDK 通道断）；
2. 本体**同一时刻只允许一个导航任务**——这是选型约束，写代码时必须按段下发；
3. `ctrl_source=3` 是任务被本体接手后的正常上报（App 也一样），别再当成"遥控器抢权"；
4. 我们自己的重试策略（逐点 + 5s×6）会把一次能力占用放大成整条航线失败。

---

## 8. 命令速查

```bash
# 从开发机 / 背包机穿透到本体（.246 是跳板）
ssh root@192.168.41.246 'hostname'
ssh root@192.168.41.246 'journalctl -u robodog-device-client -f'    # 我们的客户端日志

# Orin NX（算法机）
ssh robot@192.168.234.234
  sudo systemctl status robot-launch          # 唯一相关 unit
  robot-launch list                           # 21 个 egg
  tail -f /home/robot/.robot_launch_logs/robot_sdk_node.stdout
  ss -ltn | grep -E ':10010|:12000|:58787'    # 网关 / 文件服务 / 启动器 API
  df -h /dev/shm

# RK3588（运控机）
ssh firefly@192.168.234.1
  robot-launch list                           # 28 个 egg
  tail -f /home/firefly/.robot_launch_logs/dog_task.stdout
  df -h /dev/shm
  ss -ltn | grep -E ':8554|:1935|:58787'      # RTSP / RTMP / 启动器 API
```
