<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { message } from "@/utils/message";
import { loginRules } from "./utils/rule";
import { debounce } from "@pureadmin/utils";
import { useNav } from "@/layout/hooks/useNav";
import { useEventListener } from "@vueuse/core";
import { ElMessageBox } from "element-plus";
import type { FormInstance } from "element-plus";
import { $t, transformI18n } from "@/plugins/i18n";
import { useLayout } from "@/layout/hooks/useLayout";
import LoginPhone from "./components/LoginPhone.vue";
import LoginRegist from "./components/LoginRegist.vue";
import LoginUpdate from "./components/LoginUpdate.vue";
import LoginQrCode from "./components/LoginQrCode.vue";
import { useUserStoreHook } from "@/store/modules/user";
import { initRouter, getTopMenu } from "@/router/utils";
import { ReImageVerify } from "@/components/ReImageVerify";
import { ref, reactive, watch, computed, nextTick } from "vue";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import { useTranslationLang } from "@/layout/hooks/useTranslationLang";
import { useDataThemeChange } from "@/layout/hooks/useDataThemeChange";

import dayIcon from "@/assets/svg/day.svg?component";
import darkIcon from "@/assets/svg/dark.svg?component";
import Lock from "~icons/ri/lock-fill";
import User from "~icons/ri/user-3-fill";
import Stack from "~icons/ri/stack-line";
import Shield from "~icons/ri/shield-check-line";
import Database from "~icons/ri/database-2-line";
import Api from "~icons/ri/api-line";
import MapIcon from "~icons/ri/map-2-line";
import Cpu from "~icons/ri/cpu-line";
import ImageIcon from "~icons/ri/image-line";
import Settings from "~icons/ri/settings-4-line";
import Question from "~icons/ri/question-line";
import Translate from "~icons/ri/translate-2";

defineOptions({
  name: "Login"
});

const imgCode = ref("");
const loginDay = ref(7);
const router = useRouter();
const loading = ref(false);
const checked = ref(false);
const disabled = ref(false);
const ruleFormRef = ref<FormInstance>();
const currentPage = computed(() => {
  return useUserStoreHook().currentPage;
});

const { t } = useI18n();
const { initStorage } = useLayout();
initStorage();
const { dataTheme, overallStyle, dataThemeChange } = useDataThemeChange();
dataThemeChange(overallStyle.value);
const { title } = useNav();
const { locale, translationCh, translationEn } = useTranslationLang();

const ruleForm = reactive({
  username: "",
  password: "",
  verifyCode: ""
});

/** 密码过期后提示用户前往修改密码 */
const promptPasswordExpired = (expireDate?: string) => {
  const content = expireDate
    ? `您的密码已于 ${expireDate} 过期，为了账号安全，请尽快修改密码。`
    : "您的密码已过期，为了账号安全，请尽快修改密码。";
  ElMessageBox.confirm(content, "密码已过期", {
    confirmButtonText: "去修改",
    cancelButtonText: "稍后提醒",
    type: "warning",
    closeOnClickModal: false
  })
    .then(() => {
      router.push({ path: "/account/settings", query: { tab: "pwd" } });
    })
    .catch(() => {});
};

const onLogin = async (formEl: FormInstance | undefined) => {
  if (!formEl) return;
  await formEl.validate(valid => {
    if (valid) {
      loading.value = true;
      useUserStoreHook()
        .loginByUsername({
          username: ruleForm.username,
          password: ruleForm.password
        })
        .then(res => {
          if (res.success) {
            // 获取后端路由
            return initRouter().then(async () => {
              await router.isReady();
              disabled.value = true;
              await nextTick();
              await new Promise(resolve => setTimeout(resolve, 0));
              router
                .push(getTopMenu(true).path)
                .then(() => {
                  message(t("login.pureLoginSuccess"), { type: "success" });
                  if (res.data.passwordExpired) {
                    promptPasswordExpired(res.data.passwordExpireDate);
                  }
                })
                .finally(() => (disabled.value = false));
            });
          }
          message(t("login.pureLoginFail"), { type: "error" });
        })
        .catch(() => {
          // 错误消息已由 HTTP 拦截器统一展示，此处仅阻止未处理的 promise rejection
        })
        .finally(() => (loading.value = false));
    }
  });
};

const immediateDebounce: any = debounce(
  formRef => onLogin(formRef),
  1000,
  true
);

