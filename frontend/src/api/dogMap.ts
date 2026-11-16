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
  save: "/api/dogMap",
  upload: "/api/dogMap/upload",
  update: "/api/dogMap/",
  image: "/api/dogMap/",
  edit: "/api/dogMap/",
  publish: "/api/dogMap/",
  newVersion: "/api/dogMap/",
  delete: `/api/dogMap/`
};

export const dogMapPage = (query?: object) =>
  http.axiosGetRequest<ResultPage>(dogMapUrls.page, query);
export const dogMapGetAll = () =>
  http.axiosGetRequest<Result>(dogMapUrls.getAll, {});
export const dogMapGetById = (id: string | number) =>
  http.axiosGetRequest<Result>(dogMapUrls.update + id, {});
export const dogMapSave = (param?: object) =>
  http.axiosPostRequest<Result>(dogMapUrls.save, param);
/** 上传地图包（.zip）+ 地图信息，创建或更新地图记录 */
export const dogMapUpload = (formData: FormData) =>
  http.uploadFile<Result>(dogMapUrls.upload, formData);
export const dogMapUpdate = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogMapUrls.update + id, param);
/** 发布地图版本：状态推进为已发布 / 部分标定并记录发布时间。 */
export const dogMapPublish = (id: string | number) =>
  http.axiosPostRequest<Result>(dogMapUrls.publish + id + "/publish", {});
/** 派生并发布下一个不可变版本。 */
export const dogMapNewVersion = (id: string | number) =>
  http.axiosPostRequest<Result>(dogMapUrls.newVersion + id + "/newVersion", {});
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
    /** 编辑接口返回：改写后的地图包 SHA-256 */
    mapHash?: string;
    /** 编辑接口返回：本次实际改写的像素数 */
    erasedPixels?: number;
  };
};

/** 橡皮擦笔画：坐标是栅格像素坐标（左上角原点，x 向右、y 向下）。 */
export type MapEditStroke = {
  points: Array<{ x: number; y: number }>;
  radius?: number;
};

/**
 * 保存橡皮擦编辑：后端改写地图包内 `model.pgm` 并重算 `map_hash`。
 *
 * `eraseValue`：254 = 可通行（默认，擦掉噪点），205 = 未知。
 * 只有草稿 / 部分标定版本可编辑，已发布版本需先派生新版本。
 */
export const dogMapErase = (
  id: string | number,
  strokes: MapEditStroke[],
  eraseValue = 254
) =>
  http.axiosPostRequest<MapImageResult>(`${dogMapUrls.edit}${id}/edit`, {
    strokes,
    eraseValue
  });

/**
 * 把编辑后的地图下发到狗体（本体）。
 *
 * 链路：中台 → 业务平台 → 设备客户端 → 本体文件服务（写回 map.pgm / map.yaml）
 * → 重新载图。设备未连接、正在执行任务、或本体上没有该地图目录时会报错。
 */
export const dogMapPushToDevice = (id: string | number, deviceCode?: string) =>
  http.axiosPostRequest<Result>(`${dogMapUrls.edit}${id}/push`, {
    deviceCode: deviceCode || undefined
  });

/** 获取地图图片（后端把包内 model.pgm 转成 PNG） */
export const dogMapGetImage = (id: string | number) =>
  http.axiosGetRequest<MapImageResult>(dogMapUrls.image + id + "/image", {});
export const dogMapDelete = (id: string | number) =>
  http.axiosDelete<Result>(dogMapUrls.delete + id);
