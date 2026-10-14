# syntax=docker/dockerfile:1
FROM node:24-bookworm-slim AS web
WORKDIR /build/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

FROM rust:1.97-bookworm AS backend
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
RUN cargo build --release --locked

FROM rclone/rclone:1.75.2 AS rclone
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /config/rclone /config/service /media
COPY --from=backend /build/target/release/rclone-browser-ng /usr/local/bin/
COPY --from=rclone /usr/local/bin/rclone /usr/local/bin/rclone
COPY --from=web /build/web/dist /app/web
ENV BIND_ADDR=0.0.0.0:8080 WEB_ROOT=/app/web RCLONE_CONFIG=/config/rclone/rclone.conf DATA_DIR=/config/service
EXPOSE 8080
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/rclone-browser-ng"]
