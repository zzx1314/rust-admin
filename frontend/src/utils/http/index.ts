import Axios, {
  type AxiosInstance,
  type AxiosRequestConfig,
  type CustomParamsSerializer
} from "axios";
import type {
  RequestMethods,
  PureHttpResponse,
  PureHttpRequestConfig
} from "./types.d";
import qs from "qs";
import { stringify } from "qs";
import NProgress from "../progress";
import { getToken, formatToken, updateTokenSilently } from "@/utils/auth";
import { markSessionActivity } from "@/utils/sessionActivity";
import type { UserResult } from "@/api/user";
import { message } from "@/utils/message";
import { useUserStoreHook } from "@/store/modules/user";

/** 刷新令牌接口（与 `src/api/user.ts` 的 `urls.refreshToken` 保持一致） */
const REFRESH_TOKEN_PATH = "/api/token/refresh/";
/** 刷新失败时的重试次数（只针对网络抖动/5xx，401 直接判死） */
const REFRESH_MAX_RETRY = 2;
/** 重试退避基数（毫秒） */
const REFRESH_RETRY_DELAY_MS = 400;

// 默认 Axios 配置
const defaultConfig: AxiosRequestConfig = {
  // 请求超时时间
  timeout: 100000,
  headers: {
    Accept: "application/json, text/plain, */*",
    "Content-Type": "application/json",
    "X-Requested-With": "XMLHttpRequest"
  },
  // 数组格式参数序列化（https://github.com/axios/axios/issues/5142）
  paramsSerializer: {
    serialize: stringify as unknown as CustomParamsSerializer
  }
};

export const cleanQuery = (query: Record<string, any>): Record<string, any> => {
  if (!query) return {};
  return Object.fromEntries(
    Object.entries(query).filter(
      ([_, value]) => value !== null && value !== undefined && value !== ""
    )
  );
};

class PureHttp {
  constructor() {
    this.httpInterceptorsRequest();
    this.httpInterceptorsResponse();
  }

  /** 进行中的刷新请求（单飞：并发 401 只真正刷新一次，其余等待同一个 Promise） */
  private static tokenRefreshPromise: Promise<string> | null = null;

  /** 初始化配置对象 */
  private static initConfig: PureHttpRequestConfig = {};

  /** 保存当前`Axios`实例对象 */
  private static axiosInstance: AxiosInstance = Axios.create(defaultConfig);

  /**
   * 刷新 `access token`（连同 `refresh token` 一起换新，后端会 rotate）。
   *
   * 单飞：并发调用共享同一个请求——后端的 refresh 是一次性的，两个标签页/多个并发
   * 401 各拿旧 refresh 去换，后到的那个必然 401 被踢下线。
   * 失败不会缓存，下次调用会重新尝试。
   */
  public static refreshAccessToken(): Promise<string> {
    if (!PureHttp.tokenRefreshPromise) {
      PureHttp.tokenRefreshPromise = PureHttp.requestNewToken().finally(() => {
        PureHttp.tokenRefreshPromise = null;
      });
    }
    return PureHttp.tokenRefreshPromise;
  }

  /**
   * 真实发起一次刷新。
   *
   * - 网络抖动 / 5xx：退避重试 `REFRESH_MAX_RETRY` 次（以前一次失败就直接把人踢了）；
   * - 401 / 403：refresh 本身已失效（被顶号、超过 7 天、服务重启丢了内存会话），重试无意义。
   */
  private static async requestNewToken(): Promise<string> {
    const refreshToken = getToken()?.refreshToken;
    if (!refreshToken) throw new Error("本地没有 refreshToken，无法续期");

    let lastError: unknown;
    for (let attempt = 0; attempt <= REFRESH_MAX_RETRY; attempt++) {
      try {
        const body = (await PureHttp.axiosInstance.get(
          `${REFRESH_TOKEN_PATH}${refreshToken}`,
          { skipAuthRefresh: true } as PureHttpRequestConfig
        )) as unknown as {
          accessToken?: string;
          refreshToken?: string;
          expiresIn?: number;
        };

        if (!body?.accessToken) {
          throw new Error("刷新接口没有返回 accessToken");
        }
        updateTokenSilently({
          accessToken: body.accessToken,
          refreshToken: body.refreshToken ?? refreshToken,
          expiresIn: Number(body.expiresIn) || 0
        });
        markSessionActivity();
        return body.accessToken;
      } catch (error) {
        lastError = error;
        const status = (error as { response?: { status?: number } })?.response
          ?.status;
        if (status === 401 || status === 403) throw error;
        if (attempt < REFRESH_MAX_RETRY) {
          await new Promise(resolve =>
            setTimeout(resolve, REFRESH_RETRY_DELAY_MS * (attempt + 1))
          );
        }
      }
    }
    throw lastError ?? new Error("刷新 token 失败");
  }

