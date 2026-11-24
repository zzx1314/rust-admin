# 数据采集背包 · 离线直连演示 Dashboard

> **仓库位置**：本目录是主仓库 `rust-admin/business/backpack_dashboard`（2026-11-19 并入，保留原提交
> 历史）；板端部署位置（2026-11-20 起）是 `/opt/xjpt/backpack_dashboard`（原 `/root/261113/backpack_dashboard`
> 已删除，软链也不留）。改代码/发布约定见同目录
> [`AGENTS.md`](AGENTS.md)，平台侧契约与排障见 `business/docs/backpack-video-integration.md`。
> 不在板子旁边时，用 `deploy/backpack-sim/` 在本地复刻 Agent + 相机 + 中继。

方案 A：板端 Python Agent（FastAPI） + 前端 WebRTC Dashboard。  
Windows 侧**零安装**——只需用 Chrome 打开 `http://192.168.234.200:8000`。

## 架构

```
USB 摄像头(usbcam0-3, MJPG/720p30)
      │ ffmpeg 转 H264(h264_rkmpp) 推 RTSP
      ▼
snap mediamtx(复用现场已有实例)  ──RTSP:8554──▶ dog-ipcam(红外, 已由现场提供)
      │ WebRTC WHEP(:8889)         ┌───────────────┐
      ▼                            │ 浏览器 WebRTC  │
  Agent(FastAPI :8000)  ◀──REST/SSE──▶ Dashboard(Chrome, 零安装)
  （封装 SHT4x/4G/WiFi/环境/SSD/风扇/聆思 采集 + LED/补光/聆思 控制 + SOS·STOP 监听）
```

**视频协议（重要）**

- 浏览器**无法原生播放 RTSP**，因此 5 路视频统一经 **mediamtx 的 WebRTC(WHEP)** 低延迟拉流。
- 前端使用 mediamtx 官方 `reader.js`（`static/reader.js`，已随项目内置），  
  连接 `http://<板IP>:8889/<stream>/whep`。
- 若 WHEP 被跨域拦截，前端会**自动回退**到 mediamtx 自带播放页 `http://<板IP>:8889/<stream>`（同源，无跨域问题）。
- **注意：VLC 能收 RTSP 只证明 `:8554` 监听正常，不等于 `:8889` 的 WebRTC 已开。**  
  浏览器视频依赖 WebRTC，请用下方命令确认/开启（见部署第 3 步）。

其余（温湿度/4G/WiFi/环境/SSD/风扇/聆思）经 Agent 的 REST/SSE 实时展示；  
LED 闪烁、补光灯开关由 Agent 控制接口触发；SOS/STOP 经 evdev 实时上报。

## 目录

```
backpack_dashboard/
├── backend/
│   ├── agent.py          # FastAPI 主程序
│   ├── config.py         # 全部可调参数（设备路径/端口/映射）
│   ├── telemetry.py      # SHT4x/4G/WiFi/环境/SSD/风扇/聆思 采集
│   ├── control.py        # LED 闪烁 / 补光开关
│   ├── stream.py         # ffmpeg 推流编排 + 自动拉起
│   ├── buttons.py        # SOS/STOP evdev 监听
│   ├── requirements.txt
│   └── start.sh          # 启动脚本
├── frontend/
│   ├── index.html
│   └── static/{app.js,style.css}
├── mediamtx/
│   └── mediamtx.yml      # RTSP + WebRTC 配置
└── README.md
```

## 部署到背包核心板（Ubuntu / RK3576）

1. 拷贝整个 `backpack_dashboard/` 到板端，例如 `/opt/xjpt/backpack_dashboard/`。
2. 安装 Python 依赖（一次性，**联网/离线二选一**）：

   **方式 A：联网安装**（板端能访问 PyPI 时，最简单）
   ```bash
   cd /opt/xjpt/backpack_dashboard/backend
   pip3 install -r requirements.txt
   ```

   **方式 B：离线安装**（板端无网络，但手头有另一台能上网的同架构机器时）

   > 板端 CPU 是 RK3576（aarch64），下载包必须也在 **aarch64 Linux** 上做，否则
   > pip 拉到的 x86_64 wheel 装不上去；`evdev` 还会因缺 `libevdev` 头文件导致本地编译失败。

   ① 在**能上网的同架构机器**上把全部 whl 下载到 `./package/`：
   ```bash
   cd /opt/xjpt/backpack_dashboard/backend
   mkdir -p package
   pip3 download \
       --dest ./package \
       --requirement requirements.txt
   ```
   下载完成后 `./package/` 里会有 `fastapi`、`uvicorn`、`pyserial`、`evdev`
   以及它们的全部传递依赖（whl 文件）。

   ② 把整个 `./package/` 目录**拷贝到板端**（任选一种方式：U盘、`scp -r`、网线直传等）：
   ```bash
   # 假设板端开了 ssh，从下载机执行：
   scp -r package root@192.168.234.200:/opt/xjpt/backpack_dashboard/backend/
   ```

   ③ 在板端**离线安装**（pip 禁止访问网络索引，只从 `./package/` 找包）：
   ```bash
   cd /opt/xjpt/backpack_dashboard/backend
   pip3 install \
       --no-index \
       --find-links ./package \
       --requirement requirements.txt
   ```
   `--no-index` 是关键——告诉 pip 不要去 PyPI 找，强制只用本地 `./package/` 里的 whl。
   这样即便现场临时断网也能完成安装。

   > `package/` 目录与平台/Python 版本绑定、体积大，已在 `.gitignore` 中排除，
   > 不要进版本控制。
3. 确认 mediamtx（snap）已安装并正在运行，且 **WebRTC 监听 `:8889` 已开启**（浏览器靠 WebRTC 拉流）：
   ```bash
   sudo snap install mediamtx
   sudo snap connect mediamtx:network

   # 查看当前 snap 配置（确认 webrtcAddress 为 :8889、webrtc 开启）
   sudo snap get mediamtx

   # 若 WebRTC 未开，编辑 snap 配置文件后重启：
   sudo vim /var/snap/mediamtx/common/mediamtx.yml
   #   设 webrtc: yes
   #      webrtcAddress: :8889
   #      webrtcAllowOrigin: '*'      # 允许跨域 WHEP（默认即 *，保留即可）
   sudo snap restart mediamtx

   # 验证 WebRTC WHEP 端点是否就绪（返回 200/204 即正常）：
   curl -s -o /dev/null -w "WHEP HTTP %{http_code}\n" -X OPTIONS http://127.0.0.1:8889/cam0/whep
   ```
   > 红外 `dog-ipcam` 现已由该 mediamtx 提供（VLC 可拉 rtsp://192.168.234.200:8554/dog-ipcam），  
   > 本项目**不再启动第二个 mediamtx**，只把四路摄像头推到这个已有实例。  
   > 浏览器视频能否出图，取决于 `:8889` 的 WebRTC 是否就绪，与 RTSP 是否可用无关。
