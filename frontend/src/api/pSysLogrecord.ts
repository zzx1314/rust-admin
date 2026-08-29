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

const pSysLogrecordUrls = {
  page: `/api/sysLog/getPage`,
  save: "/api/sysLog/save",
  delete: `/api/sysLog/`,
  update: "/api/sysLog/"
};

// 分页
export const pSysLogrecordPage = (query?: object) => {
  return http.axiosGetRequest<ResultPage>(pSysLogrecordUrls.page, query);
};
// 保存
export const pSysLogrecordSave = (param?: object) => {
  return http.axiosPostRequest<Result>(pSysLogrecordUrls.save, param);
};
// 修改
export const pSysLogrecordUpdate = (param: {
  id: string | number;
  [key: string]: unknown;
}) => {
  return http.axiosPut<Result>(pSysLogrecordUrls.update + param.id, param);
};
// 删除
export const pSysLogrecordDelete = (param?: object) => {
  return http.axiosDelete<Result>(pSysLogrecordUrls.delete + param);
};
