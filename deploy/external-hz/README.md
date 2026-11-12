# 外网正式环境部署（root@39.107.55.13 · `/opt/hz/rust-admin`）

中台（`backend` + `frontend`）与业务平台（`business/server` + `business/web`）
同机部署，**不需要 nginx、不需要 Redis**：

| 服务 | 端口 | 内容 | 对外地址 |
| --- | --- | --- | --- |
| `rust-admin-zentai` | 3005 | 中台 API + 管理前端（前端由 Rust 进程自己托管） | <http://39.107.55.13:3005> |
| `rust-admin-robodog` | 3100 | 业务平台控制台 + 设备 WebSocket 网关 | <http://39.107.55.13:3100>，设备网关 `ws://39.107.55.13:3100/ws/device` |

> 3000 端口已被本机 `frps` 占用，所以中台用 3005。两个端口都需要在阿里云安全组放通。

## 目标机环境与构建方式

目标机是 CentOS 7：**glibc 2.17、systemd 219、2 核 3.6G 内存**。因此：

- 二进制编成 **x86_64-unknown-linux-musl 静态**（不依赖目标机 glibc / libssl）；
- systemd unit 只用 219 支持的指令（`StartLimitInterval` 在 `[Service]` 段，
  不写 `ReadWritePaths` / `TasksMax`，否则日志里全是 `Unknown lvalue`）。

构建在本机（22 核）完成，首次约 30 秒，之后增量几秒：

```bash
sudo apt install musl-tools                      # 一次性：提供 musl-gcc
rustup target add x86_64-unknown-linux-musl      # 一次性
```

## 一键发布

```bash
./deploy/external-hz/push.sh              # 打包 + 上传 + 安装重启
./deploy/external-hz/push.sh --skip-web   # 前端没改时的快速发布
```

分步等价于：

```bash
./deploy/external-hz/build.sh             # → /tmp/rust-admin-out
tar czf /tmp/rust-admin-deploy.tgz -C /tmp/rust-admin-out .
scp /tmp/rust-admin-deploy.tgz root@39.107.55.13:/tmp/
ssh root@39.107.55.13 'rm -rf /tmp/ra-deploy && mkdir /tmp/ra-deploy &&
  tar xzf /tmp/rust-admin-deploy.tgz -C /tmp/ra-deploy &&
  cd /tmp/ra-deploy && bash install.sh'
```

`install.sh` 幂等：重复执行即升级。数据库（`data/`）与密钥（`*.env`）保留，
旧配置文件先备份成 `*.bak.<时间戳>`。

## 目录布局

```text
/opt/hz/rust-admin/
├── zentai.env                     # APP_JWT_SECRET / APP_DOG__BUSINESS_API_TOKEN（600）
├── robodog.env                    # ROBODOG_JWT_SECRET / ROBODOG_SERVICE_TOKEN（600）
├── client.toml.example            # 机器狗客户端要用的配置片段
├── zentai/
│   ├── x-rust                     # 中台二进制（静态）
│   ├── config/config.toml
│   ├── migrations/                # 启动时从工作目录读取并执行
│   ├── web/                       # frontend/dist
│   └── data/{users.db,device_uploads/}
└── robodog/
    ├── robodog-ops-server         # 业务平台二进制（静态）
    ├── server.toml
    ├── migrations/
    ├── web/                       # business/web/dist
    └── data/{robodog.db,map_ingest/}
```

## 运维

```bash
systemctl status rust-admin-zentai rust-admin-robodog
journalctl -u rust-admin-zentai -f
journalctl -u rust-admin-robodog -f
systemctl restart rust-admin-zentai        # 会话存内存：重启后需重新登录
```

## 机器狗客户端

现场机器人上的 `business/client/client.toml` 指向外网地址：

```toml
[server]
url = "ws://39.107.55.13:3100"
```

`[robot] host` 仍是机器狗自己的内网地址（默认 `192.168.234.234`），不受影响。
也可以只用命令行覆盖、不改文件：

```bash
cargo run -- --server ws://39.107.55.13:3100      # 其余参数按现场需要
```

## 已验证（2026-11-12 首次上线）

- `GET http://39.107.55.13:3005/` → 200，前端 index.html；`/static/js/*` 命中长缓存 + gzip（2.7MB → 913KB）；
- 深链 `/dogDevice` → 200（SPA 回落）；`/api/不存在` → 404 JSON（不会被回落成 HTML）；
- 登录 `POST /api/token`（前端 AES 加密密码）→ 返回 access/refresh token，内存会话可读、错误 token 401；
- 业务平台 `GET :3100/api/health` → 200；控制台首页 200；
- 业务平台反向控制信道已连上中台：`ws://127.0.0.1:3005/api/dogDevice/control-plane/ws`；
- 模拟客户端从外网连 `ws://39.107.55.13:3100/ws/device` 注册成功，设备自动同步到中台台账。
