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
STAMP="$(date +%Y%m%d%H%M%S)"

info() { printf '\033[36m[install]\033[0m %s\n' "$*"; }
warn() { printf '\033[33m[install]\033[0m %s\n' "$*"; }

[[ $EUID -eq 0 ]] || { echo "请用 root 执行" >&2; exit 1; }

random_hex() { od -An -tx1 -N32 /dev/urandom | tr -d ' \n'; }

# ---------------------------------------------------------------- 目录与用户
info "准备目录 $BASE"
mkdir -p "$ZENTAI"/{config,migrations,web,data/device_uploads} "$ROBODOG"/{migrations,web,data/map_ingest}

# ---------------------------------------------------------------- 停旧服务
for unit in rust-admin-zentai rust-admin-robodog; do
  if systemctl list-unit-files "$unit.service" >/dev/null 2>&1; then
    systemctl stop "$unit" 2>/dev/null || true
  fi
done

# ---------------------------------------------------------------- 二进制
info "安装二进制"
install -m 0755 "$HERE/zentai/x-rust" "$ZENTAI/x-rust"
install -m 0755 "$HERE/robodog/robodog-ops-server" "$ROBODOG/robodog-ops-server"

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
  "RUST_LOG=info,robodog_ops_server=info,tower_http=warn"

# ---------------------------------------------------------------- systemd
info "安装 systemd 服务"
install -m 0644 "$HERE/systemd/rust-admin-zentai.service" /etc/systemd/system/
install -m 0644 "$HERE/systemd/rust-admin-robodog.service" /etc/systemd/system/
systemctl daemon-reload
systemctl enable rust-admin-zentai.service rust-admin-robodog.service >/dev/null
systemctl restart rust-admin-zentai.service rust-admin-robodog.service

sleep 3
info "服务状态"
for unit in rust-admin-zentai rust-admin-robodog; do
  printf '  %-22s %s\n' "$unit" "$(systemctl is-active "$unit.service" 2>/dev/null || true)"
done

info "本机自检"
for port in 3000 3100; do
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://127.0.0.1:$port/" || true)"
  printf '  :%s / → HTTP %s\n' "$port" "${code:-无响应}"
done
curl -s --max-time 5 http://127.0.0.1:3100/api/health || true
echo
info "完成。中台 http://<本机IP>:3000 ，业务平台/设备网关 :3100"
