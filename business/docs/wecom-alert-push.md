# 企业微信群机器人告警推送

> 状态：已上线（2026-11 落地并端到端验证）
> 代码：`server/src/notify.rs`（核心）、`server/src/handlers/notifications.rs`（接口）、
> `server/migrations/019_notifications.sql`（表）
> 需求来源：执行任务时出现异常告警，要推到企业微信群，值班的人手机能收到

本文是这份功能的**设计与扩展说明**，不是使用手册。使用手册在
[`../README.md`](../README.md) 的「企业微信告警推送」一节；本文记录的是
「为什么这么做」「参数在哪」「以后要加东西该改哪里」。

---

## 1. 先说结论：能做什么，不能做什么

| 能 | 不能 |
| --- | --- |
| 把告警 / 任务异常以 markdown 卡片推到**一个**企业微信群 | **用代码创建群机器人** —— 官方没有这个接口，只能在企微客户端里手动建 |
| 按级别过滤、按业务类型分开关、夜间只推严重 | 接收群里的回复（webhook 单向，只能发） |
| 失败时在页面上看到企业微信返回的 `errcode` | 保证送达（企业微信不返回 ACK，只有 `errcode: 0`） |

**"建机器人"这一步是人工的**，代码只负责「配了地址就发」。所以需求里
"在一个群聊创建一个机器人"落地成：文档教人建 → 拿到 webhook → 填环境变量 →
页面点「发送测试消息」验证。这三步之外的一切都自动化了。

### 建机器人（一次性人工操作）

1. 企业微信 PC 端 → 目标群 → 右上角「···」→ **添加群机器人** → 新建 → 起名 → 添加
2. 弹窗里点**复制地址**，形如
   `https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=94fdb35e-...`
3. 关掉窗口即完成

前置条件（三条任一不满足就看不到入口）：内部群（带「外部」标签的群不支持）、
群没开「仅群主/管理员可管理」、企业管理员没在管理后台禁用群机器人。

坑：
- **地址就是全部凭据**，无加签，谁拿到谁能往群里发消息
- 只有创建者能看到 webhook（其他人点开机器人详情看不到）
- 删掉机器人 → 地址永久失效，重新添加得到**新 key**
- 一个机器人绑一个群；要发两个群就建两个机器人

---

## 2. 架构

```
                    ┌─ gateway.rs  设备事件 error/fault/obstacle
                    ├─ gateway.rs  心跳看门狗（设备掉线）
  告警落库后的 5 处 ─┼─ gateway.rs  控制权抢占 AL-CTRL-*
      调用点        ├─ ai_patrol.rs AI 识别检出
                    └─ handlers/alerts.rs 手动新建
                              │
                              ▼
                   notify::alert_created(state, alert_code)
                              │  ① 查 notifications 开关（DB）
                              │  ② 按 alert_code 回查设备/园区
                              │  ③ 渲染 Notice + 去重 → try_send
                    ┌─────────┴──────────┐
   任务异常 3 处 ────┤ notify::task_abnormal(task_code, level, reason)
   （客户端上报 / 看门狗 / 手动置异常不做）   │
                              ▼
                    mpsc::channel(256)   ← 同步 try_send，永不 await
                              │
                              ▼
                      单个 consumer task
                              │  · 令牌桶限流 18/min（@all 占 2 个额度）
                              │  · 4096 字节截断
                              │  · 45009 退避重试 / 9xxxx 直接放弃
                              ▼
                    POST qyapi.weixin.qq.com/cgi-bin/webhook/send
                              │
                              ▼
                    notifications 表（sent / failed + errcode）
```

### 三条不可让步的设计约束

1. **绝不在 WS handler / 看门狗里 await 外网 HTTP。**
   一次 `qyapi` 超时是 10s，`handle_heartbeat` 是每 10s 一帧的心跳路径，
   同步发消息会直接把设备网关拖垮。所以入队用 `try_send`，队列满就丢并计数。

2. **告警写入点必须收敛成一个出口。**
   改造前 5 处各写各的 SQL、各自生成 `alert_code`（`AL-WS-*` / `AL-HB-*` /
   `AL-CTRL-*` / `AL-{next_id:03}`）。推送如果跟着复制 5 遍，半年后就没人说得清
   "哪条告警会进群"。现在调用点只写一行 `notify::alert_created(state, &code)`，
   消息内容一律由 `alert_code` 回查得到 —— 调用点不需要知道消息长什么样。

