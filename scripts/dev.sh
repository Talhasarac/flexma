#!/bin/sh
# Build everything and run Flexma locally on http://localhost:8080 (data in ./data).
# For UI work with hot reload, also run `npm run dev` in web/ and open http://localhost:5173.
set -e
cd "$(dirname "$0")/.."
./scripts/build-wasm.sh
[ -d web/node_modules ] || (cd web && npm install)
(cd web && npm run build)
exec cargo run --release -p flexma-server
