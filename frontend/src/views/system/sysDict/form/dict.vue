<script setup lang="ts">
import { ref } from "vue";
import ReCol from "@/components/ReCol";
import { dictFormRules } from "../utils/rule";
import { DictFormProps } from "../utils/types";

const props = withDefaults(defineProps<DictFormProps>(), {
  formInline: () => ({
    title: "",
    name: "",
    code: "",
    remark: ""
  })
});

const ruleFormRef = ref();
const newFormInline = ref(props.formInline);

function getRef() {
  return ruleFormRef.value;
}

defineExpose({ getRef });
</script>

<template>
  <el-form
    ref="ruleFormRef"
    label-width="auto"
    :model="newFormInline"
    :rules="dictFormRules"
  >
    <el-row :gutter="30">
      <re-col>
        <el-form-item label="名称" prop="name">
          <el-input v-model="newFormInline.name" />
        </el-form-item>
      </re-col>
      <re-col>
        <el-form-item label="编码" prop="code">
          <el-input
            v-model="newFormInline.code"
            :disabled="newFormInline.title === '修改' ? true : false"
          />
        </el-form-item>
      </re-col>
      <re-col>
        <el-form-item label="描述" prop="remark">
          <el-input
            v-model="newFormInline.remark"
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
