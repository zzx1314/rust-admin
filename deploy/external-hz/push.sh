#!/usr/bin/env bash
#
# 一条命令：本地打包 → 上传 → 在服务器上安装/重启。
#
#   ./push.sh              # 全量（含前端构建）
#   ./push.sh --skip-web   # 只编二进制（前端没改时更快）
#
# 目标机信息可用环境变量覆盖：DEPLOY_HOST（默认 root@39.107.55.13）
set -Eeuo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEPLOY_HOST="${DEPLOY_HOST:-root@39.107.55.13}"
BUNDLE=/tmp/rust-admin-deploy.tgz

info() { printf '\033[36m[push]\033[0m %s\n' "$*"; }

"$HERE/build.sh" "$@"

info "打包并上传到 $DEPLOY_HOST"
tar czf "$BUNDLE" -C "${OUT_DIR:-/tmp/rust-admin-out}" .
scp -q -o StrictHostKeyChecking=no "$BUNDLE" "$DEPLOY_HOST:/tmp/"

info "在服务器上安装"
ssh -o StrictHostKeyChecking=no "$DEPLOY_HOST" '
  set -e
  rm -rf /tmp/ra-deploy && mkdir -p /tmp/ra-deploy
  tar xzf /tmp/rust-admin-deploy.tgz -C /tmp/ra-deploy
  cd /tmp/ra-deploy && bash install.sh
  rm -rf /tmp/ra-deploy /tmp/rust-admin-deploy.tgz
' 2>&1 | grep -vE "WARNING: connection is not using|post-quantum|store now, decrypt later|may need to be upgraded"
