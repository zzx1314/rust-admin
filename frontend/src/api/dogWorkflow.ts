import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: any };

const urls = {
  base: "/api/dogWorkflow",
  uuid: "/api/dogWorkflow/uuid/"
};

export const dogWorkflowCreate = (param?: object) =>
  http.axiosPostRequest<Result>(urls.base, param);

export const dogWorkflowGetById = (id: number) =>
  http.axiosGetRequest<Result>(`${urls.base}/${id}`, {});

export const dogWorkflowGetByUuid = (uuid: string) =>
  http.axiosGetRequest<Result>(`${urls.uuid}${uuid}`, {});

export const dogWorkflowUpdate = (id: number, param?: object) =>
  http.axiosPut<Result>(`${urls.base}/${id}`, param);

export const dogWorkflowDelete = (id: number) =>
  http.axiosDelete<Result>(`${urls.base}/${id}`);
