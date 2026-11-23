'use strict';

// 全局错误兜底：任何脚本异常都显示到连接状态栏，避免静默失败
window.addEventListener('error', (e) => {
  const c = document.getElementById('conn');
  if (c) { c.textContent = 'JS错误: ' + (e.message || e.error); c.className = 'conn bad'; }
  console.error(e);
});

const state = {
  webrtc: { host: location.hostname, port: 8889 },
  cameras: {},
  infrared: { stream: 'dog-ipcam' },
  relay: null,                 // /api/relay/status 快照
};

// ---------- 机器狗前置链路状态 ----------
// 旧逻辑：relay.enabled=false 时把第 6 张卡 display:none 并移除 dogcam 单选，
// 但那张卡仍会创建 WHEP reader，连不上就 degrade 成 iframe（黑框），还被
// 统计成「1 路走了内置播放页」。现在先拿状态，未配置源就干脆不创建 reader，
// 保留一张写清原因的占位卡；AI 目标被改动时在状态栏明确说明。
async function fetchRelayStatus() {
  try {
    const r = await Promise.race([
      fetch('/api/relay/status', { cache: 'no-store' }).then(x => x.json()),
      new Promise(res => setTimeout(() => res(null), 1200)),
    ]);
    return r || null;
  } catch (_) { return null; }
}

function applyRelayUi(disabled) {
  const card = cardOf('dogcam');
  if (card && disabled) {
    card.classList.add('dogcam-off');
    const v = card.querySelector('video');
    if (v) v.style.display = 'none';
    const msg = card.querySelector('.vc-msg');
    if (msg) msg.textContent =
      '未配置机器狗前置源：板端设置 DOG_RTSP_URL，或创建 backend/dog_rtsp_url 后重启 agent';
  }
  const radio = document.querySelector('#detect-cams input[value="dogcam"]');
  const hint = document.getElementById('dogcam-hint');
  if (disabled) {
    if (radio) {
      radio.disabled = true;
      radio.checked = false;
      if (radio.closest('label')) radio.closest('label').title = '板端未配置机器狗前置 RTSP 源';
    }
    // 默认项不可用时补选第一路（cam0），否则「开始识别」会因没有目标路而失败
    const first = document.querySelector('#detect-cams input[name="detect-cam"]:not(:disabled)');
    if (first) first.checked = true;
    if (hint) hint.textContent =
      '机器狗前置未配置 RTSP 源（relay.enabled=false）：AI 目标已临时改为 cam0；配置并重启 agent 后自动恢复默认「机器狗前置」。';
  } else if (hint) {
    hint.textContent = '';
  }
}

// ---------- 初始化摄像头映射与 WebRTC 地址 ----------
async function initMeta() {
  try {
    const r = await fetch('/api/cameras');
    const m = await r.json();
    state.webrtc = m.webrtc;
    state.cameras = m.cameras;
    state.infrared = m.infrared;
  } catch (e) {
    console.warn('获取摄像头元数据失败，使用默认', e);
  }
}

// ---------- WebRTC 拉流（mediamtx WHEP，使用官方 reader.js） ----------
// 候选 1：同源信令代理 /api/whep/<stream>（8000 -> 8889 的跨域由后端转发，避免 CORS）
// 候选 2：浏览器直连 http://<host>:<port>/<stream>/whep（原逻辑，代理异常时兜底）
// 只有连续多次协商失败才降级到 mediamtx 内置播放页 iframe；降级后仍定时重试，
// 推流/mediamtx 就绪后自动撤掉 iframe 回到 <video>，识别框才能叠加。
// 注意：reader.js 内部对瞬时错误每 2s 自动重连，绝不能第一次 onError 就永久降级。
function connectWebrtc(videoEl, msgEl, stream) {
  const { host, port } = state.webrtc;
  // 版本 2：AI 开启时这张卡会被切到 preview（背包本地画好框的流），
  // 停止或换目标路时切回 baseStream。data-stream 属性始终保留原始路名，
  // cardOf() 才能继续按原始路名找到这张卡。
  let curStream = stream;
  // reader.js 用 new URL(location, conf.url) 解析会话地址，conf.url 必须是绝对 URL
  const urlsOf = (s) => [
    new URL('/api/whep/' + encodeURIComponent(s), location.href).href,
    `http://${host}:${port}/${s}/whep`,
  ];
  const playerUrlOf = (s) => `http://${host}:${port}/${s}`;
  const MAX_ERR = 3;   // 每个候选允许的连续失败次数（reader 每 2s 自愈重试）
  const RETRY_MS = 10000;
  const card = videoEl.closest('.video-card');
  let idx = 0, errCount = 0, everConnected = false, degraded = false, lastErr = '';
  let retryTimer = null;
  // 「切到 preview」是一次试探：若新流在 6s 内没出图，必须自动切回原干净流，
  // 否则用户原本正常播放的画面会永久停在一张黑帧上。
  let switchGuard = null;

  videoEl.muted = true;
  videoEl.playsInline = true;
  videoEl.autoplay = true;

  function clearSwitchGuard() {
    if (switchGuard && switchGuard.timer) clearTimeout(switchGuard.timer);
    switchGuard = null;
  }

  function closeReader() {
    try { videoEl._reader && videoEl._reader.close(); } catch (e) {}
    videoEl._reader = null;
  }

  // 从 iframe 播放页回到父页 <video>：移除 no-overlay，SVG 叠加重新生效
  function restoreDirect() {
    const iframe = card && card.querySelector('iframe');
    if (iframe) iframe.remove();
    videoEl.style.display = '';
    if (card) {
      card.classList.remove('no-overlay');
      const ov = card.querySelector('.detect-svg');
      if (ov) ov.style.display = '';
    }
    degraded = false;
    if (retryTimer) { clearTimeout(retryTimer); retryTimer = null; }
  }

  function degrade(reason) {
    if (degraded) return;
    // 这次降级发生在「试探 preview 流」期间：不要插 iframe（那会在卡片里
    // 留下一个黑色空框），直接通知上层把画面切回原来的干净流。
    if (switchGuard && !everConnected) {
      const g = switchGuard;
      switchGuard = null;
      if (g.timer) clearTimeout(g.timer);
      if (retryTimer) { clearTimeout(retryTimer); retryTimer = null; }
      closeReader();
      setTimeout(() => g.onFail(g.stream, g.from, String(reason || '')), 0);
      return;
    }
    degraded = true;
    closeReader();
    if (!card.querySelector('iframe')) {
      const iframe = document.createElement('iframe');
      iframe.src = playerUrlOf(curStream) + '?muted=true&autoplay=true';
      iframe.allow = 'autoplay; encrypted-media';
      // 必须绝对定位铺满卡片：iframe 是行内替换元素，只给 width/height:100%
      // 在 aspect-ratio 容器里会退回默认 150px，卡片下半部露出一块黑框。
      iframe.style.cssText =
        'position:absolute;inset:0;display:block;width:100%;height:100%;border:0;background:#0b1220';
      card.insertBefore(iframe, videoEl);
    }
    videoEl.style.display = 'none';
    // iframe 是独立浏览上下文，父页无法得知里面视频的真实显示区域，
    // 按其尺寸算出来的框必然错位 —— 该路暂时禁用叠加。
    card.classList.add('no-overlay');
    const ov = card.querySelector('.detect-svg');
    if (ov) { ov.style.display = 'none'; ov.innerHTML = ''; }
    let short = String(reason || 'WHEP 不可用');
    if (/setLocalDescription/i.test(short)) short = '浏览器 SDP 改写不兼容(Chromium/Edge)';
    short = short.slice(0, 46);
    msgEl.textContent = '内置播放页 · 直连失败(' + short + ')，10s 后重试';
    msgEl.title = 'WHEP 直连失败：' + String(reason || '') + '（鼠标悬停查看；成功后自动恢复框叠加）';
    msgEl.classList.add('ok');
    retryTimer = setTimeout(() => {
      degraded = false;
      errCount = 0;
      start(0);
    }, RETRY_MS);
  }

  function start(i) {
    idx = i;
    const urls = urlsOf(curStream);
    closeReader();
    msgEl.textContent = '协商中…';
    msgEl.classList.remove('ok');
    const reader = new MediaMTXWebRTCReader({
      url: urls[idx],
      // 四路推流只有 H264 视频、无音轨：跳过官方 reader.js 的音频 codec
      // SDP 改写，否则 Chromium(Edge/Chrome) 的 setLocalDescription 会抛
      // OperationError，而 Firefox 正常。
      skipAudioCodecHacks: true,
      onTrack: (evt) => {
        everConnected = true;
        clearSwitchGuard();          // 新流真的出图了，取消「切回原流」的兜底
        restoreDirect();
        if (videoEl.srcObject !== evt.streams[0]) videoEl.srcObject = evt.streams[0];
        msgEl.textContent = idx === 0 ? '已连接（同源信令）' : '已连接';
        msgEl.title = urls[idx];
        msgEl.classList.add('ok');
      },
      onError: (err) => {
        lastErr = String(err || '');
        // 已经成功出过画面：交给 reader.js 自己重连，不要再降级
        if (everConnected) return;
        errCount += 1;
        if (errCount < MAX_ERR) {
          msgEl.textContent = `协商重试 ${errCount}/${MAX_ERR}…`;
          msgEl.title = lastErr;
          return;
        }
        errCount = 0;
        if (idx + 1 < urls.length) { start(idx + 1); return; }
        degrade(lastErr || 'WHEP 协商失败');
      },
    });
    videoEl._reader = reader; // 便于卸载时 close()
  }

  // 切换这张卡实际拉的 mediamtx path（切 preview / 切回干净流）。
  // 不改动 card 的 data-stream，SVG 定位与 cardOf() 依旧按原始路名工作。
  function switchStream(next, onFail) {
    if (!next || next === curStream) return;
    clearSwitchGuard();
    const from = curStream;
    curStream = next;
    if (retryTimer) { clearTimeout(retryTimer); retryTimer = null; }
    degraded = false;
    everConnected = false;
    errCount = 0;
    closeReader();
    const iframe = card && card.querySelector('iframe');
    if (iframe) iframe.remove();
    videoEl.style.display = '';
    if (card) {
      card.classList.remove('no-overlay');
      const ov = card.querySelector('.detect-svg');
      if (ov) { ov.style.display = ''; ov.innerHTML = ''; }
    }
    // 不主动清 srcObject：新流 onTrack 时替换，切换期间保留最后一帧不黑屏
    start(0);
    if (onFail) {
      const guard = { stream: next, from, onFail, timer: null };
      guard.timer = setTimeout(() => {
        if (switchGuard !== guard || curStream !== next || everConnected) return;
        switchGuard = null;
        guard.onFail(next, from, '6s 内未收到视频轨');
      }, 6000);
      switchGuard = guard;
    }
  }

  start(0);
  return { switchStream, current: () => curStream };
}

