import { http } from "@/utils/http";

type Result = { code: number; msg: string; data?: any };

const dogGatewayUrls = {
  assign: "/api/dogGateway/assign"
};

/** 按硬件设备编号把设备分配给用户 */
export const dogGatewayAssign = (param?: object) =>
  http.axiosPostRequest<Result>(dogGatewayUrls.assign, param);

/** 查询设备已分配的用户 */
export const dogGatewayAssignees = (deviceId: string) =>
  http.axiosGetRequest<Result>(`${dogGatewayUrls.assign}/${deviceId}`, {});

/** 解除分配（管理员可传 userId 指定用户，否则解除自己） */
export const dogGatewayUnassign = (
  deviceId: string,
  userId?: string | number
) =>
  http.axiosDelete<Result>(
    `${dogGatewayUrls.assign}/${deviceId}` +
      (userId ? `?userId=${userId}` : ""),
    undefined
  );
