"""板端 Agent：FastAPI 提供静态 Dashboard + REST/SSE 接口

约定：只读取数据的用 GET，会改变板端状态的用 POST。

- GET  /                      -> 前端 Dashboard（index.html）
- GET  /static/*              -> 前端静态资源（app.js / style.css / reader.js）
- GET  /api/cameras           -> 摄像头映射 + WebRTC 信令地址
- GET  /api/relay/status      -> 机器狗前置 relay 运行状态（不含源 URL）
- *    /api/whep/*            -> WHEP 信令同源代理（OPTIONS/POST/PATCH/DELETE，媒体仍直连）
- GET  /api/temp              -> 温湿度
- GET  /api/wifi              -> WiFi 扫描（总数/名称/信号）
- GET  /api/4g                -> 4G 状态（服务/网卡/AT/SIM/信号/IP/DNS/流量）
- GET  /api/env               -> 环境信息（IP/核心温度/SSD/系统/SN）
- GET  /api/ssd               -> SSD 容量&温度
- GET  /api/fan               -> 风扇
- GET  /api/sn                -> 设备 SN
- GET  /api/lingsi            -> 聆思小板状态（ADB/MIC/ASR/模式/联网）
- GET  /api/asr               -> asr 服务状态（active/activating/inactive/failed）
- GET  /api/detect/tasks      -> AI 识别已接入模型清单（只读，不加载模型）
- GET  /api/detect/status     -> 识别运行状态/模型契约/FPS/板端画框状态（默认关闭）
- POST /api/detect/start      -> 开启常驻识别 {"task","labels","conf","cams","fps",
                                 "plan_id","point_index","check_type","expect_count"}
                                 版本 3：cams 可多路分时；背包本地画框推 preview 流；
                                 带 plan_id = 航点计划模式（命中存证到计划目录 + 数量核对）
- POST /api/detect/plan       -> [航点] 登记计划 {"plan_id","point_index","check_type",
                                 "expect_count"}（check_type: exist|count）
- POST /api/detect/plan/clear -> [航点] 清掉计划（窗口结束但识别继续跑时用）
- POST /api/detect/job        -> 下发识别任务 {"job_id","task","labels","cams",
                                 "duration","streak","plan_id","expect_count"}；
                                 任一路连续 streak 帧命中即自动结束并画框存证，超时未命中则通知前端
- POST /api/detect/job/cancel -> 取消当前任务
- GET  /api/detect/jobs       -> 已落盘结果列表（递归读 result.json，含计划目录）
- POST /api/detect/stop       -> 停止识别、关闭板端画框并清空画面上的框
- POST /api/detect/config     -> 运行中热更新场景/类别/阈值/摄像头/帧率
- POST /api/detect/once       -> [调试接口，网页已不暴露] 抓当前帧识别一次 {"cam":"cam0"}
- GET  /api/debug             -> 最近一次完整遥测快照（排错用）
- GET  /api/location          -> 设备位置：最新点 + 最近轨迹(含时间)
- POST /api/location/refresh  -> 立即触发一次采集+解析+落盘
- GET  /location              -> 设备定位地图页（Leaflet + OSM）
- GET  /api/events            -> SSE 实时推送（遥测 + 按钮事件）
- POST /api/lingsi/mode       -> 切换模式 {"mode": "call"|"voice"}
- POST /api/lingsi/volume     -> 设置音量 {"level": 70}
- POST /api/lingsi/test       -> 触发聆思测试序列
- POST /api/asr/start         -> 启动 asr 服务（后台触发 systemctl start）
- POST /api/asr/stop          -> 关闭 asr 服务（后台触发 systemctl stop）
- POST /api/led/blink         -> LED 灯带闪烁
- POST /api/filllight         -> 补光灯开关 {"state": true/false}
"""
import os
import json
import asyncio
import threading
import http.client
from contextlib import asynccontextmanager
from urllib.parse import urlparse

from fastapi import FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import StreamingResponse, FileResponse, JSONResponse, Response
from fastapi.staticfiles import StaticFiles

import config
import telemetry
import control
import stream
import relay
import buttons
import location
import detect

FRONTEND_DIR = os.path.join(os.path.dirname(__file__), "..", "frontend")


