<template>
  <el-config-provider :locale="currentLocale">
    <router-view />
    <ReDialog />
    <ReDrawer />
    <!-- <ReFloatButton :floatBtns="floatBtns" /> -->
  </el-config-provider>
</template>

<script lang="ts">
import { defineComponent } from "vue";
import { checkVersion } from "version-rocket";
import { ElConfigProvider } from "element-plus";
import { ReDialog } from "@/components/ReDialog";
import { ReDrawer } from "@/components/ReDrawer";
import en from "element-plus/es/locale/lang/en";
import ja from "element-plus/es/locale/lang/ja";
import ko from "element-plus/es/locale/lang/ko";
import zhCn from "element-plus/es/locale/lang/zh-cn";
import zhTw from "element-plus/es/locale/lang/zh-tw";
import plusEn from "plus-pro-components/es/locale/lang/en";
import plusZhCn from "plus-pro-components/es/locale/lang/zh-cn";
import ReFloatButton from "@/components/ReFloatButton";
import "@vue-flow/core/dist/style.css";
import "@vue-flow/core/dist/theme-default.css";
import "@vue-flow/controls/dist/style.css";
import "@vue-flow/minimap/dist/style.css";
import "@vue-flow/node-resizer/dist/style.css";

import Service from "~icons/ri/user-heart-line";
import Book from "~icons/ri/book-open-line";
import Max from "~icons/ri/vip-diamond-line";

export default defineComponent({
  name: "app",
  components: {
    [ElConfigProvider.name]: ElConfigProvider,
    ReDialog,
    ReDrawer
  },
  computed: {
    currentLocale() {
      switch (this.$storage.locale?.locale) {
        case "zh":
          return { ...zhCn, ...plusZhCn };
        case "en":
          return { ...en, ...plusEn };
        case "tw":
          return { ...zhTw, ...plusEn };
        case "ja":
          return { ...ja, ...plusEn };
        case "ko":
          return { ...ko, ...plusEn };
        default:
          return { ...zhCn, ...plusZhCn };
      }
    },
    floatBtns() {
      return [
        {
          tip: "优质服务",
          icon: Service,
          link: "https://pure-admin.cn/pages/service/",
          show: false
        },
        {
          tip: "保姆级文档",
          link: "https://pure-admin.cn/",
          icon: Book,
          show: false
        },
        {
          tip: "max版本",
          link: "https://pure-admin.cn/pages/max/",
          icon: Max,
          show: false
        }
      ];
    }
  },
  beforeCreate() {
    const { version, name: title } = __APP_INFO__.pkg;
    const { VITE_PUBLIC_PATH, MODE } = import.meta.env;
    // https://github.com/guMcrey/version-rocket/blob/main/README.zh-CN.md#api
    if (MODE === "production") {
      // 版本实时更新检测，只作用于线上环境
      checkVersion(
        // config
        {
          // 5分钟检测一次版本
          pollingTime: 300000,
          localPackageVersion: version,
          originVersionFileUrl: `${location.origin}${VITE_PUBLIC_PATH}version.json`
        },
        // options
        {
          title,
          description: "检测到新版本",
          buttonText: "立即更新"
        }
      );
    }
  }
});
</script>
