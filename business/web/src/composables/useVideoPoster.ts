/**
 * 「最后一帧」跨播放器共享。
 *
 * 现场直连（WHEP）⇄ 平台中继（FLV）切换、换通道、中继重启 ffmpeg 都要重新建流，
 * 起流要 0.5~7s（板端 relay 起 ffmpeg + 等关键帧）。如果这段时间画面是黑的，
 * 现场一律说「卡住了」。所以把上一路解出来的帧存成小图，新播放器没出画之前先顶着。
 *
 * 只放一张 320px 宽的 jpeg（~10KB），不做帧缓冲；换流成功播放后由播放器自己清掉。
 */
let poster = "";

export function setVideoPoster(src: string): void {
  poster = src;
}

export function getVideoPoster(): string {
  return poster;
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
