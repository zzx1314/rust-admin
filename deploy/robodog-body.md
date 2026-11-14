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

### 1.1 地址与账号（现场三台机器）

| 机器 | 地址 | 用户名 | 密码 | sudo | 作用 |
| --- | --- | --- | --- | --- | --- |
| 背包机 | `192.168.41.246`（也接 `192.168.234.200/24`） | `root` | 开发机已授权 key（密码请向运维要） | 无需（直接用 root） | 跑 `robodog-device-client`，**跳板机** |
| Orin NX（算法机） | `192.168.234.234` | `robot` | `1` | 同一密码（`echo 1 \| sudo -S …`） | `robot-launch` + `robot_sdk_node` + 网关 + 21 个 egg |
| RK3588（运控机） | `192.168.234.1` | `firefly` | `firefly` | 实测免密 | `robot-launch` + `dog_task` / `motion_control` / `power_daemon` 等 28 个 egg |

- 开发机 / 平台所在的机器**连不到 `192.168.234.x`**（现场内网，没有反向隧道），
  一律先从 `.246` 跳：开发机 → `.246` → `.234` / `.1`；
- `.234` / `.1` **只开密码登录**（`Permission denied (publickey,password)`），所以需要
  `SSH_ASKPASS`（见 §1.2）；
- 两台机器各有一个同名的 `robot-launch` 服务，**重启前先确认在哪台**（见 §2）。

### 1.2 怎么连（从开发机一把梭）

```bash
# ① 背包机：开发机已授权 key，直接进
ssh root@192.168.41.246

# ② 在背包机上生成两个免交互密码文件（一次即可，用完删）
printf '#!/bin/sh\nprintf "%%s\\n" "1"\n'        > /tmp/.ap234.sh && chmod 700 /tmp/.ap234.sh
printf '#!/bin/sh\nprintf "%%s\\n" "firefly"\n'  > /tmp/.ap1.sh   && chmod 700 /tmp/.ap1.sh

# ③ 算法机 .234（robot / 1）
SSH_ASKPASS=/tmp/.ap234.sh SSH_ASKPASS_REQUIRE=force setsid -w ssh -q \
  -o StrictHostKeyChecking=no -o PreferredAuthentications=password \
  -o PubkeyAuthentication=no robot@192.168.234.234 'hostname; robot-launch list'

# ④ 运控机 .1（firefly / firefly）
SSH_ASKPASS=/tmp/.ap1.sh SSH_ASKPASS_REQUIRE=force setsid -w ssh -q \
  -o StrictHostKeyChecking=no -o PreferredAuthentications=password \
  -o PubkeyAuthentication=no firefly@192.168.234.1 'hostname; robot-launch list'

# ⑤ 在 .234 上要 sudo 时（.234 的 sudo 也要密码）；把第 ③ 步的 ssh 命令原样接上即可
SSH_ASKPASS=/tmp/.ap234.sh SSH_ASKPASS_REQUIRE=force setsid -w ssh -q \
  -o PreferredAuthentications=password -o PubkeyAuthentication=no \
  robot@192.168.234.234 'echo 1 | sudo -S systemctl restart robot-launch'
# ⑥ 在 .1 上 sudo 免密：firefly@192.168.234.1 'sudo systemctl restart robot-launch'
```

其他要点：

- `.246` 上的客户端日志：`journalctl -u robodog-device-client -f`（现场已开 DEBUG），
  `本体软件急停按下中 / 已解除`、`指令 X 被拒绝`、`fault / …` 都在这里；
- 临时密码文件用完删：`rm -f /tmp/.ap234.sh /tmp/.ap1.sh`；
- ⚠️ **不要把 `.234` / `.1` 的 22 端口通过 frp / 公网映射出去**（本次排查动过这个念头，最后
  用 `.246` 跳板解决）：这两台是密码登录，暴露到公网等于开门；
