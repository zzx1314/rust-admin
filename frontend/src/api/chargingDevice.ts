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

const chargingDeviceUrls = {
  page: `/api/chargingDevice/getPage`,
  save: "/api/chargingDevice/create",
  delete: `/api/chargingDevice/`,
  update: "/api/chargingDevice/update"
};

// 分页
export const chargingDevicePage = (query?: object) => {
  return http.axiosGetRequest<ResultPage>(chargingDeviceUrls.page, query);
};
// 保存
export const chargingDeviceSave = (param?: object) => {
  return http.axiosPostRequest<Result>(chargingDeviceUrls.save, param);
};
// 修改
export const chargingDeviceUpdate = (param?: object) => {
  return http.axiosPut<Result>(chargingDeviceUrls.update, param);
};
// 删除
export const chargingDeviceDelete = (param?: object) => {
  return http.axiosDelete<Result>(chargingDeviceUrls.delete + param);
};
