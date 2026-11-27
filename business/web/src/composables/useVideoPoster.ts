/**
 * 「最后一帧」跨播放器共享。
 *
 * 现场直连（WHEP）⇄ 平台中继（WHEP）切换、换通道、中继重启 ffmpeg 都要重新建流，
 * 起流要 0.5~7s（板端 relay 起 ffmpeg + 等关键帧）。如果这段时间画面是黑的，
 * 现场一律说「卡住了」。所以把上一路解出来的帧存成小图，新播放器没出画之前先顶着。
 *
 * 只放一张 320px 宽的 jpeg（~10KB），不做帧缓冲；换流成功播放后由播放器自己清掉。
 *
 * **必须带时效**：静帧只能在「正在换流」的几个心跳内出现。板端硬复位 / 断网时中继
 * 会黑很久，这时候还挂着上一帧，现场就会说「卡住了」（2026-11-18 设备抽屉那次）。
 * 所以取的时候按时间戳过滤，过期就当没有；播放器另外还有一层定时清，
 * 并且**在静帧上盖一层「起流中」遮罩**（`VideoPlayer.vue`）—— 不能让静帧冒充直播。
 */
interface Shot {
  src: string;
  at: number;
}

/** 共享静帧的有效期：和播放器一次连接尝试的超时（`CONNECT_TIMEOUT_MS`）对齐 */
const MAX_AGE_MS = 20_000;

/**
 * 静帧按**流**分槽存放（多设备部署）。
 *
 * 以前只有一张：切到另一台背包时，画面上会先顶上一台狗的最后一帧 —— 现场会
 * 以为「视频串台了」。槽位 key 用房间名（`bp:<device>:<stream>`），不传 key 时
 * 落到默认槽位（老调用方行为不变）。
 */
const shots = new Map<string, Shot>();
const DEFAULT_KEY = "";

function keyOf(key?: string | null): string {
  return (key ?? DEFAULT_KEY).trim();
}

export function setVideoPoster(src: string, key?: string): void {
  const slot = keyOf(key);
  if (src) shots.set(slot, { src, at: Date.now() });
  else shots.delete(slot);
}

export function getVideoPoster(maxAgeMs = MAX_AGE_MS, key?: string): string {
  const shot = shots.get(keyOf(key));
  if (!shot) return "";
  if (Date.now() - shot.at > maxAgeMs) return "";
  return shot.src;
}

export function clearVideoPoster(key?: string): void {
  shots.delete(keyOf(key));
}

/** `<video>` 当前帧 → 320px jpeg 的 dataURL；取不到（跨域 / 还没出画）返回空串 */
export function captureVideoPoster(el: HTMLVideoElement | null | undefined): string {
  if (!el || !el.videoWidth) return "";
  try {
    const width = 320;
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = Math.max(1, Math.round((el.videoHeight / el.videoWidth) * width));
    canvas.getContext("2d")?.drawImage(el, 0, 0, canvas.width, canvas.height);
    return canvas.toDataURL("image/jpeg", 0.5);
  } catch {
    return "";
  }
}
