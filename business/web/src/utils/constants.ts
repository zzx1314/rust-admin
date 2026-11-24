export interface ActionMeta {
  icon: string;
  tone: string;
  param: string;
  trigger?: string;
  threshold?: string;
  kind: "point" | "leg";
}

export const ACTION_LIB: Record<string, ActionMeta> = {
  可见光拍照: {
    icon: "camera",
    tone: "blue",
    param: "1080P / 广角 / 单帧",
    trigger: "到达即执行",
    threshold: "—",
    kind: "point"
  },
  红外测温: {
    icon: "thermo",
    tone: "orange",
    param: "云台 P90 · ROI 中心",
    trigger: "停留后执行",
    threshold: "> 65℃ 生成告警",
    kind: "point"
  },
  "AI 识别": {
    icon: "scan",
    tone: "purple",
    param: "烟火 / 人员 / 仪表读数",
    trigger: "到达即执行",
    threshold: "置信度 > 0.85",
    kind: "point"
  },
  二维码扫码: {
    icon: "scan",
    tone: "teal",
    param: "设备资产标签",
    trigger: "停留后执行",
    threshold: "—",
    kind: "point"
  },
  语音播报: {
    icon: "sound",
    tone: "gray",
    param: "安全提示语音 01",
    trigger: "到达即执行",
    threshold: "—",
    kind: "point"
  },
  录像采集: {
    icon: "video",
    tone: "blue",
    param: "1080P / 25fps / 15s",
    trigger: "到达即执行",
    threshold: "—",
    kind: "point"
  },
  气体检测: {
    icon: "activity",
    tone: "teal",
    param: "CO / H2S / O2",
    trigger: "停留后执行",
    threshold: "CO > 30ppm 告警",
    kind: "point"
  },
  设备自检: {
    icon: "cpu",
    tone: "gray",
    param: "全传感器自检",
    trigger: "到达即执行",
    threshold: "—",
    kind: "point"
  },
  等待: {
    icon: "clock",
    tone: "gray",
    param: "等待 10s",
    trigger: "停留后执行",
    threshold: "—",
    kind: "point"
  },
  全程录像: {
    icon: "video",
    tone: "blue",
    param: "1080P / 25fps / 连续",
    kind: "leg"
  },
  连续测温: {
    icon: "thermo",
    tone: "orange",
    param: "每 2s 采样一次",
    kind: "leg"
  },
  周界扫描: {
    icon: "scan",
    tone: "purple",
    param: "侧向 120° 扫描",
    kind: "leg"
  },
  气体巡检: {
    icon: "activity",
    tone: "teal",
    param: "持续采样浓度",
    kind: "leg"
  },
  静默通行: {
    icon: "shield",
    tone: "gray",
    param: "关闭语音提示",
    kind: "leg"
  }
};

export const TRIGGERS = ["到达即执行", "停留后执行", "条件触发", "定时执行"];
export const FAIL_MODES = ["跳过继续", "重试 1 次", "重试 2 次", "中止路线", "人工接管"];
export const LEG_MODES = ["沿道路", "直线", "曲线平滑", "原地转向"];
export const AVOID_LEVELS = ["宽松", "标准", "严格"];
export const DEVICE_MODELS = ["X30-Pro", "Lite-S", "X30-Standard"];
export const PARK_TYPES = ["生产制造", "仓储物流", "研发办公", "综合园区"];
export const ZONE_TYPES = ["生产区", "仓储区", "周界", "办公区", "公共区", "地下", "作业区"];
export const TASK_TYPES = ["定时巡检", "定点拍照", "夜巡任务", "应急处置", "设备维护", "地图采集"];
export const EXECUTE_MODES = ["周期任务", "临时下发", "立即执行"];
/**
 * 任务结束动作（跑完要不要返航）。
 *
 * 值对应后端 `tasks.end_task` / 载荷 `endTaskMode`；空串 = 跟随路线，
 * 此时闭环路线仍按老行为「返航回起点」。巡检本体侧对应 `5000 kind=4` 的 `params.end_task`。
 *
 * ⚠️「原地待命」的**停车位就是路线的最后一个航点**（本体 `end_task=0` 停在最后一点），
 * 所以最后一个点必须留出 ≥0.5 m 净空：本体规划器把贴障碍的目标判成被占据
 * （`13377 无有效绕障轨迹`），任务会卡在「前往 P{n}」永远不终态。
 * 校验/发布时由服务端算净空（`business/server/src/handlers/maps/clearance.rs`）。
 */
export const END_ACTIONS = [
  { value: "", label: "跟随路线（由路线闭环方式决定）" },
  { value: "none", label: "原地待命（不返航）" },
  { value: "return_to_start", label: "返回起点" },
  { value: "return_to_charge", label: "返回充电点" }
] as const;

/**
 * 结束动作的中文文案。`loopMode` 只在「跟随路线」时有意义：
 * 闭环 → 返回起点，往返 / 单向 → 原地待命（客户端 `patrol_end_task` 的兜底）。
 */
export function endActionLabel(value?: string | null, loopMode?: string | null) {
  const found = END_ACTIONS.find(item => item.value === (value ?? ""));
  if (found && found.value) return found.label;
  return loopMode === "闭环" ? "跟随路线（闭环 → 返回起点）" : "跟随路线（原地待命）";
}
/**
 * 计划灯的可读摘要（计划详情 / 列表用）。
 *
 * **以计划为准**：计划里勾了任意一盏（`lightFill` / `lightLed`）就忽略路线航点动作里的
 * 「到点开灯 / 离开关灯」，灯从任务开始亮到终态 / 取消；`lightNightOnly` 只是把它限制在
 * 板端夜间时间窗内（默认 18:00-06:00，板子本地时间）。两盏都不勾 = 沿用航点动作。
 */
export function planLightSummary(task: {
  lightFill?: boolean | null;
  lightLed?: boolean | null;
  lightNightOnly?: boolean | null;
  lightNightStart?: string | null;
  lightNightEnd?: string | null;
}) {
  const lights = [
    task.lightFill ? "补光灯" : "",
    task.lightLed ? "LED 灯带" : ""
  ].filter(Boolean);
  // 计划里没配时段 = 板端默认（18:00-06:00）
  const window =
    task.lightNightOnly && task.lightNightStart && task.lightNightEnd
      ? `，仅夜间 ${task.lightNightStart}-${task.lightNightEnd}`
      : task.lightNightOnly
        ? "，仅夜间"
        : "";
  if (!lights.length) {
    return `按航点动作${window}`;
  }
  return `${lights.join(" + ")}（全程${window}）`;
}

export const TASK_LEVELS = ["常规", "重要", "紧急"];
export const DISPATCH_POLICIES = ["按路线顺序执行", "跳过非必经点", "仅执行采集动作"];
export const TASK_STATUSES = ["待执行", "执行中", "已暂停", "已完成", "异常"];
/**
 * 计划发布状态（`tasks.publish_status`）：**只有已发布的计划才会自动执行定时任务**。
 *
 * 草稿 = 只保存：建计划不顺手下发、后台扫描跳过、设备上线也不补发；
 * 人工「开始执行 / 重新下发」不受限制（现场要能直接跑一次）。
 */
export const PUBLISH_STATUSES = ["草稿", "已发布"] as const;
export const DEVICE_STATUSES = ["在线待命", "执行任务", "充电中", "待绑定", "离线", "故障"];
export const COORDINATE_SYSTEMS = ["WGS-84", "GCJ-02"];