- 地图 / 定位 / 任务类问题先把平台台账与 `.246` 日志看完再上机器，见 §3、§8、§10。

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
| `estop_software` 一直是 true，点「解除急停」也无效 | 先看 `sdk_node` / `body_online` | 有一个是 false → 1302 根本发不到本体 | 按本节第 1~2 行修链路 |
| 同上，但 `sdk_node=1` / `body_online=1`，`robot_mode`/`motion_status` 都是 `unknown` | `.1` `dog_task.stdout` | 当天有过 `bad_alloc` / `could not deserialize ROS message on /navigation_state` → `exit code -6` | `.1` `robot-launch restart dog_task`（不动运控，腿不掉力）；无效再按 §4.5 停栈清 `/dev/shm`。详见 §10 |
| 任务第 1 个点秒失败，报 `navigation aborted: robot reported Failed` | `task_events` 里同一秒的 `设备事件: fault` | `13446 全局规划失败，起点被障碍物占据` / `13377 无有效绕障轨迹` / `13120` `13136` 定位 | **先查起点净空与定位，不是航点**，判读与算法见 §10.8 |
| `ctrl_source=3` | 平台告警文案 | 伴随 13897/13440 → 本体残留任务；否则 → 遥控器/第三方 App | 前者：官方 App 取消任务 / 重启算法服务；后者：关遥控器电源，必要时「强制接管」 |
| 任务回 `13897 capability busy` | `.1` `dog_task` / `robot-launch list` | 本体内是否还有未结束任务 | 官方 App 取消任务；或重启本体栈（§4.5，会清掉所有本体任务） |
| 一直报 `12608 相机无图 / 13382 障碍物检测延迟` | `.234` `robot-launch list` | `perception` egg 是 `stopped`（`perception-seg` 在跑） | 需要视觉避障时再 `robot-launch start perception`，先确认模型路径 |
| `13504 内部节点异常` | 先修 `sdk_node` 链路 | 常为上下游连带 | 链路恢复后观察是否消失 |
| 平台定位徽标「未就绪」、`position=0.0, 0.0` | `.234` `robot-alg-manager.stderr` | 一直在刷 `still waiting for topic /robot_type` | 按 §8.4 有序重启 `.234`（先确认 `.1` 稳定） |
| 点「重新定位」报 `等待 4013 响应超时` | 同上 | alg-manager 未初始化，地图服务没起 | 同上（§8） |
| 任务从**第 2 个点**起全是 `13897 capability busy` | 客户端版本 | `journalctl -u robodog-device-client \| grep 13897`；日志里每个点是独立的 `5000` | 旧版客户端逐点下发且不释放能力。换 ≥2026-11-13 的客户端（分段下发 + 建任务前等 `5009 task_state=idle`），见 §7 |
| `/dev/shm` 反复 100%，清完十几分钟又满 | `.1` `df -h /dev/shm` + `sudo lsof +L1` | 大量 48MB `*.zenoh`（被 `rmw_zenohd`/各节点持有） | zenoh 池 48MB→8MB + 装 `/dev/shm` 守护，见 §7 |

---

## 6. 防复发

1. **`/dev/shm` 守护**（可选，见 §7.3）：`sudo bash /tmp/robodog-body/install-body-guard.sh`。
   守护部分有效（`/dev/shm` >85% 或 `dog_task` 崩溃循环时自动清一次）；
   zenoh `pool_size` 覆盖**实测无效**，且该补丁已于 2026-11-13 22:20 回滚。
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

### 7.3 修机器（`.1`）—— ⚠️ 2026-11-13 22:20 已回滚

