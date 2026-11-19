<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { tokenStorage } from "@/api/http";
import { apiBaseUrl, wsOrigin } from "@/utils/runtime";
import { startWhep, type WhepSession } from "@/lib/whep";
import {
  captureVideoPoster,
  clearVideoPoster,
  getVideoPoster,
  setVideoPoster
} from "@/composables/useVideoPoster";
import AppIcon from "./AppIcon.vue";

/**
 * 背包实时视频（平台中继播放器）
 *
 * 只做一件事：把板端 relay 推到平台房间的 FLV 字节流，经同源 WS 交给 flv.js 播放。
 *
 * - 实时视频页的「平台中继」、设备管理 / 计划管理的前置摄像头（`FrontCameraVideo.vue`）
 *   在跨网段 / 公网入口时都走这里；
 * - 板端 relay 常驻推流，服务端按需下发 `video_start` / `video_stop`（见 `server/src/video.rs`）；
 * - 浏览器只跟平台同源通信，不依赖操作员电脑在内网、也不需要本地网络许可。
 *
 * 设备档案里的 RTSP / HLS 地址已不再播放（2026-11-18 起统一到背包前置 `dogcam`），
 * 所以这里不再有 hls / flv(HTTP) / mp4 分支。
 *
 * ## 直播「不许闪」的四条硬规则（改这块之前先读）
 *
 * 1. **缓冲 ≠ 断开**：`waiting` / `stalled` 只置一个「缓冲中」角标，绝不盖全屏错误层。
 * 2. **重连不清画面**：断流时先把当前帧抓成 poster 继续显示，再重建 flv 播放器，
 *    所以重连过程中观众看到的是「上一帧 + 重连中」，不是黑屏闪一下。
 * 3. **快速重连 + 退避**：服务端主动断开（推流端换流 / 观看端追不上）时 250ms 就重连，
 *    连续失败才指数退避（上限 3s），不会 3s 一次空转刷屏。
 * 4. **推流端要保持在跑**：服务端最后一个观看者离开时不立刻杀 ffmpeg（30s 宽限），
 *    所以这里的 `video_start` 重复下发也不会重启推流端（板端 relay 按 stream 去重）。
 * 5. **起流期间不许是纯黑**：没有画面时用「上一帧 + 起流中遮罩」顶着，并实时显示已经等了
 *    多少秒（板端起 ffmpeg + 等关键帧 1~7s，低档位 GOP 更长）。纯黑会被现场一律读成「卡死」。
 */
const props = defineProps<{
  /**
   * 背包通道（`cam0`…`dogcam` / `preview`）：房间名是 `bp:<stream>`。
   *
   * 这条通路不经过浏览器直连板端，因此不受「换 WiFi / 换浏览器 / LNA 许可」影响。
   */
  stream?: string | null;
}>();

/** 实际解码分辨率：页面用它显示「1920×1080」而不是写死一个规格 */
const emit = defineEmits<{
  (event: "meta", size: { w: number; h: number }): void;
}>();

/**
 * 媒体通路：
 *
 * - `flv` —— 板端 relay 把 FLV 推进平台房间，浏览器经同源 WS 交给 flv.js（老通路，
 *   延时 1~2s，但只靠 TCP/443，**任何网络都能出图**）；
 * - `whep` —— 板端用 RTSP 把流转推到平台 mediamtx，浏览器经 `/api/backpack/whep/`
 *   同源反代拉 WebRTC（延时 ~300ms，需要平台侧 UDP 8189 可达）。
 *
 * 走哪条**由板端决定**（`relay_push.py` 的 `RELAY_OUTPUT`），起流时通过控制通道报
 * 给平台，前端从 `/api/backpack/video/state` 读回来 —— 不能靠猜：两条通路在平台上
 * 都表现为「房间里有推流端」。
 */
type Transport = "flv" | "whep";

type Status = "idle" | "loading" | "playing" | "retrying" | "error";

