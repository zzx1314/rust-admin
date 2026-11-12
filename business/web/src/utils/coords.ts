/**
 * 路线坐标：画布像素 ↔ 地图坐标系（米）。
 *
 * 智航 Pro 导航任务（协议 §2.5.1.4 `5000 kind=2`）的 `goals[].position` 是
 * **地图坐标系下的米**，地图由 `routes.map_version_id` 冻结；而编辑器是在
 * 800×460 的 SVG 画布上点击/拖拽的。两套坐标必须显式换算——不换算就会把
 * `x=335` 当成 335 米下发给本体，整条航线落在图外。
 *
 * 存储约定：`RouteData.points[].x/y` 一律是地图米，并带 `coordFrame: "map"`。
 * 公式与 `server/src/coords.rs` 必须保持一致：
 *
 * ```text
 * scale = min(800 / width, 460 / height)      // 底图等比铺进画布
 * drawW = width * scale, drawH = height * scale
 * rectX = (800 - drawW) / 2, rectY = (460 - drawH) / 2
 *
 * 画布 → 地图：fx = (cx - rectX) / drawW, fy = (cy - rectY) / drawH
 *              x  = originX + fx * width  * resolution
 *              y  = originY + (1 - fy) * height * resolution   // 图像 y 向下、地图 y 向上
 *
 * 地图 → 画布：fx = (x - originX) / (width * resolution)
 *              fy = 1 - (y - originY) / (height * resolution)
 *              cx = rectX + fx * drawW, cy = rectY + fy * drawH
 * ```
 *
 * 没有真实 ROS 地图包（演示基线 / 未绑定基线）时退回「示意坐标系」：
 * 0.1 m/px、原点 (0,0)、画布自身 800×460，公式照样可逆，但这类路线不能下发。
 */
import type { MapVersion } from "@/api/types";

/** 编辑画布 viewBox 宽度（px） */
export const CANVAS_WIDTH = 800;
/** 编辑画布 viewBox 高度（px） */
export const CANVAS_HEIGHT = 460;
/** 示意坐标系的虚拟比例尺（m/px） */
export const SCHEMATIC_RESOLUTION = 0.1;
/** 路线数据里的坐标语义标记 */
export const COORD_FRAME_MAP = "map";

export interface MapFrame {
  /** 米 / 像素 */
  resolution: number;
  originX: number;
  originY: number;
  /** 栅格宽高（px） */
  width: number;
  height: number;
  /** 没有真实地图包时的示意坐标系 */
  schematic: boolean;
}

/** 示意坐标系：画布自身就是地图，0.1 m/px，原点在左下角 */
export const SCHEMATIC_FRAME: MapFrame = {
  resolution: SCHEMATIC_RESOLUTION,
  originX: 0,
  originY: 0,
  width: CANVAS_WIDTH,
  height: CANVAS_HEIGHT,
  schematic: true
};

/** 解析 `"0.05 m/px"` / `"0.05"` 形式的分辨率 */
export function parseResolution(text?: string | null): number | null {
  if (!text) return null;
  const value = Number.parseFloat(text.trim().split(/\s+/)[0] ?? "");
  return Number.isFinite(value) && value > 0 ? value : null;
}

/** 解析 `"-14.85, -13.45, 0.0"` 形式的原点（只取 x/y，米） */
export function parseOrigin(text?: string | null): [number, number] | null {
  if (!text) return null;
  const parts = text.split(",").map(part => Number.parseFloat(part.trim()));
  if (parts.length < 2) return null;
  const [x, y] = parts;
  return Number.isFinite(x) && Number.isFinite(y) ? [x, y] : null;
}

/**
 * 地图版本 → 坐标几何。
 *
 * 只有「有 ROS 地图包 + 分辨率 + 米制原点 + 栅格尺寸」的版本才算真实地图；
 * 演示基线的 origin 是经纬度锚点、也没有 file_path，一律按示意坐标系处理。
 */
export function frameOf(map?: MapVersion | null): MapFrame {
  if (!map) return SCHEMATIC_FRAME;
  const resolution = parseResolution(map.resolution);
  const origin = parseOrigin(map.origin);
  const width = map.mapWidth ?? 0;
  const height = map.mapHeight ?? 0;
  const hasPackage = Boolean(map.filePath && map.filePath.trim());
  if (!hasPackage || !resolution || !origin || width <= 0 || height <= 0) {
    return SCHEMATIC_FRAME;
  }
  return {
    resolution,
    originX: origin[0],
    originY: origin[1],
    width,
    height,
    schematic: false
  };
}

function layoutOf(frame: MapFrame) {
  const scale = Math.min(CANVAS_WIDTH / frame.width, CANVAS_HEIGHT / frame.height);
  const drawW = frame.width * scale;
  const drawH = frame.height * scale;
  return {
    scale,
    drawW,
    drawH,
    rectX: (CANVAS_WIDTH - drawW) / 2,
    rectY: (CANVAS_HEIGHT - drawH) / 2
  };
}

/** 画布像素 → 地图米 */
export function canvasToMap(frame: MapFrame, canvasX: number, canvasY: number) {
  const { drawW, drawH, rectX, rectY } = layoutOf(frame);
  const fx = (canvasX - rectX) / drawW;
  const fy = (canvasY - rectY) / drawH;
  return {
    x: frame.originX + fx * frame.width * frame.resolution,
    y: frame.originY + (1 - fy) * frame.height * frame.resolution
  };
}

/** 地图米 → 画布像素 */
export function mapToCanvas(frame: MapFrame, x: number, y: number) {
  const { drawW, drawH, rectX, rectY } = layoutOf(frame);
  const fx = (x - frame.originX) / (frame.width * frame.resolution);
  const fy = 1 - (y - frame.originY) / (frame.height * frame.resolution);
  return { x: rectX + fx * drawW, y: rectY + fy * drawH };
}

/** 地图覆盖范围（米） */
export function frameExtents(frame: MapFrame) {
  return {
    minX: frame.originX,
    minY: frame.originY,
    maxX: frame.originX + frame.width * frame.resolution,
    maxY: frame.originY + frame.height * frame.resolution
  };
}

/** 点是否在地图覆盖范围内（留 1 个像素容差） */
export function frameContains(frame: MapFrame, x: number, y: number): boolean {
  const { minX, minY, maxX, maxY } = frameExtents(frame);
  const margin = frame.resolution;
  return x >= minX - margin && x <= maxX + margin && y >= minY - margin && y <= maxY + margin;
}

/** 路线数据是否已是地图米制 */
export function isMapFrameData(data?: { coordFrame?: string } | null): boolean {
  return data?.coordFrame === COORD_FRAME_MAP;
}

/** 米制坐标展示（统一 1 位小数，km 里程换算用） */
export function formatMeters(value: number): string {
  return value.toFixed(1);
}
