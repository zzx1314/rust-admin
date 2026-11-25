import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { fileURLToPath, URL } from "node:url";

export default defineConfig({
  plugins: [vue()],
  define: {
    // 构建时间写进产物：界面右下角的账号菜单里显示，排查「是不是旧缓存」时直接用
    __APP_BUILD__: JSON.stringify(
      new Date().toLocaleString("zh-CN", { hour12: false })
    )
  },
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url))
    }
  },
  server: {
    host: "0.0.0.0",
    port: 5273,
    proxy: {
      "/api": {
        target: "http://127.0.0.1:3100",
        changeOrigin: true,
        // 观看通道是 WebSocket（/api/backpack/video/ws，只做在场计数；媒体走 WebRTC）
        // 不加 ws:true 时代理只转发 HTTP，播放器不会触发起流（页面无画面、后端无日志）
        ws: true
      }
    }
  },
  build: {
    // element-plus / echarts 单个体积就超过默认 500 kB 阈值，
    // 拆到独立 vendor chunk 以便长期缓存，并按实际体积调整告警阈值
    chunkSizeWarningLimit: 1200,
    rollupOptions: {
      output: {
        manualChunks: {
          "vendor-vue": ["vue", "vue-router", "pinia"],
          "vendor-element-plus": ["element-plus"],
          "vendor-echarts": ["echarts"]
        }
      }
    }
  }
});
