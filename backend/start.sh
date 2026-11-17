#!/bin/bash
# 板端启动脚本：camera-init -> (复用已有 snap mediamtx) -> Agent(自带 ffmpeg 推流)
#
# 重要：现场 mediamtx 已通过 snap 安装并正在服务 dog-ipcam(RTSP:8554)，
# 因此本脚本默认【不再启动第二个 mediamtx】，只把四路摄像头推流到已有的 mediamtx，
# 并启动 Agent。请先确认已有 mediamtx 已开启 WebRTC（见 README）。
#
# 若你确实想用本项目自带的 mediamtx.yml 单独起一个 mediamtx，设 MEDIAMTX_MANAGED=1。
#
# 交付到未预装 librknnrt.so 的板子时，把该库放到 rknn/ 即自动启用（见文件末尾）；
# 开发板等已装系统的板子不放这个文件即可，行为与以前完全一致。
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"

echo "[start] 确保摄像头软链(camera-init.service)"
systemctl restart camera-init.service 2>/dev/null || true
sleep 1

if [ "${MEDIAMTX_MANAGED:-0}" = "1" ]; then
  echo "[start] 启动自管 mediamtx (RTSP + WebRTC)"
  pkill -x mediamtx 2>/dev/null || true
  sleep 1
  mediamtx "$HERE/../mediamtx/mediamtx.yml" >/var/log/mediamtx.log 2>&1 &
  sleep 2
else
  echo "[start] 复用已有 snap mediamtx（如未运行/未开 WebRTC，请按 README 处理）"
  if ! pgrep -x mediamtx >/dev/null; then
    echo "[warn] 未检测到 mediamtx 进程，尝试 snap restart mediamtx"
    snap restart mediamtx 2>/dev/null || true
    sleep 2
  fi
fi

echo "[start] 启动 Agent (端口 ${AGENT_PORT:-8000})"
cd "$HERE"

# ---- 自带 RKNN runtime（可选，交付用） ----
# 目标板未预装 /usr/lib/librknnrt.so，但 OpenCV / ffmpeg / mediamtx / Python
# 依赖已按相同版本预装。二进制按 SONAME 找 "librknnrt.so"，而 Linux 加载器
# 不会自动搜索可执行文件所在目录（与 Windows DLL 不同），所以必须显式加进
# LD_LIBRARY_PATH。Agent 及其子进程（含 detect.py 拉起的 C++ --serve）都会继承。
# 文件不存在时什么都不做，仍走系统 /usr/lib，对已有环境零副作用。
# 换位置：RKNN_LIB_DIR=/path/to/libdir ./start.sh
RKNN_LIB_DIR="${RKNN_LIB_DIR:-$HERE/../rknn}"
if [ -f "$RKNN_LIB_DIR/librknnrt.so" ]; then
  RKNN_LIB_DIR="$(cd "$RKNN_LIB_DIR" && pwd)"
  export LD_LIBRARY_PATH="$RKNN_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  echo "[start] 使用自带 RKNN runtime: $RKNN_LIB_DIR/librknnrt.so"
fi

exec python3 agent.py
