# 航点拍照（路线动作「可见光拍照」）：配置、链路与排查

「跑到这个点拍一张，回来能在平台上看」。四处代码：

| 在哪 | 文件 | 职责 |
|---|---|---|
| 平台编辑器 | `business/web/src/views/routes/index.vue`（`isPhotoAction` 那一段） | 配抓拍源 + 张数 |
| 设备客户端 | `business/client/src/photo.rs` | 跟 5009 到点抓拍、任务收尾回传 |
| 板端 Agent | `business/backpack_dashboard/backend/photo.py` | ffmpeg 从本机 mediamtx 抓 JPEG、落盘、清单、删除 |
| 平台服务端 | `business/server/src/photos.rs` | 入库（`task_photos`）、任务详情列表、原图、留存清理 |

---

## 1. 照片从哪来（为什么不是本体的相机）

本体侧的「任务里的 `1` = 拍照」走的是**上装** `/capture_image`：

- 上装拍照服务没起时直接 `14005`（`zhpro/route.rs::capture_hint` 已经把这个错误码
  翻译成可读提示），**照片就没有了**；
- 本体也没实现 5010/5011 巡检记录查询（协议里写着「暂不对外使用」），
  所以 `phase[].image_path` 拿不到，平台没法把图拉回来。

所以这里的照片来自**背包相机**（本来就在跑实时视频 / AI 抽帧，同一路 mediamtx）：

| `photoCamera` | 是什么 |
|---|---|
| `dogcam`（默认） | 机器狗前置可见光（按当前分辨率档位自动选 `dogcam` / `dogcam720`） |
| `cam0`~`cam3` | 背包四路 USB 相机（左前 / 右前 / 右后 / 左后） |
| `dog-ipcam` | 红外相机 |

本体侧的 `tasks=[1]` 行为**保持不变**（它拍它的，我们拍我们的），两套互不影响。

## 2. 配置

| 字段 | 位置 | 默认 | 说明 |
|---|---|---|---|
| 动作类型 | 航点动作 | — | 「可见光拍照」（`可见光拍照` / `抓拍` 都认，**录像不算**） |
| `photoCamera` | 动作 | `dogcam` | 抓拍源，见上表 |
| `photoFrames` | 动作 | `1` | 抓几张（1~5），多帧按板端 `PHOTO_FPS`（2fps）拉开，避开运动模糊 |

同一条路线里，只有配了「可见光拍照」动作的航点才会拍；没有动作的点不会产生任何请求。

## 3. 链路

```text
routeData.points[i].actions[] = 可见光拍照（photoCamera / photoFrames）
  → 设备客户端（5009 到点 `patrol.point_arrived` / `patrol.waiting`，`photo.rs`）
      POST 板端 /api/photo {planId, pointIndex, pointName, camera, frames}
    → 板端 Agent（`photo.py`）
        ffmpeg -rtsp_transport tcp -i rtsp://127.0.0.1:8554/<stream> -frames:v N -q:v 2
        → PHOTO_DIR/<planId>/pt<n>/<时间戳>-<相机>[-序号].jpg + points.json（航点下标→名字）
  → 任务终态 / 取消 / 客户端启动（`sync_pending`）
      GET 板端 /api/photo/list
      → POST 平台 /api/photos/ingest（multipart：file + meta，sha256 去重）
        → 落盘 <photo_dir>/<设备>/<任务>/pt<n>/…jpg + 一条 task_photos
          → 上传成功才 POST 板端 /api/photo/delete（失败的文件留在板端，下一轮重试）
            → 任务详情「任务照片」缩略图 / 大图 / 下载 / 删除
```

要点：

- **到点同步等**：抓拍是 `point_arrived` 里 `await` 的（不像开灯那样 `tokio::spawn`）——
  狗在点上停留只有几秒，晚一拍拍到的是下一段路。单次抓拍实测 0.5~2s，
  超时 12s 兜底，不会把 5009 的处理链路拖死。
- **幂等**：5009 每 500ms 一帧，同一个航点只拍一次（`handled_points` 集合；
  返航 / 重发时同一个点再报也不会重拍）。
- **多帧**：`-frames:v N -vf fps=2`，3 张 ≈ 1.5s 内的画面（连着抓 3 帧是同一张图，
  没意义）。
- **失败不影响巡检**：流没在推 / ffmpeg 缺失 / 磁盘不足都只回 `ok=false + error`，
  客户端只打日志；老版本 Agent（404）记一次 `unsupported`，本轮不再试。
- **回传时机**：任务终态 / 取消 / 客户端启动。照片几百 KB，但一次巡检几十张，
  不在到点就传（4G 上会跟任务抢带宽）。
