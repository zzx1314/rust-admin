"""板端采集背包 Dashboard - 全局配置

所有可调整项都通过环境变量暴露，部署时按需覆盖即可。
"""
import os

# ---- 网络 ----
# 背包核心板在离线直连场景下的固定 IP（见 yd_test_v4.0.sh: test_network_interfaces）
BOARD_IP = os.getenv("BOARD_IP", "192.168.234.200")
AGENT_PORT = int(os.getenv("AGENT_PORT", "8000"))      # Agent(Win 侧浏览器访问)
RTSP_PORT = int(os.getenv("RTSP_PORT", "8554"))        # mediamtx RTSP
WEBRTC_PORT = int(os.getenv("WEBRTC_PORT", "8889"))     # mediamtx WebRTC 信令

# 浏览器通过该地址连接 mediamtx 的 WebRTC 信令（默认即板端 IP）
MEDIAMTX_WEBRTC_HOST = os.getenv("MEDIAMTX_WEBRTC_HOST", BOARD_IP)
MEDIAMTX_WEBRTC_PORT = WEBRTC_PORT

# ---- 摄像头（需求：usbcam0-3 = 左前/右前/右后/左后） ----
# 脚本 test_camera_init 通过 camera-init.service 把 usbcam0-3 软链到 video10-13
CAMERAS = {
    "cam0": {"device": "/dev/usbcam0", "title": "左前", "stream": "cam0"},
    "cam1": {"device": "/dev/usbcam1", "title": "右前", "stream": "cam1"},
    "cam2": {"device": "/dev/usbcam2", "title": "右后", "stream": "cam2"},
    "cam3": {"device": "/dev/usbcam3", "title": "左后", "stream": "cam3"},
    # 机器狗前置：3588 的现成 RTSP 由 relay.py 归一化后推到本机 dogcam path。
    # 这里只登记「一路」，没有 device，也绝不放源 URL（含账号密码）——
    # /api/cameras 会原样返回 CAMERAS，放进来就等于把凭据发给浏览器。
    "dogcam": {"title": "机器狗前置", "stream": "dogcam",
               "source": "external-rtsp-relay"},
}

# 红外网络摄像头（已是 mediamtx 上的 RTSP 源，yd_test_v4.0.sh: test_infrared_camera）
INFRARED_RTSP = "rtsp://192.168.234.200:8554/dog-ipcam"
INFRARED_STREAM = "dog-ipcam"

# ---- 机器狗前置 RTSP（dogcam）----
# 源 URL 含账号密码，只从环境变量或板端本地文件读取，不进版本库、不进接口/日志。
# 优先级：环境变量 DOG_RTSP_URL > 本目录 dog_rtsp_url 文件。
# 两者都没有时 relay 不启动，四路与红外完全不受影响；此时 /api/relay/status
# 返回 enabled=false，页面会自动收起第 6 张卡与 AI 摄像头里的「机器狗前置」。
# 注意：已配置过源地址但机器狗没连时 enabled 仍为 true，卡片与勾选项会保留。
DOG_RTSP_URL = os.getenv("DOG_RTSP_URL", "").strip()
DOG_RTSP_URL_FILE = os.getenv(
    "DOG_RTSP_URL_FILE",
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "dog_rtsp_url"),
)
DOG_RELAY_ENABLED = os.getenv("DOG_RELAY_ENABLED", "1") == "1"
# 720p30 档 relay 推流的目标 path（1080p60 档由 mediamtx 直拉 dogcam，不经 relay）
DOG_RELAY_STREAM = os.getenv("DOG_RELAY_STREAM", "dogcam720")
# 前置分辨率档位状态文件：运行时由接口读写，内容为 "1080p60" / "720p30"
# 不存在时默认按当前部署（方案 A 直拉）取 1080p60。
DOG_PROFILE_FILE = os.getenv(
    "DOG_PROFILE_FILE",
    os.path.join(os.path.dirname(os.path.abspath(__file__)), "dogcam_profile"),
)
DOG_PROFILES = ("1080p60", "720p30")

