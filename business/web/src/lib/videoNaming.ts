/**
 * 背包视频的命名规则：**与服务端 `video.rs` 一一对应，别在页面里另拼一份**。
 *
 * | 场景 | 房间名（WS 心跳 / 看门狗） | mediamtx path |
 * | --- | --- | --- |
 * | 默认设备（没识别出编号） | `bp:<stream>` | `bp_<stream>` |
 * | 多设备 | `bp:<device>:<stream>` | `bp_<device>_<stream>` |
 *
 * 浏览器拉流走**固定的**平台反代模板：`/api/backpack/whep/<stream>/whep?device=<id>`
 * —— 设备编号是查询参数（和 `token` 一样），不为每台背包多拼一段路径。
 *
 * 设备编号是平台设备台账里的编号（板端 relay 的 `RELAY_DEVICE_ID`、
 * 设备客户端的 `ROBODOG_DEVICE_ID`）。空串 / `null` = 默认设备，沿用旧命名 ——
 * 现场已部署的单机因此完全不受影响。
 */

/** 规范化后的设备编号；空 = 默认设备 */
function deviceId(device: string | null | undefined): string {
  return (device ?? "").trim();
}

/** 平台上的推流房间名（`bp:<stream>` / `bp:<device>:<stream>`） */
export function relayRoom(device: string | null | undefined, stream: string): string {
  const id = deviceId(device);
  return id ? `bp:${id}:${stream}` : `bp:${stream}`;
}

/**
 * 平台 WHEP 反代的**相对**路径：模板固定，设备编号走查询串
 * `/backpack/whep/<stream>/whep[?device=<id>]`。
 *
 * 只返回相对路径：调用方自己拼 `apiBaseUrl()`（「现场入口」模式下页面挂在板端
 * 静态服务上，相对路径会打到 404）与 `token`（用 `&` 接在后面）。
 */
export function whepPath(device: string | null | undefined, stream: string): string {
  const id = deviceId(device);
  const path = `/backpack/whep/${encodeURIComponent(stream)}/whep`;
  return id ? `${path}?device=${encodeURIComponent(id)}` : path;
}
