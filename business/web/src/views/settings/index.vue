<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { ElMessage } from "element-plus";
import { deviceApi, notifyApi, parkApi, settingApi } from "@/api/ops";
import type { NotificationRecord, NotifyWebhookStatus, Park, Setting } from "@/api/types";
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
// webhook 在平台内配置：接口只回掩码，完整地址永远不回到浏览器
const webhookStatus = ref<NotifyWebhookStatus | null>(null);
const webhookInput = ref("");
const savingWebhook = ref(false);
// 点「显示」才向后端要一次原文；平时只有掩码，避免凭据随手进截图 / 录屏
const revealedWebhook = ref<string | null>(null);
const revealingWebhook = ref(false);

const sourceLabel = computed(() => {
  switch (webhookStatus.value?.source) {
    case "db":
      return "平台配置";
    case "env":
      return "服务端环境变量";
    default:
      return "未配置";
  }
});

async function loadWebhook() {
  try {
    webhookStatus.value = await notifyApi.webhook();
  } catch {
    webhookStatus.value = null;
  }
}

/** 当前生效地址的简述：用在输入框 placeholder，避免「看起来是空的」。 */
const currentHint = computed(() => {
  switch (webhookStatus.value?.source) {
    case "db":
      return `当前已配置（平台配置）：${webhookStatus.value.masked ?? "…"}`;
    case "env":
      return `当前已配置（服务端环境变量）：${webhookStatus.value.masked ?? "…"}`;
    default:
      return "还没有配置地址：粘贴企业微信群机器人 webhook 后保存";
  }
});

async function revealWebhook() {
  if (revealedWebhook.value) {
    revealedWebhook.value = null;
    return;
  }
  revealingWebhook.value = true;
  try {
    const status = await notifyApi.webhook(true);
    revealedWebhook.value = status.url ?? null;
    webhookStatus.value = status;
  } finally {
    revealingWebhook.value = false;
  }
}

/** 保存（`clear` = 清除平台配置、回退环境变量）。改完立即生效，不用重启服务。 */
async function saveWebhook(clear = false) {
  const value = clear ? "" : webhookInput.value.trim();
  if (!clear && !value) {
    ElMessage.warning("请先粘贴企业微信群机器人地址");
    return;
  }
  savingWebhook.value = true;
  try {
    webhookStatus.value = await notifyApi.saveWebhook(value);
    webhookInput.value = "";
    revealedWebhook.value = null;
    ElMessage.success(
      clear ? "已清除平台配置（若服务端配了环境变量则回退到它）" : "地址已保存，可点「发送测试消息」验证"
    );
  } catch {
    // 接口 4xx 的提示（如缺少 key=）已由 axios 拦截器弹出
  } finally {
    savingWebhook.value = false;
  }
}

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
    desc: "执行计划期间（设备有执行中的任务）产生的告警推送到企业微信群；掉线、控制权抢占、页面手动新建的告警只落库，不进群"
  },
  {
    key: "wecom_alert_repeat_push",
    title: "企业微信 · 重复告警也推（跟单）",
    desc: "同一问题已挂在在办工单上时仍每条都推（每次任务执行都算一次告警）；关掉则 24 小时内同一问题只推第一条"
  },
  {
    key: "wecom_alert_image",
    title: "企业微信 · 告警证据图",
    desc: "AI 识别数量核对不一致时，把板端抓拍的证据图作为图片消息跟着卡片一起发（原图 ≤2MB、JPG/PNG）；关掉只发文字卡片"
  },
  {
    key: "wecom_task_abnormal",
    title: "企业微信 · 任务执行异常",
    desc: "任务被判定异常时推送到企业微信群（带设备、进度与失败原因）"
  },
  {
    key: "alert_auto_workorder",
    title: "告警自动派单（生成工单）",
    desc: "执行计划期间的告警按当天排班找当班责任人，自动建工单并 @ 到人；关闭后仍会解析责任人，只是不建单"
  },
  {
    key: "alert_only_in_task",
    title: "只为执行计划期间的告警建单 + 推群",
    desc: "开启（默认）：只有设备正在执行任务时产生的告警才自动建工单、推到企业微信群；掉线、控制权抢占、手动新建等只落库（告警中心照常可见）。关闭则退回「所有告警都建单 + 推群」的老口径"
  },
  { key: "low_battery_recharge", title: "低电量自动回充", desc: "电量低于 20% 时自动下发回充任务" },
  { key: "daily_report_mail", title: "巡检日报邮件", desc: "每日 18:00 发送运行日报至运维组邮箱" },
  { key: "night_silent", title: "夜间静音模式", desc: "22:00 - 07:00 暂停非紧急告警声音提示" }
];

