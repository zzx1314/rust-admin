import { del, download, get, post, put, upload } from "./http";
import type {
  AccountOption,
  Alert,
  BackendMap,
  DashboardSummary,
  Device,
  LoginResponse,
  MapRaster,
  NotificationRecord,
  NotifyTestResult,
  NotifyWebhookStatus,
  MapRefs,
  MapVersion,
  Park,
  TeleopAxes,
  Route,
  RouteValidation,
  Setting,
  Task,
  Shift,
  ShiftCopyResult,
  ShiftSlot,
  ShiftStats,
  Staff,
  StaffImportResult,
  TaskEvent,
  UserInfo,
  WorkOrder,
  WorkOrderDetail,
  WorkOrderStats,
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
  /** 必须接管后才能下发摇杆与动作；`force` 用于遥控器断电但本体仍报 ctrl_source=3 */
  takeOver: (id: number, force = false) =>
    post<{
      deviceId: string;
      holder: string;
      expiresInSec: number;
      wsDelivered: boolean;
      forced: boolean;
      ctrlSource: number;
    }>(`/devices/${id}/teleop/take`, { force }),
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
  /** 设备事件流（最近 30 条）：客户端门禁拒绝只在这里能看到 */
  events: (id: number) => get<TaskEvent[]>(`/devices/${id}/events`),
  action: (id: number, action: string) =>
    post<{
      deviceId: string;
      action: string;
      label: string;
      wsDelivered: boolean;
      /** 急停 / 解除急停：服务端会把 1315 `estop.software` 回读结果校验一遍 */
      verify?: { field: string; expect: boolean; timeoutSec: number };
    }>(`/devices/${id}/action`, { action })
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
  dispatch: (id: number) => post<Task>(`/tasks/${id}/dispatch`),
  /**
   * 发布计划：之后才会到点自动执行（草稿不参与自动调度）。
   * 服务端会先校验时间窗 / 路线坐标，跑不起来的计划发布时就会报错。
   */
  publish: (id: number) => post<Task>(`/tasks/${id}/publish`),
  /** 撤回成草稿：停止自动下发（执行中 / 已暂停的要先暂停并取消） */
  unpublish: (id: number) => post<Task>(`/tasks/${id}/unpublish`)
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

export const notifyApi = {
  /** 推送记录：现场排查「告警到底推没推出去」 */
  records: (params?: object) => get<NotificationRecord[]>("/notifications", params),
  /** 同步发一条测试消息，返回企业微信的 errcode/errmsg 原文 */
  test: () => post<NotifyTestResult>("/notifications/test"),
  /** 当前 webhook 状态（来源 + 掩码地址）；`reveal` = 管理员取一次原文 */
  webhook: (reveal = false) =>
    get<NotifyWebhookStatus>("/notifications/webhook", reveal ? { reveal: true } : undefined),
  /** 保存 webhook 地址（立即生效，不用重启服务）；传空串 = 清除并回退环境变量 */
  saveWebhook: (webhook: string) =>
    put<NotifyWebhookStatus>("/notifications/webhook", { webhook })
};

export const dashboardApi = {
  summary: () => get<DashboardSummary>("/dashboard/summary")
};

/** 责任人库：排班与工单共用的处理人 */
/** 责任人「关联平台账号」下拉（只读账号元数据） */
export const accountApi = {
  list: () => get<AccountOption[]>("/accounts")
};

export const staffApi = {
  list: (params?: object) => get<Staff[]>("/staff", params),
  create: (data: object) => post<Staff>("/staff", data),
  update: (id: number, data: object) => put<Staff>(`/staff/${id}`, data),
  remove: (id: number) => del<boolean>(`/staff/${id}`),
  /** 下载 Excel 导入模板（.xlsx，含「填写说明」页） */
  downloadTemplate: () => download("/staff/import-template", "责任人导入模板.xlsx"),
  /** 上传填好的模板：逐行处理，返回新增 / 跳过 / 失败明细 */
  importExcel: (file: File) => {
    const form = new FormData();
    form.append("file", file);
    return upload<StaffImportResult>("/staff/import", form, 60000);
  }
};

/** 排班（计划层）：日期 + 班次 + 责任人 + 关联巡检计划 */
export const shiftApi = {
  list: (params?: object) => get<Shift[]>("/shifts", params),
  stats: (params?: object) => get<ShiftStats>("/shifts/stats", params),
  get: (id: number) => get<Shift>(`/shifts/${id}`),
  create: (data: object) => post<Shift>("/shifts", data),
  update: (id: number, data: object) => put<Shift>(`/shifts/${id}`, data),
  remove: (id: number) => del<boolean>(`/shifts/${id}`),
  /** 单条复制：周视图把卡片拖到目标日期 / 班次（other 天不用再填表单） */
  duplicate: (id: number, data: { shiftDate: string; slot?: string }) =>
    post<Shift>(`/shifts/${id}/duplicate`, data),
  /** 整段复制（上一周 / 上个月 → 目标区间） */
  copy: (data: object) => post<ShiftCopyResult>("/shifts/copy", data)
};

/** 班次设置：早 / 中 / 夜可改，也可自定义更多班次（名称 + 时段 + 顺序 + 停用） */
export const shiftSlotApi = {
  list: () => get<ShiftSlot[]>("/shift-slots"),
  create: (data: object) => post<ShiftSlot>("/shift-slots", data),
  update: (id: number, data: object) => put<ShiftSlot>(`/shift-slots/${id}`, data),
  remove: (id: number) => del<boolean>(`/shift-slots/${id}`)
};

/** 工单（执行层）：派发 → 接单 → 处理 → 验收 → 关闭 */
export const workOrderApi = {
  list: (params?: object) => get<WorkOrder[]>("/workorders", params),
  stats: (params?: object) => get<WorkOrderStats>("/workorders/stats", params),
  get: (id: number) => get<WorkOrderDetail>(`/workorders/${id}`),
  create: (data: object) => post<WorkOrderDetail>("/workorders", data),
  update: (id: number, data: object) => put<WorkOrderDetail>(`/workorders/${id}`, data),
  updateStatus: (id: number, data: { status: string; note?: string; actor?: string; progress?: number }) =>
    put<WorkOrderDetail>(`/workorders/${id}/status`, data),
  addLog: (id: number, content: string, actor?: string) =>
    post<WorkOrderDetail>(`/workorders/${id}/logs`, { content, actor }),
  remove: (id: number) => del<boolean>(`/workorders/${id}`)
};