class Broadcaster:
    def __init__(self):
        self.queues = set()
        self._loop = None

    def attach_loop(self, loop):
        self._loop = loop

    def subscribe(self):
        q = asyncio.Queue()
        self.queues.add(q)
        return q

    @staticmethod
    def _safe_put(q, s):
        try:
            q.put_nowait(s)
        except Exception:
            pass

    def publish(self, msg: dict):
        s = json.dumps(msg, ensure_ascii=False)
        # 跨线程安全推送：若当前不在事件循环线程，用 call_soon_threadsafe 投递
        if self._loop is not None and self._loop.is_running():
            for q in list(self.queues):
                self._loop.call_soon_threadsafe(self._safe_put, q, s)
        else:
            for q in list(self.queues):
                self._safe_put(q, s)

    def unsubscribe(self, q):
        self.queues.discard(q)


broadcaster = Broadcaster()


async def telemetry_loop(interval=5):
    while True:
        try:
            snap = await asyncio.to_thread(telemetry.snapshot)
            broadcaster.publish({"type": "telemetry", "data": snap})
        except Exception as e:
            broadcaster.publish({"type": "error", "msg": str(e)})
        await asyncio.sleep(interval)


async def location_loop(interval=60):
    """周期性采集并解析设备位置，落到板端本地文件（轨迹+最新点）。

    即使当前 NOCONN（无数据连接）也能采集小区号；联网后下一次循环会自动
    解析出经纬度（OpenCellID->IP 兜底）。失败不会中断循环。
    """
    import location as _loc
    while True:
        try:
            await asyncio.to_thread(_loc.update_location)
        except Exception as e:
            print(f"[location_loop] 更新失败: {e}", flush=True)
        await asyncio.sleep(interval)


def button_loop():
    # 按钮监听是阻塞式 read_loop()，放到守护线程，避免阻塞事件循环
    t = threading.Thread(target=buttons.read_buttons,
                         args=(broadcaster.publish,), daemon=True)
    t.start()


@asynccontextmanager
async def lifespan(app: FastAPI):
    # 推流启动里有 ffmpeg 编码器探测，放到线程执行，避免它同步阻塞事件循环、
    # 把遥测周期任务的首次采集（首屏数据来源）一并推迟。
    asyncio.create_task(asyncio.to_thread(stream.start_all))
    # 机器狗前置 RTSP 归一化 relay：未配置源时安静跳过，不影响四路
    asyncio.create_task(asyncio.to_thread(relay.start_all))
    broadcaster.attach_loop(asyncio.get_running_loop())
    asyncio.create_task(telemetry_loop())    # 起来立刻采一次，填充缓存
    asyncio.create_task(location_loop(config.LOCATION_INTERVAL if hasattr(config, "LOCATION_INTERVAL") else 60))
    button_loop()
    # 检测管理器只注入 SSE 发布函数，不启动任何进程/线程（默认关闭）
    detect.manager.set_publisher(broadcaster.publish)
    yield
    try:
        detect.manager.stop()
    except Exception:
        pass
    relay.stop_all()
    stream.stop_all()


app = FastAPI(lifespan=lifespan)

# ---- 跨源访问（业务平台 / 现场入口页面直接调本 Agent 时必需）----
# 板端自己的前端在 :8000，页面与 /api/* 同源，本来不需要 CORS。但一旦页面搬到
# 业务平台（http://39.107.55.13:3100）或现场入口（http://192.168.41.246:8080），
# 浏览器就是**跨源**：
#   - fetch /api/*            需要 Access-Control-Allow-Origin
#   - EventSource /api/events 同样受同源策略约束
#   - 路线编辑器读「AI 模型目录」= 浏览器 GET :8000/api/detect/tasks，缺了这层
#     直接报 `TypeError: Failed to fetch`
# 视频（:8889 WHEP）不经过这里，mediamtx 自己放行 webrtcAllowOrigins: ['*']。
# 注意：本 Agent **没有鉴权**，公网暴露时应收紧 AGENT_CORS_ORIGINS 白名单，
# 或者只让业务平台后端代理（服务端不受同源策略限制）。
CORS_ORIGINS = [o.strip() for o in os.getenv("AGENT_CORS_ORIGINS", "*").split(",")
                if o.strip()] or ["*"]
