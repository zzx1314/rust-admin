import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: Array<any> };
type ResultPage = {
  code: number;
  msg: string;
  data?: { records: Array<any>; total: number };
};
type ResultData<T> = { code: number; msg: string; data?: T };

const dogMediaUrls = {
  page: `/api/dogMedia/getPage`,
  getAll: "/api/dogMedia/getAll",
  save: "/api/dogMedia",
  update: "/api/dogMedia/",
  delete: `/api/dogMedia/`,
  // 图片内容（data URL）：与地图预览同一套约定
  image: "/api/dogMedia/"
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
export const dogMediaImage = (id: string | number) =>
  http.axiosGetRequest<ResultData<MediaImageResult>>(
    dogMediaUrls.image + id + "/image",
    {}
  );

export interface MediaImageResult {
  id: number;
  mediaName: string;
  contentType: string;
  size: number;
  /** `data:image/jpeg;base64,...`，直接给 <img src> */
  image: string;
}
