/**
 * 流程设计 → 地图标记的纯逻辑（不依赖 DOM / 接口，便于单独验证）
 *
 * 输入：流程画布节点 / 连线快照 + 任务节点所引用航线的航点，
 * 输出：航段折线、航点、动作标记与图例行。
 */

export type FlowNodeData = Record<string, any>;
export type FlowNode = { id: string; type?: string; data?: FlowNodeData };
export type FlowEdge = { source?: string; target?: string };
/** 世界坐标（x 东西向，y 南北向），与航线 routeData 一致 */
export type FlowPoint = [number, number];
/** 航点：wpId = routeData 数组下标 + 1（与后端一致） */
export type RoutePoint = { wpId: number; x: number; y: number };
export type RouteMap = Map<number, RoutePoint[]>;

/** 地图上的动作标记 */
export type ActionMarker = {
  point: FlowPoint;
  /** 动作类型，如 PHOTO */
  type: string;
  /** 动作中文名，如「拍照动作」 */
  action: string;
  /** 所属任务节点，如「任务1」 */
  task: string;
  /** 参数描述 / 错误原因 */
  desc: string;
  isError: boolean;
};

/** 任务节点对应的航段 */
export type TaskSegment = {
  points: FlowPoint[];
  ids: number[];
  label: string;
};

/** 图例行（地图右侧动作列表） */
export type MarkerRow = {
  task: string;
  action: string;
  type: string;
  desc: string;
  isError: boolean;
  /** 是否已定位到地图（自定义航线且无坐标时为 false） */
  located: boolean;
};

export type MarkerPlan = {
  markers: ActionMarker[];
  rows: MarkerRow[];
  segments: TaskSegment[];
  /** 整条航线（灰色背景线），航段只是其中一段时绘制 */
  routeLines: FlowPoint[][];
  taskCount: number;
};

/** 动作类型 → 中文名（与 ActionNode 的动作下拉保持一致） */
export const ACTION_LABELS: Record<string, string> = {
  TAKEOFF: "起飞动作",
  HOVER: "悬停动作",
  GOTO: "GOTO动作",
  PHOTO: "拍照动作",
  LAND: "降落动作",
  SERVICE_START_YOLO: "启动YOLO",
  SERVICE_STOP_YOLO: "停止YOLO",
  SERVICE_START_RTSP: "启动RTSP",
  SERVICE_STOP_RTSP: "停止RTSP"
};

/** 错误处理节点使用统一的动作名 */
export const ERROR_ACTION_LABEL = "错误处理";

export function actionLabel(type: string, isError: boolean) {
  if (isError) return ERROR_ACTION_LABEL;
  return ACTION_LABELS[type] || type || "未命名动作";
}

/** 连线：source → targets */
export function buildOutgoingEdges(edges: FlowEdge[]) {
  const map = new Map<string, string[]>();
  edges.forEach(edge => {
    if (!edge?.source || !edge?.target) return;
    const list = map.get(edge.source) || [];
    list.push(edge.target);
    map.set(edge.source, list);
  });
  return map;
}

/** 从 fromId 沿连线找到下一个任务节点（跳过动作 / 错误节点） */
function nextTaskNode(
  fromId: string,
  nodeMap: Map<string, FlowNode>,
  outEdges: Map<string, string[]>,
  visited: Set<string>
): FlowNode | null {
  const queue = [...(outEdges.get(fromId) || [])];
  while (queue.length) {
    const id = queue.shift() as string;
    if (visited.has(id)) continue;
    visited.add(id);
    const node = nodeMap.get(id);
    if (!node) continue;
    if (node.type === "task") return node;
    if (node.type === "end") continue;
    queue.push(...(outEdges.get(id) || []));
  }
  return null;
}

/** 画布上的任务节点顺序（无 start 节点或连线不完整时按画布顺序兜底） */
export function findTaskNodes(
  nodes: FlowNode[],
  edges: FlowEdge[]
): FlowNode[] {
  const nodeMap = new Map(
    nodes.map(node => [node.id, node] as [string, FlowNode])
  );
  const outEdges = buildOutgoingEdges(edges);
  const start = nodes.find(node => node.type === "start");

  const tasks: FlowNode[] = [];
  if (start) {
    const visited = new Set<string>([start.id]);
    let currentId: string | null = start.id;
    while (currentId) {
      const next = nextTaskNode(currentId, nodeMap, outEdges, visited);
      if (!next) break;
      tasks.push(next);
      currentId = next.id;
    }
  }

  // 兜底：把未串到主链路里的任务节点也带上
  nodes
    .filter(node => node.type === "task" && !tasks.includes(node))
    .forEach(node => tasks.push(node));
  return tasks;
}

