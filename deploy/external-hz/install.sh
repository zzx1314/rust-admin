#!/usr/bin/env bash
#
# 在目标服务器（CentOS 7 · root@39.107.55.13）上安装 /opt/hz/rust-admin。
#
# 由 build.sh 打进发布包，随包上传后在包目录里以 root 执行：
#   cd /tmp/rust-admin-deploy && ./install.sh
#
# 幂等：可重复执行用于升级。会保留 data/ 下的数据库与已生成的密钥，
# 只替换二进制、前端产物、migrations 与配置文件（旧配置先备份）。
set -Eeuo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE=/opt/hz/rust-admin
ZENTAI=$BASE/zentai
ROBODOG=$BASE/robodog
# 平台侧视频 SFU（板端 RTSP 推上来 → 浏览器 WHEP 拉下去），见 mediamtx/README 段
MEDIAMTX=$BASE/mediamtx
STAMP="$(date +%Y%m%d%H%M%S)"

info() { printf '\033[36m[install]\033[0m %s\n' "$*"; }
warn() { printf '\033[33m[install]\033[0m %s\n' "$*"; }

[[ $EUID -eq 0 ]] || { echo "请用 root 执行" >&2; exit 1; }

random_hex() { od -An -tx1 -N32 /dev/urandom | tr -d ' \n'; }

# ---------------------------------------------------------------- 目录与用户
info "准备目录 $BASE"
mkdir -p "$ZENTAI"/{config,migrations,web,data/device_uploads} "$ROBODOG"/{migrations,web,data/map_ingest} "$MEDIAMTX"

# ---------------------------------------------------------------- 停旧服务
for unit in rust-admin-zentai rust-admin-robodog mediamtx; do
  if systemctl list-unit-files "$unit.service" >/dev/null 2>&1; then
    systemctl stop "$unit" 2>/dev/null || true
  fi
done

# ---------------------------------------------------------------- 二进制
info "安装二进制"
install -m 0755 "$HERE/zentai/x-rust" "$ZENTAI/x-rust"
install -m 0755 "$HERE/robodog/robodog-ops-server" "$ROBODOG/robodog-ops-server"
# mediamtx：平台侧 WebRTC 出口。二进制只有一个，配置由下面按口令渲染（带 *.bak 备份）
install -m 0755 "$HERE/mediamtx/mediamtx" "$MEDIAMTX/mediamtx"

# ---------------------------------------------------------------- 前端与迁移
info "同步前端产物与 migrations"
rsync -a --delete "$HERE/zentai/web/" "$ZENTAI/web/"
rsync -a --delete "$HERE/robodog/web/" "$ROBODOG/web/"
rsync -a "$HERE/zentai/migrations/" "$ZENTAI/migrations/"
rsync -a "$HERE/robodog/migrations/" "$ROBODOG/migrations/"

# ---------------------------------------------------------------- 配置
for target in "$ZENTAI/config/config.toml" "$ROBODOG/server.toml"; do
  if [[ -f "$target" ]]; then
    cp -a "$target" "$target.bak.$STAMP"
    warn "已备份旧配置：$target.bak.$STAMP"
  fi
done
install -m 0644 "$HERE/config/zentai.config.toml" "$ZENTAI/config/config.toml"
install -m 0644 "$HERE/config/robodog.server.toml" "$ROBODOG/server.toml"
# 现场机器人要用的客户端配置片段（外网地址已填好）
install -m 0644 "$HERE/config/client.toml.example" "$BASE/client.toml.example"

# ---------------------------------------------------------------- mediamtx 配置
# 推流口令只放这里（600），模板里是占位符：进 git 的配置文件不含任何密钥。
# 公网地址：这台机器是 EIP + NAT，网卡上只有内网 IP，必须显式告诉 mediamtx，
# 否则它把 172.29.x.x 当候选地址发给浏览器 → ICE 永远连不上（黑屏）。
MEDIAMTX_ENV=$BASE/mediamtx.env
PUBLISH_PASS=""
if [[ -f "$MEDIAMTX_ENV" ]]; then
  PUBLISH_PASS="$(sed -n 's/^MEDIAMTX_PUBLISH_PASS=//p' "$MEDIAMTX_ENV" | head -1)"
