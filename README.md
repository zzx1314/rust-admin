# Rust Admin

A full-stack admin panel built with a **Rust** backend and **Vue 3** frontend. The backend provides a RESTful API powered by Axum and SeaORM with SQLite, while the frontend is a modern admin dashboard based on vue-pure-admin.

## ✨ Features

- **Authentication**: OAuth2 token-based auth with JWT, login/logout/refresh token support
- **User Management**: CRUD operations, pagination, password reset, enable/disable toggle, role assignment
- **Role Management**: CRUD operations, role-based access control, role-menu authorization
- **Menu Management**: Tree-structured menus, per-user menu permissions, parent-child hierarchy
- **Organization Management**: Tree-structured org units, batch delete, parent-child queries
- **Dictionary Management**: System dictionaries and dictionary items with type-based lookups
- **System Log**: Operation logging with pagination and sensitive data masking
- **Request Logging Middleware**: Automatic HTTP request tracking with response time measurement

## 🏗️ Architecture

```
rust-admin/
├── backend/           # Rust REST API (Axum + SeaORM + SQLite)
│   ├── src/
│   │   ├── api/       # HTTP layer (routes, middleware, app state)
│   │   ├── auth/      # Authentication (JWT + Redis token store)
│   │   ├── common/    # Shared utilities (error handling, pagination, traits)
│   │   ├── config/    # Application configuration
│   │   ├── migration/ # Database migrations
│   │   └── system/    # Business modules (user, role, menu, org, dict, log, auth)
│   ├── tests/         # Integration tests (organized by module)
│   └── config.toml    # Server, Redis, and database configuration
└── frontend/          # Vue 3 admin panel (vue-pure-admin)
    ├── src/
    │   ├── api/       # API service functions
    │   ├── router/    # Vue Router configuration
    │   ├── store/     # Pinia state management
    │   ├── views/     # Page components
    │   └── ...       # Components, plugins, styles, utils
    └── vite.config.ts # Dev server with API proxy
```

### Backend Architecture

The backend follows a **layered architecture** with trait-based dependency injection, inspired by Spring patterns:

| Layer | Responsibility | Example |
|-------|---------------|---------|
| **Handlers** | HTTP request/response | `handlers.rs` |
| **Service** | Business logic | `service.rs` |
| **Repository** | Data access (trait + impl) | `repository.rs` |
| **Domain** | Types, DTOs, entity aliases | `domain.rs` |
| **Entity** | SeaORM database models | `entity.rs` |

Each module under `system/` follows this consistent structure, making it easy to add new business domains.

### Frontend Architecture

