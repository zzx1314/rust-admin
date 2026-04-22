# AGENTS.md

Guidelines for agentic coding assistants working in this repository.

## Project Overview

Vue 3 + TypeScript + Vite admin panel for a drone management platform. Based on vue-pure-admin-max template. Uses Element Plus, Pinia, TailwindCSS, and pnpm 9+.

## Commands

### Development & Build

```bash
pnpm dev              # Dev server (4GB memory, port from .env)
pnpm build            # Production build (8GB memory)
pnpm build:staging    # Staging build
pnpm preview          # Preview production build
pnpm preview:build    # Build then preview
```

### Linting & Type Checking

```bash
pnpm typecheck        # TypeScript + Vue-TSC type checks (noEmit)
pnpm lint:eslint      # Lint Vue/JS/TS files (auto-fix)
pnpm lint:prettier    # Format code (auto-fix)
pnpm lint:stylelint   # Lint CSS/SCSS (auto-fix)
pnpm lint             # Run all linters sequentially
```

### Utilities

```bash
pnpm svgo             # Optimize SVG files
pnpm clean:cache      # Clear caches and reinstall
```

**Note**: No test framework currently configured. Always run `pnpm lint` before committing.

## Code Style

### Imports

- Use `import type` for type-only imports
- Group: external libs → internal imports → styles
- Omit file extensions (`.ts`, `.tsx`, `.vue`) — tsconfig uses `moduleResolution: "bundler"`
- Use `@/` alias for `src/` in imports

```typescript
import { defineStore } from "pinia";
import type { UserState } from "./types";
import { useUserApi } from "@/api/user";
import "./styles/index.scss";
```

### Formatting (Prettier)

- `bracketSpacing: true`, `singleQuote: false` (double quotes)
- `trailingComma: "none"`, `arrowParens: "avoid"`

### TypeScript

- Prefer explicit types for props/returns; avoid `any` (use `unknown`)
- Unused vars: prefix with `_` to ignore (`argsIgnorePattern: "^_"`)
- Use `const` for enum members: `enum Foo { A = "a" }`
- API response codes: `SUCCESS = 10200`, `FAIL = 10400` (see `src/api/base.ts`)
- Enforced: `consistent-type-imports` with `inline-type-imports`

### Vue Components

- Use `<script setup lang="ts">` for all new components
- Props: use `PropType<T>` or define outside component
- Multi-word component names: `PascalCase.vue`
- Place in `src/components/` with `Re*` prefix (e.g., `ReDialog`, `ReTable`)
- Icons: use `src/components/ReIcon/`

### Naming Conventions

- Files: `PascalCase` (components), `camelCase` (utilities), `kebab-case` (routes)
- Variables/functions: `camelCase`
- Constants: `SCREAMING_SNAKE_CASE`
- CSS: Tailwind utility classes preferred

### CSS/SCSS

- TailwindCSS utility classes preferred
- Global styles: `src/style/index.scss`, theme: `theme.scss`, dark: `dark.scss`
- Follow stylelint config with recess-order property ordering

## Project Structure

```
src/
├── api/              # API functions (service.ts, base.ts)
├── assets/           # Static assets
├── components/       # Reusable components (Re* prefix)
├── config/           # App configuration
├── directives/       # Vue directives
├── hooks/            # Composable hooks (use* pattern)
├── layout/           # Layout components
├── router/           # Vue Router config (routes/modules/)
├── store/            # Pinia stores (modules/)
├── style/            # Global styles
├── utils/            # Utility functions
└── views/            # Page components
```

## API Design

- API functions in `src/api/*.ts`
- Use axios instance from `@/api/service`
- Dev proxy: `/api` → `http://127.0.0.1:3000` (vite.config.ts)

```typescript
import { service } from "@/api/service";
import type { AxiosRequestConfig } from "axios";

export function getDronesList(params?: object, config?: AxiosRequestConfig) {
  return service.get("/drones/list", { params, ...config });
}
```

## Store (Pinia)

- Modules in `src/store/modules/*.ts`
- Use `defineStore` with composition API pattern
- Types in `src/store/types.ts`, state interface: `*State` (e.g., `UserState`)

## Router

- Route modules in `src/router/routes/modules/*.ts`
- Lazy loading: `component: () => import("@/views/...")`

## Key Patterns

1. **Dictionary Data**: `getDictDetail({ dictId: "..." })` from `@/api/system`
2. **Permissions**: `usePermission()` hook for auth checks
3. **Utilities**: Most integrated into `@pureadmin/utils` since v3.3.0
4. **OAuth2**: Token at `/api/token`, Bearer token in Authorization header

## Git Hooks

Husky + lint-staged run on commit:

- `.vue`: prettier → eslint → stylelint
- `.{js,ts,jsx,tsx}`: prettier → eslint
- `.{css,scss,html}`: prettier → stylelint

## Tech Stack

- **Node**: `^18.18.0 || ^20.9.0 || >=22.0.0` (`.nvmrc`: v22.15.1)
- **Package Manager**: pnpm 9+
- **Framework**: Vue 3.5+ with Composition API
- **Build**: Vite 6
- **UI**: Element Plus + TailwindCSS 4
- **State**: Pinia 3
- **Router**: Vue Router 4
- **HTTP**: Axios
- **Utils**: VueUse, @pureadmin/utils

## Development Workflow

After making code changes:

1. Run `lsp_diagnostics` on changed files
2. Run `pnpm lint` to catch issues
3. Run `pnpm typecheck` if touching TypeScript
4. Commit using `skill(commit-confirm)` when available

### Quality Rules

- Never suppress type errors (`as any`, `@ts-ignore`)
- Fix root causes, not symptoms
- Follow existing codebase patterns
- After 3 consecutive failures: STOP → REVERT → consult Oracle → ask user