4. 确认 H264 硬编码可用（否则自动回退软编 libx264，CPU 占用更高）：
   ```bash
   ffmpeg -hide_banner -encoders 2>/dev/null | grep h264_rkmpp
   # 有输出即可用；stream.py 启动时会自动探测并选择 h264_rkmpp / libx264
   ```

5. 启动（拉起 camera-init → 复用已有 mediamtx → ffmpeg 推流 → Agent）：
   ```bash
   sudo bash /opt/xjpt/backpack_dashboard/backend/start.sh
   ```
6. Windows 笔记本用网线直连背包，手动配置同网段 IP（如 192.168.234.x），  
   浏览器打开 `http://192.168.234.200:8000`。

## 需要按现场核对的可调项（config.py / 环境变量）

| 项                 | 默认值                        | 说明                                                                      |
| ----------------- | -------------------------- | ----------------------------------------------------------------------- |
| `MODEM_DEV`       | `/dev/ttyUSB2`             | 4G 模块 AT 口（已现场确认为 ttyUSB2）                                              |
| `SHT4X_APP`       | `/opt/xjpt/wenshidu/sht4x_app` | 温湿度程序路径                                                                 |
| `LED_TEST_BIN`    | `/opt/xjpt/led/test_rgbrun`    | LED 闪烁程序（手动按钮与计划灯光共用同一个进程句柄）                                |
| `BUTTON_MAP`      | SOS=639 / STOP=128         | **已用 evtest 实测**：code 639=未知键→SOS，code 128=KEY_STOP→STOP；若物理相反请交换两个环境变量 |
| `FILL_LIGHT_GPIO` | `46`                       | 补光灯 GPIO；**设为 `-1` 可禁用**（该脚临时改作摄像头触发用时）                                 |
| `CAM_TRIGGER_GPIO` | `-1`（未启用）                | 摄像头 2P 端子触发脚；**不可与 `FILL_LIGHT_GPIO` 同号**，同号时补光灯接口会直接拒绝执行 |
| `LIGHT_NIGHT_START` / `LIGHT_NIGHT_END` | `18:00` / `06:00` | 计划灯光「仅夜间自动开灯」的夜间时间窗（板端本地时间，允许跨零点）；**起止相同 = 关闭夜间判定**（退化成到点就开）；用 `GET /api/lights` 核对当前是否夜间 |
| `LIGHT_MAX_ON_SECS` | `900`                    | 计划灯光的看门狗：超过这么久自动关灯（客户端掉线/被 kill 时兜底）；手动按钮开的灯不受限 |
| `H264_ENCODER`    | `h264_rkmpp`               | 摄像头→H264 编码器，缺失自动回退 `libx264`                                           |
| `WIFI_IFACE`      | 自动探测                       | 留空自动取首个 `iw dev` 接口                                                     |
| `ASR_SERVICE`     | `asr`                      | 聆思 asr 服务（默认不启动，测试时拉起）                                                  |
| `LINGS_CMD_FIFO`  | `/tmp/voice_assistant_cmd` | 聆思模式/音量控制管道                                                             |
| `DOG_RTSP_URL`    | 空（不启用）                  | 机器狗前置 RTSP 源；空则读 `backend/dog_rtsp_url`，两者都无则 relay 不启动                  |

## 机器狗前置摄像头（`dogcam`）接入与 AI 识别

3588 板卡提供一路现成 RTSP（实测 1920×1080@60 / H264 High@L4.0 / full-range）。
`backend/relay.py` 把它拉到背包并**归一化**成与 `cam0~cam3` 逐字段一致的
`1280×720@30 / High@L3.1 / 无 B 帧 / GOP30 / 2Mbps / yuvj420p(pc) / 无音轨`，
推到本机 mediamtx 的 `dogcam` path，随后 WHEP 播放与 AI 旁路抽帧都走同一路流。

**配置源地址（二选一，都不要把带密码的 URL 写进仓库）：**

```bash
# 方式 A：环境变量，随 start.sh 一起下发
DOG_RTSP_URL='rtsp://<账号>:<密码>@192.168.234.1:8554/test' \
  sudo bash /opt/xjpt/backpack_dashboard/backend/start.sh

# 方式 B：板端本地文件（推荐，单行明文，权限收紧）
printf '%s\n' 'rtsp://<账号>:<密码>@192.168.234.1:8554/test' \
  > /opt/xjpt/backpack_dashboard/backend/dog_rtsp_url
chmod 600 /opt/xjpt/backpack_dashboard/backend/dog_rtsp_url
```

- 密码里的 `#` / `!` 必须转义成 `%23` / `%21`；命令行里 URL 要用**单引号**。
- 源 URL 只作为 ffmpeg 参数存在于进程内存，**不会**出现在 `/api/cameras`、`/api/relay/status`、前端或日志里；`ffmpeg` 的 stderr 直接丢弃，要看告警请按 `ffprobe核验回执_分辨率与编码参数修订建议.md` §8.2.5 命令块 D 手动跑。
- 运行状态：`GET /api/relay/status` → `{"enabled","running","restarts","last_error","stream"}`。进程意外退出由 5 秒看门狗自动拉起，连续失败按 5/10/20/30s 退避。
- 页面新增第 6 张视频卡（3 列网格正好两行）。版本 2 曾为**五路单选**（默认「机器狗前置」`dogcam`）；**版本 3 已恢复五路可多选**（多路分时轮询），详见文末「版本 3」章节。检出事件 `cam` 字段记为实际识别路。

### 背包未连接机器狗时的行为

页面启动时会查询一次 `/api/relay/status`（1.2s 超时兜底），据此决定这一路的 UI：

| 现场状态 | `enabled` | `running` | 第 6 张视频卡 | AI 单选项（默认 dogcam） |
|---|---|---|---|---|
| **从没配过源**（无 `dog_rtsp_url`、无环境变量，或 `DOG_RELAY_ENABLED=0`） | `false` | `false` | **保留**，白底橙框写明「未配置机器狗前置源」及配置方法；不创建 WHEP reader | **禁用 dogcam** 并自动补选第一路；状态栏与 AI 面板都说明「AI 目标已临时改为 cam0，配置并重启 agent 后恢复默认」 |
| **配过源，但狗没连 / 3588 没推流** | `true` | 随看门狗重启抖动 | **保留**，WHEP 持续重试并显示连不上 | **保留且默认选中**（此时推流不通，建议现场改选四路之一） |
| **配过源且狗正常连上** | `true` | `true` | 正常出图 | 默认选中 |

判据是 `enabled`（**只看有没有配置源地址，不代表当前是否有流**），不是链路连通性。这样设计的原因：现场部署时配置文件是常驻的，临时拔网线不该把 UI 删掉——relay 的 5 秒看门狗会自动重连。

> 早期版本在 `enabled=false` 时把整张卡 `display:none`，但那张卡仍在后台创建 WHEP reader，
> 连不上就降级成 iframe：既留下一个黑框，又被状态栏统计成「1 路走了内置播放页」，用户完全
> 不知道原因。现在未配置源时**根本不创建 reader**，改为一张写明恢复方法的占位卡，
> 并在 `/api/detect/status` 里回传 `relay` 状态供状态栏解释。

