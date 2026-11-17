<script setup lang="ts">
import { onMounted, reactive, ref } from "vue";
import { ElMessage } from "element-plus";
import { deviceApi, notifyApi, parkApi, settingApi } from "@/api/ops";
import type { NotificationRecord, Park, Setting } from "@/api/types";
import PageHeader from "@/components/PageHeader.vue";
import AppIcon from "@/components/AppIcon.vue";
import { COORDINATE_SYSTEMS } from "@/utils/constants";

const settings = ref<Setting[]>([]);
const parks = ref<Park[]>([]);
const deviceTotal = ref(0);
const values = reactive<Record<string, string>>({});
const saving = ref(false);
const testing = ref(false);
const testResult = ref<{ ok: boolean; message: string; webhook: string | null } | null>(null);
const recentPushes = ref<NotificationRecord[]>([]);

async function loadPushes() {
  try {
    recentPushes.value = (await notifyApi.records({ limit: 5 })) ?? [];
  } catch {
    recentPushes.value = [];
  }
}

async function sendTest() {
  testing.value = true;
  testResult.value = null;
  try {
    // 成功/失败都带企业微信原文：配错 key 时群里是"静默不出现"，只有 errcode 查得动
    const result = await notifyApi.test();
    testResult.value = { ok: result.sent, message: result.message, webhook: result.webhook };
    if (result.sent) {
      ElMessage.success("测试消息已发送，请检查企业微信群");
    }
    await loadPushes();
  } catch (error) {
    testResult.value = {
      ok: false,
      message: error instanceof Error ? error.message : "请求失败",
      webhook: null
    };
  } finally {
    testing.value = false;
  }
}

const platformFields = [
  { key: "platform_name", label: "平台名称", type: "text" },
  { key: "refresh_interval", label: "数据刷新频率", type: "select", options: ["5", "10", "30", "60"], unit: "秒" },
  { key: "default_park_id", label: "默认地图园区", type: "park" },
  { key: "battery_threshold", label: "低电量阈值", type: "number", unit: "%" },
  { key: "task_timeout_minutes", label: "任务超时容忍", type: "number", unit: "分钟" },
  { key: "coordinate_system", label: "坐标系统", type: "select", options: COORDINATE_SYSTEMS }
];

const switches = [
  { key: "alert_push", title: "设备异常告警推送", desc: "告警中心站内提示的总开关（企业微信推送也受它约束）" },
  { key: "task_timeout_notice", title: "任务超时提醒", desc: "任务超时 / 卡住判异常时的站内提示总开关" },
  {
    key: "wecom_alert_push",
    title: "企业微信 · 设备异常告警",
    desc: "掉线、故障、急停、控制权抢占等告警推送到企业微信群"
  },
  {
    key: "wecom_task_abnormal",
    title: "企业微信 · 任务执行异常",
    desc: "任务被判定异常时推送到企业微信群（带设备、进度与失败原因）"
  },
  {
    key: "alert_auto_workorder",
    title: "告警自动派单（生成工单）",
    desc: "告警落库后按当天排班找当班责任人，自动建工单并 @ 到人；关闭后仍会解析责任人，只是不建单"
  },
  { key: "low_battery_recharge", title: "低电量自动回充", desc: "电量低于 20% 时自动下发回充任务" },
  { key: "daily_report_mail", title: "巡检日报邮件", desc: "每日 18:00 发送运行日报至运维组邮箱" },
  { key: "night_silent", title: "夜间静音模式", desc: "22:00 - 07:00 暂停非紧急告警声音提示" }
];



async function load() {
  const [settingList, parkList, stats] = await Promise.all([
    settingApi.list(),
    parkApi.list(),
    deviceApi.stats()
  ]);
  settings.value = settingList;
  parks.value = parkList;
  deviceTotal.value = stats.total;
  for (const setting of settingList) {
    values[setting.settingKey] = setting.settingValue ?? "";
  }
}

function toggle(key: string) {
  values[key] = values[key] === "true" ? "false" : "true";
}

async function save() {
  saving.value = true;
  try {
    const items = settings.value.map(setting => ({
      settingKey: setting.settingKey,
      settingValue: values[setting.settingKey] ?? setting.settingValue
    }));
    settings.value = await settingApi.save(items);
    ElMessage.success("平台配置已保存");
  } finally {
    saving.value = false;
  }
}

onMounted(async () => {
  await load();
  await loadPushes();
});
</script>

