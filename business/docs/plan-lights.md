# 计划灯光（补光灯 / LED）：配置、链路与排查

板子 `.246` 上的两盏灯（背包板补光灯 GPIO、LED 灯带）可以随计划自动开关，
**三种触发方式（计划级优先，其次航点，最后航段）**：

- **计划级自动开灯**（`tasks.light_fill` / `light_led`，迁移 032）——推荐，现场默认：
  计划里勾了哪盏，整个计划期间就开哪盏，**航点 / 航段动作里的开灯关灯不生效**；
- **航点动作**（`routes.route_data.points[].actions[].lightFill/lightLed`）——老行为：
  到这个点开、离开关；
- **航段动作**（`routes.route_data.legs[].actions[].lightFill/lightLed`，含 `returnLeg`）——
  **起步开（离开起点航点）、走完这一段才关**：夜里让狗亮着灯走完整段路。
  到点那一刻**不开灯**——这个航点还要跑 AI 识别 / 拍照 / 停留，亮着的补光灯会把
  画面和存证图一起污染（2026-11-25 `gh-test` 现场问题，见 §1 表最后一行）。

计划级「仅夜间」（`tasks.light_night_only`，迁移 030）对三种模式都生效，判定在**板端**。

```text
计划（tasks）
├─ light_fill / light_led  ──► 载荷 lightFill / lightLed（任一为真 → 以计划为准）
└─ light_night_only        ──► 载荷 lightNightOnly（白天跳过，板端判）
        │
        ▼
client（与 Agent 同机）  ActiveTask::light_at 一处收口
├─ 有计划灯：任务一开始就开（5009 里出现这条任务），全程不跟航点，终态 / 取消才关
├─ 没计划灯、这个点有航点动作：到点开（scope=point），离开航点就关
└─ 没计划灯、只有从这里出发的航段动作：到点只记账（不开灯），
   起步（离开航点）才开（scope=leg），走完这一段才关, 途中按 LIGHT_REFRESH_AFTER 重发（看门狗续命）
        │  POST {fill, led, nightOnly, taskCode, pointIndex, scope}
        ▼
板端 Agent :8000
  POST /api/lights      → control.lights_apply（nightOnly 且白天 → skipped=daytime）
  POST /api/lights/off  → control.lights_off（补光 GPIO 拉低 + 杀 LED 进程）
  GET  /api/lights      → 状态（补光/LED、是否计划点亮、当前是否夜间、时间窗）
```

## 1. 为什么这么分层

| 决定 | 理由 |
| --- | --- |
| 灯由**客户端**调，平台不直接调 Agent | 业务平台在公网，Agent 只在内网（与航点 AI 同一条理由，见 `ai_patrol.rs` 模块注释） |
| 夜间判定在**板端** | 平台/客户端与板子可能不同时区、时钟也可能不准。判据只有 `control.is_night` 一处；灯亮了却不知道为什么亮最难查 |
| 计划级开关只下发布尔值 | 客户端只搬运 `nightOnly` / 两盏灯的开关，不自己算时间，避免「两地各说一套」 |
| **计划优先**（配了计划灯就忽略航点 / 航段动作） | 巡检要的是「这一趟亮着」，不是每个航点翻一遍；两层同时生效时现场分不清「灯为什么不亮／为什么灭了」（2026-11-23 现场要求：以计划为准） |
| 计划灯全程开、到点模式到点开 | 计划灯不等第一个航点（导航期间也是黑的就没意义），航点模式与 AI 窗口同一条收口规则（阶段不再是 `patrol.point_*`），不额外引入状态机 |
| 航段灯**到点不开**、起步才开 | 到点后这个航点还要跑 AI 识别 / 可见光拍照 / 停留，亮着的补光灯会把识别画面和存证图一起污染（2026-11-25 `gh-test`：P1 没配灯、航线 1-2 配了补光灯，P1 的航点任务还没跑完灯就亮了）。「进段」的现场语义是「狗真的开始走这一段」，不是「到点」；收口交给下一个航点（那边配了灯就覆掉，没配就 `leave`） |
| 航段灯**不**跟航点一起关 | 离开起点航点正是航段真正开始的时刻：到点时只记了账、没点亮，此刻才点亮（而不是关掉）。航段灯亮完整段是现场要的效果（夜里走完一段走廊 / 一段无灯通道） |
| LED 灯带与补光灯**同一套时机** | `lightFill` / `lightLed` 只是同一个 `LightPoint` 的两个字段，scope（航点 / 航段）决定时机，没有第二套时序。LED 走 `systemctl start/stop alarmlight`，所以「走完这一段」在下一个航点到点时 `stop`（SIGINT 熄灯，灯珠不会停在最后一帧），中间航点停留期间灯带是**灭**的；想整趟一直亮用计划里的「自动开灯」 |
| 去重键带 `LightScope` | 航段下标 = 起点航点下标，光看 `index` 会让「航点 3 的灯」和「从 P3 出发的航段灯」互相顶掉（一个把另一个的开灯记录 / 活跃态擦了，灯乱闪） |
| 同一处既配航点灯又配航段灯 → **航点优先** | 与语音播报同一条规则（板端一次只认一份开灯参数）。现场的预期是「到点那盏」；要走一整段都亮着，就只配在航段上 |
| 行进途中重发（`keep_alive`） | 板端看门狗（`LIGHT_MAX_ON_SECS`，默认 900s）只认最近一次请求；航段上没有新的「到点」帧，不重发就会走到一半黑掉 |
| 看门狗在板端 | 客户端掉线 / 被 kill / 断电重启时，灯不能一直亮着（4G 背包耗电扛不住） |

