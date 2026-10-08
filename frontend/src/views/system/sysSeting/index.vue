<script setup lang="ts">
import { ref } from "vue";
import { FormInstance } from "element-plus";
import { useSysSeting } from "./hook";
import { hasAuth } from "@/router/utils";
import ShieldCheck from "~icons/ri/shield-check-line";
import LoginBox from "~icons/ri/login-box-line";
import KeyLine from "~icons/ri/key-2-line";
import RefreshLine from "~icons/ri/refresh-line";
import SaveLine from "~icons/ri/save-line";

defineOptions({
  name: "sysSeting"
});

const addFormRef = ref<FormInstance>();
const { loading, addForm, rules, cancel, addFormInfo } = useSysSeting();
</script>

<template>
  <div class="security-page">
    <div class="page-header">
      <div class="header-left">
        <div class="header-icon">
          <ShieldCheck />
        </div>
        <div class="header-text">
          <div class="header-title">安全策略配置</div>
          <div class="header-desc">
            配置登录锁定、密码强度等安全策略，保存后下次登录生效
          </div>
        </div>
      </div>
      <div class="header-actions">
        <el-button
          v-if="hasAuth('sys_seting_save')"
          :icon="RefreshLine"
          @click="cancel(addFormRef)"
        >
          重置
        </el-button>
        <el-button
          v-if="hasAuth('sys_seting_save')"
          type="primary"
          :icon="SaveLine"
          @click="addFormInfo(addFormRef)"
        >
          保存配置
        </el-button>
      </div>
    </div>

    <el-form
      ref="addFormRef"
      v-loading="loading"
      :model="addForm"
      :rules="rules"
      label-width="100px"
      label-position="left"
      class="security-form"
    >
      <div class="section-grid">
        <el-card class="section-card" shadow="never">
          <template #header>
            <div class="section-title">
              <span class="section-icon is-login"><LoginBox /></span>
              <span>登录保护</span>
            </div>
          </template>

          <el-form-item label="尝试次数" prop="sysLoginMaxTryCount">
            <div class="field">
              <el-select
                v-model="addForm.sysLoginMaxTryCount"
                placeholder="请选择最大尝试次数"
                class="control"
              >
                <el-option label="5次" value="5" />
                <el-option label="10次" value="10" />
                <el-option label="15次" value="15" />
                <el-option label="30次" value="30" />
              </el-select>
              <span class="hint">连续失败达到该次数后锁定账户</span>
            </div>
          </el-form-item>

          <el-form-item label="锁定时长" prop="sysLoginMaxLockTime">
            <div class="field">
              <el-select
                v-model="addForm.sysLoginMaxLockTime"
                placeholder="请选择锁定时长"
                class="control"
              >
                <el-option label="5分钟" value="5" />
                <el-option label="10分钟" value="10" />
                <el-option label="15分钟" value="15" />
                <el-option label="30分钟" value="30" />
              </el-select>
              <span class="hint">账户锁定后自动解锁的等待时间</span>
            </div>
          </el-form-item>

          <el-form-item label="超时时间" prop="sysOvertime" class="is-last">
            <div class="field">
              <el-select
                v-model="addForm.sysOvertime"
                placeholder="请选择登录超时时间"
                class="control"
              >
                <el-option label="15分钟" value="900" />
                <el-option label="30分钟" value="1800" />
                <el-option label="1小时" value="3600" />
              </el-select>
              <span class="hint">无操作超过该时长，会话自动失效</span>
            </div>
          </el-form-item>
        </el-card>

        <el-card class="section-card" shadow="never">
          <template #header>
            <div class="section-title">
              <span class="section-icon is-password"><KeyLine /></span>
              <span>密码策略</span>
            </div>
          </template>

          <el-form-item label="密码长度" prop="sysPassLength">
            <div class="field">
              <div class="field-row">
                <el-select
                  v-model="addForm.sysPassShortLength"
                  placeholder="最短长度"
                  class="control-sm"
                >
                  <el-option label="8" value="8" />
                  <el-option label="9" value="9" />
                  <el-option label="10" value="10" />
                </el-select>
                <span class="px-2 text-secondary">—</span>
                <el-select
                  v-model="addForm.sysPassLength"
                  placeholder="最长长度"
                  class="control-sm"
                >
                  <el-option label="11" value="11" />
                  <el-option label="12" value="12" />
                  <el-option label="13" value="13" />
                </el-select>
              </div>
              <span class="hint">新密码长度需介于最短与最长之间</span>
            </div>
          </el-form-item>

          <el-form-item label="密码强度" prop="passCom">
            <div class="field">
              <el-select
                v-model="addForm.passCom"
                placeholder="请选择密码强度"
                class="control"
              >
                <el-option label="密码是数字" value="1" />
                <el-option label="密码是数字，字母组合" value="2" />
                <el-option label="密码是数字，字母，特殊字符组合" value="3" />
              </el-select>
              <span class="hint"
                >逐级增强：纯数字 / 数字+字母 / 再加特殊字符</span
              >
            </div>
          </el-form-item>

          <el-form-item label="密码有效期" prop="sysPassChange" class="is-last">
            <div class="field">
              <el-select
                v-model="addForm.sysPassChange"
                placeholder="请选择密码有效期"
                class="control"
              >
                <el-option label="5天" value="5" />
                <el-option label="7天" value="7" />
                <el-option label="14天" value="14" />
                <el-option label="30天" value="30" />
              </el-select>
              <span class="hint">到期前提醒用户更换密码</span>
            </div>
          </el-form-item>
        </el-card>
      </div>
    </el-form>
  </div>
