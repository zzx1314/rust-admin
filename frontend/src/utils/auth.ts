import Cookies from "js-cookie";
import { useUserStoreHook } from "@/store/modules/user";
import {
  storageSession,
  isString,
  isIncludeAllChildren
} from "@pureadmin/utils";

export interface DataInfo<T> {
  /** token */
  accessToken: string;
  /** `accessToken`的过期时间（时间戳） */
  expires: T;
  /** 用于调用刷新accessToken的接口时所需的token */
  refreshToken: string;
  /** 头像 */
  avatar?: string;
  /** 用户名 */
  username?: string;
  /** 昵称 */
  nickname?: string;
  /** 当前登录用户的角色 */
  roles?: Array<number>;
  /** 当前登录用户的按钮级别权限 */
  permissions?: Array<string>;
  /** 用户id */
  user_id?: string | number;
}

export const userKey = "user-info";
export const TokenKey = "authorized-token";
/**
 * 通过`multiple-tabs`是否在`cookie`中，判断用户是否已经登录系统，
 * 从而支持多标签页打开已经登录的系统后无需再登录。
 * 浏览器完全关闭后`multiple-tabs`将自动从`cookie`中销毁，
 * 再次打开浏览器需要重新登录系统
 * */
export const multipleTabsKey = "multiple-tabs";

/** 获取`token` */
export function getToken(): DataInfo<number> {
  // 此处与`TokenKey`相同，此写法解决初始化时`Cookies`中不存在`TokenKey`报错
  return storageSession().getItem(userKey);
}

/**
 * 后端返回的过期时间形如 `2026-11-22 10:00:00`（北京时间、没有时区后缀）。
 * Safari 不认这种格式的 `new Date()`，会得到 `NaN`，进而让 cookie 退化成「关浏览器
 * 就没了」的会话 cookie，所以这里手动拆字段。解析不出来时返回 0（= 会话 cookie）。
 */
function expiresToMillis(value: number | string | Date): number {
  if (typeof value === "number") return value;
  if (value instanceof Date) return value.getTime();
  const matched = /^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2}):(\d{2})/.exec(
    String(value).trim()
  );
  if (matched) {
    const [, year, month, day, hour, minute, second] = matched;
    // 后端固定用北京时间（UTC+8）格式化，显式带时区才不会受浏览器所在时区影响
    const parsed = Date.parse(
      `${year}-${month}-${day}T${hour}:${minute}:${second}+08:00`
    );
    if (!Number.isNaN(parsed)) return parsed;
    // 兜底：按本地时区拆字段（极老的浏览器不认带偏移的 ISO 8601）
    return new Date(
      Number(year),
      Number(month) - 1,
      Number(day),
      Number(hour),
      Number(minute),
      Number(second)
    ).getTime();
  }
  const parsed = new Date(value).getTime();
  return Number.isNaN(parsed) ? 0 : parsed;
}

/**
 * 无感刷新之后同步本地令牌（`sessionStorage` 的 `user-info` + `TokenKey` cookie）。
 * 只改令牌相关字段，不碰头像/角色等用户信息；`expiresIn` 单位为秒。
 *
 * 返回新的过期时间戳（毫秒）。
 */
export function updateTokenSilently(data: {
  accessToken: string;
  refreshToken: string;
  expiresIn: number;
}): number {
  const newExpires = Date.now() + Math.max(0, data.expiresIn) * 1000;
  const userInfo = storageSession().getItem<DataInfo<number>>(userKey);
  if (userInfo) {
    storageSession().setItem(userKey, {
      ...userInfo,
      accessToken: data.accessToken,
      refreshToken: data.refreshToken,
      expires: newExpires
    });
  }
  const cookieString = JSON.stringify({
    accessToken: data.accessToken,
    expires: newExpires,
    refreshToken: data.refreshToken
  });
  Cookies.set(
    TokenKey,
    cookieString,
    data.expiresIn > 0 ? { expires: data.expiresIn / 86400 } : undefined
  );
  return newExpires;
}

