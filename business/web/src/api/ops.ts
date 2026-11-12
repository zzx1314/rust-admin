import { del, get, post, put } from "./http";
import type {
  Alert,
  BackendMap,
  DashboardSummary,
  Device,
  LoginResponse,
  MapRaster,
  MapRefs,
  MapVersion,
  Park,
  TeleopAxes,
  Route,
  RouteValidation,
  Setting,
  Task,
  TaskEvent,
  UserInfo,
  Zone
} from "./types";

export const authApi = {
  login: (username: string, password: string) =>
    post<LoginResponse>("/auth/login", { username, password }),
  me: () => get<UserInfo>("/auth/me")
};

export const parkApi = {
  list: (params?: object) => get<Park[]>("/parks", params),
  get: (id: number) => get<Park>(`/parks/${id}`),
  create: (data: object) => post<Park>("/parks", data),
  update: (id: number, data: object) => put<Park>(`/parks/${id}`, data),
  remove: (id: number) => del<boolean>(`/parks/${id}`)
};

export const mapApi = {
  list: (params?: object) => get<MapVersion[]>("/map-versions", params),
  byPark: (parkId: number) => get<MapVersion[]>(`/parks/${parkId}/maps`),
  get: (id: string) => get<MapVersion>(`/map-versions/${encodeURIComponent(id)}`),
  raster: (id: string) =>
    get<MapRaster>(`/map-versions/${encodeURIComponent(id)}/raster`),
  refs: (id: string) =>
    get<MapRefs>(`/map-versions/${encodeURIComponent(id)}/refs`),
  publish: (id: string) =>
    post<MapVersion>(`/map-versions/${encodeURIComponent(id)}/publish`),
  newVersion: (id: string, note?: string) =>
    post<MapVersion>(`/map-versions/${encodeURIComponent(id)}/new-version`, {
      note
    }),
  backendMaps: (params?: object) =>
    get<BackendMap[]>("/map-versions/backend", params),
  importBackend: (backendMapId: number, parkId?: number, bind = true) =>
    post<MapVersion>("/map-versions/import-backend", {
      backendMapId,
      parkId,
      bind
    })
};

export const zoneApi = {
  list: (params?: object) => get<Zone[]>("/zones", params),
  create: (data: object) => post<Zone>("/zones", data),
  update: (id: number, data: object) => put<Zone>(`/zones/${id}`, data),
  remove: (id: number) => del<boolean>(`/zones/${id}`)
};

export const deviceApi = {
  list: (params?: object) => get<Device[]>("/devices", params),
  stats: () =>
    get<{
      total: number;
      online: number;
      unbound: number;
      counts: Record<string, number>;
    }>("/devices/stats"),
  get: (id: number) => get<Device>(`/devices/${id}`),
  create: (data: object) => post<Device>("/devices", data),
  update: (id: number, data: object) => put<Device>(`/devices/${id}`, data),
  remove: (id: number) => del<boolean>(`/devices/${id}`),
  /** 手动把设备绑定到园区（可同时指定区域） */
  bind: (id: number, parkId: number, zoneId?: number | null) =>
    post<Device>(`/devices/${id}/bind`, { parkId, zoneId }),
  /** 解除园区绑定，设备回到「待绑定」 */
  unbind: (id: number) => del<Device>(`/devices/${id}/bind`),
  refresh: () => post<number>("/devices/refresh"),
  command: (id: number, command: string) =>
    post<{
      deviceId: string;
      command: string;
      dispatchedAt: string;
      wsDelivered: boolean;
      teleopHolder: string | null;
    }>(`/devices/${id}/command`, { command }),

  /**
   * 重新定位（载图 + 定位，4030）。
   *
   * 不传位姿 = 用上次位姿文件做完整重定位（设备没被搬动时的常规恢复）；
   * 传 `x/y/heading`（度）= 以该位姿为初值，设备被搬动后用它纠偏。
   * 不传 `mapId` 时依次回退：设备上报的当前地图 → 园区地图基线。
   */
  locate: (
    id: number,
    payload: { mapId?: string | null; x?: number; y?: number; heading?: number } = {}
  ) =>
    post<{
      deviceId: string;
      command: string;
      dispatchedAt: string;
      wsDelivered: boolean;
    }>(`/devices/${id}/locate`, payload),

  // ── 远程接管 ────────────────────────────────────────────
  /** 必须接管后才能下发摇杆与动作 */
  takeOver: (id: number) =>
    post<{ deviceId: string; holder: string; expiresInSec: number; wsDelivered: boolean }>(
      `/devices/${id}/teleop/take`
    ),
  release: (id: number) =>
    post<{ deviceId: string; released: boolean; wsDelivered: boolean }>(
      `/devices/${id}/teleop/release`
    ),
  /** 保活，刷新接管超时 */
  ping: (id: number) =>
    post<{ deviceId: string; expiresInSec: number }>(`/devices/${id}/teleop/ping`),
  teleop: (id: number, axes: TeleopAxes) =>
    post<{ deviceId: string; stopped: boolean; wsDelivered: boolean }>(
      `/devices/${id}/teleop`,
      axes
    ),
  action: (id: number, action: string) =>
    post<{ deviceId: string; action: string; label: string; wsDelivered: boolean }>(
      `/devices/${id}/action`,
      { action }
    )
};

