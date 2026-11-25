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
    this.underrun = 0;
    this.port.onmessage = (event) => {
      const data = event.data || {};
      if (data.mute !== undefined) this.muted = !!data.mute;
      if (data.samples) this.push(new Float32Array(data.samples));
    };
  }
  push(samples) {
    for (let i = 0; i < samples.length; i += 1) {
      this.buf[this.tail % this.cap] = samples[i];
      this.tail += 1;
    }
    // 积压超过 300ms：丢最旧的，把延迟压回来
    const limit = Math.floor(sampleRate * 0.3);
    if (this.tail - this.head > limit) this.head = this.tail - limit;
  }
  process(_inputs, outputs) {
    const out = outputs[0] && outputs[0][0];
    if (!out) return true;
    const primed = this.tail - this.head >= ${JITTER_PRIME_FRAMES * 320};
    if (!primed || this.muted) {
      this.underrun += 1;
      out.fill(0);
      return true;
    }
    const step = 16000 / sampleRate;
    for (let i = 0; i < out.length; i += 1) {
      this.phase += step;
      while (this.phase >= 1) {
        this.phase -= 1;
        this.head += 1;
      }
      if (this.tail - this.head < 2) {
        out[i] = 0;
        this.underrun += 1;
        continue;
      }
      const a = this.buf[this.head % this.cap];
      const b = this.buf[(this.head + 1) % this.cap];
      out[i] = a + (b - a) * this.phase;
    }
    return true;
  }
}
registerProcessor("phone-recorder", PhoneRecorder);
registerProcessor("phone-player", PhonePlayer);
`;

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
  const stats = reactive({ rx: 0, tx: 0, underruns: 0 });

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
    connecting.value = true;
    error.value = "";
    status.value = "连接中…";
    try {
      const token = tokenStorage.get() || "";
      if (!token) throw new Error("登录已过期，请重新登录");

      context = new AudioContext();
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
        if (bytes.byteLength < FRAME_BYTES) return;
        // Int16Array 需要偶数长度与 2 字节对齐
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
        if (framesSinceReport === 0) {
          status.value = active.value ? "通话中（没有音频到达）" : status.value;
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