> **现状：本节补丁已从 `.1` 上移除，脚本仍留在 `deploy/robodog-body/` 备查。**
>
> 回滚原因（两条，都很重要）：
> 1. **zenoh 池子的覆盖没有改变实测行为**：`.1` 上仍是 48MB 段（`pool_size` 不是段大小），
>    真正的改善来自"节点不再崩溃循环"（段数 39~78 → 2）；
> 2. 装完后发现**两台机器每 5~40 分钟一起硬复位**（`.1` boot 记录 19:52 / 20:20 / 21:02 /
>    21:39 / 21:53 / 21:55 / 22:12 / 22:19，`.234` 每次晚 17~30 秒跟着起来，
>    复位前最后几行永远是 `mpp mpi_enc_test: sendto failed`）= **电源问题**，
>    与这些脚本无关。回滚是为了把变量降到最小。
>
> 如果只是要"防 `/dev/shm` 被吃满"，`install-body-guard.sh` 仍然可用（守护部分有效）；
> 但**不要**指望它能解决"反复重启/定位丢失"。

原安装方式：

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

## 8. 地图 / 定位加载不出来（`4013` 超时、`loc_status=0`、位置 0,0）

> 2026-11-13 现场实录。表现是"机器狗拿不到位置"，平台设备抽屉里定位徽标一直是
> 「未就绪」，点「重新定位」直接报错，任务因为定位没收敛而跑不动。
> **记住：这条链的根因在 `.234`，不在平台、也不在 `.1` 的运动控制。**

> **⏱ 30 秒结论**：地图/定位起不来时，99% 是 `.234` 的 `robot-alg-manager` 卡在
> `still waiting for topic /robot_type`（它要等 `.1` 报机型，两台同时重启就会错过）。
> 处置只有一条：**先确认 `.1` 健康且没在反复重启 → `ssh robot@192.168.234.234 'sudo systemctl restart robot-launch'`
> → 等 40 秒 → 平台点「重新定位」**。

### 8.1 三个层次的症状（从上往下）

| 层 | 表现 | 关键日志 |
| --- | --- | --- |
| 平台 | `loc_status=0`（或 255）、`position=0.0, 0.0`、定位徽标「未就绪」 | 设备台账 `devices.loc_status` |
| 客户端 `.246` | 点「重新定位」后立刻失败 | `重定位失败：等待 4013 响应超时` |
| 算法机 `.234` | 地图服务没起来 | `robot-alg-manager.stderr` 一直刷 `still waiting for topic /robot_type ...` |

### 8.2 根因（启动时序竞态）

```text
.234 的 robot-alg-manager 启动脚本 alg-manager.sh
  └─ 先跑 get_robot_type.py，等 .1 发来的 /robot_type（合法值只有 XG / XGW / XGWHSPD）
       └─ 拿不到就一直等（脚本里没有超时/重试）
            └─ alg-manager 初始化不完成
                 └─ 地图列表 4013 无人应答 → 客户端「重定位：等待 4013 响应超时」
                      └─ localization 停在 IDLE（loc=0）→ 平台拿不到位置
```

**什么时候会触发**：`.1`（RK3588）和 `.234`（Orin NX）**同时掉电重启**时。`.234` 比 `.1`
先就绪，`get_robot_type.py` 抓不到值就永远卡住——之后哪怕 `.1` 恢复正常也不会补。
所以「断电重启一次」经常没用，甚至越重越糟。

> 同源现象：`.234` 的 `forward` 一直刷 `All odom data buffer are empty!`、
> `localization` 输出 `localization_state: IDLE, odom_time: 0.000s, lidar_time: 0.000s`
> —— 都是 alg 栈没初始化完的下游表现。

### 8.3 三步判定（5 分钟内确认是不是这个问题）

```bash
# ① .234：alg-manager 是不是卡在等 /robot_type（关键判据）
ssh robot@192.168.234.234 'tail -c 300 /home/robot/.robot_launch_logs/robot-alg-manager.stderr'
#   卡住 → 反复出现 "still waiting for topic /robot_type ..."

# ② .234：nav2 节点是否缺失（缺了说明 alg 栈没起来）
ssh robot@192.168.234.234 '
  source /opt/ros/humble/setup.bash; export RMW_IMPLEMENTATION=rmw_zenoh_cpp ROS_DOMAIN_ID=24
  ros2 node list | grep -E "/bt_navigator|/controller_server|/global_costmap"'   # 空 = 没起来

# ③ .234：/robot_type 有没有发布者
#    Publisher count: 0 且 Subscriber count: 2 → 确诊
ros2 topic info /robot_type
```

