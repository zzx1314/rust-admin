<script setup lang="ts">
import { useRenderIcon } from "@/components/ReIcon/src/hooks";
import More from "~icons/ep/more-filled";
import { PureTableBar } from "@/components/RePureTableBar";
import PureTable from "@pureadmin/table";
import DogSearchForm from "@/components/DogUI/DogSearchForm.vue";
import DogTableActions from "@/components/DogUI/DogTableActions.vue";
import { DEVICE_COMMANDS, DEVICE_STATUS, useDogDevice } from "./hook";

defineOptions({ name: "DogDevice" });

const {
  rules,
  columns,
  queryForm,
  queryFields,
  moreQueryFields,
  dataList,
  loading,
  pagination,
  selection,
  selectedIds,
  users,
  syncingId,
  sendingId,
  dispatchingId,
  dialogFormVisible,
  title,
  addForm,
  assignVisible,
  assignUsers,
  assignMode,
  assignIds,
  dispatchVisible,
  dispatchDevice,
  dispatchForm,
  runtimeVisible,
  runtimeLoading,
  runtimeDevice,
  runtimeData,
  onSearch,
  handleSizeChange,
  handleCurrentChange,
  handleSelectionChange,
  handleUpdate,
  handleSubmit,
  handleDelete,
  openAssign,
  submitAssign,
  submitUnassign,
  handleRowCommand,
  submitDispatch,
  cancel
} = useDogDevice();
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
    <PureTableBar title="设备列表" :columns="columns" @refresh="onSearch">
      <template #buttons>
        <!-- 设备台账由业务平台同步，中台不再手工新增 -->
        <el-button
          type="success"
          plain
          :disabled="!selectedIds.length"
          @click="openAssign(selection)"
        >
          批量分配用户
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
          <!-- 操作列：只留「修改 / 删除」+ 一个「更多」菜单（与用户管理一致） -->
          <template #operation="{ row }">
            <DogTableActions
              :row="row"
              :size="size"
              @edit="handleUpdate(row)"
              @delete="handleDelete(row)"
            >
              <template #extra="{ row: current, size: currentSize }">
                <el-dropdown
                  trigger="click"
                  @command="
                    (command: string) => handleRowCommand(current, command)
                  "
                >
                  <el-button
                    class="reset-margin"
                    link
                    type="primary"
                    :size="currentSize"
                    :icon="useRenderIcon(More)"
                    :loading="
                      sendingId === current.id || syncingId === current.id
                    "
                  />
                  <template #dropdown>
                    <el-dropdown-menu>
                      <el-dropdown-item command="assign"
                        >分配用户</el-dropdown-item
                      >
                      <el-dropdown-item command="dispatch"
                        >下发任务</el-dropdown-item
                      >
                      <el-dropdown-item command="sync"
                        >同步地图</el-dropdown-item
                      >
                      <el-dropdown-item command="runtime"
                        >运行态</el-dropdown-item
                      >
                      <el-dropdown-item divided disabled class="menu-group">
                        下发指令
                      </el-dropdown-item>
                      <el-dropdown-item
                        v-for="command in DEVICE_COMMANDS"
                        :key="command"
                        :command="`cmd:${command}`"
                        :disabled="sendingId === current.id"
                      >
                        {{ command }}
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
      width="620px"
      @close="cancel"
    >
      <el-form :model="addForm" :rules="rules" label-width="100px">
        <el-form-item label="设备编号" prop="deviceId">
          <el-input
            v-model="addForm.deviceId"
            :disabled="addForm.id !== null"
            placeholder="与业务平台 deviceId 一致"
          />
        </el-form-item>
        <el-form-item label="设备名称" prop="deviceName">
          <el-input v-model="addForm.deviceName" />
        </el-form-item>
        <el-form-item label="型号" prop="model">
          <el-input v-model="addForm.model" />
        </el-form-item>
        <el-form-item label="设备状态" prop="status">
          <el-select v-model="addForm.status" style="width: 100%">
            <el-option
              v-for="status in DEVICE_STATUS"
              :key="status"
              :label="status"
              :value="status"
            />
          </el-select>
        </el-form-item>
        <el-form-item label="设备 IP" prop="deviceIp">
          <el-input v-model="addForm.deviceIp" />
        </el-form-item>
        <el-form-item label="固件版本" prop="firmware">
          <el-input v-model="addForm.firmware" />
        </el-form-item>
        <el-form-item label="园区编码" prop="parkCode">
          <el-input
            v-model="addForm.parkCode"
            placeholder="业务库 parks.park_code"
          />
        </el-form-item>
        <el-form-item label="园区名称" prop="parkName">
          <el-input v-model="addForm.parkName" />
        </el-form-item>
        <el-form-item label="备注" prop="remarks">
          <el-input v-model="addForm.remarks" type="textarea" :rows="3" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="cancel">取消</el-button>
        <el-button type="primary" @click="handleSubmit">确认</el-button>
      </template>
    </el-dialog>

    <el-dialog v-model="assignVisible" title="分配设备用户" width="560px">
      <div class="assign-summary">
        已选择 <b>{{ assignIds.length }}</b> 台设备
      </div>
      <el-form label-width="90px">
        <el-form-item label="分配方式">
          <el-radio-group v-model="assignMode">
            <el-radio value="append">追加（保留已有分配）</el-radio>
            <el-radio value="replace">覆盖（清空后重新分配）</el-radio>
          </el-radio-group>
        </el-form-item>
        <el-form-item label="选择用户">
          <el-select
            v-model="assignUsers"
            multiple
            filterable
            style="width: 100%"
            placeholder="选择中台账号"
          >
            <el-option
              v-for="user in users"
              :key="user.id"
              :label="`${user.realName || user.username}（${user.username}）`"
              :value="user.id"
            />
          </el-select>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="assignVisible = false">取消</el-button>
        <el-button type="danger" plain @click="submitUnassign">
          解除分配
        </el-button>
        <el-button type="primary" @click="submitAssign">确认分配</el-button>
      </template>
    </el-dialog>

    <el-dialog v-model="dispatchVisible" title="下发巡检任务" width="560px">
      <el-alert
        type="info"
        :closable="false"
        show-icon
        title="路线（含航点 / 航段 / 动作）由业务平台巡检路线维护，这里填写其路线 ID"
      />
      <el-form label-width="100px" style="margin-top: 16px">
        <el-form-item label="设备">
          <span>
            {{ dispatchDevice?.deviceName || dispatchDevice?.deviceId }}
            <span class="text-muted">（{{ dispatchDevice?.deviceId }}）</span>
          </span>
        </el-form-item>
        <el-form-item label="任务名称" required>
          <el-input
            v-model="dispatchForm.taskName"
            placeholder="例如：A 区夜间巡检"
          />
        </el-form-item>
        <el-form-item label="路线 ID" required>
          <el-input-number
            v-model="dispatchForm.routeId"
            :min="1"
            :controls="false"
            style="width: 160px"
          />
        </el-form-item>
        <el-form-item label="计划开始">
          <el-date-picker
            v-model="dispatchForm.planStart"
            type="datetime"
            value-format="YYYY-MM-DD HH:mm:ss"
            placeholder="留空表示立即"
            style="width: 100%"
          />
        </el-form-item>
        <el-form-item label="计划结束">
          <el-date-picker
            v-model="dispatchForm.planEnd"
            type="datetime"
            value-format="YYYY-MM-DD HH:mm:ss"
            placeholder="可选"
            style="width: 100%"
          />
        </el-form-item>
        <el-form-item label="备注">
          <el-input
            v-model="dispatchForm.description"
            type="textarea"
            :rows="2"
          />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="dispatchVisible = false">取消</el-button>
        <el-button
          type="primary"
          :loading="dispatchingId === dispatchDevice?.id"
          @click="submitDispatch"
        >
          下发任务
        </el-button>
      </template>
    </el-dialog>

    <el-drawer
      v-model="runtimeVisible"
      size="480px"
      :title="`设备运行态 · ${runtimeDevice?.deviceName || runtimeDevice?.deviceId || ''}`"
    >
      <div v-loading="runtimeLoading">
        <el-descriptions v-if="runtimeData" :column="1" border>
          <el-descriptions-item label="在线">
            <el-tag
              :type="runtimeData.online ? 'success' : 'info'"
              size="small"
            >
              {{ runtimeData.online ? "已连接" : "未连接" }}
            </el-tag>
          </el-descriptions-item>
          <el-descriptions-item label="设备状态">
            {{ runtimeData.status }}
          </el-descriptions-item>
          <el-descriptions-item label="本体链路">
            <el-tag
              :type="
                runtimeData.bodyOnline
                  ? 'success'
                  : runtimeData.runMode === 'sim'
                    ? 'info'
                    : 'warning'
              "
              size="small"
            >
              {{
                runtimeData.bodyOnline
                  ? "在线"
                  : runtimeData.runMode === "sim"
                    ? "模拟模式"
                    : "本体离线"
              }}
            </el-tag>
          </el-descriptions-item>
          <el-descriptions-item label="电量">
            {{ runtimeData.bodyOnline ? `${runtimeData.battery}%` : "—" }}
          </el-descriptions-item>
          <el-descriptions-item label="充电状态">
            {{
              runtimeData.bodyOnline
                ? runtimeData.charging
                  ? "充电中"
                  : "未充电"
                : "—"
            }}
          </el-descriptions-item>
          <el-descriptions-item label="信号">
            {{ runtimeData.signal }}
          </el-descriptions-item>
          <el-descriptions-item label="园区">
            {{ runtimeData.parkName || "未绑定" }}
          </el-descriptions-item>
          <el-descriptions-item label="最近心跳">
            {{ runtimeData.lastHeartbeat || "-" }}
          </el-descriptions-item>
          <el-descriptions-item label="当前任务">
            {{ runtimeData.currentTaskName || "-" }}
          </el-descriptions-item>
          <el-descriptions-item label="任务进度">
            <template v-if="runtimeData.currentTask">
              {{ runtimeData.currentTask.taskName }} ·
              {{ runtimeData.currentTask.taskStatus }} ·
              {{ runtimeData.currentTask.progress }}%
            </template>
            <span v-else class="text-muted">无任务</span>
          </el-descriptions-item>
          <el-descriptions-item label="接管人">
            {{ runtimeData.teleopHolder || "-" }}
          </el-descriptions-item>
        </el-descriptions>
        <el-empty v-else-if="!runtimeLoading" description="暂无运行数据" />
      </div>
    </el-drawer>
  </div>
</template>

<style scoped lang="scss">
.text-muted {
  color: var(--el-text-color-placeholder);
}

.assign-summary {
  margin-bottom: 12px;
  color: var(--el-text-color-regular);
}

/* 「更多」菜单里的分组标题（不可点击） */
.menu-group {
  font-size: 12px;
  color: var(--el-text-color-placeholder);
}
</style>