# 内网来源一律放行：现场入口可能架在任意内网机器/任意端口，板子的 wifi IP 还是
# DHCP 的，写死白名单会随 IP 变化失效。公网来源仍只认上面的显式白名单。
CORS_ORIGIN_REGEX = os.getenv(
    "AGENT_CORS_ORIGIN_REGEX",
    r"^https?://(localhost|127\.0\.0\.1"
    r"|10\.\d{1,3}\.\d{1,3}\.\d{1,3}"
    r"|192\.168\.\d{1,3}\.\d{1,3}"
    r"|172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3})(?::\d+)?$",
)
app.add_middleware(
    CORSMiddleware,
    allow_origins=CORS_ORIGINS,
    allow_origin_regex=CORS_ORIGIN_REGEX,
    allow_methods=["GET", "POST", "OPTIONS"],
    allow_headers=["*"],
    # 预检结果缓存 10 分钟：AI 面板每次改类别都会 POST，别每次都多一轮 OPTIONS
    max_age=600,
)
print("[agent] CORS allow_origins=%s regex=%s" % (CORS_ORIGINS, CORS_ORIGIN_REGEX),
      flush=True)


@app.middleware("http")
async def no_cache(request: Request, call_next):
    """禁止浏览器缓存前端与首页，避免迭代时加载到旧版 index.html/app.js
    （旧版字段结构与新版后端不一致会导致状态面板全显示 '-'）。"""
    resp = await call_next(request)
    path = request.url.path
    if path in ("/", "/location") or path.startswith("/static"):
        resp.headers["Cache-Control"] = "no-store, no-cache, must-revalidate"
        resp.headers["Pragma"] = "no-cache"
        resp.headers["Expires"] = "0"
    return resp


@app.get("/location")
async def location_page():
    """设备定位地图页（Leaflet + OSM，免 key）。"""
    return FileResponse(os.path.join(FRONTEND_DIR, "location.html"))


app.mount("/static", StaticFiles(directory=os.path.join(FRONTEND_DIR, "static")),
          name="static")


@app.get("/")
async def index():
    return FileResponse(os.path.join(FRONTEND_DIR, "index.html"))


@app.get("/api/cameras")
async def cameras():
    return {
        "cameras": config.CAMERAS,
        "infrared": {"rtsp": config.INFRARED_RTSP, "stream": config.INFRARED_STREAM},
        "webrtc": {"host": config.MEDIAMTX_WEBRTC_HOST, "port": config.MEDIAMTX_WEBRTC_PORT},
    }


@app.get("/api/relay/status")
async def relay_status():
    """机器狗前置 relay 是否常驻；只回状态，不回含账号密码的源 URL。"""
    return relay.status()


@app.get("/api/dogcam/profile")
async def dogcam_profile_get():
    """前置摄像头当前分辨率档位（1080p60 / 720p30）与对外 stream。"""
    return {
        "profile": relay.get_profile(),
        "stream": relay.dogcam_stream(),
        "profiles": list(config.DOG_PROFILES),
        "relay": relay.status(),
    }


@app.post("/api/dogcam/profile")
async def dogcam_profile_set(request: Request):
    """切换前置分辨率档位：1080p60（直拉）或 720p30（relay 重编码）。

    切换只启停 relay，不重启 mediamtx；dogcam 画面断几秒后恢复，其他五路不受影响。
    """
    b = await _detect_body(request)
    r = await asyncio.to_thread(relay.set_profile, b.get("profile"))
    return JSONResponse(r, status_code=200 if r.get("ok") else 400)


# ---- WHEP 信令同源代理 ----
# 页面在 8000 端口、mediamtx WHEP 在 8889 端口，浏览器直连属于跨源；
# 若 mediamtx 未放行 CORS，reader.js 的 OPTIONS/POST 会失败并降级到 iframe
# 内置播放页，四路视频就都无法叠加识别框。这里只转发「信令」，
# 媒体流仍是浏览器 <-> mediamtx 的 WebRTC 直连，不增加一帧解码/编码。
_WHEP_REQ_HEADERS = ("content-type", "if-match", "authorization", "accept")
_WHEP_RESP_HEADERS = ("content-type", "link", "etag", "location", "id", "accept-patch")