3. **webhook 只从环境变量读。**
   地址即凭据，写进 `server.toml` 就等于进 git。为兼容性保留了 `[wecom] webhook`
   字段，但 `Config::load()` 检测到「值来自文件而非环境变量」会打一条 warn。

---

## 3. 代码地图

| 位置 | 职责 |
| --- | --- |
| `notify.rs:39-43` | 三个常量：`MARKDOWN_LIMIT=4000` / `TEXT_LIMIT=2000` / `RATE_PER_MIN=18` |
| `notify.rs` `enum Level` | `Info < Warning < Critical`，`parse()` 容错（未知→警告） |
| `notify.rs` `enum Channel` | `Alert` / `Task`，各绑一对 settings 键 |
| `notify.rs:142` `Notice::markdown()` | 渲染，只使用企微支持的语法 |
| `notify.rs:177` `Notice::mention_text()` | @所有人用的单行文本 |
| `notify.rs:193` `truncate_bytes()` | 按字节截断且不切半个汉字 |
| `notify.rs:217` `struct Dedupe` | 5 分钟静默窗，键 `alert:{code}` / `task:{code}` |
| `notify.rs:243` `struct RateGate` | 固定窗口令牌桶 |
| `notify.rs:313` `Notifier::channel()` | 装配句柄 + 消费端；无 webhook 返回 `None` |
| `notify.rs:355` `Notifier::enqueue()` | **同步**：级别过滤 → 去重 → `try_send` |
| `notify.rs:394` `run_consumer()` | 单消费者循环，`select!` 收消息 + 60s tick 刷溢出汇总 |
| `notify.rs:470` `deliver()` | @all 那条 + markdown 那条，含重试与落库 |
| `notify.rs:551` `post()` | HTTP + `errcode` 分类（45009 / 其他 / 传输错误） |
| `notify.rs:647` `channel_enabled()` | 读两个 settings 键，缺行视为开启 |
| `notify.rs:684` `alert_created()` | 告警入口 |
| `notify.rs:735` `task_abnormal()` | 任务异常入口 |
| `notify.rs:787` `send_test()` | 同步发测试消息，返回企微原文 |
| `handlers/notifications.rs` | `GET /api/notifications`、`POST /api/notifications/test` |
| `config.rs` | `wecom_enabled` / `wecom_webhook` / `wecom_min_level` / `wecom_mention_all` / `public_url` |
| `main.rs` | 装配 `Notifier`，`notify::spawn(state, rx)` |
| `seed.rs` | 播种 `wecom_alert_push` / `wecom_task_abnormal` 两个开关 |
| `web/src/views/settings/index.vue` | 开关 + 「发送测试消息」+ 最近 5 条推送记录 |

### 触发点清单

| # | 触发 | 位置 | 级别 |
| --- | --- | --- | --- |
| 1 | 设备事件 `error` / `fault` / `obstacle` | `gateway.rs:1587` | `error`→严重，`fault` 看 `severity>=2` |
| 2 | 心跳超时被置离线 | `gateway.rs:625` | 警告 |
| 3 | `ctrl_source=3` 控制权被抢 | `gateway.rs:1363` | 警告 |
| 4 | AI 识别检出 | `ai_patrol.rs:453` | 警告 |
| 5 | 页面手动新建告警 | `handlers/alerts.rs:109` | 按填写 |
| 6 | 客户端上报任务 `异常` | `gateway.rs:1492` | 警告 |
| 7 | 任务看门狗判异常 | `gateway.rs:752` | 设备离线→**严重**，否则警告 |

**故意不推的**：急停解除 / 控制权收回 / 告警被标记已处理（状态反转，纯刷屏）；
运营在页面上手动把任务改成异常（自己点的，不用通知自己）。

---

## 4. 数据模型

### `notifications` 表（migration 019）

```sql
id, channel('wecom'), ref_type('alert'|'task'|'summary'|'test'), ref_id,
level('严重'|'警告'|'提示'), dedupe_key, payload,       -- payload = 实际发出的 markdown
status('sent'|'failed'), attempts, last_error, created_at, sent_at
```

为什么要落库而不是只打日志：现场问得最多的两件事是「告警到底推没推出去」和
「为什么群里没看到」。`journalctl` 里翻不到三个月前的失败记录，`last_error`
里存的是企业微信原文（`errcode 93000: invalid webhook url`），照着就能改配置。

`payload` 存**已截断的实际内容**，排查时能直接对着群里那条消息比对。

### 新增的 settings 键（`notification` 组）

