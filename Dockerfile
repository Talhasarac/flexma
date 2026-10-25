# syntax=docker/dockerfile:1
# Flexma: one image with the Rust server, the wasm engine and the web app.

# ---- Rust: server binary + wasm engine ----
FROM rust:1-bookworm AS rust
RUN rustup target add wasm32-unknown-unknown \
 && cargo install wasm-bindgen-cli --version 0.2.129 --locked
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY skills skills
RUN cargo build -p flexma-engine --target wasm32-unknown-unknown --profile wasm-release \
 && wasm-bindgen --target web --out-dir /wasm-pkg --out-name flexma_engine \
      target/wasm32-unknown-unknown/wasm-release/flexma_engine.wasm \
 && cargo build -p flexma-server --release

# ---- Web app ----
FROM node:22-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web ./
COPY --from=rust /wasm-pkg ./src/engine/pkg
RUN npm run build

# ---- Runtime ----
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home /data flexma \
 && mkdir -p /data && chown flexma /data
COPY --from=rust /src/target/release/flexma /usr/local/bin/flexma
COPY --from=web /web/dist /app/web
ENV FLEXMA_ADDR=0.0.0.0:8080 \
    FLEXMA_DATA_DIR=/data \
    FLEXMA_WEB_DIR=/app/web \
    RUST_LOG=flexma=info,tower_http=warn
USER flexma
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s CMD curl -fsS http://127.0.0.1:8080/api/health || exit 1
CMD ["flexma"]