## 2. 配置在哪

- **计划 → 自动开灯**（计划表单 → 「自动开灯（补光灯 / LED）」）：
  `lightFill` / `lightLed` 两个复选框 + 「仅夜间」+ **可选的夜间时段**。
  **任勾一盏就以计划为准**：航点 / 航段动作里的开灯、关灯全部不生效，灯从任务开始
  一直亮到终态 / 取消；「仅夜间」勾上时只在板端夜间窗内开（白天派单直接跳过）。
  两盏都不勾 = 沿用下面的航点 / 航段动作（老计划 / 老平台行为不变）。
- **夜间时段**（计划表单 → 「仅夜间」下面的两个时间框，`tasks.light_night_start` /
  `light_night_end`，迁移 033）：`HH:MM`，允许跨零点（`19:00 - 06:00`）。
  **留空 = 用板端默认**（`LIGHT_NIGHT_START` / `LIGHT_NIGHT_END`，18:00-06:00）。
  平台只存这两个值，客户端随 `POST /api/lights` 的 `nightStart` / `nightEnd` 传给板端，
  **判夜仍在板端**（板子本地时间）；只给一半 / 格式不对 / 起止相同接口直接 400。
  接口层拦「起止相同」是因为板端把它当「关闭夜间判定」——静默收下会让现场以为配了
  时段却永远不开灯。
- **航点动作**（路线编辑器 → 点动作 → 动作类型 **「补光」** → 「到点开灯」）：
  `lightFill` / `lightLed` 两个复选框。**只有动作类型选「补光」时才显示这两个勾**
  （以前它们出现在 AI 识别 / 拍照 / 播报… 的表单里，现场分不清灯是谁开的）；
  别的类型要开灯 = 再加一条「补光」动作。客户端仍然只看字段、不看类型名，所以
  老路线里其它类型动作上已经配的灯照样生效（编辑时会显示出来，可以取消）。
  多个动作合并为「或」（任一要求开灯就开）。**计划里配了自动开灯时这两个勾不起作用**
  （界面上有提示）。
- **航段动作**（路线编辑器 → 行进任务 → 「补光」→ 「航段开灯」）：同样是那两个复选框，
  同样只在动作类型「补光」时显示，但时机是**起步开、走完这一段关**（起步 = 离开起点
  航点，也就是这个航点的任务跑完之后；航段走完 = 到达下一个航点）。`补光` 只是动作
  卡片上的名字，真正开哪几盏灯看这两个勾（一个都不勾保存时会被拦住）。同一处既配了
  航点到点开灯又配了航段开灯时**航点优先**；**计划里配了自动开灯时一律以计划为准**。
- **板端时间窗**（`backend/config.py`）：`LIGHT_NIGHT_START` / `LIGHT_NIGHT_END`
  默认 `18:00` / `06:00`，**板端本地时间**，允许跨零点；**起止相同 = 关闭夜间判定**
  （退化成「到点就开」，现场临时要白天也开灯时用）。`LIGHT_MAX_ON_SECS`（默认 900s）
  是计划灯的看门狗上限。

## 3. 灯怎么开关

- 补光灯：`/sys/class/gpio/gpio<FILL_LIGHT_GPIO>` 写 `1`/`0`（默认 GPIO 46）。
  `FILL_LIGHT_GPIO=-1` 或与 `CAM_TRIGGER_GPIO` 同号时**拒绝执行**并回错误——但不能因为
  补光灯不可用就不开 LED（`lights_apply` 会继续点 LED，返回 `ok=false` + `errors`）。
