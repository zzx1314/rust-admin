# RoboDog OPS · 独立业务服务

`business/` 是独立于仓库根目录 `backend/`、`frontend/`、`client/` 的业务服务，
按 `designprototype/robodog-management-system.html` 原型实现，拥有自己的
Rust 后端、SQLite 数据库与 Vue 3 运营控制台。

## 目录结构

```text
business/
├── designprototype/                # 原始 HTML 交互原型（只读参考）
├── server/                        # 独立 Rust 服务（axum + sqlx + SQLite）
│   ├── src/
│   │   ├── auth.rs                # JWT 登录、鉴权中间件
│   │   ├── db.rs                  # 连接池、SQL 迁移
│   │   ├── seed.rs                # 默认管理员与演示数据
│   │   ├── geo.rs                 # 路线里程/耗时/耗电计算、顺序优化
│   │   ├── handlers/              # 园区、区域、设备、任务、路线、告警、设置、驾驶舱
│   │   └── app.rs                 # 路由装配、CORS、静态资源
│   ├── migrations/001_schema.sql  # 建表
│   ├── seed/routes.json           # 演示路线（航点/航段/动作）
│   └── data/robodog.db            # 运行时生成，删除后自动重建并重新播种
└── web/                           # 独立 Vue 3 控制台
    ├── src/api/                   # 请求封装与类型
    ├── src/components/            # 图表、状态徽标、设备抽屉、园区态势图等
    ├── src/layout/                # 原型风格的侧边栏 + 顶栏
    ├── src/views/                 # 7 个业务页面
    └── src/styles/                # 原型设计令牌与页面样式
```

## 快速启动

### 1. 启动后端（端口 3100）

```bash
cd business/server
cargo run
```

首次启动会：

1. 执行 `migrations/001_schema.sql` 建表；
2. 创建默认管理员 `admin / admin123`；
3. 写入 3 个园区、8 个巡检区域、12 台设备、4 条路线、10 个任务、5 条告警等演示数据。

### 2. 启动前端开发服务器（端口 5273）

```bash
cd business/web
pnpm install
pnpm dev
```

浏览器打开 <http://127.0.0.1:5273>，使用 `admin / admin123` 登录。
Vite 已将 `/api` 代理到 `http://127.0.0.1:3100`。

### 3. 生产模式（单端口）

```bash
cd business/web && pnpm build
cd ../server && cargo run
```

后端检测到 `business/web/dist` 后会直接托管前端，访问 <http://127.0.0.1:3100>。

## 页面与功能

| 页面 | 功能 |
| --- | --- |
| 总览驾驶舱 | KPI、园区态势 SVG、设备状态环图、实时告警、7 日趋势、事件流、导出日报、全屏 |
| 园区管理 | 园区卡片/表格、完成率、巡检区域清单、园区与区域增删改、跳转巡检计划 |
| 巡检路线 | 三栏编辑器：路线模板、SVG 画布拖拽加点/选段、航点属性、航段任务、动作库、撤销、顺序优化、校验、发布、模拟执行 |
| 设备管理 | 状态统计、筛选、电量/信号/心跳、设备详情抽屉、远程指令、批量下发、CSV 导出 |
| 任务管理 | 看板拖拽改状态、列表视图、任务创建/详情、航点执行序列、开始/暂停 |
| 告警中心 | 级别/状态筛选、单条/全部处理、告警详情 |
| 系统设置 | 平台参数、通知开关、关于平台 |

## API 速查

所有接口统一返回 `{ code, msg, data }`，成功 `code = 10200`。
除 `POST /api/auth/login` 和 `GET /api/health` 外均需 `Authorization: Bearer <token>`。

```text
POST   /api/auth/login
GET    /api/auth/me

GET    /api/parks                 POST   /api/parks
GET    /api/parks/{id}            PUT    /api/parks/{id}      DELETE /api/parks/{id}
GET    /api/zones                 POST   /api/zones
GET    /api/zones/{id}            PUT    /api/zones/{id}      DELETE /api/zones/{id}

GET    /api/devices               POST   /api/devices
GET    /api/devices/stats
POST   /api/devices/refresh
GET    /api/devices/{id}          PUT    /api/devices/{id}    DELETE /api/devices/{id}
POST   /api/devices/{id}/command

GET    /api/tasks                 POST   /api/tasks
GET    /api/tasks/stats
GET    /api/tasks/{id}            PUT    /api/tasks/{id}       DELETE /api/tasks/{id}
PUT    /api/tasks/{id}/status

GET    /api/routes                POST   /api/routes
GET    /api/routes/{id}           PUT    /api/routes/{id}      DELETE /api/routes/{id}
POST   /api/routes/{id}/validate
POST   /api/routes/{id}/publish
POST   /api/routes/{id}/optimize

GET    /api/alerts                POST   /api/alerts
GET    /api/alerts/stats
POST   /api/alerts/handle-all
GET    /api/alerts/{id}           PUT    /api/alerts/{id}      DELETE /api/alerts/{id}
POST   /api/alerts/{id}/handle

GET    /api/settings              PUT    /api/settings
GET    /api/dashboard/summary
```

## 路线数据模型

路线保存在 `routes.route_data`（JSON），结构为：

```json
{
  "schemaVersion": 2,
  "loop": "闭环",
  "points": [
    { "name": "配电柜测温点", "x": 212, "y": 294, "dwell": 15, "radius": 0.3,
      "must": true, "heading": 90, "onFail": "重试 2 次",
      "actions": [{ "type": "红外测温", "param": "云台 P90", "trigger": "停留后执行",
                    "threshold": "> 65℃ 生成告警", "retry": 2 }] }
  ],
  "legs": [
    { "mode": "沿道路", "speed": 1.2, "avoid": "标准",
      "actions": [{ "type": "全程录像", "param": "1080P / 25fps / 连续" }] }
  ],
  "returnLeg": { "mode": "沿道路", "speed": 1.5, "avoid": "标准", "actions": [] }
}
```

后端在保存时重新计算 `point_count`、`distance_m`、`duration_s`、`battery_pct`，
校验与发布接口直接读取该 JSON。

## 配置项

| 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `ROBODOG_PORT` | `3100` | 后端端口 |
| `ROBODOG_DB` | `server/data/robodog.db` | SQLite 文件路径 |
| `ROBODOG_JWT_SECRET` | 开发用默认值 | JWT 密钥，生产必须覆盖 |
| `ROBODOG_JWT_TTL_HOURS` | `24` | Token 有效期 |
| `ROBODOG_WEB_DIST` | `../web/dist` | 前端构建产物目录 |

## 当前边界

- 设备状态、电量、心跳来自本服务数据库；演示数据由启动播种生成。
- 设备控制（开始巡检 / 暂停 / 召回充电 / 远程接管）会更新设备状态并写入事件流，
  但**尚未接入真实 WebSocket 设备网关**；后续可在 `server/src` 下新增 gateway 模块，
  复用原型中的设备 token、注册、心跳与任务下发协议。
- 视频画面为占位区域，未接入 RTSP/WebRTC。
- 告警目前以数据库记录与手动处置为主，自动规则（温度/离线/低电量）预留后续扩展。

## 重置演示数据

停止后端后删除数据库文件，再次启动即可重新建表与播种：

```bash
rm -f business/server/data/robodog.db*
cd business/server && cargo run
```

## 检查命令

```bash
cd business/server && cargo clippy -- -D warnings
cd business/web && pnpm typecheck && pnpm build
```