const transport = ref<Transport>("flv");
/** WebRTC 会话句柄（`lib/whep.ts` 自带指数退避重连，我们只负责建与拆） */
let whep: WhepSession | null = null;
/**
 * 在场心跳：媒体走 WHEP 时，平台就靠这条 WS 数观看者。
 *
 * 没有它：第一个观看者进来不会触发 `video_start`（板端不会起 ffmpeg），
 * 30s 后看门狗还会认为没人看而下发 `video_stop` 把流停掉。
 * `media=0` 让服务端只做计数与起停流，不再往这里推 FLV 字节。
 */
let presence: WebSocket | null = null;
let presenceWanted = false;
let presenceTimer: ReturnType<typeof setTimeout> | null = null;
/** WHEP 会话代次：异步回调靠它判断自己是不是已经被新一轮取代 */
let whepSeq = 0;

const videoRef = ref<HTMLVideoElement | null>(null);
const stageRef = ref<HTMLDivElement | null>(null);
const status = ref<Status>("idle");
const errorText = ref("");
const muted = ref(true);
/** 卡顿时角标显示「缓冲中」，不铺错误层 */
const buffering = ref(false);
/** 断流瞬间抓下来的最后一帧：重连期间继续显示，避免黑屏闪一下 */
const poster = ref("");
const retryAttempt = ref(0);
/** 起流超过 2.5s 才提示「板端正在起流」：正常情况下几十 ms 就 playing，不该先吓人 */
const slowStart = ref(false);
/** 这一次连接已经等了多久（秒）：起流 / 重连遮罩上直接显示，别让观众对着黑屏猜 */
const waitingSecs = ref(0);

// flv.js 体积大（~150KB），只有真正需要播放时才拉进来，
// 避免设备列表 / 任务看板为了一个播放器把整个 bundle 撑大
// flv.js 的类型是全局命名空间 `FlvJs`，没有具名导出 Player
type FlvPlayer = ReturnType<typeof import("flv.js").default.createPlayer>;
let flv: FlvPlayer | null = null;
/** 每次 attach 自增，异步加载回来时据此判断是否已被新请求取代 */
let attachSeq = 0;
/**
 * 中继断流的重连计时器与次数。
 *
 * 推流端重连、观看端追不上、弱网抖动都会让这条 WS 被关掉；这里自动重连拿干净的
 * 新流（服务端会补流头 + 关键帧），不让用户手动点「重连」。
 */
let retryTimer: ReturnType<typeof setTimeout> | null = null;
/** 首连重连的等待：服务端主动断开时几乎立刻就能拿回干净的新流 */
const RETRY_BASE = 250;
const RETRY_MAX = 3000;
/** 连续失败这么多次才把错误铺在画面上（前几次只是角标提示，不打扰画面） */
const HARD_ERROR_AFTER = 5;
/**
 * 画面停住多久判定「假死」并重连（弱网下 MSE 会卡在 readyState=2 不再前进）。
 *
 * 3s 太紧：现场背包推的是中低帧率、偶尔 1~2s 没新帧很常见，一超时就重连，
 * 重连又会让服务端重新拉起 ffmpeg，越连越慢。给到 8s，既有自愈又不抖。
 */
const STALL_MS = 8000;
/**
 * 一次连接尝试多久没画面算超时。
 *
 * 现场路径：浏览器连上中继 → 平台下发 video_start → 板端起 ffmpeg（3~7s）
 * → 等首个关键帧（GOP 2s）→ 才有画面。看门狗刚把推流停掉时这串要重新走一遍，
 * 10s 太紧：一旦超时就断连，服务端数到「没人看」又把流停掉，下一轮更慢 ——
 * 现场看到的就是「一直在重连」。给到 20s，宁可多等一会也不要打死再重来。
 */
