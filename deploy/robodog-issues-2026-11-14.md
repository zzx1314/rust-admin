# 现场问题复盘：任务下发失败 / 急停卡死 / dog_task 崩溃（2026-11-14，华致办公室）

> 排查对象：`0202222672400230`（X30-Pro，背包 `.246` / 算法机 `.234` / 运控机 `.1`），
> 平台 `39.107.55.13`（中台 `:3005` + 业务平台 `:3100`）。
> 本文只记录**本次实际验证过**的结论；机器地址/账号/连接方式见
> [`robodog-body.md`](robodog-body.md) §1.1、§1.2，进程树与常规修法见该文档 §2~§5。

---

## 0. 一句话结论

1. **平台侧没问题**：任务确实下发到了本体，急停命令也确实发到了本体（`wsDelivered=true`，客户端日志有「已下发解除急停」）。
2. **急停卡死的原因**：急停位锁在 `.1` 的 **`dog_task` 进程**里（它当天多次 SIGABRT）。处置是 `.1` 上
   `robot-launch restart dog_task`，20 秒内平台就回读 `estop.software=false`。
3. **`dog_task` 反复崩溃的真因**：`.234` 与 `.1` 的 **`robots_dog_msgs` 版本不一致**
   （0.9.8 vs 0.9.0，`NavigationState.msg` 少一个 `uint8 current_floor`）。
   一开导航就 `Fast CDR ... Not enough memory in the buffer stream` → `bad_alloc` → `exit code -6`。
   **与我们下发的航点/参数无关**（参数错只会得到 `13897/13440/13898` 这类业务错误码）。
4. **可用方案（现场先用这条）**：用**官方 APK** 下发这条巡检任务可以正常执行——官方 App 走的是
   `5000 kind=4`（patrol，整条路线一个任务），我们的客户端走 `5000 kind=2`（navigation），
   后者才会把 `dog_task` 拖进那条崩掉的 `/navigation_state` 反序列化路径。
   详见 §5。
5. 另外还有两个独立问题（本次一并查清）：任务首点秒失败是 **`13446` 起点被障碍物占据**（不是航点坐标错），
   以及我们客户端一个「错误分类把可跳过航点变成整条终止」的 bug（已修）。

---

## 1. 时间线（2026-11-14）

| 时间 | 事件 |
| --- | --- |
| 10:00–10:40 | 换电池 / 多次重启本体；`13504 内部节点异常(级别3)`、`12672 传感器无数据(级别2)`、`12608` 反复出现 |
| 10:02:30 | 平台下发 TK-1112-11（RT-005），10:02:35 失败：`navigation aborted: robot reported Failed` |
| 10:02:35 | 同一秒 1315 故障：**`13446` 全局规划失败，起点被障碍物占据** + `13440 导航指令非法` |
| 10:46:33 | 遥控里点「卧倒」→ 本体把 `estop.software` 置回 `true` |
| 10:48:28 | 平台下发「解除急停」，**20 s 回读仍是 `true`**（`AL-ESTOP-…104848`） |
| 10:58:38 | 重启 `.234` 的 `robot-launch`：故障刷屏停止，**急停仍是 `true`** |
| 11:01:5x | `.1` 上 `robot-launch restart dog_task` → **11:02:02 客户端打印「本体软件急停已解除」**，平台 `estop=0 / mode=remote` |
| 11:04:26–30 | 遥控站立 → `motion_status=walk`；11:05:03 重新定位 → `loc_status=100` |
| 11:09:28 | 重新下发 TK-1112-11 → 11:09:29 起 `sdk_node` / `estop` 每 5~15 秒抖一次，进度卡在 0/5 |
| 11:10:08 起 | `.1` `dog_task` 每 6~13 秒崩一次（11:00 后累计 15 次）；`/dev/shm` 从 6% 涨到 23%（814 个段 / 958 MB） |

---

## 2. 结论一：急停卡死 = `.1` 的 `dog_task` 状态机坏了

### 2.1 症状

| 层 | 表现 |
| --- | --- |
| 平台 | 设备抽屉「本体健康检查未通过：本体软件急停已触发。遥控/任务已暂停」；`estop_software=1`、`robot_mode=unknown`、`motion_status=unknown`；但 `sdk_node_available=1`、`body_online=1`、`ctrl_source=1`、`loc_status=100` |
| 告警 | `AL-ESTOP-<设备号>-<时间戳>`：已送达设备连接，20 秒内回读没变成 `false` |
| 事件 | 「本体动作未生效 … `1302` 没有被执行」→「摇杆被拒绝: 本体软件急停已触发」 |
| 客户端 `.246` | `1302 cmd=1` 发送成功（日志「已下发解除急停，并补零摇杆」），状态不变 |

### 2.2 为什么可以先排除平台

1. 服务端日志 `sent command '解除急停'` + 事件 `wsDelivered=true`；
2. 客户端日志确认 `1302 cmd=1` 已发出；
3. **同一条命令 20 分钟前成功过**（10:45:26 下发 → 10:45:27 回读 `estop.software=false`）——
   链路、权限、门禁都没问题，是这次本体不执行。

### 2.3 判定：锁在 `dog_task`

- `.234` 重启 `robot-launch` 无效 → 锁不在算法机；
- `.1` 的 `dog_task` 是**对外提供 zsibot SDK（8080/8081）**的进程，`1315` 的 `estop` / `mode` /
  `motion_status` 都由它汇总上报。它当天崩过多次（`bad_alloc` / `/navigation_state` 反序列化 → `exit code -6`），
  内部急停位留在坏状态：进程还在、SDK 通道还在轮询（所以 `sdk_node=true`、1315 不断），
  但 `1302` 落不到状态机上。
- **判读要点**：`sdk_node=true` 只说明 `.234` 的 `robot_sdk_node` 在心跳，**不代表 `.1` 的
  `dog_task` 状态机健康**。§4 那套「`sdk_node=false` → 链路断」解释不了这一类。

### 2.4 处置与验证

```bash
# 轻：只重启 dog_task（不动 motion_control，腿不会掉力）
ssh firefly@192.168.234.1 'robot-launch restart dog_task'
# 现场实测：11:02:01 下发 → 11:02:02 客户端「本体软件急停已解除」→ 平台 estop=0 / robot_mode=remote
```

**注意**：本次 11:01 只重启了 `dog_task`、**没清 `/dev/shm`**，11:10 一开导航就崩回来了。
复发时按 [`robodog-body.md`](robodog-body.md) §4.5 做完整版（停栈 → 清 `/dev/shm` → 起栈）。

