/**
 * 业务库 ⇄ 中台地图的编号对照。
 *
 * 业务库 `map_versions.source_id` 就是中台 `dog_map.id`（机器人建图回传 / 中台导入
 * 都会写入），这是两个库之间唯一可靠的关联键。但业务地图编号 `ROS-MAP-<id>@版本`
 * 里的数字是**首次入库时**的中台 id：地图在中台被删掉后重传，业务编号保持不变
 * （路线/任务都引用它），中台侧却换成了新的 `dog_map.id` —— 所以页面上要直接给出
 * 「中台地图编号 + 地图名称 + 设备编号」，别指望从业务编号反推。
 */

/** 中台地图编号：`map_versions.source_id` → `dog_map.id` */
export function zentaiMapId(sourceId: string | null | undefined) {
  const value = (sourceId ?? "").trim();
  return /^\d+$/.test(value) ? value : null;
}
