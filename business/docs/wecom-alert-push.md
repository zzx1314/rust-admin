# 企业微信群机器人告警推送

> 状态：已上线（2026-11 落地并端到端验证；告警自动派单 2026-11 追加）
> 代码：`server/src/notify.rs`（核心）、`server/src/alert_flow.rs`（告警 → 责任人 → 工单）、
> `server/src/handlers/notifications.rs`（接口）、
> `server/migrations/019_notifications.sql`（推送记录表）、
> `server/migrations/021_alert_dispatch.sql`（告警派单列）
> 需求来源：执行任务时出现异常告警，要推到企业微信群，值班的人手机能收到；
> 并且**告警要落到具体责任人头上**（自动建工单），不能只是群里刷一条消息

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
| 按**当天排班**把告警派给当班责任人、自动建工单，并 `@` 到人 | 猜到「谁真的在现场」—— 名单来自平台里的排班，没排班就只能兜底到园区负责人 |

**"建机器人"这一步是人工的**，代码只负责「配了地址就发」。所以需求里
"在一个群聊创建一个机器人"落地成：文档教人建 → 拿到 webhook → **粘贴到
平台设置页**（或部署方写环境变量）→ 页面点「发送测试消息」验证。
这三步之外的一切都自动化了。

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
                              │  ① alert_flow::on_alert —— 按当天排班找责任人
                              │     并自动建工单（**不看推送开关**，先落单）
                              │  ② 查 notifications 开关（DB）
                              │  ③ 按 alert_code 回查设备/园区
                              │  ④ 渲染 Notice + 去重 → try_send
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

### 四条不可让步的设计约束

1. **绝不在 WS handler / 看门狗里 await 外网 HTTP。**
   一次 `qyapi` 超时是 10s，`handle_heartbeat` 是每 10s 一帧的心跳路径，
   同步发消息会直接把设备网关拖垮。所以入队用 `try_send`，队列满就丢并计数。

2. **告警写入点必须收敛成一个出口。**
   改造前 5 处各写各的 SQL、各自生成 `alert_code`（`AL-WS-*` / `AL-HB-*` /
   `AL-CTRL-*` / `AL-{next_id:03}`）。推送如果跟着复制 5 遍，半年后就没人说得清
   "哪条告警会进群"。现在调用点只写一行 `notify::alert_created(state, &code)`，
   消息内容一律由 `alert_code` 回查得到 —— 调用点不需要知道消息长什么样。

3. **派单不能依赖推送通道。**
   现场很可能根本没配 webhook（内网部署、群机器人还没建），但「告警要有人管」
   不能跟着通道一起失效。所以 `alert_flow::on_alert` 跑在 `notify::alert_created`
   的最前面、`channel_enabled()` 判断之前，推送只是它后面的一条旁路。

4. **webhook 地址仍然不进 `server.toml`，但可以进库。**
   地址即凭据，写进配置文件就等于进 git。2026-11 起改为「平台设置页优先、环境变量兜底」：

   - 存 `settings.wecom_webhook`，`value_type = 'secret'` → **不出现在**
     通用 `GET /api/settings` 列表里，`PUT /api/settings` 直接拒收；
   - 读写走专用接口 `GET/PUT /api/notifications/webhook`，**读只回掩码**
     （`…?key=94fdb35e…`），完整地址不出后端；
   - 地址**不在启动时固化**：`Notifier` 持一个可写槽（`set_webhook`），
     设置页保存后下一条消息就用新地址，**不用重启进程**；
   - 日志只打 key 末 6 位（`notify::key_tail`），和其它凭据一致。

   为什么不再强制只走环境变量：现场改一次地址要 SSH + 改 env + 重启服务，
   而「群机器人被删了、换了个群」是最常见的一次性运维动作，全卡在运维手上。
   只信任部署方锁定的部署，把 `ROBODOG_WECOM_WEBHOOK` 写进 `robodog.env`
   并且**不在页面填**即可（平台配置为空时自动回退环境变量）。