两点需要留意：

- 该判断**只在页面加载时执行一次**。运行中才掉线不会自动收起卡片，恢复出图也不需要刷新（WHEP 自带重试）。刷新页面会按当时状态重新判断。
- **已配置但狗没连时建议改选四路之一**：默认 AI 目标就是 `dogcam`，若 3588 没推流，`dogcam` path 没有 publisher，AI 抓帧线程会不断重连且拿不到帧。版本 2 只推一路，不再有「摊薄其它路帧率」的问题。

### 版本 2：AI 单路选择 + 背包本地画框

| 项 | 版本 1（已归档） | 版本 2（当前） |
|---|---|---|
| AI 目标 | cam0~cam3 + dogcam 可多选，NPU 分时 | **五路单选**，默认 `dogcam`，NPU 独占一路 |
| 框在哪画 | 浏览器 SVG 叠加坐标 | **背包本地** `backend/osd.py` 烧进 H264 帧 |
| 前端看到什么 | 干净流 + SVG 图层 | 该路卡片自动切到 `preview` 带框流，浏览器不再叠框 |
| 画框链路异常 | — | 自动回退浏览器叠加，状态栏给出具体原因 |
| 框上文字 | 英文类名 + 置信度 | **中文类名 + 同类数量 + 置信度**，例「火焰 3处 0.90」 |

链路（仅 AI 开启时存在）：`mediamtx/<cam> → osd.py ffmpeg(overlay + 重编码) → mediamtx/preview → WHEP`。
`osd.py` 用纯标准库按 640×360 RGBA 画布画矩形与 16×16 中英文点阵标签，
经 FIFO 交给 ffmpeg 放大叠加并编码；**板端不需要 Pillow / OpenCV / 字体文件**，
中文字模已预烘焙在 `backend/cjk_font.py`，AI 关闭时零额外负载。
切换目标路时 `preview` 只换输入源、path 不变，前端无需重新协商 WHEP。

#### 中文标签与同类数量（版本 2.2）

框上文字形如 **`火焰 3处 0.90`**：中文类名来自模型包 `meta.json` 的 `labels.<类>.zh`，末尾 `0.90` 是该框置信度（设 `DETECT_SHOW_CONF=0` 可关闭），
`3` 是**当前这一帧**通过类别 / 阈值过滤后、实际画出的同类框数（受 `DETECT_MAX_BOXES` 截断影响，
即「可见框数」而非模型原始检出数）。文字由 `detect.py._emit()` **统一生成一次**并写入
`box["text"]`，板端 OSD、SSE 事件、JSONL 落盘、浏览器 SVG 回退全部复用同一份，四处必然一致。

SSE / 落盘的 `event["counts"]` 另给 `{模型类别名: 本帧数量}`，平台可直接消费，不必自己再数框。
`/api/detect/status` 的 `label_format` 描述当前格式，`osd.missing_glyphs` 报告字库缺字。

| 环境变量 | 默认 | 作用 |
|---|---|---|
| `DETECT_SHOW_COUNT` | `1` | 显示「N处」；设 `0` 只显示中文类名 |
| `DETECT_SHOW_CONF` | `1` | 默认在末尾追加置信度（例「火焰 3处 0.90」）；设 `0` 关闭 |
| `DETECT_LABEL_UNIT` | `处` | 数量单位；改成别的汉字后需重跑字库生成脚本 |

**改中文名 / 加新场景后必须重跑字库**（字模离线烘焙，运行时不会读字体）：

```bash
# 在开发机执行（需要 Pillow 与一个中文字体；Windows 默认用 SimHei）
python backend/tools/gen_cjk_font.py
# 把新生成的 backend/cjk_font.py 同步到板端，重启 agent
```

字库由该脚本扫描 `rknn/models/*/meta.json` 的全部中文别名自动生成，当前 299 个字模（204 汉字 + 95 个 ASCII，覆盖 COCO80 等 10 个场景）。
**漏同步或新增汉字**时：该字在画面上显示为空心方框，状态栏列出 `中文字库缺字：…`；
若整个 `cjk_font.py` 没同步，会自动降级回旧英文字库（汉字显示 `?`）并在状态栏告警。
部署时 `backend/cjk_font.py` 必须与 `osd.py` 一起拷过去。

相关配置：`DETECT_DEFAULT_CAM`（默认 `dogcam`）、`DETECT_ALLOWED_CAMS`（五路）、
`DETECT_OSD_ENABLED`（设 `0` 强制回退浏览器画框，便于两种方案对照测试）、
`DETECT_FPS`（默认 `6`，每秒推理次数）、
`OSD_STREAM`（默认 `preview`）、`OSD_W` / `OSD_H` / `OSD_FPS`（视频 30fps）、
`OSD_FIFO_FPS`（画布 8fps）、`OSD_LOW_LATENCY`（默认 `1`）、`OSD_GOP`（默认 `30`）、
`OSD_FIFO`、`OSD_LOG`（默认 `/tmp/detect_osd.log`，ffmpeg 的 stderr 落点）、
`OSD_READY_TIMEOUT`（默认 `8` 秒，启动确认窗口）。
`/api/detect/status` 的 `osd.log_tail` 会带回 ffmpeg 日志最后 8 行，并显示在状态栏，
现场无需 SSH 上板即可区分「path 未登记 / 源流无 publisher / 编码器不可用」。
`mediamtx.yml` 必须包含 `preview: source: publisher`，**改完必须重启 mediamtx**。

#### 画框链路排错（改完配置先看这三处）

| 现象 | 判据 | 处理 |
|---|---|---|
| 点「开始识别」后该路画面变黑 / 卡片下方出现黑框 | `/api/detect/status` 里 `osd.ready=false` 或 `osd.error` 非空 | 多半是 mediamtx 没重启、`preview` path 未登记，或 `dogcam` 没有 publisher；看 `tail -50 /tmp/detect_osd.log` |
| 狗的画面整张消失，四路正常 | `curl http://127.0.0.1:8000/api/relay/status` 的 `enabled` / `running` | `enabled=false` = 板端没找到 `backend/dog_rtsp_url`（或 `DOG_RTSP_URL`）：第 6 张卡显示白底橙框占位提示，AI 目标临时改为 cam0；写入源并重启 agent 即恢复。`enabled=true, running=false` = 3588 没推流，relay 5s 看门狗在重连 |
| 状态栏显示「preview 流 N s 内未建立」 | `osd.ready=false`、`osd.error` 与 `osd.log_tail` | RTSP 输入已关掉默认流探测（`-analyzeduration 0`）；仍失败通常是 `preview` path 未登记、源流没有 publisher 或 `h264_rkmpp` 不可用。日志尾部已直接显示在状态栏 |
| 状态栏显示「1 路走了内置播放页」 | 有卡片带 `no-overlay` 类 | 该路 WHEP 协商全失败后降级成内置 iframe。未配置 dogcam 源时不再创建 reader，此提示不应再因 dogcam 出现 |
| 想立刻恢复原始画面 | — | 网页点「停止识别」；或设 `DETECT_OSD_ENABLED=0` 重启 agent，整体回退浏览器画框 |

