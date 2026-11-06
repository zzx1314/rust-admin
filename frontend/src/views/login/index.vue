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
import Robot from "~icons/ri/robot-2-line";
import Signal from "~icons/ri/signal-tower-line";
import Route from "~icons/ri/route-line";
import MapIcon from "~icons/ri/map-2-line";
import Shield from "~icons/ri/shield-check-line";
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

/** 品牌区的能力说明：与后端业务域一一对应 */
const capabilities = [
  {
    icon: Signal,
    title: "设备状态实时可见",
    desc: "网关心跳驱动的在线率、通信时间与电量"
  },
  {
    icon: Route,
    title: "任务与流程下发",
    desc: "拖拽编排任务流程，指令直达设备"
  },
  {
    icon: MapIcon,
    title: "园区地图与航线",
    desc: "地图包、航线与模型资源集中管理"
  }
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
      <div class="login-brand__glow" aria-hidden="true" />

      <header class="login-brand__head">
        <span class="brand-mark" aria-hidden="true">
          <Robot width="22" height="22" />
        </span>
        <span class="brand-name">{{ title }}</span>
      </header>

      <div class="login-brand__hero" aria-hidden="true">
        <div class="robot-dog-wrap">
          <svg
            class="robot-dog"
            viewBox="0 0 420 240"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
          >
            <defs>
              <linearGradient id="rdBeam" x1="0" y1="0" x2="1" y2="1">
                <stop offset="0%" stop-color="#22C55E" stop-opacity=".38" />
                <stop offset="100%" stop-color="#22C55E" stop-opacity="0" />
              </linearGradient>
              <radialGradient id="rdShadow" cx="50%" cy="50%" r="50%">
                <stop offset="0%" stop-color="#020617" stop-opacity=".6" />
                <stop offset="100%" stop-color="#020617" stop-opacity="0" />
              </radialGradient>
            </defs>

            <!-- 地面投影（随浮动轻微收缩） -->
            <ellipse
              class="rd-shadow"
              cx="176"
              cy="207"
              rx="120"
              ry="9"
              fill="url(#rdShadow)"
            />

            <!-- 地面基准线（虚线缓慢流动） -->
            <path
              class="rd-ground"
              d="M30 206H390"
              stroke-dasharray="4 7"
              stroke="rgb(148 163 184 / 22%)"
              stroke-width="1.5"
            />

            <!-- 会整体浮动的机身 -->
            <g class="rd-float">
              <!-- 扫描光束 -->
              <path
                class="rd-beam"
                d="M300 92 L400 176 L400 206 H286 Z"
                fill="url(#rdBeam)"
              />

              <!-- 远侧腿（弱化，制造纵深；与近侧对角腿同相位） -->
              <g class="rd-legs-far">
                <g
                  class="rd-limb"
                  style="
                    --hip-x: 148px;
                    --hip-y: 124px;
                    --knee-x: 128px;
                    --knee-y: 166px;
                    --phase: -0.55s;
                  "
                >
                  <path d="M148 124L128 166" />
                  <g class="rd-limb__shin">
                    <path d="M128 166L140 202" />
                  </g>
                </g>
                <g
                  class="rd-limb"
                  style="
                    --hip-x: 250px;
                    --hip-y: 124px;
                    --knee-x: 234px;
                    --knee-y: 166px;
                    --phase: 0s;
                  "
                >
                  <path d="M250 124L234 166" />
                  <g class="rd-limb__shin">
                    <path d="M234 166L248 202" />
                  </g>
                </g>
              </g>

              <!-- 机身与机头 -->
              <rect
                class="rd-panel"
                x="96"
                y="82"
                width="168"
                height="50"
                rx="16"
              />
              <rect
                class="rd-panel"
                x="252"
                y="74"
                width="54"
                height="42"
                rx="13"
              />
              <path class="rd-neck" d="M244 96h16" />

              <!-- 传感器桅杆 + 云台 -->
              <rect
                class="rd-panel"
                x="152"
                y="56"
                width="12"
                height="30"
                rx="5"
              />
              <rect
                class="rd-panel"
                x="136"
                y="42"
                width="44"
                height="19"
                rx="8"
              />
              <circle class="rd-accent" cx="180" cy="51.5" r="5" />

              <!-- 天线与信号弧 -->
              <path class="rd-antenna" d="M204 56v-20" />
              <path
                class="rd-signal rd-signal--1"
                d="M198 30a10 10 0 0 1 12 0"
              />
              <path
                class="rd-signal rd-signal--2"
                d="M193 24a18 18 0 0 1 22 0"
              />

              <!-- 机头镜头：扩散波 + 呼吸光 -->
              <circle class="rd-lens-ring" cx="294" cy="95" r="8" />
              <circle class="rd-lens" cx="294" cy="95" r="8" />
              <circle class="rd-lens-core" cx="296" cy="93" r="3" />

              <!-- 近侧腿（膝关节圆点跟随小腿一起摆动） -->
              <g class="rd-legs-near">
                <g
                  class="rd-limb"
                  style="
                    --hip-x: 134px;
                    --hip-y: 126px;
                    --knee-x: 112px;
                    --knee-y: 168px;
                    --phase: 0s;
                  "
                >
                  <path d="M134 126L112 168" />
                  <g class="rd-limb__shin">
                    <path d="M112 168L126 202" />
                    <circle class="rd-accent" cx="112" cy="168" r="5" />
                  </g>
                </g>
                <g
                  class="rd-limb"
                  style="
                    --hip-x: 234px;
                    --hip-y: 126px;
                    --knee-x: 214px;
                    --knee-y: 168px;
                    --phase: -0.55s;
                  "
                >
                  <path d="M234 126L214 168" />
                  <g class="rd-limb__shin">
                    <path d="M214 168L228 202" />
                    <circle class="rd-accent" cx="214" cy="168" r="5" />
                  </g>
                </g>
              </g>

              <!-- 状态灯：依次点亮，像心跳 -->
              <circle class="rd-led rd-led--1" cx="118" cy="70" r="3.5" />
              <circle class="rd-led rd-led--2" cx="130" cy="70" r="3.5" />
              <circle class="rd-led rd-led--3" cx="142" cy="70" r="3.5" />
            </g>
          </svg>
        </div>
      </div>

      <div class="login-brand__copy">
        <h2 class="brand-title">机器狗统一调度与运维</h2>
        <p class="brand-subtitle">
          设备接入、任务下发、地图与航线、数据回传，都在同一个控制台完成。
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
        <span>账号权限决定可见的设备与数据范围</span>
      </footer>
    </section>

    <!-- 右侧表单区 -->
    <section class="login-panel">
      <div class="login-panel__top">
        <div class="login-panel__brand">
          <span class="brand-mark" aria-hidden="true">
            <Robot width="20" height="20" />
          </span>
          <span class="brand-name">{{ title }}</span>
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
          <h1 class="login-title">{{ t("login.pureLogin") }}</h1>
          <p class="login-subtitle">
            使用平台账号登录，登录后将自动进入你的工作台
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
