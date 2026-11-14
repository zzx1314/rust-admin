/**
 * 极简 WHEP（WebRTC-HTTP Egress Protocol）客户端。
 *
 * 背包板端用 mediamtx 的 WebRTC 出流：`<webrtcBase>/<stream>/whep`。
 * 平台自带的 `VideoPlayer.vue` 走的是 FLV/WS 中继（平台中继设备推上来的流），
 * 与这里不是同一类通路 —— 背包是**浏览器直连板端**，要的是低延时，所以单独实现。
 *
 * 只做三件事：
 * 1. `OPTIONS` 取 ICE 服务器（mediamtx 用 `Link: <stun:…>; rel="ice-server"` 声明）；
 * 2. 等 ICE 收集完再 `POST` 完整 SDP（非 trickle，省掉 PATCH 与重传）；
 * 3. 出问题时指数退避重连，`close()` 时 `DELETE` 会话，别在板端堆垃圾会话。
 *
 * 与 Demo 的 `reader.js`（mediamtx 官方）差别：不做音频 codec 的 SDP 改写。
 * 背包推流没有音轨，官方 reader 的音频 munging 在 Chromium 上会抛
 * `setLocalDescription OperationError`，这也是 Demo 里显式传
 * `skipAudioCodecHacks: true` 的原因。
 */

export type WhepPhase = "connecting" | "live" | "retry" | "closed";

export interface WhepDetail {
  phase: WhepPhase;
  attempt: number;
  message: string;
}

export interface WhepSession {
  close: () => void;
}

export interface WhepOptions {
  /** 完整 WHEP 地址，例如 `http://192.168.234.200:8889/cam0/whep` */
  url: string;
  video: HTMLVideoElement;
  onPhase: (detail: WhepDetail) => void;
  /** ICE 收集等待上限：超过就用手上已有的候选发出，局域网通常 <100ms */
  iceWaitMs?: number;
}

const DEFAULT_ICE_WAIT_MS = 2500;
const MAX_BACKOFF_MS = 8000;

function waitIceGathering(peer: RTCPeerConnection, timeoutMs: number): Promise<void> {
  if (peer.iceGatheringState === "complete") return Promise.resolve();
  return new Promise<void>(resolve => {
    const timer = setTimeout(finish, timeoutMs);
    function finish() {
      clearTimeout(timer);
      peer.removeEventListener("icegatheringstatechange", check);
      resolve();
    }
    function check() {
      if (peer.iceGatheringState === "complete") finish();
    }
    peer.addEventListener("icegatheringstatechange", check);
  });
}

/** 从 WHEP 的 `OPTIONS` 响应里取 ICE 服务器；取不到就只靠 host candidate（局域网够用） */
async function iceServersOf(url: string): Promise<RTCIceServer[]> {
  try {
    const response = await fetch(url, { method: "OPTIONS" });
    const link = response.headers.get("Link") || response.headers.get("link") || "";
    const servers: RTCIceServer[] = [];
    for (const entry of link.split(",")) {
      const uri = /<([^>]+)>/.exec(entry)?.[1];
      if (!uri) continue;
      if (!/rel\s*=\s*"?ice-server"?/i.test(entry)) continue;
      servers.push({ urls: uri });
    }
    return servers;
  } catch {
    return [];
  }
}

/** 浏览器抛的原始错误对现场没意义，换成能直接指导排查的一句话 */
function friendly(error: unknown): string {
  const raw = error instanceof Error ? error.message : String(error);
  if (/failed to fetch|networkerror|load failed/i.test(raw)) {
    return "连不上板端视频服务（mediamtx :8889）";
  }
  if (/operationerror|not supported/i.test(raw)) {
    return "浏览器不支持该视频编码（背包推的是 H264）";
  }
  return raw;
}

export function startWhep(options: WhepOptions): WhepSession {
  const iceWaitMs = options.iceWaitMs ?? DEFAULT_ICE_WAIT_MS;
  let closed = false;
  let attempt = 0;
  let peer: RTCPeerConnection | null = null;
  let sessionUrl = "";
  let retryTimer: ReturnType<typeof setTimeout> | null = null;

  const emit = (phase: WhepPhase, message = "") =>
    options.onPhase({ phase, attempt, message });

  function cleanupPeer() {
    if (!peer) return;
    peer.onconnectionstatechange = null;
    peer.ontrack = null;
    try {
      peer.close();
    } catch {
      /* 已关闭 */
    }
    peer = null;
  }

  function endSession() {
    if (!sessionUrl) return;
    const target = sessionUrl;
    sessionUrl = "";
    // 失败就算了：板端 mediamtx 自己会回收超时会话
    void fetch(target, { method: "DELETE" }).catch(() => undefined);
  }

  function scheduleRetry(reason: string) {
    if (closed) return;
    cleanupPeer();
    endSession();
    options.video.srcObject = null;
    attempt += 1;
    const delay = Math.min(1000 * 2 ** (attempt - 1), MAX_BACKOFF_MS);
    emit("retry", `${reason}，${Math.round(delay / 1000)} 秒后重连`);
    retryTimer = setTimeout(() => {
      retryTimer = null;
      void connect();
    }, delay);
  }

  async function connect() {
    if (closed) return;
    emit("connecting", attempt ? `第 ${attempt + 1} 次重连…` : "");
    try {
      const next = new RTCPeerConnection({ iceServers: await iceServersOf(options.url) });
      peer = next;
      // 只要视频：背包推流本身没有音轨
      next.addTransceiver("video", { direction: "recvonly" });
      next.ontrack = event => {
        if (closed) return;
        options.video.srcObject =
          event.streams[0] ?? new MediaStream([event.track]);
        void options.video.play().catch(() => undefined);
        attempt = 0;
        emit("live");
      };
      next.onconnectionstatechange = () => {
        if (closed || peer !== next) return;
        if (next.connectionState === "failed") {
          scheduleRetry("WebRTC 连接失败");
        } else if (next.connectionState === "disconnected") {
          // 瞬断先给它 3s 自愈窗口（ICE restart / 网络抖动），别立刻重建会话
          setTimeout(() => {
            if (!closed && peer === next && next.connectionState === "disconnected") {
              scheduleRetry("WebRTC 连接中断");
            }
          }, 3000);
        }
      };

      await next.setLocalDescription(await next.createOffer());
      await waitIceGathering(next, iceWaitMs);
      if (closed) {
        cleanupPeer();
        return;
      }
      const response = await fetch(options.url, {
        method: "POST",
        headers: { "Content-Type": "application/sdp", Accept: "application/sdp" },
        body: next.localDescription?.sdp ?? ""
      });
      if (!response.ok) {
        scheduleRetry(`WHEP 协商失败（HTTP ${response.status}）`);
        return;
      }
      const location = response.headers.get("Location");
      if (location) {
        sessionUrl = new URL(location, options.url).toString();
      }
      const answer = await response.text();
      await next.setRemoteDescription({ type: "answer", sdp: answer });
    } catch (error) {
      scheduleRetry(friendly(error));
    }
  }

  void connect();

  return {
    close() {
      closed = true;
      if (retryTimer) {
        clearTimeout(retryTimer);
        retryTimer = null;
      }
      cleanupPeer();
      endSession();
      options.video.srcObject = null;
    }
  };
}