---

## 3. 代码地图

| 位置 | 职责 |
| --- | --- |
| `notify.rs:39-43` | 三个常量：`MARKDOWN_LIMIT=4000` / `TEXT_LIMIT=2000` / `RATE_PER_MIN=18` |
| `notify.rs` `enum Level` | `Info < Warning < Critical`，`parse()` 容错（未知→警告） |
| `notify.rs` `enum Channel` | `Alert` / `Task`，各绑一对 settings 键 |
| `notify.rs:142` `Notice::markdown()` | 渲染，只使用企微支持的语法 |
| `notify.rs` `Notice::mention_text()` | @责任人 / @所有人用的**单行提醒**：只拼级别 + 标题 + 前两个字段 |
| `notify.rs:193` `truncate_bytes()` | 按字节截断且不切半个汉字 |
| `notify.rs:217` `struct Dedupe` | 5 分钟静默窗，键 `alert:{code}` / `task:{code}` |
| `notify.rs:243` `struct RateGate` | 固定窗口令牌桶 |
| `notify.rs:313` `Notifier::channel()` | 装配句柄 + 消费端；无 webhook 返回 `None` |
| `notify.rs:355` `Notifier::enqueue()` | **同步**：级别过滤 → 去重 → `try_send` |
| `notify.rs:394` `run_consumer()` | 单消费者循环，`select!` 收消息 + 60s tick 刷溢出汇总 |
| `notify.rs` `deliver()` | @all 那条 + markdown 那条，含重试与落库；有 `mentions` 时改发 `mentioned_mobile_list` |
| `notify.rs:551` `post()` | HTTP + `errcode` 分类（45009 / 其他 / 传输错误） |
| `notify.rs:647` `channel_enabled()` | 读两个 settings 键，缺行视为开启 |
| `notify.rs` `alert_created()` | 告警入口：**先派单**（`alert_flow`）再推送 |
| `notify.rs` `task_abnormal()` | 任务异常入口：同样带「当班责任人」并 @ 到人 |
| `alert_flow.rs` `on_alert()` | 告警 → 责任人 → 自动建工单 → 回写 `alerts` 四列 |
| `alert_flow.rs` `candidate_score()` | 排班匹配打分（同任务 40 / 同区域 20 / 同园区 10 / 已发布 4 / 有责任人 2 / 在岗 1） |
| `alert_flow.rs` `responsible_for_task()` | 任务异常推送复用同一套匹配（只找人，不建单） |
| `alert_flow.rs` `auto_workorder_enabled()` | 读 `alert_auto_workorder` 开关（缺行视为开启） |
| `handlers/workorders.rs` `create_order_core()` | 建单核心（人工转单 `promote_alert=true`，自动派单 `false`） |
| `notify.rs:787` `send_test()` | 同步发测试消息，返回企微原文 |
| `notify.rs` `stored_webhook()` | 启动时从 `settings` 读平台侧地址（空 = 未配） |
| `notify.rs` `validate_webhook()` | 保存前校验：http(s)、必须带 `key=`、不能有空白 |
| `notify.rs` `mask_webhook()` / `key_tail()` | 读接口回显前 8 位 / 日志只打末 6 位 |
| `notify.rs` `Notifier::set_webhook()` | 热更新地址（设置页保存后立即生效） |
| `notify.rs` `Notifier::webhook_snapshot()` | 当前地址 + 来源（`Platform` / `Env`） |
| `handlers/notifications.rs` | `GET /api/notifications`、`POST /api/notifications/test`、`GET/PUT /api/notifications/webhook` |
| `handlers/settings.rs` | 通用设置列表过滤 `value_type='secret'`，并拒收此类键 |
| `config.rs` | `wecom_enabled` / `wecom_webhook`（环境变量兜底） / `wecom_min_level` / `wecom_mention_all` / `public_url` |
| `main.rs` | `stored_webhook()` → 装配 `Notifier`，`notify::spawn(state, rx)` |
| `seed.rs` | 播种 `wecom_alert_push` / `wecom_task_abnormal` / `alert_auto_workorder` 开关 + `wecom_webhook`（secret，空值） |
| `web/src/views/settings/index.vue` | 开关 + webhook 地址配置（掩码 + 来源）+ 「发送测试消息」+ 最近 5 条推送记录 |