- **留存**：平台 `photo_keep_days`（默认 90 天，`0` = 不清理）由
  `schedule::sweep` 每分钟顺手清；板端 `PHOTO_RETENTION_HOURS`（默认 72 小时）
  兜「没传走的」。
- **为什么不进中台「媒体资源」**：那边是 AI 存证的**训练图**（sha256 去重、
  批量打 zip 导出），巡检照片是任务执行结果，看的人、删的时机都不一样（与录像同一理由）。

## 4. 代码地图

| 位置 | 内容 |
|---|---|
| `web/src/utils/constants.ts` | `PHOTO_CAMERAS`（抓拍源清单）、`PHOTO_MAX_FRAMES` |
| `web/src/views/routes/index.vue` | `isPhotoAction`、`actionPhotoLabel`、对话框「抓拍相机 / 抓拍张数」 |
| `web/src/api/types.ts` | `RoutePointAction.photoCamera` / `photoFrames`、`TaskPhoto` |
| `web/src/api/ops.ts` | `photoApi.list / blob / remove` |
| `web/src/views/tasks/index.vue` | 「任务照片」网格（按需加载缩略图）+ 大图弹窗（下载 / 删除） |
| `client/src/photo.rs` | `parse_photo_plan`、`Photographer::{start_task, point_arrived, finish_task, sync_pending}` |
| `client/src/pump.rs` | 5009 到点 / 终态两处调用（与 AI、灯光、录像、播报同一处收口） |
| `client/src/handlers/{task,control}.rs` | 下发时 `start_task`、本地收尾 / 取消时 `finish_task` |
| `client/src/config.rs` | `--no-photo` / `ROBODOG_PHOTO_ENABLED=0` |
| `server/src/photos.rs` | `POST /api/photos/ingest`、`GET /api/tasks/{id}/photos`、`GET /api/photos/{id}/file[?download=1]`、`DELETE /api/photos/{id}`、`sweep_expired` |
| `server/migrations/035_task_photos.sql` | `task_photos` 表（`sha256` 唯一 = 重传去重） |
| `backpack_dashboard/backend/photo.py` | `capture` / `list_files` / `delete` / `status` / `sweep` |
| `deploy/backpack-sim/mock_agent.py` | 同四个接口的模拟实现（`/api/sim/state` 里的 `photo`） |

## 5. 现场排查

| 现象 | 先看什么 |
|---|---|
| 任务详情没有「任务照片」 | 路线里这个点配了「可见光拍照」吗？`journalctl -u backpack-agent` 里搜 `[photo]`，有没有「航点 N/相机 抓拍 X 张」 |
| 抓拍失败 `连接被拒 / 404` | 那一路流没在推（`GET /api/cameras`、mediamtx 里有没有这个 path）；`/tmp/photo_ffmpeg.log` 有完整 ffmpeg 输出 |
| 照片是黑的 | 同上：先看流本身（实时视频里是不是黑的） |
| 照片没出现在平台 | 客户端日志「开始回传航点照片 / 照片上传失败」；平台 `/api/photos/ingest` 会回具体原因（任务不存在 / 设备对不上 / 不是 JPEG） |
| 平台图表 404 | 文件被 `photo_keep_days` 清掉了，或有人点过「删除」（行和文件是一起删的） |
| 板子磁盘涨 | 上传成功后客户端会删板端文件；一直涨说明上传在失败（看客户端日志），或 `PHOTO_RETENTION_HOURS=0` 关掉了兜底清理 |

手工验接口（板端同机）：

```bash
curl -s 127.0.0.1:8000/api/photo/status
curl -s -XPOST 127.0.0.1:8000/api/photo -H 'Content-Type: application/json' \
  -d '{"planId":"TK-1","pointIndex":0,"pointName":"P1","camera":"dogcam","frames":1}'
curl -s 127.0.0.1:8000/api/photo/list
curl -s -XPOST 127.0.0.1:8000/api/photo/delete \
  -d '{"names":["TK-1/pt0/<文件名>.jpg"]}'
```

## 6. 回归

```bash
# 板端：抓拍 / 多帧 / 清单 / 删除 / 路径白名单 / 超期清盘（不需要相机、ffmpeg）
python3 business/backpack_dashboard/backend/tools/photo_selftest.py

# 客户端：路线解析 / 到点抓拍 / 终态回传 / 老 Agent 退让 / 5009 时序
cd business/client && cargo test photo:: && cargo test waypoint_photo

# 平台：入库 / 去重 / 列表 / 原图 / 删除 / 超期清理
cd business/server && cargo test --test photos

# 编辑器
cd business/web && pnpm build
```

用 `deploy/backpack-sim/` 在本机复刻板端时，`python3 run.py smoke` 的第 [8] 项会把
`/api/photo`（抓一张 → list → JPEG 头 → delete → 未知相机被拒）跑一遍。
