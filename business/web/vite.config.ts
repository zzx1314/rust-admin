import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { fileURLToPath, URL } from "node:url";

export default defineConfig({
  plugins: [vue()],
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
        // 实时视频走 WebSocket（/api/devices/{id}/video/ws），
        // 不加 ws:true 时代理只转发 HTTP，播放器会一直连不上（页面无画面、后端无日志）
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
