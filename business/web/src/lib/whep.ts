/**
 * 极简 WHEP（WebRTC-HTTP Egress Protocol）客户端。
 *
 * 两个用途，协议完全一样，只是地址不同：
 * 1. **现场直连**板端 mediamtx：`<webrtcBase>/<stream>/whep`（同内网 + 浏览器许可）；
 * 2. **平台中继**：`/api/backpack/whep/<stream>/whep`（平台同源反代到它自己的
 *    mediamtx，媒体仍是浏览器 ⇄ mediamtx 的 UDP，不过后端）。
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

import { isPrivateOrigin } from "@/utils/runtime";

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

/**
 * 浏览器抛的原始错误对现场没意义，换成能直接指导排查的一句话。
 *
 * 「failed to fetch」这一类**基本只有两种原因**，而且都跟板子无关（板子好着：
 * 同网段机器 `curl -X OPTIONS http://<背包>:8889/<stream>/whep` 会回 204）：
 *
 * 1. 页面本身是**公网地址**（`http://39.107.55.13:3100`）→ Chrome（141+ 灰度、157 全量）
 *    会拦截公网页面对内网 IP 的访问，除非用户在地址栏允许「本地网络访问」；
 * 2. 页面在内网但浏览器/系统层拦了（权限被拒过、防火墙、换了网段）。
 *
 * 所以把「本页来源 + 目标地址 + 现场入口」都打出来，现场不用猜（2026-11-18 反馈：
 * 操作员电脑明明在同一网段，却只看到一句「连不上板端视频服务」）。
 */
function friendly(error: unknown, url: string): string {
  const raw = error instanceof Error ? error.message : String(error);
  if (/failed to fetch|networkerror|load failed/i.test(raw)) {
    // 同源地址（`/api/backpack/whep/…`）= 平台侧 WHEP 代理，不是「浏览器直连板端」：
    // 它失败只有两种原因 —— 平台后端没在跑，或部署没开 mediamtx。
    const sameOrigin = url.startsWith("/") || url.startsWith(location.origin);
    if (sameOrigin) {
      return (
        `平台 WebRTC 出口不可达（${url}）：确认业务平台在运行、且服务器上 mediamtx 已启动` +
        `（systemctl status mediamtx）；并确认安全组放通了 UDP 8189（WebRTC 媒体）`
      );
    }
    let host = "板端";
    let entry = "现场入口 http://<背包IP>:8080";
    try {
      host = new URL(url).host;
      entry = `现场入口 http://${new URL(url).hostname}:8080`;
    } catch {
      /* 地址解析不出来就用默认文案 */
    }
    const origin = location.origin;
    if (isPrivateOrigin()) {
      return (
        `连不上板端视频服务（${host}）：本页来自 ${origin}（内网），确认背包 mediamtx 在跑、` +
        `浏览器允许本地网络访问；也可以切「平台中继」`
      );
    }
    return (
      `连不上板端视频服务（${host}）：本页来自 ${origin}（公网地址），Chrome 会拦截公网页面` +
      `访问内网 IP —— 在地址栏左侧允许「本地网络访问」后刷新，或用${entry}打开控制台，或切「平台中继」`
    );
  }
  if (/operationerror|not supported/i.test(raw)) {
    return "浏览器不支持该视频编码（背包推的是 H264）";
  }
  return raw;
}

/**
 * WHEP 的信号失败翻译成人话。
 *
 * `404` 是背包现场最常见的一种，而且**不是浏览器/网络问题**：mediamtx 只会在
 * 那条 path 上有推流（publisher）时才肯给 SDP，否则回
 * `{"error":"no stream is available on path 'dogcam'"}`。
 * 现场 2026-11-18 的实际成因是**上游没就绪**：板端重启后本体 RTSP
 * （`192.168.234.1:8554/test`）还没起来 / 正在重连，`dogcam` path 已登记但暂时没流
 * （mediamtx 日志 `dial tcp ... connect: connection refused` / `404 Not Found`）。
 * 其它可能：path 没登记、名字不一致（如写成 `dog-front-cam`）、直拉源账号密码不对。
 */
function whepFailure(status: number, url: string): string {
  const stream = /\/([^/]+)\/whep(?:\?|$)/.exec(url)?.[1] ?? "该";
  if (status === 404) {
    // 两个部署都会回 404，都是「这一路还没有流」：
    // - 浏览器直连板端：本体 RTSP 未就绪 / path 未登记；
    // - 平台反代：板端 relay 还没起 ffmpeg（按需推流，第一个观看者进来才起，3~7s）。
    return (
      `「${stream}」暂时没有流：板端还没起 ffmpeg（按需推流，约 3~7s）/ 本体 RTSP 未就绪，` +
      `10~30 秒会自动恢复；持续不恢复请看板端 relay 日志与本地 mediamtx`
    );
  }
  if (status === 401 || status === 403) return "拒绝访问（鉴权失败）";
  if (status === 400 || status === 415) return "不接受这段 SDP（编码或协议不匹配）";
  if (status === 503)
    return "平台 WebRTC 出口未启用（mediamtx 未部署 / 未配置 ROBODOG_MEDIAMTX_URL）";
  return `WHEP 协商失败（HTTP ${status}）`;
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
    // 会话地址必须用**绝对**基址解析：走平台反代时 `options.url` 是相对路径
    // （`/api/backpack/whep/<stream>/whep`），而 `new URL(path, base)` 的 base 必须是
    // 绝对地址，否则直接抛 `Invalid base URL` —— 外层 catch 会把它当「连接失败」重连，
    // 于是 answer 永远不 setRemoteDescription、ICE 一次都不开始，页面就一直「重连中」，
    // 服务端只看到 `session … deadline exceeded`（2026-11-19 现场就是这么误判成网络问题的）。
    const absoluteUrl = new URL(options.url, window.location.origin).toString();
    try {
      const next = new RTCPeerConnection({ iceServers: await iceServersOf(absoluteUrl) });
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
      const response = await fetch(absoluteUrl, {
        method: "POST",
        headers: { "Content-Type": "application/sdp", Accept: "application/sdp" },
        body: next.localDescription?.sdp ?? ""
      });
      if (!response.ok) {
        scheduleRetry(whepFailure(response.status, absoluteUrl));
        return;
      }
      const location = response.headers.get("Location");
      if (location) {
        // mediamtx 回的是站内绝对路径（`/bp_dogcam/whep/<id>`）；板端直连时 URL 是
        // 绝对地址，用请求地址当 base 两边都对
        sessionUrl = new URL(location, absoluteUrl).toString();
      }
      const answer = await response.text();
      await next.setRemoteDescription({ type: "answer", sdp: answer });
    } catch (error) {
      scheduleRetry(friendly(error, absoluteUrl));
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