const CONNECT_TIMEOUT_MS = 20_000;
let stallTimer: ReturnType<typeof setInterval> | null = null;
let slowStartTimer: ReturnType<typeof setTimeout> | null = null;
/**
 * 「上一帧」最多顶这么久：中继真断了就老实显示连接中/失败，不能拿静帧冒充直播。
 *
 * 和 [`CONNECT_TIMEOUT_MS`] 对齐：板端起 ffmpeg + 等关键帧要 1~7s，低档位（极省流
 * 320x180，rkmpp 靠丢帧保码率、实测 ~2fps）GOP 换算成时间能到 7~8s。静帧 6s 就撤掉时，
 * 剩下的时间就是**纯黑**，现场看到的就是「起流黑好大一会」。现在静帧顶上到超时为止，
 * 同时盖一层「起流中 · 已等 Ns」遮罩（静帧只陪跑，不冒充直播）。
 */
const POSTER_MAX_MS = 20_000;
let posterTimer: ReturnType<typeof setTimeout> | null = null;
let lastTime = 0;
let lastProgressAt = 0;
let attachStartedAt = 0;

/**
 * 中继地址：业务平台把板端 relay 推上来的 FLV 通过 WS 转给浏览器。
 *
 * 浏览器 WS 握手不能带 Authorization 头，令牌只能放查询串（服务端 require_auth 支持）。
 */
const relayUrl = computed(() => {
  if (!props.stream) return "";
  const token = tokenStorage.get() || "";
  // 页面可能由现场入口托管（内网静态服务），中继仍在云端：用运行时基址
  return `${wsOrigin()}/api/backpack/video/ws?stream=${encodeURIComponent(props.stream)}&token=${encodeURIComponent(token)}`;
});

const playable = computed(() => !!props.stream);

/** WHEP 地址：平台同源反代（`mediamtx :8889` 只绑回环，不对外） */
const whepUrl = computed(() => {
  if (!props.stream) return "";
  const token = tokenStorage.get() || "";
  return `${apiBaseUrl()}/backpack/whep/${encodeURIComponent(props.stream)}/whep?token=${encodeURIComponent(token)}`;
});
/** 全屏错误层只在这些状态出现：不能播 / 连续重连失败 */
const showFallback = computed(() => !playable.value || status.value === "error");

const hint = computed(() => {
  if (!props.stream) return "未选择背包通道";
  if (status.value === "error")
    return errorText.value || "中继拉流失败：确认背包上 backpack-relay 服务在线";
  if (status.value === "loading")
    return slowStart.value
      ? "板端正在起流（约 1~7s）：切中继后要重新拉 RTSP + 等关键帧"
      : "正在连接平台中继…";
  return "";
});

const badge = computed(() => {
  if (!playable.value) return "OFFLINE";
  if (status.value === "playing") return buffering.value ? "缓冲中" : "LIVE";
  if (status.value === "retrying") return `重连中 ${retryAttempt.value}`;
  if (status.value === "error") return "连接失败";
  return slowStart.value ? "起流中" : "连接中";
});

const badgeTone = computed(() => {
  if (!playable.value) return "off";
  if (status.value === "playing") return buffering.value ? "wait" : "live";
  if (status.value === "error") return "err";
  return "wait";
});

/**
 * 起流 / 重连遮罩的文案（空串 = 不显示）。
 *
 * 起流本来就慢：浏览器连上中继 → 平台下发 `video_start` → 板端起 ffmpeg（1~7s）
 * → 等首个关键帧。这期间如果画面是纯黑，现场一律说「卡住了」；显示「已等 Ns」
 * 就能把「在干活」和「真死了」区分开。
 */
const startHint = computed(() => {
  if (!playable.value) return "";
  if (status.value === "retrying")
    return `重连中（第 ${retryAttempt.value} 次）· 已等 ${waitingSecs.value}s`;
  if (status.value === "loading")
    return slowStart.value ? `起流中 · 已等 ${waitingSecs.value}s` : "正在连接平台中继…";
  return "";
});