# ---- 摄像头 2P 端子硬件触发（干接点短接 / 电平型，待现场实测确认） ----
# 详见 DEVELOPER_GUIDE.md 第 13 章；测试工具 backend/tools/gpio_short_trigger.py
# 默认 -1 = 未启用；测试期可临时设为 46。
# ⚠️ 不可与 FILL_LIGHT_GPIO 同号：补光灯『关灯』会把该脚设为 out+0（持续拉低），
#    对短接型端子等于一直触发。两者要同时存在，必须用不同的脚。
CAM_TRIGGER_GPIO = int(os.getenv("CAM_TRIGGER_GPIO", "-1"))

# ---- 视频采集/编码参数 ----
# 实测 v4l2-ctl --list-formats-ext 显示摄像头原生输出 MJPG，
# 但 WebRTC 需要 H264，因此 ffmpeg 读取 MJPG 后用 H264 硬/软编码再推 RTSP。
CAPTURE_WIDTH = int(os.getenv("CAPTURE_WIDTH", "1280"))
CAPTURE_HEIGHT = int(os.getenv("CAPTURE_HEIGHT", "720"))
CAPTURE_FPS = int(os.getenv("CAPTURE_FPS", "30"))
CAPTURE_PIXFMT = os.getenv("CAPTURE_PIXFMT", "mjpeg")  # 摄像头枚举出的原生格式 MJPG
# H264 编码器：优先 RK3588 硬编 h264_rkmpp，缺失时回退 libx264
H264_ENCODER = os.getenv("H264_ENCODER", "h264_rkmpp")
H264_BITRATE = os.getenv("H264_BITRATE", "2000k")

# ---- 硬件路径（沿用 yd_test_v4.0.sh 的取值） ----
SHT4X_APP = os.getenv("SHT4X_APP", "/root/wenshidu/sht4x_app")   # 温湿度
MODEM_DEV = os.getenv("MODEM_DEV", "/dev/ttyUSB2")               # 4G(Quectel) AT 口
LOCATION_INTERVAL = int(os.getenv("LOCATION_INTERVAL", "60"))     # 定位采集周期(秒)
WIFI_IFACE = os.getenv("WIFI_IFACE", "")                          # 空=自动探测
FILL_LIGHT_GPIO = int(os.getenv("FILL_LIGHT_GPIO", "46"))         # 补光灯 GPIO（设为 -1 可禁用，例如该脚临时改作摄像头触发用）
LED_TEST_BIN = os.getenv("LED_TEST_BIN", "/root/led/test_rgbrun")  # LED 灯带闪烁程序
SSD_DEV = os.getenv("SSD_DEV", "/dev/nvme0n1")                    # SSD 设备
LINGS_PLAY_WAV = os.getenv("LINGS_PLAY_WAV", "/root/lingsi/call.wav")  # 聆思播放测试音
LINGS_CMD_FIFO = os.getenv("LINGS_CMD_FIFO", "/tmp/voice_assistant_cmd")  # asr 控制管道
LINGS_STATE_FILE = os.getenv("LINGS_STATE_FILE", "/tmp/lingsi_last_cmd")    # 最近一次命令状态（普通文件，供前端查询）
ASR_SERVICE = os.getenv("ASR_SERVICE", "asr")            # 聆思 asr 服务名
LINGS_TEST_VOLUME = os.getenv("LINGS_TEST_VOLUME", "70")  # 测试时设置音量

# ---- SOS/STOP 按钮（gpio-keys 输入设备，已用 evtest 实测） ----
GPIO_KEYS_NAME = os.getenv("GPIO_KEYS_NAME", "gpio-keys")  # evdev 设备名
# evtest 实测：code 639 = SOS（未知键），code 128 = KEY_STOP = STOP
# 若物理按键与标签相反，交换下面两个环境变量即可
BUTTON_MAP = {
    int(k): v for k, v in {
        os.getenv("SOS_KEYCODE", "639"): "SOS",
        os.getenv("STOP_KEYCODE", "128"): "STOP",
    }.items()
}

