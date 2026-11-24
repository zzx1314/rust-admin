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
  push: "/api/dogMap/",
  shares: "/api/dogMap/",
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
/** 发布地图版本：状态推进为已发布并记录发布时间（草稿 / 部分标定都变已发布）。 */
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
  /**
   * 本笔画写入的灰度值（254 = 可通行、205 = 未知）。
   * 一次编辑里可以既擦成可通行又擦成未知，所以值跟着笔画走；
   * 省略时后端回退到请求体的 `eraseValue`（老客户端）。
   */
  value?: number;
};

/**
 * 保存橡皮擦编辑：后端改写包内编辑版栅格（优先 `map_edit/map.pgm`，没有时才用
 * `model.pgm` / `map.pgm`）并重算 `map_hash`，根目录原始地图不动。
 *
 * `eraseValue`：254 = 可通行（默认，擦掉噪点），205 = 未知；只作兜底——
 * 笔画自带 `value` 时以笔画为准（同一次编辑允许两种值混用）。
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
 * 链路：中台 → 业务平台 → 设备客户端 → 本体文件服务（有 map_edit/ 时写
 * map_edit/map.pgm + map_edit/map.yaml，否则写根目录 map.pgm / map.yaml）
 * → 重新载图 → 设备回 `commandResult`。中台会等设备真实执行结果（最长 60s）
 * 后才返回：急停、任务占用、写文件失败、未回执都会返回错误并带设备侧原因。
 */
export const dogMapPushToDevice = (id: string | number, deviceCode?: string) =>
  http.axiosPostRequest<Result>(`${dogMapUrls.push}${id}/push`, {
    deviceCode: deviceCode || undefined
  });

/** 一条地图分享关系（`deviceCode` 是分享了这张图的其他设备） */
export type MapShare = {
  mapId: number;
  deviceCode: string;
  createTime: string;
};

/** 这张地图当前分享给了哪些设备。 */
export const dogMapListShares = (id: string | number) =>
  http.axiosGetRequest<Result>(`${dogMapUrls.shares}${id}/shares`, {});

/**
 * 把地图分享给其他设备使用（不复制地图包，也不用重新建图）。
 *
 * `mode = replace` 以本次提交为准（空数组 = 清空分享）；`append` 只追加。
 * 分享后：分配给目标设备的中台用户也能看到这张图，「下发到狗体」可选该设备。
 */
export const dogMapShareDevices = (
  id: string | number,
  deviceCodes: string[],
  mode: "append" | "replace" = "replace"
) =>
  http.axiosPostRequest<Result>(`${dogMapUrls.shares}${id}/share`, {
    deviceCodes,
    mode
  });

/** 取消分享给单台设备（幂等）。 */
export const dogMapUnshareDevice = (id: string | number, deviceCode: string) =>
  http.axiosDelete<Result>(
    `${dogMapUrls.shares}${id}/share/${encodeURIComponent(deviceCode)}`
  );

/** 获取地图图片（后端优先把包内 map_edit/map.pgm 转成 PNG） */
export const dogMapGetImage = (id: string | number) =>
  http.axiosGetRequest<MapImageResult>(dogMapUrls.image + id + "/image", {});
export const dogMapDelete = (id: string | number) =>
  http.axiosDelete<Result>(dogMapUrls.delete + id);