### 触发点清单

| # | 触发 | 位置 | 级别 |
| --- | --- | --- | --- |
| 1 | 设备事件 `error` / `fault` / `obstacle` | `gateway.rs:1587` | `error`→严重，`fault` 看 `severity>=2` |
| 2 | 心跳超时被置离线 | `gateway.rs:625` | 警告 |
| 3 | `ctrl_source=3` 控制权被抢 | `gateway.rs:1363` | 警告 |
| 4 | AI 识别检出 | `ai_patrol.rs:453` | 警告 |
| 5 | 页面手动新建告警 | `handlers/alerts.rs` `create_alert` | 按填写 |
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
| `wecom_webhook` | `''` | **平台内配置的 webhook 地址**（`value_type='secret'`）。空 = 用环境变量；填了就以它为准，保存即生效 |
| `wecom_alert_push` | `true` | 设备异常告警推企业微信 |
| `wecom_task_abnormal` | `true` | 任务执行异常推企业微信 |
| `alert_auto_workorder` | `true` | 告警按当班排班自动派单并建工单（关掉只停建单，责任人解析与推送照旧） |

### `alerts` 派单列（migration 021）

```sql
staff_id, assignee, work_order_id, assignee_note
```

为什么放告警行而不新建表：派单结果是告警的一次性派生数据，独立表只会让告警列表
多一次 JOIN；`work_order_id` 非空同时充当**幂等守卫**（一条告警只派一单）。

`seed.rs` 用 `INSERT OR IGNORE` 每启动跑一次，所以**老库升级后自动补上**，
不需要手写迁移。

### 三层开关（都得为真）

```
wecom.enabled + webhook(平台配置 > 环境变量)  部署级：这套部署有没有这个通道
alert_push / task_timeout_notice        运营级：站内提示总开关
wecom_alert_push / wecom_task_abnormal  渠道级：这一类事件推不推企微
```

拆三层是因为职责不同：改**有没有**通道（`wecom.enabled`）是部署决策，改**发到哪个群**
是运维在设置页里粘地址（DB，立即生效），改"这个班次别吵了"是值班长点开关（DB）。
`channel_enabled()` 每次入队都查一次库，所以页面改完**立即生效**，
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

告警自动派单之后多四行（区域 / 关联计划 / 当班责任人 / 工单），`处置建议` 变成
一段话，链接改指 `/workorders?focus={id}`：

```
## 🔴 <font color="warning">严重</font> · 设备异常告警（已自动派单）
> 设备：RD-002
> 园区：张江智造园区
> 区域：装卸区
> 关联计划：装卸平台早班巡查
> 当班责任人：陈志远（运维一组） · 早班 08:00 - 16:00（SH-012）
> 工单：WO-1115-03（已自动派单）
> 时间：2026-11-15 18:37:47
> 告警码：AL-006

**内容**：导航指令非法（13440），航点 3 无法到达

**处置建议**：已按当班排班派给 陈志远（运维一组） · 早班 08:00 - 16:00（SH-012），
工单 WO-1115-03 已自动生成（当班排班 SH-012），请责任人到场确认并把处置结果回填到工单

[查看详情 →](http://39.107.55.13:3100/workorders?focus=12)
```

有责任人手机号（或开了 `wecom_mention_all`）时，**卡片前面**还会有一条一行 `text`
专门用于 @：markdown 不支持提及，只能另发。它只做提醒、**不重复卡片内容**：

```
【严重】设备异常告警（已自动派单）　设备：RD-002　园区：张江智造园区
```

