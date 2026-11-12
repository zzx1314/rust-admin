import type { RouteData, RouteLeg, RoutePoint } from "@/api/types";

export const STATUS_TONES: Record<string, string> = {
  运行中: "ok",
  正常: "ok",
  在线待命: "ok",
  已完成: "ok",
  已处理: "ok",
  执行任务: "info",
  执行中: "info",
  已暂停: "warn",
  充电中: "warn",
  处理中: "warn",
  部分维护: "warn",
  警告: "warn",
  维护中: "warn",
  重要: "warn",
  异常: "danger",
  故障: "danger",
  未处理: "danger",
  严重: "danger",
  告警: "danger",
  待执行: "neutral",
  待绑定: "warn",
  离线: "neutral",
  提示: "neutral",
  常规: "neutral",
  草稿: "neutral",
  已发布: "ok"
};

export function statusTone(status?: string | null): string {
  return (status && STATUS_TONES[status]) || "neutral";
}

export function batteryTone(value: number): string {
  if (value >= 80) return "ok";
  if (value >= 40) return "";
  if (value >= 20) return "warn";
  return "danger";
}

/**
 * 定位状态（智航 5009 `loc_state.status`）→ 展示文案与色标。
 *
 * 100 已收敛，可以直接接任务；120/121 正在载图/重定位（等待即可）；
 * 122/203/210 不在地图上，需要人工接管或下发「重新定位」。
 */
export function locStatusMeta(status?: number | null): { label: string; tone: string } {
  switch (status ?? 255) {
    case 100:
      return { label: "定位正常", tone: "ok" };
    case 1:
      return { label: "就绪", tone: "info" };
    case 0:
      return { label: "未载图", tone: "neutral" };
    case 120:
      return { label: "载图中", tone: "warn" };
    case 121:
      return { label: "重定位中", tone: "warn" };
    case 122:
      return { label: "定位丢失", tone: "danger" };
    case 203:
      return { label: "定位异常", tone: "danger" };
    case 210:
      return { label: "定位失败", tone: "danger" };
    default:
      return { label: "未上报", tone: "neutral" };
  }
}

/**
 * 控制源（智航 1315 `ctrl_source`）→ 展示文案与色标。
 *
 * 0 未知 / 1 APP / 2 SDK / 3 其他（硬件遥控器走这条）。
 * 平台接管后若仍是 3，说明遥控器或别的 SDK 压着控制权，摇杆不会生效。
 */
export function ctrlSourceMeta(source?: number | null): { label: string; tone: string } {
  switch (source ?? 0) {
    case 1:
      return { label: "APP 控制", tone: "info" };
    case 2:
      return { label: "SDK 控制", tone: "ok" };
    case 3:
      return { label: "其他 / 遥控器", tone: "danger" };
    default:
      return { label: "未知", tone: "neutral" };
  }
}

/** 运动状态（1315 `motion_status`）→ 中文；卧倒时摇杆前后左右都不会动 */
const MOTION_LABELS: Record<string, string> = {
  unknown: "未知",
  stand_up: "站立中",
  balance_stand: "平衡站立",
  walk: "行走",
  crawl: "匍匐",
  crawl_walk: "匍匐行走",
  lie_down: "卧倒",
  locked: "锁定",
  climb: "爬高台",
  stair: "登阶",
  slim: "侧身",
  gait: "步态",
  dsb: "DSB",
  pos_control: "位控姿态",
  sk_walk: "同膝行走"
};

export function motionStatusLabel(status?: string | null): string {
  if (!status) return "未上报";
  return MOTION_LABELS[status] ?? status;
}

/** 卧倒/锁定状态下摇杆无效，需要先站立或解除锁定 */
export function canGait(status?: string | null): boolean {
  return status !== "lie_down" && status !== "locked";
}

/** 定位是否健康（可以接任务）；无法起步时页面给出重定位入口 */
export function isLocReady(status?: number | null): boolean {
  return status === 100;
}

export function timeOnly(value?: string | null): string {
  if (!value) return "—";
  return value.length >= 16 ? value.slice(11, 16) : value;
}

export function dateTime(value?: string | null): string {
  return value || "—";
}