/** 起流慢时的补充说明：说清楚在等什么、等多久会自己重连 */
const startDetail = computed(() => {
  if (!slowStart.value) return "";
  return `板端 relay 起 ffmpeg + 等关键帧，约 1~7s；超过 ${Math.round(
    CONNECT_TIMEOUT_MS / 1000
  )}s 自动重连`;
});

function clearRetry() {
  if (retryTimer) {
    clearTimeout(retryTimer);
    retryTimer = null;
  }
}

/** 静帧超时：到点还没出画就把 poster 撤掉（同时清掉共享缓存，别传给下一个播放器） */
function armPosterTimeout() {
  if (posterTimer) clearTimeout(posterTimer);
  posterTimer = setTimeout(() => {
    posterTimer = null;
    poster.value = "";
    clearVideoPoster();
  }, POSTER_MAX_MS);
}

function clearPoster() {
  if (posterTimer) {
    clearTimeout(posterTimer);
    posterTimer = null;
  }
  poster.value = "";
}

function armSlowStartHint() {
  clearSlowStartHint();
  slowStartTimer = setTimeout(() => {
    if (status.value === "loading") slowStart.value = true;
  }, 2500);
}

function clearSlowStartHint() {
  if (slowStartTimer) {
    clearTimeout(slowStartTimer);
    slowStartTimer = null;
  }
  slowStart.value = false;
}

/** 只在销毁播放器内核，保留 `<video>` 与画面（重连时不留黑屏） */
function teardownPlayer() {
  if (flv) {
    flv.destroy();
    flv = null;
  }
  teardownWhep();
}

/** 拆 WebRTC 会话：`close()` 会 DELETE 掉 mediamtx 的会话，不留僵尸 reader */
function teardownWhep() {
  whepSeq += 1;
  const session = whep;
  whep = null;
  if (session) session.close();
}

/**
 * 问服务端：这一路的媒体走 WebRTC 还是 FLV。
 *
 * 结果是 `bp:<stream>` 房间的 `transport` 字段（板端 `video_transport` 报上来）。
 * 查不到（没登录 / 推流端未连接 / 老人板端）一律当 `flv` —— 老通路永远在，不能因为
 * 一个探测请求失败就把画面搞没。
 */
async function detectTransport(): Promise<Transport> {
  if (!props.stream) return "flv";
  const token = tokenStorage.get() || "";
  try {
    const response = await fetch(`${apiBaseUrl()}/backpack/video/state`, {
      headers: token ? { Authorization: `Bearer ${token}` } : undefined
    });
    if (!response.ok) return "flv";
    const body = (await response.json()) as { data?: { rooms?: Array<Record<string, unknown>> } };
    const room = body?.data?.rooms?.find(item => item.room === `bp:${props.stream}`);
    return room?.transport === "rtsp" ? "whep" : "flv";
  } catch {
    return "flv";
  }
}

/** 在场心跳：媒体走 WHEP 时，平台只能靠这条 WS 知道「有人在看」 */
function openPresence() {
  if (!props.stream) return;
  presenceWanted = true;
  if (presence) return;
  const token = tokenStorage.get() || "";
  const socket = new WebSocket(
    `${wsOrigin()}/api/backpack/video/ws?media=0&stream=${encodeURIComponent(props.stream)}&token=${encodeURIComponent(token)}`
  );
  presence = socket;
  socket.onclose = () => {
    if (presence !== socket) return;
    presence = null;
    if (!presenceWanted) return;
    // 平台重启 / 推流端未连接都会把它断掉：按住时重连，别让观看者计数掉成 0
    // （丢了计数 → 看门狗 30s 后下发 video_stop，板端就把 ffmpeg 停了）
    presenceTimer = setTimeout(() => {
      presenceTimer = null;
      if (presenceWanted) openPresence();
    }, 3000);
  };
}

function closePresence() {
  presenceWanted = false;
  if (presenceTimer) {
    clearTimeout(presenceTimer);
    presenceTimer = null;
  }
  const socket = presence;
  presence = null;
  if (socket) socket.close();
}