/** 任务节点后续的动作 / 错误处理节点（沿动作链遍历，遇到 task / end 停止） */
export function collectActions(
  taskId: string,
  nodeMap: Map<string, FlowNode>,
  outEdges: Map<string, string[]>
) {
  const result: Array<{ node: FlowNode; isError: boolean }> = [];
  const queue = [...(outEdges.get(taskId) || [])];
  const visited = new Set<string>();
  while (queue.length) {
    const id = queue.shift() as string;
    if (visited.has(id)) continue;
    visited.add(id);
    const node = nodeMap.get(id);
    if (!node) continue;
    if (node.type === "action" || node.type === "error") {
      result.push({ node, isError: node.type === "error" });
      queue.push(...(outEdges.get(id) || []));
    }
  }
  return result;
}

function fmt(value: unknown) {
  const num = Number(value);
  return Number.isFinite(num) ? num.toFixed(2) : "-";
}

/** 动作参数描述 */
function describeAction(type: string, action: any, segment: RoutePoint[]) {
  const params = action.params || {};
  const parts: string[] = [];
  if (type === "GOTO") {
    const wpId = Number(params.targetWp?.wpId) || 0;
    parts.push(
      wpId > 0
        ? `航点 ${wpId}`
        : `坐标 (${fmt(params.targetWp?.lat)}, ${fmt(params.targetWp?.lon)})`
    );
  } else if (type === "PHOTO") {
    parts.push(`数量 ${params.num ?? "-"}，间隔 ${params.intervalSec ?? "-"}s`);
  } else if (type === "HOVER") {
    parts.push(`悬停 ${params.timeSec ?? "-"}s`);
  } else if (type === "TAKEOFF") {
    parts.push(`起飞高度 ${params.targetAlt ?? "-"}m`);
  } else if (type.startsWith("SERVICE")) {
    parts.push(`服务 ${params.type || "-"}`);
  }
  if (action.timeoutSec != null) parts.push(`超时 ${action.timeoutSec}s`);
  if (segment.length && type !== "GOTO") {
    const first = segment[0].wpId;
    const last = segment[segment.length - 1].wpId;
    parts.push(`航段 ${first}-${last}`);
  }
  return parts.join("，");
}

/** GOTO 的目标点（优先按航点号，其次用自定义坐标） */
function gotoPoint(action: any, route: RoutePoint[]): FlowPoint | null {
  const target = action.params?.targetWp;
  const wpId = Number(target?.wpId) || 0;
  if (wpId > 0) {
    const found = route.find(point => point.wpId === wpId);
    return found ? [found.x, found.y] : null;
  }
  const lon = Number(target?.lon);
  const lat = Number(target?.lat);
  if (
    Number.isFinite(lon) &&
    Number.isFinite(lat) &&
    !(lon === 0 && lat === 0)
  ) {
    return [lon, lat];
  }
  return null;
}

/** 任务节点使用的航段：优先 pathString，其次 fromWp/toWp，最后整条航线兜底 */
function resolveSegment(data: FlowNodeData, route: RoutePoint[]): RoutePoint[] {
  const pathIds = Array.isArray(data.pathString)
    ? data.pathString.map(Number).filter((id: number) => id > 0)
    : [];
  if (pathIds.length) {
    const picked = route.filter(point => pathIds.includes(point.wpId));
    if (picked.length) return picked;
  }

  const taskWpIds = [
    Number(data.taskInfo?.fromWp?.wpId) || 0,
    Number(data.taskInfo?.toWp?.wpId) || 0
  ].filter(id => id > 0);
  if (taskWpIds.length) {
    const picked = route.filter(point => taskWpIds.includes(point.wpId));
    if (picked.length) return picked;
  }

  return route.slice();
}

/**
 * 汇总流程里的任务 → 航段 → 动作，生成地图标记。
 *
 * - 航段折线：任务节点所选航点（绿色粗线），整条航线作为灰色背景线；
 * - GOTO 动作：标记在目标航点（或自定义坐标）；
 * - 其它动作：标记在所属航段的终点航点（航段结束后执行）。
 */