### 8.4 解法（顺序很重要）

```bash
# 0) 先确认 .1 是健康的、而且没在反复重启（否则修好了也会再坏）
ssh firefly@192.168.234.1 'uptime -p; journalctl --list-boots --no-pager | tail -3'
ssh firefly@192.168.234.1 'robot-launch list | grep -E "dog_task|ecal2ros|motion_control"'

# 1) 重启 .234 的算法栈（不动 .1、不动运控；狗不会动）
ssh robot@192.168.234.234 'sudo systemctl restart robot-launch'

# 2) 等 40 秒后校验三件事
ssh robot@192.168.234.234 '
  df -h /dev/shm | tail -1
  tail -c 300 /home/robot/.robot_launch_logs/robot-alg-manager.stderr   # 应停止增长
  source /opt/ros/humble/setup.bash; export RMW_IMPLEMENTATION=rmw_zenoh_cpp ROS_DOMAIN_ID=24
  ros2 node list | grep -cE "/bt_navigator|/controller_server|/collision_monitor"'

# 3) 平台设备抽屉点「重新定位」，等 loc_status=100
```

**校验通过的样子**：`robot-alg-manager.stderr` 大小不再增长（`stat -c %s` 隔 5 秒比一次）、
`ros2 node list` 里能看到 `/bt_navigator` `/controller_server` `/collision_monitor`
`/global_costmap/global_costmap`、平台 `loc_status=100` 且 `position` 出现真实坐标。

### 8.5 如果按 8.4 还不行

| 现象 | 下一步 |
| --- | --- |
| `restart` 后 alg-manager 仍在等 `/robot_type` | 说明 `.1` 侧没在发。查 `.1`：`robot-launch list | grep -E "ecal2ros|dog_task"`、`tail /home/firefly/.robot_launch_logs/ecal2ros.stdout`（有 `eCAL Pub/Sub description mismatch` / `Scouting delay elapsed` 就是跨机 zenoh/eCAL 链路问题）→ 先修 `.1` 的栈 |
| 4013 不超时了，但 `loc_status` 一直不是 100 | 地图目录不对或文件缺：`ls /ota/alg_data/map/history_map/<目录>`（必须含 `map.yaml`/`map.pcd`/`map.pgm`）；航线的 `map_path` 与本体目录名必须一致 |
| 想临时手工补一次机型 | 在 `.234` 上 `ros2 topic pub --once /robot_type std_msgs/msg/String "{data: XGWHSPD}"`。⚠️ 注意：从 `.234` 起临时 `ros2` 客户端可能报 `Unable to connect to a Zenoh router` / `publisher's context is invalid`，这时手工补发不生效，别在这上面死磕——回到 8.4 |

### 8.6 不要做的事

1. **别直接重启 `.1`**：会掉运控使能（狗会软腿），而且不解决 `.234` 的等待问题；
2. **别反复点「重新定位」**：alg 栈没起来时每次都会 4013 超时，纯等待；
3. **别把定位问题当成"地图文件丢了"**：先看 8.3 第 ① 步，90% 是 alg-manager 卡在 `/robot_type`；
4. 别用"断电重启"当常用手段——两台同时掉电正是触发这个竞态的原因。

### 8.7 预防（软件侧待做）

1. **客户端兜底**：启动/重连后若 `loc_status != 100` 且设备档案有地图，自动载图定位一次
   （带退避，失败只记事件不刷屏）——把"每次重启要人工点重新定位"变成自动恢复；