`osd.start()` 有 `OSD_READY_TIMEOUT`（默认 8 秒）启动确认窗口：ffmpeg 若因 path 不存在 /
编码器不可用立刻退出就返回失败，前端**不会**把该路从干净流切走；只有画布确实被 ffmpeg
读走（`ready=true`）才允许切 `preview`。这里曾固定 4 秒并要求 2 帧，而 ffmpeg 默认
`analyzeduration` 最长 5 秒、探测没做完就不会打开画布 FIFO，于是把一个只是「慢一点」的
进程直接 kill，永远报「preview 流 4s 内未建立」。现在 RTSP 输入加了
`-analyzeduration 0 -probesize 32768 -flags low_delay`（与 `detect.py` 抓帧一致），
窗口放宽到 8 秒、只要求写出 1 帧。
切过去后若 6 秒内没收到视频轨（或 WHEP 协商全失败），
前端会自动切回原干净流、恢复浏览器叠框，并在状态栏给出原因；
同一个 OSD 会话内不再反复试探，避免每 3 秒状态轮询造成画面抖动。
`osd.session` 每次启动 / 换目标路递增，重新点「开始识别」即可再试一次。
WHEP 会话在切流时会主动 `DELETE`，不再在 mediamtx 侧堆积。

#### AI 那一路的延时：原理与低延迟优化（版本 2.1）

**先答「是不是每一帧都识别」：不是。** 四条节拍彼此独立：

| 环节 | 频率 | 说明 |
|---|---|---|
| 视频采集 / 推流 | 30 fps | 由 `stream.py` / `relay.py` 决定，与 AI 无关 |
| 旁路抓帧 | 8 fps | `detect.py` 的 ffmpeg 输出 MJPEG，只保留「最新一帧」覆盖写 |
| NPU 推理 | 默认 6 次/秒 | `_sched_loop()` 补偿式节拍，单路独占；实测单次约 116ms，物理上限约 8.6 次/秒 |
| 框叠加到视频 | 视频 30 fps / 画布 8 fps | 一次推理结果被缓存，画布复用同一份框，视频每帧都叠加 |

所以「框」最多滞后 `抓帧 125ms + 推理 116ms ≈ 0.25s`。**几秒级延时与推理帧率无关**，
只可能来自 preview 这条二次编码链路的帧队列。

**6 秒是怎么堆出来的（旧版两个原因叠加）**

1. **画布 FIFO 按 30fps 写，Python 跟不上。** 旧版要求纯标准库每 33ms 渲染并写走
   640×360×4 ≈ 0.9MB（27MB/s）。但 rawvideo demuxer 的 `-framerate 30` 是**按帧序号**
   打时间戳、不管实际写了多久：只要 Python 实际只有 10~15fps，画布 PTS 就持续
   **领先于真实时钟**，主视频 PTS 却按真实 30fps 推进。overlay 的 framesync 于是
   等待主视频追上，主视频帧在 demux 队列里越积越多，直到队列上限形成稳态 ——
   表现正好是「稳定落后几秒」。
2. **两个输入时钟不对齐。** RTSP 输入的 `start_time` 由 mediamtx 决定（可能是任意大值），
   rawvideo FIFO 却从 0 开始；framesync 把差值理解成「某一路落后」，继续缓存。

**已做的优化（`backend/osd.py`）**

- 画布与视频帧率解耦：`OSD_FIFO_FPS=8`（≈ 推理节奏），视频仍由主输入保持 30fps，
  overlay `repeatlast=1 / eof_action=pass` 复用上一张画布；单张画布覆盖 3.75 帧视频，
  观感不变，管道写入量从 27MB/s 降到 7MB/s，主视频不再被反压。
- 两个输入都用 `setpts=N/(fps*TB)` 按帧序号重造时间戳：起点都归零、各自匀速。
- 输入 `-fflags +genpts+nobuffer`、`-avioflags direct`、`-analyzeduration 0`、
  `-probesize 32768`、`-thread_queue_size 8/1`；输出 `-muxdelay 0`、`-muxpreload 0`、
  `-flush_packets 1`、`-vsync 0`；去掉会让 muxer 预读平移的 `-avoid_negative_ts make_zero`；
  软编加 `-tune zerolatency`。
- 无检出时直接复用预置全透明帧（零渲染零拷贝）；Linux 上把 FIFO 缓冲扩到一帧，
  0.9MB 一次 `os.write` 写完，避免几十次系统调用打乱节拍。
- 状态栏实时显示 `preview 低延迟：视频 30fps / 画布 8fps（实测写入 x，渲染 y ms）`；
  实测写入低于目标 70% 会变红并提示继续调低 `OSD_FIFO_FPS`。

**验收 / 定位剩余延时**

| 动作 | 期望 | 判定 |
|---|---|---|
| `DETECT_OSD_ENABLED=0` 重启 agent 后开 AI | 该路延时与其余四路一致 | 差异 100% 来自 preview 链路（浏览器叠框零额外编码） |
| 状态栏「实测写入」≈「画布 fps」、渲染 < 30ms | 无背压 | 明显偏低就把 `OSD_FIFO_FPS` 降到 `6` 或 `4` |
| 板端低缓存直接播 `preview` | 与网页延时接近 | 播放器也慢 → 后端 ffmpeg/mediamtx；只有网页慢 → WHEP/播放器侧 |
| `OSD_LOW_LATENCY=0` 对比 | 仅关输入/输出缓冲参数 | 帧率解耦与 setpts 仍生效，用于单独量化缓冲参数收益 |

```bash
ffplay -fflags nobuffer -flags low_delay -rtsp_transport tcp \
  rtsp://127.0.0.1:8554/preview
curl -s http://127.0.0.1:8000/api/detect/status \
  | python3 -m json.tool | grep -A16 '"osd"'
```

> `OSD_GOP` 默认 30（1 秒）；WHEP 新建会话要等下一个 IDR，设 `15` 可把首帧等待减半，
> 但不改变稳态延时。`h264_rkmpp` 硬编内部可能有 1~3 帧缓冲，这部分无法从 ffmpeg 参数消除。
> 若还要再降只能改结构：让 `osd.py` 直接读 3588 原始 RTSP（跳过 relay 一跳），或把 overlay
> 合并进 `stream.py` 的主推流 ffmpeg（彻底去掉二次编码与第二次 mediamtx 转发）。
> preview 仍是二次编码流，dogcam 还多经 relay 一次编码，评估端到端延时要把这两段算进去。
> 标签文字已是「中文类名 + 同类数量」（16×16 预烘焙点阵，板端无需字体文件）；
> 改中文名后记得重跑 `backend/tools/gen_cjk_font.py`，详见上文「中文标签与同类数量」。

## 摄像头 2P 端子触发测试（`CAM_TRIGGER_GPIO`）

