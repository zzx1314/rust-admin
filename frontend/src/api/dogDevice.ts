import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: any };
type ResultPage = {
  code: number;
  msg: string;
  data?: { records: Array<any>; total: number };
};

export type DogDeviceUser = {
  id: number;
  username: string;
  realName?: string | null;
};

export type DogDevice = {
  id: number;
  deviceId: string;
  deviceName?: string | null;
  model?: string | null;
  status: string;
  firmware?: string | null;
  deviceIp?: string | null;
  parkCode?: string | null;
  parkName?: string | null;
  remarks?: string | null;
  userId?: number | null;
  users: DogDeviceUser[];
  userIds: number[];
  createTime: string;
  updateTime?: string | null;
};

const dogDeviceUrls = {
  page: "/api/dogDevice/getPage",
  getAll: "/api/dogDevice/getAll",
  users: "/api/dogDevice/users",
  assign: "/api/dogDevice/assignUsers",
  unassign: "/api/dogDevice/unassignUsers",
  save: "/api/dogDevice",
  upsert: "/api/dogDevice/upsert",
  update: "/api/dogDevice/",
  delete: "/api/dogDevice/"
};

export const dogDevicePage = (query?: object) =>
  http.axiosGetRequest<ResultPage>(dogDeviceUrls.page, query);

export const dogDeviceGetAll = (query?: object) =>
  http.axiosGetRequest<Result>(dogDeviceUrls.getAll, query);

export const dogDeviceSave = (param?: object) =>
  http.axiosPostRequest<Result>(dogDeviceUrls.save, param);

export const dogDeviceUpsert = (param?: object) =>
  http.axiosPostRequest<Result>(dogDeviceUrls.upsert, param);

export const dogDeviceUpdate = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogDeviceUrls.update + id, param);

export const dogDeviceDelete = (id: string | number) =>
  http.axiosDelete<Result>(dogDeviceUrls.delete + id);

/** 可分配的中台用户 */
export const dogDeviceUsers = () =>
  http.axiosGetRequest<Result>(dogDeviceUrls.users, {});

/** 批量分配：mode=append 追加，mode=replace 覆盖 */
export const dogDeviceAssign = (
  deviceIds: number[],
  userIds: number[],
  mode: "append" | "replace" = "append"
) =>
  http.axiosPostRequest<Result>(dogDeviceUrls.assign, {
    deviceIds,
    userIds,
    mode
  });

/** 批量解除分配 */
export const dogDeviceUnassign = (deviceIds: number[], userIds: number[]) =>
  http.axiosPostRequest<Result>(dogDeviceUrls.unassign, {
    deviceIds,
    userIds
  });

/** 中台手动触发设备同步地图（经业务平台下发 `同步地图` 指令） */
export const dogDeviceSyncMaps = (id: string | number) =>
  http.axiosPostRequest<Result>(`/api/dogDevice/${id}/syncMaps`, {});

/**
 * 中台向设备下发一条指令。
 *
 * 中台不直连设备：指令会转发给业务平台，由业务平台的设备网关落地。
 * `requestId` 是幂等键，重试时要原样回传，设备便不会收到重复指令。
 */
export const dogDeviceCommand = (
  id: string | number,
  command: string,
  requestId?: string,
  params?: Record<string, any>
) =>
  http.axiosPostRequest<Result>(`/api/dogDevice/${id}/command`, {
    command,
    requestId,
    params
  });

/**
 * 中台给指定设备下发一条巡检任务。
 *
 * 航点/航段/动作由业务平台的巡检路线维护，中台只选择路线与计划时间。
 */
export const dogDeviceDispatchTask = (
  id: string | number,
  param: {
    taskName: string;
    routeId: number;
    planStart?: string | null;
    planEnd?: string | null;
    executeMode?: string | null;
    level?: string | null;
    description?: string | null;
    requestId?: string;
  }
) => http.axiosPostRequest<Result>(`/api/dogDevice/${id}/tasks`, param);

/** 设备运行态（在线/任务/进度/电量），只读 */
export const dogDeviceRuntime = (id: string | number) =>
  http.axiosGetRequest<Result>(`/api/dogDevice/${id}/runtime`, {});
