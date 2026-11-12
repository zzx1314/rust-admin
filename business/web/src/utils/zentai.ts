/**
 * 跨服务跳转：业务控制台 → 中台「地图管理」。
 *
 * 业务库 `map_versions.source_id` 就是中台 `dog_map.id`（机器人建图回传 / 中台导入
 * 都会写入），这是两个库之间唯一可靠的关联键。但业务地图编号 `ROS-MAP-<id>@版本`
 * 里的数字是**首次入库时**的中台 id：地图在中台被删掉后重传，业务编号保持不变
 * （路线/任务都引用它），中台侧却换成了新的 `dog_map.id` —— 所以页面上要直接给出
 * 「中台地图编号 + 地图名称 + 设备编号」，别指望从业务编号反推。
 *
 * 中台控制台地址：外网部署与业务平台同机（中台 :3005 / 业务平台 :3100），
 * 默认按当前 host 推 :3005；本地开发中台在 :3000，用 VITE_ZENTAI_CONSOLE_URL 覆盖。
 */
const CONSOLE_BASE = (() => {
  const configured = (
    import.meta.env.VITE_ZENTAI_CONSOLE_URL as string | undefined
  )?.trim();
  if (configured) return configured.replace(/\/+$/, "");
  if (typeof window === "undefined") return "";
  return `${window.location.protocol}//${window.location.hostname}:3005`;
})();

/** 中台「地图管理」页面（`p_sys_menu` 菜单 path_url） */
const ZENTAI_MAP_PAGE = "/dogPark/map/index";

/** 中台地图编号：`map_versions.source_id` → `dog_map.id` */
export function zentaiMapId(sourceId: string | null | undefined) {
  const value = (sourceId ?? "").trim();
  return /^\d+$/.test(value) ? value : null;
}

/**
 * 中台「地图管理」的直达链接（按地图名称 / 设备编号预填筛选）。
 * 返回 `null` 表示拿不到足够信息（例如本地演示基线没有中台记录）。
 */
export function zentaiMapConsoleLink(
  map:
    | {
        mapName?: string | null;
        deviceCode?: string | null;
        fileName?: string | null;
      }
    | null
    | undefined
): string | null {
  if (!CONSOLE_BASE || !map) return null;
  const params = new URLSearchParams();
  const mapName = (map.mapName ?? "").trim();
  const deviceCode = (map.deviceCode ?? "").trim();
  const fileName = (map.fileName ?? "").trim();
  if (mapName) params.set("mapName", mapName);
  else if (fileName) params.set("fileName", fileName);
  if (deviceCode) params.set("deviceCode", deviceCode);
  if (!params.size) return null;
  return `${CONSOLE_BASE}${ZENTAI_MAP_PAGE}?${params.toString()}`;
}
