import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import vueJsx from "@vitejs/plugin-vue-jsx";
import tailwindcss from "@tailwindcss/vite";
import Icons from "unplugin-icons/vite";
import svgLoader from "vite-svg-loader";
import VueI18nPlugin from "@intlify/unplugin-vue-i18n/vite";
import { fileURLToPath, URL } from "node:url";
const src = fileURLToPath(new URL("../src", import.meta.url));
const preview = fileURLToPath(new URL(".", import.meta.url));
const locales = fileURLToPath(new URL("../locales/**", import.meta.url));
export default defineConfig({
  root: preview,
  resolve: { alias: [{ find: /^@\//, replacement: `${src}/` }] },
  plugins: [tailwindcss(), vue(), vueJsx(), svgLoader(), VueI18nPlugin({ include: [locales] }), Icons({ compiler: "vue3", scale: 1 })]
});
