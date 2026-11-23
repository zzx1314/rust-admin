<script setup lang="ts">
/**
 * MapShareDialog - 地图分享给其他设备
 *
 * 分享不复制地图包：一张已存在的中台地图（手工上传的也算）可以直接分享给
 * 其他设备使用。分享后：
 * - 分配给目标设备的中台用户也能在地图列表里看到这张图，不用重新上传；
 * - 「下发到狗体」可以把这张图下发到被分享的设备；
 * - 上报设备（`dogMap.deviceCode`）本身就是可用设备，不需要也不允许重复分享。
 */
import { computed, ref } from "vue";
import { dogMapListShares, dogMapShareDevices } from "@/api/dogMap";
import { dogDeviceGetAll } from "@/api/dogDevice";
import { SUCCESS } from "@/api/base";
import { message } from "@/utils/message";

defineOptions({ name: "MapShareDialog" });

const emit = defineEmits<{ saved: [] }>();

const visible = ref(false);
const loading = ref(false);
const saving = ref(false);
const currentMap = ref<any>(null);
const devices = ref<any[]>([]);
const selected = ref<string[]>([]);

/** 上报设备：本来就是可用设备，不在分享列表里重复出现。 */
const reportedDevice = computed(() => {
  const code = currentMap.value?.deviceCode;
  return typeof code === "string" ? code.trim() : "";
});

/**
 * 设备下拉：中台设备台账 + 已分享但当前不可见的历史设备。
 * 后者必须保留，否则「只换一台」保存时会把看不见的分享悄悄清掉。
 */
const deviceOptions = computed(() => {
  const options = devices.value.map(device => ({
    code: String(device.deviceId ?? ""),
    label: device.deviceName
      ? `${device.deviceId}（${device.deviceName}）`
      : String(device.deviceId ?? "")
  }));
  for (const code of selected.value) {
    if (!options.some(option => option.code === code)) {
      options.push({ code, label: code });
    }
  }
  return options.filter(
    option => option.code && option.code !== reportedDevice.value
  );
});

async function open(map: any) {
  currentMap.value = map;
  visible.value = true;
  loading.value = true;
  selected.value = [];
  try {
    const [deviceRes, shareRes] = await Promise.all([
      dogDeviceGetAll(),
      dogMapListShares(map.id)
    ]);
    devices.value = Array.isArray(deviceRes.data) ? deviceRes.data : [];
    selected.value = (shareRes.data ?? []).map(
      (share: { deviceCode: string }) => share.deviceCode
    );
  } finally {
    loading.value = false;
  }
}

async function save() {
  if (!currentMap.value || saving.value) return;
  saving.value = true;
  try {
    const res = await dogMapShareDevices(
      currentMap.value.id,
      selected.value,
      "replace"
    );
    if (res.code !== SUCCESS) {
      message(res.msg || "分享失败", { type: "error" });
      return;
    }
    const codes: string[] = res.data?.sharedDeviceCodes ?? [];
    message(
      codes.length
        ? `已分享给 ${codes.length} 台设备：${codes.join("、")}`
        : "已取消全部分享",
      { type: "success" }
    );
    visible.value = false;
    emit("saved");
  } finally {
    saving.value = false;
  }
}

defineExpose({ open });
</script>

<template>
  <el-dialog
    v-model="visible"
    :title="`分享地图 - ${currentMap?.mapName ?? ''}`"
    width="620px"
    append-to-body
  >
    <el-alert
      type="info"
      :closable="false"
      show-icon
      title="分享不复制地图包"
      description="被分享的设备直接使用这张地图。分配给该设备的中台用户也能在地图列表里看到它，下发到狗体时可选这台设备。"
    />
    <div v-loading="loading" class="share-body">
      <div class="share-row">
        <span class="share-label">上报设备</span>
        <span class="font-mono">
          {{ reportedDevice || "无（手工上传）" }}
        </span>
      </div>
      <div class="share-row">
        <span class="share-label">分享给设备</span>
        <el-select
          v-model="selected"
          multiple
          filterable
          clearable
          collapse-tags
          collapse-tags-tooltip
          placeholder="选择要使用这张地图的其他设备"
          style="width: 100%"
        >
          <el-option
            v-for="option in deviceOptions"
            :key="option.code"
            :label="option.label"
            :value="option.code"
          />
        </el-select>
      </div>
      <div class="share-tip">
        清空后保存即取消全部分享；地图本身与地图包不受影响。
      </div>
    </div>
    <template #footer>
      <el-button @click="visible = false">取消</el-button>
      <el-button type="primary" :loading="saving" @click="save">
        保存分享
      </el-button>
    </template>
  </el-dialog>
</template>

<style lang="scss" scoped>
.share-body {
  display: flex;
  flex-direction: column;
  gap: 14px;
  margin-top: 12px;
}

.share-row {
  display: flex;
  gap: 12px;
  align-items: center;
}

.share-label {
  flex: none;
  width: 84px;
  color: var(--el-text-color-regular);
}

.share-tip {
  font-size: var(--dog-font-sm);
  line-height: var(--dog-line-body);
  color: var(--dog-text-secondary);
}
</style>
