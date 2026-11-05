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
cargo run                    # Run API server (port 3000)
cargo test                   # Run all tests
cargo clippy -- -D warnings # Lint
```

### Device client (Rust)
```bash
cd client
cargo run -- --device-id dog-001 --once    # auth + register + heartbeat check
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

Device gateway (not user JWT): `GET /api/dogGateway/auth/{deviceId}` → token,
`POST /api/dogGateway/register`, then `WS /api/notice/{deviceId}/{token}`.
End-to-end coverage lives in `backend/tests/business/dog_gateway_tests.rs`.

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