---

## 3. 结论二：`dog_task` 崩溃的真因 = 两台机器的消息定义版本不一致

### 3.1 报错原文（`.1` `/home/firefly/.robot_launch_logs/dog_task.stdout`）

```text
Fast CDR exception deserializing message of type
  robots_dog_msgs::msg::dds_::NavigationState_. Not enough memory in the buffer stream
could not deserialize ROS message ... topic '/navigation_state'
eCAL Pub/Sub description mismatch for topic hal_fault
terminate called after throwing an instance of 'std::bad_alloc'
  what():  std::bad_alloc
[ERROR] [dog_task-1]: process has died [pid 13724, exit code -6]
```

### 3.2 证据：`NavigationState.msg` 两边不一样

| 机器 | `robots_dog_msgs` | commit / 时间 | `NavigationState.msg` |
| --- | --- | --- | --- |
| `.234`（发布方 `waypoint_follower`） | **0.9.8** | `e4d3e72` / 2026-09-03 15:22 | 23 行，md5 `a25fc9f17ea2b1186b78cdb4fb332857`，mtime 09-03 15:23 |
| `.1`（订阅方 `dog_task`） | **0.9.0** | `17ee12c` / 2026-06-25 16:21 | 22 行，md5 `a0b1e20aa4941b7dfd9b75504852a025`，mtime 06-25 16:22 |

两份文件 diff 只差一行——`.234` 多了个字段：

```diff
  NavigationError[] errors
  StatusItem[] status_items
- uint8 current_floor
```

`.1` 的 `dog_task` 二进制是 **2026-08-04** 用 0.9.0 的消息编出来的（`dog-task 0.7.5-linux`，
commit `f3c4374`），按 22 行布局读；`.234` 的 `waypoint_follower` 按 0.9.8 布局写（多 1 字节）→
后续字段整体错位 → 某个长度字段被读成垃圾值 → Fast CDR 认为 buffer 不够 → `bad_alloc`。

话题归属是实测的（`.234` 上 `ros2 topic info /navigation_state -v`）：

```text
Type: robots_dog_msgs/msg/NavigationState
Publisher count: 1   → Node name: waypoint_follower
Subscription count: 2 → Node name: dog_task
```

### 3.3 为什么「一下发任务就触发」

`/navigation_state` **只在导航进行中才有消息**。所以：

- 不导航时 `dog_task` 稳定（11:02~11:10 连续 8 分钟没崩）；
- 11:09:28 下发任务 → 导航开始 → 消息一上来就崩，之后每 6~13 秒一轮，`retry` 一直涨，
  `/dev/shm` 越堆越多（814 个段 / 958 MB）；
- 每一轮崩溃都会让 SDK 通道断一下 → 平台看到 `sdk_node=false`、`estop.software=true`
  每 5~15 秒翻一次（与 [`robodog-body.md`](robodog-body.md) §7.1 记录的现象完全一致）。

**崩溃是概率性的，不是必然**：11-14 11:39–11:46 用官方 App 跑巡检时，`dog_task` 同样报了
**822 次** `/navigation_state` 反序列化错误，但**一次都没崩**（`retry=0`、uptime 稳定）。
差别在报文内容——错位后读到的长度字段有时是「小垃圾」（只是报错，`rclcpp` 记一条 ERROR 继续），
有时是「大垃圾」（申请巨量内存 → `bad_alloc` → SIGABRT）。我们的 `5000 kind=2`（1 个 goal）
恰好一直在踩后者。所以这条消息不一致是**随时会炸的雷**，不能因为「这次没崩」就忽略。

### 3.4 与我们下发的参数无关

- 我们的 `5000 kind=2` 是 **WS JSON**，由 `.234` 的 `robot_sdk_node` 翻译成 SDK 调用，从不经过这条 ROS 话题；
- 参数写错只会得到业务错误码：`13897 capability busy` / `13440 导航指令非法` /
  `13898 map not found` / `13377 无有效绕障轨迹`，走 `5001` 应答或 1315 fault，**不会 SIGABRT**；
- 同类旁证：`hal_fault` 也报 `eCAL Pub/Sub description mismatch`（那条和任务参数毫无关系）；
- 本次 `5000` 是被本体受理的（拿到了 `robotTaskId c12c96d1-f28c-...`），失败发生在**导航消息处理**里。

### 3.5 怎么修（只能厂商侧做）

| 方案 | 说明 |
| --- | --- |
| ✅ `.1` 升级 `robot-launch` 包（`dog_task` 用 `robots_dog_msgs 0.9.8` 重编） | 正路。`.1` 上只有 `/opt/robot/robot_version_manager/*/version.yaml` + 预编译二进制，**没有源码**，现场无法自己重编 |
| ✅ `.234` 的 alg 栈回退到与 `.1` 匹配的 0.9.0 时代 | 可行，但会丢掉 9-3 之后的算法更新 |
| ❌ 清 `/dev/shm` / 重启 `dog_task` | 只能续命：一开导航立刻再崩（本次已验证） |
| ⚠️ 不下发导航任务 | 临时可用（只遥控），巡检跑不了；见 §5 的官方 App 方案 |

---

## 4. 结论三：任务首点秒失败是 `13446`「起点被障碍物占据」，不是航点坐标错

失败消息本身不含原因（本体 ALG 转述）：

```text
航点 新航点 1 执行失败: 导航失败: [step AwaitArrival] navigation aborted: robot reported Failed
（第 1/5 点起 1 个航点 need_locate=true vx=1.20 vz=1.50 map=/ota/alg_data/map/history_map/2026_11_10_16_15_06）
```

真正的原因在 `task_events` 里**同一秒**的 `设备事件: fault`：

| 时间 | 故障码 |
| --- | --- |
| 11-13 21:32:35 / 11-14 10:02:35 | **`13446` 全局规划失败，起点被障碍物占据（级别2）** + `13440 导航指令非法（级别2）` |
| 之前几分钟 | `13120` 定位精度差、`13136` 定位丢数据、`13381` 定位延迟、`12672` 传感器无数据、`12608`、`13382`、`13504` |

即：**被拒绝的是「狗自己站的地方」，不是目标航点**。

### 4.1 航点/地图校验结论（RT-005，地图 `2026_11_10_16_15_06`）

