<script setup lang="ts">
import type { DogSearchField } from "./search";

defineProps<{ field: DogSearchField }>();

const value = defineModel<any>();
</script>

<template>
  <el-select
    v-if="field.type === 'select'"
    v-model="value"
    :placeholder="field.placeholder ?? `请选择${field.label}`"
    clearable
    class="!w-[150px]"
  >
    <el-option
      v-for="option in field.options ?? []"
      :key="option.value"
      :label="option.label"
      :value="option.value"
    />
  </el-select>
  <el-input-number
    v-else-if="field.type === 'number'"
    v-model="value"
    :controls="false"
    :placeholder="field.placeholder ?? `请输入${field.label}`"
    class="!w-[150px]"
  />
  <el-date-picker
    v-else-if="field.type === 'date'"
    v-model="value"
    type="date"
    :placeholder="field.placeholder ?? `请选择${field.label}`"
    :value-format="field.valueFormat ?? 'YYYY-MM-DD'"
    clearable
    class="!w-[180px]"
  />
  <el-input
    v-else
    v-model="value"
    :placeholder="field.placeholder ?? `请输入${field.label}`"
    clearable
    class="!w-[150px]"
  />
</template>