/** 起 WebRTC：媒体由 mediamtx 直接发到浏览器（UDP 8189），不进平台后端 */
function attachWhep(el: HTMLVideoElement, seq: number) {
  const mine = ++whepSeq;
  openPresence();
  whep = startWhep({
    url: whepUrl.value,
    video: el,
    onPhase: detail => {
      if (mine !== whepSeq || seq !== attachSeq) return;
      if (detail.phase === "live") return; // `onPlaying` 事件接管状态
      if (detail.phase === "connecting") {
        if (status.value !== "retrying") status.value = "loading";
        return;
      }
      if (detail.phase === "retry") {
        // 「路径上还没有 publisher」是起流中的常态（板端刚被叫起来推流），不是故障：
        // 文案由 `lib/whep.ts` 翻成人话，前几次只改角标。
        retryAttempt.value += 1;
        errorText.value = detail.message;
        status.value = retryAttempt.value >= HARD_ERROR_AFTER ? "error" : "retrying";
      }
    }
  });
}

/** 抓当前帧当 poster：重连 / 换通道期间继续显示「最后一帧」而不是黑屏 */
function capturePoster() {
  const shot = captureVideoPoster(videoRef.value);
  if (!shot) return;
  poster.value = shot;
  setVideoPoster(shot); // 切到直连 / 切通道时由下一个播放器接着顶
}

/** 断流：先留帧，再按退避重连；前几次不弹错误，只改角标 */
function scheduleRetry(reason: string) {
  if (!playable.value) return;
  if (retryTimer) return;
  // 作废可能还在异步加载 / 建连中的那一次尝试，避免两套播放器抢同一个 <video>
  attachSeq += 1;
  capturePoster();
  teardownPlayer();
  retryAttempt.value += 1;
  buffering.value = false;
  errorText.value = reason;
  const delay = Math.min(RETRY_BASE * 2 ** (retryAttempt.value - 1), RETRY_MAX);
  if (retryAttempt.value >= HARD_ERROR_AFTER) {
    status.value = "error";
  } else {
    status.value = "retrying";
  }
  // 后台标签页不重连：浏览器会冻结 muted 视频的解码，时间轴本来就不走，
  // 那时候重连毫无意义——而且每重连一次推流端就要起一遍 ffmpeg，
  // 现场表现就是「页面切走就开始反复重连」。回前台时由 onVisibility 补连。
  if (document.visibilityState === "hidden") return;
  retryTimer = setTimeout(() => {
    retryTimer = null;
    void attach();
  }, delay);
}

function destroy() {
  clearRetry();
  clearSlowStartHint();
  capturePoster(); // 交给共享缓存：切到直连 / 切通道时由下一个播放器先顶这一帧
  teardownPlayer();
  closePresence();
  const el = videoRef.value;
  if (el) {
    el.pause();
    el.removeAttribute("src");
    el.load();
  }
  status.value = "idle";
  errorText.value = "";
  poster.value = "";
  buffering.value = false;
  retryAttempt.value = 0;
  lastTime = 0;
  lastProgressAt = 0;
  attachStartedAt = 0;
}

function fail(message: string) {
  status.value = "error";
  errorText.value = message;
  // 失败就别再挂着静帧：让错误层和「重连中 N」角标说人话
  clearPoster();
  clearVideoPoster();
}

/**
 * flv.js 的直播参数：关掉 stash 缓冲，吃多少播多少（默认攒 3s 才起播，抖动就来回跳）。
 *
 * `liveBufferLatencyChasing`：播放位置比"最新帧"落后超过 `liveBufferLatencyMaxLatency`
 * 就**直接跳到直播边缘**。没有它时，MSE 里攒下的那截不会自己消化 —— 现场体感就是
 * 「越看越延迟」，最后卡在某一帧（这正是 2026-11-18 反馈的那一条）。
 * 阈值取 1.2s：中继本身 1~2s，再攒 1.2s 才追，不会因为偶尔抖一下就把画面跳得一惊一乍。
 */
