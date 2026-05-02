# syntax=docker/dockerfile:1.7
#
# anvil multi-stage build — SCAFFOLD ONLY (M0). Does not build yet.
# M1 wires up the backend stage; M3 enables embed-frontend and the working flow.
#
# Final image plan: distroless/cc:nonroot + the anvil binary (Next.js static export
# is embedded into the binary via rust-embed — see ADR 0003).
# distroless/cc carries glibc + ca-certificates + a nonroot user; it's the right base
# for default-target Rust binaries that don't need a shell.

# -- Stage 1: frontend-builder --
# FROM node:22-slim AS frontend-builder
# WORKDIR /app
# COPY frontend/package.json frontend/pnpm-lock.yaml ./
# RUN corepack enable && pnpm install --frozen-lockfile
# COPY frontend/ ./
# RUN pnpm build
# # produces /app/out

# -- Stage 2: backend-builder --
# FROM rust:1 AS backend-builder
# WORKDIR /build
# COPY backend/ ./
# COPY --from=frontend-builder /app/out ./frontend-out
# RUN cargo build --release --locked --features embed-frontend
# # produces /build/target/release/anvil

# -- Stage 3: runtime --
# FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
# COPY --from=backend-builder /build/target/release/anvil /anvil
# USER nonroot
# EXPOSE 3000
# ENTRYPOINT ["/anvil"]

# Sentinel so this Dockerfile is parseable. Replace with the runtime stage above in M1.
FROM scratch