地图包三方一致（中台 / 业务 / 本地 ingest 的 md5 均为 `1cbcb65d68e66c5ad59ec267913796c3`），
`1088×940 @ 0.05 m/px、origin(-14.85, -13.45)`；世界范围 x∈[-14.85, 39.55]、y∈[-13.45, 33.55]。
栅格分布：free 12.9% / **unknown 86.7%** / occupied 2.3%（`map_status=部分标定`）。

| 航点 | 改前净空 | 改后 | 说明 |
| --- | --- | --- | --- |
| 新航点 1 | `(7, 0)` 0.50 m | **`(7, -0.4)` 0.75 m** | 改到本体建图时自己走过的中心线上（偏离轨迹 0.08 m），RT-005 由此 v0.3 → **v0.4** |
| 新航点 2 | `(25.7, -1.4)` 0.90 m | 不变 | |
| 新航点 3 | `(27, 19.2)` 0.95 m | 不变 | |
| 新航点 4 | `(5.8, 21.3)` 0.70 m | 不变 | |

4 个点都在 free 栅格、与起点连通（按净空 ≥0.5 m 腐蚀自由区后仍可达），距建图轨迹 0.16~0.45 m。
**结论：航点坐标本身合格，不需要再改**；`13446` 的触发是「狗停得离墙 0.6 m + 定位精度差」，
由规划器的 footprint clearing 判成起点占据。

### 4.2 判读口诀

- 同一秒有 `13446` → 先看**起点**净空与定位，不要改航点；
- 有 `13120 / 13136 / 13381` → 传感器/定位降级，先修上游（`12672` 传感器无数据、`13504` 内部节点异常）；
- 有 `13897` → 本体上有没结束的任务（能力忙），不是航点问题；
- 有 `13377 无有效绕障轨迹` → 规划不出来（历史上也因 `speed_limit.vz=0` 触发过）。

起点净空的计算脚本（读业务库 `data/map_ingest/*.zip` 里的 `map.pgm`+`map.yaml` 即可，不必连本体）
见 [`robodog-body.md`](robodog-body.md) §10.8。

---

## 5. 可用方案：**用官方 APK 执行任务可以正常跑完**

现场实测：**用本体自带/官方 APK（SmartNav）下发这条巡检路线可以正常执行**，不触发上述崩溃。
原因是我们和官方走的是两条不同的任务链路（见 [`../business/docs/official-app-parity.md`](../business/docs/official-app-parity.md)）：

| | 官方 APK | 我们的客户端（`business/client`） |
| --- | --- | --- |
| 任务类型 | `5000 kind=4`（**patrol 巡检**：`PatrolTaskWriteRequest`，一条任务覆盖整条路线） | `5000 kind=2`（**navigation 导航**：`params.goals` 分段下发） |
| 进度事件 | `5009 type=patrol`：`patrol.navigating / point_arrived / waiting / photographing / returning` | `5009 type=navigation`：`navigation.*` + `detail.last_arrived_goal` |
| 与崩掉的 `/navigation_state` 的关系 | 实测同一条路线能跑完 5 个航点；`dog_task` 同样收到该话题并报 822 次反序列化错误，但**没崩**（见 §3.3 的「概率性」说明） | 同样收到该话题，但报文内容让错位后的长度字段变成天文数字 → `bad_alloc` → 连续崩 15 次 |

即：**在 `.1` 的 `robots_dog_msgs` 升到 0.9.8 之前，现场就用官方 APK 执行巡检任务**，
我们平台的 `kind=2` 导航下发暂时停用。

### 5.1 现场操作（官方 APK）

1. 手机连本体热点/局域网，官方 App 连接狗（等 `sdk_node` 就绪，不要有别的控制源）；
2. 选对地图（本体现用 `2026_11_10_16_15_06`），先「站立」、确认无急停；
3. 用 App 里已有的巡检路径（`nav_paths/自定义路径1` 等）或新建路径，下发巡检任务；
4. 跑完看 App 的巡检记录（5010/5011）确认到点与拍照。

### 5.2 用官方 APK 时的注意事项

- **平台看不到这次任务的进度/图片**：`5009 type=patrol` 我们不消费，`task_events` 里不会出现航点进度；
- 官方 App 会把控制源占住（`ctrl_source=3`），平台侧遥控/下发都会被门禁拒绝，属于正常；
- 官方 App 的路径存在本体上（`4013` 的 `path_id`），和平台业务库里的 RT-005 是两套数据，
  改了一条不会同步到另一条；
- 跑之前仍建议核对起步位置：狗停在走廊中央（离墙 ≥0.5 m），否则同样会吃 `13446`。

### 5.3 官方那次巡检到底发了什么（2026-11-14 11:39 实测）

抓包 + 本体巡检记录（`/ota/alg_data/patrol_record/eea5ee60-….json`）还原出的事实：

| 项 | 值 |
| --- | --- |
| 任务类型 | `patrol`（`5000 kind=4`），`5001` 返 `task_id=eea5ee60-bdd9-41fa-86d7-f6ff7616a81c` |
| 任务名 | `2026_11_14_11_39_14`（App 用时间戳自动命名） |
| 地图 | `2026_11_10_16_15_06`（先发 `4022` 切地图，再 `4012/4013` 取路径列表） |
| **航点来源** | **本体侧保存的路径 `path_id="自定义路径1"`**（`nav_paths/自定义路径1` 里 A~E 5 个点），**完全没用我们平台路线里的坐标** |
| 结束动作 | `end_task = return_to_start` |
| 调度 | `schedule_type=immediate`、`periodic=false`、`execute_at=null` |
| `map_type` | **0**（properties 里记 0；我方客户端一直发 1） |
| `current_floor` | 0 |
| 记录 | 落巡检记录（5010/5011 可查），`phase` 共 13 个 |
| 执行结果 | `load_localization_map` ✅ → `capture_origin_pose` ✅ → `nav:1..nav:5` **全部 ✅** → `photo:1..photo:5` **全部 ❌** → 人工 cancel（进度 53.8%，409 s） |

官方那条路径的 5 个点（都在本体 `nav_paths/自定义路径1` 里，不是 RT-005）：

| 点 | 坐标 |
| --- | --- |
| A | `(25.300, -1.400)` |
| B | `(24.150, 10.250)` |
| C | `(12.050, 11.050)` |
| D | `(27.000, 17.450)` |
| E | `(4.550, 5.250)` |

而我们的 `5000 kind=2`（11:09:29 那条，日志原文）：

