<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { ElMessage } from "element-plus";
import { mapApi } from "@/api/ops";
import type { MapRaster, MapRefs, MapVersion } from "@/api/types";
import AppIcon from "@/components/AppIcon.vue";
import MapChip from "@/components/MapChip.vue";

const props = withDefaults(
  defineProps<{ modelValue: boolean; mapId?: string | null }>(),
  { mapId: null }
);
const emit = defineEmits<{
  "update:modelValue": [value: boolean];
  updated: [map: MapVersion];
  "data-source": [];
}>();

const loading = ref(false);
const refsData = ref<MapRefs | null>(null);
const raster = ref<MapRaster | null>(null);
const view = ref<"detail" | "impact">("detail");
const strategy = ref("保留旧版本继续运行，仅新建路线使用新版");
const note = ref("园区二期改造后重新建图标定");
const submitting = ref(false);

const map = computed(() => refsData.value?.map ?? null);
const routes = computed(() => refsData.value?.routes ?? []);
const tasks = computed(() => refsData.value?.tasks ?? []);

const sourceRows = computed(() => {
  const item = map.value;
  if (!item) return [];
  return [
    ["所属园区", item.parkName ?? "—", "运营台账录入", "orange"],
    [
      "地图来源",
      item.source === "backend" ? "backend 地图库导入" : "本业务库",
      item.source === "backend" ? "根 backend dog_map" : "业务库",
      item.source === "backend" ? "teal" : "gray"
    ],
    [
      "地图包",
      item.fileName
        ? `${item.fileName}${item.fileSize ? ` · ${(item.fileSize / 1024).toFixed(0)} KB` : ""}`
        : "—",
      "backend 上传",
      "orange"
    ],
    ["frame_id", item.frameId ?? "—", "建图服务分配", "purple"],
    ["CRS", item.crs ?? "—", "系统配置 + RTK 打点", "teal"],
    ["原点锚点", item.origin ?? "—", "RTK / 人工标定", "teal"],
    ["分辨率", item.resolution ?? "—", "SLAM 栅格参数", "purple"],
    ["覆盖楼层", item.floors ?? "—", "楼层元数据", "blue"],
    [
      "语义图层",
      `${item.roadCount} 条道路 / ${item.noGoCount} 个禁行区`,
      "GIS/CAD + 人工标注",
      "orange"
    ],
    ["map_hash", item.mapHash || "缺失（禁止下发）", "地图服务", "purple"],
    ["版本 / 状态", `${item.version} · ${item.status}`, "版本库", "blue"],
    ["发布时间", item.publishedAt ?? "—", "版本库", "blue"],
    [
      "引用情况",
      `${item.routeCount} 条路线 · ${item.taskCount} 个任务`,
      "业务库聚合",
      "blue"
    ]
  ] as const;
});

async function load() {
  if (!props.mapId) {
    refsData.value = null;
    raster.value = null;
    return;
  }
  loading.value = true;
  raster.value = null;
  try {
    refsData.value = await mapApi.refs(props.mapId);
    const item = refsData.value.map;
    if (item.source === "backend" && item.filePath) {
      try {
        raster.value = await mapApi.raster(item.id);
      } catch {
        raster.value = null;
      }
    }
  } catch {
    refsData.value = null;
  } finally {
    loading.value = false;
  }
}

watch(
  () => props.modelValue,
  visible => {
    if (visible) {
      view.value = "detail";
      void load();
    }
  },
  { immediate: true }
);

watch(
  () => props.mapId,
  () => {
    if (props.modelValue) void load();
  }
);