def _whep_forward(method, target_path, body, headers):
    conn = http.client.HTTPConnection("127.0.0.1", config.WEBRTC_PORT, timeout=5)
    try:
        conn.request(method, "/" + target_path.lstrip("/"), body=body, headers=headers)
        r = conn.getresponse()
        data = r.read()
        out = {}
        for k, v in r.getheaders():
            kl = k.lower()
            if kl not in _WHEP_RESP_HEADERS:
                continue
            if kl == "location":
                # mediamtx 返回的会话地址必须改成同源绝对路径，
                # 否则后面的 PATCH(trickle ICE) / DELETE 又会跨域。
                p = urlparse(v)
                v = "/api/whep/session/" + p.path.lstrip("/")
                if p.query:
                    v += "?" + p.query
            # Link 可能有多条（多个 ICE 服务器），reader.js 按 ", " 拆分
            out[k] = (out[k] + ", " + v) if k in out else v
        return r.status, out, data
    finally:
        conn.close()


async def _whep_proxy(request: Request, target_path: str):
    body = await request.body()
    headers = {k: request.headers[k] for k in _WHEP_REQ_HEADERS if k in request.headers}
    try:
        status, out, data = await asyncio.to_thread(
            _whep_forward, request.method, target_path, body or None, headers)
    except Exception as e:
        return JSONResponse({"error": "mediamtx WHEP 不可达: %s" % e}, status_code=502)
    return Response(content=data, status_code=status, headers=out)


@app.api_route("/api/whep/session/{session_path:path}",
               methods=["GET", "OPTIONS", "POST", "PATCH", "DELETE"])
async def whep_session(request: Request, session_path: str):
    """转发 WHEP 会话地址上的 trickle-ICE PATCH / 结束 DELETE。"""
    return await _whep_proxy(request, session_path)


@app.api_route("/api/whep/{stream}", methods=["GET", "OPTIONS", "POST"])
async def whep_stream(request: Request, stream: str):
    """转发 WHEP 的 ICE 服务发现(OPTIONS) 与 SDP 协商(POST)。"""
    return await _whep_proxy(request, "%s/whep" % stream)


@app.get("/api/temp")
async def temp():
    return JSONResponse(telemetry.get_temp_humidity())


@app.get("/api/wifi")
async def wifi():
    return JSONResponse(telemetry.get_wifi())


@app.get("/api/4g")
async def g4():
    return JSONResponse(telemetry.get_4g())


@app.get("/api/env")
async def env():
    return JSONResponse(telemetry.get_env())


@app.get("/api/ssd")
async def ssd():
    return JSONResponse(telemetry.ssd_info())


@app.get("/api/fan")
async def fan():
    return JSONResponse(telemetry.get_fan())


@app.get("/api/lingsi")
async def lingsi():
    return JSONResponse(telemetry.get_lingsi_status())


@app.post("/api/lingsi/test")
async def lingsi_test():
    data = telemetry.run_lingsi_test()
    broadcaster.publish({"type": "lingsi", "data": data})
    return JSONResponse(data)


@app.post("/api/lingsi/mode")
async def lingsi_mode(request: Request):
    try:
        body = await request.json()
    except Exception:
        body = {}
    # 兼容旧前端：传 mode=call/voice；新功能传 cmd=任意指令(phone/call 0 xxx/...)
    cmd = body.get("cmd") or body.get("mode", "voice")
    return JSONResponse(telemetry.send_lingsi_cmd(cmd))


@app.post("/api/lingsi/volume")
async def lingsi_volume(request: Request):
    try:
        body = await request.json()
    except Exception:
        body = {}
    return JSONResponse(telemetry.set_lingsi_volume(body.get("level", 70)))


# PC 端对讲脚本（与板子 phone 模式配套）；置于 backend/pc_tools/ 随项目部署，
# 由本路由提供下载，避免依赖演示电脑本地的文件路径。
PC_SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                         "pc_tools", "voice_call_pc.py")


@app.get("/api/lingsi/voice_call_pc.py")
async def lingsi_pc_script():
    """下载 PC 端对讲脚本 voice_call_pc.py（phone 模式需要）。"""
    if not os.path.exists(PC_SCRIPT):
        return JSONResponse({"ok": False, "error": "脚本未部署到 pc_tools/"}, status_code=404)
    return FileResponse(PC_SCRIPT, filename="voice_call_pc.py",
                        media_type="text/plain; charset=utf-8")