function liveFlvConfig(url: string) {
  return {
    type: "flv" as const,
    url,
    isLive: true,
    hasAudio: false,
    lazyLoad: false,
    enableStashBuffer: false,
    stashInitialSize: 128,
    autoCleanupSourceBuffer: true,
    fixAudioTimestampGap: false,
    liveBufferLatencyChasing: true,
    liveBufferLatencyMaxLatency: 1.2,
    liveBufferLatencyMinRemain: 0.5
  };
}

async function attach() {
  const el = videoRef.value;
  if (!el || !playable.value) return;
  clearRetry();
  teardownPlayer();
  const seq = ++attachSeq;
  attachStartedAt = performance.now();
  // 重连途中不要退回「连接中」：角标要一直显示重连次数
  if (status.value !== "retrying") {
    status.value = "loading";
    errorText.value = "";
  }
  buffering.value = false;
  // 上一路（直连 / 上一个通道）的最后一帧先顶着：切流期间不能是黑屏。
  // 顶到一次连接尝试超时为止（见 armPosterTimeout）：板端复位后中继要几十秒才回来，
  // 那时候静帧已经过期，遮罩会接管（不能拿静帧冒充直播）。
  if (!poster.value) poster.value = getVideoPoster();
  if (poster.value) armPosterTimeout();
  waitingSecs.value = 0;
  armSlowStartHint();

  // 先问服务端这一路的媒体通路（板端起流时上报）。
  // WebRTC 优先：平台 mediamtx 有 publisher 就拉 WHEP（~300ms）；否则走老的 FLV 中继。
  transport.value = await detectTransport();
  if (seq !== attachSeq) return;
  if (transport.value === "whep") {
    attachWhep(el, seq);
    return;
  }

  // 推流端已把源转成 FLV，flv.js 的 WebSocketLoader 直接消费二进制帧
  const { default: flvjs } = await import("flv.js");
  if (seq !== attachSeq) return;
  if (!flvjs.isSupported()) {
    fail("当前浏览器不支持 FLV 播放");
    return;
  }
  const player = flvjs.createPlayer(liveFlvConfig(relayUrl.value));
  flv = player;
  player.attachMediaElement(el);
  player.load();
  player.play()?.catch(() => undefined);
  player.on(flvjs.Events.ERROR, (_type: string, detail: string) => {
    // 已销毁 / 已被新播放器取代的旧实例的迟到事件不能搅乱新流
    if (flv !== player) return;
    scheduleRetry(
      `中继断流（${detail || "unknown"}）：确认背包上 backpack-relay 服务在线`
    );
  });
}

/* ---------------- 播放状态 ---------------- */

function onPlaying() {
  status.value = "playing";
  buffering.value = false;
  clearSlowStartHint();
  clearPoster();
  errorText.value = "";
  retryAttempt.value = 0;
  poster.value = "";
  // 流里真正解出来的尺寸：推流端改了分辨率，这里会跟着变
  const el = videoRef.value;
  if (el?.videoWidth && el.videoHeight) emit("meta", { w: el.videoWidth, h: el.videoHeight });
  lastTime = videoRef.value?.currentTime ?? 0;
  lastProgressAt = performance.now();
}

function onWaiting() {
  // 直播偶发缓冲是常态：给个角标，别盖错误层
  if (status.value === "playing") buffering.value = true;
}

/** `<video>` 自身的 error：装/卸播放器时也会冒（src 被清掉），只在真正播放中才算断流 */
function onMediaError() {
  if (status.value === "idle" || status.value === "retrying") return;
  scheduleRetry("视频源不可达");
}