```json
{"kind":2,"name":"办公区域例行巡检 · 前往 新航点 1","schedule":0,"priority":0,"record":false,
 "execute_at":"1794625769189",
 "params":{"map_path":"/ota/alg_data/map/history_map/2026_11_10_16_15_06",
           "need_locate":true,"map_type":1,"goal_type":1,
           "goals":[{"position":{"x":7.0,"y":-0.4,"z":0.0},
                     "orientation":{"x":0.0,"y":0.0,"z":0.0,"w":1.0}}],
           "speed_limit":{"vx":1.2,"vy":0.0,"vz":1.5},
           "goal_tolerance":{"x":0.25,"y":0.25,"theta":0.2}}}
```

**差异一目了然**：官方是「一条 patrol 任务 + `path_id`，航点用本体上已保存的路径」，
我们是「一条 navigation 任务 + `goals[]` 坐标，航点用平台路线里的米制坐标」。
要平台彻底对上官方链路，就得实现 `5000 kind=4` 并把路线坐标先「写成本体上的路径」
（`4012` 之外还需要导航路径写接口），这不是改几个字段的事。

#### 官方巡检的请求形态（2026-11-14 13:25 从 ALG 日志实证）

抓包当时因 BPF 过滤器 bug 空转（见 §11.6），改从 `.234` 的
`/home/robot/.robot_launch_logs/robot-alg-manager.stdout` 拿到了 ALG 收到的任务摘要：

```text
[REQ ▶] CreateTask type=5000 msg_id=96 source=APP(1) kind=4 name=2026_11_14_13_25_15
╭─ Patrol Mission  2026-11-14 13:25:48.590
│  map_id   : 2026_11_10_16_15_06
│  path_id  : 自定义路径1  (自定义路径1)
│  end_task : 0 (none)
│  points   : 1
├─ waypoints
│  [1] 1     @(x=7.95, y=-0.25)  workpoint  actions: photo(1)
╰─
[patrol] startup undock disabled by config; continuing patrol
[patrol] navigation group arrived through point: 1
[patrol] take photo at point: 1 → photo failed (14005 capture_image service not ready)
```

由此确认（不需要再猜）：

1. `5000 kind=4` 是**路径驱动**：只给 `map_id` + `path_id`，**航点/动作来自本体上的路径文件**，
   请求里不需要逐点坐标；
2. `end_task` 是**数字枚举**：`0 = none（原地待命）`，`2 = 返回充电点`（文档 §2.5.5），`1` 推测为返回起点；
3. 路径文件格式（本体 `nav_paths/自定义路径1`）与文档 §2.2.3.7 完全一致：
   `[点名, {position, orientation, properties:{type, tasks}}]`，其中 `type=1` 作业点 / `0` 途径点，
   `tasks` = `[0 等待(2 s) / 1 拍照]`；ALG 会把它翻成 `workpoint actions: photo(1)`；
4. 到点作业失败码 `14005` 来自 payload 服务：`capture_image(/capture_image): service not ready`
   （`perception` egg 处于 stopped，与 §7 的「相机无图」一致）；
5. 巡检任务跑完会落 `/ota/alg_data/patrol_record/<task_id>.json`（`task` + `properties` + `phase[]`），
   与 5010/5011 同源。

**2026-11-14 13:36 已抓到 `5000 kind=4` 原始报文**（抓包工具修好后重跑一次）：

```json
{"kind":4,"name":"2026_11_14_13_35_50","schedule":0,
 "execute_at":"2026-11-14 13:36:29","priority":0,"record":true,
 "params":{"map_id":"2026_11_10_16_15_06","map_type":0,"end_task":1,
           "min_recharge_battery":25.0,
           "speed_limit":{"vx":0.5,"vy":0.0,"vz":1.5},
           "path_id":"自定义路径1"}}
```

要点（详细字段表见 [`../business/docs/official-app-parity.md`](../business/docs/official-app-parity.md) §三.1）：

- **请求不带任何坐标**，只有 `map_id` + `path_id`；航点/动作全在本体 `nav_paths/<path_id>` 文件里；
- `end_task` 数字枚举实测：`0`=none、`1`=return_to_start（本次记录 `properties.end_task="return_to_start"`）；
- `execute_at` 是**本地时间字符串** `"YYYY-MM-DD HH:MM:SS"`，不是毫秒；
- `record=true`、`min_recharge_battery=25.0`、`speed_limit.vx=0.5`（我们一直发 `record:false` / 无电量阈值 / `vx=1.2`）；
- `map_type=0` = 2D 栅格（patrol 枚举）；
- `5009` 进度序列：`patrol.preparing → capturing_origin → navigating → point_arrived → photographing → point_completed → completed → returning`，
  `last_event.data={"index":0,"total":1,"kind":1,"name":"1"}`（`index` 从 0 开始）；
- 本次任务 `success`、116.4 s（导航 79.5 s @vx0.5 + 返起点 31.9 s），拍照仍 `14005 service not ready`。

#### `map_type` 的 0 到底是什么意思（三套枚举别混）

协议里 `map_type` 有**三套互不相通的枚举**，同一个数字含义完全不同：

| 任务 | 字段位置 | 枚举 |
| --- | --- | --- |
| 建图 `5000 kind=1` | `params.map_type` | **0 = 室外建图 / 2 = 室内建图** |
| 导航 `5000 kind=2` | `params.map_type` + `params.goal_type` | **1 = 2D 栅格 / 2 = 3D（预留）/ 3 = Route 路网**；`goal_type` 应与 `map_type` 一致 |
| 巡检 `5000 kind=4`（APP/记录层） | 5011 `properties.map_type` | **0 = 2D 栅格 / 1 = 路网**（SDK 的 `MapType::kGrid=0` / `kRoadNetwork=1`） |

所以官方那次 patrol 的 `map_type=0` **就是 2D 栅格**，和我们 `kind=2` 里发 `1` 语义相同，
只是处在不同的枚举层。文档原文（§2.5.1.4 末尾）：

> SDK 公共接口继续使用 `MapType::kGrid=0`、`MapType::kRoadNetwork=1`；SDK 在协议编码时
> 分别转换为本节导航协议的 `1` 和 `3`。SDK 请求中的 `goalType=0` 表示跟随地图类型，
> 编码后会自动得到匹配的 `goal_type`。当前导航实现仅支持 `1`（2D 栅格）和 `3`（Route 路网）。

推论（对我们代码的约束）：

