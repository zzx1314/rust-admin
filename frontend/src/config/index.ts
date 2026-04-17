import axios from "axios";
import type { App } from "vue";
import { routerArrays, storageSession } from "@/store/utils";

let config: object = {};
const { VITE_PUBLIC_PATH } = import.meta.env;

const setConfig = (cfg?: unknown) => {
  config = Object.assign(config, cfg);
};

const getConfig = (key?: string): PlatformConfigs => {
  if (typeof key === "string") {
    const arr = key.split(".");
    if (arr && arr.length) {
      let data = config;
      arr.forEach(v => {
        if (data && typeof data[v] !== "undefined") {
          data = data[v];
        } else {
          data = null;
        }
      });
      return data;
    }
  }
  return config;
};

/** 获取项目动态全局配置 */
export const getPlatformConfig = async (app: App): Promise<undefined> => {
  app.config.globalProperties.$config = getConfig();
  return axios({
    method: "get",
    url: `${VITE_PUBLIC_PATH}platform-config.json`
  })
    .then(({ data: config }) => {
      let $config = app.config.globalProperties.$config;
      // 自动注入系统配置
      if (app && $config && typeof config === "object") {
        $config = Object.assign($config, config);
        app.config.globalProperties.$config = $config;
        // 设置全局配置
        setConfig($config);
      }
      return $config;
    })
    .catch(() => {
      throw "请在public文件夹下添加platform-config.json配置文件";
    });
};

/**
 * 获取项目动态全局配置V1
 */
export const getPlatformConfigV1 = async (): Promise<any> => {
  return axios({
    method: "get",
    url: `${VITE_PUBLIC_PATH}platform-config.json`
  })
    .then(({ data: config }) => {
      return config;
    })
    .catch(() => {
      throw "请在public文件夹下添加platform-config.json配置文件";
    });
};

export const refreshConfig = () => {
  getPlatformConfigV1().then(config => {
    console.log("重新获取", config);
    // responsive-configure， responsive-layout，responsive-locale，responsive-tags
    const nameSpace = responsiveStorageNameSpace();
    const configObj = Object.assign(
      {
        // 国际化 默认中文zh
        locale: storageSession().getItem(nameSpace + "locale") ?? {
          locale: config.Locale ?? "zh"
        },
        // layout模式以及主题
        layout: storageSession().getItem(nameSpace + "layout") ?? {
          layout: config.Layout ?? "vertical",
          theme: config.Theme ?? "light",
          darkMode: config.DarkMode ?? false,
          sidebarStatus: config.SidebarStatus ?? true,
          epThemeColor: config.EpThemeColor ?? "#409EFF",
          themeColor: config.Theme ?? "light", // 主题色（对应系统配置中的主题色，与theme不同的是它不会受到浅色、深色整体风格切换的影响，只会在手动点击主题色时改变）
          overallStyle: config.OverallStyle ?? "light" // 整体风格（浅色：light、深色：dark、自动：system）
        },
        // 系统配置-界面显示
        configure: storageSession().getItem(nameSpace + "configure") ?? {
          grey: config.Grey ?? false,
          weak: config.Weak ?? false,
          hideTabs: config.HideTabs ?? false,
          hideFooter: config.HideFooter ?? true,
          showLogo: config.ShowLogo ?? true,
          showModel: config.ShowModel ?? "smart",
          multiTagsCache: config.MultiTagsCache ?? false,
          stretch: config.Stretch ?? false
        }
      },
      config.MultiTagsCache
        ? {
            // 默认显示顶级菜单tag
            tags: storageSession().getItem(nameSpace + "tags") ?? routerArrays
          }
        : {}
    );
    for (const [key, value] of Object.entries(configObj)) {
      storageSession().setItem(nameSpace + key, value);
    }
  });
};

/** 本地响应式存储的命名空间 */
const responsiveStorageNameSpace = () => getConfig().ResponsiveStorageNameSpace;

export { getConfig, setConfig, responsiveStorageNameSpace };
