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
  signal: number;
  temperature: number;
  runtimeMinutes: number;
  odometer: number;
  position: string | null;
  currentTaskName: string | null;
  lastHeartbeat: string | null;
  firmware: string | null;
  deviceIp: string | null;
  createdAt: string;
  updatedAt: string;
}

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
  routeCount: number;
  taskCount: number;
}

export interface BackendMap {
  id: number;
  mapName: string;
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