四路摄像头的 2P 端子厂家称「短接即可触发拍照」，但**只验证过短接有效**，
尚不确定它是「干接点/光耦输入」还是「电平型输入」——两者对 GPIO 的驱动要求完全不同。

### 第一步：先搞清端子到底是什么信号

⚠️ **注意：端子悬空时摄像头本来就正常出流**，所以「盯着画面看触发有没有生效」是**无效的观测方法**。
触发多半是"抓一张静止图"或"发一个事件"，不会改变视频流。先用 `trigger_probe.py` 探明方向和作用：

```bash
cd /opt/xjpt/backpack_dashboard/backend/tools

# ① 环境指纹：USB 接口构成（有没有 HID 接口）/ 输入设备 / UVC 控件 / 内核日志
python3 trigger_probe.py --video /dev/video121

# ② 监听见面事件：运行后**手动短接 2P 端子**，看有没有按键事件
sudo python3 trigger_probe.py --watch-input --seconds 60

# ③ 差分探测（推荐）：自动触发前后各采一次快照，打印差异
python3 trigger_probe.py --diff --video /dev/video121 --gpio 46 --count 3
python3 trigger_probe.py --diff --video /dev/video121      # 不接 GPIO → 手动短接后回车
```

判读：

| 现象 | 结论 |
|------|------|
| USB 接口里有 `class=03 HID` | 端子是「按键型」触发，短接等于按下快门键 |
| `--watch-input` 抓到 KEY 事件 | 同上，且 Linux 侧可用 evdev 检测 |
| UVC 控件值发生变化 | 有 XU 反馈，可做成闭环 |
| 四项全无变化 | 多半是摄像头内部行为（存图），或端子其实是**输出**，需万用表/示波器 |

### 第二步：推测是输入，再试驱动方式

工具：`backend/tools/gpio_short_trigger.py`

```bash
cd /opt/xjpt/backpack_dashboard/backend/tools

# 0) 先看引脚现状（补光灯关灯后可能是 out+0 = 持续拉低 = 持续触发）
python3 gpio_short_trigger.py --gpio 46 --status
python3 gpio_short_trigger.py --gpio 46 --release     # 恢复高阻，停止持续触发

# 1) 三种驱动方式依次自动跑一遍，人工观察哪种有效（推荐）
python3 gpio_short_trigger.py --gpio 46 --sweep --count 3 --interval 3000

# 2) 也可单独测某一种
python3 gpio_short_trigger.py --gpio 46 --mode short   # 空闲高阻 / 触发拉低（等价手动短接）
python3 gpio_short_trigger.py --gpio 46 --mode low     # 空闲推高 / 触发拉低
python3 gpio_short_trigger.py --gpio 46 --mode high    # 空闲拉低 / 触发推高
```

结果判读：

| 现象 | 结论 | 下一步 |
|------|------|--------|
| 只有 `short` 有效 | 干接点/光耦输入型，只能模拟短接 | 建议加一级 MOS 管做隔离（文档 13.12） |
| `short` 和 `low` 都有效 | 电平型且低有效 | 空闲态用高阻即可，不必推高 |
| 只有 `high` 有效 | 电平型且高有效 | 注意空闲时该脚是拉低的 |
| 三种都无效 | 未共地 / 触发模式没切 / 电流不足 / 脚没引到端子 | 按 上文 4 步排查 |

> 2026-10-15 实测进展：**触发语义已确认** —— 端子是**电平型输入**、
> **空闲为高、靠"高→低"的下降沿触发**（必须先 `echo 1` 再 `echo 0`）。
> 这也解释了"手动短接能触发"：短接 = 从高拉到低，本质是同一个动作。
> 因此推荐 `--mode low`（空闲推高 / 触发拉低），PWM 方案同样适用，不需要 MOS 管模拟短接。
> 详见 `DEVELOPER_GUIDE.md` 13.13。

### 第三步（关键实验）：数帧数，判断是否"外触发出流"

⚠️ **不能靠看画面**：厂商软件只把最新一帧显示在窗口、刷新很快，人眼分不出 2fps 和 30fps，
也数不出张数。改用**在固定时间窗口内统计实际交付的帧数**——结论是数字，不用肉眼判断。

```bash
cd /opt/xjpt/backpack_dashboard/backend/tools

# 一条命令跑完三段：基线 → 低频触发 2Hz → 高频触发 30Hz，并自动给出结论
python3 trigger_frame_test.py --video /dev/video121 --gpio 46
```

**⚠️ 实测结论（2026-10-15）：这条路已否 —— 2P 端子与视频流出帧无关。**

| 阶段 | 触发频率 | 输出下降沿数 | 实收帧数 | 平均 fps |
|------|---------|-------------|---------|---------|
| ① 不触发（基线） | — | 0 | 278 | 27.8 |
| ② 低频触发 | 2 Hz | ≈20 | 284 | 28.4 |
| ③ 高频触发 | 30 Hz | ≈299 | 285 | 28.5 |

触发 0 / 20 / 299 次，帧数都是 ~280 → **触发与出帧完全无关**。
该端子等效于**快门按键**（触发一次存一张静图），视频流端点始终自由运行 ~28.5fps。
**因此不能用它做四路帧同步。** 替代方案见 `DEVELOPER_GUIDE.md` 13.14：

1. **向厂家追问**（优先）：端子是 SNAP 快门还是 SENSOR 外触发同步？是否支持外触发/多机同步模式、有无 FSIN 同步引脚、如何切换？
2. **软件方案**：服务器端 ffmpeg `xstack` 合流成一路（同帧由服务器保证，浏览器只拉一路）
3. **换硬件**：选带 FSIN 输入 + 同步输出、提供 SDK 的工业相机模组

> 四路推流（`stream.py`）与网页演示**不受影响**——本来就没用触发；
> 仅"需要严格同时曝光"的场景（环视拼接 / 3D 重建 / 运动物体跨画面分析）需要按上面三条路径择一。

<details>
<summary>备选：人眼法（无法数帧时用）</summary>

⚠️ **必须用低频（1~2Hz）方波，不能用 30Hz** —— 自由运行和外触发在 30Hz 下都是 30fps，
两种结果完全一样，测了等于没测。降到 1~2Hz 才能拉开数量级。

```bash
# 终端 1：输出低频方波（每个周期 1 个下降沿 = 1 次触发）
python3 gpio_short_trigger.py --gpio 46 --wave --hz 2 --seconds 20

# 终端 2：同时看画面
ffplay -f v4l2 -input_format mjpeg -video_size 1280x720 -framerate 30 /dev/videoXXX
```

| 观察结果 | 含义 |
|----------|------|
| 画面变成约 **hz** 帧/秒的**"幻灯片"** | ✅ 真·外触发模式，四路同步成立 |
| 画面依旧**流畅 30fps** | ❌ 触发与视频流无关，只是拍照功能，须另找方案 |

</details>

### 第四步（可选·与同步路线无关）：扫出「最小高电平保持时间」