/** 中继默认档位：改完立即生效（平台随 `video_start` 下发 / 保存时广播 `video_config`）。
 *
 *  现场板子出公网主要走 4G 模块（实测上行 0.25~0.6Mbps），720p/1000k 起流就会被反压，
 *  所以默认是省流档；链路更差时板端还会再降到极省流档（慢刷新，但不断线）。
 */
const relayTiers = [
  {
    value: "light",
    label: "高清 720p15 / 1000k",
    hint: "画质最好；需要稳定 ≥2Mbps 上行（Wi-Fi / 有线），4G 模块基本撑不住"
  },
  {
    value: "degrade",
    label: "省流 640x360@12 / 500k（推荐 4G）",
    hint: "4G 模块的主力档位；链路更差时自动再降到极省流档"
  },
  {
    value: "degrade2",
    label: "极省流 320x180@8 / 200k",
    hint: "弱 4G / 流量卡：约 2~3fps 慢刷新，但不会反复断线重连"
  }
];

const currentRelayTier = computed(
  () => relayTiers.find(tier => tier.value === values.video_relay_profile) ?? relayTiers[1]
);

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
  await Promise.all([loadPushes(), loadWebhook()]);
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
            企业微信推送地址在下方「企业微信群机器人」里配置，填好即生效；
            未配置时上方开关只影响站内提示。
          </div>
        </div>
      </div>
    </div>

    <div class="card mt18">
      <div class="card-head">
        <div class="card-title"><AppIcon name="video" />视频中继</div>
        <button class="btn" :disabled="saving" @click="save">
          <AppIcon name="check" />{{ saving ? "保存中…" : "保存档位" }}
        </button>
      </div>
      <div class="card-body">
        <p class="muted" style="font-size: 12px; margin: 0 0 12px; line-height: 1.7">
          背包实时视频走「平台中继」时，板端起流的<b>默认档位</b>在这里定。现场出公网主要走
          4G 模块，默认用省流档（640x360@12 / 500k）；链路更差时板端会自己再降一档。
          改完保存立即生效，正在看的流会在几秒内就地换档（播放器不重连，只短暂等一个关键帧）。
        </p>
        <div class="form-grid">
          <div class="field">
            <label>默认档位</label>
            <select v-model="values.video_relay_profile" class="select">
              <option v-for="tier in relayTiers" :key="tier.value" :value="tier.value">
                {{ tier.label }}
              </option>
            </select>
            <span class="muted" style="font-size: 11px">{{ currentRelayTier.hint }}</span>
          </div>
        </div>
        <div class="divider" />
        <div class="setting-row">
          <div>
            <b>按上行带宽自动降档</b>
            <p>
              开启（默认）：板端发现上行吃不下当前档位时自动再降一档；关闭则锁死在上面选定的
              档位，只保留「上行彻底断掉」时的重连保护（固定流量套餐 / 只想要稳定画质时用）。
            </p>
          </div>
          <button
            class="switch"
            :class="{ on: values.video_relay_auto_degrade === 'true' }"
            aria-label="切换中继自动降档"
            @click="toggle('video_relay_auto_degrade')"
          />
        </div>
        <div class="rt-hint" style="margin-top: 12px">
          档位对应的分辨率 / 帧率 / 码率由板端 <code>backpack-relay</code> 的
          <code>RELAY_*</code> 环境变量细化（默认：高清 1280x720@20/1200k、省流
          640x360@12/500k、极省流 320x180@8/200k）。板端 relay 未升级到支持页内配置的
          版本时，这个设置不生效，仍用板端环境变量里的档位。
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
          企微群右上角「···」→ 添加群机器人 → 复制 webhook 地址 → 粘贴到下面保存即可
          （地址本身就是凭据，页面只显示掩码）。保存后不用重启服务，
          先点右上角「发送测试消息」验证：失败会直接显示企业微信返回的 errcode。
        </p>
        <div class="webhook-box">
          <div class="webhook-status">
            <span class="push-badge" :class="webhookStatus?.configured ? 'ok' : 'warn'">
              {{ webhookStatus?.configured ? "已配置" : "未配置" }}
            </span>
            <span class="muted">来源：{{ sourceLabel }}</span>
            <code v-if="webhookStatus?.masked" class="webhook-masked">{{
              webhookStatus.masked
            }}</code>
            <button
              v-if="webhookStatus?.configured"
              class="btn btn-mini"
              :disabled="revealingWebhook"
              @click="revealWebhook()"
            >
              {{ revealedWebhook ? "隐藏原文" : "显示原文" }}
            </button>
          </div>
          <div v-if="revealedWebhook" class="webhook-reveal">
            <code>{{ revealedWebhook }}</code>
            <span class="muted">仅本次显示，刷新页面即消失</span>
          </div>
          <div class="webhook-form">
            <input
              v-model="webhookInput"
              class="input"
              :placeholder="currentHint"
              @keyup.enter="saveWebhook()"
            />
            <button class="btn primary" :disabled="savingWebhook" @click="saveWebhook()">
              {{ savingWebhook ? "保存中…" : "保存地址" }}
            </button>
            <button
              class="btn"
              :disabled="savingWebhook || webhookStatus?.source !== 'db'"
              @click="saveWebhook(true)"
            >
              清除
            </button>
          </div>
          <p class="muted" style="font-size: 11.5px; margin: 8px 0 0; line-height: 1.7">
            输入框留空<b>不会</b>清除已有配置 —— 只有粘贴新地址并保存才会覆盖；
            点「显示原文」可以找回上次填的完整地址（仅管理员，日志里不记地址）。
          </p>
          <p class="muted" style="font-size: 11.5px; margin: 8px 0 0; line-height: 1.7">
            平台配置优先于部署时的环境变量 <code>ROBODOG_WECOM_WEBHOOK</code>；
            「清除」后回退到环境变量。改了群机器人（删掉再重建会换 key）记得回来重新粘一次。
          </p>
        </div>
        <div v-if="webhookStatus && !webhookStatus.enabled" class="rt-hint warn" style="margin-bottom: 12px">
          部署级总开关已关闭（<code>wecom.enabled = false</code>）：地址会保存，但不会真的发消息。
        </div>
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
              { alert: "告警", task: "任务异常", summary: "积压汇总", test: "测试", count: "数量核对回执" }[item.refType] ??
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
/* webhook 配置区：地址即凭据，只展示掩码 + 来源 */
.webhook-box {
  margin: 0 0 14px;
  padding: 10px 12px;
  border: 1px solid var(--line-2);
  border-radius: 10px;
  background: #fbfdff;
}
.webhook-status {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
  font-size: 11.5px;
}
.webhook-masked {
  font-family: var(--mono, monospace);
  font-size: 11px;
  color: var(--text-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* 「显示原文」：仅本次显示，不落 localStorage */
.webhook-reveal {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
  padding: 6px 8px;
  border: 1px dashed var(--line-2);
  border-radius: 8px;
  background: #fff;
}
.webhook-reveal code {
  flex: 1 1 auto;
  font-family: var(--mono, monospace);
  font-size: 11px;
  word-break: break-all;
}
.webhook-reveal .muted {
  flex: 0 0 auto;
  font-size: 10.5px;
}
.btn-mini {
  padding: 2px 8px;
  font-size: 11px;
  line-height: 1.5;
}
.webhook-form {
  display: flex;
  gap: 8px;
}
.webhook-form .input {
  flex: 1 1 auto;
  min-width: 0;
}
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
