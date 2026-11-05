import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: Array<any> };
type ResultPage = {
  code: number;
  msg: string;
  data?: { records: Array<any>; total: number };
};

const dogTaskUrls = {
  page: `/api/dogTask/getPage`,
  getAll: "/api/dogTask/getAll",
  save: "/api/dogTask",
  update: "/api/dogTask/",
  delete: `/api/dogTask/`,
  status: "/api/dogTask/status/"
};

export const dogTaskPage = (query?: object) =>
  http.axiosGetRequest<ResultPage>(dogTaskUrls.page, query);
export const dogTaskGetAll = () =>
  http.axiosGetRequest<Result>(dogTaskUrls.getAll, {});
export const dogTaskSave = (param?: object) =>
  http.axiosPostRequest<Result>(dogTaskUrls.save, param);
export const dogTaskUpdate = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogTaskUrls.update + id, param);
export const dogTaskDelete = (id: string | number) =>
  http.axiosDelete<Result>(dogTaskUrls.delete + id);
export const dogTaskUpdateStatus = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogTaskUrls.status + id, param);
