import { defineStore } from "pinia";
import {
  type userType,
  store,
  router,
  routerArrays,
  storageSession
} from "../utils";
import { type UserResult, getLogin, refreshTokenApi } from "@/api/user";
import { useMultiTagsStoreHook } from "./multiTags";
import { type DataInfo, setToken, userKey } from "@/utils/auth";

import aesUtils from "@/utils/aes";
import { refreshConfig } from "@/config";

export const useUserStore = defineStore("pure-user", {
  state: (): userType => ({
    // 头像
    avatar: storageSession().getItem<DataInfo<number>>(userKey)?.avatar ?? "",
    // 用户名
    username:
      storageSession().getItem<DataInfo<number>>(userKey)?.username ?? "",
    // 昵称
    nickname:
      storageSession().getItem<DataInfo<number>>(userKey)?.nickname ?? "",
    // 页面级别权限
    roles: storageSession().getItem<DataInfo<number>>(userKey)?.roles ?? [],
    // 按钮级别权限
    permissions:
      storageSession().getItem<DataInfo<number>>(userKey)?.permissions ?? [],
    // 前端生成的验证码（按实际需求替换）
    verifyCode: "",
    // 判断登录页面显示哪个组件（0：登录（默认）、1：手机登录、2：二维码登录、3：注册、4：忘记密码）
    currentPage: 0,
    // 是否勾选了登录页的免登录
    isRemembered: false,
    // 登录页的免登录存储几天，默认7天
    loginDay: 7
  }),
  actions: {
    /** 存储头像 */
    SET_AVATAR(avatar: string) {
      this.avatar = avatar;
    },
    /** 存储用户名 */
    SET_USERNAME(username: string) {
      this.username = username;
    },
    /** 存储昵称 */
    SET_NICKNAME(nickname: string) {
      this.nickname = nickname;
    },
    /** 存储角色 */
    SET_ROLES(roles: Array<number>) {
      this.roles = roles;
    },
    /** 存储按钮级别权限 */
    SET_PERMS(permissions: Array<string>) {
      this.permissions = permissions;
    },
    /** 存储前端生成的验证码 */
    SET_VERIFYCODE(verifyCode: string) {
      this.verifyCode = verifyCode;
    },
    /** 存储登录页面显示哪个组件 */
    SET_CURRENTPAGE(value: number) {
      this.currentPage = value;
    },
    /** 存储是否勾选了登录页的免登录 */
    SET_ISREMEMBERED(bool: boolean) {
      this.isRemembered = bool;
    },
    /** 设置登录页的免登录存储几天 */
    SET_LOGINDAY(value: number) {
      this.loginDay = Number(value);
    },
    /** 登入 */
    async loginByUsername(data) {
      data.password = aesUtils.encode(data.password, "");
      data.grant_type = "password";
      data.scope = "server";
      return new Promise<UserResult>((resolve, reject) => {
        getLogin(data)
          .then(response => {
            if (response?.success) setToken(response.data);
            resolve(response);
          })
          .catch(error => {
            reject(error);
          });
      });
    },
    /** 前端登出（不调用接口） */
    logOut() {
      this.username = "";
      this.roles = [];
      this.permissions = [];
      storageSession().clear();
      refreshConfig();
      useMultiTagsStoreHook().handleTags("equal", [...routerArrays]);
      router.push("/login");
    },
    /** 刷新`token` */
    async handRefreshToken(refreshToken: string) {
      const res = await refreshTokenApi(refreshToken);

      if (!res) throw new Error("refresh token failed");
      const userInfo = storageSession().getItem<DataInfo<number>>(userKey);
      userInfo.accessToken = res.access_token;
      userInfo.refreshToken = res.refresh_token;
      storageSession().setItem(userKey, userInfo);
      return res;
    }
  }
});

export function useUserStoreHook() {
  return useUserStore(store);
}
