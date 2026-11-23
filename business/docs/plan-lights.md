# 计划灯光（补光灯 / LED）：配置、链路与排查

板子 `.246` 上的两盏灯（背包板补光灯 GPIO、LED 灯带）现在可以随计划自动开关：
**航点动作**决定「到这个点开哪几盏」，**计划**决定「是不是只在夜里开」。

```text
路线编辑器（航点动作）
  lightFill = 开补光灯 / lightLed = 开 LED 灯带
        │  写进 routes.route_data.points[].actions[]（JSON，无需迁移）
        ▼
计划（tasks.light_night_only，迁移 030）
  └─ 勾了「仅夜间自动开灯」→ 载荷 task_dispatch.lightNightOnly = true
        │
        ▼
client（与 Agent 同机）
  5009 patrol.point_arrived / patrol.waiting（detail_index）
        │  POST {fill, led, nightOnly, taskCode, pointIndex}
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
| 计划级开关只下发布尔值 | 客户端只搬运 `nightOnly`，不自己算时间，避免「两地各说一套」 |
| 到点开、离开关 | 与 AI 窗口同一条收口规则（阶段不再是 `patrol.point_*`），不额外引入状态机 |
| 看门狗在板端 | 客户端掉线 / 被 kill / 断电重启时，灯不能一直亮着（4G 背包耗电扛不住） |

## 2. 配置在哪

- **航点动作**（路线编辑器 → 点动作 → 「到点开灯」）：`lightFill` / `lightLed` 两个
  复选框，与动作类型（AI 识别、可见光拍照…）无关——不配 AI 也能只开灯。多个动作合并
  为「或」（任一要求开灯就开）。
- **计划**（计划表单 → 「夜间开灯（补光灯 / LED）」）：`lightNightOnly`。
  勾上 `true` = 只在夜间时段开；不勾 `false` / 老平台不发 = 到点就开。
- **板端时间窗**（`backend/config.py`）：`LIGHT_NIGHT_START` / `LIGHT_NIGHT_END`
  默认 `18:00` / `06:00`，**板端本地时间**，允许跨零点；**起止相同 = 关闭夜间判定**
  （退化成「到点就开」，现场临时要白天也开灯时用）。`LIGHT_MAX_ON_SECS`（默认 900s）
  是计划灯的看门狗上限。

## 3. 灯怎么开关

- 补光灯：`/sys/class/gpio/gpio<FILL_LIGHT_GPIO>` 写 `1`/`0`（默认 GPIO 46）。
  `FILL_LIGHT_GPIO=-1` 或与 `CAM_TRIGGER_GPIO` 同号时**拒绝执行**并回错误——但不能因为
  补光灯不可用就不开 LED（`lights_apply` 会继续点 LED，返回 `ok=false` + `errors`）。
- LED 灯带：厂商只提供 `test_rgbrun`（**循环闪烁，没有常亮接口**），所以「开」= 拉起
  进程、「关」= 按进程组 `SIGTERM`。进程句柄 `_LED_PROC` 由 `control.py` 持有，
  **手动按钮（`/api/led/blink`）与计划共用**，否则计划结束时杀不掉手动拉起的那一个。
- 关灯时机：离开航点 / 到达无灯光动作的航点 / 任务终态 / 任务取消 / 本地跑收尾 /
  看门狗超时。客户端 `PointLights::leave` 只在**自己真的开过灯**时才发 `off`
  （白天跳过、Agent 不可用都不发），以免顺手关掉操作员手动开的灯。
- 手动灯的看门狗豁免：`/api/filllight`、`/api/led/blink` 不开 `managed`，一直亮着。

## 4. 代码地图

| 位置 | 作用 |
| --- | --- |
| `business/backpack_dashboard/backend/control.py` | `is_night` / `night_window` / `lights_apply` / `lights_off` / `lights_status` / 看门狗 |
| `business/backpack_dashboard/backend/agent.py` | `GET/POST /api/lights`、`POST /api/lights/off`；手动接口保持原样 |
| `business/backpack_dashboard/backend/tools/lights_selftest.py` | 时间窗 / 白天跳过 / GPIO+LED / 看门狗 自检（不需要板子） |
| `business/server/migrations/030_task_light_night_only.sql` | `tasks.light_night_only`（默认 0） |
| `business/server/src/dispatch.rs` + `gateway_types.rs` | 载荷 `lightNightOnly`（总是下发，`false` 也发） |
| `business/client/src/lights.rs` | 解析动作 / 到点开灯 / 离开关灯 / 去重；`lightNightOnly` 透传 |
| `business/client/src/main.rs` | 5009 到点、离开、终态、取消、本地收尾的挂钩 |
| `business/web/src/views/routes/index.vue` | 航点动作两个复选框 + 动作卡片「灯光：…」 |
| `business/web/src/views/tasks/index.vue` | 计划「仅夜间自动开灯」 |
| `deploy/backpack-sim/mock_agent.py` | 模拟 `/api/lights`（`SIM_NIGHT=1/0` 可钉死白天/夜间） |

## 5. 现场排查

1. 灯完全不亮：
   - `curl -s 127.0.0.1:8000/api/lights`：看 `fill_available`（GPIO 被禁/同号时为
     false）、`led_bin_exists`、`night`、`night_window.enabled`；
   - `POST /api/lights` 手工打一发，看返回 `errors`；
   - 客户端日志 grep `航点灯光`：`开灯失败 / 关灯失败` 会带 Agent 的原因。
2. 白天不亮（预期行为）：计划勾了「仅夜间自动开灯」，且当前不在时间窗内 →
   `POST /api/lights` 返回 `skipped=daytime`，客户端日志有「在白天到达，跳过开灯」。
   要白天也开：计划取消勾选，或把 `LIGHT_NIGHT_START`/`LIGHT_NIGHT_END` 设成相同值。
3. 灯一直亮着：先看客户端有没有收到任务终态/取消（阶段卡在 `patrol.point_*` 就不会
   发 `off`）；最迟 `LIGHT_MAX_ON_SECS` 后板端看门狗会关。手动按钮开的灯本来就不看门狗。
4. LED 关了但还在闪：`test_rgbrun` 自己 fork 了子进程。`led_off` 用进程组杀，
   若厂商程序改过（不再继承进程组），需要在板端 `pkill -f test_rgbrun` 兜底。

## 6. 回归

```bash
python3 business/backpack_dashboard/backend/tools/lights_selftest.py
cd business/client && cargo test lights::            # 解析 / 开灯去重 / 白天跳过 / 关灯
cd business/server && cargo test --test api_smoke task_light_night_only
cd business/backpack-sim && python3 run.py smoke     # SIM_NIGHT=1 时能看到整条链路
```

`deploy/backpack-sim/mock_agent.py` 的 `/api/lights` 与板端字段一致：`SIM_NIGHT=1` 钉死
夜间、`SIM_NIGHT=0` 钉死白天（验证「仅夜间」白天跳过），不设按本地时间窗。