  /** 请求拦截 */
  private httpInterceptorsRequest(): void {
    PureHttp.axiosInstance.interceptors.request.use(
      async (config: PureHttpRequestConfig): Promise<any> => {
        // 开启进度条动画
        if (
          !config.url.includes("/token/check_token") &&
          !config.url.includes("/statistics")
        ) {
          NProgress.start();
        }
        // 优先判断post/get等方法是否传入回调，否则执行初始化设置等回调
        if (typeof config.beforeRequestCallback === "function") {
          config.beforeRequestCallback(config);
          return config;
        }
        if (PureHttp.initConfig.beforeRequestCallback) {
          PureHttp.initConfig.beforeRequestCallback(config);
          return config;
        }
        /** 请求白名单，放置一些不需要`token`的接口（通过设置请求白名单，防止`token`过期后再请求造成的死循环问题） */
        const whiteList = ["/token", "/login"];
        return whiteList.some(url => config.url.endsWith(url))
          ? config
          : new Promise(resolve => {
              const data = getToken();
              if (data) {
                // 在请求头添加权限
                config.headers["Authorization"] = formatToken(data.accessToken);
                resolve(config);
              } else {
                resolve(config);
              }
            });
      },
      error => {
        return Promise.reject(error);
      }
    );
  }

  /** 响应拦截 */
  private httpInterceptorsResponse(): void {
    const instance = PureHttp.axiosInstance;

    instance.interceptors.response.use(
      (response: PureHttpResponse) => {
        NProgress.done();
        /** 前台页面的接口流量也算「还在用」，否则轮询/看视频的页面会被当成空闲 */
        markSessionActivity();
        return response.data; // 统一返回 response.data
      },
      async (error: any) => {
        const { response, config } = error;
        NProgress.done();

        if (!response || !config) return Promise.reject(error);

        if (config.skipAuthRefresh) {
          return Promise.reject(error);
        }

        /** 登录/注册等接口的 401 不走 token 刷新逻辑，直接走下面的错误提示 */
        const isLoginRequest = config.url?.includes("/token");
        const needRefresh =
          response.status === 401 && !config._retry && !isLoginRequest;

        if (needRefresh) {
          config._retry = true;
          let accessToken: string;
          try {
            /**
             * 单飞刷新：同一时刻的多个 401 只会真正调一次刷新接口，
             * 之前那套「挂起队列」在重放之后到达的 401 会永远没有 resolve，
             * 页面表现为卡住而不是重试。
             */
            accessToken = await PureHttp.refreshAccessToken();
          } catch (e) {
            message("登录已过期，请重新登录", { type: "error" });
            useUserStoreHook().logOut();
            return Promise.reject(e);
          }

          config.headers = config.headers ?? {};
          config.headers["Authorization"] = formatToken(accessToken);
          // 重放一次；若重放仍然 401，`_retry` 已置位，会走到下面的登出分支
          return PureHttp.axiosInstance(config);
        }

        // 处理其他错误
        if (response.status === 500) {
          message("服务器错误", { type: "error" });
        } else if (
          [400, 401, 424].includes(response.status) &&
          response.data?.msg
        ) {
          message(response.data.msg, { type: "error" });
        } else if (response.data?.msg) {
          message(response.data.msg, { type: "error" });
        } else {
          message("系统错误", { type: "error" });
        }

        // 只在 401 未授权且非登录请求时登出，避免密码错误时触发无意义的登出
        if (
          (response.status === 401 || response.status === 424) &&
          !isLoginRequest
        ) {
          useUserStoreHook().logOut();
        }
        return Promise.reject(error);
      }
    );
  }

  /** 通用请求工具函数 */
  public request<T>(
    method: RequestMethods,
    url: string,
    param?: AxiosRequestConfig,
    axiosConfig?: PureHttpRequestConfig
  ): Promise<T> {
    const config = {
      method,
      url,
      ...param,
      ...axiosConfig
    } as PureHttpRequestConfig;

    // 单独处理自定义请求/响应回调
    return new Promise((resolve, reject) => {
      PureHttp.axiosInstance
        .request(config)
        .then((response: undefined) => {
          resolve(response);
        })
        .catch(error => {
          reject(error);
        });
    });
  }

  /** 单独抽离的`post`工具函数 */
  public post<T, P>(
    url: string,
    params?: AxiosRequestConfig<P>,
    config?: PureHttpRequestConfig
  ): Promise<T> {
    return this.request<T>("post", url, params, config);
  }

  /** 单独抽离的`get`工具函数 */
  public get<T, P>(
    url: string,
    params?: AxiosRequestConfig<P>,
    config?: PureHttpRequestConfig
  ): Promise<T> {
    return this.request<T>("get", url, params, config);
  }