正文、工单号、处置建议只留在 markdown 卡片里——两边都拼全的话，群里每条告警
都会变成两条几乎一样的消息，现场会以为是重复告警。

「谁去处理」来自**当天排班**（`shifts`）而不是计划（`tasks`）：计划只描述
「哪个设备在什么时候跑哪条路线」，谁负责是按天、按班次排出来的。匹配打分见
`alert_flow.rs::candidate_score`，兜底 **园区负责人 → 未指派**（兜底也会建单，
让值班长在看板上改派，而不是把告警闷掉）。

任务异常多两行（任务名+编号、进度）、一行「当班责任人」和一段**处置建议**，
链接指向 `/tasks?focus={id}`。链接的前缀来自 `ROBODOG_PUBLIC_URL`，
**不配就不带链接**（猜一个地址比不给更容易误导人）。

### 企业微信的硬约束 → 代码里的处理

| 约束（官方） | 处理 | 参数 |
| --- | --- | --- |
| 20 条/分钟/机器人，超了 `45009` | 本地令牌桶 18/min，超出的**排队不丢** | `RATE_PER_MIN` |
| markdown 正文 ≤ 4096 **字节**（中文 3 字节/字），超了整条被拒 `40058` | 按字节截断 + `…（已截断）`，断言不切半个汉字 | `MARKDOWN_LIMIT=4000` |
| text ≤ 2048 字节 | 同上 | `TEXT_LIMIT=2000` |
| markdown **不支持 @ 提及** | `mention_all` 时**另发一条 text** 带 `mentioned_list:["@all"]`；否则有责任人手机号时用 `mentioned_mobile_list` @ 到人（两种都算多发一条）。**这条 text 只做提醒**：级别 + 标题 + 设备/园区，不拼正文 | `wecom_mention_all` / `staff.phone` |
| 只能发不能收 | 消息带链接，处理仍在控制台 | — |
| 支持语法有限 | 只用 `##` / `>` / `**粗**` / `[文字](url)` / `<font color>`；**不用 `-` 列表**（有单测守着） | — |

`<font color>` 只认三个值，映射：`严重→warning`（橙红）、`警告→comment`（灰）、
`提示→info`（绿）。所以严重/警告靠 emoji + 颜色双重区分。

---

## 6. 去重、限流、重试、溢出

这四个是"不被现场骂"的关键，参数都在 `notify.rs` 顶部。

| 机制 | 行为 | 为什么必须 |
| --- | --- | --- |
| **同类告警不刷屏** | 同一设备 + 同一**结构化去重键**（`alerts.dedup_key`：`ai:<设备>:<任务>:<航点>:<类别>` / `fault:<设备>:<故障码>` / `hb:` / `ctrl:`）+ 24h 内已有**在办**自动派单工单 → 新告警挂到那张单上，**不入队**（`alert_flow::find_open_order` + `Assignment.repeated`）；没给键的来源退回旧行为（标题完全相等） | 机器人会每 90s 重复报同一个 `12608`，光靠 `alert_code` 去重无效（每次 code 都不同）；AI 告警内容带置信度/证据路径，**按标题比等于没去重**（2026-11-17 起改用结构化键，见迁移 024） |
| **跟单补推**（上面那条的例外） | 挂回原单的告警如果**数量核对的检出数变了**（1 个 → 2 个），补推一条「跟单：数值有变化」，带上「上次 1 个 → 本次 2 个」与「原工单，未新建」；同一数值重复报继续静默。`alert_flow::count_change` + `Assignment.follow_up` | 2026-11-17 现场：同一航点先报「检查 3 个，实际检出 1 个」（推过群），下一轮报 2 个被完全吞掉，现场以为「配了 3 个却没告警」。置信度抖动不补推（没有信息量） |
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

「系统设置 → 企业微信群机器人」：

- **地址配置**：显示「已配置 / 未配置 + 来源（平台配置 / 服务端环境变量）+ 掩码地址」，
  粘贴新地址保存即生效；「清除」把平台配置置空、回退环境变量；