<template>
  <div>
    <PageHeader title="系统设置" subtitle="平台参数、告警策略与通知偏好" />

    <div class="grid row-2b">
      <div class="card">
        <div class="card-head">
          <div class="card-title"><AppIcon name="settings" />平台参数</div>
        </div>
        <div class="card-body">
          <div class="form-grid">
            <div v-for="field in platformFields" :key="field.key" class="field">
              <label>{{ field.label }}</label>
              <select
                v-if="field.type === 'select'"
                v-model="values[field.key]"
                class="select"
              >
                <option v-for="option in field.options" :key="option" :value="option">
                  {{ option }}{{ field.unit ? ` ${field.unit}` : "" }}
                </option>
              </select>
              <select
                v-else-if="field.type === 'park'"
                v-model="values[field.key]"
                class="select"
              >
                <option v-for="park in parks" :key="park.id" :value="String(park.id)">
                  {{ park.parkName }}
                </option>
              </select>
              <input
                v-else
                v-model="values[field.key]"
                class="input"
                :type="field.type === 'number' ? 'number' : 'text'"
              />
              <span v-if="field.type === 'number'" class="muted" style="font-size: 11px">
                {{ field.unit }}
              </span>
            </div>
          </div>
          <div class="divider" />
          <div style="display: flex; justify-content: flex-end">
            <button class="btn primary" :disabled="saving" @click="save">
              <AppIcon name="check" />{{ saving ? "保存中…" : "保存配置" }}
            </button>
          </div>
        </div>
      </div>

      <div class="card">
        <div class="card-head">
          <div class="card-title"><AppIcon name="bell" />通知策略</div>
        </div>
        <div class="card-body">
          <div v-for="item in switches" :key="item.key" class="setting-row">
            <div>
              <b>{{ item.title }}</b>
              <p>{{ item.desc }}</p>
            </div>
            <button
              class="switch"
              :class="{ on: values[item.key] === 'true' }"
              :aria-label="`切换 ${item.title}`"
              @click="toggle(item.key)"
            />
          </div>
          <div class="rt-hint" style="margin-top: 12px">
            企业微信推送需要先在服务端配置 <code>ROBODOG_WECOM_WEBHOOK</code>；
            未配置时下方开关只影响站内提示。
          </div>
        </div>
      </div>
    </div>

    <div class="card mt18">
      <div class="card-head">
        <div class="card-title"><AppIcon name="bell" />企业微信群机器人</div>
        <button class="btn" :disabled="testing" @click="sendTest">
          <AppIcon name="send" />{{ testing ? "发送中…" : "发送测试消息" }}
        </button>
      </div>
      <div class="card-body">
        <p class="muted" style="font-size: 12px; margin: 0 0 12px; line-height: 1.7">
          群里「添加群机器人」→ 复制 webhook 地址 → 写入服务端环境变量
          <code>ROBODOG_WECOM_WEBHOOK</code> 后重启服务。配好先点右上角按钮验证：
          消息会同步发出，失败时下方直接显示企业微信返回的错误码。
        </p>
        <div
          v-if="testResult"
          class="rt-hint"
          :class="testResult.ok ? 'ok' : 'warn'"
          style="margin-bottom: 12px"
        >
          {{ testResult.ok ? "✅" : "❌" }} {{ testResult.message }}
          <span v-if="testResult.webhook" class="muted">（{{ testResult.webhook }}）</span>
        </div>
        <div v-if="recentPushes.length" class="push-list">
          <div v-for="item in recentPushes" :key="item.id" class="push-row">
            <span class="push-badge" :class="item.status === 'sent' ? 'ok' : 'warn'">
              {{ item.status === "sent" ? "已发送" : "失败" }}
            </span>
            <span class="push-level">{{ item.level }}</span>
            <span class="push-type">{{
              { alert: "告警", task: "任务异常", summary: "积压汇总", test: "测试" }[item.refType] ??
              item.refType
            }}</span>
            <span class="muted push-time">{{ item.createdAt }}</span>
            <span v-if="item.lastError" class="push-error">{{ item.lastError }}</span>
          </div>
        </div>
        <p v-else class="muted" style="font-size: 12px; margin: 0">
          暂无推送记录（推送通道未配置或还没有触发过告警）。
        </p>
      </div>
    </div>

    <div class="card mt18">
      <div class="card-head">
        <div class="card-title"><AppIcon name="info" />关于平台</div>
      </div>
      <div class="card-body">
        <div class="grid g3">
          <div class="park-stat"><b>V 1.0.0</b><span>平台版本</span></div>
          <div class="park-stat"><b>{{ deviceTotal }} 台</b><span>接入设备</span></div>
          <div class="park-stat"><b>99.96 %</b><span>近 30 日可用性</span></div>
        </div>
        <p class="muted" style="font-size: 11.5px; margin: 14px 0 0; line-height: 1.7">
          本服务为 business 目录下的独立 RoboDog OPS 业务服务，包含独立 Rust 后端、
          SQLite 数据库与 Vue 3 运营控制台。页面数据来自业务数据库，设备控制指令记录在事件流中。
        </p>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 企业微信推送记录：一行一条，失败行直接把 errcode 摊开，现场照着改配置 */
.push-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.push-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 9px;
  border: 1px solid var(--line-2);
  border-radius: 8px;
  font-size: 11.5px;
  background: #fbfdff;
}
.push-badge {
  flex: 0 0 auto;
  padding: 1px 7px;
  border-radius: 6px;
  font-size: 10.5px;
  font-weight: 600;
}
.push-badge.ok {
  background: #e7f6ec;
  color: #1a7f43;
}
.push-badge.warn {
  background: #fdeaea;
  color: #b42318;
}
.push-level {
  flex: 0 0 auto;
  font-weight: 600;
}
.push-type {
  flex: 0 0 auto;
  color: var(--text-3);
}
.push-time {
  flex: 0 0 auto;
  font-size: 10.5px;
}
.push-error {
  flex: 1 1 auto;
  color: #b42318;
  font-family: var(--mono, monospace);
  font-size: 10.5px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.rt-hint.warn {
  background: #fff5f5;
  border-color: #f5d0ce;
  color: #b42318;
}
.rt-hint.ok {
  background: #f0f9f3;
  border-color: #cfe9d8;
  color: #1a7f43;
}
</style>