2. **平台提示**：4013 超时时把「地图服务未就绪（alg-manager 未初始化）」直接显示在设备抽屉，
   而不是只报"超时"；
3. **报厂商**：`get_robot_type.py` 应该用 `transient_local`（latched）订阅或带超时重试；
   `.1` 的 `/robot_type` 最好周期发布，别只发一次。

---

## 9. 命令速查

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

# .234 / .1 只开密码登录：在背包机上用 SSH_ASKPASS 免交互（账号/密码/完整命令见 §1.1、§1.2）
#   printf '#!/bin/sh\nprintf "%%s\\n" "<密码>"\n' > /tmp/.ap.sh && chmod 700 /tmp/.ap.sh
#   SSH_ASKPASS=/tmp/.ap.sh SSH_ASKPASS_REQUIRE=force setsid -w ssh -q \
#     -o PreferredAuthentications=password -o PubkeyAuthentication=no robot@192.168.234.234 'hostname'
#   用完 rm /tmp/.ap.sh
```

---

## 10. 第三起：急停卡死，点「解除急停」20 秒后回读仍是 `true`（2026-11-14）

> **⏱ 30 秒结论**：平台确实下发了、客户端也确实发了 `1302 cmd=1`、链路健康
> （`sdk_node=1` / `body_online=1`），但 `estop.software` 一直是 `true`、
> `robot_mode` / `motion_status` 全是 `unknown` —— **急停位锁在 `.1` 的 `dog_task` 进程里**
> （它当天崩过好几次）。处置：`.1` 上 `robot-launch restart dog_task`（不动 `motion_control`，
> 腿不会掉力）；复发就按 §4.5 停栈清 `/dev/shm` 再起。

### 10.1 症状

| 层 | 表现 |
| --- | --- |
| 平台 | 设备抽屉「本体健康检查未通过：本体软件急停已触发。遥控/任务已暂停」；台账 `estop_software=1`、`robot_mode=unknown`、`motion_status=unknown`、`ctrl_source=1`、`sdk_node_available=1`、`body_online=1`、`loc_status=100`（定位是好的） |
| 告警 | `AL-ESTOP-<设备号>-<时间戳>`（警告，未处理）：已送达设备连接，但 20 秒内本体回读没变成 `estop.software=false` |
| 事件 | 「本体动作未生效 … `1302` 没有被执行」→ 之后「摇杆被拒绝: 本体软件急停已触发」 |
| 客户端 `.246` | 先打印过「本体软件急停已解除」，卧倒后又打「本体软件急停按下中（estop.software=true）」；`1302 cmd=1` 发送成功但状态不变 |

**别被「本体健康检查未通过」误导**：平台只是照实回显 `1315 estop.software=true`，
禁用遥控/下发按钮（`web/src/components/DeviceDrawer.vue` 的 `healthIssue`），
服务端没有停任何东西。

### 10.2 先排除平台侧（3 条证据，1 分钟）

1. 服务端日志 `sent command '解除急停'` + 事件里 `wsDelivered=true` → 命令进了设备连接；
2. 客户端日志「已下发解除急停，并补零摇杆防止恢复急停前运动」→ `1302` 真的发出去了；
3. **同一条命令 20 分钟前成功过**（10:45:26 下发 → 10:45:27 回读 `estop.software=false`）
   → 链路、权限、门禁都没问题，是这次**本体不执行**。

### 10.3 排查路径（含走过的弯路）

| 步骤 | 结果 |
| --- | --- |
| `.234` `robot-launch list` | 17 个 egg 全 `running`、`retry=0`；`robot_sdk_node.stdout` 每 200ms 刷 `Battery error: error=0x0000020F bits_8_15=0x02` + `[FAULT] 2 fault(s)` |
| `.234` `sudo systemctl restart robot-launch` | 算法侧故障刷屏停了，**急停仍是 `true`** → 锁不在算法机 |
| `.1` `robot-launch list` / `df -h /dev/shm` | `dog_task` / `motion_control` / `power_daemon` 都 `running`、`retry=0`；`/dev/shm` 6% |
| `.1` `dog_task.stdout` | 当天多次崩溃：`what(): std::bad_alloc`、`could not deserialize ROS message on /navigation_state` → `exit code -6`（10:03 / 10:27 / 10:33 / 10:40 各起一次） |
| `.1` 各节点 `stderr` | `eCAL Pub/Sub description mismatch for topic hal_fault`（`dog_task`、`power_daemon` 都在刷）；`motion_control` 历史 472 条 `rknn_init error! ret = -6` |

### 10.4 根因

`dog_task` 是 `.1` 上**对外提供 zsibot SDK（8080/8081）**的进程，`1315` 的 `estop` /
`mode` / `motion_status` 都由它从运控汇总上报。它当天多次 SIGABRT 之后，内部急停位 /
状态机留在坏状态：进程还在、SDK 通道还在轮询（所以 `sdk_node=1`、1315 也不断），
但 `1302 cmd=1` 落不到状态机上 → 回读永远是 `true`，`robot_mode` / `motion_status`
一起变 `unknown`。

> **判读要点**：`sdk_node=true` 只说明 `.234` 的 `robot_sdk_node` 在心跳，
> **不代表 `.1` 的 `dog_task` 状态机健康**。§4 的结论（`sdk_node=false` → 链路断）
> 解释不了这一类，别再去反复重启 `.234`。

### 10.5 解法（从轻到重）

```bash
# ① 轻：只重启 dog_task（不动 motion_control，腿不会掉力；狗站着也能做）
ssh firefly@192.168.234.1 'robot-launch restart dog_task'
#   10~20 秒后平台应看到 estop_software=0、robot_mode=remote
#   现场实测：11:02:01 下发 → 11:02:02 客户端打「本体软件急停已解除」

