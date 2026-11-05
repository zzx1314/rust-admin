<script setup lang="ts">
import { ref } from "vue";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import Search from "~icons/ep/search";
import Refresh from "~icons/ep/refresh";
import Down from "~icons/ep/arrow-down";
import Up from "~icons/ep/arrow-up";
import SearchField from "./SearchField.vue";
import type { DogSearchField } from "./search";

withDefaults(
  defineProps<{
    fields: DogSearchField[];
    moreFields?: DogSearchField[];
    loading?: boolean;
  }>(),
  { moreFields: () => [], loading: false }
);

const emit = defineEmits<{ search: []; reset: [] }>();

const model = defineModel<Record<string, any>>({ required: true });

const moreCondition = ref(false);

function setValue(prop: string, value: any) {
  if (model.value) model.value[prop] = value;
}
</script>

<template>
  <el-card>
    <el-form
      :inline="true"
      :model="model"
      class="bg-bg_color w-[99/100] pl-8 pt-4"
    >
      <el-form-item
        v-for="field in fields"
        :key="field.prop"
        :label="field.label"
        :prop="field.prop"
      >
        <SearchField
          :field="field"
          :model-value="model?.[field.prop]"
          @update:model-value="value => setValue(field.prop, value)"
        />
      </el-form-item>
      <el-collapse-transition>
        <div v-show="moreCondition">
          <el-form-item
            v-for="field in moreFields"
            :key="field.prop"
            :label="field.label"
            :prop="field.prop"
          >
            <SearchField
              :field="field"
              :model-value="model?.[field.prop]"
              @update:model-value="value => setValue(field.prop, value)"
            />
          </el-form-item>
        </div>
      </el-collapse-transition>
      <el-form-item>
        <el-button
          type="primary"
          :icon="useRenderIcon(Search)"
          :loading="loading"
          @click="emit('search')"
        >
          搜索
        </el-button>
        <el-button :icon="useRenderIcon(Refresh)" @click="emit('reset')">
          重置
        </el-button>
        <el-button
          v-if="moreFields.length"
          type="primary"
          :icon="moreCondition ? useRenderIcon(Down) : useRenderIcon(Up)"
          link
          @click="moreCondition = !moreCondition"
        />
      </el-form-item>
    </el-form>
  </el-card>
</template>
