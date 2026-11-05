import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: Array<any> };

const dogServicesUrls = {
  getAll: "/api/dogServices/getAll",
  save: "/api/dogServices",
  update: "/api/dogServices/",
  delete: `/api/dogServices/`
};

export const dogServicesGetAll = () =>
  http.axiosGetRequest<Result>(dogServicesUrls.getAll, {});
export const dogServicesSave = (param?: object) =>
  http.axiosPostRequest<Result>(dogServicesUrls.save, param);
export const dogServicesUpdate = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogServicesUrls.update + id, param);
export const dogServicesDelete = (id: string | number) =>
  http.axiosDelete<Result>(dogServicesUrls.delete + id);
