<script setup lang="ts">
import { ref } from "vue";
import type { FormInstance } from "element-plus";
import { useDogRoute } from "./hook";
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import AddFill from "~icons/ri/add-circle-line";
import More from "~icons/ep/more-filled";
import DropperLine from "~icons/ri/dropper-line";
import DogTableActions from "@/components/DogUI/DogTableActions.vue";
import { PureTableBar } from "@/components/RePureTableBar";
import PureTable from "@pureadmin/table";
import DogSearchForm from "@/components/DogUI/DogSearchForm.vue";

defineOptions({ name: "DogRoute" });

const addFormRef = ref<FormInstance>();
const {
  buttonClass,
  queryForm,
  dataList,
  loading,
  dialogFormVisible,
  title,
  pagination,
  addForm,
  rules,
  columns,
  queryFields,
  moreQueryFields,
  mapOptions,
  parkOptions,
  syncMapOptions,
  baselineVisible,
  baselineLoading,
  baselineInfo,
  baselineRefs,
  openBaseline,
  onSearch,
  handleUpdate,
  handleDelete,
  designRoute,
  handleSizeChange,
  handleCurrentChange,
  handleSelectionChange,
  handleSubmit,
  cancel,
  openDia
} = useDogRoute();
</script>
<template>
  <div class="main">
    <DogSearchForm
      v-model="queryForm"
      :fields="queryFields"
      :more-fields="moreQueryFields"
      :loading="loading"
      @search="onSearch"
      @reset="cancel"
    />
    <PureTableBar title="路线列表" :columns="columns" @refresh="onSearch">
      <template #buttons>
        <el-button
          type="primary"
          :icon="useRenderIcon(AddFill)"
          @click="openDia('新增路线', addFormRef)"
        >
          新增
        </el-button>
      </template>
      <template v-slot="{ size, checkList, dynamicColumns }">
        <pure-table
          border
          adaptive
          align-whole="center"
          showOverflowTooltip
          table-layout="auto"
          :loading="loading"
          :size="size"
          :data="dataList"
          :columns="dynamicColumns"
          :checkList="checkList"
          :pagination="pagination"
          :paginationSmall="size === 'small'"
          :header-cell-style="{
            background: 'var(--el-table-row-hover-bg-color)',
            color: 'var(--el-text-color-primary)'
          }"
          @selection-change="handleSelectionChange"
          @page-size-change="handleSizeChange"
          @page-current-change="handleCurrentChange"
        >
          <template #operation="{ row }">
            <DogTableActions
              :row="row"
              :size="size"
              @edit="handleUpdate(row, addFormRef)"
              @delete="handleDelete(row)"
            >
              <template #extra="{ row: r, size: s }">
                <el-dropdown class="ml-2">
                  <el-button
                    class="ml-4 mt-0.5"
                    link
                    type="primary"
                    :size="s"
                    :icon="useRenderIcon(More)"
                  />
                  <template #dropdown>
                    <el-dropdown-menu>
                      <el-dropdown-item>
                        <el-button
                          :class="buttonClass"
                          link
                          type="primary"
                          :size="s"
                          :icon="useRenderIcon(DropperLine)"
                          @click="openBaseline(r)"
                        >
                          地图基线
                        </el-button>
                      </el-dropdown-item>
                      <el-dropdown-item>
                        <el-button
                          :class="buttonClass"
                          link
                          type="primary"
                          :size="s"
                          :icon="useRenderIcon(DropperLine)"
                          @click="designRoute(r)"
                        >
                          设计路线
                        </el-button>
                      </el-dropdown-item>
                    </el-dropdown-menu>
                  </template>
                </el-dropdown>
              </template>
            </DogTableActions>
          </template>
        </pure-table>
      </template>
    </PureTableBar>

    <el-dialog
      v-model="dialogFormVisible"
      :title="title"
      width="600px"
      @close="cancel"
    >
      <el-form
        ref="addFormRef"
        :model="addForm"
        :rules="rules"
        label-width="100px"
      >
        <el-form-item label="路线名称" prop="routeName">
          <el-input v-model="addForm.routeName" placeholder="请输入路线名称" />
        </el-form-item>
        <el-form-item label="路线类型" prop="routeType">
          <el-select
            v-model="addForm.routeType"
            placeholder="请选择路线类型"
            style="width: 100%"
          >
            <el-option label="在线地图" value="在线地图" />
            <el-option label="离线地图" value="离线地图" />
          </el-select>
        </el-form-item>
        <el-form-item label="路线状态" prop="routeStatus">
          <el-select
            v-model="addForm.routeStatus"
            placeholder="请选择路线状态"
            style="width: 100%"
          >
            <el-option label="启用" value="启用" />
            <el-option label="停用" value="停用" />
          </el-select>
        </el-form-item>
        <el-form-item label="路线描述" prop="routeDescription">
          <el-input
            v-model="addForm.routeDescription"
            type="textarea"
            placeholder="请输入路线描述"
          />
        </el-form-item>
        <el-form-item label="所属园区" prop="parkId">
          <el-select
            v-model="addForm.parkId"
            placeholder="请选择所属园区"
            clearable
            style="width: 100%"
            @change="syncMapOptions"
          >
            <el-option
              v-for="opt in parkOptions"
              :key="opt.value"
              :label="opt.label"
              :value="opt.value"
            />
          </el-select>
        </el-form-item>
        <el-form-item label="地图基线" prop="mapId">
          <el-select
            v-model="addForm.mapId"
            placeholder="请先选择园区，再选择已发布的地图基线"
            clearable
            style="width: 100%"
          >
            <el-option
              v-for="opt in mapOptions"
              :key="opt.value"
              :label="opt.label"
              :value="opt.value"
            />
          </el-select>
          <div class="mt-1 text-xs text-gray-400">
            创建后冻结版本引用；地图发布新版本不会自动改写本路线坐标。
          </div>
        </el-form-item>
        <el-form-item label="路线数据" prop="routeData">
          <el-input
            v-model="addForm.routeData"
            type="textarea"
            :rows="3"
            placeholder="请输入路线数据（JSON格式）"
          />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="cancel">取消</el-button>
        <el-button type="primary" @click="handleSubmit(addFormRef)">
          确定
        </el-button>
      </template>
    </el-dialog>

    <el-dialog
      v-model="baselineVisible"
      title="地图基线（路线冻结引用）"
      width="760px"
    >
      <div v-loading="baselineLoading">
        <el-descriptions :column="2" border>
          <el-descriptions-item label="地图名称">
            {{ baselineInfo.mapName || "-" }}
          </el-descriptions-item>
          <el-descriptions-item label="版本 / 状态">
            {{ baselineInfo.mapVersion || "-" }} ·
            {{ baselineInfo.mapStatus || "-" }}
          </el-descriptions-item>
          <el-descriptions-item label="frame_id">
            {{ baselineInfo.frameId || "-" }}
          </el-descriptions-item>
          <el-descriptions-item label="CRS">
            {{ baselineInfo.crs || "-" }}
          </el-descriptions-item>
          <el-descriptions-item label="原点锚点">
            {{ baselineInfo.mapOrigin || "-" }}
          </el-descriptions-item>
          <el-descriptions-item label="分辨率">
            {{ baselineInfo.resolution || "-" }}
          </el-descriptions-item>
          <el-descriptions-item label="覆盖楼层">
            {{ baselineInfo.floors || "-" }}
          </el-descriptions-item>
          <el-descriptions-item label="栅格尺寸">
            <span v-if="baselineInfo.mapWidth && baselineInfo.mapHeight">
              {{ baselineInfo.mapWidth }}×{{ baselineInfo.mapHeight }}
            </span>
            <span v-else>-</span>
          </el-descriptions-item>
          <el-descriptions-item label="发布时间">
            {{ baselineInfo.publishTime || "-" }}
          </el-descriptions-item>
          <el-descriptions-item label="map_hash" :span="2">
            <span class="font-mono text-xs">
              {{ baselineInfo.mapHash || "缺失（禁止下发）" }}
            </span>
          </el-descriptions-item>
          <el-descriptions-item label="引用路线">
            {{ baselineRefs.routes?.length || 0 }} 条
          </el-descriptions-item>
          <el-descriptions-item label="关联任务">
            {{ baselineRefs.taskCount || 0 }} 个
          </el-descriptions-item>
        </el-descriptions>
        <el-alert
          class="mt-3"
          type="info"
          :closable="false"
          title="版本不可变：地图发布新版本不会自动改写本路线坐标，需要人工确认迁移。"
        />
      </div>
    </el-dialog>
  </div>
</template>

<style scoped lang="scss">
.my-card ::v-deep(.el-card__body) {
  padding: 10px 10px 0;
}

:deep(.el-dropdown-menu__item i) {
  margin: 0;
}

:deep(.el-link) {
  padding-left: 10px;
}

:deep(.el-form-item) {
  margin-bottom: 12px;
}
</style>
