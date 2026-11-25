# 计划语音播报（航点动作 / 行进任务）：配置、链路与排查

「到点了让狗喊一句 / 一路走一路喊」这件事的完整说明。三处代码：

| 在哪 | 文件 | 职责 |
|---|---|---|
| 平台编辑器 | `business/web/src/views/routes/index.vue`（`isSpeakAction` 一段） | 配话术 + 循环时长，按板端白名单校验 |
| 设备客户端 | `business/client/src/speak.rs` | 跟 5009 状态机起停（到点 / 进段 / 走完整段 / 终态） |
| 板端 Agent | `business/backpack_dashboard/backend/speak.py` | 写聆思命令 FIFO（`call 0/1 <内容>` / `call stop`） |

---

## 1. 两个动作

| 动作 | 挂在哪 | 字段 | 行为 |
|---|---|---|---|
| 语音播报 | 航点动作（`points[i].actions[]`） | `speakText` / `speakSeconds`（0~600） | 到达该航点播报；`speakSeconds > 0` 时循环播这么久 |
| 语音播报 | 行进任务（`legs[i].actions[]`、`returnLeg.actions`） | `speakText` / `speakSeconds`（默认 120） | **进入该航段时**开始播（= 到达起点航点），**走完整段**（= 到达下一个航点）/ 终态 / 取消停 |

- **航段播报的收口点是「到达下一个航点」，不是「离开起点航点」**：离开起点航点正是这一段
  刚开始，在那里 stop 等于「配了也一秒都听不到」（2026-11-25 现场 gh-test `lrc`：
  P2→P3 配了航段播报，狗一出 P2 就静音）。代码里靠 `speak.rs::Scope` 区分：
  `Point` 离开航点停、`Leg` 到下一个航点才停。

- **航段的下标语义**：`legs[i]` 是 P(i+1) → P(i+2)，所以它的「起点航点下标」是 `i`；
  `returnLeg` 的起点是最后一个航点。客户端按这个映射把「航段播报」挂到到点事件上。
- **同一点两处都配**：航点动作优先，航段那条忽略（板端一次只能播一条，合并容易超长）。
- **「静默通行」不是播报**：它是航段动作里的反义标记（画面提示用），既不播报也不
  屏蔽别的动作——播报只由显式的「语音播报」触发，没有默认会响的东西。
- **老数据**：没有 `speakText` 的动作不播报。动作库里 `param = "安全提示语音 01"`
  只是占位名，客户端不会把它念出来（`speak.rs::action_text` 里挡掉了）。

## 2. 链路

```text
路线动作 speakText / speakSeconds
  → task_dispatch 的 routeData（原样下发，服务端不加工）
    → 设备客户端（5009 状态机，`client/src/speak.rs`）
        到点 / 进段: POST 板端 /api/speak {text, loop, seconds, planId, pointIndex, scope}
        航点播报离开航点 / 航段播报到达下一个航点 / 终态 / 取消: POST 板端 /api/speak/stop {reason}
      → 板端 Agent（`speak.py`）
        call 0 <内容>   单次播报（说完 Agent 主动 call stop 回到 voice）
        call 1 <内容>   循环播报（到 seconds / 看门狗 SPEAK_MAX_SECS 自动停）
        call stop       停止并回到 voice 模式
          → 聆思 asr 服务的命令 FIFO（`LINGS_CMD_FIFO`，默认 /tmp/voice_assistant_cmd）
```

- **TTS 在云端**：播报前 Agent 调 `ensure_asr()`；asr 没起 / 没联网时回
  `ok=false + error`，客户端只打一行日志，**巡检继续**。
- **一次只播一条**：板端 FIFO 是单通道，新 `call` 会打断旧播报。所以 Agent 每次 start
  先显式 `call stop` 再起新的；客户端也不需要「并发播报」。
- **为什么要收尾**：循环播报必须由客户端收口，否则狗走了还在原地喊。板端看门狗
  （`SPEAK_MAX_SECS`，默认 600s）只是兜底（客户端被 kill / 掉线）。**什么时候收**看动作
  挂在哪儿：航点播报 = 离开该航点；航段播报 = 到达下一个航点（这一段走完）/ 终态 / 取消。
- **单次播报也要回到 voice**：Agent 按文本长度估时（`0.4 × 字数 + 3s`）后主动
  `call stop`——一直留在 call 模式既占网络，也让「小谷小谷」唤醒词失效。
