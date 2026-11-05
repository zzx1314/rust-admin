import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: Array<any> };
type ResultPage = {
  code: number;
  msg: string;
  data?: { records: Array<any>; total: number };
};

const dogMediaUrls = {
  page: `/api/dogMedia/getPage`,
  getAll: "/api/dogMedia/getAll",
  save: "/api/dogMedia",
  update: "/api/dogMedia/",
  delete: `/api/dogMedia/`
};

export const dogMediaPage = (query?: object) =>
  http.axiosGetRequest<ResultPage>(dogMediaUrls.page, query);
export const dogMediaGetAll = () =>
  http.axiosGetRequest<Result>(dogMediaUrls.getAll, {});
export const dogMediaSave = (param?: object) =>
  http.axiosPostRequest<Result>(dogMediaUrls.save, param);
export const dogMediaUpdate = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogMediaUrls.update + id, param);
export const dogMediaDelete = (id: string | number) =>
  http.axiosDelete<Result>(dogMediaUrls.delete + id);
