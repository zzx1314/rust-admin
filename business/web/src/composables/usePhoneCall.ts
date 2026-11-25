/**
 * 网页对讲（浏览器 ⇄ 板端音频，经设备客户端中继）。
 *
 * 板端 `voice_call` 只会跟同网段的 UDP 对端说话，公网浏览器发不了裸 UDP。
 * 设备客户端（与板端同机）当那个「对端」，音频借设备 WS 上下行：
 *
 * ```text
 * 本页面 ──WS /api/devices/{id}/audio/ws?token=…──▶ 平台 ──▶ 客户端 ──UDP──▶ 板端
 *        ◀────────── 二进制 PCM16@16k / 20ms ─────────────────────────
 * ```
 *
 * 两端都不做编解码：板端就是 16kHz / 单声道 / S16_LE / 20ms（640B 一帧），
 * 浏览器这边用两个 AudioWorklet 做「48k ⇄ 16k 重采样」和抖动缓冲。
 *
 * 三条约定（改这块先看）：
 * 1. **必须先起播放再连 WS**：不然前几帧会被丢掉，听起来像「对面没说话」；
 * 2. **一定要收尾**：关弹窗 / 切任务 / 页面卸载都要 `stop()`，否则板子停在 phone
 *    模式（唤醒词失效、还占着 FIFO），平台侧也会一直以为有人在通话；
 * 3. **建议戴耳机**：浏览器 AEC 能压掉一部分回授，但外放 + 大音量仍会啸叫。
 */
import { onBeforeUnmount, reactive, ref } from "vue";
import { ElMessage } from "element-plus";
import { tokenStorage } from "@/api/http";
import { wsOrigin } from "@/utils/runtime";

/** 20ms @ 16kHz 单声道 S16 = 640 字节 */
const FRAME_BYTES = 640;
/** 抖动缓冲目标：攒到 3 帧（60ms）再出声，与 PC 端脚本一致 */
const JITTER_PRIME_FRAMES = 3;
/** 工作线程源码（内联成 Blob URL，省掉打包 worklet 的那套配置） */
const WORKLET_SOURCE = `
// 48k → 16k 采集：3 取平均后打包成 20ms 的 Int16 帧发给主线程
class PhoneRecorder extends AudioWorkletProcessor {
  constructor() {
    super();
    this.acc = 0;
    this.sum = 0;
    this.out = new Int16Array(320);
    this.n = 0;
    this.muted = false;
    this.port.onmessage = (event) => {
      const data = event.data || {};
      if (data.mute !== undefined) this.muted = !!data.mute;
    };
  }
  process(inputs) {
    const channel = inputs[0] && inputs[0][0];
    if (!channel) return true;
    for (let i = 0; i < channel.length; i += 1) {
      this.sum += channel[i];
      this.acc += 1;
      if (this.acc < 3) continue;
      const value = Math.max(-1, Math.min(1, this.sum / 3));
      this.out[this.n] = value < 0 ? value * 0x8000 : value * 0x7fff;
      this.sum = 0;
      this.acc = 0;
      this.n += 1;
      if (this.n < this.out.length) continue;
      if (!this.muted) this.port.postMessage(this.out.slice());
      this.out = new Int16Array(320);
      this.n = 0;
    }
    return true;
  }
}
// 16k → 48k 播放：单调递增的读写指针 + 线性插值；数据不够就出静音
class PhonePlayer extends AudioWorkletProcessor {
  constructor() {
    super();
    this.cap = 16000;            // 1s 的环，够装 50 帧
    this.buf = new Float32Array(this.cap);
    this.head = 0;               // 读指针（单调递增）
    this.tail = 0;               // 写指针（单调递增）
    this.phase = 0;
    this.muted = false;
    // 欠载统计：数据不够、只能出静音的渲染块数（每秒回给主线程算成 ms）
    this.starvedBlocks = 0;
    this.quantum = 128;          // 渲染量子，首次 process 时按实际值覆盖
    this.reportedAt = 0;
    this.port.onmessage = (event) => {
      const data = event.data || {};
      if (data.mute !== undefined) this.muted = !!data.mute;
      if (data.samples) this.push(data.samples);
    };
  }
  // ⚠️ 必须自己把 Int16 的 **数值** 归一化到 [-1,1]：Web Audio 的 output 是
  // Float32 归一化信号，|x|>1 会被渲染器硬钳到 ±1 —— 满幅方波听起来就是
  // 「对讲机嗡嗡」。也别指望 \`new Float32Array(int16Array)\`：它是逐元素赋值
  // （30000 还是 30000），不会除以 32768。
  push(pcm) {
    for (let i = 0; i < pcm.length; i += 1) {
      this.buf[this.tail % this.cap] = pcm[i] / 32768;
      this.tail += 1;
    }
    // 积压超过 300ms：丢最旧的，把延迟压回来（limit 是 16k 采样数，别用 sampleRate）
    const limit = Math.floor(this.cap * 0.3);
    if (this.tail - this.head > limit) this.head = this.tail - limit;
  }
  // 每秒一次把「欠载 / 积压」回给主线程：不然页面只能盯着「收 N 帧」猜为什么卡
  report(quantum) {
    if (quantum) this.quantum = quantum;
    if (currentTime - this.reportedAt < 1) return;
    this.reportedAt = currentTime;
    this.port.postMessage({
      type: "stats",
      starvedMs: Math.round((this.starvedBlocks * this.quantum * 1000) / sampleRate),
      backlogMs: Math.round(((this.tail - this.head) / 16000) * 1000)
    });
  }
  process(_inputs, outputs) {
    const out = outputs[0] && outputs[0][0];
    if (!out) return true;
    const primed = this.tail - this.head >= ${JITTER_PRIME_FRAMES * 320};
    if (!primed || this.muted) {
      // 自己按的「静音」不算欠载（那是有意的静默），数据不够才算
      if (!this.muted) this.starvedBlocks += 1;
      out.fill(0);
      this.report(out.length);
      return true;
    }
    const step = 16000 / sampleRate;
    let starved = false;
    for (let i = 0; i < out.length; i += 1) {
      this.phase += step;
      while (this.phase >= 1) {
        this.phase -= 1;
        this.head += 1;
      }
      if (this.tail - this.head < 2) {
        out[i] = 0;
        starved = true;
        continue;
      }
      const a = this.buf[this.head % this.cap];
      const b = this.buf[(this.head + 1) % this.cap];
      out[i] = a + (b - a) * this.phase;
    }
    if (starved) this.starvedBlocks += 1;
    this.report(out.length);
    return true;
  }
}
registerProcessor("phone-recorder", PhoneRecorder);
registerProcessor("phone-player", PhonePlayer);
`;