> 由于第三步已否掉"用端子做同步"，这一步**只在需要做"高频抓拍"时才有意义**。
> 一次触发耗时 = 高保持 + 低保持；若高保持必须 ≥100ms，最高只有约 10Hz。

```bash
python3 gpio_short_trigger.py --gpio 46 --probe-high
# 每档触发 3 次并停顿 3 秒，盯着厂商软件数"照片多了几张"，对得上的最小档位就是答案
```

| 最小有效高保持 | 最高触发频率（低保持 100ms） | 对 30fps 同步 |
|----------------|------------------------------|---------------|
| ≤ 5 ms | ~100 Hz 以上 | ✅ 轻松 |
| ~20 ms | ~30–40 Hz | ✅ 刚好够用 |
| ≥ 100 ms | ~10 Hz | ❌ 做不了 |

⚠️ **测试期间务必让补光灯让出这个脚**（两种做法任选）：
```bash
# 做法 A：禁用补光灯（推荐，持久）
FILL_LIGHT_GPIO=-1 python3 agent.py          # 或写进 start.sh 的环境变量
# 做法 B：测试期间不要从网页点击补光灯开关
```
否则补光灯会反复改写该脚的方向与电平，和你设的触发逻辑互相打架，造成误触发甚至持续触发，
测试结果会完全乱掉。`control.py::fill_light()` 在检测到两者同号时会直接拒绝执行并返回错误。

## 四路帧时间戳采集与同步量化（`tools/multi_cam_timeline.py`）

时间精度专题：采集每路每帧的**微秒级到达时间戳**，量化四路的"同步散布"。

```bash
cd /opt/xjpt/backpack_dashboard/backend/tools

# 四路采 10 秒（先小分辨率试 —— USB2 上四路 1080p 会超带宽）
python3 multi_cam_timeline.py \
    --cams /dev/video121,/dev/video122,/dev/video123,/dev/video124 \
    --seconds 10 --size 640x480 --csv /root/timeline.csv

# 带 GPIO 触发（每 2 秒拉低一次并记录触发时刻）
python3 multi_cam_timeline.py --cams <4路> --seconds 20 --gpio 46 --csv /root/timeline.csv
```

**原理**：每路一个常驻 ffmpeg（`-c:v copy -f image2pipe`，**不落盘不重编码**），
程序按 JPEG 的 SOI→段解析→SOS→EOI **分帧**，每切出一帧立刻记 `time.time()` → μs 级时间戳。

**⚠️ 三个必须先明确的前提**：

1. **跟那根 2pin 没关系** —— 拔掉 2P 线照样能测（图像照出），GPIO 只是**软件时间标记**
2. **不存在"GPIO 触发出图的静图节点"** —— video122 是 Metadata Capture；uvcvideo 不实现 still image；
   触发与取帧之间**没有硬件因果**（`bTriggerUsage=0`）
3. **四路接同一个 GPIO ≠ 硬件同步** —— 共用一个信号只提供软件时间参考点，**不能让四路同时曝光**

| 目标 | 能否实现 |
|------|---------|
| 每路帧间隔/帧率稳定性（μs 级时间戳） | ✅ |
| 四路帧到达时刻的相互散布（同步误差量化） | ✅ |
| "拉低 GPIO → 拿到帧"的链路延迟 | ✅（但这是**软件取帧延迟**） |
| **"触发 → 摄像头产出对应图像"的延时** | ❌ 做不到 |
| **四路同时曝光** | ❌ 需厂家 FSIN 同步或换模组 |

## 获取"触发拍照"的那张图（`tools/uvc_snapshot.py`）

四路同步暂时不需要（网页演示够用），但如果想拿到"GPIO 触发时拍的那张图"，用这个工具：

```bash
cd /opt/xjpt/backpack_dashboard/backend/tools

# ① 探测机制：列所有 video 节点能力 / USB 接口构成 / Still Image 描述符 / 相关 XU 控件
python3 uvc_snapshot.py --probe --video /dev/video121

# ② 单纯从视频流抓一帧
python3 uvc_snapshot.py --grab --video /dev/video121 -o /tmp/snap.jpg

# ③ 先发一次 GPIO 触发再抓帧（与 ② 对比）
python3 uvc_snapshot.py --grab --video /dev/video121 --gpio 46 --trigger -o /tmp/after.jpg

# ④ 软快门：常驻读流，每次触发后保存一张（最接近厂商软件行为）
python3 uvc_snapshot.py --shutter --video /dev/video121 --gpio 46 --count 3 --outdir /tmp/shots

# ⑤ 不接 GPIO，按回车手动拍
python3 uvc_snapshot.py --shutter --video /dev/video121 --outdir /tmp/shots
```

**关键结论**：既然触发信号是**我们自己发的**，触发时刻本来就知道——
在发下降沿的同时从正在读的视频流里存当前帧，就复刻了厂商软件的"拍照"，
**不依赖摄像头是否真的支持静图上传**。甚至可以用板子上已有的 **SOS/STOP 按键**当快门，
连那根 2P 线都不需要。

| | 本方案的图 | 若摄像头真支持静图上载 |
|--|-----------|----------------------|
| 来源 | 视频流里的一帧 | 摄像头独立上传的 JPEG |
| 分辨率 | 受视频流分辨率限制 | 可能高于视频流 |
| 时机精度 | ± 一个帧间隔（~35ms） | 精确对应触发瞬间 |
| 依赖 | **不依赖**摄像头特殊功能 | 依赖固件 + vendor 协议 |

机制判定与实现细节见 `DEVELOPER_GUIDE.md` 13.15。

## 聆思小板（asr）交互说明

- 服务默认不启动；点「执行测试」时 Agent 会 `systemctl start asr` 并等待 2s（与 `yd_test_v4.0.sh` 一致）。
- 两种模式（写 `/tmp/voice_assistant_cmd`）：
  - `voice`（默认）：联网下支持「小谷小谷」唤醒对话交互（对话请保持周围环境安静）；
  - `call`：播放默认音频（用作播放测试）。
- 音量：`set volume=0~100`（默认测试音量 70）。
- 前端提供「call 模式 / voice 模式 / 音量」按钮，可实时切换；状态面板显示当前模式。

## 已知待现场验证的点

- 四路推流：确认 `usbcam0-3` 软链存在、`h264_rkmpp` 可用（否则自动回退软编，占用 CPU 更高）。
- 红外 `dog-ipcam` 由已有 mediamtx 承载（8554），本项目复用；如现场该路径不是 mediamtx 提供，  
  改在 `mediamtx.yml` 用 `source: rtsp://...` 拉流代理，或设 `MEDIAMTX_MANAGED=1` 自起 mediamtx。
- SOS/STOP 按键已实测（639/128），若物理标签与功能相反，交换 `SOS_KEYCODE`/`STOP_KEYCODE` 即可。
- SN：取自 `/dev/mmcblk0p5`（hexdump 第 3 行 ASCII 列），按 16 位 `[0-9A-Z]` 校验，0 偏多会告警。
- 聆思播放测试：call 模式下 asr 拉起且模式生效即判通过；真实扬声器出声需现场人耳确认。