  /**
   * post请求,from表单
   * @param url url
   * @param params 参数
   */
  public axiosPost<T>(url, params): Promise<T> {
    return new Promise((resolve, reject) => {
      PureHttp.axiosInstance
        .post(url, qs.stringify(params), {
          headers: {
            "content-type": "application/x-www-form-urlencoded"
          }
        })
        .then((response: undefined) => {
          resolve(response);
        })
        .catch(err => {
          reject(err);
        });
    });
  }

  /**
   * from 请求直接返回response
   * @param url
   * @param params
   */
  public axiosPostFrom<T>(url: string, params: any, heads?: any): Promise<T> {
    return new Promise((resolve, reject) => {
      PureHttp.axiosInstance
        .post(url, qs.stringify(params), {
          headers: {
            ...(heads || {}),
            "content-type": "application/x-www-form-urlencoded"
          }
        })
        .then((response: undefined) => {
          resolve(response);
        })
        .catch(err => {
          reject(err);
        });
    });
  }

  /**
   * 登录请求
   * @param url
   * @param params
   * @param heads
   */
  public axiosPostFromLogin(
    url: string,
    params: any,
    heads?: any
  ): Promise<UserResult> {
    return new Promise((resolve, reject) => {
      PureHttp.axiosInstance
        .post(url, qs.stringify(params), {
          headers: {
            ...(heads || {}),
            "content-type": "application/x-www-form-urlencoded"
          }
        })
        .then((response: any) => {
          const result = {
            success: true,
            data: {
              avatar: "", // 需要填充实际值
              accessToken: response.access_token,
              refreshToken: response.refresh_token,
              expires: response.expires,
              username: response.username,
              nickname: response.nickname,
              permissions: response.permissions,
              roles: response.roles,
              user_id: response.user_id,
              passwordExpired: response.password_expired,
              passwordExpireDate: response.password_expire_date
            }
          };
          resolve(result);
        })
        .catch(err => {
          reject(err);
        });
    });
  }

  /**
   * post请求
   * @param url
   * @param params
   */
  public axiosPostRequest<T>(url, params?: object): Promise<T> {
    return new Promise((resolve, reject) => {
      PureHttp.axiosInstance
        .post(url, params, {
          headers: {
            "content-type": "application/json"
          }
        })
        .then((response: undefined) => {
          resolve(response);
        })
        .catch(err => {
          reject(err);
        });
    });
  }

  /**
   * get请求
   * @param url url
   * @param params 参数
   */
  public axiosGet<T>(url, params?: object): Promise<T> {
    return new Promise((resolve, reject) => {
      PureHttp.axiosInstance
        .get(url, { params })
        .then((response: undefined) => {
          resolve(response);
        })
        .catch(err => {
          reject(err);
        });
    });
  }

  public axiosGetDown<T>(url, range): Promise<T> {
    return new Promise((resolve, reject) => {
      PureHttp.axiosInstance
        .get(url, {
          responseType: "blob",
          headers: {
            Authentication: sessionStorage.getItem("token"),
            Range: range
          }
        })
        .then((response: undefined) => {
          resolve(response);
        })
        .catch(err => {
          reject(err);
        });
    });
  }

  /**
   * get请求
   * @param url
   * @param params
   */
  public axiosGetRequest<T>(url, params): Promise<T> {
    const cleanedQuery = cleanQuery(params);
    return new Promise((resolve, reject) => {
      PureHttp.axiosInstance
        .get(
          url + "?" + qs.stringify(cleanedQuery, { arrayFormat: "repeat" }),
          {}
        )
        .then((response: undefined) => {
          resolve(response);
        })
        .catch(error => {
          reject(error);
        });
    });
  }

  /**
   * delete 请求
   * @param url
   * @param params
   */

  public axiosDelete<T>(url, params?: object): Promise<T> {
    return new Promise((resolve, reject) => {
      PureHttp.axiosInstance
        .delete(url, params)
        .then((response: undefined) => {
          resolve(response);
        })
        .catch(err => {
          reject(err);
        });
    });
  }

  /**
   * put请求
   * @param url
   * @param params
   */
  public axiosPut<T>(url, params): Promise<T> {
    return new Promise((resolve, reject) => {
      PureHttp.axiosInstance
        .put(url, params)
        .then((response: undefined) => {
          resolve(response);
        })
        .catch(err => {
          reject(err);
        });
    });
  }

  /**
   * put请求添加config
   * @param url
   * @param params
   * @param config
   */
  public axiosPutRequest<T>(url, params, config): Promise<T> {
    return new Promise((resolve, reject) => {
      PureHttp.axiosInstance
        .put(url, params, config)
        .then((response: undefined) => {
          resolve(response);
        })
        .catch(err => {
          reject(err);
        });
    });
  }

