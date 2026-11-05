import { useUserStoreHook } from "@/store/modules/user";

/**
 * 管理员角色 code（登录接口 permissions 字段即角色 code）。
 * 平台默认管理员角色 sysadm 的 code 为 "110"；如需扩展请在此追加。
 */
export const ADMIN_ROLE_CODES = ["110", "admin", "administrator", "role_admin"];

/** 当前登录用户是否为平台管理员。 */
export function isPlatformAdmin(): boolean {
  const permissions = useUserStoreHook().permissions ?? [];
  return permissions.some(code => ADMIN_ROLE_CODES.includes(code));
}