## 版本 3：多路识别 + 任务化下发 + 前置分辨率切换

### ① 五路可多选（恢复多路分时）
- AI 识别目标从「五路单选」改回**五路可多选**（左前/右前/右后/左后 + 机器狗前置），红外不参与。默认仍选 `dogcam`。
- **NPU 是单模型串行**，多路在 `_sched_loop()` 里分时轮询，不是真并发：每路实际帧率 ≈ `DETECT_FPS ÷ 路数`。5 路、总 6fps 时每路约 1.2fps。
- `DETECT_SINGLE_CAM=1` 可退回单选（前端改回 radio）；默认 `0` 为多选。常驻识别与任务下发共用同一组勾选。

### ② 任务化识别下发（`POST /api/detect/job`）
入参：
```json
{"job_id":"fire_check_001","task":"fire","labels":null,"conf":0.25,
 "cams":["dogcam","cam0"],"fps":6,"duration":5,"streak":3}
```
- `job_id`：自定义任务 ID，留空自动生成 `job_时间戳`；用于存证目录与文件名。
- `duration`：识别时长（秒，默认 5，上限 `DETECT_JOB_DURATION_MAX=120`）；超时未命中自动结束并通知页面「未找到」。
- `streak`：**同一路连续 N 帧命中**才算找到（默认 3，上限 10）；任一路达标即结束。多路时每路独立计数，某路某帧未命中就清零该路。
- 下发后通过 SSE 推送：`job_started` / `job_progress`（命中进度）/ `job_done`（含 found、cam、证据图路径）/ `job_cancelled` / `job_error`。
- 支持同时下发多路（在 `cams` 里选多路），但同一时刻只跑一个任务（NPU 单模型）；前端防重复提交。

**存证**（`DETECT_JOBS_DIR`，默认 `backend/detect_jobs/`）：
```
detect_jobs/<job_id>/
  <cam>_<job_id>.jpg   # 命中帧，中文框已画上（复用 cjk_font 点阵，板端零依赖）
  result.json          # job_id/found/found_cam/found_cam_title/boxes/elapsed_s/duration/streak...
```
未命中也会写 `result.json`（`found:false`），无证据图。`GET /api/detect/jobs` 列出全部结果。`POST /api/detect/job/cancel` 取消当前任务。

### ③ 前置摄像头分辨率切换
页面顶部「机器狗前置分辨率」切换条，两档：
- **1080P·60帧（默认）**：mediamtx 直拉 3588 源流（`dogcam`），不经 relay 重编码，CPU 最低。
- **720P·30帧**：relay 重编码推 `dogcam720`（1280×720/30/h264_rkmpp/2Mbps），规格与四路一致。

切换由 `POST /api/dogcam/profile {"profile":"1080p60"|"720p30"}` 完成，**只启停 relay、不重启 mediamtx**，dogcam 断几秒恢复，其他五路不受影响；档位写入 `backend/dogcam_profile`（重启保持）。前端 WHEP、AI 抓帧、板端 OSD 烧框的输入流都会随档位在 `dogcam`/`dogcam720` 间切换，逻辑路名 `dogcam` 不变。

**板端一次性准备**（720p30 档需要）：在 snap 生效配置 `/var/snap/mediamtx/common/mediamtx.yml` 新增一条 path，重启 mediamtx 一次：
```yaml
  dogcam720:
    source: publisher
```
（项目内 `mediamtx/mediamtx.yml` 已同步这份声明，仅 `MEDIAMTX_MANAGED=1` 时直接生效。）

### ④ 航点计划：计划 ID + 两类识别任务（版本 4）

业务平台的航点任务下发识别时，除「目标（场景 + 类别 + 阈值）」外还带一个**计划 ID**
（`plan_id`，取业务任务号 `taskCode`）、**航点下标**（`point_index`，0-based，与
`routeData.points` / 5009 的 `pointIndex` 一致）和**任务类型**（`check_type`）。
板端据此把证据落到计划目录：

```
detect_jobs/<plan_id>/pt<point_index>/
  <cam>_<时间戳>.jpg   # 命中帧（已画中文框，未命中则没有）
  result.json          # plan_id / point_index / check_type / found / found_count /
                       # expect_count / count_ok / count_alert / evidence
```

传入方式（几种都支持，按调用方选）：

| 接口 | 入参 | 说明 |
|---|---|---|
| `POST /api/detect/start` | `plan_id`、`point_index`、`check_type`、`expect_count`、`media_dir`、`save_frames` | 航点窗口走这个（业务客户端代调） |
| `POST /api/detect/config` | 同上 | 操作员已手动开着识别时热更新计划，不重启识别 |
| `POST /api/detect/plan` | 同上 | 只登记/换计划，不动识别参数；`POST /api/detect/plan/clear` 清掉 |
| `POST /api/detect/job` | 同上 + 原 job 参数 | 任务化下发也走计划目录存证 |

`media_dir` / `save_frames`：**媒体归档目录与固定帧数**（中台「参数配置」随
`ai_start` 下发，缺省用 `MEDIA_DATA_DIR` / `DETECT_PLAN_SAVE_FRAMES`）。命中时除了写
`detect_jobs/<plan_id>/pt<n>/`（临时存证，企业微信推完即删**单个航点**，任务终态 / 取消时
设备客户端再把这个计划的目录**整个删掉**——没命中的航点 / 数量任务的检出不推图，只靠
`ai_cleanup` 永远清不干净），还会把
**检出数最大的前 N 帧**写进媒体归档目录，供中台设备页的「同步媒体」逐张回传：

```
/media/media-data/picture/<plan_id>/pt<point_index>/
  <cam>_01_<时间戳>.jpg …     # N 帧**原始帧**（不画框），第 1 张是结论帧
  result.json                 # 同 detect_jobs 的结论 + media_dir / media_frames
```

媒体归档的图是**训练数据**（中台「媒体资源」以下载为主），所以只存相机出来的原始
帧，**不画框、不加文字**；带框的那份留在 `detect_jobs` 当告警证据（企业微信配图）。

两边目录必须指向同一份配置（中台参数配置是唯一真源）：板端按它**写**，
设备客户端按它**扫描回传**，改了中台配置两边一起变。

#### 两类任务（`check_type`）

| 类型 | 语义 | 命中 | 窗口结束仍未命中 |
|---|---|---|---|
| `exist` 检测是否存在 | 只判「有没有」 | `plan_hit`（`count_ok=null`），不判数量 | `plan_miss`（`count_alert=false`），只算「没找到」 |
| `count` 检测数量是否正确 | 还要核对数量 | `plan_hit` + `found_count` 与 `expect_count` 比对，不一致 `count_alert=true` | `plan_miss` + `count_alert=true`（0 ≠ N 也是数量不一致） |

- 不传 `check_type` 时按旧口径推断：**给了 `expect_count` 就是 `count`**，否则 `exist`
  （老调用方无需改动）。`exist` 任务即使误传 `expect_count` 也不判数量。