# ② 重：dog_task 反复崩（bad_alloc / OS error 12）时按 §4.5
ssh firefly@192.168.234.1 '
  sudo systemctl stop robot-launch
  sudo find /dev/shm -mindepth 1 -delete     # 只清内容，别删目录
  sudo systemctl start robot-launch'
```

排查顺序记牢：**`.234` 重启只能清算法侧的故障刷屏，清不了这个急停**；
`dog_task` 才是第一个该动的。

### 10.6 验证判据

```text
平台：estop_software=0 / robot_mode=remote / sdk_node=1 / ctrl_source=1
客户端：INFO 本体软件急停已解除
```

注意 `.234` 重启过之后 `loc_status` 会掉到 `0`（地图没载），这时上报的 `position`
是无效值（本次实测 `(-0.21, -1.40)` 正好落在障碍格里）。所以还要：

1. 点「站立」→ `motion_status` 变 `stand_up/walk`；
2. 点「重新定位」→ 等 `loc_status=100`；
3. 用平台上的坐标核对**起点净空 ≥0.5 m**（算法见 §10.8），达标再下发任务。

### 10.7 复发预防（待厂商 / 待改）

1. `dog_task` 崩溃的两个根：`/navigation_state` 消息反序列化失败（跨机 ROS 消息定义不一致）
   与 `/dev/shm` 残留（`bad_alloc`）。前者要厂商统一镜像/消息定义，后者靠 §4.5 + §7 的守护。
2. `eCAL Pub/Sub description mismatch for topic hal_fault`：跨机故障话题结构不一致，
   fault 明细解不出来，平台只能看到 `2 fault(s)` 这种笼统数字。
3. `Battery error=0x0000020F`（bit0~3 + bit9）**换电池后依然常驻**，需厂商给出位定义；
   它有可能就是急停被反复置上的来源。
4. 平台文案可以更准：`AL-ESTOP-*` / 「本体动作未生效」现在只说「重启本体算法服务或整机重启」，
   建议改成「先在 `.1` 执行 `robot-launch restart dog_task`，无效再停栈清 `/dev/shm` 或整机重启」。

### 10.8 附：为什么「任务第 1 个点秒失败」多半不是航点问题

同一天还查了一起：任务在第一个航点秒失败，报

```text
航点 新航点 1 执行失败: 导航失败: [step AwaitArrival] navigation aborted: robot reported Failed
（第 1/5 点起 1 个航点 need_locate=true vx=1.20 vz=1.50 map=/ota/alg_data/map/history_map/…）
```

这句是**本体 ALG 转述**，不含原因。真正的原因在 `task_events` 里**同一秒**的
`设备事件: fault`：

| code | 文案 | 含义 |
| --- | --- | --- |
| `13446` | 全局规划失败，起点被障碍物占据（级别2） | **狗自己站的位置**在代价地图里被判成占据 → 规划器直接拒绝出轨迹 |
| `13440` | 导航指令非法（级别2） | 与 `13446` 常成对出现 |
| `13377` | 无有效绕障轨迹，搜索失败，等待中 | 规划失败的另一形态（也见于 `speed_limit.vz=0` 的历史 bug） |
| `13120` / `13136` / `13381` | 定位精度差（传感器频率下降）/ 定位丢数据 / 定位延迟 | 传感器链路降级，定位不可信 |
| `12672` / `12608` / `13382` / `13504` | 传感器无数据(无法运行) / RS 与 lidar 时间不同步 / 感知延迟 / 内部节点异常 | 上游传感器/节点问题 |
| `13897` | capability busy | 本体上还有没结束的任务（见 §5、§7.4） |

本次两次失败（11-13 21:32:35、11-14 10:02:35）后面都紧跟 `13446` + `13440`，
而航点经校验是合格的：在 free 栅格上、离最近障碍 0.50 m、距本体建图轨迹 0.39 m、
与起点连通（净空 ≥0.5 m 仍可达）。真正的触发是「狗停在离墙 0.6 m 处 + 定位精度差」，
被规划器判成起点占据。

**起点净空怎么算**（拿业务库 `data/map_ingest/*.zip` 里的地图包就够，不用连本体）：

```python
# 依赖：pip install numpy pillow
import numpy as np
from PIL import Image
from collections import deque

img = np.array(Image.open("map.pgm"))          # 0=障碍 128=未知 254/255=free
origin_x, origin_y, res = -14.85, -13.45, 0.05  # 取 map.yaml 的 origin / resolution
occ = img < 100                                 # 障碍
h, w = img.shape

# 从所有障碍格出发做多源 BFS，得到每格到最近障碍的距离（格）
dist = np.full((h, w), 1 << 30, np.int32)
dq = deque()
for r, c in zip(*np.nonzero(occ)):
    dist[r, c] = 0
    dq.append((r, c))
while dq:
    r, c = dq.popleft()
    for dr, dc in ((1, 0), (-1, 0), (0, 1), (0, -1)):
        rr, cc = r + dr, c + dc
        if 0 <= rr < h and 0 <= cc < w and dist[rr, cc] > dist[r, c] + 1:
            dist[rr, cc] = dist[r, c] + 1
            dq.append((rr, cc))

def clearance(x, y):
    c = round((x - origin_x) / res)
    r = round(h - 1 - (y - origin_y) / res)    # 图像 y 向下、地图 y 向上
    return dist[r, c] * res                    # 米

# 判据：≥0.5 m 才算安全起点（本次把 (7, 0) 调到 (7, -0.4)，净空 0.50 → 0.75 m，
#       RT-005 因此从 v0.3 升到 v0.4）
```

> 注意 trinary 语义：`map.yaml` 是 `free_thresh 0.2 / occupied_thresh 0.5`，
> 只有 `p > 204` 才算 free，`p≈128` 是**未知**。所以「在白色区」和「在灰色区」完全是两件事，
> 后者在 `track_unknown_space: true` 的代价地图上规划不出来。