@app.get("/api/asr")
async def asr_status():
    """查询 asr 服务状态（active/activating/inactive/failed/unknown）。"""
    return JSONResponse(telemetry.get_asr_status())


@app.post("/api/asr/start")
async def asr_start():
    return JSONResponse(telemetry.start_asr())


@app.post("/api/asr/stop")
async def asr_stop():
    return JSONResponse(telemetry.stop_asr())


@app.get("/api/sn")
async def sn():
    return JSONResponse(telemetry.get_sn())


@app.get("/api/detect/tasks")
async def detect_tasks():
    """AI 识别：已接入模型包清单（只读，不加载模型、不占 NPU）。

    扫描 rknn/models/<task>/ 并做静态契约校验；目录签名变化时自动重扫，
    因此新增模型包无需重启。张量级校验在首次加载模型（start/once）时执行。
    """
    return JSONResponse(detect.scan_tasks(), headers={"Cache-Control": "no-store"})


@app.get("/api/detect/status")
async def detect_status():
    """识别运行状态。纯内存，不触发采集，不影响 5s 自动刷新。"""
    return JSONResponse(detect.manager.status(), headers={"Cache-Control": "no-store"})


async def _detect_body(request: Request):
    try:
        return await request.json()
    except Exception:
        return {}


@app.post("/api/detect/start")
async def detect_start(request: Request):
    """开启识别：拉起 --serve 常驻进程 + 单路 RTSP 旁路抽帧 + 板端本地画框。

    入参可选 `plan_id` / `point_index` / `check_type` / `expect_count`：航点任务模式。
    `check_type` 两类任务：`exist`（检测是否存在）/ `count`（检测数量是否正确）。
    给了计划 id 后，命中就把证据图 + result.json 落到 `detect_jobs/<plan_id>/pt<下标>/`；
    数量任务还会核对数量，不一致在 `plan_hit` 里带 `count_alert`，窗口结束未命中的
    数量任务发 `plan_miss`（同样是 `count_alert`）。
    """
    b = await _detect_body(request)
    r = await asyncio.to_thread(
        detect.manager.start,
        b.get("task"), b.get("labels"), b.get("conf"),
        b.get("cams"), b.get("fps"),
        b.get("plan_id"), b.get("point_index"), b.get("expect_count"),
        b.get("check_type"))
    return JSONResponse(r, status_code=200 if r.get("ok") else 400)


@app.post("/api/detect/plan")
async def detect_plan(request: Request):
    """[航点任务] 登记当前计划（运行中也能调）：
    {plan_id, point_index, check_type, expect_count}。

    `check_type`：`exist` = 只判是否存在（`expect_count` 不参与）；
    `count` = 判数量是否正确（`expect_count` 必填，检出数量与它比对，不一致时
    `plan_hit` 带 `count_alert=true`；窗口结束仍未命中发 `plan_miss`）。
    不传 `check_type` 时按旧口径推断：给了 `expect_count` 就是数量任务。
    """
    b = await _detect_body(request)
    r = await asyncio.to_thread(
        detect.manager.set_plan,
        b.get("plan_id"), b.get("point_index"), b.get("expect_count"),
        b.get("check_type"))
    return JSONResponse(r, status_code=200 if r.get("ok") else 400)


@app.post("/api/detect/plan/clear")
async def detect_plan_clear():
    """[航点任务] 清掉当前计划（窗口结束但识别继续跑时用）。"""
    r = await asyncio.to_thread(detect.manager.clear_plan)
    return JSONResponse(r)


@app.post("/api/detect/job")
async def detect_job(request: Request):
    """版本 3：下发一次带 ID 的识别任务。

    入参：{job_id, task, labels, conf, cams:[...], fps, duration, streak,
           plan_id?, point_index?, check_type?, expect_count?}
    - cams 可多路（NPU 分时轮询）；任一路连续 streak 帧命中即自动结束并画框存证；
    - duration 秒内未命中则自动结束并通知前端"未找到"；
    - 带 `plan_id` 时存证落到 `detect_jobs/<plan_id>/pt<下标>/`（航点任务）；
      `check_type=count` + `expect_count` 则核对数量（未命中 = 0 ≠ N 也算不一致），
      结论在 `job_done` 的 `count_ok / count_alert` 里；
    - 进度/结果通过 SSE 的 job_started/job_progress/job_done 事件推送。
    """
    b = await _detect_body(request)
    r = await asyncio.to_thread(
        detect.manager.submit_job,
        b.get("job_id"), b.get("task"), b.get("labels"), b.get("conf"),
        b.get("cams"), b.get("fps"), b.get("duration"), b.get("streak"),
        b.get("plan_id"), b.get("point_index"), b.get("expect_count"),
        b.get("check_type"))
    return JSONResponse(r, status_code=200 if r.get("ok") else 400)


