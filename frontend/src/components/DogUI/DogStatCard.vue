<script setup lang="ts">
import { computed, type Component } from "vue";
import { usePreferredReducedMotion } from "@vueuse/core";
import { ReNormalCountTo } from "@/components/ReCountTo";

/**
 * 指标卡（KPI）
 *
 * - 数值使用等宽数字，避免刷新时宽度跳动（CLS）；
 * - 数字滚动仅为装饰：无障碍读到的是完整句子，动画层对读屏隐藏；
 * - 尊重「减少动态效果」偏好，直接渲染最终数值。
 */
const props = withDefaults(
  defineProps<{
    label: string;
    value: number;
    unit?: string;
    /** 环比文案，例如 "+12%" / "-3%" */
    percent?: string;
    /** 补充说明，例如 "本周新增" */
    hint?: string;
    icon?: Component;
    /** 覆盖图标底色，默认取设计令牌 */
    accent?: string;
    loading?: boolean;
  }>(),
  {
    unit: "",
    percent: "",
    hint: "",
    icon: undefined,
    accent: "",
    loading: false
  }
);

const preferredMotion = usePreferredReducedMotion();
const reduceMotion = computed(() => preferredMotion.value === "reduce");

const formatted = computed(() =>
  new Intl.NumberFormat("zh-CN").format(props.value)
);

const valueText = computed(
  () => `${props.label} ${formatted.value}${props.unit}`
);

const delta = computed(() => {
  const raw = String(props.percent || "").trim();
  if (!raw) return null;
  const numeric = Number.parseFloat(raw.replace("%", ""));
  if (Number.isNaN(numeric)) return { text: raw, positive: true, zero: true };
  return {
    text: numeric === 0 ? "0%" : `${numeric > 0 ? "+" : ""}${numeric}%`,
    positive: numeric >= 0,
    zero: numeric === 0
  };
});

const accentStyle = computed(() =>
  props.accent
    ? { "--dog-stat-accent": props.accent }
    : ({} as Record<string, string>)
);
</script>

<template>
  <article class="dog-stat-card" :style="accentStyle">
    <div class="dog-stat-card__head">
      <span class="dog-stat-card__label">{{ label }}</span>
      <span v-if="icon" class="dog-stat-card__icon" aria-hidden="true">
        <component :is="icon" />
      </span>
    </div>

    <div class="dog-stat-card__body">
      <p class="dog-stat-card__value dog-num">
        <span class="sr-only">{{ valueText }}</span>
        <span aria-hidden="true">
          <ReNormalCountTo
            v-if="!loading && !reduceMotion"
            :key="value"
            :end-val="value"
            :start-val="0"
            :duration="800"
            font-size="1em"
          />
          <template v-else>{{ loading ? "—" : formatted }}</template>
          <span v-if="unit" class="dog-stat-card__unit">{{ unit }}</span>
        </span>
      </p>
      <div v-if="$slots.chart" class="dog-stat-card__chart">
        <slot name="chart" />
      </div>
    </div>

    <div v-if="delta || hint" class="dog-stat-card__foot">
      <span
        v-if="delta"
        class="dog-stat-card__delta"
        :class="{
          'dog-stat-card__delta--down': !delta.positive,
          'dog-stat-card__delta--flat': delta.zero
        }"
      >
        <svg
          v-if="!delta.zero"
          class="dog-stat-card__arrow"
          viewBox="0 0 16 16"
          width="12"
          height="12"
          aria-hidden="true"
          focusable="false"
        >
          <path v-if="delta.positive" d="M8 2.5 13 9H3z" fill="currentColor" />
          <path v-else d="M8 13.5 3 7h10z" fill="currentColor" />
        </svg>
        <span>{{ delta.text }}</span>
        <span class="dog-stat-card__delta-unit">较上周</span>
      </span>
      <span v-if="hint" class="dog-stat-card__hint">{{ hint }}</span>
    </div>
  </article>
</template>

<style scoped lang="scss">
.dog-stat-card {
  --dog-stat-accent: var(--dog-accent);

  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  gap: var(--dog-space-2);
  min-width: 0;
  height: 100%;
  padding: var(--dog-space-4);
  background: var(--dog-surface-card);
  border: 1px solid var(--dog-border);
  border-radius: var(--dog-radius-lg);
  box-shadow: var(--dog-shadow-sm);
  transition:
    border-color var(--dog-duration-base) var(--dog-ease),
    box-shadow var(--dog-duration-base) var(--dog-ease),
    transform var(--dog-duration-base) var(--dog-ease);
}

@media (hover: hover) {
  .dog-stat-card:hover {
    border-color: var(--dog-border-strong);
    box-shadow: var(--dog-shadow-md);
    transform: translateY(-1px);
  }
}

.dog-stat-card__head {
  display: flex;
  gap: var(--dog-space-2);
  align-items: center;
  justify-content: space-between;
}

.dog-stat-card__label {
  overflow: hidden;
  text-overflow: ellipsis;
  font-size: 14px;
  font-weight: 500;
  line-height: 1.5;
  color: var(--dog-text-secondary);
  white-space: nowrap;
}

.dog-stat-card__icon {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  color: var(--dog-stat-accent);
  background: color-mix(in srgb, var(--dog-stat-accent) 12%, transparent);
  border-radius: var(--dog-radius-md);
}

.dog-stat-card__body {
  display: flex;
  gap: var(--dog-space-3);
  align-items: flex-end;
  justify-content: space-between;
  min-width: 0;
}

.dog-stat-card__value {
  display: flex;
  align-items: baseline;
  min-width: 0;
  margin: 0;
  font-size: 30px;
  font-weight: 700;
  line-height: 1.15;
  color: var(--dog-text-primary);
}

.dog-stat-card__unit {
  margin-left: 4px;
  font-family: inherit;
  font-size: 14px;
  font-weight: 500;
  color: var(--dog-text-muted);
}

.dog-stat-card__chart {
  flex: none;
  width: 96px;
  max-width: 45%;
  height: 52px;
}

.dog-stat-card__foot {
  display: flex;
  flex-wrap: wrap;
  gap: var(--dog-space-2);
  align-items: center;
  font-size: 12px;
  line-height: 1.6;
  color: var(--dog-text-muted);
}

.dog-stat-card__delta {
  display: inline-flex;
  gap: 2px;
  align-items: center;
  font-weight: 600;
  color: var(--dog-status-online);
}

.dog-stat-card__delta--down {
  color: var(--dog-status-fault);
}

.dog-stat-card__delta--flat {
  color: var(--dog-text-muted);
}

.dog-stat-card__delta-unit {
  font-weight: 400;
  color: var(--dog-text-muted);
}

.dog-stat-card__arrow {
  margin-top: 1px;
}

.dog-stat-card__hint {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