export function formatDuration(seconds: number): string {
  const minutes = Math.ceil(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
}

export function parseRouteData(raw?: string | null): RouteData {
  if (!raw) {
    return {
      schemaVersion: 3,
      coordFrame: "map",
      loop: "闭环",
      points: [],
      legs: [],
      returnLeg: null
    };
  }
  try {
    const parsed = JSON.parse(raw) as RouteData;
    return {
      schemaVersion: parsed.schemaVersion ?? 2,
      coordFrame: parsed.coordFrame,
      loop: parsed.loop ?? "闭环",
      points: Array.isArray(parsed.points) ? parsed.points : [],
      legs: Array.isArray(parsed.legs) ? parsed.legs : [],
      returnLeg: parsed.returnLeg ?? null
    };
  } catch {
    return {
      schemaVersion: 3,
      coordFrame: "map",
      loop: "闭环",
      points: [],
      legs: [],
      returnLeg: null
    };
  }
}

export function cloneRouteData(data: RouteData): RouteData {
  return JSON.parse(JSON.stringify(data)) as RouteData;
}

export interface RouteMetrics {
  pointCount: number;
  distanceM: number;
  durationS: number;
  actionCount: number;
  batteryPct: number;
}

export function computeRouteMetrics(data: RouteData, speed: number): RouteMetrics {
  // 航点已是地图坐标系下的米（见 utils/coords.ts），直接平面求和；
  // 旧口径的「像素 × 1.2」与地图分辨率无关，会让里程跟着画布缩放走。
  let meters = 0;
  for (let index = 0; index < data.points.length - 1; index += 1) {
    const current = data.points[index];
    const next = data.points[index + 1];
    meters += Math.hypot(next.x - current.x, next.y - current.y);
  }
  if (data.loop === "闭环" && data.points.length > 2) {
    const first = data.points[0];
    const last = data.points[data.points.length - 1];
    meters += Math.hypot(last.x - first.x, last.y - first.y);
  }
  const dwell = data.points.reduce((sum, point) => sum + Number(point.dwell || 0), 0);
  let actions = data.points.reduce(
    (sum, point) => sum + (point.actions?.length || 0),
    0
  );
  actions += data.legs.reduce((sum, leg) => sum + (leg.actions?.length || 0), 0);
  actions += data.returnLeg?.actions?.length || 0;
  const moveSeconds = meters / (speed > 0.1 ? speed : 1.2);
  return {
    pointCount: data.points.length,
    distanceM: Math.round(meters * 10) / 10,
    durationS: Math.ceil(moveSeconds + dwell + actions * 3),
    actionCount: actions,
    batteryPct: Math.min(100, Math.round((meters / 1000) * 8 + actions * 0.4))
  };
}

export function newRouteData(loop = "闭环"): RouteData {
  return {
    schemaVersion: 3,
    coordFrame: "map",
    loop,
    points: [],
    legs: [],
    returnLeg:
      loop === "闭环"
        ? { mode: "沿道路", speed: 1.5, avoid: "标准", actions: [] }
        : null
  };
}

export function newPoint(index: number, x: number, y: number): RoutePoint {
  return {
    name: `新航点 ${index}`,
    x,
    y,
    dwell: 5,
    radius: 0.4,
    must: false,
    heading: 0,
    onFail: "跳过继续",
    actions: [
      {
        type: "可见光拍照",
        param: "1080P / 广角 / 单帧",
        trigger: "到达即执行",
        threshold: "—",
        retry: 1
      }
    ]
  };
}

export function newLeg(speed: number): RouteLeg {
  return {
    mode: "沿道路",
    speed,
    avoid: "标准",
    actions: [{ type: "全程录像", param: "1080P / 25fps / 连续" }]
  };
}

export function downloadCsv(filename: string, rows: Array<Array<unknown>>): void {
  const content = rows
    .map(row =>
      row
        .map(value => `"${String(value ?? "").replaceAll('"', '""')}"`)
        .join(",")
    )
    .join("\n");
  const blob = new Blob([`\uFEFF${content}`], { type: "text/csv;charset=utf-8" });
  const link = document.createElement("a");
  link.href = URL.createObjectURL(blob);
  link.download = filename;
  link.click();
  URL.revokeObjectURL(link.href);
}

export function initials(name?: string | null): string {
  if (!name) return "管";
  return name.slice(0, 1);
}
