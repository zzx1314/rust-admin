import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: Array<any> };
type ResultPage = {
  code: number;
  msg: string;
  data?: { records: Array<any>; total: number };
};

const dogMapUrls = {
  page: `/api/dogMap/getPage`,
  getAll: "/api/dogMap/getAll",
  save: "/api/dogMap",
  upload: "/api/dogMap/upload",
  update: "/api/dogMap/",
  image: "/api/dogMap/",
  delete: `/api/dogMap/`
};

export const dogMapPage = (query?: object) =>
  http.axiosGetRequest<ResultPage>(dogMapUrls.page, query);
export const dogMapGetAll = () =>
  http.axiosGetRequest<Result>(dogMapUrls.getAll, {});
export const dogMapSave = (param?: object) =>
  http.axiosPostRequest<Result>(dogMapUrls.save, param);
/** 上传地图包（.zip）+ 地图信息，创建或更新地图记录 */
export const dogMapUpload = (formData: FormData) =>
  http.uploadFile<Result>(dogMapUrls.upload, formData);
export const dogMapUpdate = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogMapUrls.update + id, param);

/** 地图包渲染结果：PNG base64 + 栅格分辨率 / 原点（与 map_server 一致） */
export type MapImageResult = {
  code: number;
  msg: string;
  data?: {
    image: string;
    resolution: number;
    origin: number[];
    width: number;
    height: number;
  };
};

/** 获取地图图片（后端把包内 model.pgm 转成 PNG） */
export const dogMapGetImage = (id: string | number) =>
  http.axiosGetRequest<MapImageResult>(dogMapUrls.image + id + "/image", {});
export const dogMapDelete = (id: string | number) =>
  http.axiosDelete<Result>(dogMapUrls.delete + id);
