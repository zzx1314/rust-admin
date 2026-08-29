declare module "*.vue" {
  import type { DefineComponent } from "vue";
  // eslint-disable-next-line
  const component: DefineComponent<{}, {}, any>;
  export default component;
}

declare module "*.scss" {
  const scss: Record<string, string>;
  export default scss;
}

declare module "*.md" {
  const content: string;
  export default content;
}

declare module "*.md?raw" {
  const content: string;
  export default content;
}

declare module "vue3-puzzle-vcode";
declare module "vue-virtual-scroller";
declare module "vuedraggable/src/vuedraggable";