/** MSE 「假死」看门狗：playing 却长时间时间轴不前进就自己重连；一直连不上也算超时 */
function tickStall() {
  // 后台标签页：浏览器不渲染也不解码，时间轴停住是正常现象。这里若继续判
  // 「超时 / 假死」就是后台无限重连，还会反复拉起推流端 ffmpeg。
  if (document.visibilityState === "hidden") return;
  const el = videoRef.value;
  if (!el || !playable.value) {
    lastProgressAt = performance.now();
    return;
  }
  if (status.value === "loading" || status.value === "retrying") {
    const waited = attachStartedAt ? (performance.now() - attachStartedAt) / 1000 : 0;
    waitingSecs.value = Math.max(0, Math.round(waited));
    if (attachStartedAt && performance.now() - attachStartedAt > CONNECT_TIMEOUT_MS) {
      // WHEP 自带指数退避重连（`lib/whep.ts`）：这里再打死重来只会两边抢同一条流，
      // 而且每次重来都要重新 ICE 协商。连接真的建不起来时它自己会把 `failed` 报上来。
      if (transport.value === "whep") return;
      scheduleRetry("连接超时，正在重新拉流");
    }
    return;
  }
  if (status.value !== "playing") {
    lastProgressAt = performance.now();
    return;
  }
  if (el.currentTime !== lastTime) {
    lastTime = el.currentTime;
    lastProgressAt = performance.now();
    if (el.readyState >= 3) buffering.value = false;
    return;
  }
  if (performance.now() - lastProgressAt > STALL_MS) {
    scheduleRetry("画面停住，正在重新拉流");
  }
}

/** 切回标签页时视频可能已被浏览器冻结：立刻检查一次，别等看门狗 */
function onVisibility() {
  if (document.visibilityState !== "visible") return;
  // 后台被冻结的那段不能算进「没画面 / 假死」的计时，回前台重新起算
  lastProgressAt = performance.now();
  attachStartedAt = performance.now();
  // 上次重连被后台状态拦下了（见 scheduleRetry），回前台补一次
  if (!retryTimer && (status.value === "retrying" || status.value === "error")) {
    retryAttempt.value = 0;
    void attach();
    return;
  }
  tickStall();
}

function toggleMute() {
  muted.value = !muted.value;
  if (videoRef.value) videoRef.value.muted = muted.value;
}

async function enterFullscreen() {
  const el = stageRef.value;
  if (!el) return;
  if (document.fullscreenElement) {
    await document.exitFullscreen();
  } else {
    await el.requestFullscreen?.();
  }
}

watch(
  () => props.stream,
  () => {
    // 通道变化视为全新播放：清掉重连状态
    retryAttempt.value = 0;
    destroy();
    // 等 DOM 更新完再挂载播放器
    requestAnimationFrame(() => {
      void attach();
    });
  },
  { immediate: true }
);

onMounted(() => {
  document.addEventListener("visibilitychange", onVisibility);
  stallTimer = setInterval(tickStall, 1000);
});

onBeforeUnmount(() => {
  document.removeEventListener("visibilitychange", onVisibility);
  if (stallTimer) {
    clearInterval(stallTimer);
    stallTimer = null;
  }
  destroy();
});
</script>

