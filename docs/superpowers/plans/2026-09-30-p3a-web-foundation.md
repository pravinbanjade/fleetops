# FleetOps P3a — Web UI Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the React web UI that the server embeds: a TypeScript client generated from the committed OpenAPI document (with a drift check), the tab's websocket with reconnect and resubscribe, Docker events applied live to the query cache, and the first pages (setup, login, the environment shell, the dashboard and the account page).

**Architecture:**
- `web/` is a Vite + React 19 + TypeScript app. `pnpm build` writes `web/dist`, which `fleetops-server` already embeds with rust-embed and serves with an SPA fallback (P1). No server changes are needed beyond a comment and Makefile targets.
- `pnpm gen:api` runs `@hey-api/openapi-ts` on `crates/fleetops-api/openapi.json` and writes `web/src/api/gen`. The generated code is committed. `pnpm check:api` regenerates it and fails when git sees a difference.
- `src/api/client.ts` configures the generated client once:
  - every SDK call throws an `ApiError` carrying the server's `ErrorBody`;
  - the CSRF header is sent on exactly the operations whose security lists the `csrf` scheme;
  - a 401 on anything but the session probes reports a lost session.
- `src/live/ws.ts` is one websocket per tab. It carries numbered subscriptions, queues them until the socket opens, and after an unexpected close reconnects with backoff and sends every active subscription again.
- `src/live/events.ts` maps each Docker event to the query keys it makes stale and invalidates them in 100 ms batches. It also keeps a recent-events feed in the cache. Healthcheck `exec_*` noise is dropped.
- Routing is TanStack Router with code-based routes. Guards in `beforeLoad` send visitors to `/setup` while no user exists, and to `/login?redirect=…` when there is no session. Environment pages live under `/envs/$env`.

**Tech Stack:**
- Node ≥24, and pnpm 12.8.1 via corepack.
- React 19.3, TypeScript ~6.0.3, Vite 8, and Vitest 5 with jsdom and Testing Library.
- TanStack Router 1.170 and TanStack Query 5.104.
- Tailwind 4 with shadcn/ui components (new-york style, vendored), plus radix-ui, sonner and lucide-react.
- @hey-api/openapi-ts 0.99, and ESLint 10 with typescript-eslint 8.71.

**Spec:** `docs/superpowers/specs/2026-09-29-fleetops-core-docker-design.md`:
- §2.1: `web/`, with `src/api/` generated from the OpenAPI spec.
- §2.4: the frontend stack.
- §4.1: first-run setup, login, sessions, CSRF.
- §4.2: the dashboard; theme and toasts under Global UX.
- §4.3: live lists from events, with no polling.
- §5.2: the websocket protocol and client resilience.
- §6: the UI shows Docker's message, plus the request id for `Internal` errors.
- §7: strict CSP with self-only scripts.
- §9: `make dev`, `make build`, and `pnpm gen:api` with a drift check.

**Plan series for sub-project 1:**
- P1, P2a, P2b and P2c are done: the backend and the OpenAPI document.
- **P3a (this plan):** web foundation: scaffold, generated client, live layer, auth pages, shell and dashboard.
- P3b: resource pages, including the containers list, detail and create form, images, volumes, networks, compose projects and admin pages; also the command palette and route code-splitting.
- P3c: the log viewer, terminal and stats charts.
- P3d: Playwright e2e tests, `make image` and CI.

## Global Constraints

- **Toolchain:** Node ≥24. Get pnpm 12.8.1 with `corepack enable pnpm`; `package.json` pins it through `packageManager`. Install with `pnpm install --frozen-lockfile` once `pnpm-lock.yaml` is committed.
- **TypeScript stays on `~6.0.3`:** typescript-eslint 8.71 supports `>=4.8.4 <6.1.0`, so TypeScript 7 would break lint.
- **The generated client is never edited by hand:**
  - `web/src/api/gen/**` is produced only by `pnpm gen:api` and is committed.
  - `make openapi` regenerates the server document and then the client.
  - `pnpm check:api` must pass on a clean tree.
- **Operation names:** SDK functions are the document's operationIds (`getMe`, `listEnvs`, …); P2c made them stable.
- **CSP:** the server sends `script-src 'self'`, so there are no inline scripts in `index.html`. Theme handling runs in the bundle. Inline styles are allowed (`style-src 'unsafe-inline'`), which sonner and Radix need.
- **Websocket Origin:** the server rejects a websocket whose Origin does not match its Host. The Vite proxy for `/api` uses `ws: true` without `changeOrigin`, so the browser's Host header reaches the server unchanged.
- **Routes:**
  - `/setup` and `/login`.
  - `/` redirects to the first environment.
  - `/envs/$env` is the dashboard, and later pages go below it.
  - `/account` is the account page.
  - Route ids used by `getRouteApi` are `/login`, `/setup` and `/authed/shell/envs/$env/`.
- **Query keys** (`src/api/queries.ts` `keys`): everything environment-scoped starts with `["env", env]`, and a detail key extends its list key. P3b and P3c add their queries under these keys.
- **Errors:**
  - Pages show failures from `errorMessage(err)` inline (`role="alert"`) or as a toast.
  - Internal errors include `(request <id>)`.
  - A mutation without its own `onError` (and without `meta.inline`) gets a toast.
- **Spec deltas (intentional):**
  - `make dev` runs `cargo run`, not `cargo watch`, since cargo-watch is not a project dependency. Restart the server by hand after Rust changes.
  - shadcn/ui components are vendored as source in `src/components/ui`, not added with the shadcn CLI. The CLI mis-resolved the `cn` import and skipped the theme tokens.
  - Events invalidate the affected queries in batches. They do not patch cached lists in place: a refetch per burst is cheap on a local socket, and patching can come later if a host is large enough to need it.
  - The spec's shell auto-detect, ANSI logs, TanStack Table/Virtual, xterm.js, CodeMirror and uPlot belong to P3b and P3c.

## Review Focus

1. **The session ends while the app is open.** This happens when it is revoked from another tab, or expires. The next request must send the user to `/login?redirect=<where they were>`, not leave a broken page. Pinned in Task 1 (`reports a lost session, but not a 401 from the session probe`) and Task 6 (`sends the user to login when the session is lost mid-use`).
2. **`?redirect=` pointing at another site.** After login the app must only follow local paths (`/x`), never `//host` or `https://…`. Pinned in Task 5 (`safeRedirect keeps local paths only`, `ignores a redirect to another site`).
3. **The server restarts or the network drops.**
   - The "reconnecting" banner must stay up until a socket opens again, including across failed attempts.
   - Every subscription is sent again, and environment queries refetch, since events were missed.
   - Pinned in Task 2 (`reconnects with backoff and resubscribes…`, `reports reconnecting from a drop until a socket opens again`) and Task 6 (`warns while the live connection is down and refetches after it returns`).
4. **Event bursts and healthcheck noise.** A compose project starting emits dozens of events, and healthchecks emit `exec_*` events every few seconds per container. The first must cause one refetch per query, and the second none. The noise must also stay out of the dashboard feed. Pinned in Task 3 (`invalidates each affected query once per batch`, `exec and attach events refresh nothing`, `leaves exec noise out of the feed`).
5. **Engines that differ, or an environment that is down.**
   - Docker 29 sends `system df` as `*Usage` totals, and older engines send item lists. Both must render.
   - A down environment must say why instead of showing empty cards.
   - Pinned in Task 4 (`summarizeDiskUsage`) and Task 7 (`shows why an environment is down`, `follows env_status frames from the server`).

---

## File Structure

```
Makefile                                   (modify) dev, dev-web, web, test-web; build depends on web; openapi regenerates the client
crates/fleetops-server/build.rs            (modify) comment only
web/
  package.json, pnpm-lock.yaml             scripts: dev build typecheck lint test gen:api check:api
  tsconfig.json, vite.config.ts            strict TS, @/ alias, /api proxy (ws), vitest jsdom
  eslint.config.js, openapi-ts.config.ts
  index.html, public/favicon.svg
  src/
    main.tsx                               mounts <App/>
    App.tsx                                QueryClient (toasts for unhandled mutation errors), router, theme, toaster
    router.tsx                             routes and guards; safeRedirect
    styles.css                             Tailwind 4 + shadcn theme tokens (light/dark)
    theme.tsx                              light/dark/system, persisted
    api/gen/**                             GENERATED by `pnpm gen:api` (committed)
    api/client.ts                          ApiError, setCsrfToken, configureApi
    api/queries.ts                         query keys and queryOptions
    live/ws.ts                             WsClient: subscriptions, reconnect, resubscribe, binary exec frames
    live/events.ts                         keysForEvent, isNoise, EventApplier
    live/LiveProvider.tsx                  the tab's WsClient; useSubscription, useEnvEvents
    auth/session.ts                        useSession, useLogout
    components/Shell.tsx                   sidebar: env switcher + status, nav, user menu; reconnect banner
    components/ui/*.tsx                    vendored shadcn/ui: badge button card dropdown-menu input label
    lib/{utils,errors,format,df}.ts        cn; errorMessage/toastError; formatBytes/formatTime; summarizeDiskUsage
    pages/{Setup,Login,Account,Dashboard}Page.tsx, AuthLayout.tsx, misc.tsx
    test/{setup.ts,fetch.ts,fake-ws.ts,app.tsx}   test helpers
    **/*.test.ts(x)                        unit and page tests (Vitest)
```

---

### Task 1: Scaffold, the generated client, and `configureApi`

**Files:**
- Create: `web/package.json`, `web/tsconfig.json`, `web/vite.config.ts`, `web/eslint.config.js`, `web/openapi-ts.config.ts`, `web/index.html`, `web/public/favicon.svg`.
- Create: `web/src/main.tsx` (placeholder page, replaced in Task 5), `web/src/styles.css`, `web/src/lib/utils.ts` and `web/src/components/ui/{badge,button,card,dropdown-menu,input,label}.tsx`.
- Create: `web/src/api/client.ts`.
- Generated: `web/src/api/gen/**` and `web/pnpm-lock.yaml`.
- Test helpers: `web/src/test/setup.ts` (replaced in Task 5) and `web/src/test/fetch.ts`.
- Test: `web/src/api/client.test.ts`.
- Modify: `Makefile`, `crates/fleetops-server/build.rs`.

**Interfaces:**
- Produces `@/api/client`:
  - `class ApiError extends Error { status: number; code: string; requestId: string | null }`
  - `setCsrfToken(token: string | undefined): void`
  - `configureApi(opts?: { onUnauthorized?: () => void }): void`
- Produces the generated SDK at `@/api/gen/sdk.gen`: one function per operationId, which throws on error (`throwOnError: true`) and returns `{ data, request, response }`.
- Produces the generated types at `@/api/gen/types.gen`, e.g. `SessionInfo`, `EnvironmentRecord`, `DockerEvent`, `ServerMsg`, `ClientMsg`, `Topic`, `EnvStatus` and `ErrorBody`.
- Produces `@/test/fetch`:
  - `mockFetch(routes: Record<"METHOD /path", Handler | {status?, json?}>): Call[]`, where `Call` is `{ method, path, headers, body }`;
  - `json(status, body): Response`.
- Produces `@/lib/utils` `cn(...classes)`, and the ui components' named exports as vendored.

- [ ] **Step 1: Enable pnpm**

Run: `corepack enable pnpm && pnpm --version`
Expected: `12.8.1` (corepack downloads it on first use).

- [ ] **Step 2: Write the project files**

`web/package.json`:

```json
{
  "name": "fleetops-web",
  "private": true,
  "type": "module",
  "packageManager": "pnpm@12.8.1",
  "engines": {
    "node": ">=24"
  },
  "scripts": {
    "dev": "vite",
    "build": "tsc --noEmit && vite build",
    "typecheck": "tsc --noEmit",
    "lint": "eslint .",
    "test": "vitest run",
    "gen:api": "openapi-ts",
    "check:api": "openapi-ts && git diff --exit-code -- src/api/gen && test -z \"$(git status --porcelain -- src/api/gen)\""
  },
  "dependencies": {
    "@tanstack/react-query": "^5.104.0",
    "@tanstack/react-router": "^1.170.40",
    "class-variance-authority": "^0.7.1",
    "clsx": "^2.1.1",
    "cn": "^0.4.0",
    "lucide-react": "^1.48.0",
    "radix-ui": "^1.6.7",
    "react": "^19.3.0",
    "react-dom": "^19.3.0",
    "sonner": "^2.0.8",
    "tailwind-merge": "^3.7.0"
  },
  "devDependencies": {
    "@eslint/js": "^10.0.1",
    "@hey-api/openapi-ts": "^0.99.0",
    "@tailwindcss/vite": "^4.3.3",
    "@testing-library/dom": "^10.4.2",
    "@testing-library/jest-dom": "^7.0.1",
    "@testing-library/react": "^16.3.3",
    "@testing-library/user-event": "^14.6.7",
    "@types/node": "^24.19.0",
    "@types/react": "^19.3.0",
    "@types/react-dom": "^19.3.0",
    "@vitejs/plugin-react": "^6.1.1",
    "eslint": "^10.11.0",
    "eslint-plugin-react-hooks": "^7.1.1",
    "globals": "^17.12.0",
    "jsdom": "^30.1.1",
    "tailwindcss": "^4.3.3",
    "tw-animate-css": "^1.4.0",
    "typescript": "~6.0.3",
    "typescript-eslint": "^8.71.0",
    "vite": "^8.3.1",
    "vitest": "^5.0.2"
  }
}
```

`web/tsconfig.json`:

```json
{
  "compilerOptions": {
    "target": "ES2023",
    "lib": ["ES2023", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "moduleResolution": "bundler",
    "jsx": "react-jsx",
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true,
    "noUncheckedSideEffectImports": true,
    "verbatimModuleSyntax": true,
    "skipLibCheck": true,
    "noEmit": true,
    "types": ["vite/client", "vitest/globals", "@testing-library/jest-dom"],
    "paths": { "@/*": ["./src/*"] }
  },
  "include": ["src", "vite.config.ts", "openapi-ts.config.ts"]
}
```

`web/vite.config.ts`:

```ts
/// <reference types="vitest/config" />
import path from "node:path";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

// The dev server proxies the API (and its websocket) to `make dev-server`.
// The Host header is left as the browser sent it, so the server's websocket
// Origin check sees matching Origin and Host.
const api = process.env.FLEETOPS_API ?? "http://127.0.0.1:9000";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.resolve(import.meta.dirname, "src") } },
  server: {
    port: 5173,
    proxy: { "/api": { target: api, ws: true } },
  },
  // One bundle until the resource pages land (P3b splits them by route).
  build: { outDir: "dist", emptyOutDir: true, chunkSizeWarningLimit: 1024 },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["src/test/setup.ts"],
    restoreMocks: true,
  },
});
```

`web/eslint.config.js`:

```js
import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "src/api/gen"] },
  {
    files: ["**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended, reactHooks.configs.flat.recommended],
    languageOptions: { globals: globals.browser },
  },
);
```

`web/openapi-ts.config.ts`:

```ts
import { defineConfig } from "@hey-api/openapi-ts";

// `pnpm gen:api` regenerates src/api/gen from the committed server document;
// `pnpm check:api` fails when the committed client has drifted from it.
export default defineConfig({
  input: "../crates/fleetops-api/openapi.json",
  output: "src/api/gen",
  plugins: [{ name: "@hey-api/client-fetch", throwOnError: true }, "@hey-api/typescript", "@hey-api/sdk"],
});
```

`web/index.html`:

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <link rel="icon" type="image/svg+xml" href="/favicon.svg" />
    <title>FleetOps</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

`web/public/favicon.svg`:

```xml
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" rx="7" fill="#171717"/><path d="M8 10h16v3H11.5v3H21v3h-9.5v5H8z" fill="#fafafa"/></svg>
```

`web/src/test/setup.ts`:

```ts
import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

afterEach(() => {
  cleanup();
  localStorage.clear();
});
```

- [ ] **Step 3: Install and generate the client**

Run: `cd web && pnpm install && pnpm gen:api && ls src/api/gen`
Expected:
- `pnpm install` writes `pnpm-lock.yaml`.
- `openapi-ts` reports `✓ ./src/api/gen · 4 files`.
- The listing shows `client client.gen.ts core index.ts sdk.gen.ts types.gen.ts`.
- `grep -c 'ThrowOnError extends boolean = true' src/api/gen/sdk.gen.ts` prints 58.

- [ ] **Step 4: Write the failing test**

`web/src/test/fetch.ts`:

```ts
import { vi } from "vitest";

export interface Call {
  method: string;
  path: string;
  headers: Headers;
  body: unknown;
}

type Handler = (call: Call) => Response | { status?: number; json?: unknown } | undefined;

/**
 * Stubs `fetch` with `routes`, keyed "METHOD /path". A handler returns a
 * Response or `{status, json}`; unmatched requests get a 404 ErrorBody.
 * Returns the list of calls made, in order.
 */
export function mockFetch(routes: Record<string, Handler | { status?: number; json?: unknown }>) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url);
      const text = await input.text();
      const call: Call = {
        method: input.method,
        path: url.pathname + url.search,
        headers: input.headers,
        body: text ? JSON.parse(text) : undefined,
      };
      calls.push(call);
      const route = routes[`${call.method} ${url.pathname}`];
      const out = typeof route === "function" ? route(call) : route;
      if (out instanceof Response) return out;
      if (!out) return json(404, { code: "not_found", message: `no mock for ${call.method} ${url.pathname}` });
      return out.json === undefined ? new Response(null, { status: out.status ?? 204 }) : json(out.status ?? 200, out.json);
    }),
  );
  return calls;
}

export function json(status: number, body: unknown) {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}
```

