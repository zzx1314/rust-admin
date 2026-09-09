<script setup lang="ts">
import { ref } from "vue";
import { FormInstance } from "element-plus";
import { useSysSeting } from "./hook";
import { hasAuth } from "@/router/utils";

defineOptions({
  name: "sysSeting"
});

const addFormRef = ref<FormInstance>();
const { loading, addForm, rules, cancel, addFormInfo } = useSysSeting();
</script>

<template>
  <div class="main">
    <el-card v-loading="loading" class="setting-card" shadow="never">
      <template #header>
        <div class="card-header">
          <span class="card-title">安全策略配置</span>
          <span class="card-subtitle">
            配置登录锁定、密码强度等安全策略，保存后下次登录生效
          </span>
        </div>
      </template>

      <el-form
        ref="addFormRef"
        :model="addForm"
        :rules="rules"
        label-width="120px"
        class="setting-form"
      >
        <el-form-item label="锁定时长" prop="sysLoginMaxLockTime">
          <el-select
            v-model="addForm.sysLoginMaxLockTime"
            placeholder="请选择锁定时长"
            class="!w-[300px]"
          >
            <el-option label="5分钟" value="5" />
            <el-option label="10分钟" value="10" />
            <el-option label="15分钟" value="15" />
            <el-option label="30分钟" value="30" />
          </el-select>
        </el-form-item>

        <el-form-item label="最大尝试次数" prop="sysLoginMaxTryCount">
          <el-select
            v-model="addForm.sysLoginMaxTryCount"
            placeholder="请选择最大尝试次数"
            class="!w-[300px]"
          >
            <el-option label="5次" value="5" />
            <el-option label="10次" value="10" />
            <el-option label="15次" value="15" />
            <el-option label="30次" value="30" />
          </el-select>
        </el-form-item>

        <el-form-item label="密码长度" prop="sysPassLength">
          <div class="flex items-center">
            <el-select
              v-model="addForm.sysPassShortLength"
              placeholder="最短长度"
              class="!w-[140px]"
            >
              <el-option label="8" value="8" />
              <el-option label="9" value="9" />
              <el-option label="10" value="10" />
            </el-select>
            <span class="px-2 text-secondary">--</span>
            <el-select
              v-model="addForm.sysPassLength"
              placeholder="最长长度"
              class="!w-[140px]"
            >
              <el-option label="11" value="11" />
              <el-option label="12" value="12" />
              <el-option label="13" value="13" />
            </el-select>
          </div>
        </el-form-item>

        <el-form-item label="密码有效期" prop="sysPassChange">
          <el-select
            v-model="addForm.sysPassChange"
            placeholder="请选择密码有效期"
            class="!w-[300px]"
          >
            <el-option label="5天" value="5" />
            <el-option label="7天" value="7" />
            <el-option label="14天" value="14" />
            <el-option label="30天" value="30" />
          </el-select>
        </el-form-item>

        <el-form-item label="密码强度" prop="passCom">
          <el-select
            v-model="addForm.passCom"
            placeholder="请选择密码强度"
            class="!w-[300px]"
          >
            <el-option label="密码是数字" value="1" />
            <el-option label="密码是数字，字母组合" value="2" />
            <el-option label="密码是数字，字母，特殊字符组合" value="3" />
          </el-select>
        </el-form-item>

        <el-form-item label="登录超时时间" prop="sysOvertime">
          <el-select
            v-model="addForm.sysOvertime"
            placeholder="请选择登录超时时间"
            class="!w-[300px]"
          >
            <el-option label="15分钟" value="900" />
            <el-option label="30分钟" value="1800" />
            <el-option label="1小时" value="3600" />
          </el-select>
        </el-form-item>

        <el-form-item>
          <el-button
            v-if="hasAuth('sys_seting_save')"
            type="primary"
            class="submit-btn"
            @click="addFormInfo(addFormRef)"
          >
            保存
          </el-button>
          <el-button
            v-if="hasAuth('sys_seting_save')"
            @click="cancel(addFormRef)"
          >
            取消
          </el-button>
        </el-form-item>
      </el-form>
    </el-card>
  </div>
</template>

<style lang="scss" scoped>
.setting-card {
  max-width: 720px;

  :deep(.el-card__header) {
    padding: 16px 24px;
  }

  :deep(.el-card__body) {
    padding: 24px;
  }
}

.card-header {
  display: flex;
  gap: 12px;
  align-items: baseline;

  .card-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .card-subtitle {
    font-size: 13px;
    color: var(--el-text-color-secondary);
  }
}

.setting-form {
  max-width: 560px;
  padding-top: 8px;

  :deep(.el-input__wrapper),
  :deep(.el-select__wrapper) {
    border-radius: 8px;
  }
}

.text-secondary {
  color: var(--el-text-color-secondary);
}
</style>
