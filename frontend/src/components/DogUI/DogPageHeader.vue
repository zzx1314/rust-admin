<script setup lang="ts">
/**
 * 页面标题区
 *
 * 每个业务页面的唯一 h1：给「当前在哪一页」一个清晰锚点，
 * 副标题承载口径 / 权限说明（例如「仅显示已绑定给你的设备」）。
 */
withDefaults(
  defineProps<{
    title: string;
    description?: string;
    /** 小号标签，例如模块名 */
    eyebrow?: string;
    /** 标题右侧的状态 / 计数等内联信息 */
    compact?: boolean;
  }>(),
  { description: "", eyebrow: "", compact: false }
);
</script>

<template>
  <header
    class="dog-page-header"
    :class="{ 'dog-page-header--compact': compact }"
  >
    <div class="dog-page-header__main">
      <p v-if="eyebrow" class="dog-page-header__eyebrow">{{ eyebrow }}</p>
      <h1 class="dog-page-header__title">{{ title }}</h1>
      <p v-if="description" class="dog-page-header__desc">{{ description }}</p>
    </div>
    <div v-if="$slots.actions || $slots.meta" class="dog-page-header__aside">
      <slot name="meta" />
      <div v-if="$slots.actions" class="dog-page-header__actions">
        <slot name="actions" />
      </div>
    </div>
  </header>
</template>

<style scoped lang="scss">
.dog-page-header {
  display: flex;
  flex-wrap: wrap;
  gap: var(--dog-space-3);
  align-items: flex-end;
  justify-content: space-between;
  min-width: 0;
}

.dog-page-header--compact {
  align-items: center;
}

.dog-page-header__main {
  min-width: 0;
}

.dog-page-header__eyebrow {
  margin: 0 0 2px;
  font-size: 12px;
  font-weight: 600;
  line-height: 1.6;
  color: var(--dog-accent);
  text-transform: uppercase;
  letter-spacing: 0.08em;
}

.dog-page-header__title {
  margin: 0;
  font-size: 22px;
  font-weight: 700;
  line-height: 1.35;
  color: var(--dog-text-primary);
  text-wrap: balance;
}

.dog-page-header__desc {
  max-width: 72ch;
  margin: 4px 0 0;
  font-size: 13px;
  line-height: 1.6;
  color: var(--dog-text-muted);
  text-wrap: pretty;
}

.dog-page-header__aside {
  display: flex;
  flex-wrap: wrap;
  gap: var(--dog-space-2);
  align-items: center;
}

.dog-page-header__actions {
  display: flex;
  gap: var(--dog-space-2);
  align-items: center;
}

@media (width <= 640px) {
  .dog-page-header__title {
    font-size: 19px;
  }

  .dog-page-header__aside {
    width: 100%;
  }
}
</style>
