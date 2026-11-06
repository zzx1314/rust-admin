import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import vueJsx from "@vitejs/plugin-vue-jsx";
import tailwindcss from "@tailwindcss/vite";
import Icons from "unplugin-icons/vite";
import { fileURLToPath, URL } from "node:url";

const src = fileURLToPath(new URL("../src", import.meta.url));
const preview = fileURLToPath(new URL(".", import.meta.url));

export default defineConfig({
  root: preview,
  resolve: {
    alias: [
      { find: /^@\/api\/dogStatistics$/, replacement: `${preview}mock/dogStatistics.ts` },
      { find: /^@\/api\/dogDevice$/, replacement: `${preview}mock/dogDevice.ts` },
      { find: /^@\//, replacement: `${src}/` }
    ]
  },
  plugins: [tailwindcss(), vue(), vueJsx(), Icons({ compiler: "vue3", scale: 1 })]
});
