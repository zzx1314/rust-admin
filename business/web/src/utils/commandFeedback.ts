import { ElMessage } from "element-plus";
import { deviceApi } from "@/api/ops";
import type { TaskEvent } from "@/api/types";

/**
 * 下发指令后校验「设备侧到底受没受理」。
 *
 * `POST /devices/{id}/action|command` 返回 200 只代表命令送进了设备连接；客户端还有
 * 一层本体健康门禁（sdk_node / 急停 / 控制权），拒绝时只回一条 `event_type=error`
 * 的设备事件（"指令 站立 被拒绝: 本体 SDK 节点未就绪"）。不主动去看，页面上就是
 * 「点了没反应」。
 */
export async function verifyCommandAccepted(
  deviceId: number,
  label: string,
  waitMs = 1500
): Promise<TaskEvent | null> {
  await new Promise(resolve => setTimeout(resolve, waitMs));
  let events: TaskEvent[] = [];
  try {
    events = await deviceApi.events(deviceId);
  } catch {
    return null;
  }
  const latest = events[0];
  if (!latest) return null;
  const rejected = latest.eventType === "error" || /被拒绝|受理失败/.test(latest.title);
  if (!rejected) return null;
  ElMessage.warning({
    message: `${label}：设备未受理 —— ${latest.description || latest.title}`,
    duration: 6000
  });
  return latest;
}
