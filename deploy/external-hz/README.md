# 外网正式环境部署（root@39.107.55.13 · `/opt/hz/rust-admin`）

中台（`backend` + `frontend`）与业务平台（`business/server` + `business/web`）
同机部署，**不需要 nginx、不需要 Redis**：

| 服务 | 端口 | 内容 | 对外地址 |
| --- | --- | --- | --- |
| `rust-admin-zentai` | 3007 | 中台 API + 管理前端（前端由 Rust 进程自己托管） | <http://39.107.55.13:3007> |
| `rust-admin-robodog` | 3008 | 业务平台控制台 + 设备 WebSocket 网关 | <http://39.107.55.13:3008>，设备网关 `ws://39.107.55.13:3008/ws/device` |
| `mediamtx` | TCP 8554 | 板端 relay 推 RTSP 上来（WebRTC 出口的入口，Basic 认证） | `rtsp://relay:<口令>@39.107.55.13:8554/bp_<stream>` |
| `mediamtx` | **UDP 8189** | 浏览器 ⇄ mediamtx 的 WebRTC 媒体（ICE-Lite 单端口） | — |
| `mediamtx` | TCP 8189 | 上面那条的 ICE-TCP 回退（企业网/代理封 UDP 时） | — |

> 3000 端口已被本机 `frps` 占用，3005/3100 已被同机其它业务占用，所以中台用 **3007**、业务平台用 **3008**。
> **阿里云安全组要放通**：
> `3007/tcp`、`3008/tcp`、`8554/tcp`（板端推流）、`8189/udp`（WebRTC 媒体，最容易漏）、
> 可选 `8189/tcp`（ICE-TCP 回退）。8554 / 8189 都是对 `0.0.0.0/0` 开：板端走 4G，
> 出口 IP 不固定，靠 mediamtx 的 Basic 口令保护（见 `mediamtx/mediamtx.yml`）。
>
> WHEP **信令**不占新端口：浏览器打的是业务平台 `/api/backpack/whep/*`（3008），
> 由 Rust 进程反代到只绑回环的 mediamtx `127.0.0.1:8889`。

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

## 背包视频的 WebRTC 出口（mediamtx）

板端 `backpack-relay` 把 6 路流按需推到平台 mediamtx（RTSP/TCP），浏览器用 WHEP 拉
（同一份 `:3008` 前端，`VideoPlayer.vue` 自动选 WHEP 还是 FLV）。
`install.sh` 会一并安装并渲染配置：

```text
/opt/hz/rust-admin/
├── mediamtx.env                  # MEDIAMTX_PUBLISH_PASS（600，口令的唯一真源）
├── mediamtx-publish-url.txt      # 给板端抄的 RELAY_RTSP_URL（600）
└── mediamtx/
    ├── mediamtx                 # 官方静态二进制（发布包里带，目标机不联网）
    └── mediamtx.yml             # 由 deploy/external-hz/mediamtx/mediamtx.yml 渲染（0640）
```

- 配置模板在 `mediamtx/mediamtx.yml`（进 git 的版本没有口令，占位符 `__PUBLISH_PASS__` /
  `__PUBLIC_HOST__`）；`MEDIAMTX_PUBLIC_HOST` 环境变量可覆盖公网地址（默认 39.107.55.13）。
- 关键两条：`webrtcIPsFromInterfaces: false` + `webrtcAdditionalHosts: ["39.107.55.13"]`。
  这台机器是 **EIP + NAT**，网卡上只有 `172.29.62.144`；不显式告诉它公网地址，
  它会把内网 IP 当 ICE 候选发给浏览器 → 协商成功但没画面。
- 排障：`curl -s 127.0.0.1:9997/v3/paths/list`（`publisher` / `readers` / `bytesReceived`）、
  `journalctl -u mediamtx -f`。
- 板端接入与故障对照表见 `business/docs/backpack-video-integration.md` §7。

## 目录布局

```text
/opt/hz/rust-admin/
├── zentai.env                     # APP_JWT_SECRET / APP_DOG__BUSINESS_API_TOKEN（600）
├── robodog.env                    # ROBODOG_JWT_SECRET / ROBODOG_SERVICE_TOKEN /
│                                  # （可选）ROBODOG_WECOM_WEBHOOK：企业微信告警地址兜底，
│                                  #   一般直接在控制台「系统设置」里填，不用改这个文件
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
url = "ws://39.107.55.13:3008"
```

`[robot] host` 仍是机器狗自己的内网地址（默认 `192.168.234.234`），不受影响。
也可以只用命令行覆盖、不改文件：

```bash
cargo run -- --server ws://39.107.55.13:3008      # 其余参数按现场需要
```

## 已验证（2026-11-12 首次上线）

- `GET http://39.107.55.13:3007/` → 200，前端 index.html；`/static/js/*` 命中长缓存 + gzip（2.7MB → 913KB）；
- 深链 `/dogDevice` → 200（SPA 回落）；`/api/不存在` → 404 JSON（不会被回落成 HTML）；
- 登录 `POST /api/token`（前端 AES 加密密码）→ 返回 access/refresh token，内存会话可读、错误 token 401；
- 业务平台 `GET :3008/api/health` → 200；控制台首页 200；
- 业务平台反向控制信道已连上中台：`ws://127.0.0.1:3007/api/dogDevice/control-plane/ws`；
- 模拟客户端从外网连 `ws://39.107.55.13:3008/ws/device` 注册成功，设备自动同步到中台台账。
