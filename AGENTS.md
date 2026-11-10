# AGENTS.md — rust-admin

Monorepo containing a Rust backend API, a Rust device client and a Vue 3 frontend admin panel.

## Project Structure

```
./
├── backend/          # Rust REST API (axum + sea-orm + SQLite)
├── client/           # Rust robot-dog device client (dog-device-client)
└── frontend/         # Vue 3 admin panel (vue-pure-admin-max)
```

**Each subproject has its own detailed AGENTS.md — always read those first for domain-specific guidance:**
- `backend/AGENTS.md` — Rust backend patterns, SeaORM conventions, test structure
- `frontend/AGENTS.md` — Vue/TypeScript frontend patterns, component conventions
- `client/README.md` — device client usage and the device gateway wire protocol
- `DOG.md` — robot-dog business module overview (tables, endpoints, gateway)

## Commands

### Backend (Rust)
```bash
cd backend
./run.sh                     # Run API server (port 3000); exports the required secrets first
cargo run                    # Same, but requires APP_JWT_SECRET + APP_DEVICE_AUTH_SECRET to be exported
cargo test                   # Run all tests
cargo clippy -- -D warnings # Lint
```

### Device client (Rust)
```bash
cd client
cargo run -- --once                         # hardware device id + auth + register + heartbeat check
cargo run -- --print-device-id              # show the hardware-derived device id
cargo run -- --explain-device-id            # show every probed source and its status
cargo run -- --device-id-file /tmp/id --once # pin the detected id in a file
cargo run -- --no-reconnect                # disable the default auto-reconnect
cargo test                                  # Client unit tests
cargo clippy --all-targets -- -D warnings   # Lint
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

The client derives its `deviceId` from hardware (board serial / eMMC CID / DMI
UUID / physical MAC, `/etc/machine-id` as last resort; `--device-id` overrides).

Device gateway (not user JWT): `GET /api/dogGateway/auth/{deviceId}` → token,
`POST /api/dogGateway/register`, then `WS /api/notice/{deviceId}/{token}`.
The auth call requires an HMAC-SHA256 signature over
`"{deviceId}\n{timestamp}\n{nonce}"` sent as `X-Device-Timestamp`,
`X-Device-Nonce` and `X-Device-Signature`, keyed by the server's
`APP_DEVICE_AUTH_SECRET`. Requests outside a ±300 s window or replaying a
nonce are rejected with 401.
End-to-end coverage lives in `backend/tests/business/dog_gateway_tests.rs`.

## Secrets

`backend/config/config.toml` is committed and holds **no** secrets. Required
environment variables: `APP_JWT_SECRET` (≥32 bytes) and
`APP_DEVICE_AUTH_SECRET`. Optional: `APP_REDIS_PASSWORD`, `APP_CONFIG_FILE`.
Any config key can be overridden with `APP_<SECTION>__<KEY>`.

## Important Notes

- **Backend**: SQLite database at `backend/data/users.db`, Redis for session storage
- **Frontend**: Requires Node 18+/pnpm 9+, 4GB RAM for dev, 8GB for build
- Both projects have pre-commit hooks — run lint before committing
- Backend tests use UUID-based temporary database files
- Device uploads land in `[dog] upload_dir` (default `backend/data/device_uploads`)

## When to Use Which AGENTS.md

| Task | Read |
|------|------|
| Backend code changes | `backend/AGENTS.md` |
| Frontend code changes | `frontend/AGENTS.md` |
| Architecture questions | Both sub-AGENTS.md |
| Running/building | This file |
