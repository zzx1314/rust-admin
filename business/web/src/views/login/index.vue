<script setup lang="ts">
import { ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { ElMessage } from "element-plus";
import { useAuthStore } from "@/stores/auth";
import AppIcon from "@/components/AppIcon.vue";

const auth = useAuthStore();
const router = useRouter();
const route = useRoute();

const username = ref("admin");
const password = ref("admin123");
const loading = ref(false);

async function submit() {
  if (!username.value.trim() || !password.value) {
    ElMessage.warning("请输入用户名和密码");
    return;
  }
  loading.value = true;
  try {
    await auth.login(username.value.trim(), password.value);
    ElMessage.success("登录成功");
    const redirect =
      typeof route.query.redirect === "string" ? route.query.redirect : "/dashboard";
    router.push(redirect);
  } catch {
    /* handled by the http interceptor */
  } finally {
    loading.value = false;
  }
}
</script>

<template>
  <div class="login-page">
    <section class="login-visual">
      <div class="login-brand">
        <div class="brand-logo"><AppIcon name="dog" :size="26" /></div>
        <div>
          <b>机器狗巡检业务平台</b>
          <span>ROBODOG OPS</span>
        </div>
      </div>

      <div class="login-copy">
        <h1>让每一次巡检<br /><em>都可见、可控、可追溯</em></h1>
        <p>
          面向园区、设备与巡检任务的统一运营控制台。可视化编排航点与动作，
          实时掌握设备状态、任务进度与告警处置。
        </p>
        <div class="login-features">
          <div class="login-feature"><b>7</b>业务模块</div>
          <div class="login-feature"><b>24/7</b>设备监控</div>
          <div class="login-feature"><b>100%</b>过程留痕</div>
        </div>
      </div>

      <div class="login-feature" style="position: relative; z-index: 1">
        RoboDog OPS · 北京华致技术
      </div>
    </section>

    <section class="login-form-wrap">
      <div class="login-card">
        <h2>登录控制台</h2>
        <p class="login-sub">
          使用中台账号登录；登录后只看到自己管辖的设备与自己名下机器狗采集的地图。
        </p>

        <div class="field">
          <label>用户名</label>
          <input v-model="username" class="input" placeholder="请输入用户名" @keydown.enter="submit" />
        </div>
        <div class="field">
          <label>密码</label>
          <input
            v-model="password"
            class="input"
            type="password"
            placeholder="请输入密码"
            @keydown.enter="submit"
          />
        </div>

        <button class="btn primary login-submit" :disabled="loading" @click="submit">
          <AppIcon name="shield" />
          {{ loading ? "正在登录…" : "进入控制台" }}
        </button>

        <div class="login-hint">演示账号：admin / admin123</div>
      </div>
    </section>
  </div>
</template>