// ---------- SSE 实时状态 ----------
function pad2(n){return String(n).padStart(2,'0');}
function nowStr(){const d=new Date();return `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;}

function connectSSE() {
  const es = new EventSource('/api/events');
  const conn = document.getElementById('conn');
  es.onopen = () => { conn.textContent = '已连接'; conn.className = 'conn ok'; };
  es.onerror = () => { conn.textContent = '断开，重连中…'; conn.className = 'conn bad'; };
  es.onmessage = (e) => {
    let msg;
    try { msg = JSON.parse(e.data); } catch { return; }
    if (msg.type === 'telemetry') {
      window.__lastSnap = msg.data;
      if (conn) conn.textContent = '已连接 · 更新 ' + nowStr();
      updateTelemetry(msg.data);
    }
    else if (msg.type === 'button') updateButton(msg);
    else if (msg.type === 'lingsi') renderLingsi(msg.data);
    else if (msg.type === 'detect') handleDetect(msg);
    else if (msg.type === 'detect_clear') clearAllBoxes();
    else if (msg.type === 'detect_error') {
      showHint('detect-err', msg.msg || '识别异常', true);
      refreshDetectStatus();
    }
    else if (msg.type === 'detect_state') refreshDetectStatus();
    else if (msg.type === 'job_started') handleJobStarted(msg);
    else if (msg.type === 'job_progress') handleJobProgress(msg);
    else if (msg.type === 'job_done') handleJobDone(msg);
    else if (msg.type === 'job_cancelled') handleJobCancelled(msg);
    else if (msg.type === 'plan_hit') handlePlanHit(msg);
    else if (msg.type === 'plan_miss') handlePlanMiss(msg);
    else if (msg.type === 'job_error') showHint('job-err', msg.msg || '任务异常', true);
    else if (msg.type === 'error') console.warn('后端错误:', msg.msg);
  };
}

// ---------- 渲染各类状态 ----------
function kvFromObj(obj, keys) {
  return keys.map(k => {
    const v = obj[k];
    return `<div><span>${k}</span>: ${v === null || v === undefined ? '—' : v}</div>`;
  }).join('');
}

function updateTemp(t) {
  const el = document.getElementById('temp');
  if (t && t.raw) {
    el.innerHTML = `<div class="sht">${t.raw}</div>`;
  } else {
    el.innerHTML = `<div class="note">${t && t.error ? t.error : '无数据'}</div>`;
  }
}

function render4G(g) {
  if (!g || !g.lines || !g.lines.length) return '<div class="note">无数据</div>';
  const esc = s => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  return '<pre class="g4pre">' + g.lines.map(esc).join('\n') + '</pre>';
}

function updateTelemetry(d) {
  // 每个板块独立 try，避免单板块异常导致其余板块停留在初始 '-'
  try { updateTemp(d.temp || {}); }
  catch (e) { console.error('[telemetry] temp', e); }

  try { document.getElementById('g4').innerHTML = render4G(d.g4 || {}); }
  catch (e) { console.error('[telemetry] g4', e); }

  try { updateWifi(d.wifi || {}); }
  catch (e) { console.error('[telemetry] wifi', e); }

  try { updateEnv(d.env || {}); }
  catch (e) { console.error('[telemetry] env', e); }

  try { if (d.lingsi) renderLingsi(d.lingsi); }
  catch (e) { console.error('[telemetry] lingsi', e); }
}

// WiFi 折叠状态：默认仅显示前 5 条，点 "查看全部" 展开
let wifiExpanded = false;
const WIFI_COLLAPSED_LIMIT = 5;

function updateWifi(w) {
  window.__lastWifi = w;
  const networks = w.networks || [];
  const total = w.total || networks.length;
  document.getElementById('wifi-count').textContent = total;

  const visibleCount = wifiExpanded ? networks.length : Math.min(WIFI_COLLAPSED_LIMIT, networks.length);
  const rows = networks.slice(0, visibleCount).map(n =>
    `<div class="row"><span class="ssid">${n.ssid || '(隐藏)'}</span>` +
    `<span class="sig sig-${n.quality}">${n.signal ?? '?'} ${n.quality}</span>` +
    `<span class="sec">${n.security || ''}</span></div>`
  ).join('');

  let html = rows || '<div class="note">无</div>';
  // 多于 5 条时才显示折叠/展开入口
  if (networks.length > WIFI_COLLAPSED_LIMIT) {
    const label = wifiExpanded ? '收起' : `查看全部 (${networks.length})`;
    html += `<a class="wifi-toggle" id="wifi-toggle">${label}</a>`;
  }

  const wifiEl = document.getElementById('wifi');
  wifiEl.innerHTML = html;

  const toggle = document.getElementById('wifi-toggle');
  if (toggle) {
    toggle.onclick = () => {
      wifiExpanded = !wifiExpanded;
      updateWifi(window.__lastWifi || { networks: [], total: 0 });
    };
  }
}

function updateEnv(env) {
  const ifs = (env.interfaces || []).map(i => `${i.name}: ${i.ip}`).join(' | ');
  const ssd = env.ssd || {};
  const sys = env.system || {};
  document.getElementById('env').innerHTML = [
    ['网卡IP', ifs],
    ['核心板温度', env.core_temp_c != null ? env.core_temp_c + ' °C' : '—'],
    ['SSD容量', ssd.capacity_bytes ? (ssd.capacity_bytes / 1e9).toFixed(1) + ' GB' : '—'],
//    ['SSD温度', ssd.temperature_c != null ? ssd.temperature_c + ' °C' : '—'],
    ['系统', sys.uname],
    ['运行', sys.uptime],
  ].map(([k, v]) => `<div><span>${k}</span>: ${v ?? '—'}</div>`).join('');

  // SN（合规显示 SN，不合规显示 '-'）
  const sn = env.sn || {};
  const snEl = document.getElementById('sn');
  if (sn.sn) {
    snEl.innerHTML = `<div>SN: <b>${sn.sn}</b></div><div class="note">${sn.note || ''}</div>`;
    snEl.className = 'kv ok';
  } else {
    snEl.innerHTML = `<div>SN: <b>—</b></div><div class="note">${sn.note || '获取失败'}</div>`;
    snEl.className = 'kv bad';
  }
}

const ASR_STATE_LABEL = {
  'active': '运行中',
  'activating': '启动中',
  'inactive': '未运行',
  'failed': '启动失败',
  'unknown': '未知',
};

let broadcasting = false;   // 当前是否处于「播报中」（开始/停止共用一个按钮）

function renderLingsi(l) {
  const asrState = l.asr_state || (l.asr_running ? 'active' : 'inactive');
  const asrDisplay = ASR_STATE_LABEL[asrState] || asrState;
  const modeLabel = { voice: '语音交互', call: '播报/录音', phone: '模拟电话' }[l.mode] || (l.mode || '—');
  const rows = [
    ['ADB', l.adb_detected ? '已识别' : '未识别'],
    ['MIC', l.mic_detected ? '已识别' : '未识别'],
    ['ASR服务', asrDisplay],
    ['当前模式', modeLabel],
    ['联网', l.network_ok ? '是' : '否'],
  ];
  if (l.phone) rows.push(['通话对端', `${l.phone.ip}:${l.phone.port}`]);
  if (l.broadcasting) rows.push(['播报', '进行中']);
  document.getElementById('lingsi').innerHTML =
    rows.map(([k, v]) => `<div><span>${k}</span>: ${v ?? '—'}</div>`).join('');

  // 非 phone 模式且未联网 → 顶部提示（call/voice/播报 需背包联网）
  const stHint = document.getElementById('lingsi-status-hint');
  if (stHint) stHint.textContent = (l.net_required && !l.network_ok)
    ? '⚠ 当前未联网，call / voice / 播报 等功能需联网后使用' : '';

  // 同步播报按钮（开始/停止共用一个）
  broadcasting = !!l.broadcasting;
  const bb = document.getElementById('btn-call-broadcast');
  if (bb) bb.textContent = broadcasting ? '停止播报' : '开始播报';

  // phone 通话中徽标
  const badge = document.getElementById('phone-badge');
  if (badge) badge.textContent = (l.mode === 'phone') ? '📞 通话中' : '';
}

function updateButton(b) {
  if (b.error) { console.warn(b.error); return; }
  const el = document.getElementById(b.label.toLowerCase());
  if (!el) return;
  if (b.state === 'pressed') {
    el.className = 'ind on';
    el.textContent = `${b.label}：已触发`;
  } else if (b.state === 'released') {
    el.className = 'ind off';
    el.textContent = `${b.label}：未触发`;
  }
}

// ---------- 控制按钮 ----------
async function postJson(url, body) {
  try {
    const r = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body || {}),
    });
    return await r.json();
  } catch (e) { return { ok: false, error: String(e) }; }
}

document.getElementById('btn-led').onclick = async () => {
  const b = document.getElementById('btn-led');
  const old = b.textContent;
  b.disabled = true; b.textContent = '闪烁中…';
  await postJson('/api/led/blink');
  setTimeout(() => { b.textContent = old; b.disabled = false; }, 1500);
};
document.getElementById('btn-fill-on').onclick = async () => {
  const r = await postJson('/api/filllight', { state: true });
  document.getElementById('fill-state').textContent = r.ok && r.state === 'on' ? '开启' : '—';
};
document.getElementById('btn-fill-off').onclick = async () => {
  const r = await postJson('/api/filllight', { state: false });
  document.getElementById('fill-state').textContent = r.ok && r.state === 'off' ? '关闭' : '—';
  refreshLights();
};

// 计划灯光（航点动作 + 计划「仅夜间自动开灯」）状态。
// 现场经常要回答「灯为什么亮着」：这里同时显示是谁点的灯、亮多久、当前算不算夜间。
async function refreshLights() {
  const el = document.getElementById('light-plan');
  if (!el) return;
  let s;
  try {
    const r = await fetch('/api/lights', { cache: 'no-store' });
    s = await r.json();
  } catch (e) { return; }
  const window_ = s.night_window || {};
  const parts = [];
  parts.push(`补光 ${s.fill ? '开' : '关'}`);
  parts.push(`LED ${s.led ? '亮' : '关'}`);
  if (s.on_secs != null) parts.push(`已亮 ${Math.round(Number(s.on_secs))}s`);
  parts.push(`${s.night ? '夜间' : '白天'} ${window_.enabled === false ? '（未启用夜间窗）' : `${window_.start || '—'}-${window_.end || '—'}`}`);
  el.textContent = parts.join(' · ');
  el.title = s.source ? `来源：${s.source}` : '';
}
// 旧的 "执行测试" 按钮已拆为 "启动服务"/"关闭服务" 两个独立按钮
async function refreshLingsi() {
  try {
    const r = await fetch('/api/lingsi');
    renderLingsi(await r.json());
  } catch (e) {}
}

// 按钮点击微反馈：短暂闪绿 500ms，文字不变 —— 让用户能确认点击已生效，
// 但又不像旧版那样改文字+disabled，符合"按下后无需变化"的要求
function flashOk(btn) {
  if (!btn) return;
  const old = btn.style.background;
  btn.style.background = 'var(--ok)';
  setTimeout(() => { btn.style.background = old; }, 500);
}

document.getElementById('btn-asr-start').onclick = async (ev) => {
  flashOk(ev.currentTarget);
  await postJson('/api/asr/start');
  // 1.5s 后自动刷新一次面板，让用户尽快看到 asr 状态变化
  setTimeout(refreshLingsi, 1500);
};
document.getElementById('btn-asr-stop').onclick = async (ev) => {
  flashOk(ev.currentTarget);
  await postJson('/api/asr/stop');
  setTimeout(refreshLingsi, 1500);
};
document.getElementById('btn-mode-call').onclick = async (ev) => {
  flashOk(ev.currentTarget);
  await postJson('/api/lingsi/mode', { mode: 'call' });
  setTimeout(refreshLingsi, 500);
};
document.getElementById('btn-mode-voice').onclick = async (ev) => {
  flashOk(ev.currentTarget);
  await postJson('/api/lingsi/mode', { mode: 'voice' });
  setTimeout(refreshLingsi, 500);
};
document.getElementById('btn-vol').onclick = async (ev) => {
  flashOk(ev.currentTarget);
  const v = document.getElementById('vol').value;
  await postJson('/api/lingsi/volume', { level: v });
  setTimeout(refreshLingsi, 500);
};

// ---------- 模拟电话 phone ----------
// 板端命令：phone <PC端IP> <端口> 发起通话；phone stop 挂断并回到 voice。
// PC 默认 IP 192.168.234.1（网页已预填，可按需修改）。
document.getElementById('btn-phone-start').onclick = async (ev) => {
  flashOk(ev.currentTarget);
  const ip = (document.getElementById('phone-ip').value || '').trim();
  const port = (document.getElementById('phone-port').value || '').trim() || '5000';
  if (!/^\d{1,3}(\.\d{1,3}){3}$/.test(ip)) {
    showHint('phone-err', 'PC 端 IP 格式不正确（示例 192.168.234.1）', true);
    return;
  }
  showHint('phone-err', '', false);
  await postJson('/api/lingsi/mode', { cmd: `phone ${ip} ${port}` });
  setTimeout(refreshLingsi, 500);
};
document.getElementById('btn-phone-stop').onclick = async (ev) => {
  flashOk(ev.currentTarget);
  showHint('phone-err', '', false);
  await postJson('/api/lingsi/mode', { cmd: 'phone stop' });
  setTimeout(refreshLingsi, 500);
};

// ---------- 播报内容 call（带内容，单次/循环） ----------
// 板端命令：call 0 <内容> 单次；call 1 <内容> 循环；call stop 停止并回到 voice。
// 内容限制：非空、无空格、仅中文汉字+中文标点、中文≤30 字。
function validateBroadcast(text) {
  const t = (text || '').trim();
  if (!t) return '播报内容不能为空';
  if (/\s/.test(t)) return '不能包含空格';
  if (!/^[\u4e00-\u9fff，。！？、；：（）《》…—·0-9]+$/.test(t))
    return '仅支持中文汉字及中文标点，不能有英文/特殊符号';
  const cn = (t.match(/[\u4e00-\u9fff]/g) || []).length;
  if (cn > 30) return '中文汉字不能超过 30 个';
  if (t.length > 40) return '总长度过长';
  return '';
}
function showHint(id, msg, bad) {
  const el = document.getElementById(id);
  if (!el) return;
  el.textContent = msg || '';
  el.classList.toggle('bad', !!bad && !!msg);
}
document.getElementById('btn-call-broadcast').onclick = async (ev) => {
  const btn = ev.currentTarget;
  if (broadcasting) {
    flashOk(btn);
    await postJson('/api/lingsi/mode', { cmd: 'call stop' });
    broadcasting = false;
    btn.textContent = '开始播报';
    setTimeout(refreshLingsi, 500);
    return;
  }
  const text = document.getElementById('call-text').value;
  const loop = document.getElementById('call-loop').value;  // '0' 单次 / '1' 循环
  const err = validateBroadcast(text);
  if (err) { showHint('call-hint', err, true); return; }
  showHint('call-hint', '', false);
  flashOk(btn);
  await postJson('/api/lingsi/mode', { cmd: `call ${loop} ${text}` });
  broadcasting = true;
  btn.textContent = '停止播报';
  setTimeout(refreshLingsi, 500);
};

// ---------- 面板刷新按钮（统一封装：圆形旋转图标 + 点击拉取 + 应用结果） ----------
function bindRefresh(btnId, url, apply) {
  document.getElementById(btnId).onclick = async (ev) => {
    const btn = ev.currentTarget;
    btn.classList.add('spin');
    try {
      const r = await fetch(url);
      apply(await r.json());
    } catch (e) {
      console.warn(`[refresh] ${url} 失败`, e);
    }
    setTimeout(() => btn.classList.remove('spin'), 600);
  };
}

// SN 独立刷新：只更新 SN 面板（不连带环境信息）
function updateSN(sn) {
  const snEl = document.getElementById('sn');
  if (sn && sn.sn) {
    snEl.innerHTML = `<div>SN: <b>${sn.sn}</b></div><div class="note">${sn.note || ''}</div>`;
    snEl.className = 'kv ok';
  } else {
    snEl.innerHTML = `<div>SN: <b>—</b></div><div class="note">${(sn && sn.note) || '获取失败'}</div>`;
    snEl.className = 'kv bad';
  }
}

bindRefresh('btn-temp', '/api/temp', updateTemp);
bindRefresh('btn-g4',   '/api/4g',   (d) => { document.getElementById('g4').innerHTML = render4G(d); });
bindRefresh('btn-wifi', '/api/wifi', updateWifi);
bindRefresh('btn-env',  '/api/env',  updateEnv);
bindRefresh('btn-sn',   '/api/sn',   updateSN);
bindRefresh('btn-lingsi-refresh', '/api/asr', (d) => {
  // 直接刷新 ASR 服务状态（asr 默认可能未启动，独立 GET /api/asr）
  const label = ASR_STATE_LABEL[d.state] || d.state || '未知';
  document.getElementById('lingsi').innerHTML = [
    ['ASR服务', label],
  ].map(([k, v]) => `<div><span>${k}</span>: ${v ?? '—'}</div>`).join('');
  // 紧接着再拉一次完整状态补全其他字段
  setTimeout(refreshLingsi, 200);
});

// ---------- 调试：双击副标题查看最近一次原始遥测 JSON ----------
function initDebug() {
  const sub = document.getElementById('subtitle');
  if (!sub) return;
  sub.style.cursor = 'pointer';
  sub.addEventListener('dblclick', () => {
    const dbg = document.getElementById('dbg');
    if (!dbg) return;
    if (dbg.hidden) {
      dbg.textContent = JSON.stringify(window.__lastSnap || {}, null, 2);
      dbg.hidden = false;
    } else {
      dbg.hidden = true;
    }
  });
}

// ---------- AI 识别：场景清单 + 识别控制 + 视频框叠加 ----------
function escHtml(s) {
  return String(s ?? '').replace(/[&<>"']/g, c => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'
  }[c]));
}

let detectTasks = [];
let detectRunning = false;
let detectState = null;             // 最近一次 /api/detect/status 快照
let jobBusy = false;                // 任务下发进行中（防重复提交）
let lastDetectLabelTouch = 0;
let lastDetectCamTouch = 0;
const boxAnim = {};                 // cam -> 两次检测结果之间的插值动画状态
const boxShown = {};                // cam -> 当前画面上的框（插值中间态）
const DETECT_FPS_MAX = 12;          // 与 config.py 的 DETECT_FPS_MAX 保持一致

// ---- 版本 2：背包本地画框 ----
// osdActive=true 时该路画面直接播带框的 preview 流，浏览器不再叠 SVG；
// 板端画框没起来（或 DETECT_OSD_ENABLED=0）时自动回退浏览器叠加。
let osdActive = false;
let aiCam = null;                   // 当前 AI 目标路（原始路名）
let aiPreviewStream = 'preview';
let osdFailedCam = null;            // 该路切 preview 失败过，本次 OSD 会话不再重试
let lastOsdSession = null;          // 后端 osd.start() 的代号，变化=允许重新试探
let lastOsdReady = false;

function applyAiSource(wantOsd, cam, previewStream) {
  aiPreviewStream = previewStream || 'preview';
  const target = (wantOsd && cam && osdFailedCam !== cam) ? cam : null;
  if (target === aiCam && !!target === osdActive) return;   // 每 3s 轮询，状态没变就别动
  // 旧目标路切回它自己的干净流
  if (aiCam && aiCam !== target) {
    const oldCard = cardOf(aiCam);
    if (oldCard && oldCard._whep) {
      oldCard._whep.switchStream(oldCard._baseStream || aiCam);
    }
  }
  // 新目标路切到板端带框预览流；失败时立刻切回，绝不把正常画面留在黑屏上
  if (target) {
    const card = cardOf(target);
    if (card && card._whep) {
      card._whep.switchStream(aiPreviewStream, (failedStream) => {
        osdFailedCam = target;
        applyAiSource(false, null, aiPreviewStream);   // 切回干净流 + 恢复浏览器叠框
        showHint('detect-err',
          '板端画框预览流 ' + failedStream + ' 未出图，已切回原始画面并回退浏览器叠加；'
          + '请确认 mediamtx 已重启并登记 ' + failedStream + ' path', true);
      });
    } else {
      // 卡片不存在（例如被隐藏）：不切流，直接回退浏览器叠加
      osdFailedCam = target;
    }
  }
  clearAllBoxes();
  aiCam = target;
  osdActive = !!target;
  document.body.classList.toggle('osd-on', osdActive);
}

function renderDetect(d) {
  const box = document.getElementById('detect');
  if (!box) return;
  const tasks = d.tasks || [];
  const cnt = document.getElementById('detect-count');
  const note = document.getElementById('detect-note');
  if (cnt) cnt.textContent = String(tasks.length);
  if (note) note.textContent = d.note || '';
  if (d.error) {
    box.innerHTML = `<div class="detect-issue">${escHtml(d.error)}</div>`;
    return;
  }
  if (!tasks.length) {
    box.innerHTML = '<div class="detect-empty">尚未接入模型包（放入 rknn/models/&lt;task&gt;/ 后自动出现）</div>';
    return;
  }
  box.innerHTML = tasks.map(t => {
    const labels = t.labels || [];
    const shown = labels.filter(l => !l.ignored);
    // 多类别模型（如 combined 29 类）只列前 10 个，避免场景卡片被撑爆；
    // 完整类别始终在下方勾选区，可全选/取消。
    const chips = shown.slice(0, 10).map(l => {
      const color = /^#[0-9a-fA-F]{6}$/.test(l.color || '') ? l.color : null;
      const st = color ? ` style="border-color:${color}"` : '';
      return `<span class="detect-chip"${st}>${escHtml(l.zh || l.name)}</span>`;
    }).join('');
    const more = shown.length > 10
      ? `<span class="detect-muted">等 ${shown.length} 类（下方勾选区可全选/取消）</span>` : '';
    const ignored = labels.filter(l => l.ignored).length;
    const issues = (t.issues || []).map(i => `<div class="detect-issue">${escHtml(i)}</div>`).join('');
    return `<div class="detect-task">
      <div class="detect-hd">
        <span class="detect-name">${escHtml(t.name)}</span>
        <span class="detect-meta">${escHtml(t.task)} · ${Number(t.nc) || 0} 类</span>
        <span class="detect-badge ${t.ok ? 'ok' : 'warn'}">${t.ok ? '可用' : '待修正'}</span>
      </div>
      ${t.desc ? `<div class="detect-desc">${escHtml(t.desc)}</div>` : ''}
      <div class="detect-chips">${chips || '<span class="detect-empty">无可用类别</span>'}${more}${ignored ? `<span class="detect-muted">已忽略 ${ignored} 类</span>` : ''}</div>
      ${issues}
    </div>`;
  }).join('');

  detectTasks = tasks;
  const sel = document.getElementById('detect-task');
  if (sel) {
    const prev = sel.value;
    // 手动刷新场景清单时保留用户已勾选的类别，避免被重置成全选
    const kept = new Set(Array.from(document.querySelectorAll('#detect-labels input:checked')).map(i => i.value));
    const usable = tasks.filter(t => t.ok);
    sel.innerHTML = usable.length
      ? usable.map(t => `<option value="${escHtml(t.task)}">${escHtml(t.name)}（${escHtml(t.task)}）</option>`).join('')
      : '<option value="">无可用场景</option>';
    if (prev && Array.from(sel.options).some(o => o.value === prev)) sel.value = prev;
    applyTaskDefaults(sel.value !== prev ? null : kept);
  }
}

function currentTaskInfo() {
  const sel = document.getElementById('detect-task');
  return detectTasks.find(t => t.task === (sel && sel.value)) || null;
}

function applyTaskDefaults(keep) {
  const t = currentTaskInfo();
  const confEl = document.getElementById('detect-conf');
  if (t && confEl && !confEl.dataset.touched) confEl.value = String(t.conf ?? 0.25);
  renderDetectLabels(keep);
}

function renderDetectLabels(keep) {
  const box = document.getElementById('detect-labels');
  if (!box) return;
  const t = currentTaskInfo();
  const labels = ((t && t.labels) || []).filter(l => !l.ignored);
  box.innerHTML = labels.length
    ? labels.map(l => {
        const on = keep ? keep.has(l.name) : true;
        return `<label class="detect-check"><input type="checkbox" value="${escHtml(l.name)}"${on ? ' checked' : ''} />${escHtml(l.zh || l.name)}</label>`;
      }).join('')
    : '<span class="detect-muted">该场景暂无可用类别</span>';
}

function renderDetectCams() {
  const box = document.getElementById('detect-cams');
  if (!box) return;
  // 版本 3：五路可多选，默认「机器狗前置」（狗前 RTSP / dogcam）。
  // NPU 单模型串行，多路在后端分时轮询：每路实际帧率 ≈ 总帧率 ÷ 路数。
  // 红外 dog-ipcam 不参与 AI，故不在此列。
  const rows = [['dogcam', '机器狗前置'], ['cam0', '左前'],
                ['cam1', '右前'], ['cam2', '右后'], ['cam3', '左后']];
  const single = !!(detectState && detectState.multi_cam === false);
  const type = single ? 'radio' : 'checkbox';
  box.innerHTML = rows
    .map(([v, n], i) => `<label class="detect-check"><input type="${type}" name="detect-cam" value="${v}"${i === 0 ? ' checked' : ''} />${n}</label>`)
    .join('');
}

// 收集当前勾选的识别路（供常驻识别与任务下发共用）
function selectedDetectCams() {
  return Array.from(document.querySelectorAll('#detect-cams input[name="detect-cam"]:checked'))
    .map(el => el.value);
}

const CAM_LABELS = {dogcam: '机器狗前置', cam0: '左前', cam1: '右前', cam2: '右后', cam3: '左后'};
function camName(c) { return CAM_LABELS[c] || c; }

// ---------- 前置摄像头分辨率档位切换 ----------
let dogcamProfile = '1080p60';
function applyDogcamProfileUi(profile) {
  dogcamProfile = profile;
  document.querySelectorAll('#dogcam-profile button').forEach(b => {
    b.classList.toggle('active', b.getAttribute('data-profile') === profile);
  });
  const hint = document.getElementById('dogcam-profile-hint');
  if (hint) hint.textContent = profile === '1080p60'
    ? '当前：mediamtx 直拉源 1920×1080@60，不经 relay 重编码'
    : '当前：relay 重编码 1280×720@30（dogcam720）';
}
function bindDogcamProfile() {
  document.querySelectorAll('#dogcam-profile button').forEach(btn => {
    btn.onclick = async () => {
      const profile = btn.getAttribute('data-profile');
      if (profile === dogcamProfile) return;
      const hint = document.getElementById('dogcam-profile-hint');
      if (hint) hint.textContent = '切换中…前置画面短暂中断';
      let r;
      try {
        r = await postJson('/api/dogcam/profile', { profile });
      } catch (e) {
        if (hint) hint.textContent = '切换失败：' + (e.message || e);
        return;
      }
      if (!r.ok) {
        if (hint) hint.textContent = '切换失败：' + (r.error || '未知错误');
        return;
      }
      applyDogcamProfileUi(r.profile);
      // 同步这张卡的「干净流」名：AI 停止后切回时用新档位的 stream
      const card = cardOf('dogcam');
      if (card) {
        card._baseStream = r.stream || (r.profile === '1080p60' ? 'dogcam' : 'dogcam720');
        // 未处于 AI 预览时，立即把这张卡的 WHEP 切到新 stream
        if (!osdActive && card._whep && card._whep.switchStream) {
          card._whep.switchStream(card._baseStream);
        }
      }
      if (hint) hint.textContent = (r.profile === '1080p60'
        ? '已切换：1080P60 直拉' : '已切换：720P30 重编码') + '，画面恢复中…';
    };
  });
}

async function refreshDetect() {
  const box = document.getElementById('detect');
  if (!box) return;
  try {
    const r = await fetch('/api/detect/tasks', { cache: 'no-store' });
    renderDetect(await r.json());
  } catch (e) {
    box.innerHTML = `<div class="detect-issue">场景清单获取失败：${escHtml(e.message || e)}</div>`;
  }
}

// ---------- SVG 叠加层 ----------
function initDetectOverlays() {
  document.querySelectorAll('.video-card').forEach(card => {
    if (card.querySelector('.detect-svg')) return;
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    svg.setAttribute('class', 'detect-svg');
    svg.setAttribute('preserveAspectRatio', 'none');
    card.appendChild(svg);
  });
}

function cardOf(cam) {
  if (!/^[A-Za-z0-9_-]+$/.test(cam || '')) return null;
  return document.querySelector('.video-card[data-stream="' + cam + '"]');
}

function textWidth(s) {
  let w = 0;
  for (const ch of String(s)) w += /[\u4e00-\u9fff]/.test(ch) ? 11 : 6.5;
  return w;
}

// 后端每路每秒只给若干次结果，直接重建 SVG 会让框「一格一格跳」。
// 在两次结果之间用 requestAnimationFrame 线性插值，把离散框补成连续运动；
// 插值结束、画面静止后不再重绘，不额外吃 CPU。
function buildBoxSvg(boxes) {
  let html = '';
  boxes.forEach(b => {
    const w = Math.max(1, b.x2 - b.x1), h = Math.max(1, b.y2 - b.y1);
    const ty = Math.max(0, b.y1 - 18);
    html += `<rect x="${b.x1.toFixed(1)}" y="${b.y1.toFixed(1)}" width="${w.toFixed(1)}" height="${h.toFixed(1)}" fill="none" stroke="${b.color}" stroke-width="2" />`
          + `<rect x="${b.x1.toFixed(1)}" y="${ty.toFixed(1)}" width="${(b.tw || 40).toFixed(1)}" height="17" rx="3" fill="${b.color}" />`
          + `<text x="${(b.x1 + 5).toFixed(1)}" y="${(ty + 12.5).toFixed(1)}" font-size="11" fill="#fff" font-family="sans-serif">${escHtml(b.label)}</text>`;
  });
  return html;
}

// ---------- 版本 3：任务事件 ----------
function handleJobStarted(msg) {
  jobBusy = true;
  showHint('job-err', '', false);
  const box = document.getElementById('job-result');
  if (box) box.innerHTML = `<div class="detect-muted">任务 ${escHtml(msg.job_id)} 已下发，识别中…`
    + `（${escHtml(checkTypeName(msg.check_type))}，${(msg.cams || []).map(camName).join('、')}，${msg.duration}s`
    + `${msg.expect_count ? `，检查 ${escHtml(String(msg.expect_count))} 个` : ''}）</div>`;
  const je = document.getElementById('job-status');
  if (je) je.innerHTML = `<b>任务 ${escHtml(msg.job_id)}</b> · ${escHtml(checkTypeName(msg.check_type))} · 识别中`;
  refreshDetectStatus();
}
function checkTypeName(kind) {
  return kind === 'count' ? '检测数量是否正确' : '检测是否存在';
}

function handleJobProgress(msg) {
  const je = document.getElementById('job-status');
  if (je) je.innerHTML = `<b>任务 ${escHtml(msg.job_id)}</b> · ${camName(msg.cam)} 命中 ${msg.streak}/${msg.need}（本帧 ${msg.boxes} 框）`;
}
// 版本 4：航点计划命中（带计划 id 的识别）。证据图 + result.json 已落到
// detect_jobs/<计划id>/pt<航点下标>/，这里把数量核对结论也显示出来。
function handlePlanHit(msg) {
  const box = document.getElementById('job-result');
  const je = document.getElementById('job-status');
  if (je) je.innerHTML = `<b class="ok">航点命中</b> · 计划 ${escHtml(msg.plan_id || '—')}`
    + `${msg.point_index === null || msg.point_index === undefined ? '' : ' 航点 ' + escHtml(String(msg.point_index))}`
    + ` · ${escHtml(checkTypeName(msg.check_type))}`
    + ` · ${camName(msg.found_cam)}（${escHtml(String(msg.found_count || 0))} 个目标）`;
  if (!box) return;
  const ec = msg.expect_count;
  const countLine = (ec === null || ec === undefined)
    ? '<span class="detect-muted">数量核对：未判断（存在性任务 / 未填检查数量）</span>'
    : (msg.count_ok
      ? `<span class="ok">数量核对：一致（检查 ${escHtml(String(ec))} / 检出 ${escHtml(String(msg.found_count))}）</span>`
      : `<span class="bad">数量核对：不一致（检查 ${escHtml(String(ec))} / 检出 ${escHtml(String(msg.found_count))}）→ 已生成数量告警</span>`);
  box.innerHTML = `<div class="job-hit">✅ 计划 <b>${escHtml(msg.plan_id || '—')}</b> 发现目标<br>`
    + `<span class="detect-muted">存证目录：${escHtml(msg.dir || '')}</span><br>`
    + `<span class="detect-muted">证据图：${escHtml(msg.evidence || '')}</span><br>`
    + `<span class="detect-muted">描述：${escHtml(msg.result_json || '')}</span><br>`
    + countLine + '</div>';
  refreshDetectStatus();
}
function handleJobDone(msg) {
  jobBusy = false;
  const je = document.getElementById('job-status');
  const box = document.getElementById('job-result');
  if (msg.found) {
    const name = msg.cam_title || camName(msg.cam);
    if (je) je.innerHTML = `<b class="ok">任务完成</b> · ${escHtml(checkTypeName(msg.check_type))} · ${name} 发现目标，已自动结束（${Number(msg.elapsed_s).toFixed(1)}s）`;
    const ec = msg.expect_count;
    const countLine = (ec === null || ec === undefined)
      ? '<span class="detect-muted">数量核对：未判断（存在性任务 / 未填检查数量）</span>'
      : (msg.count_ok
        ? `<span class="ok">数量核对：一致（检查 ${escHtml(String(ec))} / 检出 ${escHtml(String(msg.found_count))}）</span>`
        : `<span class="bad">数量核对：不一致（检查 ${escHtml(String(ec))} / 检出 ${escHtml(String(msg.found_count))}）→ 已生成数量告警</span>`);
    if (box) box.innerHTML = `<div class="job-hit">✅ <b>${escHtml(name)}</b> 发现目标，推理自动结束<br>`
      + `<span class="detect-muted">证据图：${escHtml(msg.evidence || '')}</span><br>`
      + `<span class="detect-muted">描述：${escHtml(msg.result_json || '')}</span><br>`
      + countLine + '</div>';
  } else {
    if (je) je.innerHTML = `<b class="warn">任务结束</b> · ${msg.error ? '异常：' + escHtml(msg.error) : '识别时间内未找到目标'}`;
    if (box) box.innerHTML = `<div class="job-miss">⏱ 识别时间内未找到目标，推理自动结束${msg.error ? '<br>' + escHtml(msg.error) : ''}</div>`;
  }
  refreshDetectStatus();
}
function handleJobCancelled(msg) {
  jobBusy = false;
  const je = document.getElementById('job-status');
  if (je) je.innerHTML = `任务 ${escHtml(msg.job_id || '')} 已取消`;
  refreshDetectStatus();
}

// 窗口结束仍未命中：存在性任务是「没找到」，数量任务是「数量不一致（0 ≠ N）」
function handlePlanMiss(msg) {
  const box = document.getElementById('job-result');
  const je = document.getElementById('job-status');
  const countMode = msg.check_type === 'count' && msg.expect_count !== null && msg.expect_count !== undefined;
  if (je) {
    je.innerHTML = countMode
      ? `<b class="bad">数量不一致</b> · 计划 ${escHtml(msg.plan_id || '—')}（检查 ${escHtml(String(msg.expect_count))} / 检出 0）`
      : `<b class="warn">未找到目标</b> · 计划 ${escHtml(msg.plan_id || '—')}`;
  }
  if (!box) return;
  box.innerHTML = `<div class="${countMode ? 'job-hit' : 'job-miss'}">`
    + (countMode
      ? `⚠ 计划 <b>${escHtml(msg.plan_id || '—')}</b> 数量核对不一致（检查 ${escHtml(String(msg.expect_count))} / 检出 0）→ 已生成数量告警<br>`
      : `⏱ 计划 <b>${escHtml(msg.plan_id || '—')}</b> 窗口内未找到目标<br>`)
    + `<span class="detect-muted">存证目录：${escHtml(msg.dir || '')}</span><br>`
    + `<span class="detect-muted">描述：${escHtml(msg.result_json || '')}</span></div>`;
  refreshDetectStatus();
}

function handleDetect(msg) {
  // 板端已把框烧进视频，浏览器不再叠加；osd 未就绪时 applyAiSource 会
  // 把 osdActive 置回 false，这里自然恢复浏览器叠加（自动回退）。
  if (osdActive) return;
  const card = cardOf(msg.cam);
  if (!card || card.classList.contains('no-overlay')) return;
  const svg = card.querySelector('.detect-svg');
  const video = card.querySelector('video');
  if (!svg || !video || video.style.display === 'none') return;
  const W = video.clientWidth || card.clientWidth;
  const H = video.clientHeight || card.clientHeight;
  if (!W || !H) return;

  // 后端给的框是「抽帧原图」像素坐标；video 用 object-fit: cover 会裁切，
  // 必须按实际显示区域反算，否则框整体偏移。
  const fw = Number(msg.frame_w) || 1280;
  const fh = Number(msg.frame_h) || 720;
  const scale = Math.max(W / fw, H / fh);
  const ox = (W - fw * scale) / 2;
  const oy = (H - fh * scale) / 2;

  const to = [];
  (msg.boxes || []).forEach(b => {
    const x1 = Number(b.x1) * scale + ox, y1 = Number(b.y1) * scale + oy;
    const x2 = Number(b.x2) * scale + ox, y2 = Number(b.y2) * scale + oy;
    if (![x1, y1, x2, y2].every(Number.isFinite)) return;
    const color = /^#[0-9a-fA-F]{6}$/.test(b.color || '') ? b.color : '#E24B4A';
    // 框上文字由后端统一生成（中文名 + 当前帧同类数量，例「火焰 3处」），
    // 与板端 OSD 烧进视频的完全一致；老版本后端没有 text 时才退回旧写法。
    const label = b.text || `${b.zh || b.label} ${Number(b.conf).toFixed(2)}`;
    to.push({ x1, y1, x2, y2, color, label, tw: textWidth(label) + 10 });
  });

  const now = performance.now();
  const cam = msg.cam;
  const prev = boxAnim[cam];
  const gap = prev ? now - prev.at : 0;
  // 插值时长覆盖两次结果的整个间隔：四路分时只有 2~4fps（gap≈250~500ms），
  // 若固定 220ms，框动一下就冻住几百毫秒，看起来就是「卡」。
  // 让框在整个间隔里持续平滑移动，观感接近连续跟踪（零 NPU 成本）。
  const dur = Math.max(60, Math.min(900, gap ? gap * 0.9 : 150));
  // 保持时间随间隔延长，推理变慢也不会一闪一闪。
  const hold = Math.max(400, Math.min(2500, (gap || 167) * 2.5));
  const shown = boxShown[cam] || [];
  boxAnim[cam] = {
    from: to.map((t, i) => shown[i] || t), to,
    t0: now, dur, at: now, hold, W, H, final: false,
  };
  svg.setAttribute('viewBox', `0 0 ${W} ${H}`);
}

const lerp = (a, b, p) => a + (b - a) * p;

// 单一 rAF 循环：只在插值进行中或刚收到新结果时重绘
function renderBoxes() {
  const now = performance.now();
  Object.keys(boxAnim).forEach(cam => {
    const a = boxAnim[cam];
    const card = cardOf(cam);
    const svg = card && card.querySelector('.detect-svg');
    if (!svg) return;
    if (card.classList.contains('no-overlay') || now - a.at > a.hold) {
      svg.innerHTML = '';
      delete boxAnim[cam]; delete boxShown[cam];
      return;
    }
    const p = Math.min(1, (now - a.t0) / a.dur);
    // 必须画一次 p=1 的终点帧再停止重绘，否则框会卡在倒数第二帧
    if (p >= 1 && a.final) return;
    const cur = a.to.map((t, i) => {
      const f = a.from[i] || t;
      return {
        x1: lerp(f.x1, t.x1, p), y1: lerp(f.y1, t.y1, p),
        x2: lerp(f.x2, t.x2, p), y2: lerp(f.y2, t.y2, p),
        color: t.color, label: t.label, tw: t.tw,
      };
    });
    boxShown[cam] = cur;
    svg.innerHTML = buildBoxSvg(cur);
    a.final = (p >= 1);
  });
  requestAnimationFrame(renderBoxes);
}

function clearAllBoxes() {
  document.querySelectorAll('.video-card .detect-svg').forEach(s => { s.innerHTML = ''; });
  Object.keys(boxAnim).forEach(k => delete boxAnim[k]);
  Object.keys(boxShown).forEach(k => delete boxShown[k]);
}
requestAnimationFrame(renderBoxes);

// ---------- 识别控制 ----------
function checkedValues(boxId) {
  return Array.from(document.querySelectorAll(`#${boxId} input:checked`)).map(i => i.value);
}

