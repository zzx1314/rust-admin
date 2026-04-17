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

const pSysMessageUrls = {
  page: `/api/pSysMessage/page`,
  save: "/api/pSysMessage/save",
  delete: `/api/pSysMessage/`,
  update: "/api/pSysMessage/update",
  list: "/api/pSysMessage/list",
  readMessage: "/api/pSysMessage/readMessage/"
};

// 系统消息分页
export const pSysMessagePage = (query?: object) => {
  return http.axiosGetRequest<ResultPage>(pSysMessageUrls.page, query);
};
// 系统消息列表
export const pSysMessageList = (query?: object) => {
  return http.axiosGetRequest<Result>(pSysMessageUrls.list, query);
};
// 系统消息已读
export const pSysMessageReadMessage = (param?: number, data?: object) => {
  return http.axiosPostRequest<Result>(
    pSysMessageUrls.readMessage + param,
    data
  );
};
// 系统消息保存
export const pSysMessageSave = (param?: object) => {
  return http.axiosPostRequest<Result>(pSysMessageUrls.save, param);
};
// 系统消息修改
export const pSysMessageUpdate = (param?: object) => {
  return http.axiosPut<Result>(pSysMessageUrls.update, param);
};
// 系统消息删除
export const pSysMessageDelete = (param?: object) => {
  return http.axiosDelete<Result>(pSysMessageUrls.delete + param);
};