`web/src/api/client.test.ts`:

```ts
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError, configureApi, setCsrfToken } from "./client";
import { getMe, listEnvs, logout } from "./gen/sdk.gen";
import { mockFetch } from "@/test/fetch";

describe("api client", () => {
  beforeEach(() => {
    setCsrfToken(undefined);
    configureApi();
  });

  it("sends the CSRF token on mutations only", async () => {
    const calls = mockFetch({ "GET /api/v1/envs": { json: [] }, "POST /api/v1/auth/logout": { status: 204 } });
    setCsrfToken("tok");
    await listEnvs();
    await logout();
    expect(calls[0].headers.get("x-csrf-token")).toBeNull();
    expect(calls[1].headers.get("x-csrf-token")).toBe("tok");
  });

  it("throws ApiError with the server's error body", async () => {
    mockFetch({
      "GET /api/v1/envs": { status: 503, json: { code: "env_unavailable", message: "docker is down", request_id: "r1" } },
    });
    const err = await listEnvs().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err).toMatchObject({ status: 503, code: "env_unavailable", message: "docker is down", requestId: "r1" });
  });

  it("throws ApiError for a non-JSON error response", async () => {
    mockFetch({ "GET /api/v1/envs": () => new Response("bad gateway", { status: 502 }) });
    const err = await listEnvs().catch((e: unknown) => e);
    expect(err).toMatchObject({ status: 502, code: "internal" });
  });

  it("passes network errors through unchanged", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => Promise.reject(new TypeError("Failed to fetch"))));
    const err = await listEnvs().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(TypeError);
  });

  it("reports a lost session, but not a 401 from the session probe", async () => {
    const onUnauthorized = vi.fn();
    configureApi({ onUnauthorized });
    const lost = { status: 401, json: { code: "unauthorized", message: "no session" } };
    mockFetch({ "GET /api/v1/auth/me": lost, "GET /api/v1/envs": lost });
    await getMe().catch(() => {});
    expect(onUnauthorized).not.toHaveBeenCalled();
    await listEnvs().catch(() => {});
    expect(onUnauthorized).toHaveBeenCalledOnce();
  });
});
```

- [ ] **Step 5: Run it to verify it fails**

Run: `cd web && pnpm test`
Expected: FAIL with `Failed to resolve import "./client" from "src/api/client.test.ts"`.

- [ ] **Step 6: Implement the client configuration**

`web/src/api/client.ts`:

```ts
import { client } from "./gen/client.gen";
import type { ErrorBody } from "./gen/types.gen";

/** An error response from the API, carrying the server's `ErrorBody`. */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly requestId: string | null;

  constructor(status: number, body: Partial<ErrorBody> | undefined) {
    super(body?.message || `request failed with status ${status}`);
    this.name = "ApiError";
    this.status = status;
    this.code = body?.code ?? "internal";
    this.requestId = body?.request_id ?? null;
  }
}

let csrfToken: string | undefined;

/** Stores the session's CSRF token; every mutating operation sends it. */
export function setCsrfToken(token: string | undefined) {
  csrfToken = token;
}

export interface ApiOptions {
  /** Called when a request other than the session probes gets a 401. */
  onUnauthorized?: () => void;
}

// Requests whose 401 is an answer, not a lost session.
const PROBES = ["/api/v1/auth/me", "/api/v1/auth/login", "/api/v1/auth/setup"];

/**
 * Points the generated client at this origin, makes every SDK call throw
 * an `ApiError` on failure, and supplies the CSRF header for the
 * operations whose security lists the `csrf` scheme.
 */
export function configureApi(opts: ApiOptions = {}) {
  client.setConfig({
    baseUrl: window.location.origin,
    credentials: "same-origin",
    throwOnError: true,
    auth: (scheme) => (scheme.name === "x-csrf-token" ? csrfToken : undefined),
  });
  client.interceptors.error.clear();
  client.interceptors.error.use((error, response, request) => {
    // No response: the request never reached the server (network error).
    if (!response || !request) return error;
    const body = typeof error === "object" && error !== null ? (error as Partial<ErrorBody>) : undefined;
    const path = new URL(request.url).pathname;
    if (response.status === 401 && !PROBES.includes(path)) {
      opts.onUnauthorized?.();
    }
    return new ApiError(response.status, body);
  });
}
```

- [ ] **Step 7: Run it to verify it passes**

Run: `cd web && pnpm test`
Expected: 5 passed (1 file).

- [ ] **Step 8: Styles, the vendored ui components, and the placeholder page**

These are shadcn/ui's new-york components with their imports pointed at `@/lib/utils`. Copy them exactly; later tasks use their exported names.

`web/src/styles.css`:

```css
@import "tailwindcss";
@import "tw-animate-css";

@custom-variant dark (&:is(.dark *));

:root {
  --radius: 0.5rem;
  --background: oklch(1 0 0);
  --foreground: oklch(0.145 0 0);
  --card: oklch(1 0 0);
  --card-foreground: oklch(0.145 0 0);
  --popover: oklch(1 0 0);
  --popover-foreground: oklch(0.145 0 0);
  --primary: oklch(0.205 0 0);
  --primary-foreground: oklch(0.985 0 0);
  --secondary: oklch(0.97 0 0);
  --secondary-foreground: oklch(0.205 0 0);
  --muted: oklch(0.97 0 0);
  --muted-foreground: oklch(0.556 0 0);
  --accent: oklch(0.97 0 0);
  --accent-foreground: oklch(0.205 0 0);
  --destructive: oklch(0.577 0.245 27.325);
  --border: oklch(0.922 0 0);
  --input: oklch(0.922 0 0);
  --ring: oklch(0.708 0 0);
}

.dark {
  --background: oklch(0.145 0 0);
  --foreground: oklch(0.985 0 0);
  --card: oklch(0.205 0 0);
  --card-foreground: oklch(0.985 0 0);
  --popover: oklch(0.205 0 0);
  --popover-foreground: oklch(0.985 0 0);
  --primary: oklch(0.922 0 0);
  --primary-foreground: oklch(0.205 0 0);
  --secondary: oklch(0.269 0 0);
  --secondary-foreground: oklch(0.985 0 0);
  --muted: oklch(0.269 0 0);
  --muted-foreground: oklch(0.708 0 0);
  --accent: oklch(0.269 0 0);
  --accent-foreground: oklch(0.985 0 0);
  --destructive: oklch(0.704 0.191 22.216);
  --border: oklch(1 0 0 / 10%);
  --input: oklch(1 0 0 / 15%);
  --ring: oklch(0.556 0 0);
}

@theme inline {
  --radius-sm: calc(var(--radius) - 4px);
  --radius-md: calc(var(--radius) - 2px);
  --radius-lg: var(--radius);
  --color-background: var(--background);
  --color-foreground: var(--foreground);
  --color-card: var(--card);
  --color-card-foreground: var(--card-foreground);
  --color-popover: var(--popover);
  --color-popover-foreground: var(--popover-foreground);
  --color-primary: var(--primary);
  --color-primary-foreground: var(--primary-foreground);
  --color-secondary: var(--secondary);
  --color-secondary-foreground: var(--secondary-foreground);
  --color-muted: var(--muted);
  --color-muted-foreground: var(--muted-foreground);
  --color-accent: var(--accent);
  --color-accent-foreground: var(--accent-foreground);
  --color-destructive: var(--destructive);
  --color-border: var(--border);
  --color-input: var(--input);
  --color-ring: var(--ring);
}

@layer base {
  * {
    @apply border-border outline-ring/50;
  }
  body {
    @apply bg-background text-foreground antialiased;
  }
}
```

`web/src/lib/utils.ts`:

```ts
import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
```

`web/src/components/ui/button.tsx`:

```tsx
import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"
import { Slot } from "radix-ui"

const buttonVariants = cva(
  "inline-flex shrink-0 items-center justify-center gap-2 rounded-md text-sm font-medium whitespace-nowrap transition-all outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground hover:bg-primary/90",
        destructive:
          "bg-destructive text-white hover:bg-destructive/90 focus-visible:ring-destructive/20 dark:bg-destructive/60 dark:focus-visible:ring-destructive/40",
        outline:
          "border bg-background shadow-xs hover:bg-accent hover:text-accent-foreground dark:border-input dark:bg-input/30 dark:hover:bg-input/50",
        secondary:
          "bg-secondary text-secondary-foreground hover:bg-secondary/80",
        ghost:
          "hover:bg-accent hover:text-accent-foreground dark:hover:bg-accent/50",
        link: "text-primary underline-offset-4 hover:underline",
      },
      size: {
        default: "h-9 px-4 py-2 has-[>svg]:px-3",
        xs: "h-6 gap-1 rounded-md px-2 text-xs has-[>svg]:px-1.5 [&_svg:not([class*='size-'])]:size-3",
        sm: "h-8 gap-1.5 rounded-md px-3 has-[>svg]:px-2.5",
        lg: "h-10 rounded-md px-6 has-[>svg]:px-4",
        icon: "size-9",
        "icon-xs": "size-6 rounded-md [&_svg:not([class*='size-'])]:size-3",
        "icon-sm": "size-8",
        "icon-lg": "size-10",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Button({
  className,
  variant = "default",
  size = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
  }) {
  const Comp = asChild ? Slot.Root : "button"

  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Button, buttonVariants }
```

`web/src/components/ui/input.tsx`:

```tsx
import * as React from "react"
import { cn } from "@/lib/utils"

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "h-9 w-full min-w-0 rounded-md border border-input bg-transparent px-3 py-1 text-base shadow-xs transition-[color,box-shadow] outline-none selection:bg-primary selection:text-primary-foreground file:inline-flex file:h-7 file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 md:text-sm dark:bg-input/30",
        "focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50",
        "aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40",
        className
      )}
      {...props}
    />
  )
}

export { Input }
```

`web/src/components/ui/label.tsx`:

```tsx
import * as React from "react"
import { cn } from "@/lib/utils"
import { Label as LabelPrimitive } from "radix-ui"

function Label({
  className,
  ...props
}: React.ComponentProps<typeof LabelPrimitive.Root>) {
  return (
    <LabelPrimitive.Root
      data-slot="label"
      className={cn(
        "flex items-center gap-2 text-sm leading-none font-medium select-none group-data-[disabled=true]:pointer-events-none group-data-[disabled=true]:opacity-50 peer-disabled:cursor-not-allowed peer-disabled:opacity-50",
        className
      )}
      {...props}
    />
  )
}

export { Label }
```

`web/src/components/ui/card.tsx`:

```tsx
import * as React from "react"
import { cn } from "@/lib/utils"

function Card({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card"
      className={cn(
        "flex flex-col gap-6 rounded-xl border bg-card py-6 text-card-foreground shadow-sm",
        className
      )}
      {...props}
    />
  )
}

function CardHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-header"
      className={cn(
        "@container/card-header grid auto-rows-min grid-rows-[auto_auto] items-start gap-2 px-6 has-data-[slot=card-action]:grid-cols-[1fr_auto] [.border-b]:pb-6",
        className
      )}
      {...props}
    />
  )
}

function CardTitle({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-title"
      className={cn("leading-none font-semibold", className)}
      {...props}
    />
  )
}

function CardDescription({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-description"
      className={cn("text-sm text-muted-foreground", className)}
      {...props}
    />
  )
}

function CardAction({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-action"
      className={cn(
        "col-start-2 row-span-2 row-start-1 self-start justify-self-end",
        className
      )}
      {...props}
    />
  )
}

function CardContent({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-content"
      className={cn("px-6", className)}
      {...props}
    />
  )
}

function CardFooter({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-footer"
      className={cn("flex items-center px-6 [.border-t]:pt-6", className)}
      {...props}
    />
  )
}

export {
  Card,
  CardHeader,
  CardFooter,
  CardTitle,
  CardAction,
  CardDescription,
  CardContent,
}
```

`web/src/components/ui/badge.tsx`:

```tsx
import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "@/lib/utils"
import { Slot } from "radix-ui"

const badgeVariants = cva(
  "inline-flex w-fit shrink-0 items-center justify-center gap-1 overflow-hidden rounded-full border border-transparent px-2 py-0.5 text-xs font-medium whitespace-nowrap transition-[color,box-shadow] focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 [&>svg]:pointer-events-none [&>svg]:size-3",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground [a&]:hover:bg-primary/90",
        secondary:
          "bg-secondary text-secondary-foreground [a&]:hover:bg-secondary/90",
        destructive:
          "bg-destructive text-white focus-visible:ring-destructive/20 dark:bg-destructive/60 dark:focus-visible:ring-destructive/40 [a&]:hover:bg-destructive/90",
        outline:
          "border-border text-foreground [a&]:hover:bg-accent [a&]:hover:text-accent-foreground",
        ghost: "[a&]:hover:bg-accent [a&]:hover:text-accent-foreground",
        link: "text-primary underline-offset-4 [a&]:hover:underline",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  }
)

function Badge({
  className,
  variant = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"span"> &
  VariantProps<typeof badgeVariants> & { asChild?: boolean }) {
  const Comp = asChild ? Slot.Root : "span"

  return (
    <Comp
      data-slot="badge"
      data-variant={variant}
      className={cn(badgeVariants({ variant }), className)}
      {...props}
    />
  )
}

export { Badge, badgeVariants }
```

`web/src/components/ui/dropdown-menu.tsx`:

