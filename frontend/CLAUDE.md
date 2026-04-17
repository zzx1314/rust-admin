# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is the frontend for a drone management platform, built with Vue 3, TypeScript, Vite, Element Plus, and TailwindCSS. It's based on vue-pure-admin-max template.

## Commands

### Development
```bash
pnpm dev              # Start dev server (4GB memory, port from .env)
pnpm serve            # Alias for dev
```

### Build
```bash
pnpm build            # Production build (8GB memory)
pnpm build:staging    # Staging build
pnpm preview          # Preview production build locally
pnpm preview:build    # Build then preview
```

### Linting & Type Checking
```bash
pnpm typecheck        # TypeScript + Vue-TSC type checks
pnpm lint:eslint      # Lint Vue/JS/TS files (auto-fix)
pnpm lint:prettier    # Format code (auto-fix)
pnpm lint:stylelint   # Lint CSS/SCSS (auto-fix)
pnpm lint             # Run all linters
```

### Utilities
```bash
pnpm svgo                         # Optimize SVG files
pnpm clean:cache          # Clear caches and reinstall
```

## Tech Stack

- **Node**: `^18.18.0 || ^20.9.0 || >=22.0.0`
- **Package Manager**: pnpm 9+
- **Framework**: Vue 3.5+ with Composition API
- **Language**: TypeScript
- **Build Tool**: Vite 6
- **UI Library**: Element Plus + TailwindCSS 4
- **State Management**: Pinia 3
- **Router**: Vue Router 4
- **HTTP**: Axios
- **Utilities**: VueUse, @pureadmin/utils

## Project Structure

```
src/
├── api/              # API functions (service.ts, base.ts, domain-specific files)
├── assets/           # Static assets (images, icons, SVGs)
├── components/       # Reusable components (Re* prefix convention)
├── hooks/            # Composable hooks (use* pattern)
├── layout/           # Layout components (sidebar, header, etc.)
├── router/           # Vue Router config (routes/modules/)
├── store/            # Pinia stores (store/modules/, store/types.ts)
├── style/            # Global styles (index.scss, theme.scss, dark.scss)
├── utils/            # Utility functions
└── views/            # Page components (system/, drones/, etc.)
```

## Code Style

### Import Conventions
- Use `import type` for type-only imports
- Group imports: external libs → internal imports → styles
- **Omit file extensions** (`.ts`, `.tsx`, `.vue`) in imports - tsconfig uses `moduleResolution: "bundler"`

```typescript
import { defineStore } from "pinia";
import type { UserState } from "./types";
import { useUserApi } from "@/api/user";
import "./styles/index.scss";
```

### Vue Components
- Use `<script setup lang="ts">` for all new components
- Props: use `PropType<T>` or define outside component
- Multi-word component names: `PascalCase.vue`
- Components in `src/components/` use `Re*` prefix (e.g., `ReDialog`, `ReTable`)

### Naming Conventions
- Files: `PascalCase` for components, `camelCase` for utilities, `kebab-case` for routes
- Variables/functions: `camelCase`
- Constants: `SCREAMING_SNAKE_CASE`
- API files: descriptive names (e.g., `dronesDevice.ts`, `system.ts`)
- Store modules: `src/store/modules/*.ts` (e.g., `user.ts`, `permission.ts`)

### TypeScript
- Avoid `any`; use `unknown` or explicit types
- Unused vars: prefix with `_` to ignore
- API response codes: `SUCCESS = 10200`, `FAIL = 10400` (from `src/api/base.ts`)

### Error Handling
- Check API responses against `SUCCESS` code
- Provide user-friendly error messages
- Use `try/catch` with typed errors

### CSS/SCSS
- TailwindCSS utility classes preferred
- Global styles in `src/style/index.scss`
- Follow stylelint config (recess-order property ordering)

## API Design

- API functions in `src/api/*.ts`
- Use service from `@/api/service` (axios instance)
- Dev proxy in vite.config.ts: `/api` → `http://192.168.41.227:8081`

```typescript
import { service } from "@/api/service";
import type { AxiosRequestConfig } from "axios";

export function getDronesList(params?: object, config?: AxiosRequestConfig) {
  return service.get("/drones/list", { params, ...config });
}
```

## Store (Pinia)

- Modules in `src/store/modules/*.ts`
- Use `defineStore` with composition API
- Types in `src/store/types.ts`
- State interface naming: `*State` (e.g., `UserState`)

## Router

- Route modules in `src/router/routes/modules/*.ts`
- Lazy loading: `component: () => import("@/views/...")`

## Backend Integration

This frontend connects to a Quarkus Java backend at the root directory (`../backend/`).
- OAuth2 token endpoint: `/api/token`
- Authorization header: `Bearer {token}`
- Backend CLAUDE.md at `../backend/CLAUDE.md` has full backend documentation

## Key Patterns

1. **Dictionary Data**: Use `getDictDetail({ dictId: "..." })` from `@/api/system`
2. **Icons**: Use `src/components/ReIcon/` for icon components
3. **Layout**: Multiple layouts supported, configured per route
4. **Permissions**: Use `usePermission()` hook for auth checks

## Important Notes

- Most utilities/hooks integrated into [@pureadmin/utils](https://pure-admin-utils.netlify.app/) since v3.3.0
- Use `@/` alias for `src/` in imports
- Git hooks run `pnpm lint` before commit (prettier → eslint → stylelint)
- Test framework not currently configured