/**
 * 浏览器能不能做网页对讲。
 *
 * `AudioWorklet` 与 `navigator.mediaDevices.getUserMedia` 都**只在安全上下文**可用：
 * `https://` / `http://localhost` / 被浏览器手动加白的 http 源。用
 * `http://39.107.55.13:3008` 这种明文 IP 打开时两者都是 undefined —— 直接点会炸成
 * 「Cannot read properties of undefined (reading 'addModule')」，所以要提前拦。
 */
export function phoneCallSupport(): {
  ok: boolean;
  reason: string;
  /** 是「页面不是安全上下文」（能靠 https / 浏览器白名单解决），而不是浏览器太旧 */
  insecure: boolean;
} {
  // ⚠️ 不要碰 `AudioContext.prototype.audioWorklet`：它是 `BaseAudioContext` 上的
  // **accessor**，在原型对象上读会抛 `TypeError: Illegal invocation`（我们踩过：
  // 计划管理页在 setup 阶段直接白屏）。用 `typeof AudioWorkletNode` 判断：
  // 它是 `[SecureContext]` 暴露的全局构造器，不安全上下文里根本不存在。
  try {
    const secure = typeof window !== "undefined" && window.isSecureContext === true;
    const hasWorklet = typeof AudioWorkletNode !== "undefined";
    const hasMic = !!navigator?.mediaDevices?.getUserMedia;
    if (secure && hasWorklet && hasMic) {
      return { ok: true, reason: "", insecure: false };
    }
    if (!secure) {
      const host = typeof location === "undefined" ? "" : location.host;
      const origin = typeof location === "undefined" ? "" : location.origin;
      return {
        ok: false,
        insecure: true,
        reason:
          `当前页面是 http（${host}），浏览器只在 https 下开放麦克风与 AudioWorklet。` +
          `请用 https 访问，或把 ${origin} 加入浏览器「当作安全来源」白名单后重开页面`
      };
    }
    return {
      ok: false,
      insecure: false,
      reason: "当前浏览器不支持网页对讲（需要 AudioWorklet + 麦克风权限），建议用较新的 Chrome / Edge"
    };
  } catch (cause) {
    // 再兜一层：这段代码跑在页面 setup 里，任何异常都会让整个计划管理页白屏
    return {
      ok: false,
      insecure: false,
      reason: `无法检测浏览器能力（${cause instanceof Error ? cause.message : String(cause)}）`
    };
  }
}