```tsx
"use client"

import * as React from "react"
import { cn } from "@/lib/utils"
import { CheckIcon, ChevronRightIcon, CircleIcon } from "lucide-react"
import { DropdownMenu as DropdownMenuPrimitive } from "radix-ui"

function DropdownMenu({
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Root>) {
  return <DropdownMenuPrimitive.Root data-slot="dropdown-menu" {...props} />
}

function DropdownMenuPortal({
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Portal>) {
  return (
    <DropdownMenuPrimitive.Portal data-slot="dropdown-menu-portal" {...props} />
  )
}

function DropdownMenuTrigger({
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Trigger>) {
  return (
    <DropdownMenuPrimitive.Trigger
      data-slot="dropdown-menu-trigger"
      {...props}
    />
  )
}

function DropdownMenuContent({
  className,
  sideOffset = 4,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Content>) {
  return (
    <DropdownMenuPrimitive.Portal>
      <DropdownMenuPrimitive.Content
        data-slot="dropdown-menu-content"
        sideOffset={sideOffset}
        className={cn(
          "z-50 max-h-(--radix-dropdown-menu-content-available-height) min-w-[8rem] origin-(--radix-dropdown-menu-content-transform-origin) overflow-x-hidden overflow-y-auto rounded-md border bg-popover p-1 text-popover-foreground shadow-md data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95",
          className
        )}
        {...props}
      />
    </DropdownMenuPrimitive.Portal>
  )
}

function DropdownMenuGroup({
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Group>) {
  return (
    <DropdownMenuPrimitive.Group data-slot="dropdown-menu-group" {...props} />
  )
}

function DropdownMenuItem({
  className,
  inset,
  variant = "default",
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Item> & {
  inset?: boolean
  variant?: "default" | "destructive"
}) {
  return (
    <DropdownMenuPrimitive.Item
      data-slot="dropdown-menu-item"
      data-inset={inset}
      data-variant={variant}
      className={cn(
        "relative flex cursor-default items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-hidden select-none focus:bg-accent focus:text-accent-foreground data-[disabled]:pointer-events-none data-[disabled]:opacity-50 data-[inset]:pl-8 data-[variant=destructive]:text-destructive data-[variant=destructive]:focus:bg-destructive/10 data-[variant=destructive]:focus:text-destructive dark:data-[variant=destructive]:focus:bg-destructive/20 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4 [&_svg:not([class*='text-'])]:text-muted-foreground data-[variant=destructive]:*:[svg]:text-destructive!",
        className
      )}
      {...props}
    />
  )
}

function DropdownMenuCheckboxItem({
  className,
  children,
  checked,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.CheckboxItem>) {
  return (
    <DropdownMenuPrimitive.CheckboxItem
      data-slot="dropdown-menu-checkbox-item"
      className={cn(
        "relative flex cursor-default items-center gap-2 rounded-sm py-1.5 pr-2 pl-8 text-sm outline-hidden select-none focus:bg-accent focus:text-accent-foreground data-[disabled]:pointer-events-none data-[disabled]:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
        className
      )}
      checked={checked}
      {...props}
    >
      <span className="pointer-events-none absolute left-2 flex size-3.5 items-center justify-center">
        <DropdownMenuPrimitive.ItemIndicator>
          <CheckIcon className="size-4" />
        </DropdownMenuPrimitive.ItemIndicator>
      </span>
      {children}
    </DropdownMenuPrimitive.CheckboxItem>
  )
}

function DropdownMenuRadioGroup({
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.RadioGroup>) {
  return (
    <DropdownMenuPrimitive.RadioGroup
      data-slot="dropdown-menu-radio-group"
      {...props}
    />
  )
}

function DropdownMenuRadioItem({
  className,
  children,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.RadioItem>) {
  return (
    <DropdownMenuPrimitive.RadioItem
      data-slot="dropdown-menu-radio-item"
      className={cn(
        "relative flex cursor-default items-center gap-2 rounded-sm py-1.5 pr-2 pl-8 text-sm outline-hidden select-none focus:bg-accent focus:text-accent-foreground data-[disabled]:pointer-events-none data-[disabled]:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
        className
      )}
      {...props}
    >
      <span className="pointer-events-none absolute left-2 flex size-3.5 items-center justify-center">
        <DropdownMenuPrimitive.ItemIndicator>
          <CircleIcon className="size-2 fill-current" />
        </DropdownMenuPrimitive.ItemIndicator>
      </span>
      {children}
    </DropdownMenuPrimitive.RadioItem>
  )
}

function DropdownMenuLabel({
  className,
  inset,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Label> & {
  inset?: boolean
}) {
  return (
    <DropdownMenuPrimitive.Label
      data-slot="dropdown-menu-label"
      data-inset={inset}
      className={cn(
        "px-2 py-1.5 text-sm font-medium data-[inset]:pl-8",
        className
      )}
      {...props}
    />
  )
}

function DropdownMenuSeparator({
  className,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Separator>) {
  return (
    <DropdownMenuPrimitive.Separator
      data-slot="dropdown-menu-separator"
      className={cn("-mx-1 my-1 h-px bg-border", className)}
      {...props}
    />
  )
}

function DropdownMenuShortcut({
  className,
  ...props
}: React.ComponentProps<"span">) {
  return (
    <span
      data-slot="dropdown-menu-shortcut"
      className={cn(
        "ml-auto text-xs tracking-widest text-muted-foreground",
        className
      )}
      {...props}
    />
  )
}

function DropdownMenuSub({
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Sub>) {
  return <DropdownMenuPrimitive.Sub data-slot="dropdown-menu-sub" {...props} />
}

function DropdownMenuSubTrigger({
  className,
  inset,
  children,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.SubTrigger> & {
  inset?: boolean
}) {
  return (
    <DropdownMenuPrimitive.SubTrigger
      data-slot="dropdown-menu-sub-trigger"
      data-inset={inset}
      className={cn(
        "flex cursor-default items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-hidden select-none focus:bg-accent focus:text-accent-foreground data-[inset]:pl-8 data-[state=open]:bg-accent data-[state=open]:text-accent-foreground [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4 [&_svg:not([class*='text-'])]:text-muted-foreground",
        className
      )}
      {...props}
    >
      {children}
      <ChevronRightIcon className="ml-auto size-4" />
    </DropdownMenuPrimitive.SubTrigger>
  )
}

function DropdownMenuSubContent({
  className,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.SubContent>) {
  return (
    <DropdownMenuPrimitive.SubContent
      data-slot="dropdown-menu-sub-content"
      className={cn(
        "z-50 min-w-[8rem] origin-(--radix-dropdown-menu-content-transform-origin) overflow-hidden rounded-md border bg-popover p-1 text-popover-foreground shadow-lg data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95",
        className
      )}
      {...props}
    />
  )
}

export {
  DropdownMenu,
  DropdownMenuPortal,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuLabel,
  DropdownMenuItem,
  DropdownMenuCheckboxItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuSub,
  DropdownMenuSubTrigger,
  DropdownMenuSubContent,
}
```

`web/src/main.tsx`:

```tsx
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";

// The routed app replaces this page in Task 5.
createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <p className="p-6">FleetOps</p>
  </StrictMode>,
);
```

- [ ] **Step 9: Makefile targets and the build script comment**

```diff
--- a/Makefile
+++ b/Makefile
@@ -1,4 +1,12 @@
-.PHONY: dev-server test test-docker lint fmt build openapi
+.PHONY: dev dev-server dev-web web test test-docker test-web lint fmt build openapi
+
+# The API on :9000 and the Vite dev server on :5173, which proxies /api to it.
+dev:
+	$(MAKE) -j2 dev-server dev-web
+
+dev-web:
+	pnpm -C web install --frozen-lockfile
+	pnpm -C web dev
 
 dev-server:
 	DOCKER_HOST=$${DOCKER_HOST:-$$(docker context inspect --format '{{.Endpoints.docker.Host}}' 2>/dev/null)} \
@@ -7,6 +15,15 @@ dev-server:
 test:
 	cargo test --workspace
 
+# Typecheck, lint and unit tests for the web UI, and a check that the
+# committed API client matches the committed OpenAPI document.
+test-web:
+	pnpm -C web install --frozen-lockfile
+	pnpm -C web typecheck
+	pnpm -C web lint
+	pnpm -C web test
+	pnpm -C web check:api
+
 # Integration tests against the Docker daemon of the current docker context.
 test-docker:
 	DOCKER_HOST=$$(docker context inspect --format '{{.Endpoints.docker.Host}}') \
@@ -19,10 +36,16 @@ lint:
 fmt:
 	cargo fmt --all
 
-build:
+# Builds the web UI into web/dist, which the server binary embeds.
+web:
+	pnpm -C web install --frozen-lockfile
+	pnpm -C web build
+
+build: web
 	cargo build --release -p fleetops-server
 
-# Regenerates the committed OpenAPI document after an API change;
-# `cargo test` fails until it matches the code.
+# Regenerates the committed OpenAPI document after an API change, and the
+# web client from it; `cargo test` and `make test-web` fail until both match.
 openapi:
 	cargo run -q -p fleetops-server -- --openapi > crates/fleetops-api/openapi.json
+	pnpm -C web gen:api
```

```diff
--- a/crates/fleetops-server/build.rs
+++ b/crates/fleetops-server/build.rs
@@ -1,5 +1,6 @@
-//! rust-embed needs web/dist to exist at compile time. Until the web UI is
-//! built (plan P3), write a placeholder page so the server still compiles.
+//! rust-embed needs web/dist to exist at compile time. When the web UI has
+//! not been built (`make web`), write a placeholder page so the server
+//! still compiles.
 use std::{fs, path::PathBuf};
 
 const PLACEHOLDER: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>FleetOps</title></head>\
```

- [ ] **Step 10: Check the whole web project and the embedded build**

Run: `cd web && pnpm typecheck && pnpm lint && pnpm build && grep -c '<script>' dist/index.html; cd .. && cargo build -p fleetops-server`
Expected:
- typecheck and lint are clean.
- Vite prints `✓ built`.
- `grep -c` prints `0`: there are no inline scripts, as the CSP requires.
- The server compiles with the real `web/dist` embedded.

- [ ] **Step 11: Commit, then run the drift check on the clean tree**

```bash
git add Makefile crates/fleetops-server/build.rs web
git commit -m "feat(web): scaffold the web UI and the client generated from the OpenAPI document"
make test-web
```

Expected: `make test-web` passes. Its last step, `check:api`, regenerates the client and finds no difference.

---

### Task 2: The websocket client

**Files:**
- Create: `web/src/live/ws.ts`.
- Test helper: `web/src/test/fake-ws.ts`.
- Test: `web/src/live/ws.test.ts`.

**Interfaces:**
- Consumes: `ClientMsg`, `ServerMsg`, `EnvStatus` and `Topic` from `@/api/gen/types.gen`.
- Produces `@/live/ws`:
  - `interface SubSpec { topic: Topic; env?: string; id?: string; opts?: unknown }`
  - `interface SubHandlers { onData(data: unknown); onBinary?(bytes: Uint8Array); onGap?(dropped: number); onEnd?(msg); onError?(msg); onResubscribe?() }`
  - `type ConnState = "connecting" | "open" | "reconnecting"`
  - `interface WsOptions { url; WebSocketImpl?; backoff?(attempt): ms; onState?(state); onEnvStatus?(env, status); onReconnect?() }`
  - `class WsClient`, with the methods:
    - `constructor(opts)`
    - `start()`
    - `close()`
    - `subscribe(spec, handlers): { sub: number; unsubscribe(): void }`
    - `send(msg: ClientMsg): boolean`
    - `sendBinary(sub, bytes): boolean`
  - `defaultBackoff(attempt)`, which gives 500 ms doubling up to 30 s.
  - `wsUrl(loc?)`, which gives `ws(s)://<host>/api/v1/ws`.
- Produces `@/test/fake-ws` `FakeWebSocket`. Its static members are `instances` and `latest()`. Its test controls are `open()`, `receive(msg | ArrayBuffer)`, `drop()` and `frames()`.

- [ ] **Step 1: Write the failing test**

`web/src/test/fake-ws.ts`:

```ts
/** A WebSocket stand-in the test drives by hand. */
export class FakeWebSocket {
  static instances: FakeWebSocket[] = [];
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;

  readonly url: string;
  readyState = FakeWebSocket.CONNECTING;
  binaryType = "blob";
  sent: (string | Uint8Array)[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: (() => void) | null = null;

  constructor(url: string) {
    this.url = url;
    FakeWebSocket.instances.push(this);
  }

  static latest() {
    return FakeWebSocket.instances[FakeWebSocket.instances.length - 1];
  }

  send(data: string | Uint8Array) {
    this.sent.push(data);
  }

  /** The JSON text frames sent so far, parsed. */
  frames() {
    return this.sent.filter((s): s is string => typeof s === "string").map((s) => JSON.parse(s));
  }

  close() {
    this.readyState = FakeWebSocket.CLOSED;
    this.onclose?.();
  }

  // Test controls.
  open() {
    this.readyState = FakeWebSocket.OPEN;
    this.onopen?.();
  }

  receive(msg: unknown) {
    this.onmessage?.({ data: msg instanceof ArrayBuffer ? msg : JSON.stringify(msg) });
  }

  drop() {
    this.readyState = FakeWebSocket.CLOSED;
    this.onclose?.();
  }
}
```

`web/src/live/ws.test.ts`:

```ts
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { defaultBackoff, WsClient, wsUrl } from "./ws";
import { FakeWebSocket } from "@/test/fake-ws";

function client(extra: Partial<ConstructorParameters<typeof WsClient>[0]> = {}) {
  const ws = new WsClient({
    url: "ws://test/api/v1/ws",
    WebSocketImpl: FakeWebSocket as unknown as typeof WebSocket,
    backoff: () => 100,
    ...extra,
  });
  ws.start();
  return ws;
}

describe("WsClient", () => {
  beforeEach(() => {
    FakeWebSocket.instances = [];
    vi.useFakeTimers();
  });
  afterEach(() => vi.useRealTimers());

  it("queues subscriptions until the socket opens", () => {
    const ws = client();
    ws.subscribe({ topic: "events", env: "local" }, { onData: () => {} });
    const sock = FakeWebSocket.latest();
    expect(sock.sent).toEqual([]);
    sock.open();
    expect(sock.frames()).toEqual([{ op: "sub", sub: 1, topic: "events", env: "local" }]);
  });

  it("routes data, gap and end to the subscription's handlers", () => {
    const ws = client();
    const sock = FakeWebSocket.latest();
    sock.open();
    const onData = vi.fn();
    const onGap = vi.fn();
    const onEnd = vi.fn();
    const other = vi.fn();
    ws.subscribe({ topic: "logs", env: "local", id: "c1" }, { onData, onGap, onEnd });
    ws.subscribe({ topic: "events", env: "local" }, { onData: other });
    sock.receive({ sub: 1, type: "data", data: [{ line: "hi" }] });
    sock.receive({ sub: 1, type: "gap", dropped: 7 });
    sock.receive({ sub: 1, type: "end", reason: "eof" });
    sock.receive({ sub: 1, type: "data", data: "after end" });
    expect(onData).toHaveBeenCalledExactlyOnceWith([{ line: "hi" }]);
    expect(onGap).toHaveBeenCalledWith(7);
    expect(onEnd).toHaveBeenCalledWith({ sub: 1, type: "end", reason: "eof" });
    expect(other).not.toHaveBeenCalled();
  });

  it("drops a subscription the server refused", () => {
    const ws = client();
    const sock = FakeWebSocket.latest();
    sock.open();
    const onError = vi.fn();
    ws.subscribe({ topic: "stats", env: "nope", id: "c1" }, { onData: () => {}, onError });
    sock.receive({ sub: 1, type: "error", code: "not_found", message: "no env" });
    expect(onError).toHaveBeenCalledOnce();
    sock.drop();
    vi.advanceTimersByTime(100);
    FakeWebSocket.latest().open();
    expect(FakeWebSocket.latest().frames()).toEqual([]);
  });

  it("sends unsub and ignores later frames", () => {
    const ws = client();
    const sock = FakeWebSocket.latest();
    sock.open();
    const onData = vi.fn();
    const { unsubscribe } = ws.subscribe({ topic: "events", env: "local" }, { onData });
    unsubscribe();
    unsubscribe();
    sock.receive({ sub: 1, type: "data", data: {} });
    expect(onData).not.toHaveBeenCalled();
    expect(sock.frames().at(-1)).toEqual({ op: "unsub", sub: 1 });
    expect(sock.frames().filter((f) => f.op === "unsub")).toHaveLength(1);
  });

  it("reconnects with backoff and resubscribes every active subscription", () => {
    const delays: number[] = [];
    const onReconnect = vi.fn();
    const ws = client({ backoff: (n) => (delays.push(n), 1000 * (n + 1)), onReconnect });
    FakeWebSocket.latest().open();
    const onResubscribe = vi.fn();
    ws.subscribe({ topic: "events", env: "local" }, { onData: () => {}, onResubscribe });
    ws.subscribe({ topic: "stats", env: "local", id: "c1" }, { onData: () => {} });

    FakeWebSocket.latest().drop();
    vi.advanceTimersByTime(999);
    expect(FakeWebSocket.instances).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(FakeWebSocket.instances).toHaveLength(2);
    FakeWebSocket.latest().drop(); // the attempt failed
    vi.advanceTimersByTime(2000);
    const sock = FakeWebSocket.latest();
    expect(FakeWebSocket.instances).toHaveLength(3);
    sock.open();

    expect(delays).toEqual([0, 1]);
    expect(sock.frames()).toEqual([
      { op: "sub", sub: 1, topic: "events", env: "local" },
      { op: "sub", sub: 2, topic: "stats", env: "local", id: "c1" },
    ]);
    expect(onResubscribe).toHaveBeenCalledOnce();
    expect(onReconnect).toHaveBeenCalledOnce();
  });

  it("reports reconnecting from a drop until a socket opens again", () => {
    const states: string[] = [];
    client({ onState: (s) => states.push(s) });
    FakeWebSocket.latest().open();
    FakeWebSocket.latest().drop();
    vi.advanceTimersByTime(100);
    FakeWebSocket.latest().drop();
    vi.advanceTimersByTime(100);
    FakeWebSocket.latest().open();
    expect(states).toEqual(["connecting", "open", "reconnecting", "open"]);
  });

  it("reports reconnecting when the first connection fails", () => {
    const states: string[] = [];
    client({ onState: (s) => states.push(s) });
    FakeWebSocket.latest().drop();
    expect(states).toEqual(["connecting", "reconnecting"]);
  });

  it("does not reconnect after close(), and start() opens it again with the same subscriptions", () => {
    const ws = client();
    FakeWebSocket.latest().open();
    ws.subscribe({ topic: "events", env: "local" }, { onData: () => {} });
    ws.close();
    vi.advanceTimersByTime(60_000);
    expect(FakeWebSocket.instances).toHaveLength(1);
    ws.start();
    FakeWebSocket.latest().open();
    expect(FakeWebSocket.latest().frames()).toEqual([{ op: "sub", sub: 1, topic: "events", env: "local" }]);
  });

  it("does not connect until started", () => {
    new WsClient({ url: "ws://test/api/v1/ws", WebSocketImpl: FakeWebSocket as unknown as typeof WebSocket });
    expect(FakeWebSocket.instances).toHaveLength(0);
  });

  it("reports env_status frames", () => {
    const onEnvStatus = vi.fn();
    client({ onEnvStatus });
    const sock = FakeWebSocket.latest();
    sock.open();
    sock.receive({ type: "env_status", env: "local", status: { state: "down", reason: "gone" } });
    expect(onEnvStatus).toHaveBeenCalledWith("local", { state: "down", reason: "gone" });
  });

  it("frames binary exec input and routes binary output by sub id", () => {
    const ws = client();
    const sock = FakeWebSocket.latest();
    sock.open();
    const onBinary = vi.fn();
    const { sub } = ws.subscribe({ topic: "exec", env: "local", id: "c1" }, { onData: () => {}, onBinary });
    ws.sendBinary(sub, new Uint8Array([104, 105]));
    expect(sock.sent.at(-1)).toEqual(new Uint8Array([0, 0, 0, 1, 104, 105]));
    const out = new Uint8Array([0, 0, 0, 1, 111, 107]).buffer;
    sock.receive(out);
    expect(onBinary).toHaveBeenCalledWith(new Uint8Array([111, 107]));
  });

  it("ignores frames that are not JSON or name no subscription", () => {
    client();
    const sock = FakeWebSocket.latest();
    sock.open();
    expect(() => sock.onmessage?.({ data: "not json" })).not.toThrow();
    expect(() => sock.receive({ type: "error", code: "invalid", message: "bad frame", sub: null })).not.toThrow();
  });
});

describe("wsUrl", () => {
  it("uses wss on https", () => {
    expect(wsUrl({ protocol: "https:", host: "fleet.lan:9443" } as Location)).toBe("wss://fleet.lan:9443/api/v1/ws");
    expect(wsUrl({ protocol: "http:", host: "localhost:5173" } as Location)).toBe("ws://localhost:5173/api/v1/ws");
  });
});

describe("defaultBackoff", () => {
  it("doubles from 500 ms up to 30 s", () => {
    expect([0, 1, 2, 5, 6, 20].map(defaultBackoff)).toEqual([500, 1000, 2000, 16000, 30000, 30000]);
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd web && pnpm test src/live/ws.test.ts`
Expected: FAIL with `Failed to resolve import "./ws" from "src/live/ws.test.ts"`.

