import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: Array<any> };
type ResultPage = {
  code: number;
  msg: string;
  data?: { records: Array<any>; total: number };
};

const dogRouteUrls = {
  page: `/api/dogRoute/getPage`,
  getAll: "/api/dogRoute/getAll",
  save: "/api/dogRoute",
  update: "/api/dogRoute/",
  delete: `/api/dogRoute/`
};
export const dogRoutePage = (query?: object) =>
  http.axiosGetRequest<ResultPage>(dogRouteUrls.page, query);
export const dogRouteGetAll = () =>
  http.axiosGetRequest<Result>(dogRouteUrls.getAll, {});
export const dogRouteGetById = (id: string | number) =>
  http.axiosGetRequest<{ code: number; msg: string; data?: any }>(
    dogRouteUrls.delete + id,
    {}
  );
export const dogRouteSave = (param?: object) =>
  http.axiosPostRequest<Result>(dogRouteUrls.save, param);
export const dogRouteUpdate = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogRouteUrls.update + id, param);
export const dogRouteDelete = (id: string | number) =>
  http.axiosDelete<Result>(dogRouteUrls.delete + id);