useEventListener(document, "keypress", ({ code }) => {
  if (
    ["Enter", "NumpadEnter"].includes(code) &&
    !disabled.value &&
    !loading.value
  )
    immediateDebounce(ruleFormRef.value);
});

watch(imgCode, value => {
  useUserStoreHook().SET_VERIFYCODE(value);
});
watch(checked, bool => {
  useUserStoreHook().SET_ISREMEMBERED(bool);
});
watch(loginDay, value => {
  useUserStoreHook().SET_LOGINDAY(value);
});

/** 品牌区能力说明：对应中台提供的四类基础能力 */
const capabilities = [
  {
    icon: Shield,
    title: "统一认证与授权",
    desc: "账号、角色、菜单与按钮级权限集中管理"
  },
  {
    icon: Stack,
    title: "业务资源管理",
    desc: "地图、模型、媒体与参数配置的统一台账"
  },
  {
    icon: Database,
    title: "数据与审计",
    desc: "数据字典、操作审计与安全策略一体化"
  },
  {
    icon: Api,
    title: "开放接口",
    desc: "为业务平台提供稳定的 REST API 与统一响应约定"
  }
];

/** 轨道上的资源节点：环绕中台中枢展示 */
const hubs = [
  { label: "地图", angle: -90, icon: MapIcon },
  { label: "模型", angle: 0, icon: Cpu },
  { label: "媒体", angle: 90, icon: ImageIcon },
  { label: "配置", angle: 180, icon: Settings }
];

function onTranslation(lang: string) {
  if (lang === locale.value) return;
  lang === "en" ? translationEn() : translationCh();
}
</script>