- [ ] **Step 3: Implement the client**

`web/src/live/ws.ts`:

```ts
import type { ClientMsg, EnvStatus, ServerMsg, Topic } from "@/api/gen/types.gen";

type EndMsg = Extract<ServerMsg, { type: "end" }>;
type ErrorMsg = Extract<ServerMsg, { type: "error" }>;

export interface SubSpec {
  topic: Topic;
  env?: string;
  id?: string;
  opts?: unknown;
}

export interface SubHandlers {
  onData(data: unknown): void;
  /** Exec output: the payload of a binary frame for this subscription. */
  onBinary?(bytes: Uint8Array): void;
  onGap?(dropped: number): void;
  /** The server ended the subscription; it is no longer active. */
  onEnd?(msg: EndMsg): void;
  /** The server refused or failed the subscription; it is no longer active. */
  onError?(msg: ErrorMsg): void;
  /** The connection was re-established and the subscription sent again. */
  onResubscribe?(): void;
}

/**
 * `connecting` until the first socket opens; `reconnecting` from any
 * unexpected close until a socket opens again.
 */
export type ConnState = "connecting" | "open" | "reconnecting";

export interface WsOptions {
  url: string;
  WebSocketImpl?: typeof WebSocket;
  /** Delay before reconnect attempt `n` (0-based). */
  backoff?: (attempt: number) => number;
  onState?(state: ConnState): void;
  onEnvStatus?(env: string, status: EnvStatus): void;
  /** Called after a reconnect, once every subscription has been sent again. */
  onReconnect?(): void;
}

export const defaultBackoff = (attempt: number) => Math.min(30_000, 500 * 2 ** attempt);

export function wsUrl(loc: Location = window.location) {
  return `${loc.protocol === "https:" ? "wss" : "ws"}://${loc.host}/api/v1/ws`;
}

interface Active {
  spec: SubSpec;
  handlers: SubHandlers;
}

/**
 * The tab's single websocket to `/api/v1/ws`. It multiplexes numbered
 * subscriptions, queues them until the socket is open, and on an
 * unexpected close reconnects with backoff and sends every active
 * subscription again.
 */
export class WsClient {
  private readonly opts: WsOptions;
  private socket: WebSocket | null = null;
  private readonly subs = new Map<number, Active>();
  private nextSub = 1;
  private attempt = 0;
  private opened = false;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private stopped = false;

  constructor(opts: WsOptions) {
    this.opts = opts;
  }

  /** Opens the connection; `close()` ends it and a later `start()` reopens it. */
  start() {
    if (this.socket || this.timer) return;
    this.stopped = false;
    this.connect();
  }

  subscribe(spec: SubSpec, handlers: SubHandlers): { sub: number; unsubscribe: () => void } {
    const sub = this.nextSub++;
    this.subs.set(sub, { spec, handlers });
    this.sendSub(sub, spec);
    return {
      sub,
      unsubscribe: () => {
        if (this.subs.delete(sub)) this.send({ op: "unsub", sub });
      },
    };
  }

  /** Sends a text frame if the socket is open; returns whether it was sent. */
  send(msg: ClientMsg): boolean {
    if (this.socket?.readyState !== WebSocket.OPEN) return false;
    this.socket.send(JSON.stringify(msg));
    return true;
  }

  /** Sends exec stdin as a binary frame: `[u32 sub, big-endian][bytes]`. */
  sendBinary(sub: number, bytes: Uint8Array): boolean {
    if (this.socket?.readyState !== WebSocket.OPEN) return false;
    const frame = new Uint8Array(4 + bytes.length);
    new DataView(frame.buffer).setUint32(0, sub);
    frame.set(bytes, 4);
    this.socket.send(frame);
    return true;
  }

  close() {
    this.stopped = true;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    const socket = this.socket;
    this.socket = null;
    this.opened = false;
    socket?.close();
  }

  private sendSub(sub: number, spec: SubSpec) {
    this.send({ op: "sub", sub, topic: spec.topic, env: spec.env, id: spec.id, opts: spec.opts });
  }

  private connect() {
    const Impl = this.opts.WebSocketImpl ?? WebSocket;
    const socket = new Impl(this.opts.url);
    socket.binaryType = "arraybuffer";
    this.socket = socket;
    if (!this.opened && this.attempt === 0) this.opts.onState?.("connecting");
    socket.onopen = () => {
      const reconnect = this.opened;
      this.opened = true;
      this.attempt = 0;
      this.opts.onState?.("open");
      for (const [sub, { spec, handlers }] of this.subs) {
        this.sendSub(sub, spec);
        if (reconnect) handlers.onResubscribe?.();
      }
      if (reconnect) this.opts.onReconnect?.();
    };
    socket.onmessage = (ev: MessageEvent) => this.receive(ev.data);
    socket.onclose = () => {
      if (this.socket !== socket) return;
      this.socket = null;
      if (this.stopped) return;
      if (this.attempt === 0) this.opts.onState?.("reconnecting");
      const delay = (this.opts.backoff ?? defaultBackoff)(this.attempt++);
      this.timer = setTimeout(() => {
        this.timer = null;
        this.connect();
      }, delay);
    };
  }

  private receive(data: unknown) {
    if (data instanceof ArrayBuffer) {
      if (data.byteLength < 4) return;
      const sub = new DataView(data).getUint32(0);
      this.subs.get(sub)?.handlers.onBinary?.(new Uint8Array(data, 4));
      return;
    }
    let msg: ServerMsg;
    try {
      msg = JSON.parse(String(data)) as ServerMsg;
    } catch {
      return;
    }
    if (msg.type === "env_status") {
      this.opts.onEnvStatus?.(msg.env, msg.status);
      return;
    }
    const sub = msg.sub;
    if (sub == null) return;
    const active = this.subs.get(sub);
    if (!active) return;
    switch (msg.type) {
      case "data":
        active.handlers.onData(msg.data);
        break;
      case "gap":
        active.handlers.onGap?.(msg.dropped);
        break;
      case "end":
        this.subs.delete(sub);
        active.handlers.onEnd?.(msg);
        break;
      case "error":
        this.subs.delete(sub);
        active.handlers.onError?.(msg);
        break;
    }
  }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 19 passed (2 files); typecheck and lint are clean.

- [ ] **Step 5: Commit**

```bash
git add web/src/live web/src/test/fake-ws.ts
git commit -m "feat(web): websocket client with subscriptions, reconnect and resubscribe"
```

---

### Task 3: Query keys, and Docker events applied to the cache

**Files:**
- Create: `web/src/api/queries.ts` and `web/src/live/events.ts`.
- Test: `web/src/live/events.test.ts`.

**Interfaces:**
- Consumes: the SDK functions `getAuthStatus`, `getMe`, `listSessions`, `listEnvs`, `getEnvStatus`, `getEngineInfo`, `getDiskUsage`, `listContainers`, `listImages`, `listVolumes` and `listNetworks`. Also `setCsrfToken` from Task 1.
- Produces `@/api/queries`:
  - `keys`:
    - `authStatus`, `me`, `sessions`, `envs`
    - `env(env)`, `envStatus(env)`, `info(env)`, `df(env)`
    - `containers(env)`, `container(env, id)`, `images(env)`, `image(env, id)`
    - `volumes(env)`, `volume(env, name)`, `networks(env)`, `network(env, id)`
    - `composeProjects(env)`, `recentEvents(env)`
  - Query options:
    - `authStatusQuery` and `meQuery`. `meQuery` stores the CSRF token, has `staleTime: Infinity` and does not retry.
    - `sessionsQuery` and `envsQuery`.
    - `envStatusQuery(env)`, `infoQuery(env)`, `dfQuery(env)`, `containersQuery(env)`, `imagesQuery(env)`, `volumesQuery(env)`, `networksQuery(env)` and `recentEventsQuery(env)`.
  - Types: `EngineInfo`, `DiskUsage` and `UsageTotals`.
- Produces `@/live/events`:
  - `RECENT_EVENTS = 50`
  - `isNoise(ev): boolean`
  - `keysForEvent(env, ev): QueryKey[]`
  - `class EventApplier`, with `constructor(qc, delay = 100)`, `apply(env, ev)`, `flush()` and `dispose()`.

- [ ] **Step 1: Write the failing test**

`web/src/live/events.test.ts`:

```ts
import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EventApplier, keysForEvent, RECENT_EVENTS } from "./events";
import { keys } from "@/api/queries";
import type { DockerEvent } from "@/api/gen/types.gen";

const ev = (kind: string, action: string, attributes: Record<string, string> = {}): DockerEvent => ({
  kind,
  action,
  id: "abc",
  attributes,
  time: 1,
});

describe("keysForEvent", () => {
  it("container state changes refresh containers, compose projects and counts", () => {
    expect(keysForEvent("local", ev("container", "start"))).toEqual([
      keys.containers("local"),
      keys.composeProjects("local"),
      keys.info("local"),
    ]);
  });

  it("container create and destroy also refresh in-use flags and disk usage", () => {
    const got = keysForEvent("local", ev("container", "destroy"));
    expect(got).toContainEqual(keys.images("local"));
    expect(got).toContainEqual(keys.volumes("local"));
    expect(got).toContainEqual(keys.df("local"));
  });

  it("health status changes refresh the container list", () => {
    expect(keysForEvent("local", ev("container", "health_status: healthy"))).toContainEqual(keys.containers("local"));
  });

  it("exec and attach events refresh nothing", () => {
    expect(keysForEvent("local", ev("container", "exec_start: sh"))).toEqual([]);
    expect(keysForEvent("local", ev("container", "exec_die"))).toEqual([]);
    expect(keysForEvent("local", ev("container", "attach"))).toEqual([]);
  });

  it("images, volumes and networks refresh their lists", () => {
    expect(keysForEvent("local", ev("image", "pull"))).toContainEqual(keys.images("local"));
    expect(keysForEvent("local", ev("volume", "create"))).toContainEqual(keys.volumes("local"));
    expect(keysForEvent("local", ev("network", "connect", { container: "c1" }))).toEqual([
      keys.networks("local"),
      keys.container("local", "c1"),
    ]);
  });

  it("daemon events refresh the whole environment and unknown kinds nothing", () => {
    expect(keysForEvent("local", ev("daemon", "reload"))).toEqual([keys.env("local")]);
    expect(keysForEvent("local", ev("plugin", "enable"))).toEqual([]);
  });
});

describe("EventApplier", () => {
  let qc: QueryClient;
  beforeEach(() => {
    vi.useFakeTimers();
    qc = new QueryClient();
  });
  afterEach(() => vi.useRealTimers());

  it("invalidates each affected query once per batch", () => {
    const spy = vi.spyOn(qc, "invalidateQueries");
    const applier = new EventApplier(qc, 100);
    for (let i = 0; i < 20; i++) applier.apply("local", ev("container", "start"));
    expect(spy).not.toHaveBeenCalled();
    vi.advanceTimersByTime(100);
    expect(spy).toHaveBeenCalledTimes(3);
    expect(spy).toHaveBeenCalledWith({ queryKey: keys.containers("local") }, { cancelRefetch: false });
  });

  it("marks matching cached queries stale and leaves others alone", () => {
    qc.setQueryData(keys.containers("local"), []);
    qc.setQueryData(keys.networks("local"), []);
    const applier = new EventApplier(qc, 100);
    applier.apply("local", ev("container", "die"));
    applier.flush();
    expect(qc.getQueryState(keys.containers("local"))?.isInvalidated).toBe(true);
    expect(qc.getQueryState(keys.networks("local"))?.isInvalidated).toBe(false);
  });

  it("leaves exec noise out of the feed", () => {
    const applier = new EventApplier(qc, 100);
    applier.apply("local", ev("container", "exec_start: pg_isready"));
    applier.apply("local", ev("container", "start"));
    expect(qc.getQueryData<DockerEvent[]>(keys.recentEvents("local"))!.map((e) => e.action)).toEqual(["start"]);
    applier.dispose();
  });

  it("keeps the newest events first, capped", () => {
    const applier = new EventApplier(qc, 100);
    for (let i = 0; i < RECENT_EVENTS + 5; i++) applier.apply("local", { ...ev("container", "start"), time: i });
    const feed = qc.getQueryData<DockerEvent[]>(keys.recentEvents("local"))!;
    expect(feed).toHaveLength(RECENT_EVENTS);
    expect(feed[0].time).toBe(RECENT_EVENTS + 4);
    applier.dispose();
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd web && pnpm test src/live/events.test.ts`
Expected: FAIL with `Failed to resolve import "./events" from "src/live/events.test.ts"`.

- [ ] **Step 3: Implement the keys and the applier**

`web/src/api/queries.ts`:

```ts
import { queryOptions } from "@tanstack/react-query";
import { setCsrfToken } from "./client";
import {
  getAuthStatus,
  getDiskUsage,
  getEngineInfo,
  getEnvStatus,
  getMe,
  listContainers,
  listEnvs,
  listImages,
  listNetworks,
  listSessions,
  listVolumes,
} from "./gen/sdk.gen";
import type { DockerEvent } from "./gen/types.gen";

/**
 * Query keys. Everything scoped to an environment starts with
 * `["env", env]`, so one prefix invalidates all of it; a resource's
 * detail key extends its list key.
 */
export const keys = {
  authStatus: ["auth", "status"] as const,
  me: ["auth", "me"] as const,
  sessions: ["auth", "sessions"] as const,
  envs: ["envs"] as const,
  env: (env: string) => ["env", env] as const,
  envStatus: (env: string) => ["env", env, "status"] as const,
  info: (env: string) => ["env", env, "info"] as const,
  df: (env: string) => ["env", env, "df"] as const,
  containers: (env: string) => ["env", env, "containers"] as const,
  container: (env: string, id: string) => ["env", env, "containers", id] as const,
  images: (env: string) => ["env", env, "images"] as const,
  image: (env: string, id: string) => ["env", env, "images", id] as const,
  volumes: (env: string) => ["env", env, "volumes"] as const,
  volume: (env: string, name: string) => ["env", env, "volumes", name] as const,
  networks: (env: string) => ["env", env, "networks"] as const,
  network: (env: string, id: string) => ["env", env, "networks", id] as const,
  composeProjects: (env: string) => ["env", env, "compose-projects"] as const,
  /** The newest Docker events seen over the websocket; filled by the live layer, never fetched. */
  recentEvents: (env: string) => ["live", env, "events"] as const,
};

export const authStatusQuery = queryOptions({
  queryKey: keys.authStatus,
  queryFn: async () => (await getAuthStatus()).data,
});

export const meQuery = queryOptions({
  queryKey: keys.me,
  queryFn: async () => {
    const { data } = await getMe();
    setCsrfToken(data.csrf_token);
    return data;
  },
  staleTime: Infinity,
  retry: false,
});

export const sessionsQuery = queryOptions({
  queryKey: keys.sessions,
  queryFn: async () => (await listSessions()).data,
});

export const envsQuery = queryOptions({
  queryKey: keys.envs,
  queryFn: async () => (await listEnvs()).data,
});

const path = (env: string) => ({ path: { env } });

export const envStatusQuery = (env: string) =>
  queryOptions({ queryKey: keys.envStatus(env), queryFn: async () => (await getEnvStatus(path(env))).data });

export const infoQuery = (env: string) =>
  queryOptions({ queryKey: keys.info(env), queryFn: async () => (await getEngineInfo(path(env))).data as EngineInfo });

export const dfQuery = (env: string) =>
  queryOptions({ queryKey: keys.df(env), queryFn: async () => (await getDiskUsage(path(env))).data as DiskUsage });

export const containersQuery = (env: string) =>
  queryOptions({ queryKey: keys.containers(env), queryFn: async () => (await listContainers(path(env))).data });

export const imagesQuery = (env: string) =>
  queryOptions({ queryKey: keys.images(env), queryFn: async () => (await listImages(path(env))).data });

export const volumesQuery = (env: string) =>
  queryOptions({ queryKey: keys.volumes(env), queryFn: async () => (await listVolumes(path(env))).data });

export const networksQuery = (env: string) =>
  queryOptions({ queryKey: keys.networks(env), queryFn: async () => (await listNetworks(path(env))).data });

export const recentEventsQuery = (env: string) =>
  queryOptions<DockerEvent[]>({
    queryKey: keys.recentEvents(env),
    queryFn: () => [],
    staleTime: Infinity,
    gcTime: Infinity,
  });

/** The fields of Docker's `GET /info` the UI reads (the API passes it through). */
export interface EngineInfo {
  Name?: string;
  ServerVersion?: string;
  OperatingSystem?: string;
  Architecture?: string;
  NCPU?: number;
  MemTotal?: number;
  Containers?: number;
  ContainersRunning?: number;
  ContainersPaused?: number;
  ContainersStopped?: number;
  Images?: number;
}

/** Docker's `GET /system/df`, as the API passes it through: newer engines send `*Usage` totals, older ones item lists. */
export interface DiskUsage {
  ImageUsage?: UsageTotals;
  ContainerUsage?: UsageTotals;
  VolumeUsage?: UsageTotals;
  BuildCacheUsage?: UsageTotals;
  LayersSize?: number;
  Images?: { Size?: number }[] | null;
  Containers?: { SizeRw?: number }[] | null;
  Volumes?: { UsageData?: { Size?: number } | null }[] | null;
  BuildCache?: { Size?: number }[] | null;
}

export interface UsageTotals {
  TotalSize?: number;
  Reclaimable?: number;
}
```

`web/src/live/events.ts`:

```ts
import type { QueryClient, QueryKey } from "@tanstack/react-query";
import { keys } from "@/api/queries";
import type { DockerEvent } from "@/api/gen/types.gen";

/** How many events the dashboard feed keeps. */
export const RECENT_EVENTS = 50;

const QUIET = ["attach", "detach", "resize", "top", "export", "archive-path", "extract-to-dir"];

/**
 * Container events that change nothing the UI shows: exec (healthchecks
 * run one every few seconds per container), attach and file copies.
 */
export function isNoise(ev: DockerEvent): boolean {
  const action = ev.action.split(":")[0];
  return ev.kind === "container" && (action.startsWith("exec_") || QUIET.includes(action));
}

/**
 * The queries a Docker event makes stale; none for noise.
 */
export function keysForEvent(env: string, ev: DockerEvent): QueryKey[] {
  if (isNoise(ev)) return [];
  const action = ev.action.split(":")[0];
  switch (ev.kind) {
    case "container": {
      const out: QueryKey[] = [keys.containers(env), keys.composeProjects(env), keys.info(env)];
      if (["create", "destroy"].includes(action)) {
        out.push(keys.images(env), keys.volumes(env), keys.df(env));
      }
      return out;
    }
    case "image":
      return [keys.images(env), keys.info(env), keys.df(env)];
    case "volume":
      return [keys.volumes(env), keys.df(env)];
    case "network": {
      const out: QueryKey[] = [keys.networks(env)];
      const container = ev.attributes.container;
      if (container) out.push(keys.container(env, container));
      return out;
    }
    case "daemon":
      return [keys.env(env)];
    default:
      return [];
  }
}

/**
 * Applies Docker events to the query cache: it records each event other
 * than noise in the recent-events feed at once, and invalidates the affected queries in
 * batches, so a burst of events (a compose project starting) causes one
 * refetch per query rather than one per event.
 */
export class EventApplier {
  private readonly qc: QueryClient;
  private readonly delay: number;
  private readonly pending = new Map<string, QueryKey>();
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(qc: QueryClient, delay = 100) {
    this.qc = qc;
    this.delay = delay;
  }

  apply(env: string, ev: DockerEvent) {
    if (isNoise(ev)) return;
    this.qc.setQueryData<DockerEvent[]>(keys.recentEvents(env), (old = []) => [ev, ...old].slice(0, RECENT_EVENTS));
    for (const key of keysForEvent(env, ev)) this.pending.set(JSON.stringify(key), key);
    if (this.pending.size > 0 && !this.timer) {
      this.timer = setTimeout(() => this.flush(), this.delay);
    }
  }

  flush() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    const batch = [...this.pending.values()];
    this.pending.clear();
    for (const queryKey of batch) {
      void this.qc.invalidateQueries({ queryKey }, { cancelRefetch: false });
    }
  }

  dispose() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.pending.clear();
  }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 29 passed (3 files); typecheck and lint are clean.

- [ ] **Step 5: Commit**

```bash
git add web/src/api/queries.ts web/src/live/events.ts web/src/live/events.test.ts
git commit -m "feat(web): query keys and batched cache invalidation from Docker events"
```

---

### Task 4: Theme, error text, and formatting

**Files:**
- Create: `web/src/theme.tsx`, `web/src/lib/errors.ts`, `web/src/lib/format.ts` and `web/src/lib/df.ts`.
- Test: `web/src/theme.test.tsx`, `web/src/lib/lib.test.ts` and `web/src/lib/df.test.ts`.

**Interfaces:**
- Consumes: `ApiError` (Task 1), and `DiskUsage` and `UsageTotals` (Task 3).
- Produces `@/theme`:
  - `type Theme = "light" | "dark" | "system"`
  - `readTheme()`
  - `resolveTheme(theme, prefersDark)`
  - `<ThemeProvider>`
  - `useTheme(): { theme, setTheme }`
- Produces `@/lib/errors`: `errorMessage(err): string` and `toastError(err)`.
- Produces `@/lib/format`: `formatBytes(n)`, which gives 1000-based units and `—` for invalid input, and `formatTime(unix)`.
- Produces `@/lib/df`:
  - `summarizeDiskUsage(df): DiskSummary`
  - `DiskSummary`, which is `{ images, containers, volumes, buildCache }`.
  - `Usage`, which is `{ size, reclaimable? }`.

- [ ] **Step 1: Write the failing tests**

`web/src/theme.test.tsx`:

```tsx
import { act, render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { readTheme, resolveTheme, ThemeProvider, useTheme, type Theme } from "./theme";

function Probe({ onReady }: { onReady: (set: (t: Theme) => void) => void }) {
  onReady(useTheme().setTheme);
  return null;
}

describe("theme", () => {
  it("resolves system from the OS preference", () => {
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
    expect(resolveTheme("light", true)).toBe("light");
  });

  it("applies and remembers the chosen theme", () => {
    let set!: (t: Theme) => void;
    render(
      <ThemeProvider>
        <Probe onReady={(s) => (set = s)} />
      </ThemeProvider>,
    );
    act(() => set("dark"));
    expect(document.documentElement).toHaveClass("dark");
    expect(readTheme()).toBe("dark");
    act(() => set("system"));
    expect(document.documentElement).not.toHaveClass("dark");
    expect(readTheme()).toBe("system");
  });
});
```

`web/src/lib/lib.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/client";
import { errorMessage } from "./errors";
import { formatBytes } from "./format";

describe("formatBytes", () => {
  it("scales by 1000 with one decimal", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(999)).toBe("999 B");
    expect(formatBytes(1500)).toBe("1.5 KB");
    expect(formatBytes(2_340_000_000)).toBe("2.3 GB");
  });

  it("shows a dash for missing or invalid sizes", () => {
    expect(formatBytes(undefined)).toBe("—");
    expect(formatBytes(-1)).toBe("—");
    expect(formatBytes(Number.NaN)).toBe("—");
  });
});