export interface PhoneCallOptions {
  /** 设备主键（`devices.id`） */
  deviceId: number;
}

export function usePhoneCall() {
  const active = ref(false);
  const connecting = ref(false);
  const muted = ref(false);
  const status = ref("未开始");
  const error = ref("");
  // `underruns` = 播放欠载的毫秒数（worklet 每秒回报）；`drops` = 因帧长不符被整条丢弃的下行帧
  // （板端不是 640B/20ms 时的唯一痕迹）；`backlog` = 播放缓冲积压（ms，≈ 端到端延迟）
  const stats = reactive({ rx: 0, tx: 0, underruns: 0, drops: 0, backlog: 0 });

  let socket: WebSocket | null = null;
  let context: AudioContext | null = null;
  let player: AudioWorkletNode | null = null;
  let recorder: AudioWorkletNode | null = null;
  let mic: MediaStream | null = null;
  let workletUrl: string | null = null;
  let framesSinceReport = 0;
  let reportTimer: number | null = null;

  function reset() {
    active.value = false;
    connecting.value = false;
    muted.value = false;
    stats.rx = 0;
    stats.tx = 0;
    stats.underruns = 0;
    stats.drops = 0;
    stats.backlog = 0;
  }

  function teardown() {
    if (reportTimer !== null) {
      window.clearInterval(reportTimer);
      reportTimer = null;
    }
    if (recorder) {
      recorder.port.onmessage = null;
      recorder.disconnect();
      recorder = null;
    }
    if (player) {
      player.port.onmessage = null;
      player.disconnect();
      player = null;
    }
    if (mic) {
      mic.getTracks().forEach(track => track.stop());
      mic = null;
    }
    if (context) {
      void context.close().catch(() => undefined);
      context = null;
    }
    if (workletUrl) {
      URL.revokeObjectURL(workletUrl);
      workletUrl = null;
    }
    const ws = socket;
    socket = null;
    if (ws && ws.readyState <= WebSocket.OPEN) {
      try {
        ws.close();
      } catch {
        // 关不掉就算了，服务端会按「浏览器断开」收尾
      }
    }
  }

  /**
   * 开始通话：先起播放链路，再连 WS（顺序反了会丢头几帧）。
   *
   * 失败（没有麦克风权限 / 设备客户端不在线 / 板端 asr 没起来）都会把原因写进
   * `error` 并收尾，页面按 `status` 展示。
   */
  async function start({ deviceId }: PhoneCallOptions) {
    if (active.value || connecting.value) return;
    // 不安全上下文（http + 非 localhost）下 AudioWorklet / getUserMedia 都不存在，
    // 先给一句人话，别让页面抛「Cannot read properties of undefined」
    const support = phoneCallSupport();
    if (!support.ok) {
      connecting.value = false;
      error.value = support.reason;
      status.value = support.insecure ? "需要 https" : "浏览器不支持";
      return;
    }
    connecting.value = true;
    error.value = "";
    status.value = "连接中…";
    try {
      const token = tokenStorage.get() || "";
      if (!token) throw new Error("登录已过期，请重新登录");

      context = new AudioContext();
      if (!context.audioWorklet) {
        throw new Error(
          "浏览器没有开放 AudioWorklet（需要 https 或把本站加入「当作安全来源」白名单）"
        );
      }
      workletUrl = URL.createObjectURL(
        new Blob([WORKLET_SOURCE], { type: "application/javascript" })
      );
      await context.audioWorklet.addModule(workletUrl);
      if (context.state === "suspended") await context.resume();

      player = new AudioWorkletNode(context, "phone-player", {
        numberOfInputs: 0,
        numberOfOutputs: 1,
        outputChannelCount: [1]
      });
      player.connect(context.destination);
      // 播放侧每秒回报一次欠载 / 积压（worklet 自己数的，主线程拿不到）
      player.port.onmessage = event => {
        const data = event.data as
          | { type?: string; starvedMs?: number; backlogMs?: number }
          | undefined;
        if (!data || data.type !== "stats") return;
        if (typeof data.starvedMs === "number") stats.underruns = data.starvedMs;
        if (typeof data.backlogMs === "number") stats.backlog = data.backlogMs;
      };

      mic = await navigator.mediaDevices.getUserMedia({
        audio: {
          channelCount: 1,
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true
        }
      });
      recorder = new AudioWorkletNode(context, "phone-recorder", {
        numberOfInputs: 1,
        numberOfOutputs: 1,
        outputChannelCount: [1]
      });
      // 采集节点的输出必须接出去，否则音频图不会拉它（接一个静音增益即可）
      const sink = context.createGain();
      sink.gain.value = 0;
      recorder.connect(sink);
      sink.connect(context.destination);
      const source = context.createMediaStreamSource(mic);
      source.connect(recorder);

      recorder.port.onmessage = event => {
        const frame = event.data as Int16Array | undefined;
        if (!frame) return;
        if (!socket || socket.readyState !== WebSocket.OPEN) return;
        if (muted.value) return;
        socket.send(frame.buffer);
        stats.tx += 1;
      };

      const ws = new WebSocket(
        `${wsOrigin()}/api/devices/${deviceId}/audio/ws?token=${encodeURIComponent(token)}`
      );
      ws.binaryType = "arraybuffer";
      socket = ws;

      ws.onmessage = message => {
        if (typeof message.data === "string") {
          let payload: { type?: string; state?: string; error?: string } = {};
          try {
            payload = JSON.parse(message.data);
          } catch {
            return;
          }
          if (payload.type !== "phone_state") return;
          if (payload.state === "started") {
            active.value = true;
            connecting.value = false;
            status.value = "通话中";
          } else if (payload.state === "error" || payload.state === "stopped") {
            error.value = payload.error || "通话已结束";
            status.value = payload.state === "stopped" ? "已挂断" : "通话失败";
            teardown();
            reset();
            if (error.value) ElMessage.error(error.value);
          }
          return;
        }
        if (!(message.data instanceof ArrayBuffer)) return;
        const bytes = new Uint8Array(message.data);
        // 板端固定 20ms@16k = 640B：短于这个长度没法按 2 字节对齐出一帧完整 PCM，
        // 整条丢弃**并计数**（以前是静默丢，板端换成 10ms/8k 时表现为「没声音」且查不出）
        if (bytes.byteLength < FRAME_BYTES) {
          stats.drops += 1;
          return;
        }
        // Int16Array 需要偶数长度与 2 字节对齐（奇数长度截掉尾部半个采样）
        const samples = new Int16Array(
          bytes.buffer,
          bytes.byteOffset,
          Math.floor(bytes.byteLength / 2)
        );
        // 拷一份再转移：WS 的 ArrayBuffer 会复用，直接转移过去会读到别的帧
        const copy = new Int16Array(samples);
        player?.port.postMessage({ samples: copy }, [copy.buffer]);
        stats.rx += 1;
        framesSinceReport += 1;
      };

      ws.onerror = () => {
        error.value = "WebSocket 连接失败（鉴权 / 网络）";
      };
      ws.onclose = () => {
        if (active.value) {
          error.value = error.value || "通话连接已断开";
          status.value = "已断开";
        }
        teardown();
        reset();
      };

      reportTimer = window.setInterval(() => {
        if (framesSinceReport === 0 && active.value) {
          status.value = stats.drops > 0
            ? `通话中（帧长不是 640B，已丢 ${stats.drops} 条）`
            : "通话中（没有音频到达）";
        }
        framesSinceReport = 0;
      }, 5000);

      status.value = "等待板端接通…";
    } catch (cause) {
      error.value = cause instanceof Error ? cause.message : String(cause);
      status.value = "通话失败";
      teardown();
      reset();
    }
  }

  /** 挂断（幂等）：停麦克风 / 播放，关 WS —— 服务端据此下发 `phone_stream stop` */
  async function stop(_reason = "挂断") {
    if (!socket && !context) {
      reset();
      return;
    }
    status.value = "已挂断";
    teardown();
    reset();
  }

  /** 静音：只是不再上传麦克风（下行照常听得见） */
  function setMuted(value: boolean) {
    muted.value = value;
    recorder?.port.postMessage({ mute: value });
  }

  onBeforeUnmount(() => {
    void stop("页面关闭");
  });

  return { active, connecting, muted, status, error, stats, start, stop, setMuted };
}