- **顺序敏感**：客户端对播报是**串行 await**（不像开灯那样 spawn），
  否则「离开航点停播」可能抢在「到点开播」前面落地，把新播报掐掉。

## 2.1 手动「呼叫」（任务详情页喊话）

除了路线里的自动播报，**任务详情页**还有一个「远程呼叫」：操作员在计划执行过程中填一句话，
狗当场用 TTS 喊出来（现场疏导、通知撤离、找人）。

```text
web 任务详情「呼叫」
  → POST /api/tasks/{id}/call {text, loop, seconds}     （server/src/handlers/tasks.rs::call_task）
    → 设备指令 `呼叫`（ws：push_command，参数 params.params.{text,loop,seconds}）
      → 设备客户端（client/src/handlers/command.rs → speak::manual_call）
        → 板端 POST /api/speak/call {text, loop, seconds, manual:true}
          → speak.py::start（同一套净化 / 同一个看门狗，**不做幂等**）
输出「停止」→ POST /api/tasks/{id}/call/stop → 设备指令 `停止呼叫` → 板端 /api/speak/stop
```

- **不去重**：航点播报的幂等是为了 5009 每 500ms 一帧不重播；人工呼叫正好相反——
  同一句话再点一次就是要再喊一遍（板端 `manual=true` 用时间戳做幂等键）。
- **循环默认 30s**：平台把 `seconds` 夹在 5~600；不传时默认 30（板端对 `manual` 也是 30），
  免得点一下就在园区里喊十分钟。要停就点「停止呼叫」——板端 FIFO 单通道，
  所以这也会停掉该设备上正在循环的航点播报，那正是操作员想要的。
- **不看本体门禁**：呼叫只跟板端 Agent 说话（客户端与它同机），本体关机 / 断链也要能喊，
  所以服务端不过 `ensure_ready`、客户端把它放在本体门禁之前。
- **设备不在线要显式失败**：`wsDelivered=false` 时服务端直接 400「未送达」，
  不会留下一条假的「已下发」。
- **留痕**：每次呼叫写一条 `task_events(event_type='call')`，任务详情的事件流里能看到
  “谁在什么时候喊了什么”。

## 3. 内容限制（板端固件）

| 规则 | 值 | 在哪实现 |
|---|---|---|
| 不能有空格 | 所有空白都去掉 | `speak.py::normalize` / `speak.rs::sanitize_text` |
| 只认中文汉字 + 中文标点 + 数字 | `，。！？、；：（）《》〈〉「」『』【】…—·` + `0-9` | 同上 |
| 中文汉字上限 | `SPEAK_MAX_CHARS` = 30 | 同上 |
| 总字符上限 | `SPEAK_MAX_LEN` = 40 | 同上 |

编辑器在保存时**硬拦**（提示哪些字符不支持），板端只在运行期「丢掉非法字符 + 截断」，
不会因为配歪了就报错——现场最怕的是「任务因为一句话跑不起来」。

⚠️ 这份白名单同时是**命令注入的护栏**：FIFO 那条路是 `echo "{text}" > fifo`（shell），
放开字符集之前先想清楚这一点。

## 4. 代码地图

| 位置 | 内容 |
|---|---|
| `web/src/utils/constants.ts` | `SPEAK_PRESETS`（常用话术）、`SPEAK_MAX_*`、`SPEAK_LEG_DEFAULT_SECS` |
| `web/src/utils/speak.ts` | `normalizeSpeakText`（路线编辑器与任务详情「呼叫」共用同一份净化/校验） |
| `web/src/views/routes/index.vue` | `isSpeakAction`、`actionSpeakLabel`、对话框「播报内容 / 循环播报时长」 |
| `web/src/views/tasks/index.vue` | 任务详情「远程呼叫（喊话）」`sendCall` / `stopCall` |
| `web/src/api/types.ts` | `RoutePointAction.speakText` / `speakSeconds`、`DeviceCommandResult` |
| `client/src/speak.rs` | `parse_speak_plan`、`sanitize_text`、`Speaker::{start_task, point_arrived, leave_point, finish_task}`、`manual_call` / `manual_stop` |
| `client/src/pump.rs` | 5009 到点 / 离开 / 终态三处调用（与 AI、灯光、录像同一处收口） |
| `client/src/handlers/command.rs` | 下发时 `start_task`、本地任务收尾 / 取消时 `finish_task`；`呼叫` / `停止呼叫` 指令 |
| `client/src/config.rs` | `--no-speak` / `ROBODOG_SPEAK_ENABLED=0` |
| `backpack_dashboard/backend/speak.py` | `normalize` / `start` / `stop` / `status` + 看门狗（`manual=true` = 人工呼叫） |
| `backpack_dashboard/backend/agent.py` | `POST /api/speak`、`POST /api/speak/call`、`POST /api/speak/stop`、`GET /api/speak/status` |
| `deploy/backpack-sim/mock_agent.py` | 同四个接口的模拟实现（`/api/sim/state` 里也能看到） |