function collectDetectConfig() {
  const sel = document.getElementById('detect-task');
  const allLabels = Array.from(document.querySelectorAll('#detect-labels input')).map(i => i.value);
  const picked = checkedValues('detect-labels');
  return {
    task: sel ? sel.value : '',
    // 全选时传 null，表示「跟随该场景的全部有效类别」
    labels: (allLabels.length && picked.length === allLabels.length) ? null : picked,
    conf: parseFloat(document.getElementById('detect-conf').value) || 0.25,
    cams: checkedValues('detect-cams'),
    fps: Math.min(DETECT_FPS_MAX, Math.max(1,
      parseInt(document.getElementById('detect-fps').value, 10) || 6)),
  };
}

async function refreshDetectStatus() {
  const el = document.getElementById('detect-status');
  if (!el) return;
  let s;
  try {
    const r = await fetch('/api/detect/status', { cache: 'no-store' });
    s = await r.json();
  } catch (e) { return; }
  detectState = s;
  detectRunning = !!s.running;
  document.body.classList.toggle('detect-on', detectRunning);
  // 任务状态快照（有 job 时显示进行中）
  if (s.job) {
    const j = s.job;
    const hits = Object.entries(j.hits || {}).map(([c, n]) => `${camName(c)} ${n}/${j.streak}`).join('，') || '—';
    const je = document.getElementById('job-status');
    if (je) je.innerHTML = `<b>任务 ${escHtml(j.id)}</b> · ${escHtml(j.task || '')} · ${Number(j.elapsed_s).toFixed(1)}/${j.duration}s · 命中 ${escHtml(hits)}`;
  }
  const osd = s.osd || {};
  // 后端每次 osd.start()（开始识别 / 换目标路）都会换一个 session；
  // OSD 从「没出帧」变成「出帧」也算一次新机会。除此之外一次失败后不再
  // 每 3s 反复切流抖动。
  const ready = !!osd.ready;
  if (osd.session !== lastOsdSession || (ready && !lastOsdReady)) {
    lastOsdSession = osd.session;
    osdFailedCam = null;
  }
  lastOsdReady = ready;
  // 只认「真正在出帧」的 OSD：光有 ffmpeg 进程不代表 preview 有 publisher
  applyAiSource(!!s.running && ready,
                (s.cams || [])[0] || null, s.preview_stream);
  const lines = [];
  if (!s.bin_exists) {
    lines.push(`<span class="detect-warn">未编译 yolov8_detector · 板端执行：cd rknn/code/code &amp;&amp; cmake -S . -B build &amp;&amp; cmake --build build -j</span>`);
    lines.push(`<span class="detect-muted">当前查找路径：${escHtml(s.bin || '')}</span>`);
  }
  if (s.error) lines.push(`<span class="detect-warn">${escHtml(s.error)}</span>`);
  // 狗前置链路：即使没开 AI 也显示，否则「机器狗前置卡片凭空不出图」无从排查
  const rel = s.relay || state.relay || {};
  if (rel.enabled === false) {
    lines.push('<span class="detect-warn">机器狗前置未配置 RTSP 源（relay.enabled=false）：机器狗前置卡片不出图，AI 目标已临时改为 cam0；板端设置 DOG_RTSP_URL 或创建 backend/dog_rtsp_url 后重启 agent 即恢复默认</span>');
  } else if (rel.enabled === true && rel.running === false) {
    lines.push(`<span class="detect-warn">机器狗前置源已配置但 relay 未运行（重启 ${Number(rel.restarts) || 0} 次）：检查 3588 是否在推流、网线、源 URL 是否可达</span>`);
  }
  const degraded = document.querySelectorAll('.video-card.no-overlay').length;
  if (s.running && degraded && !osdActive) {
    lines.push(`<span class="detect-warn">${degraded} 路走了内置播放页，暂不能叠加框（原因见画面左下角提示，直连恢复后自动生效）</span>`);
  }
  if (s.running) {
    const aiTarget = (s.cams || [])[0] || '—';
    lines.push(`<b>识别中</b> · ${escHtml(s.task)} · 阈值 ${Number(s.conf).toFixed(2)} · 目标 ${escHtml(aiTarget)} · ${s.fps} 帧/秒`);
    const lf = s.label_format || {};
    lines.push(`<span class="detect-muted">框上文字：${lf.lang === 'zh' ? '中文' : '英文'}类名${lf.show_count ? ' + 同类数量' : ''}${lf.show_conf ? ' + 置信度' : ''}（例 ${escHtml(lf.example || '火焰 3处')}；同类数量按当前帧统计）</span>`);
    const miss = osd.missing_glyphs || [];
    if (miss.length) {
      lines.push(`<span class="detect-warn">中文字库缺字：${escHtml(miss.join(' '))}，画面显示为空心方框；请在开发机重跑 backend/tools/gen_cjk_font.py 并同步 cjk_font.py</span>`);
    }
    if (osd.font === 'ascii5x7') {
      lines.push('<span class="detect-warn">板端缺少 cjk_font.py，已降级英文字库（汉字显示为 ?）；请同步 backend/cjk_font.py</span>');
    }
    if (osdActive && osd.ready) {
      lines.push(`<span class="detect-muted">框在背包本地烧进视频 → ${escHtml(s.preview_stream || 'preview')}（该路画面已切换，浏览器不叠框）</span>`);
      // 画布写入实测值直接摆出来：低于目标 70% 说明 Python 渲染在反压视频，
      // 这是「AI 那一路比别路慢好几秒」最直观的自诊断依据。
      const wfps = Number(osd.write_fps) || 0;
      const cfps = Number(osd.fifo_fps) || 0;
      const slow = !!(cfps && wfps && wfps < cfps * 0.7);
      lines.push(`<span class="${slow ? 'detect-warn' : 'detect-muted'}">preview 低延迟${osd.low_latency === false ? '（已关闭）' : ''}：视频 ${Number(osd.video_fps) || 30}fps / 画布 ${cfps || '—'}fps（实测写入 ${wfps || '—'}，渲染 ${Number(osd.render_ms) || 0}ms）${slow ? '；画布写入跟不上，请继续调低 OSD_FIFO_FPS' : ''}</span>`);
    } else if (osdFailedCam) {
      lines.push(`<span class="detect-warn">${escHtml(s.preview_stream || 'preview')} 流未能出图，已切回 ${escHtml(osdFailedCam)} 原始画面并回退浏览器叠加；检查 mediamtx 是否重启并登记该 path，ffmpeg 日志 ${escHtml(osd.log || '/tmp/detect_osd.log')}</span>`);
    } else if (osd.enabled === false) {
      lines.push('<span class="detect-muted">板端画框已禁用（DETECT_OSD_ENABLED=0），使用浏览器叠加</span>');
    } else {
      // osd.error 末尾已带日志摘要，这里拆开显示，网页上更易读
      const errMain = String(osd.error || '').split('日志尾部：')[0];
      lines.push(`<span class="detect-warn">板端画框未就绪${errMain ? '：' + escHtml(errMain) : ''}，已回退浏览器叠加</span>`);
      if (osd.log_tail) lines.push(`<span class="detect-muted">ffmpeg 日志尾部：${escHtml(osd.log_tail)}</span>`);
    }
    const vals = Object.keys(s.stats || {}).map(c => Number(s.stats[c].fps) || 0);
    const avg = vals.length ? vals.reduce((a, b) => a + b, 0) / vals.length : 0;
    const nCam = Math.max(1, (s.cams || []).length);
    // 单进程串行分时：每秒总推理次数 = 1000/单次耗时，四路平分。
    // 把物理上限直接摆出来，避免「目标 12」看起来像程序没跑够。
    const msVals = Object.keys(s.stats || {}).map(c => Number(s.stats[c].last_ms) || 0).filter(x => x > 0);
    const ms = msVals.length ? Math.max(...msVals) : 0;
    if (ms) {
      const cap = 1000 / ms / nCam;
      lines.push(`<span class="detect-muted">物理上限约 ${cap.toFixed(1)} 帧/秒（单次推理 ${ms}ms，单路独占 NPU，无分时摊薄）</span>`);
    }
    // 启动头几秒各路 fps 还没收敛，等所有勾选路都有数据再判断是否降速
    if (vals.length >= nCam && avg && avg < s.fps * 0.8) {
      lines.push(`<span class="detect-warn">实测约 ${avg.toFixed(1)} 帧/秒：目标 ${s.fps} 超出 NPU 能力，已按物理上限运行；可适当下调目标帧率</span>`);
    }
    const per = Object.keys(s.stats || {}).sort().map(c => {
      const v = s.stats[c];
      return `${escHtml(c)} ${v.fps || 0}fps / ${v.last_ms || 0}ms / 命中 ${v.hits || 0}`;
    }).join('　');
    if (per) lines.push(`<span class="detect-muted">${per}</span>`);
    (s.warnings || []).forEach(w => lines.push(`<span class="detect-warn">${escHtml(w)}</span>`));
  } else if (!s.error) {
    lines.push('未开启（点击「开始识别」后占用 NPU，平时不影响遥测刷新）');
  }
  el.innerHTML = lines.join('<br />');
  // 以后端实际配置纠正勾选状态（例如切场景瞬间后端会重置为全选）；
  // 用户刚操作过勾选框时留 1.5s 宽限，避免把正在进行的勾选覆盖掉。
  if (s.running && Date.now() - lastDetectLabelTouch > 1500) {
    const picked = s.labels;   // null = 全部有效类别
    document.querySelectorAll('#detect-labels input').forEach(i => {
      i.checked = (picked == null) || (Array.isArray(picked) && picked.indexOf(i.value) >= 0);
    });
  }
  // 以后端实际生效的单路纠正 radio（后端 _norm_cams 可能把非法值回落成 dogcam）
  if (s.running && Date.now() - lastDetectCamTouch > 1500) {
    const cam = (s.cams || [])[0];
    if (cam) {
      const radios = Array.from(document.querySelectorAll('#detect-cams input[type="radio"]'));
      const usable = radios.filter(i => !i.disabled);
      // dogcam 未配置源时该 radio 被禁用；后端若仍指向它（例如直接调 API 启动），
      // 落到第一路可用摄像头，避免出现「没有任何 radio 被选中」。
      const picked = usable.find(i => i.value === cam) || usable[0];
      radios.forEach(i => { if (!i.disabled) i.checked = (i === picked); });
    }
  }
  const startBtn = document.getElementById('btn-detect-start');
  const stopBtn = document.getElementById('btn-detect-stop');
  if (startBtn) startBtn.disabled = detectRunning;
  if (stopBtn) stopBtn.disabled = !detectRunning;
}

