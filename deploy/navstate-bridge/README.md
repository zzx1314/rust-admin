# navstate_bridge：`/navigation_state` 版本兼容桥（方案 3 操作手册）

> 场景：`.234`（算法机）`robots_dog_msgs 0.9.8` 与 `.1`（运控机）`0.9.0` 不一致，
> `NavigationState.msg` 末尾多一个 `uint8 current_floor` → `dog_task` 反序列化偶发 `bad_alloc` →
> 一开导航就崩。根治要厂商出匹配版本；本桥是**现场兜底**：把消息末尾多出来的字节截掉，
> 让 `dog_task` 按旧布局正常解析，从而**继续使用 `5000 kind=2` 导航模式**。
>
> 背景与证据见 [`../robodog-issues-2026-11-14.md`](../robodog-issues-2026-11-14.md) §3、§12。

```
/waypoint_follower (.234)
      │  /navigation_state            ← 0.9.8 编码（末尾多 1 字节）
      ▼
[navstate_bridge]  (.1)               ← 本目录编出来的节点
      │  /navigation_state_v090       ← 截掉尾部字节，即 0.9.0 布局
      ▼
dog_task (.1)                          ← 启动参数加 remap：navigation_state:=navigation_state_v090
```

**不动 `.234`，只动 `.1`**（bridge + dog_task 的 remap）。

---

## 0. 三个前提，先确认（10 分钟）

### 0.1 差异确实只是「末尾多 1 个字段」

```bash
# 两台机器分别执行，md5 应该不同
md5sum /opt/ros/humble/share/robots_dog_msgs/msg/NavigationState.msg
# 比较内容（在任一机器上 diff 两份文件）：应只有最后一行 `uint8 current_floor` 的差异
```

如果 diff 显示**中间字段也变了**，本方案不适用（截尾无效），只能走厂商升级。

### 0.2 `dog_task` 的订阅能 remap

```bash
# 看它当前是怎么起的（launch 生成的命令行里已有 --ros-args）
ps -ef | grep -E "/opt/ros/humble/lib/dog_task/dog_task" | grep -v grep

# 找它的 launch 文件 / robot-launch 的 egg 定义
grep -rln "dog_task" /opt/robot /opt/ros/humble/share 2>/dev/null | grep -E "launch|yaml|json|sh" | head
```

- 若是 **ROS2 launch（python）** → 后面加 `remappings=[('navigation_state', 'navigation_state_v090')]`；
- 若是 **robot-launch 的 egg（yaml/json/sh）** → 在它的 `args`/`command` 里加
  `--ros-args -r navigation_state:=navigation_state_v090`。

### 0.3 记下 `dog_task` 的运行环境（**必须**和 bridge 一致）

```bash
PID=$(pgrep -f /opt/ros/humble/lib/dog_task/dog_task | head -1)
sudo tr '\0' '\n' < /proc/$PID/environ | grep -E "ROS_|RMW_|ZENOH|CYCLONE|FASTRTPS" | sort
```

典型会看到 `RMW_IMPLEMENTATION=rmw_zenoh_cpp`、`ROS_DOMAIN_ID=24`（以现场实测为准）。
**bridge 必须用同一套环境变量启动**，否则两个节点互相看不见（最常见翻车点）。

---

## 1. 编译（推荐在 `.234` 上编，产物拷到 `.1`）

先看有没有工具链：

```bash
which colcon cmake g++ ; ls /opt/ros/humble/include/rclcpp >/dev/null && echo "rclcpp 头文件 OK"
```

### 1.1 有工具链（`.234` 大概率有，它有 `robot_alg_manager_cpp`）

```bash
mkdir -p ~/navstate_bridge_ws/src && cd ~/navstate_bridge_ws/src
# 把本目录（deploy/navstate-bridge）整体拷成 navstate_bridge
scp -r root@192.168.41.246:/path/to/navstate-bridge ./navstate_bridge   # 或者直接 git clone 仓库
cd ~/navstate_bridge_ws
source /opt/ros/humble/setup.bash
colcon build --packages-select navstate_bridge
ls install/navstate_bridge/lib/navstate_bridge/navstate_bridge        # 产物
```

### 1.2 把产物拷到 `.1`（同架构 aarch64、同 ROS Humble，直接可跑）

