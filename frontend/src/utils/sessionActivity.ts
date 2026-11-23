/**
 * 会话活动心跳。
 *
 * 谁算「用户还在用」：DOM 交互事件（mousedown/mousemove/keydown…）**和**前台的接口
 * 流量（任意请求成功返回）。之前只认 DOM 事件，于是挂在「实时视频」「设备状态」
 * 「告警看板」这类靠轮询/推流刷新的页面上不动鼠标，15 分钟后会被会话超时直接踢
 * 回登录页（看起来就是「莫名登录失效」）。
 *
 * 刻意两处收窄，避免把「浏览器开着」当成「人在用」：
 *   1. 只在页面可见（`document.visibilityState === "visible"`）时才算活动，后台标签
 *      页的轮询不续命；
 *   2. 通知节流到 1s，高频轮询不会把订阅者刷爆。
 */

type ActivityListener = () => void;

/** 两次通知之间的最小间隔（毫秒），用于节流 */
const NOTIFY_GAP_MS = 1000;

const listeners = new Set<ActivityListener>();
let lastNotifiedAt = 0;

/** 标记一次会话活动（DOM 事件与接口响应都往这里打点） */
export function markSessionActivity(): void {
  if (
    typeof document !== "undefined" &&
    document.visibilityState !== "visible"
  ) {
    return;
  }
  const now = Date.now();
  if (now - lastNotifiedAt < NOTIFY_GAP_MS) return;
  lastNotifiedAt = now;
  listeners.forEach(listener => listener());
}

/** 订阅会话活动，返回取消订阅函数 */
export function onSessionActivity(listener: ActivityListener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
