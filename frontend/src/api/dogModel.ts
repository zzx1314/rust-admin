import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: Array<any> };
type ResultPage = {
  code: number;
  msg: string;
  data?: { records: Array<any>; total: number };
};

const dogModelUrls = {
  page: `/api/dogModel/getPage`,
  getAll: "/api/dogModel/getAll",
  save: "/api/dogModel",
  update: "/api/dogModel/",
  delete: `/api/dogModel/`
};

export const dogModelPage = (query?: object) =>
  http.axiosGetRequest<ResultPage>(dogModelUrls.page, query);
export const dogModelGetAll = () =>
  http.axiosGetRequest<Result>(dogModelUrls.getAll, {});
export const dogModelSave = (param?: object) =>
  http.axiosPostRequest<Result>(dogModelUrls.save, param);
export const dogModelUpdate = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogModelUrls.update + id, param);
export const dogModelDelete = (id: string | number) =>
  http.axiosDelete<Result>(dogModelUrls.delete + id);