@app.post("/api/detect/job/cancel")
async def detect_job_cancel():
    """取消当前任务（等同停止识别，另发 job_cancelled 事件）。"""
    r = await asyncio.to_thread(detect.manager.cancel_job)
    return JSONResponse(r)


@app.get("/api/detect/jobs")
async def detect_jobs():
    """列出已落盘的结果（递归读每个 result.json，含计划目录）。"""
    return JSONResponse(detect.manager.list_jobs(), headers={"Cache-Control": "no-store"})


@app.post("/api/detect/stop")
async def detect_stop():
    r = await asyncio.to_thread(detect.manager.stop)
    return JSONResponse(r)


@app.post("/api/detect/config")
async def detect_config(request: Request):
    b = await _detect_body(request)
    r = await asyncio.to_thread(
        detect.manager.configure,
        b.get("task"), b.get("labels"), b.get("conf"),
        b.get("cams"), b.get("fps"),
        b.get("plan_id"), b.get("point_index"), b.get("expect_count"),
        b.get("check_type"))
    return JSONResponse(r, status_code=200 if r.get("ok") else 400)


@app.post("/api/detect/once")
async def detect_once(request: Request):
    """抓当前帧识别一次；检测未开启时走一次性 CLI，不启动常驻服务。"""
    b = await _detect_body(request)
    r = await asyncio.to_thread(
        detect.manager.once,
        b.get("cam"), b.get("task"),
        b.get("conf"), b.get("labels"))
    return JSONResponse(r, status_code=200 if r.get("ok") else 400)


@app.get("/api/debug")
async def debug():
    """返回最近一次完整遥测快照（结构化 JSON），便于排查各板块为何为空。"""
    return JSONResponse(telemetry.snapshot())


@app.get("/api/location")
async def location_api(limit: int = 200):
    """返回设备位置：最新点 + 最近轨迹(含时间)。"""
    latest = location.load_latest()
    traj = location.load_trajectory(limit=limit)
    return JSONResponse({
        "latest": latest,
        "trajectory": traj,
        "open_cell_id_configured": bool(location.OPENCELLID_KEY),
        "cellocation_configured": True,   # cellocation 免 key，默认可用
    })


@app.post("/api/location/refresh")
async def location_refresh():
    """立即触发一次采集+解析+落盘（不等待周期任务）。返回新记录。"""
    try:
        rec = await asyncio.to_thread(location.update_location)
        return JSONResponse(rec)
    except Exception as e:
        return JSONResponse({"error": str(e)}, status_code=500)


@app.post("/api/led/blink")
async def led_blink():
    return JSONResponse(control.led_blink())


@app.post("/api/filllight")
async def filllight(request: Request):
    try:
        body = await request.json()
    except Exception:
        body = {}
    state = bool(body.get("state", True))
    return JSONResponse(control.fill_light(state))


@app.get("/api/events")
async def events():
    q = broadcaster.subscribe()

    async def gen():
        try:
            # 首帧直接复用最近一次快照（纯内存，毫秒级），页面一打开各框就有数据；
            # 不再在此同步跑完整采集（旧版会让首屏等十几秒）。最新值由 5s
            # 周期任务自动推送，前端无需轮询即可持续刷新。
            snap = telemetry.cached_snapshot()
            if snap is not None:
                yield f"data: {json.dumps({'type': 'telemetry', 'data': snap}, ensure_ascii=False)}\n\n"
            yield "retry: 3000\n\n"
            yield ": keep-alive\n\n"
            while True:
                msg = await q.get()
                yield f"data: {msg}\n\n"
        except asyncio.CancelledError:
            pass
        finally:
            broadcaster.unsubscribe(q)

    return StreamingResponse(gen(), media_type="text/event-stream",
                             headers={"Cache-Control": "no-cache",
                                      "X-Accel-Buffering": "no"})


if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=config.AGENT_PORT, log_level="info")
