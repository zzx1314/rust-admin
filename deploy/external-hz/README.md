# 外网正式环境部署（root@39.107.55.13 · `/opt/hz/rust-admin`）

中台（`backend` + `frontend`）与业务平台（`business/server` + `business/web`）
同机部署，**不需要 nginx、不需要 Redis**：

| 服务 | 端口 | 内容 |
| --- | --- | --- |
| `rust-admin-zentai` | 3000 | 中台 API + 管理前端（前端由 Rust 进程自己托管） |
| `rust-admin-robodog` | 3100 | 业务平台控制台 + 设备 WebSocket 网关 + 前端 |

目标机是 CentOS 7（**glibc 2.17**），本机 `cargo build` 出来的二进制跑不起来，
`build.sh` 因此在 `rust:alpine` 容器里编 **musl 静态二进制**（不依赖目标机 glibc）。

## 1. 本地打包

```bash
./deploy/external-hz/build.sh          # 前端 + 两个 musl 二进制 → deploy/external-hz/out
```

产物：`out/`（约 60 MB）里有 `zentai/`、`robodog/`、`config/`、`systemd/`、`install.sh`。

## 2. 上传

```bash
cd deploy/external-hz && tar czf /tmp/rust-admin-deploy.tgz -C out .
scp /tmp/rust-admin-deploy.tgz root@39.107.55.13:/tmp/
```

## 3. 安装 / 升级

```bash
ssh root@39.107.55.13
rm -rf /tmp/rust-admin-deploy && mkdir /tmp/rust-admin-deploy
tar xzf /tmp/rust-admin-deploy.tgz -C /tmp/rust-admin-deploy
cd /tmp/rust-admin-deploy && ./install.sh
```

`install.sh` 幂等：重复执行即升级。数据库（`data/`）与密钥（`*.env`）保留，
旧配置文件先备份成 `*.bak.<时间戳>`。

## 4. 目录布局

```text
/opt/hz/rust-admin/
├── zentai.env                     # APP_JWT_SECRET / APP_DOG__BUSINESS_API_TOKEN（600）
├── robodog.env                    # ROBODOG_JWT_SECRET / ROBODOG_SERVICE_TOKEN（600）
├── zentai/
│   ├── x-rust                     # 中台二进制
│   ├── config/config.toml
│   ├── migrations/                # 启动时按工作目录读取并执行
│   ├── web/                       # frontend/dist
│   └── data/{users.db,device_uploads/}
└── robodog/
    ├── robodog-ops-server
    ├── server.toml
    ├── web/                       # business/web/dist
    └── data/{robodog.db,map_ingest/}
```

## 5. 运维

```bash
systemctl status rust-admin-zentai rust-admin-robodog
journalctl -u rust-admin-zentai -f
journalctl -u rust-admin-robodog -f
systemctl restart rust-admin-zentai       # 会话存内存：重启后需重新登录
```

## 6. 机器狗客户端

客户端 `business/client` 的 `client.toml` 指向外网地址：

```toml
[server]
url = "ws://39.107.55.13:3100"       # 设备网关
```

本体地址（`[robot] host`）仍是机器狗自己的内网地址，不受影响。

## 端口与安全组

需要放通 **3000**、**3100** 入站（阿里云安全组 + 本机 iptables 已为空）。
若安全组暂时无法开放，可先只放通其中一个端口，用本机 `curl` 验证。
