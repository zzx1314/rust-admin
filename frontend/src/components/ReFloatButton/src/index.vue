<script setup lang="ts">
import { ref } from "vue";
import type { floatBtnsType } from "./type";
import { isUrl, openLink } from "@pureadmin/utils";

import Open from "~icons/ri/open-arm-line";
import Close from "~icons/ep/close";

defineOptions({
  name: "ReFloatButton"
});

const props = defineProps({
  floatBtns: {
    type: Array as PropType<floatBtnsType>,
    default: () => []
  }
});

const isOpen = ref(false);
const mergeFloatBtns = ref<Array<floatBtnsType>>(
  props.floatBtns.concat({
    icon: Open,
    show: true
  })
);

function onShow(btn: floatBtnsType, index: number) {
  if (isUrl(btn?.link)) openLink(btn.link);
  const length = mergeFloatBtns.value.length - 1;
  if (index === length) {
    isOpen.value = !isOpen.value;
    Array.from({ length }).forEach((_, k) => {
      mergeFloatBtns.value[k].show = !mergeFloatBtns.value[k].show;
    });
    mergeFloatBtns.value[index].icon = isOpen.value ? Close : Open;
  } else {
    return;
  }
}
</script>

<template>
  <div class="pure-float-btn-group">
    <div v-for="(btn, index) in mergeFloatBtns" :key="index">
      <button
        v-if="btn.show"
        v-tippy="{
          content: btn?.tip,
          placement: 'left'
        }"
        v-motion-slide-bottom
        class="pure-float-btn"
        type="button"
        :delay="20 * index"
        @click="onShow(btn, index)"
      >
        <IconifyIconOffline :icon="btn.icon" />
      </button>
    </div>
  </div>
</template>

<style lang="scss" scoped>
.pure-float-btn-group {
  position: fixed;
  right: 10px;
  bottom: 60px;

  .pure-float-btn {
    width: 40px;
    height: 40px;
    margin-top: 10px;
    cursor: pointer;
    background: #fff;
    border: none;
    border-radius: 50%;
    box-shadow:
      0 6px 16px 0 rgb(0 0 0 / 8%),
      0 3px 6px -4px rgb(0 0 0 / 12%),
      0 9px 28px 8px rgb(0 0 0 / 5%);

    svg {
      margin: 0 auto;
      font-size: 20px;
      color: rgb(0 0 0 / 88%);
    }

    &:hover {
      background: var(--el-border-color-extra-light);
    }
  }
}
</style>