1. `kind=2` 继续发 `map_type=goal_type=1` 是对的；**不能把 0 透传到 kind=2**——kind=2 里 0 未定义
   （客户端现在把 `0` 归一化成 `1`，正是为此，别去掉这段）；
2. 将来实现 `kind=4` 时必须发 **`map_type=0`（2D 栅格）**；若误发 `1`，那是**路网**，
   会切到 road-network 规划器——而这张图的 `map.geojson` 只有 2 个孤立节点、0 条边，必然失败；
3. `kind=4` 是否也带 `goal_type`：目前抓到的 `properties` 里没有该字段，**待 P0 抓包确认**
   （SDK 层 `goalType=0` 的语义是「跟随地图类型」，编码时会自动补齐）；
4. 建图 `kind=1` 的 `map_type` 是第三个枚举（0=室外、2=室内），别和上面两个混用。

> 仍未拿到的：官方 `5000 kind=4` 的**完整 params 字段表**（`dwell_sec` / `patrolSpeedVx` /
> `minimum_battery` / `record` / `repeat` / `nav_begin_time` / `record_time` …）。
> 第一次抓包把这帧漏了（原因与修法见 §11.2），工具已修，再跑一次官方任务即可补齐。

#### 对齐官方链路：还缺什么（复盘中查到的资料边界）

把 `business/docs/zhihang-pro-api-v1.0.3.txt` 又翻了一遍，结论是**大部分信息其实已经在文档里**：

| 需要的东西 | 现状 | 出处 |
| --- | --- | --- |
| 路径**读** | ✅ 有 | `4022` 请求 `{map_id}` → `4023` 响应 `{code,msg,map_id,path_names[],paths{名: [[点名,{position,orientation,properties?}]]}}`（§2.3.3） |
| 路径**写** | ✅ 有（标注「暂不对外」） | `4024` 添加 / `4026` 删除 / `4028` 修改，响应 `4025/4027/4029` |
| 路径点语义 | ✅ 有 | §2.2.3.7：`properties.type` = 0 途径点 / 1 作业点；`tasks` = `[0 等待(固定 2 s) / 1 拍照]`，可重复（如 `[0,1,0]`） |
| 巡检进度 5009 | ✅ 有完整示例 | §2.6：`stage=patrol.point_arrived`、`last_event.data{index,total,name,kind}`、`detail.last_arrived_point{…,arrived_at_ms}`、`current_task.params{map_id,path_id,map_type,end_task}` |
| patrol 阶段名单 | ✅ 有 | §2.6.6：preparing/undocking/localizing/capturing_origin/navigating/retrying/point_arrived/waiting/photographing/point_completed/stopping/stopped/returning/returning_to_charge/recharging/completed |
| 巡检记录 5010/5011 | ✅ 有 | §2.7（含 `properties` 与 `phase[].error{code,domain,message}`） |
| 任务控制 | ✅ 有 | `5002/5004/5006` 取消/暂停/恢复 → `5003/5005/5007`；`5012/5013` 巡检返回起点 |
| `end_task` 语义 | ⚠️ 部分 | 文档只明确 `end_task=2` = 跑完返充；`0/1` 的数值枚举在 kind=4 章节里 |
| **`5000 kind=4` 的 params 字段表** | ❌ **缺** | 正文只写「kind=4 patrol（暂不对外开放）」，没有字段表 |

关键线索：§2.2.3.7 里写着「与 `5000` 巡检任务内联点位（**§4.1.7** 的 `goals[].actions[].type`）
取值口径一致」——**但我们手上的 txt 里没有 §4.1.7**。说明这份协议文档是**删节版/旧版**，
厂商侧存在描述 kind=4 内联点位的章节。所以补齐顺序是：

1. **向厂商要完整版协议文档**（含 §4.1.7 与 `kind=4` 字段表）——最省事；
2. 要不到，再抓一次包（3~5 分钟，工具已就绪）：`5000 kind=4` 那一帧 + `--show-heartbeat` 的 5009。

### 5.4 平台侧要真接上，需要做的（下一步）

1. 抓一次官方 App 的 WS 报文，把 `kind=4` 的 `params` 还原——工具已备好：`business/tools/wsniff.py`
   （纯标准库，`live` 抓包 / `pcap` 离线 / `compare` 参数比对），用法见本文 §11；
2. 客户端实现 `5000 kind=4` 巡检任务（一条任务覆盖整条路线 + `path_id`），并按 `patrol.*` 驱动进度；
3. 在那之前，平台下发一律走「已发布地图 + `kind=2`」会踩到本崩溃，**不要在现场使用**。

---

## 6. 结论四：客户端一个 bug（已修）

`route.rs` 把排障用的诊断尾巴（含 `need_locate=…`）拼进了 `Err`，而 `is_locate_error()`
恰好用 `"need_locate"` 当判据 → **任何航段失败都被判成「定位错误」**：

- 可跳过的航点（`onFail=跳过继续`）不再跳过，一条瞬态失败就终止整条航线；
- 证据：每次失败的事件都是 `0/5（100%）`、`skipped=0`。

已改为「诊断尾巴由 `run()` 拼在展示消息里，`run_segment()` 只回本体原话」，并加了回归断言。
详见 `business/client/README.md`（分段下发流程那一段）。

---

## 7. 其他现场背景（非直接原因，但会放大问题）

| 项 | 现象 | 影响 |
| --- | --- | --- |
| `.1` `motion_control` | 历史 472 条 `rknn_init error! ret = -6`（NPU 初始化失败） | 运控侧不稳定，建议一并反馈厂商 |
| `.1` `hal_fault` | `eCAL Pub/Sub description mismatch`（`dog_task`、`power_daemon` 都在报） | 故障明细解不出来，平台只看到 `2 fault(s)` 这类笼统数字 |
| `.1` 电池 | `Battery error: error=0x0000020F bits_8_15=0x02` **换电池后仍常驻**；电量 95%→75% 用了 26 分钟 | 需厂商给位定义；也可能把急停反复置回 |
| 地图 | `部分标定`、86.7% 为 unknown | 定位一旦飘到灰色区，`13446` 概率变高；建议重扫或补标定 |
| `.234` | `13504 内部节点异常(级别3)`、`12608 RS 与 lidar 时间不同步`、`perception status, no camera image (lidar only)` | 传感器链路降级 → `13120/13136` → 定位不可信 |

---

## 8. 待办（按优先级）

1. **【厂商】** 提供与 `robots_dog_msgs 0.9.8` 匹配的 `.1` `robot-launch` / `dog_task` 升级包
   （或给出 `.234` 的兼容降级包）——这是本崩溃的根治项；
