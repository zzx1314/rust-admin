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
  /** 实时视频地址（RTSP / HLS / FLV / WebRTC） */
  videoUrl: string | null;
  /** 视频协议类型：none / hls / flv / webrtc / rtsp / mp4 */
  videoType: string;
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
  /** 运动状态：1315 `motion_status`（卧倒时摇杆走不动） */
  motionStatus: string | null;
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
  | "light_on"
  | "light_off"
  /** 1302 cmd=60：通用模式（1304 摇杆的 lx/ly 只有在这个模式下生效） */
  | "generic_mode"
  /** 1302 cmd=61：原地模式 */
  | "in_place_mode"
  /** 1302 cmd=62：导航模式 */
  | "navigation_mode";

export interface Task {
  id: number;
  taskCode: string;
  taskName: string;
  taskType: string | null;
  executeMode: string | null;
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
  planStart: string | null;
  planEnd: string | null;
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
}

export interface RoutePoint {
  name: string;
  x: number;
  y: number;
  dwell: number;
  radius: number;
  must: boolean;
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
  parkId: number;
  parkName: string | null;
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