describe("errorMessage", () => {
  it("adds the request id to internal errors only", () => {
    expect(errorMessage(new ApiError(500, { code: "internal", message: "internal error", request_id: "r9" }))).toBe(
      "internal error (request r9)",
    );
    expect(errorMessage(new ApiError(409, { code: "conflict", message: "name in use", request_id: "r9" }))).toBe(
      "name in use",
    );
    expect(errorMessage(new TypeError("Failed to fetch"))).toBe("Failed to fetch");
  });
});
```

`web/src/lib/df.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { summarizeDiskUsage } from "./df";

describe("summarizeDiskUsage", () => {
  it("reads the totals newer engines send", () => {
    expect(
      summarizeDiskUsage({
        ImageUsage: { TotalSize: 3000, Reclaimable: 1000 },
        ContainerUsage: { TotalSize: 20, Reclaimable: 5 },
        VolumeUsage: { TotalSize: 400 },
        BuildCacheUsage: { TotalSize: 7, Reclaimable: 7 },
      }),
    ).toEqual({
      images: { size: 3000, reclaimable: 1000 },
      containers: { size: 20, reclaimable: 5 },
      volumes: { size: 400, reclaimable: undefined },
      buildCache: { size: 7, reclaimable: 7 },
    });
  });

  it("sums the item lists older engines send", () => {
    expect(
      summarizeDiskUsage({
        LayersSize: 3000,
        Containers: [{ SizeRw: 5 }, { SizeRw: 15 }, {}],
        Volumes: [{ UsageData: { Size: 400 } }, { UsageData: null }],
        BuildCache: null,
      }),
    ).toEqual({ images: { size: 3000 }, containers: { size: 20 }, volumes: { size: 400 }, buildCache: { size: 0 } });
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && pnpm test src/theme.test.tsx src/lib`
Expected: FAIL, 3 files, with `Failed to resolve import "./theme"`, `"./errors"` and `"./df"`.

- [ ] **Step 3: Implement**

`web/src/theme.tsx`:

```tsx
import { createContext, useContext, useEffect, useState, type ReactNode } from "react";

export type Theme = "light" | "dark" | "system";

const KEY = "fleetops-theme";

export function readTheme(): Theme {
  try {
    const t = localStorage.getItem(KEY);
    return t === "light" || t === "dark" ? t : "system";
  } catch {
    return "system";
  }
}

export function resolveTheme(theme: Theme, prefersDark: boolean): "light" | "dark" {
  return theme === "system" ? (prefersDark ? "dark" : "light") : theme;
}

const query = () => window.matchMedia?.("(prefers-color-scheme: dark)");

const ThemeContext = createContext<{ theme: Theme; setTheme: (t: Theme) => void } | null>(null);

/** Applies the chosen theme as the `dark` class on <html> and remembers it. */
export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(readTheme);

  useEffect(() => {
    const mq = query();
    const apply = () => {
      const dark = resolveTheme(theme, mq?.matches ?? false) === "dark";
      document.documentElement.classList.toggle("dark", dark);
      document.documentElement.style.colorScheme = dark ? "dark" : "light";
    };
    apply();
    if (theme !== "system" || !mq) return;
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [theme]);

  const setTheme = (t: Theme) => {
    try {
      if (t === "system") localStorage.removeItem(KEY);
      else localStorage.setItem(KEY, t);
    } catch {
      // Storage may be unavailable (private mode); the choice lasts for this tab.
    }
    setThemeState(t);
  };

  return <ThemeContext value={{ theme, setTheme }}>{children}</ThemeContext>;
}

export function useTheme() {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme outside ThemeProvider");
  return ctx;
}
```

`web/src/lib/errors.ts`:

```ts
import { toast } from "sonner";
import { ApiError } from "@/api/client";

/**
 * The text to show for a failed request: the server's message (Docker's
 * own, for upstream errors), plus the request id for internal errors so a
 * user can find the server log line.
 */
export function errorMessage(err: unknown): string {
  if (err instanceof ApiError) {
    return err.code === "internal" && err.requestId ? `${err.message} (request ${err.requestId})` : err.message;
  }
  if (err instanceof Error) return err.message;
  return String(err);
}

export function toastError(err: unknown) {
  toast.error(errorMessage(err));
}
```

`web/src/lib/format.ts`:

```ts
const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

/** Bytes in powers of 1000 with one decimal place above 1 KB: 1.5 GB. */
export function formatBytes(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n) || n < 0) return "—";
  let i = 0;
  while (n >= 1000 && i < UNITS.length - 1) {
    n /= 1000;
    i++;
  }
  return i === 0 ? `${n} B` : `${n.toFixed(1)} ${UNITS[i]}`;
}

/** Unix seconds as a local date and time. */
export function formatTime(unix: number): string {
  return new Date(unix * 1000).toLocaleString();
}
```

`web/src/lib/df.ts`:

```ts
import type { DiskUsage, UsageTotals } from "@/api/queries";

export interface Usage {
  size: number;
  /** Absent on engines that do not report it. */
  reclaimable?: number;
}

export interface DiskSummary {
  images: Usage;
  containers: Usage;
  volumes: Usage;
  buildCache: Usage;
}

const sum = (xs: (number | undefined)[] | null | undefined) => (xs ?? []).reduce<number>((a, b) => a + (b ?? 0), 0);

const totals = (u: UsageTotals): Usage => ({ size: u.TotalSize ?? 0, reclaimable: u.Reclaimable });

/** Sizes per resource kind from either shape of Docker's `system df`. */
export function summarizeDiskUsage(df: DiskUsage): DiskSummary {
  if (df.ImageUsage || df.ContainerUsage || df.VolumeUsage || df.BuildCacheUsage) {
    return {
      images: totals(df.ImageUsage ?? {}),
      containers: totals(df.ContainerUsage ?? {}),
      volumes: totals(df.VolumeUsage ?? {}),
      buildCache: totals(df.BuildCacheUsage ?? {}),
    };
  }
  return {
    images: { size: df.LayersSize ?? sum(df.Images?.map((i) => i.Size)) },
    containers: { size: sum(df.Containers?.map((c) => c.SizeRw)) },
    volumes: { size: sum(df.Volumes?.map((v) => v.UsageData?.Size)) },
    buildCache: { size: sum(df.BuildCache?.map((b) => b.Size)) },
  };
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 36 passed (6 files); typecheck and lint are clean.

- [ ] **Step 5: Commit**

```bash
git add web/src/theme.tsx web/src/theme.test.tsx web/src/lib
git commit -m "feat(web): theme, error text and size formatting"
```

---

### Task 5: Routes, guards, and the setup and login pages

**Files:**
- Create: `web/src/router.tsx` (without the shell and account routes, which Task 6 adds) and `web/src/App.tsx`.
- Create: `web/src/pages/{LoginPage,SetupPage,AuthLayout,misc}.tsx`, and `web/src/pages/DashboardPage.tsx` (a heading only; Task 7 fills it in).
- Modify: `web/src/main.tsx` and `web/src/test/setup.ts`.
- Test helper: `web/src/test/app.tsx`.
- Test: `web/src/router.test.tsx` and `web/src/pages/auth.test.tsx`.

**Interfaces:**
- Consumes: `configureApi`, `setCsrfToken`, `ApiError`, `keys`, `authStatusQuery`, `envsQuery`, `meQuery`, `errorMessage`, `toastError`, `ThemeProvider` and `useTheme`. From the SDK it uses `login` and `setup`.
- Produces `@/router`:
  - `createAppRouter(queryClient, history?)`
  - `safeRedirect(target): string | undefined`
  - `envRoute`
  - `RouterContext { queryClient }`
  - The route ids `/setup`, `/login`, `authed` and `/envs/$env`.
- Produces `@/App`:
  - `createQueryClient()`
  - `createApp(history?)`, which returns `{ queryClient, router }` and calls `configureApi` with an `onUnauthorized` that clears `me` and navigates to `/login?redirect=<current href>`.
  - `<App history?>`
- Produces `@/test/app`: `ADMIN: SessionInfo`, `ENVS`, `loggedIn` (the mock routes of a set-up instance) and `renderApp(path): { history, screen }`.
- `test/setup.ts` now stubs `WebSocket` with `FakeWebSocket` in every test, and supplies the `matchMedia`, `ResizeObserver`, pointer-capture and `scrollTo` shims that sonner and Radix need in jsdom.

- [ ] **Step 1: Write the failing tests**

`web/src/test/setup.ts` (replaces the Task 1 version):

```ts
import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, beforeEach, vi } from "vitest";
import { FakeWebSocket } from "./fake-ws";

// jsdom lacks these browser APIs; sonner and Radix menus use them.
window.matchMedia ??= (query: string) =>
  ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  }) as MediaQueryList;
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
Element.prototype.scrollIntoView ??= () => {};
window.scrollTo = () => {};
Element.prototype.hasPointerCapture ??= () => false;
Element.prototype.releasePointerCapture ??= () => {};

beforeEach(() => {
  FakeWebSocket.instances = [];
  vi.stubGlobal("WebSocket", FakeWebSocket);
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  localStorage.clear();
  document.documentElement.className = "";
});
```

`web/src/test/app.tsx`:

```tsx
import { render, screen } from "@testing-library/react";
import { createMemoryHistory } from "@tanstack/react-router";
import { App } from "@/App";
import type { SessionInfo } from "@/api/gen/types.gen";

export const ADMIN: SessionInfo = {
  csrf_token: "csrf-1",
  user: { id: 1, username: "admin", role: "admin", created_at: 0, updated_at: 0 },
};

export const ENVS = [{ id: 1, name: "local", kind: "docker-local", url: "unix:///var/run/docker.sock", created_at: 0, updated_at: 0 }];

/** API routes for a set-up instance with a logged-in admin and one environment. */
export const loggedIn = {
  "GET /api/v1/auth/status": { json: { setup_required: false, setup_open: false } },
  "GET /api/v1/auth/me": { json: ADMIN },
  "GET /api/v1/envs": { json: ENVS },
  "GET /api/v1/envs/local/status": { json: { state: "up", version: { version: "29.5.2", api_version: "1.52", os: "linux", arch: "arm64" } } },
  "GET /api/v1/envs/local/info": { json: {} },
  "GET /api/v1/envs/local/df": { json: {} },
  "GET /api/v1/envs/local/volumes": { json: [] },
  "GET /api/v1/envs/local/networks": { json: [] },
};

/** Renders the whole app at `path`; returns the memory history to inspect navigation. */
export function renderApp(path: string) {
  const history = createMemoryHistory({ initialEntries: [path] });
  render(<App history={history} />);
  return { history, screen };
}
```

`web/src/router.test.tsx`:

```tsx
import { waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { safeRedirect } from "./router";
import { mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";

const unauthorized = { status: 401, json: { code: "unauthorized", message: "not logged in" } };

describe("routing", () => {
  it("sends every page to setup while no user exists", async () => {
    mockFetch({ "GET /api/v1/auth/status": { json: { setup_required: true, setup_open: true } } });
    const { history, screen } = renderApp("/envs/local");
    expect(await screen.findByText("Welcome to FleetOps")).toBeInTheDocument();
    expect(history.location.pathname).toBe("/setup");
  });

  it("sends visitors without a session to login, remembering where they were going", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/auth/me": unauthorized });
    const { history, screen } = renderApp("/envs/local");
    expect(await screen.findByText("Log in to FleetOps")).toBeInTheDocument();
    expect(history.location.pathname).toBe("/login");
    expect(history.location.search).toContain("redirect=%2Fenvs%2Flocal");
  });

  it("opens the first environment from the start page", async () => {
    mockFetch(loggedIn);
    const { history, screen } = renderApp("/");
    expect(await screen.findByRole("heading", { name: "local" })).toBeInTheDocument();
    expect(history.location.pathname).toBe("/envs/local");
  });

  it("explains an instance with no environments", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs": { json: [] } });
    const { screen } = renderApp("/");
    expect(await screen.findByText("No environments")).toBeInTheDocument();
  });

  it("sends a logged-in user away from login and setup", async () => {
    mockFetch(loggedIn);
    const { history } = renderApp("/login");
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local"));
  });

  it("shows not found for unknown paths", async () => {
    mockFetch(loggedIn);
    const { screen } = renderApp("/nope/nothing");
    expect(await screen.findByText("Page not found")).toBeInTheDocument();
  });
});

describe("safeRedirect", () => {
  it("keeps local paths only", () => {
    expect(safeRedirect("/envs/local?x=1")).toBe("/envs/local?x=1");
    expect(safeRedirect("//evil.example/")).toBeUndefined();
    expect(safeRedirect("https://evil.example/")).toBeUndefined();
    expect(safeRedirect(42)).toBeUndefined();
  });
});
```

`web/src/pages/auth.test.tsx`:

```tsx
import { waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { ADMIN, loggedIn, renderApp } from "@/test/app";

const fresh = { "GET /api/v1/auth/status": { json: { setup_required: true, setup_open: true } } };

describe("setup page", () => {
  it("creates the administrator and opens the app", async () => {
    let created = false;
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/status": () => json(200, { setup_required: !created, setup_open: !created }),
      "POST /api/v1/auth/setup": () => ((created = true), json(201, ADMIN)),
    });
    const { history, screen } = renderApp("/setup");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Password"), "correct-horse-9");
    await user.type(screen.getByLabelText("Confirm password"), "correct-horse-9");
    await user.click(screen.getByRole("button", { name: "Create administrator" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local"));
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ username: "admin", password: "correct-horse-9" });
  });

  it("refuses mismatched passwords without calling the server", async () => {
    const calls = mockFetch(fresh);
    const { screen } = renderApp("/setup");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Password"), "correct-horse-9");
    await user.type(screen.getByLabelText("Confirm password"), "correct-horse-8");
    await user.click(screen.getByRole("button", { name: "Create administrator" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The passwords do not match.");
    expect(calls.some((c) => c.method === "POST")).toBe(false);
  });

  it("shows the server's validation message", async () => {
    mockFetch({
      ...fresh,
      "POST /api/v1/auth/setup": { status: 400, json: { code: "invalid", message: "password must be 8-1024 characters" } },
    });
    const { screen } = renderApp("/setup");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Password"), "short");
    await user.type(screen.getByLabelText("Confirm password"), "short");
    await user.click(screen.getByRole("button", { name: "Create administrator" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("password must be 8-1024 characters");
  });

  it("explains a locked setup window", async () => {
    mockFetch({ "GET /api/v1/auth/status": { json: { setup_required: true, setup_open: false } } });
    const { screen } = renderApp("/setup");
    expect(await screen.findByText("Setup is locked")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create administrator" })).toBeNull();
  });
});

describe("login page", () => {
  const out = { ...loggedIn, "GET /api/v1/auth/me": { status: 401, json: { code: "unauthorized", message: "no" } } };

  it("logs in and returns to the page the user asked for", async () => {
    mockFetch({ ...out, "POST /api/v1/auth/login": { json: ADMIN } });
    const { history, screen } = renderApp("/login?redirect=%2Felsewhere");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Username"), "admin");
    await user.type(screen.getByLabelText("Password"), "correct-horse-9");
    await user.click(screen.getByRole("button", { name: "Log in" }));
    await waitFor(() => expect(history.location.pathname).toBe("/elsewhere"));
  });

  it("shows the server's message for bad credentials and rate limiting", async () => {
    let n = 0;
    mockFetch({
      ...out,
      "POST /api/v1/auth/login": () =>
        ++n === 1
          ? json(401, { code: "unauthorized", message: "invalid username or password" })
          : json(429, { code: "rate_limited", message: "too many failed logins; try again in 5 minutes" }),
    });
    const { screen } = renderApp("/login");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Username"), "admin");
    await user.type(screen.getByLabelText("Password"), "wrong-password");
    await user.click(screen.getByRole("button", { name: "Log in" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("invalid username or password");
    await user.click(screen.getByRole("button", { name: "Log in" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("try again in 5 minutes"));
  });

  it("ignores a redirect to another site", async () => {
    mockFetch({ ...out, "POST /api/v1/auth/login": { json: ADMIN } });
    const { history, screen } = renderApp("/login?redirect=%2F%2Fevil.example%2F");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Username"), "admin");
    await user.type(screen.getByLabelText("Password"), "correct-horse-9");
    await user.click(screen.getByRole("button", { name: "Log in" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local"));
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && pnpm test src/router.test.tsx src/pages`
Expected: FAIL, 2 files, with `Failed to resolve import "./router"` and `Failed to resolve import "@/test/app"` (which imports `@/App`).

- [ ] **Step 3: Implement the router and the app**

`web/src/router.tsx`:

```tsx
import type { QueryClient } from "@tanstack/react-query";
import {
  createRootRouteWithContext,
  createRoute,
  createRouter,
  Outlet,
  redirect,
  type RouterHistory,
} from "@tanstack/react-router";
import { ApiError } from "@/api/client";
import { authStatusQuery, envsQuery, meQuery } from "@/api/queries";
import { DashboardPage } from "@/pages/DashboardPage";
import { LoginPage } from "@/pages/LoginPage";
import { NoEnvironmentsPage, NotFoundPage } from "@/pages/misc";
import { SetupPage } from "@/pages/SetupPage";

export interface RouterContext {
  queryClient: QueryClient;
}

/** A local path to return to after login; anything else is dropped. */
export function safeRedirect(target: unknown): string | undefined {
  return typeof target === "string" && target.startsWith("/") && !target.startsWith("//") ? target : undefined;
}

/** The session, or null when there is none (a 401 from `/auth/me`). */
async function session(qc: QueryClient) {
  try {
    return await qc.ensureQueryData(meQuery);
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) return null;
    throw e;
  }
}

const rootRoute = createRootRouteWithContext<RouterContext>()({
  component: Outlet,
  notFoundComponent: NotFoundPage,
});

const setupRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/setup",
  beforeLoad: async ({ context }) => {
    const status = await context.queryClient.fetchQuery(authStatusQuery);
    if (!status.setup_required) throw redirect({ to: "/login" });
    return { setupOpen: status.setup_open };
  },
  component: SetupPage,
});

const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/login",
  validateSearch: (s: Record<string, unknown>): { redirect?: string } => ({ redirect: safeRedirect(s.redirect) }),
  beforeLoad: async ({ context, search }) => {
    const status = await context.queryClient.fetchQuery(authStatusQuery);
    if (status.setup_required) throw redirect({ to: "/setup" });
    if (await session(context.queryClient)) throw redirect({ to: search.redirect ?? "/" });
  },
  component: LoginPage,
});

/** Everything behind a session: sends visitors to setup or login first. */
const authedRoute = createRoute({
  getParentRoute: () => rootRoute,
  id: "authed",
  beforeLoad: async ({ context, location }) => {
    const status = await context.queryClient.fetchQuery(authStatusQuery);
    if (status.setup_required) throw redirect({ to: "/setup" });
    if (!(await session(context.queryClient))) {
      throw redirect({ to: "/login", search: { redirect: location.href } });
    }
  },
  component: Outlet,
});

const indexRoute = createRoute({
  getParentRoute: () => authedRoute,
  path: "/",
  beforeLoad: async ({ context }) => {
    const envs = await context.queryClient.fetchQuery(envsQuery);
    if (envs.length > 0) throw redirect({ to: "/envs/$env", params: { env: envs[0].name } });
  },
  component: NoEnvironmentsPage,
});

export const envRoute = createRoute({
  getParentRoute: () => authedRoute,
  path: "/envs/$env",
  component: Outlet,
});

const dashboardRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "/",
  component: DashboardPage,
});

const routeTree = rootRoute.addChildren([
  setupRoute,
  loginRoute,
  authedRoute.addChildren([indexRoute, envRoute.addChildren([dashboardRoute])]),
]);

export function createAppRouter(queryClient: QueryClient, history?: RouterHistory) {
  return createRouter({
    routeTree,
    context: { queryClient },
    history,
    defaultPreload: "intent",
    defaultPreloadStaleTime: 0,
  });
}

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof createAppRouter>;
  }
}
```

`web/src/App.tsx`:

```tsx
import { MutationCache, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, type RouterHistory } from "@tanstack/react-router";
import { useState } from "react";
import { Toaster } from "sonner";
import { configureApi } from "@/api/client";
import { keys } from "@/api/queries";
import { toastError } from "@/lib/errors";
import { createAppRouter } from "@/router";
import { ThemeProvider, useTheme } from "@/theme";

/**
 * Mutations with no error handling of their own (no `onError`, and not
 * shown inline via `meta.inline`) report failures as a toast.
 */
export function createQueryClient() {
  return new QueryClient({
    defaultOptions: { queries: { retry: 1, refetchOnWindowFocus: false } },
    mutationCache: new MutationCache({
      onError: (err, _vars, _ctx, mutation) => {
        if (!mutation.options.onError && !mutation.meta?.inline) toastError(err);
      },
    }),
  });
}

export function createApp(history?: RouterHistory) {
  const queryClient = createQueryClient();
  const router = createAppRouter(queryClient, history);
  configureApi({
    onUnauthorized: () => {
      queryClient.removeQueries({ queryKey: keys.me });
      const here = router.state.location.href;
      void router.navigate({ to: "/login", search: { redirect: here } });
    },
  });
  return { queryClient, router };
}

export function App({ history }: { history?: RouterHistory }) {
  const [{ queryClient, router }] = useState(() => createApp(history));
  return (
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
        <ThemedToaster />
      </QueryClientProvider>
    </ThemeProvider>
  );
}

function ThemedToaster() {
  const { theme } = useTheme();
  return <Toaster theme={theme} richColors closeButton />;
}
```

`web/src/main.tsx` (replaces the placeholder):

```tsx
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
```

- [ ] **Step 4: Implement the pages**

`web/src/pages/AuthLayout.tsx`:

```tsx
import type { ReactNode } from "react";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";

export function AuthLayout({ title, description, children }: { title: string; description?: string; children: ReactNode }) {
  return (
    <div className="flex min-h-svh items-center justify-center p-6">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle className="text-xl">{title}</CardTitle>
          {description && <CardDescription>{description}</CardDescription>}
        </CardHeader>
        <CardContent>{children}</CardContent>
      </Card>
    </div>
  );
}

export function FormError({ message }: { message: string | null }) {
  if (!message) return null;
  return (
    <p role="alert" className="text-sm text-destructive">
      {message}
    </p>
  );
}
```

`web/src/pages/LoginPage.tsx`:

```tsx
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { getRouteApi, useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { setCsrfToken } from "@/api/client";
import { keys } from "@/api/queries";
import { login } from "@/api/gen/sdk.gen";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/errors";
import { AuthLayout, FormError } from "./AuthLayout";

const route = getRouteApi("/login");

export function LoginPage() {
  const { redirect } = route.useSearch();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const m = useMutation({
    mutationFn: async () => (await login({ body: { username, password } })).data,
    onSuccess: (session) => {
      setCsrfToken(session.csrf_token);
      qc.setQueryData(keys.me, session);
      void navigate({ to: redirect ?? "/" });
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    m.mutate();
  };

  return (
    <AuthLayout title="Log in to FleetOps">
      <form onSubmit={submit} className="grid gap-4">
        <div className="grid gap-2">
          <Label htmlFor="username">Username</Label>
          <Input id="username" autoComplete="username" required value={username} onChange={(e) => setUsername(e.target.value)} />
        </div>
        <div className="grid gap-2">
          <Label htmlFor="password">Password</Label>
          <Input
            id="password"
            type="password"
            autoComplete="current-password"
            required
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
        </div>
        <FormError message={m.error ? errorMessage(m.error) : null} />
        <Button type="submit" disabled={m.isPending}>
          {m.isPending ? "Logging in…" : "Log in"}
        </Button>
      </form>
    </AuthLayout>
  );
}
```

`web/src/pages/SetupPage.tsx`:

```tsx
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { getRouteApi, useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { setCsrfToken } from "@/api/client";
import { keys } from "@/api/queries";
import { setup } from "@/api/gen/sdk.gen";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/errors";
import { AuthLayout, FormError } from "./AuthLayout";

const route = getRouteApi("/setup");

export function SetupPage() {
  const { setupOpen } = route.useRouteContext();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [username, setUsername] = useState("admin");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [mismatch, setMismatch] = useState(false);
  const m = useMutation({
    mutationFn: async () => (await setup({ body: { username, password } })).data,
    onSuccess: (session) => {
      setCsrfToken(session.csrf_token);
      qc.setQueryData(keys.me, session);
      qc.setQueryData(keys.authStatus, { setup_required: false, setup_open: false });
      void navigate({ to: "/" });
    },
  });

  if (!setupOpen) {
    return (
      <AuthLayout title="Setup is locked">
        <p className="text-sm text-muted-foreground">
          No administrator was created within 10 minutes of FleetOps starting, so setup was locked to stop anyone else
          claiming this instance. Restart FleetOps and create the administrator straight away.
        </p>
      </AuthLayout>
    );
  }

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setMismatch(password !== confirm);
    if (password === confirm) m.mutate();
  };

  return (
    <AuthLayout title="Welcome to FleetOps" description="Create the administrator account.">
      <form onSubmit={submit} className="grid gap-4">
        <div className="grid gap-2">
          <Label htmlFor="username">Username</Label>
          <Input id="username" autoComplete="username" required value={username} onChange={(e) => setUsername(e.target.value)} />
        </div>
        <div className="grid gap-2">
          <Label htmlFor="password">Password</Label>
          <Input
            id="password"
            type="password"
            autoComplete="new-password"
            required
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
        </div>
        <div className="grid gap-2">
          <Label htmlFor="confirm">Confirm password</Label>
          <Input
            id="confirm"
            type="password"
            autoComplete="new-password"
            required
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
          />
        </div>
        <FormError message={mismatch ? "The passwords do not match." : m.error ? errorMessage(m.error) : null} />
        <Button type="submit" disabled={m.isPending}>
          {m.isPending ? "Creating…" : "Create administrator"}
        </Button>
      </form>
    </AuthLayout>
  );
}
```

`web/src/pages/misc.tsx`:

```tsx
import { Link } from "@tanstack/react-router";

export function NotFoundPage() {
  return (
    <div className="mx-auto max-w-md p-10 text-center">
      <h1 className="text-xl font-semibold">Page not found</h1>
      <Link to="/" className="mt-4 inline-block text-sm underline">
        Go to the start page
      </Link>
    </div>
  );
}

export function NoEnvironmentsPage() {
  return (
    <div className="mx-auto max-w-md p-10 text-center">
      <h1 className="text-xl font-semibold">No environments</h1>
      <p className="mt-2 text-sm text-muted-foreground">
        FleetOps registers the local Docker socket when it can reach one at start. Check that the socket is mounted and
        readable, then restart FleetOps.
      </p>
    </div>
  );
}
```

`web/src/pages/DashboardPage.tsx`:

```tsx
import { useParams } from "@tanstack/react-router";

/** The environment's start page; its cards arrive in Task 7. */
export function DashboardPage() {
  const { env } = useParams({ strict: false });
  return (
    <div className="grid gap-4 p-6">
      <h1 className="text-2xl font-semibold">{env}</h1>
    </div>
  );
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 50 passed (8 files); typecheck and lint are clean.

- [ ] **Step 6: Commit**

```bash
git add web/src
git commit -m "feat(web): routes with setup and login guards, and the setup and login pages"
```

---

### Task 6: The live provider, the environment shell, and the account page

**Files:**
- Create: `web/src/live/LiveProvider.tsx`, `web/src/auth/session.ts`, `web/src/components/Shell.tsx` and `web/src/pages/AccountPage.tsx`.
- Modify: `web/src/router.tsx`, which gets the `LiveProvider` around the authenticated routes, the `shell` layout and `/account`.
- Test: `web/src/components/Shell.test.tsx` and `web/src/pages/AccountPage.test.tsx`.
- Modify test: `web/src/router.test.tsx` gets the lost-session test.

**Interfaces:**
- Consumes:
  - From Task 2: `WsClient`, `wsUrl`, `ConnState`, `SubSpec` and `SubHandlers`.
  - From Task 3: `EventApplier`, `keys`, `envsQuery`, `envStatusQuery`, `meQuery` and `sessionsQuery`.
  - From Task 4: `useTheme`, `errorMessage` and `formatTime`.
  - From the SDK: `logout`, `changePassword` and `revokeSession`.
- Produces `@/live/LiveProvider`:
  - `<LiveProvider>`
  - `useLive(): { ws: WsClient | null; state: ConnState }`
  - `useSubscription(spec | null, handlers)`: it subscribes while mounted, and again when `spec` changes by value.
  - `useEnvEvents(env)`
- Produces `@/auth/session`: `useSession(): SessionInfo` and `useLogout()`, a mutation that clears the cache and CSRF token and then goes to `/login`.
- Produces `@/components/Shell`: `<Shell>` and `<EnvStatusBadge env>`.
  - `Shell`'s `NAV` array is where P3b adds its pages.
- The route ids become `/authed/shell/envs/$env/` (the dashboard) and `/authed/shell/account`.

- [ ] **Step 1: Write the failing tests**

`web/src/components/Shell.test.tsx`:

```tsx
import { act, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { ADMIN, loggedIn, renderApp } from "@/test/app";

describe("shell", () => {
  it("logs out and returns to the login page", async () => {
    let out = false;
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/me": () => (out ? json(401, { code: "unauthorized", message: "no session" }) : json(200, ADMIN)),
      "POST /api/v1/auth/logout": () => ((out = true), new Response(null, { status: 204 })),
    });
    const { history, screen } = renderApp("/envs/local");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "admin" }));
    await user.click(await screen.findByRole("menuitem", { name: "Log out" }));
    await waitFor(() => expect(history.location.pathname).toBe("/login"));
    expect(calls.find((c) => c.path === "/api/v1/auth/logout")?.headers.get("x-csrf-token")).toBe("csrf-1");
  });

  it("switches the theme from the user menu", async () => {
    mockFetch(loggedIn);
    const { screen } = renderApp("/envs/local");
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "admin" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Dark" }));
    expect(document.documentElement).toHaveClass("dark");
  });

  it("warns while the live connection is down and refetches after it returns", async () => {
    const calls = mockFetch(loggedIn);
    const { screen } = renderApp("/envs/local");
    await screen.findByRole("heading", { name: "local" });
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    act(() => FakeWebSocket.latest().open());
    act(() => FakeWebSocket.latest().drop());
    expect(await screen.findByText(/Lost the live connection/)).toBeInTheDocument();
    const status = () => calls.filter((c) => c.path === "/api/v1/envs/local/status").length;
    const before = status();
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(2), { timeout: 2000 });
    act(() => FakeWebSocket.latest().open());
    await waitFor(() => expect(screen.queryByText(/Lost the live connection/)).toBeNull());
    await waitFor(() => expect(status()).toBeGreaterThan(before));
  });
});
```

`web/src/pages/AccountPage.test.tsx`:

```tsx
import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";

