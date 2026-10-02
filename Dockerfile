# Multi-stage build for Vyasa.
#
# Not a scratch/musl image: wasmtime and Tantivy both want a real libc, and
# a statically linked build of them is more maintenance than the size saving
# is worth here. The runtime stage is debian-slim with only ca-certificates
# and tzdata added.
FROM rust:1.99-slim AS builder
WORKDIR /build
# Build dependencies for the native crates in the tree (ring, tantivy).
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY themes-starter ./themes-starter
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

# Uploads and the search index are the only writable state; both belong on
# a volume so an image upgrade does not discard them.
RUN useradd --system --uid 10001 vyasa \
    && mkdir -p /opt/vyasa/media /opt/vyasa/index \
    && chown -R vyasa:vyasa /opt/vyasa
VOLUME ["/opt/vyasa/media", "/opt/vyasa/index"]
USER vyasa

EXPOSE 3000
ENTRYPOINT ["vyasa"]
CMD ["serve"]
