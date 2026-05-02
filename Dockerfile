# syntax=docker/dockerfile:1.7
#
# Multi-stage build:  frontend bundle → cargo release → distroless runtime.
#
# Final base is gcr.io/distroless/cc-debian12:nonroot rather than `scratch`
# because the binary links dynamically to glibc + libssl at runtime: rustls
# pulls `ring` (compiled C) and the kube-rs HTTP client follows the system
# certificate trust store. Cross-compiling to musl would buy ~5 MB but adds
# a toolchain to maintain that this single-target homelab project doesn't
# need. distroless/cc-debian12:nonroot ships glibc + ca-certificates and
# uses uid 65532; nothing else.

# -- Stage 1: frontend-builder ----------------------------------------------
# node:22-slim uses Debian's glibc, matching what pnpm's native bindings
# expect. -alpine flips to musl and occasionally trips on the
# `unrs-resolver` postinstall step; -slim is the safer default here.
FROM node:22-slim AS frontend-builder
WORKDIR /workspace/frontend
RUN corepack enable
COPY frontend/package.json frontend/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY frontend/ ./
RUN pnpm build
# produces /workspace/frontend/out

# -- Stage 2: backend-builder -----------------------------------------------
# `rust-embed`'s `#[folder = "../frontend/out"]` resolves relative to
# CARGO_MANIFEST_DIR. We mirror the repo layout inside the builder so the
# relative path lines up exactly with what dev sees on disk.
FROM rust:1-bookworm AS backend-builder
WORKDIR /workspace/backend
COPY backend/ ./
COPY --from=frontend-builder /workspace/frontend/out /workspace/frontend/out
RUN cargo build --release --locked --features embed
# produces /workspace/backend/target/release/anvil

# -- Stage 3: runtime -------------------------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=backend-builder /workspace/backend/target/release/anvil /usr/local/bin/anvil
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/anvil"]