The frontend is based on the [vue-pure-admin](https://github.com/pure-admin/vue-pure-admin-max) template with Element Plus UI and TailwindCSS:

- **Vue 3** Composition API with `<script setup>` syntax
- **Pinia** for state management
- **Element Plus** component library
- **TailwindCSS 4** utility-first styling
- **Vue Router** with lazy-loaded route modules
- **Axios** for HTTP requests with auto-refresh token support
- **i18n** multi-language support (en, zh-CN, zh-TW, ja, ko)

## 🛠️ Tech Stack

### Backend

| Technology | Version | Purpose |
|-----------|---------|---------|
| [Rust](https://www.rust-lang.org/) | Edition 2024 | Language |
| [Axum](https://github.com/tokio-rs/axum) | 0.8 | Web framework |
| [SeaORM](https://www.sea-ql.org/SeaORM/) | 1.1 | ORM (SQLite) |
| [Tokio](https://tokio.rs/) | 1 | Async runtime |
| [Redis](https://redis.io/) | — | Token/session storage |
| [SQLite](https://www.sqlite.org/) | — | Primary database |
| [JWT](https://jwt.io/) | — | Authentication tokens |

### Frontend

| Technology | Version | Purpose |
|-----------|---------|---------|
| [Vue](https://vuejs.org/) | 3.5+ | UI framework |
| [TypeScript](https://www.typescriptlang.org/) | 5.8+ | Type safety |
| [Vite](https://vite.dev/) | 6 | Build tool |
| [Element Plus](https://element-plus.org/) | 2.9+ | UI component library |
| [TailwindCSS](https://tailwindcss.com/) | 4 | Utility CSS |
| [Pinia](https://pinia.vuejs.org/) | 3 | State management |
| [Vue Router](https://router.vuejs.org/) | 4 | Routing |
| [VxeTable](https://vxetable.cn/) | 4.6 | Advanced table component |
| [ECharts](https://echarts.apache.org/) | 5 | Charts |
| [Axios](https://axios-http.com/) | 1.9 | HTTP client |

## 🚀 Getting Started

### Prerequisites

- **Rust** (latest stable, edition 2024 support)
- **Node.js** `^18.18.0 || ^20.9.0 || >=22.0.0`
- **pnpm** `>=9`
- **Redis** server running on `localhost:6379`

### 1. Clone the Repository

```bash
git clone <your-repo-url>
cd rust-admin
```

### 2. Start the Backend

```bash
cd backend

# Configure the database and Redis (edit config/config.toml if needed)
# The default config uses SQLite (auto-created) and localhost Redis

# Run database migrations and start the server
cargo run
```

The API server starts at `http://127.0.0.1:3000`.

### 3. Start the Frontend

```bash
cd frontend

# Install dependencies
pnpm install

# Start the dev server
pnpm dev
```

The frontend dev server starts (port configured in `.env`), with `/api` requests proxied to `http://127.0.0.1:3000`.

> **Note**: Run the backend and frontend in separate terminal sessions.

## 📡 API Endpoints

All API endpoints are prefixed with `/api`. Protected routes require a Bearer token in the `Authorization` header.

| Module | Endpoint Pattern | Description |
|--------|-----------------|-------------|
| **Auth** | `POST /api/token` | Login |
| **Auth** | `GET /api/token/logout` | Logout |
| **Auth** | `GET /api/token/refresh/{refresh_token}` | Refresh token |
| **Auth** | `GET /api/token/check_token` | Validate token |
| **User** | `GET /api/sysUser/getPage` | Users (paginated) |
| **User** | `POST/GET/PUT/DELETE /api/sysUser[/{id}]` | User CRUD |
| **User** | `PUT /api/sysUser/resetPwd` | Reset password |
| **User** | `PUT /api/sysUser/enable` | Toggle user enabled |
| **Role** | `GET /api/sysRole/getPage` | Roles (paginated) |
| **Role** | `POST/GET/PUT/DELETE /api/sysRole[/{id}]` | Role CRUD |
| **Menu** | `GET /api/sysMenu/tree` | Menu tree |
| **Menu** | `GET /api/sysMenu/user-menu` | Current user's menus |
| **Menu** | `POST/GET/PUT/DELETE /api/sysMenu[/{id}]` | Menu CRUD |
| **Org** | `GET /api/sysOrg/tree` | Organization tree |
| **Org** | `POST/GET/PUT/DELETE /api/sysOrg[/{id}]` | Org CRUD |
| **Dict** | `GET /api/sysDict/getPage` | Dictionaries (paginated) |
| **Dict** | `POST/GET/PUT/DELETE /api/sysDict[/{id}]` | Dict CRUD |
| **Dict Item** | `GET /api/sysDictItem/getByType` | Items by type |
| **Dict Item** | `POST/GET/PUT/DELETE /api/sysDictItem[/{id}]` | Dict Item CRUD |
| **Log** | `GET /api/sysLog/getPage` | Logs (paginated) |
| **Log** | `POST/GET/PUT/DELETE /api/sysLog[/{id}]` | Log CRUD |
| **Auth Config** | `GET /api/sysAuth/getMenuData/{role_code}` | Role menu data |
| **Auth Config** | `POST /api/sysAuth/setMenuAuth` | Set menu permissions |

## 🧪 Testing

### Backend

```bash
cd backend

# Run all tests
cargo test

# Run a specific test module
cargo test --test user_service_tests
cargo test --test role_api_tests
cargo test --test auth_service_tests

# Run a single test by name
cargo test -- <test_name>
```

Tests use UUID-based temporary SQLite database files that are automatically created and cleaned up.

### Frontend

```bash
cd frontend

# Lint (required before commit)
pnpm lint

# Type checking
pnpm typecheck
```

> **Note**: No frontend test framework is currently configured.

## 🔧 Configuration

### Backend (`backend/config/config.toml`)

```toml
jwt_secret = "your-secret-key"
database_url = "sqlite:data/users.db?mode=rwc"  # Adjust path to your environment

[server]
host = "127.0.0.1"
port = 3000

[redis]
host = "localhost"
port = 6379
password = "123456"
```

### Frontend (`frontend/vite.config.ts`)

The dev server proxies `/api` → `http://127.0.0.1:3000` for seamless backend integration.

## 📝 Code Quality

### Backend

```bash
cd backend
cargo fmt && cargo fmt -- --check   # Format check
cargo clippy -- -D warnings         # Lint (zero warnings)
```

### Frontend

```bash
cd frontend
pnpm lint              # Run all linters (ESLint + Prettier + Stylelint)
pnpm lint:eslint       # Lint JS/TS/Vue only
pnpm lint:prettier     # Format code
pnpm lint:stylelint    # Lint CSS/SCSS
pnpm typecheck         # TypeScript type checking
```

Husky + lint-staged automatically run linters on commit.

## 🐳 Docker Deployment

The frontend includes Docker support for containerized deployment:

```bash
cd frontend

# Build and run with Docker Compose
docker-compose up --build
```

See `frontend/Dockerfile` and `frontend/docker-compose.yml` for configuration details.

## 📦 Production Build

### Backend

```bash
cd backend
cargo build --release    # Output → target/release/x-rust
```

### Frontend

```bash
cd frontend
pnpm build               # Output → dist/
```

## 📄 License

MIT
