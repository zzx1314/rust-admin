#!/usr/bin/env python3
"""现场入口：把控制台静态产物托管在内网机器上。

## 为什么需要它

Chrome 的 **Local Network Access**（PNA 的继任者）禁止「公网页面」访问「内网地址」，
实测报错是 `corsError=InsecureLocalNetwork`：

```text
页面 http://39.107.55.13:3100  ──fetch──▶  http://192.168.41.246:8000   ✗ 被浏览器拦掉
页面 http://192.168.41.246:8080 ──fetch──▶  http://192.168.41.246:8000   ✓ 同为内网
```

背包的 REST/SSE 与控制台不同源，视频更是浏览器要直连板端 mediamtx（:8889）。
只要**页面本身在内网**打开，这两条通路都不再受限制，而业务 API 仍走云端
（内网页面访问公网 API 是允许的）。

所以：同一份 `business/web/dist`，
- 云端 `http://39.107.55.13:3100` —— 常规后台（设备/任务/地图/告警）；
- 现场 `http://<内网机>:8080`    —— 额外能看实时视频与 AI。

## 用法

```bash
CONSOLE_DIR=/opt/site-console/web \
CONSOLE_API_BASE=http://39.107.55.13:3100/api \
CONSOLE_PORT=8080 \
python3 serve.py
```

`/runtime-config.js` 由本进程生成（`window.__ROBODOG_RUNTIME__`），前端
`src/utils/runtime.ts` 读它来决定 API 基址与视频 WS 基址——**不需要重新构建前端**。

只用标准库：现场机器（含背包板子）一定有 python3，不必装 nginx。
"""

import json
import os
import posixpath
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import unquote, urlparse

ROOT = os.path.abspath(os.environ.get("CONSOLE_DIR", "/opt/site-console/web"))
API_BASE = os.environ.get(
    "CONSOLE_API_BASE", "http://39.107.55.13:3100/api"
).rstrip("/")
# 未显式给出时按 API 主机推同端口的 ws（设备视频中继也在这个服务里）
_video_ws = os.environ.get("CONSOLE_VIDEO_WS_BASE", "").strip()
if not _video_ws:
    _rest = API_BASE.split("://", 1)[-1].split("/", 1)[0]
    _scheme = "wss" if API_BASE.startswith("https://") else "ws"
    _video_ws = "%s://%s" % (_scheme, _rest)
VIDEO_WS_BASE = _video_ws.rstrip("/")
PORT = int(os.environ.get("CONSOLE_PORT", "8080"))
BIND = os.environ.get("CONSOLE_BIND", "0.0.0.0")

TYPES = {
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".mjs": "text/javascript; charset=utf-8",
    ".css": "text/css; charset=utf-8",
    ".json": "application/json; charset=utf-8",
    ".svg": "image/svg+xml",
    ".png": "image/png",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".gif": "image/gif",
    ".ico": "image/x-icon",
    ".woff": "font/woff",
    ".woff2": "font/woff2",
    ".ttf": "font/ttf",
    ".map": "application/json; charset=utf-8",
    ".txt": "text/plain; charset=utf-8",
}

RUNTIME_JS = (
    "/* 由 deploy/site-console/serve.py 生成：现场入口的运行时配置 */\n"
    "window.__ROBODOG_RUNTIME__ = %s;\n"
    % json.dumps({"apiBase": API_BASE, "videoWsBase": VIDEO_WS_BASE}, ensure_ascii=False)
)

# `index.html` 里注入 runtime-config：dist 是两份部署共用的产物，
# 不能为了现场入口单独改一版构建，所以在**服务端**注入（启动时算一次）。
RUNTIME_TAG = b'<script src="/runtime-config.js"></script>'


def load_index() -> bytes:
    path = os.path.join(ROOT, "index.html")
    if not os.path.isfile(path):
        sys.exit("CONSOLE_DIR=%s 下没有 index.html，先部署 business/web/dist" % ROOT)
    with open(path, "rb") as fh:
        html = fh.read()
    if RUNTIME_TAG in html:
        return html
    marker = b"</head>"
    if marker in html:
        return html.replace(marker, RUNTIME_TAG + marker, 1)
    return RUNTIME_TAG + html


INDEX_HTML = None


def _safe_path(url_path: str) -> str:
    """把 URL 路径映射到 ROOT 内，挡住 `..` 穿越。"""
    clean = posixpath.normpath(unquote(url_path)).lstrip("/")
    if clean in ("", "."):
        return ROOT
    target = os.path.abspath(os.path.join(ROOT, clean))
    if not target.startswith(ROOT):
        return ROOT  # 越界一律当作根，回落到 index.html
    return target


class Handler(BaseHTTPRequestHandler):
    server_version = "SiteConsole/1.0"

    def log_message(self, fmt, *args):  # 只在错的时候留痕，别把 journal 刷满
        if args and str(args[1] if len(args) > 1 else "").startswith(("4", "5")):
            sys.stderr.write("[site-console] %s\n" % (fmt % args))

    def _send(self, body: bytes, ctype: str, cache: str, code: int = 200):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", cache)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def _serve_file(self, path: str, cache: str):
        with open(path, "rb") as fh:
            body = fh.read()
        ext = os.path.splitext(path)[1].lower()
        self._send(body, TYPES.get(ext, "application/octet-stream"), cache)

    def do_HEAD(self):
        self.do_GET()

    def do_GET(self):
        path = urlparse(self.path).path

        if path == "/runtime-config.js":
            # 配置随部署变化：禁止缓存，否则改了 API 地址页面还在用旧的
            return self._send(
                RUNTIME_JS.encode("utf-8"),
                TYPES[".js"],
                "no-store, no-cache, must-revalidate",
            )

        if path == "/healthz":
            return self._send(b"ok", "text/plain; charset=utf-8", "no-store")

        target = _safe_path(path)
        if os.path.isfile(target):
            # 带 hash 的构建产物可长缓存；index.html 必须每次回源
            cache = (
                "no-store, no-cache, must-revalidate"
                if target.endswith(".html")
                else "public, max-age=31536000, immutable"
            )
            return self._serve_file(target, cache)

        # SPA 深链（/monitor、/devices…）回落到 index.html
        if INDEX_HTML:
            return self._send(
                INDEX_HTML, TYPES[".html"], "no-store, no-cache, must-revalidate"
            )

        self._send("console dist 未部署".encode("utf-8"), "text/plain; charset=utf-8", "no-store", 404)


def main():
    global INDEX_HTML
    INDEX_HTML = load_index()
    print(
        "[site-console] %s:%s  dir=%s  api=%s  videoWs=%s"
        % (BIND, PORT, ROOT, API_BASE, VIDEO_WS_BASE),
        flush=True,
    )
    ThreadingHTTPServer((BIND, PORT), Handler).serve_forever()


if __name__ == "__main__":
    main()
