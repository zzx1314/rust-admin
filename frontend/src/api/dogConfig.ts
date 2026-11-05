import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: Array<any> };
type ResultPage = {
  code: number;
  msg: string;
  data?: { records: Array<any>; total: number };
};

const dogConfigUrls = {
  page: `/api/dogConfig/getPage`,
  getAll: "/api/dogConfig/getAll",
  save: "/api/dogConfig",
  update: "/api/dogConfig/",
  delete: `/api/dogConfig/`
};

export const dogConfigPage = (query?: object) =>
  http.axiosGetRequest<ResultPage>(dogConfigUrls.page, query);
export const dogConfigGetAll = () =>
  http.axiosGetRequest<Result>(dogConfigUrls.getAll, {});
export const dogConfigSave = (param?: object) =>
  http.axiosPostRequest<Result>(dogConfigUrls.save, param);
export const dogConfigUpdate = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogConfigUrls.update + id, param);
export const dogConfigDelete = (id: string | number) =>
  http.axiosDelete<Result>(dogConfigUrls.delete + id);