fi
if [[ -z "$PUBLISH_PASS" ]]; then
  PUBLISH_PASS="$(random_hex)"
  umask 077
  printf 'MEDIAMTX_PUBLISH_PASS=%s\n' "$PUBLISH_PASS" > "$MEDIAMTX_ENV"
  chmod 0600 "$MEDIAMTX_ENV"
else
  warn "沿用已有推流口令：$MEDIAMTX_ENV"
fi
PUBLIC_HOST="${MEDIAMTX_PUBLIC_HOST:-39.107.55.13}"
if [[ -f "$MEDIAMTX/mediamtx.yml" ]]; then
  cp -a "$MEDIAMTX/mediamtx.yml" "$MEDIAMTX/mediamtx.yml.bak.$STAMP"
fi
sed -e "s|__PUBLISH_PASS__|$PUBLISH_PASS|g" \
    -e "s|__PUBLIC_HOST__|$PUBLIC_HOST|g" \
    "$HERE/mediamtx/mediamtx.yml" > "$MEDIAMTX/mediamtx.yml"
chmod 0640 "$MEDIAMTX/mediamtx.yml"
# 板端 relay 要填的地址（含口令）。靠界面/文档传口令必然出错，落成文件让现场直接抄。
printf 'RTSP 推流地址（板端 .246 的 /etc/robodog/relay.env）：\nRELAY_RTSP_URL=rtsp://relay:%s@%s:8554/bp_\n' \
  "$PUBLISH_PASS" "$PUBLIC_HOST" > "$BASE/mediamtx-publish-url.txt"
chmod 0600 "$BASE/mediamtx-publish-url.txt"

# ---------------------------------------------------------------- HTTPS 证书（自签）
# 浏览器只在安全上下文（https / localhost / 手动加白的源）里开放麦克风与 AudioWorklet，
# 网页对讲必须 https；443 被厂商 OTA 的 nginx 占着，所以业务平台另起 3443（见 server.toml）。
# 证书**带 IP SAN**（Chrome 早就不看 CN），并把自己作成 CA：导入到操作系统的
# 「受信任的根证书颁发机构」后浏览器不再报警（证书放在 $BASE/tls/server.crt）。
TLS_DIR=$BASE/tls
mkdir -p "$TLS_DIR"
if [[ ! -s "$TLS_DIR/server.crt" || ! -s "$TLS_DIR/server.key" ]]; then
  info "生成自签证书（CN=$PUBLIC_HOST，含 IP SAN，10 年）"
  cat > "$TLS_DIR/openssl.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = v3
prompt = no

[dn]
O = RoboDog
CN = $PUBLIC_HOST

[v3]
subjectAltName = IP:$PUBLIC_HOST, IP:127.0.0.1, DNS:localhost
basicConstraints = critical, CA:TRUE
keyUsage = critical, digitalSignature, keyEncipherment, keyCertSign
extendedKeyUsage = serverAuth
EOF
  ( umask 077; openssl req -x509 -newkey rsa:2048 -sha256 -days 3650 -nodes \
      -config "$TLS_DIR/openssl.cnf" \
      -keyout "$TLS_DIR/server.key" -out "$TLS_DIR/server.crt" >/dev/null 2>&1 )
fi
chmod 0644 "$TLS_DIR/server.crt"
chmod 0600 "$TLS_DIR/server.key"

# ---------------------------------------------------------------- 密钥
# 服务令牌两边必须一致；已存在就沿用，避免升级后两边不匹配。
TOKEN=""
if [[ -f "$BASE/robodog.env" ]]; then
  TOKEN="$(sed -n 's/^ROBODOG_SERVICE_TOKEN=//p' "$BASE/robodog.env" | head -1)"
fi
[[ -n "$TOKEN" ]] || TOKEN="$(random_hex)"

write_env() {
  local file="$1"; shift
  if [[ -f "$file" ]]; then
    warn "保留已有密钥文件：$file"
    return
  fi
  umask 077
  : > "$file"
  for line in "$@"; do printf '%s\n' "$line" >> "$file"; done
  chmod 0600 "$file"
}

write_env "$BASE/zentai.env" \
  "APP_JWT_SECRET=$(random_hex)" \
  "APP_DOG__BUSINESS_API_TOKEN=$TOKEN" \
  "APP_CONFIG_FILE=$ZENTAI/config/config.toml" \
  "RUST_LOG=info,x_rust=info,tower_http=warn"