async function publishNewVersion() {
  if (!props.mapId) return;
  submitting.value = true;
  try {
    const created = await mapApi.newVersion(props.mapId, note.value);
    ElMessage.success(`地图 ${created.id} 已发布；历史路线保持旧版引用`);
    emit("updated", created);
    view.value = "detail";
    await load();
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <el-dialog
    :model-value="modelValue"
    :title="view === 'detail' ? '地图基线详情' : '地图版本变更 · 影响分析'"
    width="900px"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div v-loading="loading" class="map-baseline">
      <template v-if="map">
        <div class="map-baseline-head">
          <MapChip :map-id="map.id" :status="map.status" />
          <span class="muted">{{ map.parkName }} · {{ map.note }}</span>
        </div>

        <template v-if="view === 'detail'">
          <div class="rt-hint" style="margin: 10px 0 12px">
            本页为<b>只读</b>，所有值来自后端地图服务已发布的版本。「数据来源」标明每个字段由谁产生，前端不生产地图数据。
          </div>
          <div class="table-wrap">
            <table class="table">
              <thead>
                <tr><th>字段</th><th>当前值</th><th>数据来源</th></tr>
              </thead>
              <tbody>
                <tr v-for="row in sourceRows" :key="row[0]">
                  <td>{{ row[0] }}</td>
                  <td><span :class="{ mono: row[0] !== '所属园区' }">{{ row[1] }}</span></td>
                  <td><span class="tag" :class="row[3]">{{ row[2] }}</span></td>
                </tr>
              </tbody>
            </table>
          </div>

          <div v-if="raster" class="map-raster-preview">
            <div class="map-raster-head">
              <b><AppIcon name="layers" />ROS 栅格地图</b>
              <span class="muted mono">
                {{ raster.width }} × {{ raster.height }} px · {{ raster.resolution }} · origin
                {{ raster.origin }}
              </span>
            </div>
            <img :src="raster.image" alt="ROS model.pgm 栅格地图" />
          </div>

          <div class="card-head" style="margin-top: 16px">
            <div class="card-title">
              <AppIcon name="route" />引用该基线的路线
              <span class="card-sub">版本不可变，发布新版不会自动迁移这些路线</span>
            </div>
          </div>
          <div class="table-wrap">
            <table class="table">
              <thead>
                <tr><th>路线编号</th><th>路线名称</th><th>状态</th><th>版本</th><th>航点</th></tr>
              </thead>
              <tbody>
                <tr v-for="route in routes" :key="route.id">
                  <td class="mono">{{ route.routeCode }}</td>
                  <td>{{ route.routeName }}</td>
                  <td>{{ route.status }}</td>
                  <td class="mono">{{ route.version }}</td>
                  <td>{{ route.pointCount }} 个</td>
                </tr>
                <tr v-if="!routes.length">
                  <td colspan="5"><div class="empty-box">暂无路线引用该版本</div></td>
                </tr>
              </tbody>
            </table>
          </div>

          <div class="actions" style="margin-top: 14px">
            <button class="btn" @click="emit('data-source')">
              <AppIcon name="cpu" />数据来源与接口
            </button>
            <button class="btn" @click="view = 'impact'">
              <AppIcon name="refresh" />发布新版本 / 查看变更影响
            </button>
          </div>
        </template>

        <template v-else>
          <div class="rt-hint" style="margin-top: 12px">
            发布新版本不会改写任何历史路线的坐标。以下引用对象需逐条确认迁移策略，旧版本将保留至引用清零后归档。
          </div>
          <div class="table-wrap" style="margin-top: 14px">
            <table class="table">
              <thead>
                <tr><th>路线编号</th><th>路线名称</th><th>航点数</th><th>处理状态</th></tr>
              </thead>
              <tbody>
                <tr v-for="route in routes" :key="route.id">
                  <td class="mono">{{ route.routeCode }}</td>
                  <td>{{ route.routeName }}</td>
                  <td>{{ route.pointCount }} 个</td>
                  <td><span class="tag orange">待人工确认迁移</span></td>
                </tr>
                <tr v-if="!routes.length">
                  <td colspan="4"><div class="empty-box">该版本无引用路线，可直接发布</div></td>
                </tr>
              </tbody>
            </table>
          </div>

          <div class="form-grid" style="margin-top: 16px">
            <div class="field" style="grid-column: 1 / -1">
              <label>坐标迁移策略</label>
              <select v-model="strategy" class="select">
                <option>仿射变换自动迁移（原点/比例尺变化时）</option>
                <option>保留旧版本继续运行，仅新建路线使用新版</option>
                <option>作废并重绘受影响路线</option>
              </select>
            </div>
            <div class="field" style="grid-column: 1 / -1">
              <label>变更说明</label>
              <input v-model="note" class="input" />
            </div>
          </div>
          <div class="rt-hint">
            影响范围：{{ routes.length }} 条路线 · {{ tasks.length }} 个关联任务 ·
            {{ refsData?.deviceCount ?? 0 }} 台设备需同步地图包。下发前设备会校验地图 hash。
          </div>
          <div class="actions" style="margin-top: 14px">
            <button class="btn" @click="view = 'detail'">返回详情</button>
            <button class="btn primary" :disabled="submitting" @click="publishNewVersion">
              <AppIcon name="send" />{{ submitting ? "发布中…" : "发布新版本" }}
            </button>
          </div>
        </template>
      </template>
      <div v-else-if="!loading" class="empty-box">该园区尚未绑定地图基线</div>
    </div>
  </el-dialog>
</template>