</template>

<style lang="scss" scoped>
.security-page {
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 20px 24px;
}

.page-header,
.security-form {
  width: 100%;
  max-width: 1120px;
}

.section-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 16px;
  align-items: start;
}

@media (width <= 900px) {
  .section-grid {
    grid-template-columns: 1fr;
  }
}

.page-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 16px;

  .header-left {
    display: flex;
    gap: 14px;
    align-items: center;
    min-width: 0;
  }

  .header-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 48px;
    height: 48px;
    font-size: 26px;
    color: var(--el-color-primary);
    background: var(--el-color-primary-light-9);
    border-radius: 12px;
  }

  .header-title {
    font-size: 18px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .header-desc {
    margin-top: 4px;
    font-size: 13px;
    color: var(--el-text-color-secondary);
  }

  .header-actions {
    display: flex;
    flex-shrink: 0;
    gap: 12px;
    margin-left: 16px;
  }
}

.section-card {
  :deep(.el-card__header) {
    padding: 12px 18px;
  }

  :deep(.el-card__body) {
    padding: 18px 18px 6px;
  }
}

.section-title {
  display: flex;
  gap: 10px;
  align-items: center;
  font-size: 15px;
  font-weight: 600;
  color: var(--el-text-color-primary);

  .section-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 30px;
    height: 30px;
    font-size: 17px;
    border-radius: 8px;

    &.is-login {
      color: var(--el-color-primary);
      background: var(--el-color-primary-light-9);
    }

    &.is-password {
      color: var(--el-color-warning);
      background: var(--el-color-warning-light-9);
    }
  }
}

.el-form-item {
  :deep(.el-input__wrapper),
  :deep(.el-select__wrapper) {
    border-radius: 8px;
  }

  &.is-last {
    margin-bottom: 12px;
  }
}

.control {
  width: 100%;
}

.control-sm {
  flex: 1 1 0;
  min-width: 0;
}

.field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}

.field-row {
  display: flex;
  align-items: center;
}

.hint {
  font-size: 12px;
  color: var(--el-text-color-placeholder);
}

.text-secondary {
  color: var(--el-text-color-secondary);
}
</style>
