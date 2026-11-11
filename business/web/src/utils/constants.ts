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
export const TASK_LEVELS = ["常规", "重要", "紧急"];
export const DISPATCH_POLICIES = ["按路线顺序执行", "跳过非必经点", "仅执行采集动作"];
export const TASK_STATUSES = ["待执行", "执行中", "已完成", "异常"];
export const DEVICE_STATUSES = ["在线待命", "执行任务", "充电中", "待绑定", "离线", "故障"];
export const COORDINATE_SYSTEMS = ["WGS-84", "GCJ-02"];