| key | 默认 | 说明 |
| --- | --- | --- |
| `wecom_alert_push` | `true` | 设备异常告警推企业微信 |
| `wecom_task_abnormal` | `true` | 任务执行异常推企业微信 |

`seed.rs` 用 `INSERT OR IGNORE` 每启动跑一次，所以**老库升级后自动补上**，
不需要手写迁移。

### 三层开关（都得为真）

```
wecom.enabled + ROBODOG_WECOM_WEBHOOK   部署级：这套部署有没有这个通道
alert_push / task_timeout_notice        运营级：站内提示总开关
wecom_alert_push / wecom_task_abnormal  渠道级：这一类事件推不推企微
```

拆三层是因为职责不同：改地址要重启进程（配置），改"这个班次别吵了"是值班长在
页面上点（DB）。`channel_enabled()` 每次入队都查一次库，所以页面改完**立即生效**，
代价是每次告警多 1 条索引查询（SQLite 亚毫秒，告警频率下无所谓）。

缺行视为**开启** —— 新键在旧库还没播种时，不能把告警静默掉。

---

## 5. 消息模板

```
## 🔴 <font color="warning">严重</font> · 设备异常告警
> 设备：RD-002
> 园区：张江智造园区
> 时间：2026-11-15 18:37:47
> 告警码：AL-006

**内容**：导航指令非法（13440），航点 3 无法到达

[查看详情 →](http://39.107.55.13:3100/alerts)
```

任务异常多两行（任务名+编号、进度）和一段**处置建议**，链接指向
`/tasks?focus={id}`。链接的前缀来自 `ROBODOG_PUBLIC_URL`，**不配就不带链接**
（猜一个地址比不给更容易误导人）。

### 企业微信的硬约束 → 代码里的处理

| 约束（官方） | 处理 | 参数 |
| --- | --- | --- |
| 20 条/分钟/机器人，超了 `45009` | 本地令牌桶 18/min，超出的**排队不丢** | `RATE_PER_MIN` |
| markdown 正文 ≤ 4096 **字节**（中文 3 字节/字），超了整条被拒 `40058` | 按字节截断 + `…（已截断）`，断言不切半个汉字 | `MARKDOWN_LIMIT=4000` |
| text ≤ 2048 字节 | 同上 | `TEXT_LIMIT=2000` |
| markdown **不支持 @ 提及** | `mention_all` 时**另发一条 text** 带 `mentioned_list:["@all"]` | `wecom_mention_all` |
| 只能发不能收 | 消息带链接，处理仍在控制台 | — |
| 支持语法有限 | 只用 `##` / `>` / `**粗**` / `[文字](url)` / `<font color>`；**不用 `-` 列表**（有单测守着） | — |

`<font color>` 只认三个值，映射：`严重→warning`（橙红）、`警告→comment`（灰）、
`提示→info`（绿）。所以严重/警告靠 emoji + 颜色双重区分。

---

## 6. 去重、限流、重试、溢出

这四个是"不被现场骂"的关键，参数都在 `notify.rs` 顶部。

| 机制 | 行为 | 为什么必须 |
| --- | --- | --- |
| **去重** | 同 `dedupe_key` 5 分钟内只放行一次（`DEDUPE_WINDOW`） | 心跳看门狗 15s 一轮、客户端 5s 一报进度，同一件事会被"发现"很多次；且看门狗和客户端上报都会判任务异常，靠它合并成一条 |
| **限流** | 令牌桶 18/min，满了 `sleep` 到下一窗口 | 超 20 条/分钟企微直接拒，多余的消息会丢 |
| **@all 计双份** | `cost = mention_all ? 2 : 1` | @all 实际发两条，按"消息条数"占额度 |
| **重试** | `45009` → 等到下一窗口重试一次；传输错误 → 1s/3s 退避共 3 轮；`9xxxx`（key 失效）→ 立即放弃 | 网络抖动值得重试，配置错误重试一万次也没用 |
| **溢出汇总** | 队列满时丢并计数，60s 汇总一条「另有 N 条未逐条推送」 | 宁可在群里说"积压了 47 条"，也不能静默吞掉 |

单消费者串行发送是刻意的：并发发只会更快撞限流，且顺序会乱。
代价是一条慢请求会阻塞后面的消息 —— 队列 256 深，10s 超时 × 最坏情况也可接受。

---

## 7. 排查手册

### 页面

「系统设置 → 企业微信群机器人」：右上按钮发测试消息（同步发，失败直接显示 errcode），
下面列最近 5 条推送记录（带 `last_error`）。

### SQL