# ---- AI 识别（RKNN / YOLOv8 旁路检测）----
# 检测默认关闭：网页点「开始识别」才创建进程/线程，平时不占 NPU，也不影响首屏刷新。
_HERE = os.path.dirname(os.path.abspath(__file__))

# 统一模型根，默认与 start.sh 同树：backpack_dashboard/rknn/models/
DETECT_MODELS_DIR = os.getenv(
    "DETECT_MODELS_DIR",
    os.path.normpath(os.path.join(_HERE, "..", "rknn", "models")),
)
# C++ 常驻推理程序（需在板端 cmake --build build 编译一次）
DETECT_BIN = os.getenv(
    "DETECT_BIN",
    os.path.normpath(os.path.join(_HERE, "..", "rknn", "code", "code",
                                  "build", "yolov8_detector")),
)
# 抓帧临时目录（每路一张最新 JPEG，覆盖写）
DETECT_FRAME_DIR = os.getenv("DETECT_FRAME_DIR", "/tmp/detect_frames")
# 检出事件落盘（JSONL，防丢/取证回溯），默认后端目录，与位置记录文件同处
DETECT_EVENTS_FILE = os.getenv(
    "DETECT_EVENTS_FILE", os.path.join(_HERE, "detect_events.jsonl"))
# RTSP 旁路拉流地址（mediamtx 在本机；不碰 v4l2 设备节点）
DETECT_RTSP_HOST = os.getenv("DETECT_RTSP_HOST", "127.0.0.1")
DETECT_FPS = int(os.getenv("DETECT_FPS", "6"))        # 每秒推理次数（单路独占 NPU）
DETECT_FPS_MAX = int(os.getenv("DETECT_FPS_MAX", "12"))  # 网页可调上限；再高会拖垮 NPU
DETECT_CONF = float(os.getenv("DETECT_CONF", "0.25")) # 默认置信度阈值
DETECT_MAX_BOXES = int(os.getenv("DETECT_MAX_BOXES", "20"))

# ---- 版本 2.2：检出框的标签文字（中文 + 同类数量）----
# 由 detect.py._emit() 统一生成一次，板端 OSD / SSE / 事件落盘 / 浏览器 SVG
# 回退全部复用同一个 box["text"]，避免四处各算一遍导致数量或语言不一致。
# 例：火焰 3处 0.90（同类数量 = 当前这一帧过滤后实际画出的该类框数，末尾为置信度）
DETECT_SHOW_COUNT = os.getenv("DETECT_SHOW_COUNT", "1") == "1"
DETECT_SHOW_CONF = os.getenv("DETECT_SHOW_CONF", "1") == "1"   # 默认开启：标签末尾追加置信度（0.90）；设 0 关闭
DETECT_LABEL_UNIT = os.getenv("DETECT_LABEL_UNIT", "处")        # 改字后需重跑字库生成脚本

# ---- AI 识别摄像头范围（版本 3：恢复多路分时）----
# 红外 dog-ipcam 不参与 AI。任务下发可指定用其中哪几路；NPU 单模型串行，
# 多路是分时轮询而非真并发：每路实际帧率 ≈ DETECT_FPS / 路数。
DETECT_ALLOWED_CAMS = ["cam0", "cam1", "cam2", "cam3", "dogcam"]
DETECT_DEFAULT_CAM = os.getenv("DETECT_DEFAULT_CAM", "dogcam")
# 前端是否允许一次选多路（true=复选，false=单选）。保留配置便于回退到演示版单路口径。
DETECT_SINGLE_CAM = os.getenv("DETECT_SINGLE_CAM", "0") == "1"
DETECT_CAMS = [c.strip() for c in
               os.getenv("DETECT_CAMS", DETECT_DEFAULT_CAM).split(",")
               if c.strip()] or [DETECT_DEFAULT_CAM]