- LED 灯带：没有常亮接口，厂商程序都是「跑起来就一直刷颜色」，所以「开」= 让程序跑、
  「关」= 让它停。**开关走 systemd unit**：`systemctl start/stop alarmlight`
  （`LED_SERVICE`，板端 `/etc/systemd/system/alarmlight.service` →
  `ExecStart=/opt/xjpt/led/alarmlight`，红蓝 100ms 爆闪；仓库副本
  `deploy/backpack-alarmlight.service`）。为什么不用 `Popen` + 杀进程：厂商程序只在
  **SIGINT** 里写「全 0 + reset」熄灯，老逻辑发 SIGTERM 是默认终止 —— 进程没了、灯珠停在
  最后一帧一直亮着；unit 的 `KillSignal=SIGINT` + `TimeoutStopSec=10` 才是真正的关灯，
  而且进程归 systemd 管，Agent 重启不会留下继续抢 SPI 的孤儿进程。
  没有这个 unit 的板子（开发机 / 模拟器 / 老部署）自动退回直接跑 `LED_TEST_BIN`
  （`LED_CONTROL=auto|systemd|proc`），兜底路径关灯也是先 SIGINT 再退 SIGTERM/SIGKILL。
  ⚠️ unit **不要 `enable`**：灯由 Agent 按需 start，开机自启等于一上电就一直爆闪。
- 开灯时机：**计划灯** = 5009 里一出现这条任务就开（不用等第一个航点），
  `PointLights` 按 `(taskCode, Point, PLAN_LIGHT_INDEX=-1)` 去重，一条任务只发一次，
  但每 `LIGHT_REFRESH_AFTER`（300s）重发一次——板端看门狗（`LIGHT_MAX_ON_SECS`，
  默认 900s）只认最近一次请求，不刷新的话 15 分钟后的长航次会被它关灯；
  **航点模式的灯** = 到达该航点（`light_at(detail_index)` 命中航点动作）时开；
  **航段模式的灯** = 到达该航段起点航点时**只记账不开灯**（`PointLights::defer_leg`），
  等狗真的起步——阶段离开 `patrol.point_*`（`light_at` 命中 `legs[起点]` / `returnLeg`）
  才 `POST /api/lights`；行进途中由 `PointLights::keep_alive` 按同一个间隔重发。
- 关灯时机：**计划灯** = 任务终态 / 取消 / 本地跑收尾（航点切换不关）；
  **航点模式** = 离开航点（`leave_point` 收 scope=point 的灯）/ 到达无灯光动作的航点 /
  终态 / 取消 / 本地跑收尾；
  **航段模式** = 到达下一个航点（那边配了航点灯就覆掉，否则走 `leave` 收灯）/ 终态 / 取消 /
  本地跑收尾，**行进途中不关**；
  三种情况都有板端看门狗超时兜底。客户端 `PointLights::leave` 只在**自己真的开过灯**
  时才发 `off`（白天跳过、Agent 不可用都不发），以免顺手关掉操作员手动开的灯。
- 收尾用 `PointLights::finish_task(taskCode, reason)`：它会把这条任务标成「已收尾」，
  迟到的「到点开灯」（5009 到点帧与终态帧只差一帧，开灯又是 spawn 出去的）不会把灯
  重新点亮；同名任务重新下发时 `start_task(taskCode)` 清掉这些痕迹。
- 手动灯的看门狗豁免：`/api/filllight`、`/api/led/blink` 不开 `managed`，一直亮着。

## 4. 代码地图

