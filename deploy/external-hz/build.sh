#!/usr/bin/env bash
#
# 打包「中台 + 业务平台」正式环境发布产物。
#
# 目标机是 CentOS 7（glibc 2.17），直接用本机 `cargo build` 出来的二进制会因
# 缺少新版本 glibc 符号而跑不起来。所以这里在 `rust:alpine` 容器里编译
# **x86_64-unknown-linux-musl 静态二进制**：产物不依赖目标机的 glibc / libssl，
# 拷过去就能运行。
#
# 用法：
#   ./build.sh              # 全量构建到 /tmp/rust-admin-out（OUT_DIR 可改）
#   ./build.sh --skip-web   # 跳过前端构建（dist 已是最新时）
set -Eeuo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# 产物放仓库外：避免几MB二进制被提交进 git
OUT="${OUT_DIR:-/tmp/rust-admin-out}"

SKIP_WEB=false
[[ "${1:-}" == "--skip-web" ]] && SKIP_WEB=true

info() { printf '\033[36m[build]\033[0m %s\n' "$*"; }

# ---------------------------------------------------------------- 前端产物
if [[ "$SKIP_WEB" == false ]]; then
  info "构建中台前端 frontend/dist"
  ( cd "$ROOT/frontend" && pnpm build )

  info "构建业务平台控制台 business/web/dist"
  ( cd "$ROOT/business/web" && pnpm build )
fi

# ------------------------------------------------------------ musl 静态二进制
#
# 用 named volume 缓存 target，重复构建不用从头编译依赖。
build_musl() {
  local src_dir="$1" crate_dir="$2" bin="$3" dest="$4" cache_volume="$5"
  info "编译 $crate_dir/$bin（静态 musl）"
  docker run --rm \
    -v "$ROOT/$src_dir":/src \
    -v "$cache_volume":/cache \
    -v "$dest":/out \
    -w "/src/$crate_dir" \
    -e CARGO_TARGET_DIR=/cache/target \
    rust:1-alpine \
    sh -c 'set -eux; apk add --no-cache build-base perl pkgconf >/dev/null; cargo build --release; cp "/cache/target/release/'"$bin"'" /out/'
}

rm -rf "$OUT"
mkdir -p "$OUT"/{zentai/migrations,zentai/web,robodog/web,systemd,config}

build_musl . backend x-rust "$OUT/zentai" rustadmin_musl_cache
build_musl business server robodog-ops-server "$OUT/robodog" rustadmin_musl_cache_server

# ---------------------------------------------------------------- 装配产物
info "拷贝前端与运行期必需文件"
cp -r "$ROOT/frontend/dist/." "$OUT/zentai/web/"
cp -r "$ROOT/business/web/dist/." "$OUT/robodog/web/"
# 中台启动时会从工作目录读取 migrations/ 执行数据库迁移
cp -r "$ROOT/backend/migrations/." "$OUT/zentai/migrations/"
cp "$HERE/config/zentai.config.toml" "$OUT/config/"
cp "$HERE/config/robodog.server.toml" "$OUT/config/"
cp "$HERE/systemd/"*.service "$OUT/systemd/"
cp "$HERE/install.sh" "$OUT/"

info "产物就绪：$OUT"
du -sh "$OUT"/* 2>/dev/null | sed 's/^/       /'