const sessions = [
  { id: 1, created_at: 0, last_seen_at: 1790000000, expires_at: 0, current: true, user_agent: "Firefox", ip: "10.0.0.2" },
  { id: 2, created_at: 0, last_seen_at: 1790000000, expires_at: 0, current: false, user_agent: "Chrome", ip: "10.0.0.3" },
];

describe("account page", () => {
  it("revokes another session", async () => {
    let list = sessions;
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/sessions": () => json(200, list),
      "DELETE /api/v1/auth/sessions/2": () => ((list = [sessions[0]]), new Response(null, { status: 204 })),
    });
    const { screen } = renderApp("/account");
    const user = userEvent.setup();
    const chrome = (await screen.findByText("Chrome")).closest("li")!;
    await user.click(within(chrome).getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(screen.queryByText("Chrome")).toBeNull());
    expect(calls.find((c) => c.method === "DELETE")?.headers.get("x-csrf-token")).toBe("csrf-1");
    expect(within(screen.getByText("Firefox").closest("li")!).getByRole("button", { name: "Log out" })).toBeInTheDocument();
  });

  it("changes the password", async () => {
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/sessions": { json: sessions },
      "PUT /api/v1/auth/password": { status: 204 },
    });
    const { screen } = renderApp("/account");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Current password"), "old-password-1");
    await user.type(screen.getByLabelText("New password"), "new-password-1");
    await user.type(screen.getByLabelText("Confirm new password"), "new-password-1");
    await user.click(screen.getByRole("button", { name: "Change password" }));
    expect(await screen.findByText("Password changed")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "PUT")?.body).toEqual({
      current_password: "old-password-1",
      new_password: "new-password-1",
    });
    expect(screen.getByLabelText("Current password")).toHaveValue("");
  });

  it("shows a wrong current password inline", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/auth/sessions": { json: sessions },
      "PUT /api/v1/auth/password": { status: 401, json: { code: "unauthorized", message: "current password is wrong" } },
    });
    const { history, screen } = renderApp("/account");
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("Current password"), "nope-nope-1");
    await user.type(screen.getByLabelText("New password"), "new-password-1");
    await user.type(screen.getByLabelText("Confirm new password"), "new-password-1");
    await user.click(screen.getByRole("button", { name: "Change password" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("current password is wrong");
    expect(history.location.pathname).toBe("/account");
  });
});
```

`web/src/router.test.tsx`:

```diff
--- a/web/src/router.test.tsx
+++ b/web/src/router.test.tsx
@@ -1,8 +1,9 @@
 import { waitFor } from "@testing-library/react";
