<script setup lang="ts">
import { ref } from "vue";
import ReCol from "@/components/ReCol";
import { formRules } from "../utils/rule";
import { FormProps } from "../utils/types";
import { usePublicHooks } from "../../hooks";

const props = withDefaults(defineProps<FormProps>(), {
  formInline: () => ({
    title: "新增",
    label: "",
    value: "",
    color: "#6abe39",
    sort: 999,
    status: 1,
    remark: ""
  })
});

const ruleFormRef = ref();
const { switchStyle } = usePublicHooks();
const newFormInline = ref(props.formInline);
const predefineColors = ref([
  "#6abe39",
  "#e84749",
  "#9fceff",
  "#fab6b6",
  "#172412",
  "#274a17",
  "#2b1316",
  "#58191c"
]);

function getRef() {
  return ruleFormRef.value;
}

defineExpose({ getRef });
</script>

<template>
  <el-form
    ref="ruleFormRef"
    :model="newFormInline"
    :rules="formRules"
    label-width="82px"
  >
    <el-row :gutter="30">
      <re-col>
        <el-form-item label="字典标签" prop="label">
          <el-input
            v-model="newFormInline.label"
            clearable
            placeholder="请输入字典标签"
          />
        </el-form-item>
      </re-col>
      <re-col>
        <el-form-item label="字典值" prop="value">
          <el-input
            v-model="newFormInline.value"
            clearable
            placeholder="请输入字典值"
          />
        </el-form-item>
      </re-col>

      <re-col>
        <el-form-item label="标签颜色">
          <el-input
            v-model="newFormInline.color"
            class="color-input"
            clearable
            placeholder="请输入或选择标签颜色"
          >
            <template #append>
              <el-color-picker
                v-model="newFormInline.color"
                :predefine="predefineColors"
              />
            </template>
          </el-input>
        </el-form-item>
      </re-col>
      <re-col>
        <el-form-item label="排序">
          <el-input-number
            v-model="newFormInline.sort"
            class="w-full!"
            :min="1"
            controls-position="right"
            placeholder="请输入排序"
          />
        </el-form-item>
      </re-col>

      <re-col v-if="newFormInline.title === '新增'">
        <el-form-item label="状态">
          <el-switch
            v-model="newFormInline.status"
            inline-prompt
            :active-value="1"
            :inactive-value="0"
            active-text="启用"
            inactive-text="停用"
            :style="switchStyle"
          />
        </el-form-item>
      </re-col>

      <re-col>
        <el-form-item label="备注">
          <el-input
            v-model="newFormInline.remark"
            placeholder="请输入备注信息"
            maxlength="200"
            :rows="3"
            show-word-limit
            type="textarea"
          />
        </el-form-item>
      </re-col>
    </el-row>
  </el-form>
</template>

<style lang="scss" scoped>
.color-input {
  :deep(.el-input-group__append) {
    padding: 0;
  }
}
</style>