<template>
  <div class="login-page">
    <!-- 左侧品牌区：小屏隐藏，避免挤占表单空间 -->
    <section class="login-brand">
      <div class="login-brand__grid" aria-hidden="true" />
      <div
        class="login-brand__glow login-brand__glow--one"
        aria-hidden="true"
      />
      <div
        class="login-brand__glow login-brand__glow--two"
        aria-hidden="true"
      />

      <header class="login-brand__head">
        <span class="brand-mark" aria-hidden="true">
          <Stack width="20" height="20" />
        </span>
        <span class="brand-name">{{ title }}</span>
      </header>

      <div class="login-brand__hero" aria-hidden="true">
        <div class="hub">
          <div class="hub__rings">
            <span class="hub__ring hub__ring--inner" />
            <span class="hub__ring hub__ring--outer" />
            <span class="hub__orbit">
              <i class="hub__pulse" />
            </span>
          </div>

          <div class="hub__core">
            <span class="hub__core-icon">
              <Stack width="22" height="22" />
            </span>
            <b>业务中台</b>
            <em>Middle Platform</em>
          </div>

          <div
            v-for="hub in hubs"
            :key="hub.label"
            class="hub__node"
            :style="{ '--angle': `${hub.angle}deg` }"
          >
            <span class="hub__node-inner">
              <component :is="hub.icon" width="14" height="14" />
              {{ hub.label }}
            </span>
          </div>
        </div>
      </div>

      <div class="login-brand__copy">
        <h2 class="brand-title">统一业务中台</h2>
        <p class="brand-subtitle">
          把资源、权限与数据治理收敛到一处，为业务平台提供稳定的能力底座。
        </p>
        <ul class="brand-capabilities">
          <li
            v-for="item in capabilities"
            :key="item.title"
            class="brand-capability"
          >
            <span class="brand-capability__icon" aria-hidden="true">
              <component :is="item.icon" width="16" height="16" />
            </span>
            <span>
              <b>{{ item.title }}</b>
              <em>{{ item.desc }}</em>
            </span>
          </li>
        </ul>
      </div>

      <footer class="login-brand__foot">
        <Shield width="14" height="14" aria-hidden="true" />
        <span>账号权限决定可见的菜单与业务资源</span>
      </footer>
    </section>

    <!-- 右侧表单区 -->
    <section class="login-panel">
      <div class="login-panel__top">
        <div class="login-panel__brand">
          <span class="brand-mark brand-mark--light" aria-hidden="true">
            <Stack width="18" height="18" />
          </span>
          <span class="brand-name brand-name--dark">{{ title }}</span>
        </div>

        <div class="login-panel__tools">
          <el-dropdown trigger="click" @command="onTranslation">
            <button
              type="button"
              class="login-icon-btn"
              aria-label="切换语言 / Switch language"
            >
              <Translate width="16" height="16" />
            </button>
            <template #dropdown>
              <el-dropdown-menu>
                <el-dropdown-item
                  command="zh"
                  :class="{ 'is-current': locale === 'zh' }"
                >
                  简体中文
                </el-dropdown-item>
                <el-dropdown-item
                  command="en"
                  :class="{ 'is-current': locale === 'en' }"
                >
                  English
                </el-dropdown-item>
              </el-dropdown-menu>
            </template>
          </el-dropdown>
          <el-switch
            v-model="dataTheme"
            inline-prompt
            aria-label="切换明暗主题"
            :active-icon="dayIcon"
            :inactive-icon="darkIcon"
            @change="dataThemeChange"
          />
        </div>
      </div>

      <div class="login-card">
        <div class="login-card__head">
          <span class="login-card__eyebrow">业务中台 · 管理控制台</span>
          <h1 class="login-title">{{ t("login.pureLogin") }}</h1>
          <p class="login-subtitle">
            使用中台账号登录，登录后将自动进入你的工作台
          </p>
        </div>

        <el-form
          v-if="currentPage === 0"
          ref="ruleFormRef"
          :model="ruleForm"
          :rules="loginRules"
          size="large"
          label-position="top"
          class="login-form"
          @submit.prevent
        >
          <el-form-item
            :label="t('login.pureUsername')"
            prop="username"
            :rules="[
              {
                required: true,
                message: transformI18n($t('login.pureUsernameReg')),
                trigger: 'blur'
              }
            ]"
          >
            <el-input
              v-model="ruleForm.username"
              clearable
              autocomplete="username"
              :placeholder="t('login.pureUsername')"
              :prefix-icon="useRenderIcon(User)"
            />
          </el-form-item>

          <el-form-item :label="t('login.purePassword')" prop="password">
            <el-input
              v-model="ruleForm.password"
              clearable
              show-password
              autocomplete="current-password"
              :placeholder="t('login.purePassword')"
              :prefix-icon="useRenderIcon(Lock)"
            />
          </el-form-item>

          <el-form-item :label="t('login.pureVerifyCode')" prop="verifyCode">
            <el-input
              v-model="ruleForm.verifyCode"
              clearable
              autocomplete="one-time-code"
              :placeholder="t('login.pureVerifyCode')"
              :prefix-icon="useRenderIcon('ri:shield-keyhole-line')"
            >
              <template #append>
                <ReImageVerify v-model:code="imgCode" />
              </template>
            </el-input>
          </el-form-item>

          <div class="login-options">
            <el-checkbox v-model="checked" size="default">
              {{ loginDay }} {{ t("login.pureRemember") }}
            </el-checkbox>
            <el-tooltip
              :content="t('login.pureRememberInfo')"
              placement="top"
              effect="dark"
            >
              <button
                type="button"
                class="login-icon-btn login-icon-btn--sm"
                :aria-label="t('login.pureRememberInfo')"
              >
                <Question width="15" height="15" />
              </button>
            </el-tooltip>
          </div>

          <el-button
            class="login-submit"
            size="large"
            type="primary"
            native-type="submit"
            :loading="loading"
            :disabled="disabled"
            @click="onLogin(ruleFormRef)"
          >
            {{ t("login.pureLogin") }}
          </el-button>
        </el-form>

        <!-- 手机号登录 -->
        <LoginPhone v-if="currentPage === 1" />
        <!-- 二维码登录 -->
        <LoginQrCode v-if="currentPage === 2" />
        <!-- 注册 -->
        <LoginRegist v-if="currentPage === 3" />
        <!-- 忘记密码 -->
        <LoginUpdate v-if="currentPage === 4" />

        <p class="login-card__hint">
          账号由中台管理员统一分配，如需开通请联系系统管理员
        </p>
      </div>

      <p class="login-copyright">
        Copyright © {{ new Date().getFullYear() }} {{ title }}
      </p>
    </section>
  </div>
</template>

<style>
/* 登录页布局样式。
   需要覆盖 Element Plus 内部元素（label / checkbox 等），因此不使用 scoped。 */
@import url("@/style/login.css");
</style>

<style lang="scss" scoped>
:deep(.el-input-group__append, .el-input-group__prepend) {
  padding: 0;
}
</style>
