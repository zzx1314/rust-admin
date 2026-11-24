export interface ApiEnvelope<T> {
  code: number;
  msg: string;
  data: T;
}

export interface UserInfo {
  id: number;
  username: string;
  displayName: string | null;
  role: string;
  /** 关联的中台账号；本地服务账号为空 */
  zentaiUserId: number | null;
  zentaiUsername: string | null;
  isAdmin: boolean;
}

export interface LoginResponse {
  token: string;
  user: UserInfo;
}

export interface Park {
  id: number;
  parkCode: string;
  parkName: string;
  parkDescription: string | null;
  parkAddress: string | null;
  area: number;
  ownerName: string | null;
  parkType: string | null;
  status: string;
  color: string;
  mapVersionId: string | null;
  mapStatus: string;
  createdAt: string;
  updatedAt: string;
  zoneCount: number;
  deviceCount: number;
  taskCount: number;
  doneCount: number;
}

export interface Zone {
  id: number;
  parkId: number;
  parkName: string | null;
  zoneName: string;
  zoneType: string | null;
  frequency: string | null;
  status: string;
  deviceCount: number;
  createdAt: string;
  updatedAt: string;
}

export interface Device {
  id: number;
  deviceId: string;
  model: string | null;
  parkId: number | null;
  parkName: string | null;
  zoneId: number | null;
  zoneName: string | null;
  status: string;
  battery: number;
  temperature: number;
  runtimeMinutes: number;
  odometer: number;
  position: string | null;
  currentTaskName: string | null;
  lastHeartbeat: string | null;
  firmware: string | null;
  deviceIp: string | null;
  /** 当前远程接管人；为空表示无人接管 */
  teleopHolder: string | null;
  teleopAt: string | null;
  /** 本体链路是否在线；false 时电量/温度等是模拟值，不可当真实数据展示 */
  bodyOnline: boolean;
  /** 客户端工作模式：robot（接本体）/ sim（纯模拟）/ null（未上报） */
  runMode: "robot" | "sim" | null;
  /** 充电状态：本体 1315 上报（不是按电量猜的） */
  charging: boolean;
  /** 本体工作模式：1315 `mode`（协议里的 MachineStatus），如 remote / auto */
  robotMode: string | null;
  /** 定位状态：100 正常 / 121 重定位中 / 122 定位丢失 / 210 定位失败 / 255 未上报 */
  locStatus: number;
  /** 机器人当前生效地图（业务侧 mapId）；为空表示未上报或未载图 */
  activeMapId: string | null;
  /** 控制源：0 未知 / 1 APP / 2 SDK / 3 其他（含硬件遥控器） */
  ctrlSource: number;
  /** 强制接管中：遥控器断电但本体仍报 ctrlSource=3，二次确认后跳过了这条门禁 */
  teleopForce: boolean;
  /** 运动状态：1315 `motion_status`（卧倒时摇杆走不动） */
  motionStatus: string | null;
  /** 1315 `estop.software`：软件急停是否按下 */
  estopSoftware: boolean | null;
  /** 1207 `sdk_node`：本体算法节点是否就绪 */
  sdkNodeAvailable: boolean | null;
  createdAt: string;
  updatedAt: string;
}

/** 遥控摇杆三轴，取值 -1 ~ 1 */
export interface TeleopAxes {
  forward: number;
  strafe: number;
  turn: number;
  speed?: number;
}

/** 本体动作机器码（协议 1302 有对应 cmd 码值的动作） */
export type DeviceAction =
  | "stand_up"
  | "lie_down"
  | "emergency_stop"
  /** 1302 cmd=1：清 estop.software（现场 X30-Pro 无物理急停按钮，只能靠它解锁） */
  | "clear_emergency_stop"
  | "light_on"
  | "light_off"
  /** 1302 cmd=60：通用模式（1304 摇杆的 lx/ly 只有在这个模式下生效） */
  | "generic_mode"
  /** 1302 cmd=61：原地模式 */
  | "in_place_mode"
  /** 1302 cmd=62：导航模式 */
  | "navigation_mode"
  /** 1302 cmd=110 / 111：避障开关（感知降级时规划器会误判终点附近有障碍） */
  | "avoid_on"
  | "avoid_off";

