#!/usr/bin/env bash
# 交叉编译 aarch64（Ubuntu 22.04 / glibc 2.35）版本，并校验 glibc 符号上限。
#
# 目标机：192.168.41.246（ubuntu-arm64，aarch64，无 Rust 工具链、无外网）
# 前置：rustup target add aarch64-unknown-linux-gnu
#       推荐安装 cargo-zigbuild + zig（本机 glibc 较新时普通 gcc 交叉链接会要求 GLIBC_2.39）
#
# 用法：./deploy/build-aarch64.sh [--max-glibc 2.35]
set -euo pipefail

MAX_GLIBC=2.35
for arg in "$@"; do
  case "$arg" in
    --max-glibc) shift; MAX_GLIBC="${1:-2.35}" ;;
    -h|--help) awk 'NR>1 && /^#/ {sub(/^# ?/, ""); print; next} NR>1 {exit}' "$0"; exit 0 ;;
    *) echo "未知参数: $arg" >&2; exit 2 ;;
  esac
done

cd "$(dirname "${BASH_SOURCE[0]}")/.."
TARGET=aarch64-unknown-linux-gnu
BIN="target/$TARGET/release/robodog-device-client"

echo "==> 编译 $TARGET"
# 本机 glibc 比目标机新（两者都 ≥ 2.39）时，直接 gcc 交叉链接出来的二进制会要求
# GLIBC_2.39，目标机跑不起来。cargo-zigbuild 能按指定的 glibc 版本链接，所以优先用它。
if command -v cargo-zigbuild >/dev/null 2>&1 && command -v zig >/dev/null 2>&1; then
  echo "    用 cargo zigbuild（按 GLIBC_$MAX_GLIBC 链接）"
  cargo zigbuild --release --target "$TARGET.$MAX_GLIBC"
else
  echo "    未装 cargo-zigbuild/zig，退回普通交叉编译（产物可能要求本机 glibc）"
  cargo build --release --target "$TARGET"
fi

READELF="${TARGET%%-*}-linux-gnu-readelf"
if ! command -v "$READELF" >/dev/null 2>&1; then
  echo "找不到 $READELF，跳过 glibc 校验（apt install gcc-aarch64-linux-gnu）" >&2
  exit 0
fi

REQUIRED="$("$READELF" -V "$BIN" | grep -oE 'GLIBC_[0-9.+]+' | sed 's/GLIBC_//' | sort -uV | tail -1)"
echo "==> 二进制最高要求 GLIBC_$REQUIRED（目标机上限 $MAX_GLIBC）"
if [[ "$(printf '%s\n%s\n' "$REQUIRED" "$MAX_GLIBC" | sort -V | tail -1)" != "$MAX_GLIBC" ]]; then
  echo "✗ 产物要求 GLIBC_$REQUIRED > $MAX_GLIBC，目标机跑不起来" >&2
  echo "  装 cargo-zigbuild + zig 后重跑本脚本，或在目标机本地编译" >&2
  exit 1
fi

ls -lh "$BIN"
echo "==> 拷贝到目标机："
echo "    scp $BIN root@192.168.41.246:/opt/robodog/client/robodog-device-client"
