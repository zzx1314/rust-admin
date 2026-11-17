/**
 * 背包 AI 模型目录（识别场景 + 识别类别）。
 *
 * 路线编辑器配航点「AI 识别」动作时用它，数据来源与「实时视频」页的 AI 面板完全一致：
 * - 代理模式：`GET /api/backpack/status`（业务服务聚合板端 Agent）；
 * - 直连模式（`mode = direct`，现场入口）：浏览器自己向板端要 `/api/detect/tasks`。
 *
 * 两边拿到的是同一份模型包清单（`BackpackTask[]`），所以航点动作里勾的类别名
 * （英文 `name`）与实时视频下发的 `labels` 是同一套 —— 服务端 `ai_patrol`
 * 直接把它塞进 `/api/detect/start`，不再按中文关键词猜场景。
 *
 * 只按需加载（打开「添加航点动作」且类型选了 AI 识别），避免路线页首屏等板端；
 * 失败不抛异常，由调用方展示 `error` 并给「重新读取」。
 */
import { ref } from "vue";
import { loadStatus, type BackpackTask } from "@/api/backpack";

export interface LiveAiSelection {
  task: string;
  labels: string[];
  conf: number;
}

export function useAiCatalog() {
  const tasks = ref<BackpackTask[]>([]);
  const loading = ref(false);
  const error = ref("");
  /** 「实时视频」当前正在跑的识别（场景 + 已勾选类别 + 阈值），用来做航点动作的默认值 */
  const live = ref<LiveAiSelection | null>(null);

  async function load(force = false) {
    if (loading.value) return;
    if (tasks.value.length && !force) return;
    loading.value = true;
    error.value = "";
    try {
      // 与「实时视频」页同一套通道退让：浏览器直连不通时自动走设备客户端隧道，
      // 所以在园区外（出差 / 公网入口）也能拿到模型目录来配航点动作
      const data = await loadStatus();
      const list = data.detect.tasks?.tasks ?? [];
      tasks.value = list;
      const status = data.detect.status;
      live.value =
        status?.running && status.task
          ? { task: status.task, labels: status.labels ?? [], conf: status.conf }
          : null;
      if (!list.length) {
        error.value =
          data.detect.tasks_error ||
          data.agent.error ||
          (data.configured
            ? "板端没有可用模型包（rknn/models/ 为空）"
            : "未配置数据采集背包（ROBODOG_BACKPACK_URL，或让板端客户端上线隧道）");
      }
    } catch (err) {
      tasks.value = [];
      live.value = null;
      error.value = err instanceof Error ? err.message : "读取背包 AI 模型目录失败";
    } finally {
      loading.value = false;
    }
  }

  function taskOf(id: string) {
    return tasks.value.find(item => item.task === id) ?? null;
  }

  /** 某个场景的可用类别（与实时视频 AI 面板一致：过滤板端标记 `ignored` 的） */
  function labelsOf(id: string) {
    return (taskOf(id)?.labels ?? []).filter(label => !label.ignored);
  }

  return { tasks, loading, error, live, load, taskOf, labelsOf };
}