## 5. 现场排查

| 现象 | 先看什么 |
|---|---|
| 到点没声音 | `journalctl -u backpack-agent | grep '\[speak\]'`：有没有「单次播报 / 循环播报」这一行；没有 = 客户端没下发（看客户端日志 `语音播报：…`） |
| 航段配了播报却不响 | 客户端日志里有没有 `scope=leg` 的那条 `语音播报：…`：有 = 板端没播（asr / 网络）；没有 = 客户端没解析到（看 `含 N 个播报航点，M 个播报航段`）或该点同时配了航点播报（航点优先） |
| 航段播报刚响就被掐 | 以前是「离开起点航点就 stop」（2026-11-25 已修）。现在只有到达下一个航点 / 终态 / 取消才停；若现场仍被掐，先确认客户端版本，再看是不是 `speakSeconds` 到期（默认 120s，长航段可调大） |
| 播报「半句话」 | 内容里有英文 / 特殊符号被板端丢掉了。看 Agent 日志「内容已净化：丢弃 N 个不支持的字符」，回编辑器改文案 |
| 报「asr 未启动」 | 板端没网或 `systemctl status asr` 挂了；播报是云端 TTS，**断网就是不能播** |
| 一直在喊 | 客户端没发 stop（客户端被 kill / 掉线）；等 `SPEAK_MAX_SECS` 看门狗，或 `curl -XPOST :8000/api/speak/stop -d '{"reason":"manual"}'` |
| 播报卡在 call 模式（唤不醒） | 同上：单次播报会在估时后自动 `call stop`；手工 `POST /api/speak/stop` 立即恢复 |
| 老平台 / 老板子 | 板端 Agent 没升级时客户端记 `unsupported`（一轮任务只试一次），升级 `backpack-agent` 后自动恢复 |
| 详情页点了「呼叫」没反应 | 报「未送达」= 设备客户端没连（看业务平台设备在线状态）；报「内容不能为空/只支持中文…」= 文案里有英文/空格，改完再点；接口 200 但没声音按上面「到点没声音」查 |

手工验接口（板端同机）：

```bash
curl -s 127.0.0.1:8000/api/speak/status
curl -s -XPOST 127.0.0.1:8000/api/speak \
  -H 'Content-Type: application/json' \
  -d '{"text":"巡检中，请注意避让","loop":true,"seconds":30,"planId":"TK-1","pointIndex":0}'
curl -s -XPOST 127.0.0.1:8000/api/speak/call -H 'Content-Type: application/json' \
  -d '{"text":"请避让","loop":false}'
curl -s -XPOST 127.0.0.1:8000/api/speak/stop -d '{"reason":"manual"}'
```

## 6. 回归

```bash
# 板端：内容净化 / 幂等 / 循环看门狗 / 收尾（不需要板子、聆思、网络）
python3 business/backpack_dashboard/backend/tools/speak_selftest.py

# 客户端：路线解析 / 到点播报 / 离开停播 / 终态收口 / 老 Agent 退让 / 手动呼叫不去重
cd business/client && cargo test speak:: && cargo test waypoint_speech

# 业务平台：呼叫下发到设备的契约（离线报未送达 / 参数透传 / 事件留痕）
cd business/server && cargo test --test api_smoke task_call_dispatches_speak_to_the_device

# 平台编辑器：类型 + 构建
cd business/web && pnpm build
```

用 `deploy/backpack-sim/` 在本机复刻板端时，`python3 run.py smoke` 之外还可以直接
`curl 127.0.0.1:8000/api/speak/status` 看假 Agent 收到了什么（`/api/sim/state` 的
`speak` 字段）。