```sql
-- 最近 20 条推送：失败的看 last_error
SELECT created_at, status, level, ref_type, ref_id, attempts, last_error
  FROM notifications ORDER BY id DESC LIMIT 20;

-- 某个告警推了几次、什么时候推的
SELECT * FROM notifications WHERE dedupe_key = 'alert:AL-006';
```

### 接口

```bash
GET  /api/notifications?limit=20&status=failed&refType=task
POST /api/notifications/test
```

### 错误码

| errcode | 含义 | 怎么办 |
| --- | --- | --- |
| `93000` | webhook 地址无效 | 机器人被删了或地址复制不全，重新建一个 |
| `45009` | 超频 | 调低 `RATE_PER_MIN`，或收紧 `min_level` |
| `40058` | 内容超长 | 理论上已被截断兜住；真出现说明 `MARKDOWN_LIMIT` 被改大了 |
| `connection refused` / `timeout` | 服务器出不了公网 | 见 §9「内网部署」 |

### 日志

```bash
journalctl -u rust-admin-robodog -f | grep -i wecom
```
启动时打 `企业微信告警推送已配置 enabled=… min_level=… key_tail=2c1314`
（**只打 key 末 6 位**，够核对是哪一个，又不把凭据写进日志）。
发送失败打 `wecom: 推送失败 key=… error=…`。

---

## 8. 端到端验证记录（2026-11）

真实群里收到 4 条：

1. `curl` 裸测 markdown —— 验证地址可用
2. 🔴 设备异常告警（`AL-006` / `RD-002` / 张江智造园区）—— 验证告警路径
3. 🟠 任务执行异常（`装卸平台早班巡查` / 进度 42% / silent 120s）—— 验证看门狗路径
   （手法：临时库里把任务的 `updated_at` 推回 2020，等看门狗自己判）
4. 🟢 测试消息（`/api/notifications/test`）—— 验证页面按钮

外加一次假 key 测试，页面正确显示
`errcode 93000: invalid webhook url`，且**没有**写进 `notifications` 的 sent 行。

自动化覆盖：`cargo test` 171 通过（含 `notify` 单测 10 个、`config` 单测 18 个，其中 4 个是本次新增的企微配置用例），
`cargo clippy --all-targets` 0 警告，`pnpm typecheck` + `pnpm build` 通过。
单测重点：markdown 只用企微支持的语法（断言不出现 `- ` 列表）、空字段不渲染、
超长必被压到限制内、截断点落在字符边界、去重窗口内外行为、令牌桶容量与等待时间。

---

## 9. 内网部署：发送方选择

企业微信是公网服务（`qyapi.weixin.qq.com:443`），**出站**必须通。

| 部署形态 | 能否直发 | 做法 |
| --- | --- | --- |
| 外网正式环境（`39.107.55.13:3100`） | 能 | 当前实现，业务平台直发 |
| 现场内网（无入站端口，靠反向控制信道拨中台） | **要确认** | 若不能出公网，需要中台代发 |

中台代发的改动面（预埋好了）：`notify.rs` 的出口只有 `post()` 一个函数，
将来加一个「转发到中台内部接口」的 `post` 实现即可，其余逻辑不用动 —— 
在 `crates/backend` 加 `POST /api/dogInternal/notify/wecom`，业务平台用已有的
`ROBODOG_SERVICE_TOKEN` 调它。

先确认：在业务平台机器上 `curl -m 5 https://qyapi.weixin.qq.com` 通不通。

---

## 10. 后续可做的（按性价比排序）

| 想法 | 说明 | 量级 |
| --- | --- | --- |
| **推送记录页** | 后端 `GET /api/notifications` 已就绪，设置页现在只显示最近 5 条；做成「告警中心」的一个页签，支持按状态/类型筛选、手动重发 | 半天 |
| **多群路由** | 严重 → 运维群，一般 → 值班群。现在 `Notifier` 只持有一个 `tx`，扩成 `Vec<(Channel, Sender)>` + 每个目标一套限流；配置从单 `webhook` 变成 `[[wecom.targets]]` | 1 天 |
| **恢复通知** | 设备恢复在线 / 急停解除 / 控制权收回时补一条"已恢复"。要解决"给哪条消息回复"的问题（企微不支持消息引用），建议用独立的 `ref_type='recover'` + 原 `dedupe_key` 关联 | 半天 |
| **@ 具体人** | markdown 不支持 @，可用 text 的 `mentioned_list:[userid]` / `mentioned_mobile_list:[手机号]`。需要维护"哪个园区找谁"的映射表 | 半天 |
| **夜间静音联动** | `night_silent` 开关已存在于 settings，可以在 `channel_enabled()` 里带上「22:00-07:00 只放行严重」 | 2 小时 |
| **告警聚合** | 同一设备连续 N 条告警折叠成一条"设备 X 在过去 10 分钟触发 N 次告警" | 半天 |