2. **【厂商】** 说明 `Battery error 0x020F` 位定义、`rknn_init error -6`、`hal_fault` 描述不一致的原因；
3. **【我们】** 抓官方 App 的 `kind=4` 报文，客户端实现巡检任务对齐（`official-app-parity.md` §三.1）；
4. **【我们】** 平台加「下发前守卫」：起点净空 <0.5 m、电量低、近 5 分钟有 `12672/13120/13136`、
   `dog_task` 崩溃循环时直接拒发并给中文原因；
5. **【我们】** `need_locate` 决策改用本体 `active_map` 比对，避免无谓重载地图（会重置位姿）。

---

## 9. 证据采集命令（下次直接抄）

```bash
# .246 客户端（现场已开 DEBUG）
journalctl -u robodog-device-client --since "-10 min" | grep -aE "急停|sdk_node|fault|task_progress"

# .234 算法机
robot-launch list
tail -60 /home/robot/.robot_launch_logs/robot_sdk_node.stdout     # Battery error / [FAULT] / Model not ready
grep -a "still waiting for topic /robot_type" /home/robot/.robot_launch_logs/robot-alg-manager.stderr

# .1 运控机
robot-launch list                                                # 看 retry / uptime
tail -40 /home/firefly/.robot_launch_logs/dog_task.stdout        # 崩溃原因（CDR/bad_alloc/deserialize）
df -h /dev/shm; sudo ls /dev/shm | wc -l                         # 段数量暴涨 = 崩溃残留/zenoh 池

# 平台侧（业务库）
sqlite3 /opt/hz/rust-admin/robodog/data/robodog.db \
  "select battery,estop_software,robot_mode,motion_status,ctrl_source,loc_status,position,updated_at
     from devices where id=15;"
sqlite3 /opt/hz/rust-admin/robodog/data/robodog.db \
  "select created_at,title,description from task_events where device_id=15 order by id desc limit 30;"

# 消息版本比对（关键判据）
md5sum /opt/ros/humble/share/robots_dog_msgs/msg/NavigationState.msg     # 两台机器各跑一次
cat /opt/robot/robot_version_manager/robots_dog_msgs/version.yaml
```

---

## 10. 给厂商的问题单（可直接复制）

> **标题**：`.234` / `.1` 的 `robots_dog_msgs` 版本不一致，导航时 `dog_task` 必崩（SIGABRT）
>
> **环境**：设备 `0202222672400230`（X30-Pro）。`.234` = Orin NX 算法机；`.1` = RK3588 运控机。
>
> **现象**：只要开始导航（`5000 kind=2`），`.1` 的 `dog_task` 每 6~13 秒崩溃重启一次；
> 平台侧表现为 `sdk_node` 与 `estop.software` 每 5~15 秒抖一次、任务进度停在 0/N。
> 用官方 App（`5000 kind=4` 巡检）执行同一路线**不崩**。
>
> **日志**：
> ```
> Fast CDR exception deserializing message of type robots_dog_msgs::msg::dds_::NavigationState_.
>   Not enough memory in the buffer stream
> eCAL Pub/Sub description mismatch for topic hal_fault
> terminate called after throwing an instance of 'std::bad_alloc'
> [ERROR] [dog_task-1]: process has died [pid 13724, exit code -6]
> ```
>
> **证据**：
> | | `.234` | `.1` |
> | --- | --- | --- |
> | `robots_dog_msgs` | **0.9.8**（`e4d3e72`，2026-09-03 15:22） | **0.9.0**（`17ee12c`，2026-06-25 16:21） |
> | `NavigationState.msg` | 23 行，含 `uint8 current_floor`，md5 `a25fc9f1…` | 22 行，无该字段，md5 `a0b1e20a…` |
> | 相关二进制 | `waypoint_follower`（发布方） | `dog-task 0.7.5-linux`（`f3c4374`，二进制构建于 2026-08-04） |
>
> **请求**：提供与 `robots_dog_msgs 0.9.8` 匹配的 `.1` `robot-launch` / `dog_task` 升级包；
> 或说明 `.234` 应回退到哪个 alg 版本才能与 `.1` 的 0.9.0 兼容。
>
> **附带问题**：
> 1. `Battery error: error=0x0000020F bits_8_15=0x02` 常驻（已更换电池），请给出位定义；
> 2. `motion_control` 持续打印 `rknn_init error! ret = -6`；
> 3. 两机均报 `eCAL Pub/Sub description mismatch for topic hal_fault`，fault 明细取不到。

---

## 11. 附录：抓官方 App 的报文，和我们的 5000 参数逐字段比对

要确认「官方 App 到底发了什么」，就得抓手机 ↔ 网关（`.234:10010`）的 WebSocket 报文。
现场这两台机器**没有 tcpdump / tshark**，所以工具用 Python 标准库自己实现：
[`../business/tools/wsniff.py`](../business/tools/wsniff.py)（`AF_PACKET` 原始套接字 + TCP 重组 +
RFC6455 帧解析，含客户端 mask；也能离线读 `.pcap`）。

### 11.1 三种用法

```bash
# ① 在网关所在机器（.234）实时抓，root 才能开原始套接字
sudo python3 wsniff.py live --seconds 1800 --ports 10010 --out /tmp/ws-official.jsonl
#   期间用官方 APK 走一遍任务；Ctrl-C 或到时间自动收尾

# 只看某个对端（例如手机 IP），并把心跳过滤关掉以便看摇杆
sudo python3 wsniff.py live --host 192.168.234.8 --show-heartbeat

# ② 有 pcap（Wireshark / tcpdump 抓的）时离线解析
python3 wsniff.py pcap --in cap.pcap --ports 10010 --out ws.jsonl

# ③ 官方抓包 vs 我方客户端日志/JSON 的 5000 参数比对
python3 wsniff.py compare --official /tmp/ws-official.jsonl --client client-5000.log
```

默认会过滤高频心跳（`1300/1315/1317/1325/1327/1401/5009/1207/1304`），只打印并落盘
建任务/地图/定位/应答这类报文；需要看全量时加 `--show-heartbeat` 或 `--full`。

### 11.2 现场本次怎么跑（2026-11-14 11:28 起）

