#!/bin/sh
# Build the Flexma engine to WebAssembly and generate JS bindings into web/src/engine/pkg.
# Requires: rustup target add wasm32-unknown-unknown; cargo install wasm-bindgen-cli --version <same as Cargo.toml>
# Optional: wasm-opt (binaryen) for a smaller, faster module.
set -e
cd "$(dirname "$0")/.."
PROFILE=${PROFILE:-wasm-release}
cargo build -p flexma-engine --target wasm32-unknown-unknown --profile "$PROFILE"
OUT=web/src/engine/pkg
mkdir -p "$OUT"
wasm-bindgen --target web --out-dir "$OUT" --out-name flexma_engine \
  "target/wasm32-unknown-unknown/$PROFILE/flexma_engine.wasm"
if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals \
    "$OUT/flexma_engine_bg.wasm" -o "$OUT/flexma_engine_bg.wasm"
fi
ls -la "$OUT"