write_env "$BASE/robodog.env" \
  "ROBODOG_JWT_SECRET=$(random_hex)" \
  "ROBODOG_SERVICE_TOKEN=$TOKEN" \
  "ROBODOG_SERVER_CONFIG=$ROBODOG/server.toml" \
  "# 平台侧 mediamtx 的 WHEP 信令地址（/api/backpack/whep/* 反向代理到这里）" \
  "ROBODOG_MEDIAMTX_URL=http://127.0.0.1:8889" \
  "# 企业微信群机器人告警推送：取消注释并填 webhook 地址后重启本服务" \
  "# （地址即凭据，只放这里，不要写进 server.toml）" \
  "# ROBODOG_WECOM_WEBHOOK=https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=xxx" \
  "RUST_LOG=info,robodog_ops_server=info,tower_http=warn"

# 老部署升级：robodog.env 已存在时 write_env 不会重写，这里把新键补进去
ensure_env() {
  local file="$1" key="$2" value="$3"
  grep -q "^${key}=" "$file" 2>/dev/null && return 0
  printf '%s=%s\n' "$key" "$value" >> "$file"
}
ensure_env "$BASE/robodog.env" ROBODOG_MEDIAMTX_URL "http://127.0.0.1:8889"

# ---------------------------------------------------------------- systemd
info "安装 systemd 服务"
install -m 0644 "$HERE/systemd/rust-admin-zentai.service" /etc/systemd/system/
install -m 0644 "$HERE/systemd/rust-admin-robodog.service" /etc/systemd/system/
install -m 0644 "$HERE/systemd/mediamtx.service" /etc/systemd/system/
systemctl daemon-reload
systemctl enable mediamtx.service rust-admin-zentai.service rust-admin-robodog.service >/dev/null
systemctl restart mediamtx.service rust-admin-zentai.service rust-admin-robodog.service

sleep 3
info "服务状态"
for unit in mediamtx rust-admin-zentai rust-admin-robodog; do
  printf '  %-22s %s\n' "$unit" "$(systemctl is-active "$unit.service" 2>/dev/null || true)"
done

info "本机自检"
for port in 3007 3008; do
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://127.0.0.1:$port/" || true)"
  printf '  :%s / → HTTP %s\n' "$port" "${code:-无响应}"
done
curl -s --max-time 5 http://127.0.0.1:3008/api/health || true
echo
# HTTPS（自签）：本机不看证书，只看端口能不能握手、页面能不能出来
https_code="$(curl -sk -o /dev/null -w '%{http_code}' --max-time 5 "https://127.0.0.1:3443/" || true)"
printf '  https://127.0.0.1:3443 / → HTTP %s\n' "${https_code:-无响应}"
if [[ "${https_code}" != "200" ]]; then
  warn "HTTPS 没起来：journalctl -u rust-admin-robodog | grep -i tls / HTTPS（证书路径见 server.toml）"
fi
# mediamtx 控制 API 只绑回环：能列出 path 就说明 SFU 起来了
paths="$(curl -s --max-time 5 http://127.0.0.1:9997/v3/paths/list || true)"
if [[ -n "$paths" ]]; then
  printf '  mediamtx :9997/v3/paths/list → %s…\n' "${paths:0:120}"
else
  printf '  mediamtx :9997/v3/paths/list → 无响应（看 journalctl -u mediamtx）\n'
fi
info "完成。中台 http://<本机IP>:3007 ，业务平台/设备网关 :3008"
info "业务平台 HTTPS（自签，操作员用这个：网页对讲/麦克风需要安全上下文）：https://$PUBLIC_HOST:3443"
info "  首次打开浏览器会提示「不安全」→ 继续前往即可；想不再提示：把 $BASE/tls/server.crt 导入到"
info "  Windows「受信任的根证书颁发机构」/ macOS 钥匙串（设「始终信任」）/ Android「安装 CA 证书」"
warn "阿里云安全组需要额外放通：TCP 3443（业务平台 HTTPS，浏览器网页对讲必需）"
info "WebRTC 出口（mediamtx）：板端推 rtsp://<本机IP>:8554/bp_<stream>，浏览器 WHEP 走 /api/backpack/whep/<stream>"
warn "阿里云安全组需要放通：TCP 8554（板端推流）、UDP 8189（WebRTC 媒体）、可选 TCP 8189（ICE-TCP 回退）"
info "板端推流口令在 $BASE/mediamtx-publish-url.txt（顺手改掉现场 .246 的 relay.env 后重启 backpack-relay）"
