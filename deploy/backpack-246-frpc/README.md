# `.246` 背包机的 frpc（公网 3006 → 本机 ssh 22）

现场那台背包机（`192.168.41.246`）只在现场 Wi-Fi 内网可达，开发机不在同一网段时
完全失联。这里用**已有的公网 frps**（`39.107.55.13:8090`，中台 3000/3005 那台机器上
跑着的 `frps`）做一条反向隧道：

```
ssh -p 3006 root@39.107.55.13   →   frps    →   .246 的 frpc   →   127.0.0.1:22
```

- 公网侧：`39.107.55.13:3006`（阿里云安全组已放通，2026-11-19 实测可达）
- 内网侧：`.246` 的 `frpc`（`aarch64`），出网走**4G**（默认路由 metric 100），
  与视频中继 / 设备心跳同一条链路；Wi-Fi 掉了不影响
- frps 侧没有配 `auth.token`，**别在隧道里传明文密码**——只走 key 登录

## 文件

| 文件 | 板端位置 | 说明 |
| --- | --- | --- |
| `frpc.toml` | `/etc/frp/frpc.toml`（600） | 服务端地址 + `backpack-246-ssh` 代理 |
| `frpc.service` | `/etc/systemd/system/frpc.service` | `Restart=always`，随开机自启 |

二进制：`/opt/frp/frpc`（frp **v0.68.0**，必须与公网 `frps` 版本一致）。

## 安装 / 升级

`.246` 上不了 GitHub（`curl https://github.com` → Network is unreachable），
所以**在开发机下载 `arm64` 包再 scp**（注意别拿公网上那份 `amd64` 的）：

```bash
# 开发机（github 直连不通时加代理：export https_proxy=http://127.0.0.1:7890）
curl -fLO https://github.com/fatedier/frp/releases/download/v0.68.0/frp_0.68.0_linux_arm64.tar.gz
scp frp_0.68.0_linux_arm64.tar.gz root@192.168.41.246:/tmp/
scp deploy/backpack-246-frpc/frpc.toml root@192.168.41.246:/etc/frp/frpc.toml
scp deploy/backpack-246-frpc/frpc.service root@192.168.41.246:/etc/systemd/system/

# .246
cd /tmp && tar xzf frp_0.68.0_linux_arm64.tar.gz
install -d /opt/frp /etc/frp
install -m 755 /tmp/frp_0.68.0_linux_arm64/frpc /opt/frp/frpc
/opt/frp/frpc -v            # 0.68.0
chmod 600 /etc/frp/frpc.toml
systemctl daemon-reload
systemctl enable --now frpc
```

## 验证

```bash
# .246
journalctl -u frpc -n 20 --no-pager       # login to server success / start proxy success
ip route get 39.107.55.13                 # 走 4G（enx…，metric 100）

# 公网机器
ss -lntp | grep 3006                      # frps 在 [::]:3006 监听

# 任意外网机器
ssh -p 3006 root@39.107.55.13 'hostname'  # ubuntu-arm64
```

首次从新机器连要 `-o StrictHostKeyChecking=accept-new`，否则会报
`Host key verification failed`（`[39.107.55.13]:3006` 是另一台机器的 host key，
和 22 端口那条不是一回事）。

## 排障

| 现象 | 原因 / 处理 |
| --- | --- |
| `ssh -p 3006` 卡住无响应 | frps 在监听但没有客户端：到 `.246` 看 `systemctl status frpc`；4G 掉线时 `ip route` 会没有到公网的默认路由 |
| `Connection refused` | 隧道没建立（frps 上 3006 没有监听）→ frpc 日志里找 `login to server failed` |
| `login to server failed: connect timeout` | `.246` 出网断了（4G 拨号 / 安全组），先 `ping 39.107.55.13`、`ip route get 39.107.55.13` |
| `ssh: Permission denied (publickey)` | 隧道是通的，问题在 `.246` 的 sshd 授权：`/root/.ssh/authorized_keys` |
| 启动即退出 | 二进制拿错了架构（`amd64` 装到 `aarch64` 上）；`file /opt/frp/frpc` 应为 `ARM aarch64` |

## 回滚 / 卸载

```bash
systemctl disable --now frpc
rm -f /etc/systemd/system/frpc.service /etc/frp/frpc.toml /opt/frp/frpc
systemctl daemon-reload
```

公网侧不用动：`frps.toml` 里没有 `allowPorts`（默认放开所有远端端口），
3006 只是被这条隧道占用、隧道一断就释放。