# ---- 版本 3：任务化识别（下发 job_id / duration / 连续命中即停 / 存证）----
# 识别时长上限，防止一次任务长时间占住 NPU（秒）
DETECT_JOB_DURATION_MAX = int(os.getenv("DETECT_JOB_DURATION_MAX", "120"))
DETECT_JOB_DURATION_DEFAULT = int(os.getenv("DETECT_JOB_DURATION_DEFAULT", "5"))
# 同一路连续命中多少次即判定"找到目标"并自动结束
DETECT_JOB_HIT_STREAK = int(os.getenv("DETECT_JOB_HIT_STREAK", "3"))
DETECT_JOB_HIT_STREAK_MAX = int(os.getenv("DETECT_JOB_HIT_STREAK_MAX", "10"))
# 任务存证根目录：jobs/<job_id>/<cam>_<job_id>.jpg + result.json
DETECT_JOBS_DIR = os.getenv("DETECT_JOBS_DIR", os.path.join(_HERE, "detect_jobs"))

# 板端本地画框：把框烧进视频再推一条 preview 流，浏览器只播带框视频、不再叠 SVG。
# 设为 0 可随时回退到「浏览器画框」，便于两种方案对照测试。
DETECT_OSD_ENABLED = os.getenv("DETECT_OSD_ENABLED", "1") == "1"
OSD_STREAM = os.getenv("OSD_STREAM", "preview")       # mediamtx 上的带框预览 path
OSD_FIFO = os.getenv("OSD_FIFO", "/tmp/detect_osd.fifo")  # RGBA 画布管道
OSD_LOG = os.getenv("OSD_LOG", "/tmp/detect_osd.log")     # ffmpeg 日志（preview 起不来时先看这里）
OSD_W = int(os.getenv("OSD_W", "640"))                # 画布尺寸，ffmpeg 放大到 1280x720
OSD_H = int(os.getenv("OSD_H", "360"))
OSD_FPS = int(os.getenv("OSD_FPS", "30"))             # 带框「视频」输出帧率（由源流驱动）
# 画布 FIFO 的写入帧率。旧版把它设成和视频一样的 30fps，等于要求纯 Python
# 每 33ms 渲染并写走 640*360*4≈0.9MB；只要渲染/管道写慢半拍，overlay 就会
# 反压主视频输入，帧在 ffmpeg 队列里越积越多，表现就是「AI 那一路慢好几秒」。
# 框本来就只按 DETECT_FPS(默认 6Hz) 更新，画布按 8fps 写、overlay 自动重复
# 上一帧即可；视频仍由主输入保持 30fps，画质与跟手程度不变。
OSD_FIFO_FPS = int(os.getenv("OSD_FIFO_FPS", "8"))
# 低延迟 ffmpeg 参数总开关（默认开）；设 0 回到旧参数，便于 A/B 对照测延时。
OSD_LOW_LATENCY = os.getenv("OSD_LOW_LATENCY", "1") == "1"
OSD_GOP = int(os.getenv("OSD_GOP", "30"))             # preview 关键帧间隔；设 15 可缩短 WHEP 首帧等待
# 启动确认窗口：ffmpeg 要依次完成 RTSP 连接、编码器初始化、打开并读走画布首帧。
# 旧版固定 4s，会把一个只是「慢一点」的 ffmpeg 直接 kill，前端永远看到
# 「preview 4s 内未建立」。RK3576 上实测放宽到 8s 更稳。
OSD_READY_TIMEOUT = float(os.getenv("OSD_READY_TIMEOUT", "8"))

# ---- SN 码 ----
SN_PARTITION = os.getenv("SN_PARTITION", "/dev/mmcblk0p5")  # 脚本 test_sn_code 取值分区
