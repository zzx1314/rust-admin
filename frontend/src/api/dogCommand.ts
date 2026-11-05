import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: Array<any> };
type ResultPage = {
  code: number;
  msg: string;
  data?: { records: Array<any>; total: number };
};

const dogCommandUrls = {
  page: `/api/dogCommand/getPage`,
  getAll: "/api/dogCommand/getAll",
  save: "/api/dogCommand",
  update: "/api/dogCommand/",
  delete: `/api/dogCommand/`,
  status: "/api/dogCommand/status/"
};

export const dogCommandPage = (query?: object) =>
  http.axiosGetRequest<ResultPage>(dogCommandUrls.page, query);
export const dogCommandGetAll = () =>
  http.axiosGetRequest<Result>(dogCommandUrls.getAll, {});
export const dogCommandSave = (param?: object) =>
  http.axiosPostRequest<Result>(dogCommandUrls.save, param);
export const dogCommandUpdate = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogCommandUrls.update + id, param);
export const dogCommandDelete = (id: string | number) =>
  http.axiosDelete<Result>(dogCommandUrls.delete + id);
export const dogCommandUpdateStatus = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogCommandUrls.status + id, param);
