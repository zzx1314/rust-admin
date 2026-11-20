import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: Array<any> };
type ResultPage = {
  code: number;
  msg: string;
  data?: { records: Array<any>; total: number };
};

const dogMediaUrls = {
  page: `/api/dogMedia/getPage`,
  getAll: "/api/dogMedia/getAll",
  save: "/api/dogMedia",
  update: "/api/dogMedia/",
  delete: `/api/dogMedia/`,
  // 训练数据下载：单张（attachment）与批量 zip
  file: "/api/dogMedia/",
  export: "/api/dogMedia/export"
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
/** 触发浏览器保存一个 blob（训练数据下载用）。 */
function saveBlob(blob: Blob, filename: string) {
  const url = window.URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);
  window.URL.revokeObjectURL(url);
}

function timestamp(): string {
  const now = new Date();
  const pad = (value: number) => String(value).padStart(2, "0");
  return (
    `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}` +
    `-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`
  );
}

/** 下载单张原图（服务端返回 attachment 原始字节）。 */
export const dogMediaDownload = async (id: string | number) => {
  const blob = await http.axiosGetDown<Blob>(
    dogMediaUrls.file + id + "/file",
    ""
  );
  saveBlob(new Blob([blob]), `media-${id}.jpg`);
};

/**
 * 批量下载：勾选 `ids` 优先，否则按当前筛选条件导出（服务端打包 zip，
 * 包内是原图 + manifest.json；上限 200 张 / 100MB）。
 */
export const dogMediaExport = async (params: Record<string, unknown>) => {
  const search = new URLSearchParams();
  Object.entries(params).forEach(([key, value]) => {
    if (value === undefined || value === null || value === "") return;
    search.set(key, String(value));
  });
  const query = search.toString();
  const url = `${dogMediaUrls.export}${query ? `?${query}` : ""}`;
  const blob = await http.axiosGetDown<Blob>(url, "");
  saveBlob(
    new Blob([blob], { type: "application/zip" }),
    `media-${timestamp()}.zip`
  );
};