```bash
# 在 .234 上后台起（30 分钟窗口），输出两个文件：
#   /tmp/ws-official.jsonl  每条报文一行 JSON
#   /tmp/ws-official.log    控制台摘要（含「新连接 …」能看到手机 IP）
ssh robot@192.168.234.234 'sudo -n nohup python3 -u /tmp/wsniff.py live \
  --seconds 1800 --ports 10010 --out /tmp/ws-official.jsonl > /tmp/ws-official.log 2>&1 &'
```

抓到的新连接（现场实测）：

| 对端 | 身份 |
| --- | --- |
| `192.168.234.200` | 背包机 `.246`（我们自己的 `robodog-device-client`） |
| `192.168.234.8` | 疑似手机 / 官方 App 那台（待确认） |

> **第一次抓包的教训（2026-11-14 11:32–11:48）**：抓到了 396 条报文，但**关键的
> `5000`（建任务请求）漏了**——C→S 方向在 11:39:33 附近丢了一个 TCP 段，旧版把缺口后面的
> 字节直接拼上来，WS 帧边界整体错位，之后该方向的报文全部解析不出来（`5001` 是靠 S→C 方向拿到的）。
> 已修的三点：
> 1. **内核 BPF 过滤**（`ip and tcp and port 10010`），用户态只处理相关包；
> 2. **接收缓冲** `--buf-mb 64`（默认 32）+ 服务里 `ExecStartPre=sysctl -w net.core.rmem_max=67108864`，
>    实测 `SO_RCVBUF` 到 128 MB（默认只有 0.4 MB，这是丢包的根源）；
> 3. **缺口检测**：只解析「从期望位置连续的前缀」，缺一段就停在那里并在 stderr 打
>    `⚠ 丢包：<conn> C→S 在偏移 N 处缺 M 字节`，同时往 JSONL 写一条 `type="gap"` 记录，
>    等丢的段补上后增量继续（不会再把流拼成垃圾）。

跑完官方 App 任务后：

```bash
# 收工（避免一直抓）
ssh robot@192.168.234.234 'sudo -n pkill -f "/tmp/wsniff.py live"'
# 取两个文件回来
scp root@192.168.41.246:/tmp/ws-official.jsonl .   # 先在 .234 上拷到 .246
```

### 11.3 打印与比对长什么样

抓包摘要（每条一行，带时分秒毫秒）：

```text
11:28:18.768 C→S type=1304  # 摇杆
11:31:02.410 C→S type=4013  # 地图/路径列表应答
11:31:04.902 C→S type=5000 kind=4 name="办公区域例行巡检"  # 创建任务
11:31:05.031 S→C type=5001  # 创建任务应答
```

比对输出（`compare` 子命令）：

```text
官方 App：抓到 1 条 5000 报文
  #1 kind=4 name=办公区域例行巡检
我方客户端：kind=2 name=华致办公室 · 前往 新航点 1

=== params 字段 ===
dwell_sec            官方=5              我方=<缺>  ← 不同
goals                官方=<缺>           我方=[{"position": {"x": 7.0, "y": -0.4}}]  ← 不同
map_id               官方=1              我方=<缺>  ← 不同
path_id              官方=自定义路径1      我方=<缺>  ← 不同
patrolSpeedVx        官方=1.2            我方=<缺>  ← 不同
...
官方独有（我方没实现）：dwell_sec, end_task, map_id, minimum_battery, path_id, patrolSpeedVx
我方独有（官方没用）：goal_tolerance, goal_type, goals, map_path, map_type, need_locate, speed_limit
```

> 我方 `5000` 载荷不用额外准备：客户端以 DEBUG 运行时每条都会打
> `5000 载荷（…）: {json}`（`journalctl -u robodog-device-client | grep "5000 载荷"`），
> `compare --client` 直接吃这份日志就行。

### 11.4 重启也不丢：装成 systemd 服务（本次已在 `.234` 上好）

抓包跑在 **`.234`**（网关所在机器）上，所以**重启 `.246` 不影响它**。要连 `.234` 重启也自动恢复，
就做成开机自启的服务（`--seconds 0` = 抓到手动停，`--append` = 不截断上一轮）：

```bash
# 脚本放持久目录（/opt 是根分区，不是 tmpfs）
sudo mkdir -p /opt/robot/diag /var/log/wsniff
sudo install -m 0755 wsniff.py /opt/robot/diag/wsniff.py

sudo tee /etc/systemd/system/wsniff.service >/dev/null <<'UNIT'
[Unit]
Description=ZhiHang gateway WebSocket capture (diagnostic, tcp/10010)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/bin/python3 -u /opt/robot/diag/wsniff.py live --seconds 0 \
  --ports 10010 --append --out /var/log/wsniff/ws-official.jsonl
Restart=always
RestartSec=5
Nice=5

[Install]
WantedBy=multi-user.target
UNIT

sudo systemctl daemon-reload
sudo systemctl enable --now wsniff.service     # 开机自启 + 立刻开始
systemctl is-enabled wsniff.service            # enabled
journalctl -u wsniff -f                        # 摘要（含「新连接 …」，能看到手机 IP）
```

- 抓到的数据：`/var/log/wsniff/ws-official.jsonl`（追加写，重启不覆盖）；
- 收工：`sudo systemctl disable --now wsniff.service`（再想用时 `enable --now` 即可）；
- 它**只读网卡、不写本体、不下发任何指令**，对机器狗行为零影响；
- 日志涨得很慢（心跳类报文默认过滤）；长期挂着时顺手看一眼大小：`ls -lh /var/log/wsniff/`。

### 11.5 注意事项

1. **必须在网关所在机器抓**：手机直连 `.234:10010`，在背包机 `.246` 上抓不到；
2. 只支持 IPv4 + 明文 `ws://`（网关没有 TLS）；
3. 官方 App 会占住控制源（`ctrl_source=3`），期间平台摇杆/下发会被门禁挡住，属正常；
4. 抓包会短暂占用 `/dev/shm`? 不会——但 `dog_task` 崩溃循环时 `dog_task.stdout` 会狂涨，
   记得顺手看一眼 `df -h /dev/shm`；
5. 抓完记得 `pkill`，别让它在现场一直跑。

---

## 12. 方案 3：继续用 `kind=2` 导航模式的现场兜底（`/navigation_state` 裁剪桥）

> 完整操作手册（含编译、remap、systemd、验证、回滚、常见问题）见
> [`navstate-bridge/README.md`](navstate-bridge/README.md)，代码在 `deploy/navstate-bridge/`。