/**
 * @description 设置`token`以及一些必要信息并采用无感刷新`token`方案
 * 无感刷新：后端返回`accessToken`（访问接口使用的`token`）、`refreshToken`（用于调用刷新`accessToken`的接口时所需的`token`，`refreshToken`的过期时间（比如30天）应大于`accessToken`的过期时间（比如2小时））、`expires`（`accessToken`的过期时间）
 * 将`accessToken`、`expires`、`refreshToken`这三条信息放在key值为authorized-token的cookie里（过期自动销毁）
 * 将`avatar`、`username`、`nickname`、`roles`、`permissions`、`refreshToken`、`expires`这七条信息放在key值为`user-info`的localStorage里（利用`multipleTabsKey`当浏览器完全关闭后自动销毁）
 */
export function setToken(data: DataInfo<Date | string>) {
  let expires = 0;
  const { accessToken, refreshToken } = data;
  const { isRemembered, loginDay } = useUserStoreHook();
  expires = expiresToMillis(data.expires as number | string | Date); // 后端返回 "YYYY-MM-DD HH:mm:ss"（北京时间）
  const cookieString = JSON.stringify({ accessToken, expires, refreshToken });

  expires > 0
    ? Cookies.set(TokenKey, cookieString, {
        expires: (expires - Date.now()) / 86400000
      })
    : Cookies.set(TokenKey, cookieString);

  Cookies.set(
    multipleTabsKey,
    "true",
    isRemembered
      ? {
          expires: loginDay
        }
      : {}
  );

  function setUserKey({ avatar, username, nickname, roles, permissions }) {
    useUserStoreHook().SET_AVATAR(avatar);
    useUserStoreHook().SET_USERNAME(username);
    useUserStoreHook().SET_NICKNAME(nickname);
    useUserStoreHook().SET_ROLES(roles);
    useUserStoreHook().SET_PERMS(permissions);
    storageSession().setItem(userKey, {
      accessToken,
      refreshToken,
      expires,
      avatar,
      username,
      nickname,
      roles,
      permissions
    });
    console.log("storageSession", storageSession().getItem(userKey));
  }

  if (data.username && data.roles) {
    const { username, roles } = data;
    setUserKey({
      avatar: data?.avatar ?? "",
      username,
      nickname: data?.nickname ?? "",
      roles,
      permissions: data?.permissions ?? []
    });
  } else {
    const avatar =
      storageSession().getItem<DataInfo<number>>(userKey)?.avatar ?? "";
    const username =
      storageSession().getItem<DataInfo<number>>(userKey)?.username ?? "";
    const nickname =
      storageSession().getItem<DataInfo<number>>(userKey)?.nickname ?? "";
    const roles =
      storageSession().getItem<DataInfo<number>>(userKey)?.roles ?? [];
    const permissions =
      storageSession().getItem<DataInfo<number>>(userKey)?.permissions ?? [];
    setUserKey({
      avatar,
      username,
      nickname,
      roles,
      permissions
    });
  }
}

/** 删除`token`以及key值为`user-info`的localStorage信息 */
export function removeToken() {
  Cookies.remove(TokenKey);
  Cookies.remove(multipleTabsKey);
  storageSession().removeItem(userKey);
}

/** 格式化token（jwt格式） */
export const formatToken = (token: string): string => {
  return "Bearer " + token;
};

/** 是否有按钮级别的权限（根据登录接口返回的`permissions`字段进行判断）*/
export const hasPerms = (value: string | Array<string>): boolean => {
  if (!value) return false;
  const allPerms = "*:*:*";
  const { permissions } = useUserStoreHook();
  if (!permissions) return false;
  if (permissions.length === 1 && permissions[0] === allPerms) return true;
  const isAuths = isString(value)
    ? permissions.includes(value)
    : isIncludeAllChildren(value, permissions);
  return isAuths ? true : false;
};
