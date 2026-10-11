# Flexma: notes for agents working on this repo

Flexma is a self-hosted, Figma-like collaborative design tool. It has three Rust crates, a React web app, and an agent skill. See README.md for the overview.

## Layout

| Path | What it holds |
|---|---|
| `crates/flexma-doc` | The document model, shared by the server and the wasm engine. Change the node schema here only. The `define_node!` macro in `node.rs` is the single list of node properties; serialization, `set_prop` and `get_prop` derive from it. |
| `crates/flexma-engine` | The wasm editor. `editor.rs` holds state and the local/remote op pipeline. Interaction is in `tools.rs`, commands in `commands.rs`, overlays in `overlay.rs`, WebGL2 in `render.rs` and `gl.rs`, and the JS API in `lib.rs`. |
| `crates/flexma-server` | The axum server. `rooms.rs` has one actor per open document. `mcp.rs` is the MCP tools, `oauth.rs` is the OAuth 2.1 server for MCP clients, and the `api_*.rs` files are REST. |
| `web/src/editor` | Editor UI. `store.ts` bridges engine and socket into React via version counters. |
| `web/src/{pages,components,lib}` | Dashboard, auth and settings. |
| `skills/flexma` | The skill served at `/agents/skills/flexma/*` and embedded in the server binary with `include_str!`. Rebuild the server after editing it. |

## Commands

```bash
cargo test                                   # model, sync and layout tests
./scripts/build-wasm.sh                      # engine → web/src/engine/pkg (needed before the web build)
cd web && npx tsc -b && npm run build        # typecheck + bundle
cargo run -p flexma-server                   # http://localhost:8080 (serves web/dist)
```

## Invariants

- Every document change is an `Op` (`create`/`update`/`delete`), applied through `Document::apply`.
  - The server is authoritative and echoes ops back as acks.
  - Last writer wins per property.
- Instance children are derived state. `flexma_doc::components::sync_instances` expands them on every replica, with ids `instanceId~sourceId`, so they are never stored or sent.
  - Ops aimed at derived nodes must go through `components::route_ops`, which turns them into `overrides` updates on the instance. Both `Editor::apply_local` and the server do this.
- Auto layout (`flexma_doc::layout`) is derived state. Every replica recomputes it locally after applying ops, and it is never sent as ops.
- Sibling order uses fractional-index strings (`index`); ties are broken by node id.
- Local edits go through `Editor::apply_local`, which records undo, queues the op for sync and re-runs layout. Remote ops go through `apply_remote`.
- Agent (MCP) edits go through `RoomHandle::mutate`, the same pipeline as people, so they broadcast live.
- The wasm-bindgen crate version is pinned (`=0.2.129`) and must match the CLI.
