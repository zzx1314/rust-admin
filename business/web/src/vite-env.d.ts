/// <reference types="vite/client" />

/**
 * 构建期环境变量（vite 注入）。
 * 现有：`VITE_ZENTAI_CONSOLE_URL` —— 中台控制台地址，业务控制台「在中台查看地图」
 * 用它拼直达链接；不配时按当前 host 推 `:3005`（外网同机部署）。
 */
interface ImportMetaEnv {
  readonly VITE_ZENTAI_CONSOLE_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