export interface Task {
  id: number;
  taskCode: string;
  taskName: string;
  taskType: string | null;
  executeMode: string | null;
  /**
   * 结束动作（返航开关）：null = 跟随路线；`none` = 原地待命；
   * `return_to_start` / `return_to_charge`，见 `utils/constants.ts::END_ACTIONS`。
   */
  endTask: string | null;
  /**
   * 计划级「自动开灯」的两盏灯：勾了就**以计划为准**——航点动作的到点开灯 / 离开关灯
   * 不再生效，灯从任务开始亮到终态 / 取消。两盏都不勾 = 沿用航点动作。
   */
  lightFill: boolean;
  lightLed: boolean;
  /**
   * 计划级「仅夜间自动开灯」：`true` = 上面那两盏灯只在夜间时段开。
   *
   * 夜间判定在**板端**（板子本地时间），平台/客户端只把开关和时段转下去。
   */
  lightNightOnly: boolean;
  /**
   * 计划里自定义的夜间时段（`HH:MM`，可跨零点）；两个都为 `null` = 用板端默认
   * （`LIGHT_NIGHT_START` / `LIGHT_NIGHT_END`，默认 18:00-06:00）。
   */
  lightNightStart: string | null;
  lightNightEnd: string | null;
  level: string | null;
  deviceId: number | null;
  deviceCode: string | null;
  parkId: number | null;
  parkName: string | null;
  zoneId: number | null;
  zoneName: string | null;
  routeId: number | null;
  routeCode: string | null;
  routeName: string | null;
  /** 每天的执行窗口（`HH:MM`，允许跨零点）；由时间选择器写入，不再手填 */
  planStart: string | null;
  planEnd: string | null;
  /** 周期日：`1,2,3,4,5`（1=周一 … 7=周日）；null / 空 = 每天 */
  scheduleDays: string | null;
  /** 最近一次「到点自动下发」的日期（`YYYY-MM-DD`），同一天不会重复跑 */
  lastDispatchedOn: string | null;
  /**
   * 发布状态：`草稿` / `已发布`。
   *
   * **只有「已发布」的计划才会到点自动执行**：草稿只保存，建计划不顺手下发、
   * 扫描器跳过、设备上线也不补发（人工「开始执行」仍可跑）。
   * 与执行状态 `taskStatus` 是两回事，见 `utils/constants.ts::PUBLISH_STATUSES`。
   */
  publishStatus: string;
  /** 最近一次发布的时间（撤回不清空） */
  publishedAt: string | null;
  taskStatus: string;
  progress: number;
  dispatchPolicy: string | null;
  description: string | null;
  /** 智航 Pro 本体侧任务标识 */
  robotTaskId: string | null;
  /** 智航 Pro 执行阶段（mapping.* / navigation.* / patrol.* / recharge.* …） */
  robotStage: string | null;
  robotUpdatedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface RoutePointAction {
  type: string;
  param?: string;
  trigger?: string;
  threshold?: string;
  retry?: number;
  /**
   * 「AI 识别」专用：背包模型场景（Agent 的 `task`，如 `fire`）。
   * 来自「实时视频 → AI 识别」的模型目录；为空时服务端按 `param` 里的中文关键词匹配。
   */
  scene?: string;
  /**
   * 「AI 识别」专用：勾选的识别类别（Agent 的 label `name`，与实时视频下发一致）。
   * 为空表示该场景的全部类别。
   */
  labels?: string[];
  /**
   * 「AI 识别」专用：任务类型。
   *
   * - `exist`：检测是否存在——找到目标就存证/上报，不判数量；
   * - `count`：检测数量是否正确——还要核对数量，不一致（含「一个都没找到」）告警。
   *
   * 老数据没有这个字段时后端按「有没有 `expectCount`」推断。
   */
  checkType?: "exist" | "count";
  /**
   * 「AI 识别」专用：检查目标数量（`checkType: "count"` 时必填）。
   *
   * 由板端把 AI 检出的目标数与它比对，不一致时上报数量告警（告警中心 + 企业微信）。
   */
  expectCount?: number | null;
  /**
   * 「AI 识别」专用（仅 `checkType: "exist"`）：**未检出也告警**。
   *
   * `exist` 默认只在「检出」时告警（适合「不该出现」的目标：消防通道占用、明火、闯入）。
   * 勾上它，窗口正常到期却一个目标都没找到时会落一条「AI 识别未检出」告警——
   * 适合「必须有」的东西（灭火器、气体钢瓶、值班人员）。
   *
   * `count` 任务自带「0 ≠ N」数量告警，这个字段对它无效。
   */
  alertWhenMissing?: boolean;
  /**
   * 「AI 识别」专用（仅 `checkType: "count"`）：数量**一致**时也通知企业微信。
   *
   * 默认关闭：数量任务只在「不一致」时告警（告警中心 + 企业微信 + 自动派单）。
   * 勾上后，核对一致也会发一条群消息作为巡检回执，但**不落告警、不建工单**——
   * 正常结论不该进处置列表。
   */
  notifyWhenCountOk?: boolean;
  /**
   * 到点开**补光灯**（背包板 GPIO，经板端 Agent `/api/lights`）。
   *
   * 与 AI 识别无关：不配 AI 动作的航点也能只开灯。离开航点 / 任务结束时自动关灯；
   * 计划里勾了「仅夜间自动开灯」时只有夜间时段才真的开。
   */
  lightFill?: boolean;
  /**
   * 到点开 **LED 灯带**（板端 Agent 拉起厂商的 `test_rgbrun`，行为是循环闪烁）。
   *
   * 同上：离开航点 / 任务结束时关；计划「仅夜间自动开灯」勾上时只在夜间开。
   */
  lightLed?: boolean;
  /**
   * 「录像采集」专用：这个航点录多久（秒，3~300，默认 15）。
   *
   * 板端 Agent 到点起 ffmpeg（本机 mediamtx 的 dogcam），离开航点就停；
   * 老数据没有这个字段时服务端/客户端从 `param` 文本里读（「1080P / 25fps / 15s」）。
   */
  recordSeconds?: number;
  /**
   * 「全程录像」专用：板端切片长度（秒，30~1800，默认 300）。
   *
   * 全程录像跨航点一直录到任务终态；按这个长度切 mp4，客户端逐段回传到平台，
   * 弱网断一段只重传一段。
   */
  recordSegmentSeconds?: number;
}

/**
 * 巡检录像（航点「录像采集」/ 航段「全程录像」）。
 *
 * 板端 Agent 落 mp4 → 设备客户端回传 → 任务详情「任务录像」播放 / 下载。
 * `scope = "point"` 是航点录像（`pointIndex` 有值），`"task"` 是全程切片（`pointIndex = -1`）。
 */
export interface TaskRecording {
  id: number;
  taskId: number | null;
  taskCode: string;
  deviceCode: string;
  planId: string | null;
  pointIndex: number;
  pointName: string | null;
  scope: "point" | "task";
  fileName: string;
  fileSize: number;
  /** 录像时长（秒） */
  durationSec: number;
  startedAt: string | null;
  endedAt: string | null;
  createdAt: string;
}

/** AI 识别的两类任务：检测是否存在 / 检测数量是否正确 */
export type AiCheckType = "exist" | "count";

export interface RoutePoint {
  name: string;
  x: number;
  y: number;
  dwell: number;
  radius: number;
  must: boolean;
  /**
   * 到点后机身转过头对准的偏航角（度），0° = 地图 +X 方向。
   *
   * 下发链路：`heading` →(`Pose::new(x, y, heading.to_radians())`) → 本体路径点的
   * `orientation` 四元数（只有 yaw，roll/pitch 恒 0）。**不是云台角度**——
   * 本体协议没有云台/PTZ 指令，这里转的是整只狗。
   */
  heading: number;
  onFail: string;
  actions: RoutePointAction[];
}

export interface RouteLeg {
  mode: string;
  speed: number;
  avoid: string;
  actions: RoutePointAction[];
}

export interface RouteData {
  schemaVersion: number;
  /**
   * 坐标语义：`map` = 航点 x/y 是地图坐标系下的米（schema v3）。
   * 缺失说明是历史数据（800×460 画布像素），服务端拒绝下发。
   */
  coordFrame?: "map";
  loop: string;
  points: RoutePoint[];
  legs: RouteLeg[];
  returnLeg: RouteLeg | null;
}

export interface Route {
  id: number;
  routeCode: string;
  routeName: string;
  parkId: number | null;
  parkName: string | null;
  zoneId: number | null;
  zoneName: string | null;
  model: string | null;
  status: string;
  version: string;
  loopMode: string;
  defaultSpeed: number;
  routeDescription: string | null;
  routeData: string | null;
  pointCount: number;
  distanceM: number;
  durationS: number;
  batteryPct: number;
  publishedAt: string | null;
  mapVersionId: string | null;
  mapStatus: string | null;
  createdAt: string;
  updatedAt: string;
  usedTaskCount: number;
}

export interface MapVersion {
  id: string;
  /** 归属园区；`null` 表示还没绑定园区的设备地图 */
  parkId: number | null;
  parkName: string | null;
  /** 采集这张地图的设备编号 */
  deviceCode: string | null;
  mapName: string;
  version: string;
  frameId: string | null;
  crs: string | null;
  origin: string | null;
  resolution: string | null;
  floors: string | null;
  roadCount: number;
  noGoCount: number;
  status: string;
  mapHash: string | null;
  mapWidth: number | null;
  mapHeight: number | null;
  note: string | null;
  publishedAt: string | null;
  createdAt: string;
  source: string;
  sourceId: string | null;
  fileName: string | null;
  filePath: string | null;
  fileSize: number | null;
  ownerUserId: number | null;
  ownerUsername: string | null;
  sourceRef: string | null;
  routeCount: number;
  taskCount: number;
}

export interface BackendMap {
  id: number;
  mapName: string;
  /** 归属园区编码（业务库 parks.park_code，中台只存不校验） */
  parkCode: string | null;
  /** 归属园区名称 */
  parkName: string | null;
  fileName: string | null;
  filePath: string | null;
  fileFormat: string | null;
  fileSize: number | null;
  remarks: string | null;
  mapVersion: string | null;
  frameId: string | null;
  crs: string | null;
  mapOrigin: string | null;
  resolution: string | null;
  floors: string | null;
  roadCount: number | null;
  nogoCount: number | null;
  mapStatus: string | null;
  mapHash: string | null;
  publishTime: string | null;
  mapWidth: number | null;
  mapHeight: number | null;
  createTime: string | null;
  /** 中台 dog_map.user_id（地图归属用户） */
  userId: number | null;
  ownerUsername: string | null;
}

export interface MapRefRoute {
  id: number;
  routeCode: string;
  routeName: string;
  status: string;
  version: string;
  pointCount: number;
}

export interface MapRefTask {
  id: number;
  taskCode: string;
  taskName: string;
  taskStatus: string;
  routeId: number | null;
}

export interface MapRefs {
  map: MapVersion;
  routes: MapRefRoute[];
  tasks: MapRefTask[];
  deviceCount: number;
}

export interface MapRaster {
  mapId: string;
  image: string;
  resolution: string | null;
  origin: string | null;
  width: number;
  height: number;
}

export interface Alert {
  id: number;
  alertCode: string;
  level: string;
  content: string;
  deviceId: number | null;
  deviceCode: string | null;
  parkId: number | null;
  parkName: string | null;
  status: string;
  occurredAt: string;
  handledAt: string | null;
  createdAt: string;
  /** 派单到的责任人（按当班排班匹配，见 server/src/alert_flow.rs） */
  staffId: number | null;
  assignee: string | null;
  /** 自动生成的工单；非空表示这条告警已经派过单 */
  workOrderId: number | null;
  /** 工单编号（`WO-1115-01`） */
  workOrderCode: string | null;
  /** 派单依据：当班排班 SH-001（早班 08:00 - 16:00）/ 园区负责人 / 当天没有排班… */
  assigneeNote: string | null;
}

/** 企业微信推送记录（`notifications` 表） */
export interface NotificationRecord {
  id: number;
  channel: string;
  refType: string;
  refId: number | null;
  level: string;
  dedupeKey: string | null;
  payload: string;
  status: string;
  attempts: number;
  lastError: string | null;
  createdAt: string;
  sentAt: string | null;
}

/** 测试消息的同步结果：成功/失败都带企业微信原文 */
export interface NotifyTestResult {
  sent: boolean;
  message: string;
  webhook: string | null;
}

/**
 * 企业微信 webhook 的当前状态。
 * 接口永不返回完整地址（地址即凭据），只有掩码后的 `masked`。
 */
export interface NotifyWebhookStatus {
  /** 有没有可用地址：false 时告警不会真的进群 */
  configured: boolean;
  /** `db` = 平台里填的，`env` = 部署方的环境变量，`null` = 没配 */
  source: "db" | "env" | null;
  masked: string | null;
  /** 部署级总开关（`wecom.enabled`），关掉时填了也不发 */
  enabled: boolean;
  /**
   * 完整地址：只有管理员点「显示」（`?reveal=true`）才有值，
   * 普通读取连这个字段都不会出现。
   */
  url?: string | null;
}

export interface Setting {
  id: number;
  groupName: string;
  settingKey: string;
  settingValue: string | null;
  valueType: string;
  description: string | null;
  updatedAt: string;
}

export interface TaskEvent {
  id: number;
  eventType: string;
  title: string;
  description: string | null;
  deviceCode: string | null;
  taskCode: string | null;
  createdAt: string;
}

export interface DashboardSummary {
  kpis: {
    onlineDevices: { value: number; total: number; foot: string };
    todayTasks: { value: number; completed: number; rate: number; foot: string };
    parks: { value: number; zones: number; area: number; foot: string };
    pendingAlerts: { value: number; total: number; foot: string };
  };
  deviceStatus: Array<{ name: string; value: number; color: string }>;
  alerts: Alert[];
  trend: { days: string[]; plan: number[]; done: number[] };
  timeline: TaskEvent[];
  parks: Park[];
  mapDevices: Device[];
  totals: { devices: number; zones: number; tasks: number; alerts: number };
}

export interface RouteValidationItem {
  ok: boolean;
  title: string;
  description: string;
}

export interface RouteValidation {
  routeId: number;
  routeCode: string;
  routeName: string;
  passed: number;
  total: number;
  metrics: {
    pointCount: number;
    distanceM: number;
    durationS: number;
    actionCount: number;
    batteryPct: number;
    dwellS: number;
  };
  items: RouteValidationItem[];
}

/** 责任人库：排班与工单共用的运维人员 */
export interface Staff {
  id: number;
  staffCode: string;
  name: string;
  role: string | null;
  team: string | null;
  phone: string | null;
  color: string;
  status: string;
  skills: string | null;
  /**
   * 关联的平台账号（`users.id`）：决定「我的工单」与处置权限。
   * 空 = 没有平台账号的现场人员（外包/临时工），派给他的单按公共待办处理。
   */
  userId?: number | null;
  /** 绑定账号的用户名（列表展示用） */
  userName?: string | null;
  createdAt: string;
  updatedAt: string;
  /** 周视图范围内班次数（列表按 from/to 统计） */
  shiftCount: number;
  /** 在办工单数（状态不是「已完成」） */
  openOrderCount: number;
}

/** 责任人「关联平台账号」下拉选项（`GET /api/accounts`，不含任何敏感列） */
export interface AccountOption {
  id: number;
  username: string;
  displayName: string | null;
  role: string;
  /** 该账号已绑定的责任人（一个账号最多绑一个） */
  staffId: number | null;
  staffName: string | null;
}

/** 责任人库 Excel 导入：逐行结果（只回报跳过 / 失败的行） */
export interface StaffImportRow {
  /** Excel 行号（1 开始） */
  row: number;
  name: string;
  /** 跳过（重名 / 工号重复）/ 失败（内容不合法或写库失败） */
  status: "skipped" | "failed";
  message: string;
}

/** 责任人库 Excel 导入结果 */
export interface StaffImportResult {
  /** 文件里的数据行数（不含表头与空行） */
  total: number;
  created: number;
  skipped: number;
  failed: number;
  createdNames: string[];
  issues: StaffImportRow[];
}

/** 排班：某天某班次由某个责任人负责某条巡检计划 */
export interface Shift {
  id: number;
  shiftCode: string;
  /** YYYY-MM-DD */
  shiftDate: string;
  slot: string;
  timeRange: string;
  staffId: number | null;
  ownerName: string;
  staffRole: string | null;
  staffTeam: string | null;
  staffColor: string | null;
  parkId: number | null;
  parkName: string | null;
  zoneId: number | null;
  zoneName: string | null;
  taskId: number | null;
  taskName: string | null;
  deviceCount: number;
  status: string;
  note: string | null;
  createdAt: string;
  updatedAt: string;
  createdBy: number | null;
}

/** 班次定义：早 / 中 / 夜是默认班次，「班次设置」里可以自定义更多班次 */
export interface ShiftSlot {
  id: number;
  name: string;
  /** HH:MM */
  startTime: string;
  /** HH:MM；结束时间不晚于开始时间表示跨零点 */
  endTime: string;
  /** `08:00 - 16:00`（服务端按起止时间拼好） */
  timeRange: string;
  color: string;
  /** 周视图行顺序，小的在上 */
  sortOrder: number;
  /** 停用的班次不出现在新建排班下拉里，历史排班照旧显示 */
  active: boolean;
  note: string | null;
  /** 引用该班次的排班条数：有引用时只能停用，不能删除 */
  shiftCount: number;
  createdAt: string;
  updatedAt: string;
}

/** 整段复制排班的跳过明细（目标时段已占用 / 目标区间天数不足） */
export interface ShiftCopySkip {
  shiftDate: string;
  slot: string;
  ownerName: string;
  reason: string;
}

export interface ShiftCopyResult {
  /** 源区间命中的排班条数 */
  source: number;
  created: number;
  /** 覆盖模式清掉的目标排班条数 */
  replaced: number;
  skipped: ShiftCopySkip[];
}

export interface ShiftStats {
  total: number;
  counts: Record<string, number>;
  published: number;
  pending: number;
  owners: number;
}

/** 工单：异常与人工任务的派发、处理与闭环 */
export interface WorkOrder {
  id: number;
  orderCode: string;
  title: string;
  orderType: string;
  source: string;
  sourceRef: string | null;
  priority: string;
  staffId: number | null;
  assignee: string;
  staffTeam: string | null;
  staffColor: string | null;
  parkId: number | null;
  parkName: string | null;
  deviceId: number | null;
  deviceCode: string | null;
  deadline: string | null;
  status: string;
  progress: number;
  description: string | null;
  alertId: number | null;
  closedAt: string | null;
  createdAt: string;
  updatedAt: string;
  createdBy: number | null;
}

export interface WorkOrderLog {
  id: number;
  workOrderId: number;
  actor: string;
  action: string;
  content: string;
  createdAt: string;
}

/** 工单关联的告警：自动派单时由告警建单，同类告警会持续挂到同一张单上 */
export interface WorkOrderAlert {
  id: number;
  alertCode: string;
  level: string;
  /** 告警正文，AI 数量核对/检出会在这里带上板端证据图路径 */
  content: string;
  status: string;
  occurredAt: string;
}

export type WorkOrderDetail = WorkOrder & {
  logs: WorkOrderLog[];
  /** 这张单挂着的全部告警（按时间正序），现场据此翻历次识别结论与证据路径 */
  alerts: WorkOrderAlert[];
};

export interface WorkOrderStats {
  total: number;
  counts: Record<string, number>;
  open: number;
}
