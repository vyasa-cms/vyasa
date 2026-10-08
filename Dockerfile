# Multi-stage build for Vyasa.
#
# Not a scratch/musl image: wasmtime and Tantivy both want a real libc, and
# a statically linked build of them is more maintenance than the size saving
# is worth here. The runtime stage is debian-slim with only ca-certificates
# and tzdata added.
# Bookworm, like the runtime stage: a newer glibc in the builder can leave
# the binary needing symbols the runtime does not have.
FROM rust:1.99-slim-bookworm AS builder
WORKDIR /build
# Build dependencies for the native crates in the tree (ring, tantivy).
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# Compiled into the binary: the starter themes and the logo marks.
COPY themes-starter ./themes-starter
COPY assets ./assets
RUN cargo build --release --bin vyasa

FROM node:22-slim AS admin-builder
WORKDIR /admin
# pnpm-workspace.yaml carries the dependency overrides (security pins) the
# lockfile was resolved with; without it a frozen install refuses.
COPY admin/package.json admin/pnpm-lock.yaml admin/pnpm-workspace.yaml ./
RUN npm install -g pnpm@11 && pnpm install --frozen-lockfile
COPY admin/ .
RUN pnpm build

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*

# The server reads the admin bundle and themes from its working directory,
# so it runs from a fixed prefix rather than from /.
WORKDIR /opt/vyasa
COPY --from=builder /build/target/release/vyasa /usr/local/bin/vyasa
COPY --from=admin-builder /admin/dist /opt/vyasa/admin/dist
COPY themes-starter /opt/vyasa/themes-starter

# Uploads and the search index are the only durable state; mount volumes
# there (docker-compose.yml does), or point VYASA_MEDIA_DIR and
# VYASA_INDEX_DIR elsewhere. Nothing else is written outside VYASA_RUN_DIR,
# so the image runs with a read-only root filesystem and a tmpfs on /tmp.
RUN useradd --system --uid 10001 vyasa \
    && mkdir -p /opt/vyasa/media /opt/vyasa/index \
    && chown -R vyasa:vyasa /opt/vyasa
ENV VYASA_RUN_DIR=/tmp/vyasa-run
USER vyasa

EXPOSE 3000
ENTRYPOINT ["vyasa"]
CMD ["serve"]
