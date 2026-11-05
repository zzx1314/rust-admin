import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: Array<any> };
type ResultPage = {
  code: number;
  msg: string;
  data?: { records: Array<any>; total: number };
};

const dogParkUrls = {
  page: `/api/dogPark/getPage`,
  getAll: "/api/dogPark/getAll",
  save: "/api/dogPark",
  update: "/api/dogPark/",
  delete: `/api/dogPark/`
};

export const dogParkPage = (query?: object) =>
  http.axiosGetRequest<ResultPage>(dogParkUrls.page, query);
export const dogParkGetAll = () =>
  http.axiosGetRequest<Result>(dogParkUrls.getAll, {});
export const dogParkSave = (param?: object) =>
  http.axiosPostRequest<Result>(dogParkUrls.save, param);
export const dogParkUpdate = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogParkUrls.update + id, param);
export const dogParkDelete = (id: string | number) =>
  http.axiosDelete<Result>(dogParkUrls.delete + id);