let detectConfigTimer = null;
let detectConfigPatch = {};
function queueDetectConfig(patch) {
  if (!detectRunning) return;
  // 合并 300ms 内的多次改动：否则「切场景 + 调阈值」只会发出最后一项
  Object.assign(detectConfigPatch, patch);
  clearTimeout(detectConfigTimer);
  detectConfigTimer = setTimeout(async () => {
    const body = detectConfigPatch;
    detectConfigPatch = {};
    const r = await postJson('/api/detect/config', body);
    if (!r.ok) showHint('detect-err', r.error || '配置更新失败', true);
    else { showHint('detect-err', '', false); refreshDetectStatus(); }
  }, 300);
}

function bindDetectControls() {
  renderDetectCams();

  const taskSel = document.getElementById('detect-task');
  if (taskSel) taskSel.onchange = () => {
    applyTaskDefaults();
    queueDetectConfig({ task: taskSel.value, labels: null });
  };

  const confEl = document.getElementById('detect-conf');
  if (confEl) {
    confEl.dataset.touched = '0';
    confEl.onchange = () => {
      confEl.dataset.touched = '1';
      let v = parseFloat(confEl.value);
      if (!Number.isFinite(v)) v = 0.25;
      v = Math.min(0.95, Math.max(0.05, v));
      confEl.value = String(v);
      queueDetectConfig({ conf: v });
    };
  }

  const fpsEl = document.getElementById('detect-fps');
  if (fpsEl) fpsEl.onchange = () => {
    const v = Math.min(DETECT_FPS_MAX, Math.max(1, parseInt(fpsEl.value, 10) || 6));
    fpsEl.value = String(v);
    queueDetectConfig({ fps: v });
  };

  ['detect-labels', 'detect-cams'].forEach(id => {
    const box = document.getElementById(id);
    if (!box) return;
    box.addEventListener('change', () => {
      if (id === 'detect-labels') lastDetectLabelTouch = Date.now();
      if (id === 'detect-cams') lastDetectCamTouch = Date.now();
      const patch = id === 'detect-labels'
        ? { labels: collectDetectConfig().labels }
        : { cams: collectDetectConfig().cams };
      if (id === 'detect-cams' && !patch.cams.length) {
        showHint('detect-err', '至少选择一路摄像头', true);
        return;
      }
      if (id === 'detect-labels' && !checkedValues('detect-labels').length) {
        showHint('detect-err', '至少勾选一个识别类别', true);
        return;
      }
      showHint('detect-err', '', false);
      queueDetectConfig(patch);
    });
  });

  document.getElementById('btn-detect-start').onclick = async () => {
    const cfg = collectDetectConfig();
    if (!cfg.task) { showHint('detect-err', '没有可用的识别场景', true); return; }
    if (!cfg.cams.length) { showHint('detect-err', '至少选择一路摄像头', true); return; }
    if (!checkedValues('detect-labels').length) { showHint('detect-err', '至少勾选一个识别类别', true); return; }
    showHint('detect-err', '', false);
    const r = await postJson('/api/detect/start', cfg);
    if (!r.ok) { showHint('detect-err', r.error || '启动失败', true); }
    else { clearAllBoxes(); detectRunning = true; }
    refreshDetectStatus();
  };

  document.getElementById('btn-detect-stop').onclick = async () => {
    await postJson('/api/detect/stop', {});
    clearAllBoxes();
    detectRunning = false;
    refreshDetectStatus();
  };

  // 版本 3：任务下发
  const submitBtn = document.getElementById('btn-job-submit');
  if (submitBtn) submitBtn.onclick = async () => {
    if (jobBusy) { showHint('job-err', '任务进行中，请等待结束或先取消', true); return; }
    const cfg = collectDetectConfig();
    if (!cfg.task) { showHint('job-err', '没有可用的识别场景', true); return; }
    if (!cfg.cams.length) { showHint('job-err', '至少选择一路摄像头', true); return; }
    const jid = (document.getElementById('job-id').value || '').trim();
    const planId = (document.getElementById('job-plan-id').value || '').trim();
    const checkType = (document.getElementById('job-check-type').value || 'exist').trim();
    const expectRaw = (document.getElementById('job-expect-count').value || '').trim();
    const expectCount = (checkType === 'count' && expectRaw)
      ? Math.max(1, Math.min(999, parseInt(expectRaw, 10) || 0)) : null;
    if (checkType === 'count' && !expectCount) {
      showHint('job-err', '检测数量是否正确时必须填写检查目标数量', true);
      return;
    }
    const duration = parseInt(document.getElementById('job-duration').value, 10) || 5;
    const streak = parseInt(document.getElementById('job-streak').value, 10) || 3;
    showHint('job-err', '', false);
    const r = await postJson('/api/detect/job', {
      job_id: jid, task: cfg.task, labels: cfg.labels, conf: cfg.conf,
      cams: cfg.cams, fps: cfg.fps, duration: duration, streak: streak,
      plan_id: planId || null, point_index: null,
      check_type: checkType, expect_count: expectCount,
    });
    if (!r.ok) { showHint('job-err', r.error || '下发失败', true); return; }
    clearAllBoxes();
    refreshDetectStatus();
  };
  const cancelBtn = document.getElementById('btn-job-cancel');
  if (cancelBtn) cancelBtn.onclick = async () => {
    await postJson('/api/detect/job/cancel', {});
    jobBusy = false;
    refreshDetectStatus();
  };

}

