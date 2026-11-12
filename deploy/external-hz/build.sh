#!/usr/bin/env bash
#
# 打包「中台 + 业务平台」正式环境发布产物。
#
# 目标机是 CentOS 7（glibc 2.17），本机直接 `cargo build` 出来的二进制会因缺少
# 新版 glibc 符号而跑不起来（`GLIBC_2.34 not found`）。所以这里编的是
# **x86_64-unknown-linux-musl 静态二进制**：不依赖目标机 glibc / libssl，
# 拷过去就能跑。用的是本机工具链（musl-gcc），比 Docker 快得多，也没有镜像层开销。
#
# 依赖（一次装好）：
#   sudo apt install musl-tools            # 提供 musl-gcc
#   rustup target add x86_64-unknown-linux-musl
#
# 耗时：首次编全部依赖约 1 分钟出头（22 核），之后只编改动的 crate，几十秒。
# 两个 crate 并行编译，互不共享 target 目录（共享会被 cargo 的目录锁串行化）。
#
# 用法：
#   ./build.sh                 # 前端 + 两个二进制 → /tmp/rust-admin-out
#   ./build.sh --skip-web      # 只编二进制（dist 已是最新时）
#   OUT_DIR=~/dist ./build.sh  # 自定义产物目录
set -Eeuo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# 产物放仓库外：避免几十 MB 二进制被提交进 git
OUT="${OUT_DIR:-/tmp/rust-admin-out}"

SKIP_WEB=false
[[ "${1:-}" == "--skip-web" ]] && SKIP_WEB=true

info() { printf '\033[36m[build]\033[0m %s\n' "$*"; }
warn() { printf '\033[33m[build]\033[0m %s\n' "$*"; }
now() { date +%s; }

TARGET=x86_64-unknown-linux-musl
# musl 目标需要 C 工具链：sqlite（bundled）、ring 等要在编译期编 C 代码
export CC_x86_64_unknown_linux_musl="${CC_x86_64_unknown_linux_musl:-musl-gcc}"
export AR_x86_64_unknown_linux_musl="${AR_x86_64_unknown_linux_musl:-ar}"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER:-musl-gcc}"

# ---------------------------------------------------------------- 环境检查
if ! command -v musl-gcc >/dev/null 2>&1; then
  echo "缺少 musl-gcc：sudo apt install musl-tools" >&2
  exit 1
fi
if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
  echo "缺少 $TARGET target：rustup target add $TARGET" >&2
  exit 1
fi

T0="$(now)"

# ---------------------------------------------------------------- 前端产物
if [[ "$SKIP_WEB" == false ]]; then
  info "构建中台前端 frontend/dist"
  ( cd "$ROOT/frontend" && pnpm build )

  info "构建业务平台控制台 business/web/dist"
  ( cd "$ROOT/business/web" && pnpm build )
else
  info "跳过前端构建（--skip-web）"
fi

# ------------------------------------------------------------ 编译静态二进制
build_musl() {
  local crate_dir="$1" bin="$2" dest="$3" log="$4"
  cargo build --release --locked --target "$TARGET" --manifest-path "$ROOT/$crate_dir/Cargo.toml" > "$log" 2>&1
  # strip 去掉符号表：25MB → 18MB，上传快一半
  strip --strip-all "$ROOT/$crate_dir/target/$TARGET/release/$bin"
  cp "$ROOT/$crate_dir/target/$TARGET/release/$bin" "$dest/"
}

rm -rf "$OUT"
mkdir -p "$OUT"/{zentai/migrations,zentai/web,robodog/migrations,robodog/web,systemd,config}

T1="$(now)"
info "并行编译两个 musl 静态二进制（$TARGET）"
build_musl backend x-rust "$OUT/zentai" /tmp/build-zentai.log &
BACKEND_PID=$!
build_musl business/server robodog-ops-server "$OUT/robodog" /tmp/build-robodog.log &
SERVER_PID=$!

status=0
wait "$BACKEND_PID" || { echo "中台编译失败：" >&2; tail -20 /tmp/build-zentai.log >&2; status=1; }
wait "$SERVER_PID" || { echo "业务平台编译失败：" >&2; tail -20 /tmp/build-robodog.log >&2; status=1; }
[[ "$status" -eq 0 ]] || exit 1

# 静态链接自检：漏了 --target 或 musl 工具链不生效时，产物会带 glibc 依赖
for binary in "$OUT/zentai/x-rust" "$OUT/robodog/robodog-ops-server"; do
  # `file` 对 musl 静态 PIE 输出 "static-pie linked"，对普通静态输出
  # "statically linked"；带 glibc 依赖的会显示 "dynamically linked"。
  if ! file "$binary" | grep -qE "statically linked|static-pie linked"; then
    echo "$binary 不是静态链接产物，目标机（glibc 2.17）会跑不起来" >&2
    exit 1
  fi
done
info "二进制编译完成，用时 $(( $(now) - T1 ))s"

# ---------------------------------------------------------------- 装配产物
info "拷贝前端与运行期必需文件"
cp -r "$ROOT/frontend/dist/." "$OUT/zentai/web/"
cp -r "$ROOT/business/web/dist/." "$OUT/robodog/web/"
# 两个服务启动时都会从工作目录读取 migrations/ 执行数据库迁移
cp -r "$ROOT/backend/migrations/." "$OUT/zentai/migrations/"
cp -r "$ROOT/business/server/migrations/." "$OUT/robodog/migrations/"
cp "$HERE/config/zentai.config.toml" "$OUT/config/"
cp "$HERE/config/robodog.server.toml" "$OUT/config/"
cp "$HERE/systemd/"*.service "$OUT/systemd/"
cp "$HERE/install.sh" "$OUT/"
chmod +x "$OUT/install.sh"

info "产物就绪：$OUT（总用时 $(( $(now) - T0 ))s）"
du -sh "$OUT"/* 2>/dev/null | sed 's/^/       /'
info "下一步：tar czf /tmp/rust-admin-deploy.tgz -C $OUT . && scp /tmp/rust-admin-deploy.tgz root@39.107.55.13:/tmp/"