- **「显示原文」**：找回上次配的完整地址（只管理员，仅本次显示，不落 localStorage）；
  输入框留空**不会**清除已有配置 —— 它只是「要覆盖成什么」的入口；
- 右上按钮发测试消息（同步发，失败直接显示 errcode）；
- 下面列最近 5 条推送记录（带 `last_error`）。

### SQL

```sql
-- 最近 20 条推送：失败的看 last_error
SELECT created_at, status, level, ref_type, ref_id, attempts, last_error
  FROM notifications ORDER BY id DESC LIMIT 20;

-- 某个告警推了几次、什么时候推的
SELECT * FROM notifications WHERE dedupe_key = 'alert:AL-006';

-- 平台里配了地址没有？**不要 SELECT setting_value**（那是凭据，会进终端历史 / 截图）
SELECT setting_key, length(setting_value) AS value_len, updated_at
  FROM settings WHERE setting_key = 'wecom_webhook';
-- value_len = 0 → 平台没配，按环境变量走
```

### 接口

```bash
GET  /api/notifications?limit=20&status=failed&refType=task
POST /api/notifications/test
GET  /api/notifications/webhook            # configured / source(db|env) / masked / enabled
GET  /api/notifications/webhook?reveal=true   # 管理员取一次原文（响应多一个 url 字段）
PUT  /api/notifications/webhook            # {"webhook":"…"}；空串 = 清除并回退环境变量
```

`url` 字段用 `skip_serializing_if` 抹掉，普通读取**连 null 都没有** —— 前端拿到的
响应里根本不存在这个键，避免「顺手 console.log 就泄漏」。`reveal` 要管理员
（`claims.role == "admin"` 或中台管理员），否则 403；日志只记用户名。

### 错误码

| errcode | 含义 | 怎么办 |
| --- | --- | --- |
| `93000` | webhook 地址无效 | 机器人被删了或地址复制不全：重新建一个，把新地址粘到设置页保存（不用重启） |
| `45009` | 超频 | 调低 `RATE_PER_MIN`，或收紧 `min_level` |
| `40058` | 内容超长 | 理论上已被截断兜住；真出现说明 `MARKDOWN_LIMIT` 被改大了 |
| `connection refused` / `timeout` | 服务器出不了公网 | 见 §9「内网部署」 |

### 日志