| 位置 | 作用 |
| --- | --- |
| `business/backpack_dashboard/backend/control.py` | `is_night` / `night_window` / `lights_apply` / `lights_off` / `lights_status` / 看门狗 / LED 的 `systemctl start|stop`（`led_mode` 判定 + `LED_TEST_BIN` 兜底） |
| `business/backpack_dashboard/backend/agent.py` | `GET/POST /api/lights`、`POST /api/lights/off`；手动接口保持原样 |
| `business/backpack_dashboard/backend/tools/lights_selftest.py` | 时间窗 / 白天跳过 / GPIO+LED / 看门狗 / `systemctl start|stop`（stub 的 systemd 状态机）自检（不需要板子） |
| `business/server/migrations/030_task_light_night_only.sql` | `tasks.light_night_only`（默认 0） |
| `business/server/migrations/032_task_plan_lights.sql` | `tasks.light_fill` / `light_led`（默认 0；回填已勾「仅夜间」的计划为两盏都开） |
| `business/server/migrations/033_task_light_night_window.sql` | `tasks.light_night_start` / `light_night_end`（NULL = 板端默认） |
| `business/server/src/dispatch.rs` + `gateway_types.rs` | 载荷 `lightNightOnly` / `lightFill` / `lightLed`（都总是下发） |
| `business/client/src/state.rs` | `ActiveTask::light_at`（计划灯优先 → 航点动作 → 该点出发的航段）、`bind_nav_task`（巡检任务号绑定） |
| `business/client/src/lights.rs` | `LightScope`（航点 / 航段）/ 解析动作 / 航点灯到点开 / **航段灯到点只记账（`pending`）、起步（`leave_point`）才开** / `leave`（收灯 + 清待开记账）/ `keep_alive`（航段灯途中续命）/ 去重 / `start_task` / `finish_task`；`lightNightOnly` 与 `NightWindow`（`nightStart`/`nightEnd`）透传 |
| `business/client/src/session.rs` / `pump.rs` / `handlers/task.rs` | 5009 到点、离开（`leave_point` + `keep_alive`）、终态、取消、本地收尾的挂钩 |
| `business/web/src/views/routes/index.vue` | 航点动作「到点开灯」+ 行进任务「航段开灯」两个复选框 + 动作卡片「灯光：到点开…/整段开…」+ 「计划优先」提示 |
| `business/web/src/utils/constants.ts` | 动作库里的 `补光`（`kind: "both"`）：只开灯不干别的时用它，两侧都能挂 |
| `business/web/src/views/tasks/index.vue` | 计划「自动开灯（补光灯 / LED）+ 仅夜间」；详情页摘要 `planLightSummary` |
| `deploy/backpack-alarmlight.service` | LED 灯带的 systemd unit（`ExecStart=/opt/xjpt/led/alarmlight`，`KillSignal=SIGINT`）；仓库 = 板端 |
| `deploy/backpack-sim/mock_agent.py` | 模拟 `/api/lights`（`SIM_NIGHT=1/0` 可钉死白天/夜间） |

## 5. 现场排查

1. 灯完全不亮：
   - `curl -s 127.0.0.1:8000/api/lights`：看 `fill_available`（GPIO 被禁/同号时为
     false）、`led_control`（`systemd` = 走 `systemctl start/stop`，`proc` = 退回直接
     跑 `LED_TEST_BIN`，后者再看 `led_bin_exists`）、`led_service_state`（unit 现在的
     active/inactive/failed）、`night`、`night_window.enabled`；
   - `systemctl status alarmlight` / `journalctl -u alarmlight -n 50`：unit 起不来（SPI
     被占、`/dev/spidev1.0` 权限）就是这里的报错；⚠️ **接口报「已点亮」不代表灯珠亮**，
     只代表 unit `active`（SPI 写失败程序会退出，`Restart=on-failure` 会反复重启，
     `journalctl` 里能看见）；
   - `pgrep -af 'test_rgbrun|alarmlight'`：**只能有一个写者**。老版本 Agent 留下的
     `test_rgbrun` 孤儿会跟 unit 抢 SPI（新版 Agent 启动后第一次开关灯会自动 `pkill`
     掉它），手动兜底：`pkill -f /opt/xjpt/led/test_rgbrun`；
   - `POST /api/lights` 手工打一发，看返回 `errors`；
   - 客户端日志 grep `航点灯光`：`开灯失败 / 关灯失败` 会带 Agent 的原因。
   - **先确认 5009 跟上了这条任务**：巡检的本体任务号由客户端的 `RouteRunner` 自己发
     5000 拿，`ActiveTask.nav_task_id` 空着的时候 `handle_status_frame` 会整帧跳过，
     航点灯光 / AI 一起静默（日志里连「已开灯」都不会有）。正常应能看到
     `任务 TK-… 绑定本体任务号 …：5009 的航点 AI / 航点灯光从此生效` 和
     `任务 TK-… 含 N 个灯光航点`；只有后者没有前者、或者两行都没有，就往
     「5000 被本体拒绝（13897 capability busy / 定位未就绪）」方向查。
     2026-11-23 19:35 那次「计划勾了自动开灯但灯不亮」就是这条链路断了（兜底绑定被删），
     回归 `pump::tests::route_task_must_bind_the_runner_task_id_before_5009_is_followed`。
