# AGENTS.md — rust-admin

Monorepo containing a Rust backend API, a Vue 3 frontend admin panel and the standalone `business/` robot-dog service (Rust backend + Vue console).

## Project Structure

```
./
├── backend/          # Rust REST API (axum + sea-orm + SQLite)
├── business/         # Standalone robot-dog service: parks/devices/tasks/routes + console
└── frontend/         # Vue 3 admin panel (vue-pure-admin-max)
```

**Each subproject has its own detailed AGENTS.md — always read those first for domain-specific guidance:**
- `backend/AGENTS.md` — Rust backend patterns, SeaORM conventions, test structure
- `frontend/AGENTS.md` — Vue/TypeScript frontend patterns, component conventions
- `business/README.md` — standalone robot-dog service (parks, devices, tasks, routes)
- `DOG.md` — middle-platform resource module (maps, models, media, config)

## Commands

### Backend (Rust)
```bash
cd backend
./run.sh                     # Run API server (port 3000); exports the required secrets first
cargo run                    # Same, but requires APP_JWT_SECRET to be exported
cargo test                   # Run all tests
cargo clippy -- -D warnings # Lint
```

### Frontend (Vue)
```bash
cd frontend
pnpm dev                     # Dev server
pnpm build                   # Production build
pnpm lint                    # Lint (required before commit)
```

### Running Both
No combined command — run backend and frontend in separate terminals:
```bash
# Terminal 1
cd backend && cargo run

# Terminal 2
cd frontend && pnpm dev
```

## API Integration

Frontend proxies `/api` → `http://127.0.0.1:3000` (configured in `frontend/vite.config.ts`).

OAuth2 token endpoint: `/api/token` with Bearer token in Authorization header.

Device onboarding, gateways and task/route management are owned by the
`business/` service (see `business/README.md`). The root backend exposes only
the resource domains listed in `DOG.md`.

## Secrets

`backend/config/config.toml` is committed and holds **no** secrets. Required
environment variable: `APP_JWT_SECRET` (≥32 bytes). Optional:
`APP_REDIS_PASSWORD`, `APP_CONFIG_FILE`, `APP_DOG__BUSINESS_API_TOKEN`
（中台设备页「同步地图 / 指令 / 任务」调用业务平台内部接口的共享令牌，需与业务平台
`ROBODOG_SERVICE_TOKEN` 一致）。
Any config key can be overridden with `APP_<SECTION>__<KEY>`.

业务平台部署在没有入站端口的内网时，中台无法主动连它，改由业务平台拨入中台
的反向控制信道（`GET /api/dogDevice/control-plane/ws`）。此时业务平台需额外配置
`ROBODOG_ZENTAI_WS_URL`（如 `ws://10.0.0.8:3000`）；不配置则沿用「中台直连
HTTP」模式。两者共用同一个服务令牌，见 `business/README.md`。

`backend/run.sh` exports the required secret (and probes Redis for
`APP_REDIS_PASSWORD`) before running `cargo run`. Set `APP_REDIS_PASSWORD`
when Redis enforces `requirepass` (the compose one uses `123456`), otherwise
login fails with `Redis set failed: NOAUTH: Authentication required`.

## Important Notes

- **Backend**: SQLite database at `backend/data/users.db`, Redis for session storage
- **Frontend**: Requires Node 18+/pnpm 9+, 4GB RAM for dev, 8GB for build
- Both projects have pre-commit hooks — run lint before committing
- Backend tests use UUID-based temporary database files
- Map uploads land in `[dog] upload_dir` (default `backend/data/device_uploads/maps`)

## When to Use Which AGENTS.md

| Task | Read |
|------|------|
| Backend code changes | `backend/AGENTS.md` |
| Frontend code changes | `frontend/AGENTS.md` |
| Robot-dog park/device/task/route changes | `business/README.md` |
| Architecture questions | Both sub-AGENTS.md |
| Running/building | This file |