```bash
journalctl -u rust-admin-robodog -f | grep -i wecom
```
启动时打 `企业微信告警推送已配置 enabled=… min_level=… source=db|env key_tail=2c1314`
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
| ~~**@ 具体人**~~ | ✅ 已做：按当天排班取 `staff.phone`，用 `mentioned_mobile_list` @ 到人（`alert_flow` + `Notice.mentions`）。责任人库没填手机号时只在卡片里显示姓名 | 已完成 |
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
| @ 那一条 text 也拼上正文和前两个字段外的全部内容 | 群里每条告警都是两条几乎一样的消息（卡片 + 一长行），现场误以为告警重复了 | `Notice::mention_text()` 只当提醒：级别 + 标题 + 设备/园区；正文只留在 markdown 卡片里 |
| 长文本直接发 | `40058 content exceed max length` 整条被拒 | 企微按**字节**算，中文 3 字节/字 |
| 前端传 `ref_type` 后端收 `refType` | 筛选静默不生效 | 这个项目里模型统一 `#[serde(rename_all="camelCase")]`，Query 结构体也要加 |
| 改 `AppState` 加字段 | 7 个集成测试编译失败 | 新增字段记得改 `tests/*.rs` 的 `AppState { .. }` 字面量；`Notifier::disabled()` 就是为这个准备的 |
| 自动派单复用「告警转工单」时顺手把告警置「处理中」 | `control_source` 三个用例全红：抢占告警刚建出来就被改成「处理中」，「未处理」计数变 0 | 人工转单 = 已接手 → 推进告警状态；**自动派单的工单还在「待接单」，告警必须留在「未处理」**（`create_order_core(..., promote_alert)`） |
| 派单逻辑写在推送开关后面 | 没配 webhook 的环境里告警不建单也没人管，功能等于不存在 | 派单在建单前、`channel_enabled()` 判断之前执行 |
| 一条告警一张工单，不看去重 | 上线当天机器人每 90s 报一次 `12608`，5 分钟堆 2 张单 + 2 条群消息 | 先去重再建单：同设备 + 24h 内在办 → 复用老单并不推群 |
| 拿工单标题当「同一件事」的判据 | AI 告警内容带置信度（0.41 → 0.77）/ 证据图路径，标题一变就每轮巡检新建一张单，24h 去重等于没有 | 告警写入时带 `dedup_key`（`alerts` + `work_orders` 两列，迁移 024），`find_open_order` 优先按它比；只有无键来源才退回标题 |
| 只改 `handlers/alerts.rs` 的 SELECT | `/dashboard/summary` 500（驾驶舱自己拼了一份 `Alert` 查询） | `Alert` 结构体加列时，全仓搜一遍 `query_as::<Alert>` / `build_query_as::<Alert>` |
| webhook 只在启动时从环境变量读 | 「换个群」要 SSH 改 env 再重启，现场只能等运维 | 地址放进 `settings`（secret）+ `Notifier` 可写槽：页面保存即生效；环境变量保留为兜底 |
| 页面只显示掩码 + 空输入框 | 现场以为「之前配的地址丢了」，反复重新粘贴，甚至以为被清空 | 输入框 placeholder 写清「当前已配置（来源）：<掩码>」，再加「显示原文」（`?reveal=true`，仅管理员） |
| 把 secret 直接塞进通用 `settings` 行 | `GET /api/settings` 明文回显地址（地址即凭据，进页面就上屏幕） | `value_type='secret'` 的行从通用列表过滤掉，专用接口只回掩码；`PUT /api/settings` 拒收 secret 键 |

---

## 12. 部署清单

```bash
# 1. 服务器能出公网
curl -m 5 -sS https://qyapi.weixin.qq.com -o /dev/null && echo ok

# 2. 填 webhook —— 两条路，二选一
#    a) 平台里填（推荐，运维自己就能改）：系统设置 → 企业微信群机器人 → 粘贴地址 → 保存地址
#    b) 环境变量（想锁定不让页面改时用）：robodog.env 权限 600，不进 git
ssh root@39.107.55.13
vi /opt/hz/rust-admin/robodog.env
#   ROBODOG_WECOM_WEBHOOK=https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=xxx
#   ROBODOG_PUBLIC_URL=http://39.107.55.13:3100

systemctl restart rust-admin-robodog
journalctl -u rust-admin-robodog -n 20 | grep 企业微信   # 应看到"已配置 … source=… key_tail=xxxxxx"

# 3. 页面验证：系统设置 → 企业微信群机器人 → 发送测试消息
#    （平台配置改完立即生效，只有改环境变量才需要 restart）
```

`deploy/external-hz/install.sh` 的 `robodog.env` 模板已带注释占位；
`write_env` 遇到已存在的文件会**保留**，所以重新发布不会冲掉现场填的 key。
`deploy/external-hz/config/robodog.server.toml` 已配好 `public_url` 与 `[wecom] enabled`。

### 相关环境变量

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `ROBODOG_WECOM_WEBHOOK` | 空 | webhook 地址（**兜底**：平台设置页为空时才用它）。两边都空 = 整个通道关闭 |
| `ROBODOG_WECOM_ENABLED` | `true` | 部署级总开关 |
| `ROBODOG_WECOM_MIN_LEVEL` | `警告` | 推送下限（`严重`/`警告`/`提示`） |
| `ROBODOG_WECOM_MENTION_ALL` | `false` | 严重级别是否 @所有人（占双倍额度） |
| `ROBODOG_PUBLIC_URL` | 空 | 控制台对外地址，拼消息里的链接 |