### 想加一个告警源

三种情况，按代价从低到高：

1. **新代码里 insert 告警** → 落库后加一行：
   ```rust
   crate::notify::alert_created(state, &alert_code).await;
   ```
   前提：`alert_code` 必须**全局唯一**（`alerts.alert_code` 有 UNIQUE 约束）。
   ⚠ 历史上 `handle_device_event` 用的是 `AL-WS-{日期}-{小时}`，不带设备号，
   导致同一小时全平台第二条故障插入就失败 —— 已改为 `AL-WS-{device_id}-{秒级时间戳}`。
   **新写的 alert_code 一定要带设备号和秒级/毫秒级时间戳。**

2. **不是告警，但是业务上"出事"** → 直接构造 `Notice` 入队，参考 `task_abnormal`：
   ```rust
   state.notifier.enqueue(Notice { level, ref_type: "…", dedupe_key, title, fields, .. });
   ```
   记得给 `dedupe_key` 一个能代表"同一件事"的键，否则看门狗会刷屏。

3. **新的推送渠道（钉钉/飞书/短信）** → 不动 `Notice` 和调用点，只加一个
   `enum Channel` 分支和一个 `post_xxx()`；`notifications.channel` 列已经预留。

---

## 11. 踩过的坑（别再踩）

| 坑 | 现象 | 结论 |
| --- | --- | --- |
| 告警 `alert_code` 不含设备号 | 同一小时第二条故障**静默丢失** | UNIQUE 约束 + `.ok()` 吞错 = 无声数据丢失。凡是 `.ok()` 放弃错误的地方都值得回头看 |
| 在 handler 里直接 `reqwest::post().await` | 企微超时 → 心跳处理卡 10s → 设备全掉线 | 外网调用一律入队 |
| markdown 里用 `- ` 写列表 | 群里显示字面量 `- xxx` | 群机器人 markdown 只支持部分语法 |
| 假设 markdown 能 @ | @ 不生效，没人收到提醒 | 必须另发 text + `mentioned_list` |
| 长文本直接发 | `40058 content exceed max length` 整条被拒 | 企微按**字节**算，中文 3 字节/字 |
| 前端传 `ref_type` 后端收 `refType` | 筛选静默不生效 | 这个项目里模型统一 `#[serde(rename_all="camelCase")]`，Query 结构体也要加 |
| 改 `AppState` 加字段 | 7 个集成测试编译失败 | 新增字段记得改 `tests/*.rs` 的 `AppState { .. }` 字面量；`Notifier::disabled()` 就是为这个准备的 |

---

## 12. 部署清单

```bash
# 1. 服务器能出公网
curl -m 5 -sS https://qyapi.weixin.qq.com -o /dev/null && echo ok

# 2. 填 webhook（robodog.env 权限 600，不进 git）
ssh root@39.107.55.13
vi /opt/hz/rust-admin/robodog.env
#   ROBODOG_WECOM_WEBHOOK=https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=xxx
#   ROBODOG_PUBLIC_URL=http://39.107.55.13:3100

systemctl restart rust-admin-robodog
journalctl -u rust-admin-robodog -n 20 | grep 企业微信   # 应看到"已配置 … key_tail=xxxxxx"

# 3. 页面验证：系统设置 → 企业微信群机器人 → 发送测试消息
```

`deploy/external-hz/install.sh` 的 `robodog.env` 模板已带注释占位；
`write_env` 遇到已存在的文件会**保留**，所以重新发布不会冲掉现场填的 key。
`deploy/external-hz/config/robodog.server.toml` 已配好 `public_url` 与 `[wecom] enabled`。

### 相关环境变量

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `ROBODOG_WECOM_WEBHOOK` | 空 | webhook 地址。空 = 整个通道关闭 |
| `ROBODOG_WECOM_ENABLED` | `true` | 部署级总开关 |
| `ROBODOG_WECOM_MIN_LEVEL` | `警告` | 推送下限（`严重`/`警告`/`提示`） |
| `ROBODOG_WECOM_MENTION_ALL` | `false` | 严重级别是否 @所有人（占双倍额度） |
| `ROBODOG_PUBLIC_URL` | 空 | 控制台对外地址，拼消息里的链接 |
