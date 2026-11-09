<script setup lang="ts">
import { onMounted, reactive, ref } from "vue";
import { ElMessage } from "element-plus";
import { deviceApi, parkApi, settingApi } from "@/api/ops";
import type { Park, Setting } from "@/api/types";
import PageHeader from "@/components/PageHeader.vue";
import AppIcon from "@/components/AppIcon.vue";
import { COORDINATE_SYSTEMS } from "@/utils/constants";

const settings = ref<Setting[]>([]);
const parks = ref<Park[]>([]);
const deviceTotal = ref(0);
const values = reactive<Record<string, string>>({});
const saving = ref(false);

const platformFields = [
  { key: "platform_name", label: "平台名称", type: "text" },
  { key: "refresh_interval", label: "数据刷新频率", type: "select", options: ["5", "10", "30", "60"], unit: "秒" },
  { key: "default_park_id", label: "默认地图园区", type: "park" },
  { key: "battery_threshold", label: "低电量阈值", type: "number", unit: "%" },
  { key: "task_timeout_minutes", label: "任务超时容忍", type: "number", unit: "分钟" },
  { key: "coordinate_system", label: "坐标系统", type: "select", options: COORDINATE_SYSTEMS }
];

const switches = [
  { key: "alert_push", title: "设备异常告警推送", desc: "设备触发故障 / 温度异常时推送站内通知" },
  { key: "task_timeout_notice", title: "任务超时提醒", desc: "任务超过计划结束时间 10 分钟未完成时提醒" },
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

onMounted(load);
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
            通知策略仅保存平台侧开关状态，邮件与短信通道可在后续接入。
          </div>
        </div>
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