```bash
# 在 .234 上
tar czf /tmp/navstate_bridge.tgz -C ~/navstate_bridge_ws install/navstate_bridge
# 从 .246 上拉，再推到 .1
scp robot@192.168.234.234:/tmp/navstate_bridge.tgz /tmp/
scp /tmp/navstate_bridge.tgz firefly@192.168.234.1:/tmp/
# 在 .1 上
sudo mkdir -p /opt/robot/navstate_bridge
sudo tar xzf /tmp/navstate_bridge.tgz -C /opt/robot/navstate_bridge --strip-components=1
/opt/robot/navstate_bridge/lib/navstate_bridge/navstate_bridge --help || true   # 能起来就说明动态库齐
```

> 若 `.1` 自己就有 `colcon/cmake/g++` 和 `rclcpp` 头文件，也可以直接在 `.1` 上编，省掉拷贝。

---

## 2. 先干跑（dry-run）验证长度

先不改 dog_task，只让 bridge 统计帧长，确认「截 1 字节」是对的：

```bash
source /opt/ros/humble/setup.bash
export RMW_IMPLEMENTATION=rmw_zenoh_cpp ROS_DOMAIN_ID=24     # ← 用 0.3 抄下来的实际值
/opt/robot/navstate_bridge/lib/navstate_bridge/navstate_bridge \
  --ros-args -p publish:=false -p log_every:=50
```

然后**在平台或官方 App 触发一次导航**，观察输出：

```text
[dry-run] /navigation_state 收到 50 帧，最近一帧 1287 字节（将截为 1286）
```

判据：长度应当**很稳定**（同一状态下来回抖动不超过几十字节），说明截尾可行。
如果长度在几百/几万之间跳，或出现 `丢弃异常帧`，停下来找厂商。

---

## 3. 加 remap（改 `dog_task` 的启动）

### 3.1 若是 ROS2 launch（python）

找到启动 `dog_task` 的那个 `Node(...)`，加一行：

```python
Node(
    package='dog_task', executable='dog_task', name='dog_task',
    parameters=[params_file],
    remappings=[('navigation_state', 'navigation_state_v090')],   # ← 新增
    output='screen',
)
```

### 3.2 若是 robot-launch 的 egg（脚本/yaml）

在它的命令行参数里加一段（`--ros-args` 只能出现一次，追加到已有的 `--ros-args` 后面即可）：

```bash
--ros-args -r navigation_state:=navigation_state_v090
```

### 3.3 重启 dog_task 并确认 remap 生效

```bash
robot-launch restart dog_task            # 或者 sudo systemctl restart robot-launch
ps -ef | grep /opt/ros/humble/lib/dog_task/dog_task | grep -v grep      # 命令行里应能看到 -r navigation_state:=...
```

---

## 4. 起 bridge（systemd，开机自启）

```bash
sudo tee /etc/systemd/system/navstate-bridge.service >/dev/null <<'UNIT'
[Unit]
Description=NavigationState version bridge (robots_dog_msgs 0.9.8 -> 0.9.0)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
# ↓↓↓ 必须与 dog_task 一致（用第 0.3 步抄下来的真实值）
Environment=RMW_IMPLEMENTATION=rmw_zenoh_cpp
Environment=ROS_DOMAIN_ID=24
# 如果 dog_task 还带别的 ROS/ZENOH 环境变量，一并写上（或改用 EnvironmentFile）
ExecStart=/opt/robot/navstate_bridge/lib/navstate_bridge/navstate_bridge \
  --ros-args -p in_topic:=/navigation_state -p out_topic:=/navigation_state_v090 \
  -p drop_tail:=1 -p log_every:=200
Restart=always
RestartSec=3
Nice=0

[Install]
WantedBy=multi-user.target
UNIT
sudo systemctl daemon-reload
sudo systemctl enable --now navstate-bridge.service
journalctl -u navstate-bridge -n 20 --no-pager
```

看到这两行就对了：

```text
navstate_bridge 启动：/navigation_state → /navigation_state_v090（type=..., drop_tail=1, publish=true）
/navigation_state → /navigation_state_v090 已转发 200/200 帧（累计 ... 字节），最近 in=1287 out=1286
```

---

## 5. 验证（按顺序）

