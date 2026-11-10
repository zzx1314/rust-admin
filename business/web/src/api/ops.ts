import { del, get, post, put } from "./http";
import type {
  Alert,
  DashboardSummary,
  Device,
  LoginResponse,
  MapRefs,
  MapVersion,
  Park,
  Route,
  RouteValidation,
  Setting,
  Task,
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
  refs: (id: string) =>
    get<MapRefs>(`/map-versions/${encodeURIComponent(id)}/refs`),
  publish: (id: string) =>
    post<MapVersion>(`/map-versions/${encodeURIComponent(id)}/publish`),
  newVersion: (id: string, note?: string) =>
    post<MapVersion>(`/map-versions/${encodeURIComponent(id)}/new-version`, {
      note
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
    get<{ total: number; online: number; counts: Record<string, number> }>(
      "/devices/stats"
    ),
  get: (id: number) => get<Device>(`/devices/${id}`),
  create: (data: object) => post<Device>("/devices", data),
  update: (id: number, data: object) => put<Device>(`/devices/${id}`, data),
  remove: (id: number) => del<boolean>(`/devices/${id}`),
  refresh: () => post<number>("/devices/refresh"),
  command: (id: number, command: string) =>
    post<{ deviceId: string; command: string; dispatchedAt: string }>(
      `/devices/${id}/command`,
      { command }
    )
};

export const taskApi = {
  list: (params?: object) => get<Task[]>("/tasks", params),
  stats: () =>
    get<{ total: number; counts: Record<string, number>; doneRate: number }>(
      "/tasks/stats"
    ),
  get: (id: number) => get<Task>(`/tasks/${id}`),
  create: (data: object) => post<Task>("/tasks", data),
  update: (id: number, data: object) => put<Task>(`/tasks/${id}`, data),
  remove: (id: number) => del<boolean>(`/tasks/${id}`),
  updateStatus: (id: number, taskStatus: string, progress?: number) =>
    put<Task>(`/tasks/${id}/status`, { taskStatus, progress })
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
