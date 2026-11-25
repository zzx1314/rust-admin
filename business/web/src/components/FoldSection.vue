<script setup lang="ts">
/**
 * 可折叠分区：一行标题 + 一大块内容（详情页那种一屏放不下的长内容）。
 *
 * 为什么不用 `el-collapse`：这些分区自己带头部按钮（「回传板端录像」）、图标和全局的
 * `rt-sec-title` 样式（`styles/prototype.css`），塞进 el-collapse 的头部结构里就得跟
 * 它的样式打架。这里只做一件事：标题整行可点 + 内容用 `v-show` 隐藏。
 *
 * 用 `v-show` 而不是 `v-if`：折叠期间**不销毁**内容 —— 视频/地图不重新协商
 * （WebRTC 重连要几秒），照片的 objectURL 也不用重建（一次巡检几十张，重建就是重下）。
 *
 * 用法：
 *   <FoldSection v-model:open="foldOpen.photos" title="任务照片（3）" icon="camera">
 *     <template #actions><button class="btn sm">…</button></template>
 *     …内容…
 *   </FoldSection>
 */
import AppIcon from "@/components/AppIcon.vue";

const props = withDefaults(
  defineProps<{
    /** 标题；条数直接写进标题里（「任务录像（3）」），收起时也不藏信息 */
    title: string;
    /** 右上角图标（留空就不画） */
    icon?: string;
    /** 是否展开，配合 `v-model:open` 由父组件控制 */
    open?: boolean;
  }>(),
  { icon: "", open: true }
);

const emit = defineEmits<{ (event: "update:open", value: boolean): void }>();

function toggle() {
  emit("update:open", !props.open);
}
</script>

<template>
  <section class="fold-sec">
    <div
      class="rt-sec-title fold-head"
      role="button"
      tabindex="0"
      :aria-expanded="open"
      :title="open ? '收起这一块' : '展开这一块'"
      @click="toggle"
      @keydown.enter.prevent="toggle"
      @keydown.space.prevent="toggle"
    >
      <span>{{ title }}</span>
      <span class="fold-right">
        <!-- 头部按钮（如「回传板端录像」）：点它只做它自己的事，不能顺带折叠 -->
        <span class="fold-actions" @click.stop><slot name="actions" /></span>
        <AppIcon v-if="icon" :name="icon" />
        <span class="fold-caret" :class="{ open }" aria-hidden="true" />
      </span>
    </div>
    <div v-show="open" class="fold-body"><slot /></div>
  </section>
</template>