export const taskApi = {
  list: (params?: object) => get<Task[]>("/tasks", params),
  stats: () =>
    get<{ total: number; counts: Record<string, number>; doneRate: number }>(
      "/tasks/stats"
    ),
  get: (id: number) => get<Task>(`/tasks/${id}`),
  events: (id: number) => get<TaskEvent[]>(`/tasks/${id}/events`),
  create: (data: object) => post<Task>("/tasks", data),
  update: (id: number, data: object) => put<Task>(`/tasks/${id}`, data),
  remove: (id: number) => del<boolean>(`/tasks/${id}`),
  updateStatus: (id: number, taskStatus: string, progress?: number) =>
    put<Task>(`/tasks/${id}/status`, { taskStatus, progress }),
  /** 真正下发任务（含航点 / 地图）；设备离线时接口直接报错 */
  dispatch: (id: number) => post<Task>(`/tasks/${id}/dispatch`)
};

export const routeApi = {
  list: (params?: object) => get<Route[]>("/routes", params),
  get: (id: number) => get<Route>(`/routes/${id}`),
  create: (data: object) => post<Route>("/routes", data),
  update: (id: number, data: object) => put<Route>(`/routes/${id}`, data),
  remove: (id: number) => del<boolean>(`/routes/${id}`),
  validate: (id: number) => post<RouteValidation>(`/routes/${id}/validate`),
  publish: (id: number) => post<Route>(`/routes/${id}/publish`),
  optimize: (id: number) => post<Route>(`/routes/${id}/optimize`)
};

export const alertApi = {
  list: (params?: object) => get<Alert[]>("/alerts", params),
  stats: () =>
    get<{ total: number; counts: Record<string, number>; pending: number }>(
      "/alerts/stats"
    ),
  get: (id: number) => get<Alert>(`/alerts/${id}`),
  create: (data: object) => post<Alert>("/alerts", data),
  update: (id: number, data: object) => put<Alert>(`/alerts/${id}`, data),
  remove: (id: number) => del<boolean>(`/alerts/${id}`),
  handle: (id: number) => post<Alert>(`/alerts/${id}/handle`),
  handleAll: () => post<number>("/alerts/handle-all")
};

export const settingApi = {
  list: () => get<Setting[]>("/settings"),
  save: (items: Array<{ settingKey: string; settingValue: string | null }>) =>
    put<Setting[]>("/settings", items)
};

export const dashboardApi = {
  summary: () => get<DashboardSummary>("/dashboard/summary")
};