<template>
  <div ref="stageRef" class="video-stage">
    <video
      ref="videoRef"
      class="video-el"
      :class="{ on: status === 'playing' }"
      :muted="muted"
      playsinline
      autoplay
      @playing="onPlaying"
      @waiting="onWaiting"
      @stalled="onWaiting"
      @error="onMediaError"
    />

    <!-- 断流重连期间继续显示最后一帧，避免「黑一下再闪回来」 -->
    <img v-if="poster && status !== 'playing'" class="video-poster" :src="poster" alt="" />

    <!-- 起流 / 重连：上一帧后面盖一层「已等 Ns」。起流本来要 1~7s，纯黑会被当成卡死 -->
    <div v-if="startHint" class="video-starting">
      <i class="video-starting-dot" />
      <strong>{{ startHint }}</strong>
      <small v-if="startDetail">{{ startDetail }}</small>
    </div>

    <div v-if="showFallback" class="video-fallback">
      <AppIcon name="video" />
      <p>{{ hint }}</p>
      <button v-if="playable && status === 'error'" class="btn sm" @click="attach">
        <AppIcon name="refresh" />重连
      </button>
    </div>

    <div class="video-top">
      <span class="video-badge" :class="badgeTone"><i />{{ badge }}</span>
      <span v-if="playable" class="video-proto">
        {{ transport === "whep" ? "WebRTC · 中继" : "FLV · 中继" }}
      </span>
    </div>

    <div v-if="playable" class="video-bottom">
      <button class="video-btn" :title="muted ? '取消静音' : '静音'" @click="toggleMute">
        <AppIcon name="sound" :style="{ opacity: muted ? 0.45 : 1 }" />
      </button>
      <button class="video-btn" title="全屏" @click="enterFullscreen">
        <AppIcon name="maximize" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.video-stage {
  position: relative;
  width: 100%;
  aspect-ratio: 16 / 9;
  border-radius: 10px;
  overflow: hidden;
  background: #0b1220;
  border: 1px solid var(--line);
}

.video-el {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
  opacity: 0;
  transition: opacity 0.2s;
}

.video-el.on {
  opacity: 1;
}

/* 重连期间垫在底下的最后一帧：和 video 同样 object-fit，切换不跳位 */
.video-poster {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.video-fallback {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: #94a3b8;
  padding: 12px;
  text-align: center;
  background: rgb(11 18 32 / 72%);
}

/* 起流 / 重连遮罩：只占中间一块，上一帧仍可见（既能说明在干活，也不冒充直播） */
.video-starting {
  position: absolute;
  left: 50%;
  top: 50%;
  transform: translate(-50%, -50%);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 4px;
  max-width: 82%;
  padding: 10px 14px;
  border-radius: 8px;
  background: rgb(11 18 32 / 78%);
  border: 1px solid rgb(148 163 184 / 22%);
  color: #e2e8f0;
  text-align: center;
  pointer-events: none;
}

.video-starting strong {
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.02em;
}

.video-starting small {
  font-size: 10px;
  line-height: 1.5;
  color: #94a3b8;
}

.video-starting-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #f59e0b;
  animation: pulse 1.2s infinite;
}

.video-fallback p {
  font-size: 12px;
  line-height: 1.6;
  margin: 0;
  max-width: 80%;
}

.video-top {
  position: absolute;
  top: 8px;
  left: 8px;
  right: 8px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  pointer-events: none;
}

.video-badge {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.06em;
  padding: 3px 7px;
  border-radius: 4px;
  background: rgb(15 23 42 / 72%);
}

.video-badge i {
  width: 6px;
  height: 6px;
  border-radius: 50%;
}

.video-badge.live {
  color: #f87171;
}

.video-badge.live i {
  background: #ef4444;
  animation: pulse 1.4s infinite;
}

.video-badge.wait {
  color: #fbbf24;
}

.video-badge.wait i {
  background: #f59e0b;
}

.video-badge.err {
  color: #fca5a5;
}

.video-badge.err i {
  background: #ef4444;
}

.video-badge.off {
  color: #94a3b8;
}

.video-badge.off i {
  background: #64748b;
}

.video-proto {
  font-size: 10px;
  color: #cbd5f5;
  background: rgb(15 23 42 / 72%);
  padding: 3px 6px;
  border-radius: 4px;
  font-family: var(--mono, monospace);
}

.video-bottom {
  position: absolute;
  right: 8px;
  bottom: 8px;
  display: flex;
  gap: 6px;
}

.video-btn {
  width: 26px;
  height: 26px;
  display: grid;
  place-items: center;
  border-radius: 6px;
  border: none;
  cursor: pointer;
  color: #e2e8f0;
  background: rgb(15 23 42 / 72%);
}

.video-btn:hover {
  background: rgb(15 23 42 / 92%);
}

@keyframes pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.35;
  }
}
</style>
