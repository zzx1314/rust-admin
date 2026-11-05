import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: Array<any> };
type ResultPage = {
  code: number;
  msg: string;
  data?: { records: Array<any>; total: number };
};

const dogUserDeviceUrls = {
  page: `/api/dogUserDevice/getPage`,
  getAll: "/api/dogUserDevice/getAll",
  save: "/api/dogUserDevice",
  delete: `/api/dogUserDevice/`,
  byUser: "/api/dogUserDevice/byUser/"
};

export const dogUserDevicePage = (query?: object) =>
  http.axiosGetRequest<ResultPage>(dogUserDeviceUrls.page, query);
export const dogUserDeviceGetAll = () =>
  http.axiosGetRequest<Result>(dogUserDeviceUrls.getAll, {});
export const dogUserDeviceSave = (param?: object) =>
  http.axiosPostRequest<Result>(dogUserDeviceUrls.save, param);
export const dogUserDeviceDelete = (id: string | number) =>
  http.axiosDelete<Result>(dogUserDeviceUrls.delete + id);
export const dogUserDeviceByUser = (userId: string | number) =>
  http.axiosGetRequest<Result>(dogUserDeviceUrls.byUser + userId, {});
