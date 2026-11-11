import type { RouteData, RouteLeg, RoutePoint } from "@/api/types";

export const STATUS_TONES: Record<string, string> = {
  运行中: "ok",
  正常: "ok",
  在线待命: "ok",
  已完成: "ok",
  已处理: "ok",
  执行任务: "info",
  执行中: "info",
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
    return { schemaVersion: 2, loop: "闭环", points: [], legs: [], returnLeg: null };
  }
  try {
    const parsed = JSON.parse(raw) as RouteData;
    return {
      schemaVersion: parsed.schemaVersion ?? 2,
      loop: parsed.loop ?? "闭环",
      points: Array.isArray(parsed.points) ? parsed.points : [],
      legs: Array.isArray(parsed.legs) ? parsed.legs : [],
      returnLeg: parsed.returnLeg ?? null
    };
  } catch {
    return { schemaVersion: 2, loop: "闭环", points: [], legs: [], returnLeg: null };
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
  let pixels = 0;
  for (let index = 0; index < data.points.length - 1; index += 1) {
    const current = data.points[index];
    const next = data.points[index + 1];
    pixels += Math.hypot(next.x - current.x, next.y - current.y);
  }
  if (data.loop === "闭环" && data.points.length > 2) {
    const first = data.points[0];
    const last = data.points[data.points.length - 1];
    pixels += Math.hypot(last.x - first.x, last.y - first.y);
  }
  const meters = pixels * 1.2;
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
    schemaVersion: 2,
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
