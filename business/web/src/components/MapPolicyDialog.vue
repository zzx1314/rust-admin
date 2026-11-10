<script setup lang="ts">
import AppIcon from "@/components/AppIcon.vue";

withDefaults(defineProps<{ modelValue: boolean; mode?: "policy" | "lineage" }>(), {
  mode: "policy"
});

const emit = defineEmits<{ "update:modelValue": [value: boolean] }>();

const apis = [
  ["GET", "/api/parks/{parkId}/maps?status=published", "园区页基线徽章、新建路线版本下拉"],
  ["GET", "/api/map-versions/{id}", "基线详情：frame_id / CRS / 原点 / 分辨率 / hash"],
  ["GET", "/api/map-versions/{id}/raster · /tiles/{z}/{x}/{y}", "画布底图渲染"],
  ["GET", "/api/map-versions/{id}/semantics", "道路 / 禁行区 / 区域 GeoJSON 图层"],
  ["GET", "/api/map-versions/{id}/impact", "发布新版前的受影响路线 / 任务清单"],
  ["GET", "/api/devices/{id}/map-state", "设备当前 map_hash 与定位状态"],
  ["POST", "/api/map-jobs", "发起建图或重标定工单（不直接写版本）"],
  ["POST", "/api/map-versions/{id}/publish", "地图管理台发布，前端不调用"]
];

const lineageSteps = [
  [
    "机器狗建图采集",
    "下发「地图采集」任务，设备走遍园区回传 rosbag / pose graph / 点云。原型中的「一期周界全景建图」「地下车库建图补采」就是这一步",
    "机器人侧",
    "blue"
  ],
  [
    "地图服务离线建图与标定",
    "Cartographer / SLAM Toolbox / 厂商 SDK 生成 occupancy grid，标定原点、朝向、比例尺，分配 frame_id，计算 map_hash",
    "地图服务",
    "purple"
  ],
  [
    "叠加语义图层",
    "导入 GIS/CAD 总平图，运营人员在地图上框选道路、禁行区、充电座、电梯与楼层连接点，输出 GeoJSON",
    "GIS + 人工",
    "teal"
  ],
  [
    "发布不可变 MapVersion",
    "后端版本库落库，置为「已发布」，旧版本保留归档。这一步不属于前端，前端最多发起建图/重标定工单",
    "地图服务",
    "blue"
  ],
  [
    "前端只读消费 + 下发校验",
    "前端只取元数据、栅格/瓦片、语义图层三类数据；任务下发时设备比对自身 map_hash，不一致拒绝执行",
    "前端 + 设备",
    "orange"
  ]
];
</script>

<template>
  <el-dialog
    :model-value="modelValue"
    :title="mode === 'policy' ? '园区与地图的绑定时机' : '地图基线的数据来源'"
    width="820px"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <template v-if="mode === 'policy'">
      <div class="muted" style="font-size: 11.5px; margin-bottom: 12px">
        坐标系是路线的前置依赖，不是路线的附属配置。
      </div>
      <div class="policy-flow">
        <div class="policy-step">
          <span class="policy-n">1</span>
          <div>
            <b>园区建档时 · 首次绑定（必需）</b>
            <p>
              绑定坐标系定义：frame_id、原点、分辨率、CRS、楼层、道路与禁行区。此时园区从「资产台账」变成「可规划空间」。未绑定
              → 状态为「仅台账」，禁用一切路线与巡检任务入口。
            </p>
          </div>
        </div>
        <div class="policy-step">
          <span class="policy-n">2</span>
          <div>
            <b>地图标定后 · 发布不可变版本</b>
            <p>
              每次建图／重标定产生新 mapVersion，旧版本不删除、不覆盖。园区只持有一个「当前生效基线」指针，供新建路线使用。
            </p>
          </div>
        </div>
        <div class="policy-step">
          <span class="policy-n">3</span>
          <div>
            <b>创建路线时 · 冻结版本引用（不再绑定）</b>
            <p>
              路线落库写入 mapVersionId + frame_id + 坐标单位，之后引用冻结。地图发新版不会自动迁移历史路线，必须走影响分析
              → 人工确认 → 灰度 → 旧版保留至引用清零。
            </p>
          </div>
        </div>
        <div class="policy-step">
          <span class="policy-n">4</span>
          <div>
            <b>任务下发时 · 校验一致性</b>
            <p>
              下发包携带路线 + mapVersion + 地图 hash。设备执行前校验自身 SLAM 地图 hash，不一致则拒绝执行并告警，避免「拿
              A 版地图跑 B 版路线」。
            </p>
          </div>
        </div>
      </div>
      <div class="rt-hint">
        为什么不能后绑：航点坐标脱离坐标系只是一串无意义数字；补绑时已无法获知旧坐标的原点与比例尺，坐标迁移不可逆。多楼层、禁行区冲突、续航估算也全部依赖这一层。
      </div>
    </template>

    <template v-else>
      <div class="rt-hint" style="margin: 0 0 12px">
        前端不生产地图数据，只消费已发布的版本。标定、坐标转换与版本发布都在机器人侧 / 后端地图服务完成。
      </div>
      <div class="policy-flow">
        <div v-for="(step, index) in lineageSteps" :key="step[0]" class="policy-step">
          <span class="policy-n">{{ index + 1 }}</span>
          <div style="flex: 1">
            <b>{{ step[0] }}</b>
            <p>{{ step[1] }}</p>
          </div>
          <span class="tag" :class="step[3]" style="flex: none; align-self: flex-start">{{
            step[2]
          }}</span>
        </div>
      </div>
      <div class="card-head" style="margin-top: 18px">
        <div class="card-title">
          <AppIcon name="cpu" />前端需要对接的接口
          <span class="card-sub">三类消费数据：元数据 / 栅格瓦片 / 语义图层</span>
        </div>
      </div>
      <div class="table-wrap">
        <table class="table">
          <thead>
            <tr>
              <th>方法</th>
              <th>接口</th>
              <th>用途</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="api in apis" :key="api[1]">
              <td>
                <span class="tag" :class="api[0] === 'GET' ? 'blue' : 'purple'">{{ api[0] }}</span>
              </td>
              <td class="mono" style="font-size: 11px">{{ api[1] }}</td>
              <td>{{ api[2] }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="rt-hint">
        <b>职责边界</b> · frame_id、resolution、origin、map_hash 由建图服务生成，前端只读展示，不应手填。
      </div>
    </template>

    <template #footer>
      <button class="btn" @click="emit('update:modelValue', false)">关闭</button>
    </template>
  </el-dialog>
</template>