  public axiosPatch(url, params) {
    const promise = new Promise((resolve, reject) => {
      PureHttp.axiosInstance
        .patch(url, params)
        .then((response: undefined) => {
          resolve(response);
        })
        .catch(err => {
          reject(err);
        });
    });
    return promise;
  }

  public axiosGetAll(url, config) {
    const promise = new Promise((resolve, reject) => {
      PureHttp.axiosInstance
        .get(url, config)
        .then((response: undefined) => {
          resolve(response);
        })
        .catch(err => {
          reject(err);
        });
    });
    return promise;
  }

  /**
   *
   * @param URL 下载地址
   * @param mode 下载方式 get post
   * @param name 下载文件名
   * @param param 参数
   */
  public downloadUrlMode(url, mode, name, param) {
    const promise = new Promise((resolve, reject) => {
      PureHttp.axiosInstance({
        url: url,
        method: mode,
        data: param,
        params: param,
        headers: {
          Authentication: sessionStorage.getItem("token"),
          Accept: "application/json"
        },
        responseType: "arraybuffer"
      })
        .then((response: undefined) => {
          // name 截取后缀，以点
          const fileType = name.substring(name.lastIndexOf(".") + 1);
          const blob = new Blob([response], {
            type: "application/" + fileType
          });
          resolve(response);
          const fileName = name;
          // const fileName = name;
          const link = document.createElement("a");
          link.href = URL.createObjectURL(blob);
          link.download = fileName;
          document.body.appendChild(link);
          link.click();
          window.setTimeout(function () {
            URL.revokeObjectURL(link.href);
            document.body.removeChild(link);
          }, 0);
        })
        .catch(err => {
          reject(err);
        });
    });
    return promise;
  }

  /**
   * 文件预览
   * @param url
   * @param type
   */
  public previewFile(url, type) {
    const promise = new Promise((resolve, reject) => {
      PureHttp.axiosInstance({
        url: url,
        method: "GET",
        headers: {
          Authentication: sessionStorage.getItem("token"),
          Accept: "application/json"
        },
        responseType: type
      })
        .then((response: undefined) => {
          resolve(response);
        })
        .catch(err => {
          reject(err);
        });
    });
    return promise;
  }

  /**
   * 下载
   * @param URL
   */
  public downloadUrl(URL) {
    PureHttp.axiosInstance({
      method: "get",
      url: URL,
      headers: {
        Authentication: sessionStorage.getItem("token"),
        Accept: "application/json"
      },
      responseType: "blob"
    }).then((response: undefined) => {
      const blob = new Blob([response], { type: "application/zip" });
      const downloadElement = document.createElement("a");
      const url = window.URL.createObjectURL(blob);
      downloadElement.href = url;
      downloadElement.download = name + ".zip";
      downloadElement.click();
      window.URL.revokeObjectURL(url);
    });
  }

  /**
   * 上传文件
   * @param URL
   * @param formData
   */
  public uploadFile<T>(URL, formData): Promise<T> {
    const url = URL;
    const headers = {
      Authentication: sessionStorage.getItem("token"),
      "Content-Type": "multipart/form-data"
    };
    return PureHttp.axiosInstance.post(url, formData, { headers: headers });
  }

  /**
   * 获取数据流
   * @param url
   */
  public getBufferFile(url) {
    const promise = new Promise((resolve, reject) => {
      PureHttp.axiosInstance({
        url,
        method: "get",
        headers: {
          Authentication: sessionStorage.getItem("token"),
          Accept: "application/json"
        },
        responseType: "arraybuffer"
      })
        .then(response => {
          resolve(response.data);
        })
        .catch(err => {
          reject(err);
        });
    });
    return promise;
  }

  /**
   * 上传文件
   * @param URL
   * @param formData
   */
  public upFile(URL, formData) {
    const url = URL;
    const headers = { "Content-Type": "multipart/form-data" };
    const promise = new Promise((resolve, reject) => {
      PureHttp.axiosInstance
        .post(url, formData, { headers: headers })
        .then(response => {
          resolve(response.data);
        })
        .catch(err => {
          reject(err);
        });
    });
    return promise;
  }

  /**
   * 将二进制文件转化成base64
   * @param data
   */
  public getImageBase64(data) {
    const blob = new Blob([data], { type: "image/jpg" }); //类型一定要写！！！
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.readAsDataURL(blob);
      reader.onload = () => resolve(reader.result);
      reader.onerror = error => reject(error);
    });
  }
}

export const http = new PureHttp();

/**
 * 刷新会话令牌（单飞、失败重试、成功后自己落盘），
 * 供拦截器之外的调用方（如 `useSessionTimeout` 的定时保活）使用。
 */
export const refreshAccessToken = (): Promise<string> =>
  PureHttp.refreshAccessToken();
