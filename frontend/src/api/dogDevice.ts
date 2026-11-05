import { http } from "@/utils/http";

type Result = {
  code: number;
  msg: string;
  data?: Array<any>;
};

type ResultPage = {
  code: number;
  msg: string;
  data?: {
    records: Array<any>;
    total: number;
  };
};

const dogDeviceUrls = {
  page: `/api/dogDevice/getPage`,
  getAll: "/api/dogDevice/getAll",
  save: "/api/dogDevice",
  update: "/api/dogDevice/",
  delete: `/api/dogDevice/`,
  status: "/api/dogDevice/status/"
};

export const dogDevicePage = (query?: object) => {
  return http.axiosGetRequest<ResultPage>(dogDeviceUrls.page, query);
};
export const dogDeviceGetAll = () => {
  return http.axiosGetRequest<Result>(dogDeviceUrls.getAll, {});
};
export const dogDeviceSave = (param?: object) => {
  return http.axiosPostRequest<Result>(dogDeviceUrls.save, param);
};
export const dogDeviceUpdate = (id: string | number, param?: object) => {
  return http.axiosPut<Result>(dogDeviceUrls.update + id, param);
};
export const dogDeviceDelete = (id: string | number) => {
  return http.axiosDelete<Result>(dogDeviceUrls.delete + id);
};
export const dogDeviceUpdateStatus = (id: string | number, param?: object) => {
  return http.axiosPut<Result>(dogDeviceUrls.status + id, param);
};