+import userEvent from "@testing-library/user-event";
 import { describe, expect, it } from "vitest";
 import { safeRedirect } from "./router";
-import { mockFetch } from "@/test/fetch";
-import { loggedIn, renderApp } from "@/test/app";
+import { json, mockFetch } from "@/test/fetch";
+import { ADMIN, loggedIn, renderApp } from "@/test/app";
 
 const unauthorized = { status: 401, json: { code: "unauthorized", message: "not logged in" } };
 
@@ -46,6 +47,22 @@ describe("routing", () => {
     const { screen } = renderApp("/nope/nothing");
     expect(await screen.findByText("Page not found")).toBeInTheDocument();
   });
+
+  it("sends the user to login when the session is lost mid-use", async () => {
+    let lost = false;
+    mockFetch({
+      ...loggedIn,
+      "GET /api/v1/auth/me": () => (lost ? json(401, unauthorized.json) : json(200, ADMIN)),
+      "GET /api/v1/auth/sessions": () => (lost ? json(401, unauthorized.json) : json(200, [])),
+    });
+    const { history, screen } = renderApp("/envs/local");
+    await screen.findByRole("heading", { name: "local" });
+    lost = true;
+    await userEvent.click(screen.getByRole("button", { name: "admin" }));
+    await userEvent.click(await screen.findByRole("menuitem", { name: "Account" }));
+    await waitFor(() => expect(history.location.pathname).toBe("/login"));
+    expect(history.location.search).toContain("redirect=%2Faccount");
+  });
 });
 
 describe("safeRedirect", () => {
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && pnpm test src/components src/pages/AccountPage.test.tsx src/router.test.tsx`
Expected: 7 FAIL, 7 PASS (the existing router tests). The failures are:
- `Unable to find role="button" and name "admin"`, since there is no user menu yet;
- `Unable to find a label with the text of: Current password`;
- `expected [] to have a length of 1`, since no websocket is opened yet.

- [ ] **Step 3: Implement the live provider and the session hooks**

`web/src/live/LiveProvider.tsx`:

```tsx
import { useQueryClient } from "@tanstack/react-query";
import { createContext, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { keys } from "@/api/queries";
import type { DockerEvent } from "@/api/gen/types.gen";
import { EventApplier } from "./events";
import { WsClient, wsUrl, type ConnState, type SubHandlers, type SubSpec } from "./ws";

interface Live {
  ws: WsClient | null;
  state: ConnState;
}

const LiveContext = createContext<Live>({ ws: null, state: "connecting" });

/**
 * Owns the tab's websocket while a user is logged in. Environment status
 * frames update the status query; after a reconnect every
 * environment-scoped query is refetched, since events may have been missed.
 */
export function LiveProvider({ children }: { children: ReactNode }) {
  const qc = useQueryClient();
  const [state, setState] = useState<ConnState>("connecting");
  const [ws] = useState(
    () =>
      new WsClient({
        url: wsUrl(),
        onState: setState,
        onEnvStatus: (env, status) => qc.setQueryData(keys.envStatus(env), status),
        onReconnect: () => void qc.invalidateQueries({ queryKey: ["env"] }),
      }),
  );

  useEffect(() => {
    ws.start();
    return () => ws.close();
  }, [ws]);

  const live = useMemo(() => ({ ws, state }), [ws, state]);
  return <LiveContext value={live}>{children}</LiveContext>;
}

export function useLive() {
  return useContext(LiveContext);
}

/**
 * Holds a subscription while the component is mounted and `spec` is set.
 * A new subscription is made when `spec` changes by value; the latest
 * handlers are always used.
 */
export function useSubscription(spec: SubSpec | null, handlers: SubHandlers) {
  const { ws } = useLive();
  const ref = useRef(handlers);
  useEffect(() => {
    ref.current = handlers;
  });
  const specKey = spec ? JSON.stringify(spec) : null;
  useEffect(() => {
    if (!ws || !specKey) return;
    const { unsubscribe } = ws.subscribe(JSON.parse(specKey) as SubSpec, {
      onData: (d) => ref.current.onData(d),
      onBinary: (b) => ref.current.onBinary?.(b),
      onGap: (n) => ref.current.onGap?.(n),
      onEnd: (m) => ref.current.onEnd?.(m),
      onError: (m) => ref.current.onError?.(m),
      onResubscribe: () => ref.current.onResubscribe?.(),
    });
    return unsubscribe;
  }, [ws, specKey]);
}

/** Keeps `env`'s queries live from its Docker event stream. */
export function useEnvEvents(env: string) {
  const qc = useQueryClient();
  const [applier] = useState(() => new EventApplier(qc));
  useEffect(() => () => applier.dispose(), [applier]);
  useSubscription({ topic: "events", env }, { onData: (d) => applier.apply(env, d as DockerEvent) });
}
```

`web/src/auth/session.ts`:

```ts
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { setCsrfToken } from "@/api/client";
import { meQuery } from "@/api/queries";
import { logout } from "@/api/gen/sdk.gen";

/** The logged-in session; only used under the authenticated routes, which load it first. */
export function useSession() {
  const { data } = useQuery(meQuery);
  if (!data) throw new Error("useSession outside the authenticated routes");
  return data;
}

export function useLogout() {
  const qc = useQueryClient();
  const navigate = useNavigate();
  return useMutation({
    mutationFn: () => logout(),
    onSettled: () => {
      setCsrfToken(undefined);
      qc.clear();
      void navigate({ to: "/login" });
    },
  });
}
```

- [ ] **Step 4: Implement the shell and the account page, and route them**

`web/src/components/Shell.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { Link, Outlet, useNavigate, useParams } from "@tanstack/react-router";
import { ChevronsUpDown, LayoutDashboard, LogOut, Monitor, Moon, Sun, User } from "lucide-react";
import { envsQuery, envStatusQuery } from "@/api/queries";
import { useLogout, useSession } from "@/auth/session";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useEnvEvents, useLive } from "@/live/LiveProvider";
import { useTheme, type Theme } from "@/theme";

/** The environment-scoped pages, in sidebar order. P3b adds the resource pages. */
const NAV = [{ to: "/envs/$env", label: "Dashboard", icon: LayoutDashboard }] as const;

export function Shell() {
  const { env } = useParams({ strict: false });
  return (
    <div className="flex min-h-svh">
      <aside className="flex w-56 shrink-0 flex-col gap-4 border-r p-3">
        <Link to="/" className="px-2 text-lg font-semibold">
          FleetOps
        </Link>
        <EnvSwitcher current={env} />
        {env && (
          <nav className="grid gap-1" aria-label="Environment">
            {NAV.map(({ to, label, icon: Icon }) => (
              <Link
                key={to}
                to={to}
                params={{ env }}
                activeOptions={{ exact: true }}
                className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm hover:bg-accent data-[status=active]:bg-accent data-[status=active]:font-medium"
              >
                <Icon className="size-4" />
                {label}
              </Link>
            ))}
          </nav>
        )}
        <div className="mt-auto">
          <UserMenu />
        </div>
      </aside>
      <main className="min-w-0 flex-1">
        <ConnectionBanner />
        {env && <EnvLive env={env} />}
        <Outlet />
      </main>
    </div>
  );
}

function EnvLive({ env }: { env: string }) {
  useEnvEvents(env);
  return null;
}

function ConnectionBanner() {
  const { state } = useLive();
  if (state !== "reconnecting") return null;
  return (
    <div role="status" className="bg-destructive/10 px-4 py-2 text-sm text-destructive">
      Lost the live connection to the server. Reconnecting…
    </div>
  );
}

function EnvSwitcher({ current }: { current?: string }) {
  const { data: envs = [] } = useQuery(envsQuery);
  const navigate = useNavigate();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" className="justify-between" aria-label="Environment">
          <span className="truncate">{current ?? "Choose environment"}</span>
          {current ? <EnvStatusBadge env={current} /> : <ChevronsUpDown className="size-4 opacity-50" />}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-52">
        <DropdownMenuLabel>Environments</DropdownMenuLabel>
        {envs.map((e) => (
          <DropdownMenuItem key={e.id} onSelect={() => void navigate({ to: "/envs/$env", params: { env: e.name } })}>
            {e.name}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function EnvStatusBadge({ env }: { env: string }) {
  const { data } = useQuery(envStatusQuery(env));
  if (!data) return null;
  return data.state === "up" ? (
    <Badge variant="secondary">up</Badge>
  ) : (
    <Badge variant="destructive" title={data.reason}>
      down
    </Badge>
  );
}

const THEMES: { value: Theme; label: string; icon: typeof Sun }[] = [
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
  { value: "system", label: "System", icon: Monitor },
];

function UserMenu() {
  const { user } = useSession();
  const { theme, setTheme } = useTheme();
  const logout = useLogout();
  const navigate = useNavigate();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" className="w-full justify-start gap-2">
          <User className="size-4" />
          <span className="truncate">{user.username}</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-52">
        <DropdownMenuLabel>
          {user.username} <span className="font-normal text-muted-foreground">({user.role})</span>
        </DropdownMenuLabel>
        <DropdownMenuItem onSelect={() => void navigate({ to: "/account" })}>Account</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">Theme</DropdownMenuLabel>
        <DropdownMenuRadioGroup value={theme} onValueChange={(v) => setTheme(v as Theme)}>
          {THEMES.map(({ value, label, icon: Icon }) => (
            <DropdownMenuRadioItem key={value} value={value}>
              <Icon className="size-4" />
              {label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => logout.mutate()}>
          <LogOut className="size-4" />
          Log out
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
```

`web/src/pages/AccountPage.tsx`:

```tsx
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { toast } from "sonner";
import { keys, sessionsQuery } from "@/api/queries";
import { changePassword, revokeSession } from "@/api/gen/sdk.gen";
import { useLogout, useSession } from "@/auth/session";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/errors";
import { formatTime } from "@/lib/format";
import { FormError } from "./AuthLayout";

export function AccountPage() {
  const { user } = useSession();
  return (
    <div className="grid max-w-3xl gap-4 p-6">
      <h1 className="text-2xl font-semibold">Account</h1>
      <p className="text-sm text-muted-foreground">
        Signed in as <span className="font-medium text-foreground">{user.username}</span> ({user.role})
      </p>
      <PasswordCard />
      <SessionsCard />
    </div>
  );
}

function PasswordCard() {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [mismatch, setMismatch] = useState(false);
  const m = useMutation({
    mutationFn: () => changePassword({ body: { current_password: current, new_password: next } }),
    onSuccess: () => {
      setCurrent("");
      setNext("");
      setConfirm("");
      toast.success("Password changed");
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    setMismatch(next !== confirm);
    if (next === confirm) m.mutate();
  };
  return (
    <Card>
      <CardHeader>
        <CardTitle>Change password</CardTitle>
      </CardHeader>
      <CardContent>
        <form onSubmit={submit} className="grid max-w-sm gap-4">
          <div className="grid gap-2">
            <Label htmlFor="current-password">Current password</Label>
            <Input
              id="current-password"
              type="password"
              autoComplete="current-password"
              required
              value={current}
              onChange={(e) => setCurrent(e.target.value)}
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="new-password">New password</Label>
            <Input
              id="new-password"
              type="password"
              autoComplete="new-password"
              required
              value={next}
              onChange={(e) => setNext(e.target.value)}
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="confirm-password">Confirm new password</Label>
            <Input
              id="confirm-password"
              type="password"
              autoComplete="new-password"
              required
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
            />
          </div>
          <FormError message={mismatch ? "The passwords do not match." : m.error ? errorMessage(m.error) : null} />
          <Button type="submit" className="justify-self-start" disabled={m.isPending}>
            Change password
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}

function SessionsCard() {
  const qc = useQueryClient();
  const sessions = useQuery(sessionsQuery);
  const logout = useLogout();
  const revoke = useMutation({
    mutationFn: (id: number) => revokeSession({ path: { id } }),
    onSuccess: () => void qc.invalidateQueries({ queryKey: keys.sessions }),
  });
  return (
    <Card>
      <CardHeader>
        <CardTitle>Sessions</CardTitle>
      </CardHeader>
      <CardContent>
        {sessions.error && <FormError message={errorMessage(sessions.error)} />}
        <ul className="grid gap-3" aria-label="Sessions">
          {sessions.data?.map((s) => (
            <li key={s.id} className="flex items-center justify-between gap-4 text-sm">
              <div className="min-w-0">
                <div className="truncate font-medium">
                  {s.user_agent ?? "Unknown browser"}
                  {s.current && <span className="ml-2 text-xs text-muted-foreground">(this session)</span>}
                </div>
                <div className="text-muted-foreground">
                  {s.ip ?? "unknown address"} · last seen {formatTime(s.last_seen_at)}
                </div>
              </div>
              {s.current ? (
                <Button variant="outline" size="sm" onClick={() => logout.mutate()}>
                  Log out
                </Button>
              ) : (
                <Button variant="outline" size="sm" disabled={revoke.isPending} onClick={() => revoke.mutate(s.id)}>
                  Revoke
                </Button>
              )}
            </li>
          ))}
        </ul>
      </CardContent>
    </Card>
  );
}
```

```diff
--- a/web/src/router.tsx
+++ b/web/src/router.tsx
@@ -9,6 +9,9 @@ import {
 } from "@tanstack/react-router";
 import { ApiError } from "@/api/client";
 import { authStatusQuery, envsQuery, meQuery } from "@/api/queries";
+import { Shell } from "@/components/Shell";
+import { LiveProvider } from "@/live/LiveProvider";
+import { AccountPage } from "@/pages/AccountPage";
 import { DashboardPage } from "@/pages/DashboardPage";
 import { LoginPage } from "@/pages/LoginPage";
 import { NoEnvironmentsPage, NotFoundPage } from "@/pages/misc";
@@ -72,7 +75,11 @@ const authedRoute = createRoute({
       throw redirect({ to: "/login", search: { redirect: location.href } });
     }
   },
-  component: Outlet,
+  component: () => (
+    <LiveProvider>
+      <Outlet />
+    </LiveProvider>
+  ),
 });
 
 const indexRoute = createRoute({
@@ -85,8 +92,14 @@ const indexRoute = createRoute({
   component: NoEnvironmentsPage,
 });
 
-export const envRoute = createRoute({
+const shellRoute = createRoute({
   getParentRoute: () => authedRoute,
+  id: "shell",
+  component: Shell,
+});
+
+export const envRoute = createRoute({
+  getParentRoute: () => shellRoute,
   path: "/envs/$env",
   component: Outlet,
 });
@@ -97,10 +110,16 @@ const dashboardRoute = createRoute({
   component: DashboardPage,
 });
 
+const accountRoute = createRoute({
+  getParentRoute: () => shellRoute,
+  path: "/account",
+  component: AccountPage,
+});
+
 const routeTree = rootRoute.addChildren([
   setupRoute,
   loginRoute,
-  authedRoute.addChildren([indexRoute, envRoute.addChildren([dashboardRoute])]),
+  authedRoute.addChildren([indexRoute, shellRoute.addChildren([envRoute.addChildren([dashboardRoute]), accountRoute])]),
 ]);
 
 export function createAppRouter(queryClient: QueryClient, history?: RouterHistory) {
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 57 passed (10 files); typecheck and lint are clean.

- [ ] **Step 6: Commit**

```bash
git add web/src
git commit -m "feat(web): live connection, environment shell and account page"
```

---

### Task 7: The dashboard, and a check against the real server

**Files:**
- Modify: `web/src/pages/DashboardPage.tsx`.
- Test: `web/src/pages/DashboardPage.test.tsx`.

**Interfaces:**
- Consumes:
  - `envStatusQuery`, `infoQuery`, `dfQuery`, `volumesQuery`, `networksQuery` and `recentEventsQuery`;
  - `summarizeDiskUsage`, `formatBytes`, `formatTime` and `errorMessage`;
  - the route id `/authed/shell/envs/$env/`.
- Produces: the dashboard (spec §4.2), which shows:
  - the container counts by state, and the image, volume and network counts;
  - host info;
  - disk usage with the reclaimable amount;
  - the recent-events feed;
  - the down reason when the environment is unavailable.
- Accessible names the tests (and P3d's e2e) use:
  - the stat values are labelled `Containers`, `Images`, `Volumes` and `Networks`;
  - `Disk usage` (a `dl`) and `Recent events` (a `ul`).

- [ ] **Step 1: Write the failing test**

`web/src/pages/DashboardPage.test.tsx`:

```tsx
import { act, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { loggedIn, renderApp } from "@/test/app";

const info = {
  Containers: 3,
  ContainersRunning: 2,
  ContainersPaused: 0,
  ContainersStopped: 1,
  Images: 5,
  ServerVersion: "29.5.2",
  OperatingSystem: "Ubuntu 24.04",
  Architecture: "aarch64",
  NCPU: 2,
  MemTotal: 2_100_000_000,
};

/** The events subscription the dashboard's shell opened, once the socket is open. */
async function eventsSub() {
  await waitFor(() => expect(FakeWebSocket.instances.length).toBeGreaterThan(0));
  const sock = FakeWebSocket.latest();
  act(() => sock.open());
  const sub = sock.frames().find((f) => f.op === "sub" && f.topic === "events");
  expect(sub).toMatchObject({ env: "local" });
  return { sock, sub: sub.sub as number };
}

describe("dashboard", () => {
  it("shows counts, host details and disk usage", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/info": { json: info },
      "GET /api/v1/envs/local/df": { json: { ImageUsage: { TotalSize: 3_000_000_000, Reclaimable: 1_000_000_000 } } },
      "GET /api/v1/envs/local/volumes": { json: [{ name: "v1" }, { name: "v2" }] },
    });
    const { screen } = renderApp("/envs/local");
    await waitFor(() => expect(screen.getByLabelText("Containers")).toHaveTextContent("3"));
    expect(screen.getByText("2 running · 0 paused · 1 stopped")).toBeInTheDocument();
    expect(screen.getByLabelText("Images")).toHaveTextContent("5");
    await waitFor(() => expect(screen.getByLabelText("Volumes")).toHaveTextContent("2"));
    expect(screen.getByText("29.5.2")).toBeInTheDocument();
    expect(within(screen.getByLabelText("Disk usage")).getByText("3.0 GB (1.0 GB reclaimable)")).toBeInTheDocument();
  });

  it("updates counts and the event feed live from Docker events", async () => {
    let running = 2;
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/info": () => json(200, { ...info, ContainersRunning: running }),
    });
    const { screen } = renderApp("/envs/local");
    await screen.findByText("2 running · 0 paused · 1 stopped");
    const { sock, sub } = await eventsSub();
    running = 3;
    act(() =>
      sock.receive({
        sub,
        type: "data",
        data: { kind: "container", action: "start", id: "c1", name: "web", attributes: {}, time: 1790000000 },
      }),
    );
    expect(await screen.findByText("container start")).toBeInTheDocument();
    expect(await screen.findByText("3 running · 0 paused · 1 stopped")).toBeInTheDocument();
  });

  it("shows why an environment is down", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/status": { json: { state: "down", reason: "socket not found" } } });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByRole("alert")).toHaveTextContent("socket not found");
  });

  it("follows env_status frames from the server", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/info": { json: info } });
    const { screen } = renderApp("/envs/local");
    await screen.findByText("29.5.2");
    const { sock } = await eventsSub();
    act(() => sock.receive({ type: "env_status", env: "local", status: { state: "down", reason: "daemon stopped" } }));
    expect(await screen.findByRole("alert")).toHaveTextContent("daemon stopped");
  });

  it("shows the error when engine info fails", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/info": { status: 502, json: { code: "upstream", message: "Docker said no" } },
    });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByText("Docker said no", {}, { timeout: 3000 })).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd web && pnpm test src/pages/DashboardPage.test.tsx`
Expected: 5 FAIL. The failures include `Unable to find a label with the text of: Containers` and `Unable to find role="alert"`.

- [ ] **Step 3: Implement the dashboard**

`web/src/pages/DashboardPage.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { dfQuery, envStatusQuery, infoQuery, networksQuery, recentEventsQuery, volumesQuery } from "@/api/queries";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { summarizeDiskUsage, type DiskSummary, type Usage } from "@/lib/df";
import { errorMessage } from "@/lib/errors";
import { formatBytes, formatTime } from "@/lib/format";

const route = getRouteApi("/authed/shell/envs/$env/");

export function DashboardPage() {
  const { env } = route.useParams();
  const status = useQuery(envStatusQuery(env));
  const info = useQuery(infoQuery(env));
  const df = useQuery(dfQuery(env));
  const volumes = useQuery(volumesQuery(env));
  const networks = useQuery(networksQuery(env));
  const events = useQuery(recentEventsQuery(env));

  if (status.data?.state === "down") {
    return (
      <Page title={env}>
        <p role="alert" className="text-sm text-destructive">
          This environment is unavailable: {status.data.reason}
        </p>
      </Page>
    );
  }
  const i = info.data;
  const d = df.data;

  return (
    <Page title={env}>
      {info.error && (
        <p role="alert" className="text-sm text-destructive">
          {errorMessage(info.error)}
        </p>
      )}
      <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <Stat title="Containers" value={i?.Containers}>
          {i && (
            <span>
              {i.ContainersRunning ?? 0} running · {i.ContainersPaused ?? 0} paused · {i.ContainersStopped ?? 0} stopped
            </span>
          )}
        </Stat>
        <Stat title="Images" value={i?.Images} />
        <Stat title="Volumes" value={volumes.data?.length} />
        <Stat title="Networks" value={networks.data?.length} />
      </div>
      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>Host</CardTitle>
          </CardHeader>
          <CardContent>
            <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1 text-sm">
              <Row k="Engine" v={i?.ServerVersion} />
              <Row k="OS" v={i?.OperatingSystem} />
              <Row k="Architecture" v={i?.Architecture} />
              <Row k="CPUs" v={i?.NCPU} />
              <Row k="Memory" v={i ? formatBytes(i.MemTotal) : undefined} />
            </dl>
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle>Disk usage</CardTitle>
          </CardHeader>
          <CardContent>
            {d ? <DiskUsageList df={summarizeDiskUsage(d)} /> : <p className="text-sm text-muted-foreground">—</p>}
          </CardContent>
        </Card>
      </div>
      <Card>
        <CardHeader>
          <CardTitle>Recent events</CardTitle>
        </CardHeader>
        <CardContent>
          {events.data?.length ? (
            <ul className="grid gap-1 text-sm" aria-label="Recent events">
              {events.data.map((e, n) => (
                <li key={`${e.time}-${e.id}-${e.action}-${n}`} className="flex gap-3">
                  <span className="shrink-0 text-muted-foreground tabular-nums">{formatTime(e.time)}</span>
                  <span className="shrink-0 font-medium">
                    {e.kind} {e.action}
                  </span>
                  <span className="truncate text-muted-foreground">{e.name ?? e.id.slice(0, 12)}</span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-sm text-muted-foreground">Events appear here as they happen.</p>
          )}
        </CardContent>
      </Card>
    </Page>
  );
}

function Page({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="grid gap-4 p-6">
      <h1 className="text-2xl font-semibold">{title}</h1>
      {children}
    </div>
  );
}

function Stat({ title, value, children }: { title: string; value: number | undefined; children?: ReactNode }) {
  return (
    <Card className="gap-2">
      <CardHeader>
        <CardTitle className="text-sm font-medium text-muted-foreground">{title}</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-1">
        <span className="text-3xl font-semibold tabular-nums" aria-label={title}>
          {value ?? "—"}
        </span>
        {children && <span className="text-xs text-muted-foreground">{children}</span>}
      </CardContent>
    </Card>
  );
}

function DiskUsageList({ df }: { df: DiskSummary }) {
  const rows: [string, Usage][] = [
    ["Images", df.images],
    ["Containers", df.containers],
    ["Volumes", df.volumes],
    ["Build cache", df.buildCache],
  ];
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1 text-sm" aria-label="Disk usage">
      {rows.map(([k, u]) => (
        <Row
          key={k}
          k={k}
          v={u.reclaimable === undefined ? formatBytes(u.size) : `${formatBytes(u.size)} (${formatBytes(u.reclaimable)} reclaimable)`}
        />
      ))}
    </dl>
  );
}

function Row({ k, v }: { k: string; v: ReactNode }) {
  return (
    <>
      <dt className="text-muted-foreground">{k}</dt>
      <dd>{v ?? "—"}</dd>
    </>
  );
}
```

- [ ] **Step 4: Run all checks**

Run: `make test-web && cargo test --workspace && make lint`
Expected:
- `make test-web` passes: 62 web tests in 11 files, with a clean typecheck, lint and `check:api`.
- The Rust workspace still passes: 240 tests.
- Rust lint is clean.

- [ ] **Step 5: Check it against the real server**

Run it in two terminals: `make web && make dev-server` in one, and a browser pointed at `http://127.0.0.1:9000/` in the other. Use a fresh `./.dev-data`: `rm -rf .dev-data` first.

Expected:
- `/` redirects to `/setup`. Creating the administrator lands on `/envs/local`.
- The dashboard shows the engine version and the counts. Disk usage shows sizes, not `0 B`.
- `docker run --rm -d --name p3a-probe alpine sleep 30` shows `container create` and `container start` in Recent events within a second, and the running count goes up.
- Healthcheck `exec_*` events do not appear.
- Stopping the server (Ctrl-C) shows "Lost the live connection…".
  - The banner stays while the server is down.
  - After `make dev-server` again, it clears without a page reload.
- The browser console has no CSP violations.

Then check the dev proxy: run `make dev` and open `http://localhost:5173/`. The same Recent events update works through Vite's websocket proxy.

- [ ] **Step 6: Commit**

```bash
git add web/src/pages
git commit -m "feat(web): live dashboard with counts, host, disk usage and recent events"
```

---

## Deferred (tracked)

- **P3b:**
  - containers list, detail, create, recreate and duplicate;
  - images (with pull and build progress over the `task` topic), volumes, networks and compose projects;
  - the admin pages: users, environments and audit;
  - the command palette and keyboard shortcuts;
  - per-route code splitting (the bundle is ~520 kB; `chunkSizeWarningLimit` is raised to 1024 until then).
- **P3c:**
  - The log viewer (virtualized, 200k-line ring buffer, `gap` markers), the terminal (xterm.js over binary exec frames, resize) and the stats charts.
  - Logs and exec use `SubHandlers.onResubscribe` to reset their view after a reconnect, since resubscribing replays `tail` lines or starts a new exec.
- **P3d:**
  - Playwright e2e for the spec §8 flow;
  - `make image` (multi-arch musl, distroless);
  - CI running `make lint`, `make test`, `make test-docker`, `make test-web` (including `check:api`), `make openapi && git diff --exit-code`, e2e and the image build.
- **Development only:** React StrictMode logs "WebSocket is closed before the connection is established" once per page load in development, because StrictMode mounts, unmounts and remounts the provider. Production builds do not do this.
- Events invalidate rather than patch cached lists (see the spec deltas).
- **P2c minors still open:**
  - 405 responses have an empty body.
  - `fleetops --openapi` fails when an env var is invalid, and panics on a broken pipe.
  - A plain `.route()` could add an undocumented route.
  - An oversized build upload gets 400, not 413.
  - `ForceQuery.volumes` is documented where it is ignored.
  - There is rustdoc link syntax in `ClientMsg.opts`.
  - Two raw `oneshot` calls bypass the contract checker.
- The P2b, P2a and P1 minors are unchanged from the P2c plan's Deferred section.
