# Flexma: notes for agents working on this repo

Flexma is a self-hosted, Figma-like collaborative design tool. It has three Rust crates, a React web app, and an agent skill. See README.md for the overview.

## Layout

| Path | What it holds |
|---|---|
| `crates/flexma-doc` | The document model, shared by the server and the wasm engine. Change the node schema here only. The `define_node!` macro in `node.rs` is the single list of node properties; serialization, `set_prop` and `get_prop` derive from it. |
| `crates/flexma-engine` | The wasm editor. `editor.rs` holds state and the local/remote op pipeline. Interaction is in `tools.rs`, commands in `commands.rs`, overlays in `overlay.rs`, and the JS API in `lib.rs`. `render.rs` decides what to draw and hands it to a `backend.rs` backend: WebGPU in `gpu.rs` (WGSL in `shaders/`) or the WebGL2 fallback in `gl.rs`. Keep the two backends' shaders in step. |
| `crates/flexma-server` | The axum server. `rooms.rs` has one actor per open document. `mcp.rs` is the MCP tools, `oauth.rs` is the OAuth 2.1 server for MCP clients, and the `api_*.rs` files are REST. `paint.rs` stores brush strokes and tile snapshots. |
| `web/src/editor` | Editor UI. `store.ts` bridges engine and socket into React via version counters. |
| `web/src/{pages,components,lib}` | Dashboard, auth and settings. |
| `skills/flexma` | The skill served at `/agents/skills/flexma/*` and embedded in the server binary with `include_str!` (`api_tokens.rs`). Rebuild the server after editing it. |
| `skills/unity-ui-to-flexma` | An optional skill (Unity uGUI export, with the editor scripts it installs), served the same way and installed by `/agents/install/unity-ui-to-flexma.sh` or `.ps1`. Every file in a skill folder must be listed in `api_tokens.rs`. |

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
- Shared styles are derived state too. STYLE nodes live at the document root (`Document::pages()` lists only pages), and layers link to them with `fillStyleId` / `strokeStyleId` / `textStyleId`. `flexma_doc::styles::sync_styles` copies style values in on every replica after `components::sync_instances`.
  - Ops go through `styles::route_ops` before `components::route_ops`, so editing a styled prop directly detaches that style.
- Auto layout (`flexma_doc::layout`) is derived state. Every replica recomputes it locally after applying ops, and it is never sent as ops.
- Sibling order uses fractional-index strings (`index`); ties are broken by node id.
- Local edits go through `Editor::apply_local`, which records undo, queues the op for sync and re-runs layout. Remote ops go through `apply_remote`.
- Agent (MCP) edits go through `RoomHandle::mutate`, the same pipeline as people, so they broadcast live.
- Documents are stored and loaded in parts (`flexma_doc::parts`): a shared part (pages, styles, every component with its layers) and one per page, stored as `shared` and `page:<id>`.
  - Rooms save only the parts whose JSON changed (`doc_store`), and write the whole document to `files.document` when they close, for older servers. Parts are the document only while `files.parts_version` equals `files.version`. Read stored documents through `doc_store`, never `files.document`.
  - Editors open with `?doc=zlib&pages=stream` (`parts::stream`): the page being opened comes first, the other pages follow as frames that the engine merges (`merge_zlib`). Commands that touch every page wait until they have all arrived.
  - A page's frame repeats its components, and the server's copy replaces the one from the first frame. When the last page is in (`finish_stream`), the editor applies its unacknowledged edits again, in order, and only then loads the paint state.
  - An editor reconnecting with unacknowledged edits asks for the whole document and replays them on it. Edits made while it connects wait in `Editor::held_ops` until every page is in, and its other edits aren't sent before them.
- One room per open file (`Rooms`). A room leaves the map only once its Shutdown is queued. `Rooms::get` waits for a closing room's last save, and re-reads a file that was restored or deleted while it read it (`Rooms::epoch`).
- The wasm-bindgen crate version is pinned (`=0.2.129`) and must match the CLI.
- Paint layer pixels (PAINT nodes) are not in the document. `flexma_doc::raster` defines strokes and the dab placement every client shares. The room numbers strokes (`seq`), relays them live (`strokeLive`), stores them in `paint_strokes`, and broadcasts the finished ones (`stroke`, `strokeUndo`). The engine (`paint.rs`) stamps dabs into 256 px tiles in that order, and redraws a tile from its base snapshot when the order disagrees with what it drew. Browsers upload tile PNGs (`base`: strokes replay onto it; `view`: the latest look, for exports) to `data/paint/`. Change dab placement only together with existing files in mind, since replays must keep matching the saved tiles (`round_dabs_are_pinned` guards round brushes).
  - Textured brushes stamp built-in images from `web/public/brushes/` (ids in `raster::TIPS` / `TEXTURES`). Strokes name them by id, so a shipped image never changes: a new look gets a new id. Jitter is seeded by the stroke id, so it is the same on every client.
  - Paint GPU work is batched per frame (`Batches` in `paint.rs`): dabs for all tiles go in one submission, and only a smudge forces a flush. Paint events wake their own flag, never the renderer's (that one rebuilds the whole display list). `stats()` reports `paintPasses`, `paintSubmits` and `rebuilds` for checking this.
  - Smudge strokes read the layer under each dab (`Backend::smudge`), so a tile they touch is only redrawn together with every tile they touch (`smudge_closure`). Known limit: when a neighbor's snapshot is newer than the smudge stroke, a reload reads that newer neighbor, which can differ slightly from the painter's live view along the tile edge.
- The editor draws with WebGPU when the browser has it and falls back to WebGL2 (`web/src/engine/index.ts`). `?renderer=webgl` or `?renderer=webgpu` forces one; after a WebGPU failure a browser remembers to use WebGL2 until `?renderer=webgpu`.