export function planFlowMarkers(
  nodes: FlowNode[],
  edges: FlowEdge[],
  routes: RouteMap
): MarkerPlan {
  const plan: MarkerPlan = {
    markers: [],
    rows: [],
    segments: [],
    routeLines: [],
    taskCount: 0
  };

  const tasks = findTaskNodes(nodes, edges);
  plan.taskCount = tasks.length;
  if (!tasks.length) return plan;

  const nodeMap = new Map(
    nodes.map(node => [node.id, node] as [string, FlowNode])
  );
  const outEdges = buildOutgoingEdges(edges);
  /** 同一条航线只画一次背景线 */
  const drawnRoutes = new Set<number>();

  tasks.forEach((task, index) => {
    const taskLabel = `任务${index + 1}`;
    const data = task.data || {};
    const routeId = Number(data.routeId) || 0;
    const route = routeId > 0 ? routes.get(routeId) || [] : [];
    const segment = resolveSegment(data, route);

    if (segment.length) {
      plan.segments.push({
        points: segment.map(point => [point.x, point.y] as FlowPoint),
        ids: segment.map(point => point.wpId),
        label: taskLabel
      });
      if (
        route.length > 1 &&
        segment.length !== route.length &&
        !drawnRoutes.has(routeId)
      ) {
        drawnRoutes.add(routeId);
        plan.routeLines.push(
          route.map(point => [point.x, point.y] as FlowPoint)
        );
      }
    }

    const anchor: FlowPoint | null = segment.length
      ? [segment[segment.length - 1].x, segment[segment.length - 1].y]
      : null;

    collectActions(task.id, nodeMap, outEdges).forEach(item => {
      const action = item.node.data?.action || {};
      const type = String(action.type || "");
      const desc = item.isError
        ? action.params?.reason || "未填写错误原因"
        : describeAction(type, action, segment);
      let point = anchor;
      if (!item.isError && type === "GOTO") {
        point = gotoPoint(action, route) ?? anchor;
      }

      const row: MarkerRow = {
        task: taskLabel,
        action: actionLabel(type, item.isError),
        type,
        desc,
        isError: item.isError,
        located: point !== null
      };
      if (!row.located) row.desc = `${desc}（未定位）`;
      plan.rows.push(row);

      if (point) {
        plan.markers.push({
          point,
          type,
          action: row.action,
          task: taskLabel,
          desc,
          isError: item.isError
        });
      }
    });
  });

  return plan;
}

/* --------------------------- 标签避让布局 --------------------------- */

/** 待布局的动作标签（宽高由调用方按画布字体测量） */
export type PendingPill = {
  marker: ActionMarker;
  anchorX: number;
  anchorY: number;
  width: number;
  height: number;
};

/** 布局后的动作标签 */
export type MarkerPill = PendingPill & {
  x: number;
  y: number;
  /** 标签位于锚点的哪一侧 */
  side: "left" | "right";
};

const PILL_GAP = 6;
const PILL_OFFSET_X = 16;
const EDGE_PADDING = 8;

function overlaps(a: MarkerPill, b: MarkerPill, padding = 4) {
  return !(
    a.x + a.width + padding <= b.x ||
    b.x + b.width + padding <= a.x ||
    a.y + a.height + padding <= b.y ||
    b.y + b.height + padding <= a.y
  );
}

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), Math.max(max, min));
}

/**
 * 动作标签布局：同一锚点（同一航段终点 / 同一航点）的多个动作竖排错开，
 * 不同锚点的标签再互相避让，避免多个动作在地图上被覆盖。
 */
export function layoutMarkerPills(
  items: PendingPill[],
  bounds: { width: number; height: number }
): MarkerPill[] {
  if (!items.length) return [];

  // 1. 同一锚点的动作竖排成一列，并以锚点为中心对齐
  const groups = new Map<string, PendingPill[]>();
  items.forEach(item => {
    const key = `${item.anchorX.toFixed(1)},${item.anchorY.toFixed(1)}`;
    const list = groups.get(key) || [];
    list.push(item);
    groups.set(key, list);
  });

  const pills: MarkerPill[] = [];
  groups.forEach(list => {
    const anchorX = list[0].anchorX;
    const anchorY = list[0].anchorY;
    const maxWidth = Math.max(...list.map(item => item.width));
    const side: "left" | "right" =
      anchorX + PILL_OFFSET_X + maxWidth <= bounds.width - EDGE_PADDING
        ? "right"
        : "left";

    const totalHeight = list.reduce(
      (sum, item) => sum + item.height + PILL_GAP,
      -PILL_GAP
    );
    let top = clamp(
      anchorY - totalHeight / 2,
      EDGE_PADDING,
      bounds.height - totalHeight - EDGE_PADDING
    );

    list.forEach(item => {
      const x =
        side === "right"
          ? anchorX + PILL_OFFSET_X
          : anchorX - PILL_OFFSET_X - item.width;
      pills.push({
        marker: item.marker,
        anchorX,
        anchorY,
        width: item.width,
        height: item.height,
        x: Math.max(EDGE_PADDING, x),
        y: top,
        side
      });
      top += item.height + PILL_GAP;
    });
  });

  // 2. 不同锚点的标签互相避让：按纵向位置依次向下推开
  pills.sort((a, b) => a.y - b.y);
  const placed: MarkerPill[] = [];
  pills.forEach(pill => {
    let guard = 0;
    let moved = true;
    while (moved && guard < 200) {
      moved = false;
      guard += 1;
      for (const other of placed) {
        if (overlaps(pill, other)) {
          pill.y = other.y + other.height + 4;
          moved = true;
        }
      }
    }
    placed.push(pill);
  });

  // 3. 底部越界时回收（极端情况下会略微重叠，优先保证标签可见）
  placed.forEach(pill => {
    pill.y = Math.min(
      pill.y,
      Math.max(bounds.height - pill.height - EDGE_PADDING, EDGE_PADDING)
    );
  });

  return placed;
}