**思路**：`.234` 的 `waypoint_follower` 发的是 0.9.8 编码（末尾多 `uint8 current_floor`），
`.1` 的 `dog_task` 按 0.9.0 解析。那就把这条消息**末尾多出来的字节截掉**再喂给它：

```text
/waypoint_follower ──/navigation_state(0.9.8)──▶ navstate_bridge(截 1 字节)
                                                   └─▶ /navigation_state_v090(0.9.0)
                                                          └─▶ dog_task（remap 到该话题）
```

要点：

1. **不动 `.234`**，只改 `.1`：装 bridge + 给 `dog_task` 加一条 remap
   （`-r navigation_state:=navigation_state_v090`），然后 `robot-launch restart dog_task`；
2. bridge 用 `rclcpp` 的 generic sub/pub（`SerializedMessage`），**只认类型字符串**，
   不需要在 `.1` 上装 0.9.8 的消息包，也不需要重编任何厂商二进制；
3. **上线前必须 dry-run 验长度**（`-p publish:=false`）：同一状态下来回抖动应很小、
   且 `in - out == 1`；若长度乱跳说明差异不止末尾字段，本方案不适用；
4. **环境变量必须和 `dog_task` 一致**（`RMW_IMPLEMENTATION=rmw_zenoh_cpp`、`ROS_DOMAIN_ID`
   等，见 `dog_task` 的 `/proc/<pid>/environ`）——这是最常见的翻车点；
5. **属于现场补丁**：`robot-launch`/alg 镜像升级会覆盖 remap 与 bridge，要写进发布流程；
   厂商给出匹配版本后应撤掉。

对比另外几条路（详见 §8 待办）：

| 方案 | 成本 | 效果 | 结论 |
| --- | --- | --- | --- |
| 厂商给 `.1` 匹配版本（`dog_task` 用 0.9.8 重编） | 等厂商 | 根治 | **首选** |
| 让 `.234` 停发/改发 `/navigation_state`（若 alg 有开关） | 10 分钟勘察 | 可能一劳永逸 | 值得先试（`.246` 断开时未验证） |
| **本方案：裁剪桥 + remap** | ~1 小时 | 立刻能用 `kind=2` | 现场兜底 |
| 平台侧自愈（识别 `boot_id` 变化/`sdk_node` 抖动后重试航段） | 半天 | 降低停机 | 建议一起做 |
| 合并 `goals` 多点下发等「改报文内容」 | 几分钟 | 靠概率 | 只能当试验 |

### 11.6 抓包工具的两次踩坑（都修好了）

| 版本 | 问题 | 现象 | 修法 |
| --- | --- | --- | --- |
| v1 | 只存 `INTERESTING` 类型的 payload | 5003/5005/5007/5013 的明细丢了（后来查文档：它们只是取消/暂停/恢复/返起点的 ack，不含参数，损失可接受） | 现在所有非心跳报文都落 payload（`--max-payload` 截断保护） |
| v2 | **BPF 过滤器跳转偏移少算 2 跳** + **TCP 头偏移没按 IHL 计算** | 过滤器把自己要的包全丢了 → **抓包空转**（文件一直没有新记录，日志只有启动那几行） | 重写 `_bpf_program`（先 `ldx 4*([14]&0xf)` 算 IP 头长度，再用 `ldh [x+14]` 间接寻址），并加 **`bpf_selftest`**：挂载前用 5 条合成报文（命中端口 / 源端口 / 其它端口 / UDP / ARP）验证放行与丢弃，自检不过就退回用户态过滤而不是静默全丢 |

修好后实测：8 秒抓到 312 条（`.200` 客户端 + `.8` 手机两条连接），`5009` 里能看到
`robot.nav_state.current_floor`（正是 `.1` 那版 `NavigationState.msg` 没有的字段，反向印证 §3 的版本差异）。

挂载日志应当长这样（有 `bpf_selftest` 通过才挂）：

```text
已挂内核 BPF 过滤器（tcp 端口 [10010, 12000]）      ← 自检 5 条合成报文全部通过才会打印
接收缓冲 SO_RCVBUF = 128.0 MB
抓包中（端口 [10010, 12000]），Ctrl-C 结束…
13:32:37.147 S→C type=1315  # 本体状态               ← 有心跳输出说明真的在抓
```

---

## 13. 巡检 `min_recharge_battery`：低电会报成「origin pose unavailable」（2026-11-14 14:17）

**现象**（用新客户端的第一次真机巡检）：

```text
巡检任务失败: 导航失败: patrol step failed: origin pose unavailable for return-to-start
（path=平台-TK-1112-11 map=2026_11_10_16_15_06）
```

任务 14:17:21.294 受理，**同一毫秒**就终态失败，看着像"参数不对"，实际是电量。
ALG 日志（`.234` `robot-alg-manager.stdout`）写得很清楚：

```text
[task 7f75dce2… patrol] -> Running
[patrol] low battery — interrupting patrol to return to patrol origin (battery=25.000000%, threshold=25.000000%)
[patrol] task failed: [capability:14000] patrol step failed: origin pose unavailable for return-to-start
```

**因果链**：`min_recharge_battery = 25` 是「**巡检进行中**电量 ≤ 阈值就中断回起点」；
起步时电量已经 25% → 立刻中断 → 但「起点位姿」是任务第 2 步 `capture_origin_pose` 才采的，
还没采就中断 → 回起点拿不到位姿 → 报 `origin pose unavailable`。

**电量口径的坑**：同一时刻平台显示 **52%**，ALG 读到 **23~26%**（在掉）。
原因是客户端心跳里的 `battery` 取**两节电池里较高的一节**（展示用），而 ALG 按可用电量判断。
双电池差异大时平台读数会严重高估。

**客户端已改**（`business/client`）：

1. `RobotState` 增加 `battery_min`（`power1/power2` 取 min），两节差异 ≥5% 打 WARN；
2. 巡检下发前守卫：`battery_min ≤ min_recharge_battery` → **直接拒发**并给可操作文案
   （「请先充电到 N% 以上，或调低 --min-recharge-battery」）；
3. 终态错误翻译：`origin pose unavailable` → 自动补「最常见原因是低电保护（电量 X% ≤ 阈值 Y%），
   其次才是定位未收敛/地图未载好」；`capture_image ... service not ready` → 补「拍照失败但不影响整体」。

**现场处置**：要么充电到阈值以上（推荐 ≥ 阈值 + 10%），要么临时
`--min-recharge-battery 10`（短航线够跑）。别在 20% 出头跑巡检。