- `count` 任务没填 `expect_count` 时降级为存在性判定，并在日志里打一行提示
  （`[detect] 计划 … 是数量任务但没填检查目标数量`）。
- `found_count` = 本帧**通过类别 + 阈值过滤、且真正画出来的框数**（受
  `DETECT_MAX_BOXES` 截断影响），与框上文字「火焰 3处」里的 3 是同一份口径。
- 数量核对取「命中后连续 `DETECT_PLAN_COUNT_SAMPLE`（默认 3）帧里各类别的最大值」：
  单帧遮挡 / 运动模糊少画一个框不会误报「数量不一致」；存证图也用那一帧。
- 采样帧数取 `max(DETECT_PLAN_COUNT_SAMPLE, DETECT_PLAN_SAVE_FRAMES)`：要留 3 帧媒体
  归档就不能只采 1 帧。

SSE 事件（每个计划 × 航点各一次）：

```json
{"type":"plan_hit","plan_id":"TK-1112-11","point_index":0,"check_type":"count","found":true,
 "found_cam":"dogcam","found_cam_title":"机器狗前置","found_count":3,
 "counts":{"extinguisher":3},"expect_count":3,"count_ok":true,"count_alert":false,
 "dir":"backend/detect_jobs/TK-1112-11/pt0","evidence":".../dogcam_20261117_114848.jpg",
 "result_json":".../result.json","reason":"命中目标"}

{"type":"plan_miss","plan_id":"TK-1112-11","point_index":1,"check_type":"count","found":false,
 "found_count":0,"expect_count":3,"count_ok":false,"count_alert":true,
 "dir":"backend/detect_jobs/TK-1112-11/pt1","result_json":".../result.json","reason":"识别结束"}
```

收尾时机：命中采样结束即写盘；窗口结束（`/api/detect/stop`）、换计划、清除计划时
若还在采样，会**先把已有命中收尾**再返回；一次都没命中则写 `found:false` 的
`result.json` 并发 `plan_miss`，不会丢证据也不会漏记「没找到」。
注意：`plan_miss` 是不是「数量不一致」由调用方按窗口是否正常到期判断——航点被任务结束/
新航点提前打断时不该算账（业务侧 `ai_end.expired` 就是干这个的）。
`GET /api/detect/status` 的 `plan` 字段给出当前计划快照
（`plan_id / point_index / check_type / expect_count / done / sampling / dir`）。

**现场自检**（不需要 NPU / 摄像头，纯状态机 + 假落盘）：

```bash
python3 backend/tools/plan_count_selftest.py
# 校验：存在性 / 数量一致 / 数量不一致 / 未命中(两类) / 采样中途收尾不丢证 / 换计划收尾
# 全绿输出 ALL OK；存证写到临时目录，不影响真实 detect_jobs/
```

### ⑤ 航点灯光（补光灯 / LED）

计划上可以配「自动开灯（补光灯 / LED）+ 仅夜间」（`lightFill` / `lightLed` /
`lightNightOnly`）：**以计划为准**时客户端在任务一开始就调板端（`pointIndex=-1`，
全程不跟航点动作），终态 / 取消才关。计划没配时走老的航点动作（「到点开灯」），
到点调、离开关。

| 接口 | 入参 | 说明 |
|---|---|---|
| `POST /api/lights` | `fill`、`led`、`nightOnly`、`taskCode`、`pointIndex`、可选 `nightStart`/`nightEnd` | 开灯（计划灯 `pointIndex=-1`，航点模式是真实下标）；`nightOnly` 且不在夜间窗内 → `skipped=daytime`，**什么都不做**；`nightStart`/`nightEnd`（计划里自定义的时段，`HH:MM`）不传就用板端默认 |
| `POST /api/lights/off` | `reason` | 离开航点（航点模式）/ 任务终态 / 取消 / 看门狗 |
| `GET /api/lights` | — | 状态：`fill`/`led`/`managed`/`night`/`night_window`（当前生效的窗）/`plan_night_window`（计划自定义时段，`null`=板端默认）/`fill_available` |

夜间窗在板端判：计划里带了自己的时段就用它，否则用 `LIGHT_NIGHT_START` /
`LIGHT_NIGHT_END`（默认 18:00-06:00，允许跨零点，**起止相同 = 关闭夜间判定**）。计划灯有看门狗（`LIGHT_MAX_ON_SECS`，默认 900s）；
手动按钮（`/api/filllight`、`/api/led/blink`）开的灯不吃看门狗。
LED 只有厂商的 `test_rgbrun`（循环闪烁），所以「开」= 拉起进程、「关」= 按进程组杀掉，
手动与计划共用同一个进程句柄。

**现场自检**（不需要板子）：`python3 backend/tools/lights_selftest.py`
（时间窗边界 / 白天跳过 / GPIO 写值 / LED 进程 / 看门狗 / GPIO 被禁用时 LED 照开）。

### ⑥ 语音播报（航点「语音播报」/ 航段「语音播报」）

巡检到点 / 进入航段时让板子说一句话。命令行客户端在 5009 状态机上起停，
板端只负责把话交给聆思 TTS（与 Dashboard 的「开始播报」按钮同一条 FIFO 通道）：
`call 0 <内容>` 单次、`call 1 <内容>` 循环、`call stop` 回到 voice。

| 接口 | 入参 | 说明 |
|---|---|---|
| `POST /api/speak` | `text`、`loop`、`seconds`、`planId`、`pointIndex`、`scope`（`point`/`leg`） | 开始播报；同一 `(planId, scope, pointIndex)` + 同文本重复请求幂等；内容会先净化（去空格、丢非法字符、截断） |
| `POST /api/speak/stop` | `reason` | 停止并回到 voice（离开航点 / 航段 / 终态 / 取消 / 看门狗）；没有在播时什么都不写 |
| `GET /api/speak/status` | — | 当前内容 / 是否循环 / 已播秒数 / 上限（`SPEAK_MAX_*`） |

**TTS 在云端**：`POST /api/speak` 会先 `ensure_asr()`，没联网 / asr 起不来就回
`ok=false`，客户端只记日志、巡检照跑。内容限制（板端固件）：不能有空格，
只认中文汉字 + 中文标点 + 数字，中文 ≤30 字、总长 ≤40；这份白名单同时是
FIFO 命令注入的护栏。循环播报有看门狗（`SPEAK_MAX_SECS`，默认 600s），
单次播报说完会自动 `call stop` 回 voice（别让板子一直占着 call 模式）。

**现场自检**（不需要板子 / 聆思 / 网络）：`python3 backend/tools/speak_selftest.py`
（内容净化 / 幂等 / 循环看门狗 / 单次收尾 / 换播报先停旧的 / stop 幂等）。

### 部署到板端需同步的新增/改动文件
`backend/config.py`、`relay.py`、`detect.py`、`osd.py`、`agent.py`、**新增 `evidence.py` / `speak.py`**、`frontend/index.html`、`static/app.js`、`static/style.css`、`mediamtx/mediamtx.yml`；并在 snap 配置加 `dogcam720` path。重启 agent。