// ---------- 启动 ----------
(async function () {
  await initMeta();
  initDebug();
  // 先建好叠加层，WHEP 兜底分支才能正确禁用它
  initDetectOverlays();
  bindDetectControls();
  // 把 PC 端运行命令里的「板端IP」替换为当前实际访问的板子 IP
  const runCmd = document.getElementById('phone-run-cmd');
  if (runCmd) runCmd.textContent =
    `python voice_call_pc.py --peer ${location.hostname} --peer-port 5000 --local-port 5000`;
  // 先拿 relay 状态（1.2s 超时兜底）。未配置源时不要给 dogcam 创建 WHEP reader：
  // 它必然连不上，degrade 出的 iframe 就是「页面最下方那个黑框」，还会被统计
  // 成「1 路走了内置播放页」。改为保留一张写明原因的占位卡。
  state.relay = await fetchRelayStatus();
  const dogcamDisabled = !!(state.relay && state.relay.enabled === false);
  // 读取前置档位（1080p60 直拉 dogcam / 720p30 relay 推 dogcam720），
  // 失败时按 relay.status().stream 兜底，再退回 dogcam。
  let dogcamBase = 'dogcam';
  try {
    const pr = await fetch('/api/dogcam/profile', { cache: 'no-store' });
    const ps = await pr.json();
    applyDogcamProfileUi(ps.profile || '1080p60');
    dogcamBase = ps.stream || dogcamBase;
  } catch (e) {
    dogcamBase = (state.relay && state.relay.stream) || dogcamBase;
  }
  bindDogcamProfile();
  document.querySelectorAll('.video-card').forEach(card => {
    const stream = card.getAttribute('data-stream');
    // dogcam 卡的实际干净流随档位变化；data-stream 仍是逻辑名 dogcam
    const actual = (stream === 'dogcam') ? dogcamBase : stream;
    card._baseStream = actual;                 // 切回干净流时用（含档位 stream）
    if (stream === 'dogcam' && dogcamDisabled) return;   // 不创建 reader
    const video = card.querySelector('video');
    const msg = card.querySelector('.vc-msg');
    card._whep = connectWebrtc(video, msg, actual);
  });
  applyRelayUi(dogcamDisabled);
  // 场景清单独立异步加载，不 await，避免拖慢首屏其他面板
  refreshDetect();
  refreshDetectStatus();
  setInterval(refreshDetectStatus, 3000);
  refreshLights();
  setInterval(refreshLights, 5000);
  const dr = document.getElementById('btn-detect-refresh');
  if (dr) dr.onclick = (ev) => {
    const b = ev.currentTarget;
    b.classList.add('spin');
    setTimeout(() => b.classList.remove('spin'), 600);
    refreshDetect();
    refreshDetectStatus();
  };
  connectSSE();
})();