2. 计划里勾了自动开灯但灯没亮（或航点动作明明配了却不动）：
   - 客户端日志应有 `任务 TK-… 计划级自动开灯（补光=true LED=true）：以计划为准…`；
     没有这一行 = 服务端没下发 `lightFill`/`lightLed`（计划没勾 / 老服务端），看
     `task_dispatch 载荷` 里的这两个字段；
   - 计划没勾时反而是航点 / 航段动作生效，日志分别是 `含 N 个灯光航点，M 个灯光航段`；
   - 三种模式都受「仅夜间」限制（见下一条）；
   - 自定义时段在计划里配了时，日志有 `任务 TK-… 夜间时段：19:00-06:00（板端按板子本地
     时间判定）`；板端 `GET /api/lights` 的 `night_window` 应是计划时段、`plan_night_window`
     单独露出它（`null` = 用板端默认）。`night_window` 还是 18:00-06:00 而计划里配了
     19:00 → 说明客户端没把 `nightStart`/`nightEnd` 带下去（看客户端版本）。
3. 白天不亮（预期行为）：计划勾了「仅夜间自动开灯」，且当前不在时间窗内（板端默认
   18:00-06:00，或计划里自定义的那个时段）→
   `POST /api/lights` 返回 `skipped=daytime`，客户端日志有「在白天到达，跳过开灯」。
   要白天也开：计划取消勾选，或把 `LIGHT_NIGHT_START`/`LIGHT_NIGHT_END` 设成相同值。
4. 灯一直亮着：先看客户端有没有收到任务终态/取消（阶段卡在 `patrol.point_*` 就不会
   发 `off`）；最迟 `LIGHT_MAX_ON_SECS` 后板端看门狗会关。手动按钮开的灯本来就不看门狗。
   配了**航段灯**时「走到下一个点还不灭」先查两件事：① 下一个点自己配了航点灯
   （那边用新参数覆掉，属正常）；② 那个点的阶段帧是不是一直没来（`patrol.point_arrived`
   丢了就没人接手，等到终态才关）。日志里能看到 `已开灯（… scope=leg）` 与
   `关灯（到达无灯光动作的航点）`。
5. **到点灯就亮了（航点任务还没跑完）**：先确认这条灯配在哪一层——`航段 N-M` 上的灯
   到点**不该**亮（等起步），日志里到点那行是
   `到达航点 N，从这里出发的航段灯等离开航点再开`，起步后才有
   `已开灯（… 航段 N …）`；如果是 `已开灯（… 航点 N …）`，那就是**航点动作**也勾了灯
   （或计划里勾了「自动开灯」），不是航段那条链路的问题。
   2026-11-25 `gh-test`（P1 没配灯 / 航线 1-2 配了补光灯，到 P1 灯就亮）就是旧版客户端
   「进段 = 到达起点航点」的时序，回归 `client pump::tests::leg_lights_start_when_the_dog_leaves_the_waypoint`、
   `client lights::tests::leg_lights_start_on_departure_not_on_arrival`。
   **LED 灯带同理**（同一个 `PointLights`）：到点那一帧不许 `systemctl start alarmlight`
   （板端日志里没有 `[lights] 已点亮 LED`），起步那一帧才有；回归
   `client lights::tests::leg_led_waits_for_departure_like_the_fill_light`。
   灯带在每个航点会「熄 → 起步再亮」一次（systemd start/stop 有秒级延迟），
   要整趟不灭就把灯配到计划的「自动开灯」上。
6. LED 关了但还在闪：`test_rgbrun` 自己 fork 了子进程。`led_off` 用进程组杀，
   若厂商程序改过（不再继承进程组），需要在板端 `pkill -f test_rgbrun` 兜底。

## 6. 回归

```bash
python3 business/backpack_dashboard/backend/tools/lights_selftest.py
cd business/client && cargo test lights::            # 解析（航点 + 航段）/ 开灯去重 / 白天跳过 / 关灯 / 航段灯到点不开、起步才开
cd business/client && cargo test pump::tests         # 5009 跟随 + 计划灯全程开 / 航点模式照旧 / 航段灯起步才开、走完这一段关
cd business/client && cargo test lights::            # 收尾后迟到的开灯不能点亮（finish_task）
cd business/client && cargo test active_task_light   # light_at 优先级：计划灯 → 航点 → 航段
cd business/server && cargo test --test api_smoke plan_lights
cd business/server && cargo test --test api_smoke night_window
cd business/server && cargo test --test api_smoke task_light_night_only
cd business/backpack-sim && python3 run.py smoke     # SIM_NIGHT=1 时能看到整条链路
```

`deploy/backpack-sim/mock_agent.py` 的 `/api/lights` 与板端字段一致：`SIM_NIGHT=1` 钉死
夜间、`SIM_NIGHT=0` 钉死白天（验证「仅夜间」白天跳过），不设按本地时间窗。
