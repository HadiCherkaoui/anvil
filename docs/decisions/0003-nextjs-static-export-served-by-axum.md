# ADR 0003: Next.js Static Export Served by Axum

**Date:** 2026-05-02
**Status:** Accepted

## Context

The panel needs a web UI. Two-server setups (separate Node frontend + Rust backend) add
deploy complexity for a homelab tool: two containers, sidecar gymnastics, CORS or proxy
config. We want a **single deployable binary**.

We need a frontend stack that:
- Has an excellent component ecosystem (forms, dialogs, tables, charts later)
- Doesn't require a Node.js runtime in production
- Builds to static HTML/JS/CSS
- Co-exists with a Rust backend that serves the API

## Decision

**Next.js with App Router and `output: 'export'`**, built into `./frontend/out/`. The Rust
binary serves the static bundle:

- **Dev mode** — `tower-http::services::ServeDir` reads from `./frontend/out` on disk.
  `pnpm dev` runs Next's dev server on a separate port for HMR; the Rust API is consumed via
  cross-origin or proxied.
- **Release mode** — `rust-embed` (or `include_dir`) bakes the bundle into the binary at
  compile time. One file ships.

Both modes are gated by a Cargo feature `embed-frontend` (enabled by default in `--release`).

## Rationale

- Next.js static export gives us the React + App Router DX without needing a Node runtime in
  production.
- Single binary = trivial deployment (one Pod, no sidecar, no separate frontend image).
- `rust-embed` adds negligible binary-size cost for ~5 MB of static assets.
- `ServeDir` in dev means the frontend rebuild loop (`pnpm dev`) doesn't require recompiling
  Rust.

## Consequences / Gotchas

- **No `app/api/*` routes.** All data fetching is client-side against the Rust `/api/*`
  endpoints.
- **No Server Components data fetching.** Components that fetch must be `'use client'` and
  use TanStack Query (or `fetch` in a `useEffect`).
- **No `next/image` optimization.** Configure `images: { unoptimized: true }` in
  `next.config.ts`. Use `<img>` or static-imported images directly.
- **No middleware.** All auth happens on the Rust side (M4).
- **SPA fallback required.** Any unmatched non-`/api` GET returns `index.html` so
  client-side routing works on direct URL access (e.g., `/servers/foo`). Implement as the
  axum fallback handler.
- **Build pipeline order matters.** `pnpm build` (frontend) MUST run before
  `cargo build --release --features embed-frontend` (backend). The `Dockerfile` and CI
  encode this order.
- **Trailing slashes / dynamic segments at export time** — App Router supports static export
  for dynamic routes only via `generateStaticParams` (or kept fully client-side). The plan is
  to keep dynamic routes (e.g., `/servers/[id]`) client-rendered with the data fetched via
  TanStack Query.
