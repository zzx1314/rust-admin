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
  restore: "/api/dogMap/",
  forbidden: "/api/dogMap/",
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
    /** 这张图当前的禁止区域（`map_edit/edit_info.json` 的 forbidden_areas） */
    forbiddenAreas?: MapForbiddenArea[];
  };
};

/**
 * 禁止区域：轴对齐 / 可旋转矩形，坐标与边长都是**栅格像素**。
 *
 * 遥控器在栅格上把它刷成 `0`（占据），导航规划自然绕开；`edit_info.json` 里
 * 同步记 `forbidden_areas` + 一条 `upsert_forbidden` 动作，遥控器保存时按
 * 「根图 + 这些动作」重算栅格，所以平台改它必须连编辑记录一起改。
 */
export type MapForbiddenArea = {
  /** 秒；新增时省略（后端按毫秒时间戳生成，与遥控器同一套规则） */
  id?: number;
  center: { x: number; y: number };
  width: number;
  height: number;
  /** 绕 center 旋转，按弧度（遥控器样本里都是 0） */
  rotation?: number;
  kind?: string;
};

/**
 * 设置禁止区域（**整表替换**：传进来的列表就是最终状态，不在列表里的会被撤销
 * 并按建图原图还原那一块栅格）。只有草稿 / 部分标定可改。
 */
export const dogMapSetForbiddenAreas = (
  id: string | number,
  areas: MapForbiddenArea[]
) =>
  http.axiosPostRequest<MapImageResult>(
    `${dogMapUrls.forbidden}${id}/forbidden`,
    { areas }
  );

/** 橡皮擦笔画：坐标是栅格像素坐标（左上角原点，x 向右、y 向下）。 */
export type MapEditStroke = {
  points: Array<{ x: number; y: number }>;
  radius?: number;
  /**
   * 本笔画写入的灰度值（254 = 可通行、128 = 未知）。
   * 平台前端已不再提供「擦成未知」，只写 254（与遥控器一致）；字段保留给
   * 历史客户端 / 脚本，省略时后端回退到请求体的 `eraseValue`。
   *
   * 旧的 205 仍能提交，但后端会统一改写成 128：本体按 `map.yaml` 的
   * `free_thresh: 0.2` 会把 205 当成可通行，不能再用它表示「未知/禁行」。
   */
  value?: number;
};

/**
 * 保存橡皮擦编辑：后端改写包内编辑版栅格（优先 `map_edit/map.pgm`，没有时才用
 * `model.pgm` / `map.pgm`）并重算 `map_hash`，根目录原始地图不动。
 *
 * `eraseValue`：254 = 可通行（默认，擦掉噪点），128 = 未知/不可通行。
 * 平台页面只传 254（与遥控器一致，遥控器橡皮擦只写 254）；`128` / 旧 `205`
 * 仅为历史客户端保留，笔画自带 `value` 时以笔画为准。
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
 * 恢复原图：把编辑版栅格换回**建图原图**，并清空遥控器的编辑记录
 * （`map_edit/edit_info.json`）——不清空的话遥控器下次保存会按旧擦除圆把图又画回去。
 *
 * 只改平台侧的地图包；要写回本体还得点「下发到狗体」。
 * 与橡皮擦同一套状态约束：只有草稿 / 部分标定可恢复，已发布请先派生新版本。
 */
export const dogMapRestoreOriginal = (id: string | number) =>
  http.axiosPostRequest<MapImageResult>(
    `${dogMapUrls.restore}${id}/restore`,
    {}
  );

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
