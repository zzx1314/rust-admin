#!/usr/bin/env bash
# 现场部署脚本：把 robodog-device-client 装到 192.168.41.246 并注册为 systemd 服务。
#
# 默认值（编译进二进制，见 src/config.rs）：
#   业务服务 ws://39.107.55.13:3100（正式环境外网）
#   设备编号 启动时自动读本体 SN 分区 /dev/mmcblk0p5（无需配置）
#   本体     192.168.234.234:10010
#
# 用法（在 business/client 目录下）：
#   sudo ./deploy/install.sh                 # 编译 release + 安装 + 启动
#   sudo ./deploy/install.sh --no-start      # 只安装，不启动服务
#   sudo ./deploy/install.sh --skip-build    # 用已有 target/release 二进制
#
# 覆盖参数：改 /etc/robodog/client.toml（首次安装从 deploy/client.toml.template 生成），
# 密钥写 /etc/robodog/client.env（chmod 600）。
set -euo pipefail

PREFIX=/opt/robodog/client
CONF_DIR=/etc/robodog
CONF_FILE="$CONF_DIR/client.toml"
ENV_FILE="$CONF_DIR/client.env"
UNIT_NAME=robodog-device-client.service
UNIT_SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$UNIT_NAME"
CLIENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

START=1
BUILD=1
for arg in "$@"; do
  case "$arg" in
    --no-start) START=0 ;;
    --skip-build) BUILD=0 ;;
    -h|--help)
      awk 'NR>1 && /^#/ {sub(/^# ?/, ""); print; next} NR>1 {exit}' "$0"
      exit 0
      ;;
    *)
      echo "未知参数: $arg（--help 查看用法）" >&2
      exit 2
      ;;
  esac
done

if [[ $EUID -ne 0 ]]; then
  echo "需要 root：请用 sudo 运行" >&2
  exit 1
fi

cd "$CLIENT_DIR"

if [[ $BUILD -eq 1 ]]; then
  echo "==> 编译 release"
  cargo build --release
fi

BIN="target/release/robodog-device-client"
if [[ ! -x "$BIN" ]]; then
  echo "找不到可执行文件 $BIN，先 cargo build --release" >&2
  exit 1
fi

echo "==> 安装二进制到 $PREFIX"
install -Dm755 "$BIN" "$PREFIX/robodog-device-client"
install -Dm644 README.md "$PREFIX/README.md"

echo "==> 准备配置 $CONF_DIR"
install -d -m 750 "$CONF_DIR"
if [[ -f "$CONF_FILE" ]]; then
  echo "    已有 $CONF_FILE，保留不覆盖"
else
  install -Dm600 deploy/client.toml.template "$CONF_FILE"
  echo "    已生成 $CONF_FILE（字段说明见文件头注释）"
fi
if [[ -f "$ENV_FILE" ]]; then
  echo "    已有 $ENV_FILE，保留不覆盖"
else
  install -Dm600 deploy/client.env.example "$ENV_FILE"
  echo "    已生成 $ENV_FILE：请把 ROBODOG_API_PASSWORD 改成线上密码"
fi

echo "==> 校验生效配置（敏感字段已脱敏）"
"$PREFIX/robodog-device-client" --config "$CONF_FILE" --print-config

echo "==> 注册 systemd 服务"
install -Dm644 "$UNIT_SRC" "/etc/systemd/system/$UNIT_NAME"
systemctl daemon-reload

if [[ $START -eq 1 ]]; then
  systemctl enable --now "$UNIT_NAME"
  sleep 1
  systemctl --no-pager --lines=20 status "$UNIT_NAME" || true
  cat <<'EOF'

服务已启动。常用命令：
  journalctl -u robodog-device-client -f     # 看日志
  systemctl restart robodog-device-client    # 改完配置重启
  systemctl stop robodog-device-client       # 停服

首次上线检查：
  1. ./deploy/check.sh 全绿（或日志出现「设备注册成功: <本体 SN>」）；
  2. 控制台「设备管理」能看到该设备（状态「待绑定」），把它绑定到园区；
  3. 绑定后心跳/任务才会正常推进，建图才会回传中台。
EOF
else
  systemctl enable "$UNIT_NAME"
  echo "已注册但未启动：systemctl start $UNIT_NAME"
fi
