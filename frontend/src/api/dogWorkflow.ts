import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: Array<any> };

const dogWorkflowUrls = {
  save: "/api/dogWorkflow",
  update: "/api/dogWorkflow/",
  delete: `/api/dogWorkflow/`,
  uuid: "/api/dogWorkflow/uuid/"
};

export const dogWorkflowSave = (param?: object) =>
  http.axiosPostRequest<Result>(dogWorkflowUrls.save, param);
export const dogWorkflowUpdate = (id: string | number, param?: object) =>
  http.axiosPut<Result>(dogWorkflowUrls.update + id, param);
export const dogWorkflowDelete = (id: string | number) =>
  http.axiosDelete<Result>(dogWorkflowUrls.delete + id);
export const dogWorkflowGetByUuid = (uuid: string) =>
  http.axiosGetRequest<Result>(dogWorkflowUrls.uuid + uuid, {});
