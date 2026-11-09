import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: any };
type ResultPage = {
  code: number;
  msg: string;
  data?: { records: Array<any>; total: number };
};

const dogMapUrls = {
  page: `/api/dogMap/getPage`,
  getAll: "/api/dogMap/getAll",
  byPark: "/api/dogMap/byPark/",
  save: "/api/dogMap",
  upload: "/api/dogMap/upload",
  update: "/api/dogMap/",
  image: "/api/dogMap/",
  publish: "/api/dogMap/",
  newVersion: "/api/dogMap/",
  refs: "/api/dogMap/",
  delete: `/api/dogMap/`
};

export const dogMapPage = (query?: object) =>
  http.axiosGetRequest<ResultPage>(dogMapUrls.page, query);
export const dogMapGetAll = () =>
  http.axiosGetRequest<Result>(dogMapUrls.getAll, {});
export const dogMapGetById = (id: string | number) =>
  http.axiosGetRequest<Result>(dogMapUrls.update + id, {});
/** 园区可用的地图基线版本，默认只返回已发布 / 部分标定的版本。 */
export const dogMapGetByPark = (
  parkId: string | number,
  publishableOnly = true
) =>
  http.axiosGetRequest<Result>(dogMapUrls.byPark + parkId, {
    publishableOnly
  });
export const dogMapSave = (param?: object) =>
  http.axiosPostRequest<Result>(dogMapUrls.save, param);
/** 上传地图包（.zip）+ 地图信息，创建或更新地图记录 */
export const dogMapUpload = (formData: FormData) =>
  http.uploadFile<Result>(dogMapUrls.upload, formData);
export const dogMapUpdate = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogMapUrls.update + id, param);
/** 发布地图基线：归档同园区旧版本，并把园区指针切到该版本。 */
export const dogMapPublish = (id: string | number) =>
  http.axiosPostRequest<Result>(dogMapUrls.publish + id + "/publish", {});
/** 派生并发布下一个不可变版本（历史路线保持旧版引用）。 */
export const dogMapNewVersion = (id: string | number) =>
  http.axiosPostRequest<Result>(dogMapUrls.newVersion + id + "/newVersion", {});
/** 引用该版本的路线 / 任务清单（发布新版本前的影响分析）。 */
export const dogMapRefs = (id: string | number) =>
  http.axiosGetRequest<Result>(dogMapUrls.refs + id + "/refs", {});

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