| # | 检查 | 期望 |
| --- | --- | --- |
| 1 | `journalctl -u navstate-bridge -f` | 转发计数在涨，`in - out == 1` |
| 2 | `.1` `journalctl -u robot-launch -f` / `tail -f /home/firefly/.robot_launch_logs/dog_task.stdout` | **不再出现** `could not deserialize ROS message ... /navigation_state`；`retry` 不涨 |
| 3 | 平台设备抽屉 | 下发 `kind=2` 任务后进度从 `0/5` 往上走，不再 5 秒内失败 |
| 4 | 5009（平台事件流） | 出现 `navigation.navigating / goal_arrived`，`detail.last_arrived_goal` 递增 |
| 5 | 抓包复核（可选） | `wsniff.py live --ports 10010` 里 `5000/5001/5009` 正常，无 `13897/13440/13446` |

出现「1 帧没转发」时看 `journalctl` 的 `丢弃异常帧`：说明帧长异常（可能不是 0.9.8 的帧，或 `drop_tail` 不对）。

---

## 6. 回滚（三条命令，1 分钟）

```bash
# ① 停 bridge
sudo systemctl disable --now navstate-bridge.service
# ② 去掉 dog_task 的 remap（把第 3 步加的那行删掉）
# ③ 重启 dog_task
robot-launch restart dog_task
```

回滚后行为与原来完全一致（`dog_task` 又去订阅 `/navigation_state`，在版本不一致时继续偶发崩溃）。
建议**保留**已安装的 `/opt/robot/navstate_bridge/` 和本仓库的 `deploy/navstate-bridge/`，
厂商版本到位后一起清理。

---

## 7. 局限与风险（务必知道）

1. **只适用于「差异在末尾」**：本桥是纯字节截断，不做语义转换。若 0.9.8 在中间字段/嵌套消息
   （`NavigationTask`、`NavigationTaskStatus`、`StatusItem`）上也变了，截尾无效，只能升级/回退版本。
2. **`robot-launch`/alg 镜像升级会覆盖 remap 和 bridge**：把「装 bridge + 加 remap」写进发布流程，
   升级后重新执行第 3、4 步，或直接删掉（如果厂商已经修好版本）。
3. **bridge 挂了 dog_task 就收不到导航状态**：所以用 `Restart=always`；也可以加一条平台告警
   （`journalctl -u navstate-bridge` 里 10 秒无「转发」日志 = 桥停了）。
4. **QoS**：bridge 收/发都用 `RELIABLE / KEEP_LAST(10)`；发布端 RELIABLE、订阅端 BEST_EFFORT 都兼容。
5. **不改变时间戳与帧顺序**，只是去掉末尾字节；`dog_task` 拿到的 `current_floor` 会缺失
   （它本来就没有这个字段，所以等价于旧版行为）。
6. **不影响 `patrol`（`kind=4`）链路**：那条链路本来就跑得通，桥只是同话题转发，多一份无害的订阅。

---

## 8. 常见问题

| 现象 | 原因 / 处理 |
| --- | --- |
| bridge 起了，但一直没有「收到」日志 | ROS 环境变量不一致（`RMW_IMPLEMENTATION` / `ROS_DOMAIN_ID` / zenoh 配置），按 §0.3 比对 `dog_task` 的 `/proc/<pid>/environ` |
| 一直打印 `丢弃异常帧：buffer_length=...` | 帧长 ≤ `drop_tail + 4`，说明收到的是别的消息（话题搞错）或对端没发 |
| `dog_task` 仍报 `could not deserialize` | ① remap 没生效（`ps -ef` 里看不到 `-r`）；② `drop_tail` 不对（用 dry-run 再看长度）；③ 中间字段也变了（见 §7.1） |
| 平台看到 `sdk_node` 仍抖动 | 那是 `dog_task` 崩溃重启的连带现象；先确认 §5 第 2 条通过 |
| 想临时不转发、只看统计 | `-p publish:=false`（dry-run），对本体零影响 |

---

## 9. 相关文档

- 事故与证据链：[`../robodog-issues-2026-11-14.md`](../robodog-issues-2026-11-14.md)（§3 消息版本不一致、§12 方案 3 说明）
- 本体运维手册：[`../robodog-body.md`](../robodog-body.md)（§2 进程树、§4.5 清 shm、§10 急停卡死）
- 抓包工具：[`../../business/tools/wsniff.py`](../../business/tools/wsniff.py)（验证 5000/5009 是否正常）
