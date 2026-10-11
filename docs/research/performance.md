# Flexma performance guide

Stack: Rust (axum/tokio/SQLite) server, Rust-to-wasm core (document model, scene graph, hit testing, WebGL2 renderer), React chrome. Reference design: Figma (C++/wasm, custom tile-based WebGL renderer, server-authoritative multiplayer with last-writer-wins per property).

Provenance: statements with a source link were read in the cited primary source. Items marked **[judgement]** are engineering estimates or inferences, not measured here; benchmark before relying on them. See "Gaps" at the end.

## Executive summary: top 15 recommendations

- [ ] **1. The wasm heap owns the document.** Arena/struct-of-arrays with `u32` node ids; JS and React hold ids and versions, never node objects (Figma keeps objects in a pre-allocated typed array, so JS GC is out of the frame loop).
- [ ] **2. Cross the JS/wasm boundary rarely, with scalars.** One call per input event, one per frame; bulk data via pointer+length typed-array views. No serde/JSON/`JsValue`/`String` in hot paths; create views at point of use (memory growth detaches them).
- [ ] **3. Render only when dirty, one rAF.** Rust calls `schedule_frame()` on the clean-to-dirty edge; JS owns the single `requestAnimationFrame`. React is never in the canvas loop.
- [ ] **4. Instance everything simple.** Rect/rrect/ellipse/line: one SDF shader, 64-byte instances, one instanced draw per pipeline run, analytic AA in the fragment shader.
- [ ] **5. Paths: tessellate once, cache.** `lyon` per node, re-tessellated on geometry or zoom-bucket change; draw paths into a 4x MSAA offscreen target.
- [ ] **6. WebGL hygiene.** State cache, VAOs, one UBO, `texStorage`, `bufferSubData` with geometric growth, no `get*`/`getError`/sync `readPixels` in release, parallel shader compile; handle context loss, DPR and a back-buffer pixel cap.
- [ ] **7. Tile cache plus LOD.** 256 to 512 px tiles aligned to power-of-two zoom levels, LRU under a byte budget; composite cached tiles during gestures, refine when idle; drop shadows/blur/dashes/text outlines below zoom thresholds.
- [ ] **8. Spatial index plus camera-relative math.** `rstar` bulk-loaded AABBs for hit testing/marquee, flat AABB array for per-frame culling; compose in f64, upload camera-relative f32.
- [ ] **9. Text.** Shape in Rust with explicitly loaded fonts; glyph atlas (alpha masks) + instanced quads; MSDF later.
- [ ] **10. React on a versioned external store.** `useSyncExternalStore`, primitive selectors, per-node subscriptions, virtualized layer list from a flat wasm buffer, drag-time panel updates coalesced to the frame.
- [ ] **11. Multiplayer semantics.** Server-authoritative LWW per (object, property); parent+position as one property; fractional-index strings with jitter; ~33 ms client batches; ignore server echoes for props with unacked local edits.
- [ ] **12. Wire and server.** Compact schema-based binary, presence on a lossy channel at <=30 Hz, one actor task per document, bounded per-client queues with resync on lag.
- [ ] **13. Persistence.** In-memory document; SQLite-WAL op journal flushed ~every 500 ms; compressed checkpoint every 30 to 60 s and on last disconnect; recovery = checkpoint + replay.
- [ ] **14. Load path.** `compileStreaming` with `application/wasm`, immutable hashed URL, brotli, service-worker precache, document fetched in parallel with wasm, viewport-first render.
- [ ] **15. Build and measure.** LTO + `panic=abort` + `wasm-opt`, size tracked in CI; CI records p50/p95 frame and pointer-to-commit times on fixed 10k/100k-node documents (Compositor's benchmark method, section 8).

---

## 1. Rust to wasm

**Boundary design**
- Primitive calls are cheap; copies and allocations are not. One microbenchmark study reported roughly 50 to 100 ns per primitive wasm-bindgen call versus 1 to 3 ms to copy 1 MB (single paper, unverified methodology; treat as order of magnitude).
- The Rust and WebAssembly book is explicit that wasm only loads/stores numbers and JS can read wasm linear memory directly; wasm-bindgen exists to automate the copying patterns, which is exactly what to avoid in hot paths.
- Events in, scalars only: `engine.pointer_event(kind, x, y, mods, t)`. Use `PointerEvent.getCoalescedEvents()` only for freehand paths; for drags only the last position matters.
- Data out, views not copies: Rust fills a buffer it owns and returns `*const u32` and a length; JS wraps it with `new Uint32Array(wasm.memory.buffer, ptr, len)`.
- Data in (bulk): write into a Rust-owned scratch region from JS, then call `commit(len)`.
- Never use `serde-wasm-bindgen`/JSON per frame or per event. Strings cost a TextEncoder/Decoder pass plus a copy each way.

```rust
#[wasm_bindgen(module = "/js/glue.js")]
extern "C" { fn schedule_frame(); }            // JS: sets up ONE rAF if none queued

#[wasm_bindgen]
impl Engine {
    pub fn pointer_event(&mut self, kind: u32, x: f32, y: f32, mods: u32, t: f64) {
        self.input.push(kind, x, y, mods, t);   // mutate model/draft
        self.mark_dirty();
    }
    fn mark_dirty(&mut self) { if !self.dirty { self.dirty = true; schedule_frame(); } }
    pub fn frame(&mut self, now: f64) { self.dirty = false; self.render(now); } // one crossing/frame
    pub fn rows_ptr(&self) -> *const u32 { self.layer_rows.as_ptr() }   // flat, for React
    pub fn rows_len(&self) -> usize { self.layer_rows.len() }
}
```
```ts
// JS views are cheap to create; never keep one across a call that can allocate.
const u32 = (ptr: number, len: number) => new Uint32Array(wasm.memory.buffer, ptr, len);
```

**View invalidation (top pitfall)**: when wasm memory grows, the previous `ArrayBuffer` is detached and every view over it dies. The js-sys docs for `Float32Array::view` warn that any later allocation may invalidate the returned value. Rules: create the view immediately before use; reserve per-frame buffers up front (`Vec::with_capacity`) so growth is rare; remember linear memory grows but never shrinks, so keep image pixels out of the wasm heap (decode with `createImageBitmap`, upload straight to a texture) and cap peak usage.

**Allocator and data structures**
- The default dlmalloc is about 10 KB of code. `wee_alloc` trades allocation speed for size and is not suited to allocation-heavy loops (the book's own caveat); do not swap allocators. Remove allocations instead: reusable `Vec`s, arenas, `SmallVec`.
- Use `FxHashMap`/`ahash` (default SipHash is slow). Avoid `Rc<RefCell<_>>` scene graphs; use indices.

**Binary size and build** ([book: code size](https://rustwasm.github.io/book/reference/code-size.html))
- `lto`, `opt-level` `s`/`z` (measure both; `s` is sometimes smaller than `z`), `wasm-opt -Os/-Oz` saves another 15 to 20%, `twiggy top` to find fat, avoid `format!`/`to_string` and hidden panics (indexing, `unwrap`, division). Penpot builds one Rust crate two ways: a speed profile (`-O3`) for the editor and a size profile (`-Oz`) for the headless exporter; do the same if Flexma ships an export/worker module.
- Rust 1.87+ (LLVM 20) enables `bulk-memory` and `nontrapping-fptoint` by default for `wasm32-unknown-unknown` (plus multivalue, reference-types, sign-ext, mutable-globals). If `wasm-opt` rejects the module, pass matching `--enable-*` flags (or `--all-features`). Browsers without these features need `-Ctarget-cpu=mvp` plus nightly `-Zbuild-std`; not worth it for a 2026 self-hosted tool.
- The rustwasm GitHub org was sunset in 2025 and wasm-bindgen moved to its own org. Do not depend on `wasm-pack`; call `cargo build`, `wasm-bindgen --target web`, and `wasm-opt` directly, and pin the `wasm-bindgen` CLI to exactly the crate version.

```toml
[profile.release]
opt-level = 3            # hot crates; try "s" for the rest and compare size AND frame time
lto = "fat"
codegen-units = 1
panic = "abort"
strip = "debuginfo"
[profile.dev]
opt-level = 1
[profile.dev.package."*"]
opt-level = 3            # unoptimized wasm is unusably slow for a canvas editor [judgement]
```

**SIMD**: fixed-width `simd128` is baseline (Chrome 91, Firefox 89, Safari 16.4) but is all-or-nothing via `-Ctarget-feature=+simd128`; wasm has no runtime fallback, so older browsers need a second build selected with `wasm-feature-detect`. Gains appear only in data-parallel loops (batch transform multiply, AABB culling, color packing). **[judgement]** build one SIMD binary, require a modern browser, and verify autovectorization with a benchmark. Skip relaxed-SIMD (Safari support still missing per caniuse).

**Threads**: not worth it initially. `wasm-bindgen-rayon` needs nightly, `-Zbuild-std`, atomics+bulk-memory, `--target web`, and `SharedArrayBuffer`, which requires COOP/COEP cross-origin isolation (breaks third-party embeds/images without CORP). Figma's editor is a single-threaded core. Use plain Web Workers with message passing for decode, export and heavy tessellation instead.

**Do / don't**
- Do: JS-owned rAF calling `engine.frame()`; `console_error_panic_hook` in debug only; a "poisoned" flag after any trap (with `panic=abort` state may be inconsistent) that prompts reload.
- Don't: pass `Vec<T>`/`String`/`JsValue` per frame; hold typed-array views across calls; create a `Closure` per frame (create once, or avoid by letting JS own rAF).

**Sources**
- https://www.figma.com/blog/building-a-professional-design-tool-on-the-web/
- https://rustwasm.github.io/book/reference/code-size.html
- https://rustwasm.github.io/book/reference/js-ffi.html
- https://docs.rs/js-sys/latest/js_sys/struct.Float32Array.html (view warning)
- https://github.com/rustwasm/wasm-bindgen/discussions/3802
- https://doc.rust-lang.org/stable/rustc/platform-support/wasm32-unknown-unknown.html
- https://blog.rust-lang.org/inside-rust/2025/07/21/sunsetting-the-rustwasm-github-org
- https://v8.dev/features/simd
- https://github.com/piotr-roslaniec/wasm-bindgen-rayon
- https://raw.githubusercontent.com/penpot/penpot/develop/render-wasm/README.md

---

## 2. WebGL2 renderer

**Draw-call structure**
- Painter's order is mandatory (designs are translucent and blended), so batch by *consecutive runs* of the same pipeline/texture/blend, not spatially. A batch breaks only when pipeline, texture or blend state changes.
- Instanced quad per shape: 4 vertices from `gl_VertexID` (TRIANGLE_STRIP), per-instance attributes via `vertexAttribDivisor`. One interleaved instance VBO; update with `bufferSubData`; grow geometrically, never `bufferData` per frame; double/triple-buffer dynamic buffers (Emscripten WebGL guide).
- UBO for per-frame data (camera, DPR, time). Cache uniform locations. VAO per pipeline, bound once.
- Blend modes other than Normal need to read the destination; WebGL2 has no framebuffer fetch. Rive's WebGL2 renderer notes that on its MSAA path any non-Normal blend forces a re-read of the frame and is costlier on mobile. Render blend-mode groups to an offscreen texture and composite, and treat each as a batch break.
- Figma's rewritten graphics interface passes all resources explicitly to `draw(...)` (no global bound state), and the implementation updates only changed bindings; they batch uniforms into one buffer per submit for WebGPU. Mirror this: a small `Gfx` trait (`upload`, `draw_instanced`, `render_target`) with a GL state cache behind it, so WebGPU is a later backend swap.

```glsl
#version 300 es
// fragment: rounded-rect SDF with analytic AA (premultiplied output)
float sdRoundedBox(vec2 p, vec2 b, vec4 r) {            // iq; r = per-corner radii
  r.xy = (p.x > 0.0) ? r.xy : r.zw;  r.x = (p.y > 0.0) ? r.x : r.y;
  vec2 q = abs(p) - b + r.x;
  return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - r.x;
}
void main() {
  float d  = sdRoundedBox(v_local, v_halfSize, v_radii);      // local space, px/scale
  float aa = fwidth(d);                                       // 1 device px
  float cov  = clamp(0.5 - d / aa, 0.0, 1.0);
  float ring = clamp(0.5 - (abs(d + v_strokeW * 0.5) - v_strokeW * 0.5) / aa, 0.0, 1.0);
  vec4 c = mix(v_fill * cov, v_stroke * ring, ring);          // inside-stroke; adapt per spec
  o_color = c * v_opacity;
}
```
Expand the quad by ~1 px plus stroke width so the AA fringe is not clipped. Zed's GPUI uses the same pattern (instanced SDF rects, one shader per primitive kind) plus closed-form (erf) rounded-rect shadows instead of blur passes.

**Anti-aliasing**

| Content | Approach |
|---|---|
| Rect/rrect/ellipse/line | Analytic SDF AA (`fwidth`); canvas created with `antialias:false` |
| Tessellated paths (lyon) | 4x MSAA renderbuffer FBO, resolve with `blitFramebuffer`, then `invalidateFramebuffer`. Lyon has no built-in AA |
| Tiles / cached layers | Render each tile with the above, then composite textures with no AA |
| Compute-style analytic coverage (Vello/Raph Levien) | Not available on WebGL2 (no compute); `vello_hybrid` targets GPUs without compute but is experimental |

Do not set `alpha:false` on the context (MDN: can be expensive on some platforms); write alpha 1.0 instead. Use premultiplied alpha throughout.

**Vector paths**
- Tessellate with `lyon` (`FillTessellator`/`StrokeTessellator`) into indexed buffers; cache per node keyed by (geometry revision, tolerance bucket where tolerance ~ 0.25 / zoom, quantized to powers of two). Merge static path meshes of one layer into a shared VBO region; free-list reuse.
- Stencil-then-cover avoids tessellation but costs two passes per path and breaks batching; keep it as a phase-2 fallback for huge self-intersecting fills **[judgement]**.

**Text** (see also Section 7)
- Shaping/layout in Rust (`cosmic-text` = rustybuzz+swash, or Parley+swash). Browsers cannot enumerate system fonts, so fonts must be loaded explicitly (bundled, user-uploaded, or Local Font Access where permitted).
- Phase 1: glyph atlas of alpha masks rasterized at (font, size x dpr bucket, subpixel variant) and drawn as instanced quads, packed with a bin packer (GPUI uses `etagere`, caches up to 16 subpixel variants per glyph, stores alpha only so any color is a multiply). Evict LRU; growing the atlas clears it, so rebuild within the frame.
- Phase 2: MSDF atlas (`msdf-atlas-gen`; shader takes the median of RGB; keep `screenPxRange` >= 2 or AA fails) for sharp text at any zoom. Slug-style direct curve rendering is now unencumbered (patent dedicated to the public domain March 2026) and avoids atlases entirely; evaluate after MSDF.

**Images**
- Decode with `createImageBitmap` (off main thread), `texStorage2D` + `texSubImage2D`, mipmaps for textures that are minified, `LINEAR` min filter otherwise. Prefer RGBA8 over RGB8. Drop to a downscaled copy matched to on-screen size when zoomed far out (tldraw: a 4000 px photo shown at 200 px needs about 250 px x dpr). Keep a per-pixel VRAM budget (MDN: derive cache caps from the pixel count of a maximized window).

**Tiles, caching, dirty regions**
- Figma's renderer is tile-based with all rendering on the GPU. Penpot's wasm renderer keeps three tile groups: viewport tiles, "potentially visible" tiles, and rendered-but-off-screen cached tiles; a fixed memory budget is reserved and tiles furthest from the view are evicted first.
- Flexma: 512 px tiles (256 on low-memory devices) in device pixels, aligned to power-of-two zoom steps so a pan does not re-rasterize and a zoom snaps to the nearest level; draw the scaled previous-level tile immediately, refine when the gesture settles (this is what makes pinch-zoom feel instant). Invalidate tiles by node AABB. **[judgement]**: start without tiles for documents under ~5k visible nodes; the per-frame instanced path is faster to build and simpler; add tiles when a 100k-node benchmark fails the 16 ms budget.
- Keep a persistent scene FBO and redraw only dirty rectangles with `gl.scissor`, then blit to the canvas each frame.

**rAF scheduling and DPR**
- Only render when dirty; schedule from Rust (Section 1). A continuous loop only during animations/gestures.
- Size the canvas in device pixels with `ResizeObserver` + `devicePixelContentBoxSize` (falls back to `contentBoxSize * dpr`); re-listen when DPR changes (zoom, monitor move). Cap the back buffer (e.g., 16 MP) and scale up via CSS beyond it (MDN: render to a smaller back buffer to trade quality for speed). Snap the canvas to whole device pixels to avoid moire at fractional DPR.

**Context loss**: listen for `webglcontextlost` (call `preventDefault()` so restore can occur; stop the loop) and `webglcontextrestored` (recreate programs, buffers, textures, FBOs, tiles; re-upload images from retained `ImageBitmap`/blobs). The source of truth is always CPU-side (the wasm document), so restore = rebuild caches. Test with `WEBGL_lose_context`. Figma also built a mid-session WebGPU-to-WebGL fallback after finding startup readback tests insufficient.

**Do / don't (GL calls)**
- Do: state cache; static FBOs; compile all shaders, then link all, then query status (`KHR_parallel_shader_compile` where present); async readback via fence + PBO.
- Don't: `getError`/`get*Parameter`/`checkFramebufferStatus`/sync `readPixels` in the frame; `bufferData` to resize; `texImage2D` (use `texStorage`); GPU picking (use the CPU index).

**Sources**
- https://developer.mozilla.org/en-US/docs/Web/API/WebGL_API/WebGL_best_practices
- https://emscripten.org/docs/optimizing/Optimizing-WebGL.html
- https://www.figma.com/blog/building-a-professional-design-tool-on-the-web/
- https://www.figma.com/blog/figma-rendering-powered-by-webgpu/
- https://zed.dev/blog/videogame
- https://github.com/nical/lyon
- https://github.com/Chlumsky/msdfgen
- https://rive.app/docs/runtimes/cpp/renderers
- https://raphlinus.github.io/rust/graphics/gpu/2019/05/08/modern-2d.html
- https://penpot.app/blog/penpots-new-rendering-system/
- https://hackaday.com/blog/page/179/?cat=GNB (Slug patent dedication; verify the reference-shader license in the repo)
- https://tldraw.dev/sdk-features/performance

---

## 3. Scene graph and interaction

**Data layout**
- Struct-of-arrays columns indexed by `NodeId(u32)` + generation: local `[f32;6]` affine, cached world `[f32;6]`, world AABB `[f32;4]`, style, parent, dirty bitset, per-node `rev: u32`. Children as a per-parent vector sorted by fractional-index key.
- Transform caching: mark subtree dirty on change; recompute top-down once at frame start for dirty roots only. Compute hierarchy-derived values (effective visibility/opacity) in one top-down pass cached by document revision, not per node per event (Compositor documents exactly this cost).
- Compose in f64 on the CPU, subtract the camera origin, upload f32 (avoids jitter at large coordinates/deep zoom). Store f32 in the model (Figma uses 32-bit floats).

**Spatial index**
- `rstar`: `RTree::bulk_load` is the preferred construction (O(n log n), better structure than repeated insert); `insert` O(log n); R-trees suit many queries with few updates. Removal requires an element equal to the stored one with the same envelope, so keep the old AABB around.
- tldraw moved to an R-tree (RBush) in v4.4 for O(log n) shape queries and skips hover hit-testing while the camera is moving.
- Policy **[judgement]**: maintain the tree incrementally for small edits; when a drag moves more than ~10 to 20% of nodes (or on large imports), rebuild with `bulk_load`; benchmark the threshold. A uniform grid is a simpler alternative if node sizes are similar.
- Per-frame viewport culling can scan the flat AABB column linearly (100k AABB tests is well under a millisecond **[judgement]**); use the R-tree for hit testing and marquee, and for building the visible list when the viewport is a small fraction of the document.
- Hit testing: AABB candidates from the index, then exact geometry (point-in-rrect/ellipse/path) front-to-back; skip while the camera moves; selection and the edited node are never culled (tldraw).

**Level of detail** (tldraw defaults as a starting point: sticky shadows, dashes and text outlines off below zoom 0.35; images sized to screen; debounced zoom when >500 shapes)
- Below thresholds: skip shadows/blur/inner effects, render dashed strokes solid, skip text below ~4 px (draw a rect proxy), skip strokes under 0.5 px, merge tiny nodes into a placeholder.
- For 100k+ visible nodes zoomed out, never rebuild per-frame instance data; serve from the tile cache.

**Incremental updates**
- Mutations go through `apply_op`, which sets dirty flags, bumps per-node `rev` and the document `version`, and appends the node id to a `changed` buffer drained once per frame (also feeds React and the network).
- Batch: tldraw wraps multi-step changes so observers fire once; do the same with `begin_batch()/end_batch()` in the engine and MCP/agent entry points.

**Sources**
- https://docs.rs/rstar/latest/rstar/struct.RTree.html
- https://tldraw.dev/sdk-features/performance
- https://tldraw.dev/releases/v4.4.0
- https://penpot.app/blog/penpots-new-rendering-system/

---

## 4. React UI

**Rules**
- React never runs in the canvas loop. The engine owns pointer/keyboard/wheel on the canvas element and calls into wasm directly; React gets coarse, versioned notifications.
- External store with version counters exposed by wasm: `doc_version`, `structure_version`, `selection_version`, `tool`, `zoom`. The engine fills a `changed_nodes` Uint32 buffer; JS drains it once per frame and dispatches to per-node listeners.
- `useSyncExternalStore(subscribe, getSnapshot)`: `subscribe` defined outside the component (or memoized), `getSnapshot` must return the cached same value until the store changes (a fresh object per call loops forever), store mutations cannot be Transitions, and do not suspend on store values (React docs).

```ts
// fine-grained, primitive-returning selector hook
export function useNodeProp<T extends number | string | boolean>(id: number, prop: PropId): T {
  const sub = useCallback((cb: () => void) => store.subscribeNode(id, cb), [id]);
  return useSyncExternalStore(sub, () => store.readProp(id, prop) as T); // primitives compare by Object.is
}
// notifications coalesced to one microtask/frame
function flush() { const ids = u32(engine.changed_ptr(), engine.changed_len()); /* copy out */ dispatch(ids); engine.clear_changed(); }
```
- Selectors return primitives or cached immutable snapshots; derive objects in `useMemo`, not in `getSnapshot`.
- Layers panel: a flat, depth-annotated row list written by Rust (`[id, depth, flags]` triples) only when `structure_version` changes; virtualize with TanStack Virtual or react-window; expand/collapse state lives in the engine. Row components subscribe per-id for name/visibility/lock, so renaming one layer re-renders one row.
- Properties panel: subscribe to the selection's node set and per-prop values. During a drag, update displayed numbers at most once per animation frame (or ~15 Hz), never per pointer event. Inputs commit on blur/Enter, and typed drags (scrubbing a number) call into wasm directly.
- Batch selection: one `set_selection(ptr,len)` call and one version bump, never one call per id. Marquee updates the selection once per frame.
- Keep viewport (pan/zoom) out of React state; expose only `zoom` (throttled) for the zoom label. Overlays that follow the canvas (selection chrome) are drawn by the engine, not positioned by React.
- Avoid re-render storms: React Compiler/`memo` on rows, no inline object props to memoized children, no context for hot values.

**Sources**
- https://react.dev/reference/react/useSyncExternalStore
- https://tldraw.dev/sdk-features/performance (batched store updates)

---

## 5. Multiplayer

**Model** ([Figma, how multiplayer works](https://www.figma.com/blog/how-figmas-multiplayer-technology-works/))
- Clients talk to one authoritative server process/task per document over WebSocket. The server orders events and keeps the latest value per property per object; same property on the same object: last write received wins; different properties or objects never conflict. No timestamps needed.
- A property holds one whole value (no text merging inside a property).
- Optimistic clients: apply locally at once; when a server change arrives for a property with an unacknowledged local change, discard the server value (prevents flicker); apply it once the local change is acked.
- Object IDs embed the creating client's id so clients can create offline without a round trip.
- Reparenting: each child stores its parent+position as **one property** so the pair updates atomically. The server rejects changes that would create a cycle; the client temporarily detaches affected objects until resolved.
- Delete removes server data (the deleting client keeps it in its undo buffer). Reconnect = fetch a fresh copy, re-apply offline edits, resume.

**Fractional indexing** ([Figma, ordered sequences](https://www.figma.com/blog/realtime-editing-of-ordered-sequences/))
- Order key between 0 and 1 (exclusive), stored as a string, averaged by string arithmetic (Figma: base 95, arbitrary precision, leading "0." omitted). One write per reorder; concurrent inserts between the same neighbors may interleave and the server assigns the later one a unique position.
- Use a maintained Rust implementation (`fractional_index` crate treats the key as an opaque comparable value) or the Rocicorp-style algorithm; add jitter to reduce simultaneous-insert collisions (a jittered scheme with 30 bits and 10,000 simultaneous keys has about a 4.5% collision chance, so you still need a server tie-break). **[judgement]**: server rewrites a colliding key, and rebalances a parent's children when keys exceed ~32 chars.

**Batching and wire format**
- Figma clients send updates every 33 ms (~30 fps); the server groups multiple changes into journal entries with start/end sequence numbers. Do the same: coalesce ops per (object, property) before sending (last value wins), so a 60 Hz drag sends 30 ops/s, and a drag's intermediate values are never in the undo history.
- Wire: schema-based binary. Figma built Kiwi (compact varints, optional-field detection, forward/backward compatibility, single-pass encode/decode; no published benchmarks). In Rust use `postcard`/`bincode` with an explicit version tag, or generate Kiwi codecs. Use JSON only for the MCP surface. Compression: tungstenite's permessage-deflate status is unclear, so compress snapshots yourself (zstd/lz4) and keep per-op messages small enough not to need it.
- Presence/cursors: separate message type on the same socket (or a second socket), latest-value-wins, throttled to <=30 Hz (33 ms), receivers interpolate. Never journal presence. Drop it first under backpressure.

**Server architecture (axum + tokio)**
- One actor task per open document owns the in-memory `Doc`; sockets send `Inbound` messages through an `mpsc`; no locks around the document. Figma's earlier single-threaded Node server suffered head-of-line blocking from big encodes; Rust cut serialization >10x and one-process-per-document became affordable.
- Split each socket into reader and writer tasks. Each client has a **bounded** outbound queue; on overflow mark the client for resync (send a snapshot + sequence number) or disconnect, never let one slow client grow memory.
- If using `tokio::sync::broadcast`: handle `RecvError::Lagged(n)` by resyncing and continuing; treating it as fatal silently kills that client's stream.
- Serialize each broadcast once into shared `Bytes` and clone the handle per client; persistence runs off the actor (dedicated writer thread).

**Persistence (SQLite)** ([Figma, making multiplayer reliable](https://www.figma.com/blog/making-multiplayer-more-reliable/))
- Figma: in-memory authoritative state; originally full-file checkpoints every 30 to 60 s (compressed binary to S3; up to ~60 s of loss on crash); added a write-ahead journal (incremental changes, per-file increasing sequence numbers, checkpoints store the sequence, journal writes ~every 0.5 s vs 60 s for checkpoints) with a lock so only one instance writes a file. Recovery = last checkpoint + replay entries above its sequence. 95% of edits persisted within ~600 ms; >2.2 billion changes/day.
- Flexma equivalent: `ops(doc_id, seq, bytes)` journal table + `checkpoints(doc_id, seq, blob)` in one SQLite DB, WAL mode (`journal_mode=WAL`, `synchronous=NORMAL`; readers don't block the single writer, far fewer fsyncs; NORMAL can lose the last commits on power loss, which the 0.5 s window already accepts). Batch all ops of a 500 ms window into one transaction. Keep transactions small. Checkpoint the document every 30 to 60 s when dirty, and when the last client leaves; then truncate journal rows below the checkpoint sequence. Run SQLite checkpoints on the writer thread (the commit that triggers an automatic checkpoint is the slow one) and use a single writer connection.
- Validate recovery in CI: checkpoint A + journal == checkpoint B byte-for-byte (Figma's dark-launch check).

**Agents (MCP)**: route agent edits through the same op pipeline and transactions; coalesce bursts into one broadcast. (Compositor coalesces external changes for 300 ms for the same reason.)

**Sources**
- https://www.figma.com/blog/how-figmas-multiplayer-technology-works/
- https://www.figma.com/blog/realtime-editing-of-ordered-sequences/
- https://www.figma.com/blog/making-multiplayer-more-reliable/
- https://www.figma.com/blog/rust-in-production-at-figma/
- https://github.com/evanw/kiwi
- https://www.sqlite.org/wal.html
- https://docs.rs/fractional_index/ and https://app.unpkg.com/jittered-fractional-indexing@1.0.1/files/README.md
- https://docs.rs/tokio/latest/tokio/sync/broadcast/index.html

---

## 6. Load time

- Figma's switch from asm.js to wasm gave >3x load time (download + init + first full render), wasm parses ~20x faster than asm.js, and after caching, load no longer depends on app size. A later renderer/wasm optimization pass took a large real document from 29 s to under 8 s ([Figma, faster](https://www.figma.com/blog/figma-faster/)).
- Serve wasm with `Content-Type: application/wasm` and load via `WebAssembly.compileStreaming`/`instantiateStreaming` (wasm-bindgen's `--target web` `init()` accepts a `fetch` response). V8 only writes its compiled-code cache for streaming-compiled modules >= 128 kB, keyed by exact URL (query string changes force recompile), and a 304 keeps it valid while a 200 invalidates it. Use immutable content-hashed filenames (`Cache-Control: public, max-age=31536000, immutable`). Caching works in service workers, which can absorb the one cold compile.
- Compiled modules run 5 to 7x larger than the `.wasm`; V8 tiers Liftoff then TurboFan (30 s to minutes for huge modules), so keep the module lean.
- Brotli (or zstd) at the reverse proxy. **[judgement]** size target: < 1.5 MB brotli for the core module; track it in CI.
- Parallelize: `<link rel=modulepreload>` for JS, `<link rel=preload as=fetch crossorigin>` for the wasm and the document snapshot; start the document fetch before wasm finishes compiling.
- Document streaming **[judgement]**: send a compressed binary snapshot; parse in wasm from stream chunks; order nodes (or index them) so the viewport's frame renders first; load images lazily with a low-res thumbnail first; load fonts only when a text node needs them (Figma notes the next step is sending journal entries instead of whole files).
- Service worker: precache app shell + wasm + fonts; do not cache the document API. Version the worker with the build hash.
- Lazy-load heavy rarely used code (export, PDF, MSDF generation, importers) as separate dynamic imports or secondary wasm modules.

**Sources**
- https://www.figma.com/blog/webassembly-cut-figmas-load-time-by-3x/
- https://www.figma.com/blog/figma-faster/
- https://v8.dev/blog/wasm-code-caching
- https://web.dev/articles/webassembly-streaming

---

## 7. Pitfalls specific to Rust + wasm + WebGL

- **web-sys call cost**: every GL call crosses wasm to JS glue to the browser's native validation; Emscripten's WebGL guide says every GL call has overhead and recommends designing the renderer to emit few calls (caching only as a fallback). At a few hundred calls per frame the crossing is small **[judgement]**; the real costs are driver sync points, typed-array view creation per upload, and tens of thousands of calls. Do not build a command-buffer interpreter until a profile demands it.
- **glow vs raw web-sys vs wgpu**: `glow` is a thin layer over `web-sys` and does no batching or state tracking, so you add the state cache. `wgpu`'s WebGL2 backend brings a much larger binary (vello_hybrid's PR reports ~3 MB saved by omitting wgpu on wasm32 WebGL2), no compute, and shader translation through naga. Recommendation: own thin `Gfx` trait + `glow` now; keep shaders in a naga-translatable GLSL subset (Figma runs their GLSL through a processor + naga to emit WGSL) so WebGPU can be added behind the trait later. No published glow-vs-wgpu size comparison was found; build both for a triangle and measure.
- **Typed array uploads**: `web-sys` upload methods take typed-array views; use `bufferSubData(target, dstByteOffset, srcData, srcOffset, length)` (WebGL2) with one view created per frame instead of one per upload, remembering views die on memory growth.
- **Memory**: wasm linear memory cannot shrink; keep image pixels, tile pixels and atlas bitmaps in GPU/ImageBitmap memory, not the Rust heap. 32-bit wasm caps near 4 GB.
- **Float precision**: f32 on the GPU breaks at large coordinates; use camera-relative uploads.
- **Context attributes** (`antialias`, `alpha`, `premultipliedAlpha`, `powerPreference`, `desynchronized`) are fixed at creation. Read `MAX_TEXTURE_SIZE` etc. once at startup; do not assume desktop limits.
- **Handles and panics**: exported `#[wasm_bindgen]` structs need `.free()` (or `--weak-refs`); `Closure::forget` leaks; with `panic=abort` a panic traps and leaves state undefined (offer reload). `wasm-bindgen` CLI and crate versions must match exactly.
- **Penpot's alternative**: Skia-on-Rust via `wasm32-unknown-emscripten` and custom rust-skia binaries. It buys Skia features (text, blur) at a larger binary and a more complex toolchain (their recent PRs deal with wasm exception handling and setjmp/longjmp). Flexma's own GL renderer on `wasm32-unknown-unknown` + wasm-bindgen is the lighter path; the cost is you build text, blur and blend modes yourself.

**Sources**
- https://emscripten.org/docs/optimizing/Optimizing-WebGL.html
- https://skia.googlesource.com/external/github.com/linebender/vello/+/refs/heads/ajakubowicz-fix-wgsl-source%5E (vello_hybrid WebGL backend, 3 MB claim)
- https://www.figma.com/blog/figma-rendering-powered-by-webgpu/
- https://github.com/penpot/penpot/pull/12235

---

## 8. Lessons from Compositor

Repository: https://github.com/robbietilton/Compositor at commit `b4bfdea` (2026-10-09): a native macOS raster editor in Swift (AppKit/SwiftUI, Core Graphics, Core Image, Metal, some C). No wasm, WebGL or React, so these are patterns to port, not code to copy. Its fluidity comes from never repainting more than changed, retained GPU textures, cheap commits and cached downsamples. Key files: `Rendering/EditorCanvas.swift`, `CanvasViewport.swift`, `GPUCanvas.swift`, `TiledLayerRenderer.swift`, `DownsampleCache.swift`, `RasterSnapshot.swift`, `EffectsPreviewCache.swift`, `Document/BrushStroke.swift`, `Document/DocumentHistory.swift`, `docs/brush-performance.md`.

**What it lacks (do not copy the omissions)**: no coalesced/predicted pointer events, no custom inertia, no explicit pointer capture (AppKit sends a drag to the view that got mouse-down, which is implicit capture). On the web add `setPointerCapture`, `touch-action:none`, `getCoalescedEvents()` for brushes and optionally `getPredictedEvents()`.

### 8.1 Input handling
1. **Draft state, commit on release.** A drag writes a pending draft; rendering reads `displayed = pending ?? committed` (`displayedTransform(for:)`). Nothing enters history until pointer-up, so a drag is one undo step. History uses a begin/end depth counter and ignores no-op edits so they do not clear redo.
2. **Screen-space tolerances.** `snapTolerance = SNAP_PX / pointsPerPixel` keeps snap distance constant on screen at any zoom; drag results round to whole pixels/degrees, typed values stay exact.
3. **Modifier changes re-run the drag without pointer motion** (a `flagsChanged` monitor re-applies the marquee/crop drag at the last pointer position). Web: on Shift/Alt `keydown/keyup` during a drag call `engine.update_drag(lastX, lastY, mods)`.
4. **Gesture exclusivity**: wheel/pinch are ignored while a transform, crop, guide or brush drag is active.
5. **Edge autoscroll**: 60 Hz tick, 12 px margin, per-frame speed `min(40, 2 + past * 0.4)`; each tick pans and re-applies the drag at the unchanged pointer. Web: an rAF loop only while the pointer is held near an edge.
6. **Provisional tail for strokes** (`BrushStroke.append`): sparse samples are smoothed through a curve over a 4-sample window; the newest segment is drawn as a straight provisional tail so the stroke never trails the cursor, then replaced by the true curve on the next sample and on `flush()` at pointer-up. Provisional and permanent coverage are separate buffers. This is the same idea as rendering `getPredictedEvents()` and discarding the guess each frame.
7. **Live previews: one in flight, one pending, latest wins.** Cancelling superseded renders starved the Hue/Saturation preview (input outpaces render time), so a running render finishes and shows, then the newest request runs. Effects previews: 60 ms debounce, single worker, capped preview size (<=1536 px side, shared ~64 MiB), previous result stays on screen until the next lands (no blink), last 3 results per layer kept for undo/redo.

### 8.2 Pan and zoom
```ts
// zoom about an anchor: keep the world point under the cursor fixed (CanvasViewport.setZoom)
function zoomAt(cam: Cam, factor: number, ax: number, ay: number) {
  const wx = (ax - cam.x) / cam.zoom, wy = (ay - cam.y) / cam.zoom;  // world point under anchor
  cam.zoom = Math.min(256, Math.max(0.001, cam.zoom * factor));
  cam.x = ax - wx * cam.zoom;  cam.y = ay - wy * cam.zoom;
}
// wheel+modifier: factor = exp(-deltaY * 0.015)  (exponential => symmetric in/out; tune k per device)
// pinch: factor = 1 + magnification.  Plain scroll pans; coarse mouse wheels x12
```
- Compositor's range is `0.001..32`; drag-zoom doubles per 100 pt (`2^(dx/100)`); keyboard zoom steps a fixed table (`1/8, 1/6, 1/4, 1/3, 1/2, 2/3, 1, 1.25, 1.5, 2, 3, 4, 5, 6, 8, 12, 16`) via "first level > current + epsilon", anchored at view center; middle button pans, with a modifier a vertical drag zooms about the press point.
- Precise vs coarse wheel: `hasPreciseScrollingDeltas ? 1 : 12`. Web: scale by `event.deltaMode` (lines ~12 to 16 px). Trackpad pinch arrives as `wheel` with `ctrlKey=true` in Chromium/Firefox and as `gesturechange` in Safari; register non-passive listeners and `preventDefault()`.
- Inertia: none implemented; macOS and browsers already deliver momentum as wheel events, so apply each delta and do not synthesize inertia for wheel.
- `followsFit` flag: while true a resize re-fits; otherwise a resize/DPR change scales pan by the points-per-pixel ratio so the center document point stays put.
- Crisp mode: from 200% zoom composite at 1:1 and enlarge with nearest sampling; pixel grid from 800%. Web: switch texture filters at thresholds.

### 8.3 Frame scheduling: render only on change
- Handlers mutate the model then call `synchronizeDisplay()`, which builds an `Equatable` `DisplayState` (viewport, each layer's displayed transform, **image identity** (object id, never pixel comparison), opacity, blend, mask identity) and invalidates only if it differs from the last drawn state; AppKit coalesces to the display refresh. Port: per-node `rev` counters plus the camera folded into a cheap signature compared in Rust, texture/tile handle ids as image identity, at most one rAF.
- **Partial invalidation**: while stroking, only the stroke's dirty rect (mapped to view space, inset 2 px) is invalidated. Web: scissor to the dirty rect when refreshing the retained scene FBO.
- **Overlays never repaint the scene**: handles, guides, brush cursor, sample ring and marching ants are separate layer-backed views (the source notes that otherwise each overlay redraw repaints the canvas beneath). Ants animate at ~8 Hz, not 60. Web: a second transparent stacked canvas/SVG with `pointer-events:none` for chrome.
- **Same-frame presentation**: the Metal layer uses `presentsWithTransaction` so canvas and overlays land together. Web: update overlays and issue GL draws in the same rAF callback.
- **No flicker on renderer hand-off**: when a frame needs the slower CPU path, the last GPU frame stays up until the CPU canvas has fully redrawn. Web: keep presenting the previous frame until a replacement (tile refresh, resize) is complete.
- **Hot state is outside UI observation** (`@ObservationIgnored` on pointer position, drag state, caches), so pointer motion does not invalidate panels; the canvas host reads a few observed fields and calls `synchronizeDisplay()`. The Navigator minimap redraws "a moment after edits settle", only while visible (shown from 300% zoom). Web: React subscribes to selection/tool/zoom-label only; debounce the minimap and render it only when visible.

### 8.4 Caching and compositing
- **Retained textures, not per-frame CPU composite.** Their note: CPU compositing cost about 5 ms per full-size layer on Retina on every drag/pan/zoom frame; layers now stay on the GPU and a frame only places them. Resampling a 4000 px layer with Core Graphics cost ~24 ms per frame even for a small dirty patch, hence the downsample cache.
- **Exact 2x mip chain**, `level = floor(log2(1/scale))`, max 6, each level from the previous, keyed by image identity, LRU under a pixel budget (~400 MB RGBA). One-step big minification looks soft/grainy. Because every halving is exactly 2x, pieces aligned to the halving grid match the whole image (no seams). Web: `texStorage2D` + `generateMipmap`; align tile grid to powers of two.
- **Snap tile edges to whole device pixels**: adjacent pieces sharing a fractional edge round differently and show a hairline on translucent pixels. Web: integer-snapped tile quads, 1 to 2 px tile padding, no linear filtering across tile borders.
- **Texture lifecycle**: drop after 90 unused frames, a 3 s idle sweep, and a memory-pressure handler (keep last frame's textures, or none when critical). Intermediates used only to build smaller levels are released at end of frame. Web: LRU byte budget; `webglcontextlost` is the hard failure.
- **Tile size by purpose**: 256 px cells for live edits, 1024 px for committed baked pieces; a stroke in progress is one texture where only touched tiles upload per frame.
- **Cheap commits via immutable snapshots with structural sharing** (`RasterSnapshot`): untouched tiles shared, replacement lists flattened with a 256 px spatial hash, a contiguous bitmap materialized lazily. 4000x4000 doc, 800 px brush: mouse-up 1058 ms to 8.7 ms; median update 6.54 to 2.64 ms; p95 11.98 to 3.70 ms (Debug, synchronous CPU timings, `docs/brush-performance.md`). History capped at 100 entries / 256 MB retained, shared images counted once. Flexma: image/raster fills as immutable handles so undo is O(changed).
- **Budgets scale with the machine**: document pixel budget = RAM/16 pixels (RAM/4 bytes at 4 B/px) capped at 800 MP; max side 30,000; max single surface 200 MP; over-budget imports are rejected or cropped with a message. Web: enforce `MAX_TEXTURE_SIZE` and a per-document pixel budget at import; downscale on decode.
- **Optimized inner loops in dev builds** (pixel kernels stay optimized in Debug): mirror with `[profile.dev.package."*"] opt-level = 3`.
- **Measure like they do**: fixed scenario, report median and p95 for pointer-update and commit, display included, with the reproduction command in the repo; add a Playwright/Chrome-tracing CI equivalent.
- **Does not port**: the brush is a Metal compute kernel and WebGL2 has no compute; use fragment ping-pong on a tile or a wasm SIMD loop plus a tile upload.

**Sources**
- https://github.com/robbietilton/Compositor (files above; benchmark numbers in `docs/brush-performance.md`)

---

## Gaps and confidence notes

- Not investigated in depth: Excalidraw (Canvas2D/rough.js; not a WebGL reference), Rive beyond its renderer docs, Graphite (searches found no performance data for the Vello/wasm path), Penpot's tile code (`render-wasm/src/tiles.rs`; its 256 px tile size and 1024-entry cache come from secondary sources).
- Search-summarized pages (WebFetch of Figma, MDN, Penpot, tldraw, Zed, Emscripten, SQLite, V8) were read through a summarizer; figures quoted are those it returned. Verify any number you plan to put in a spec.
- **[judgement]** items (thresholds such as tile size, rebuild-vs-incremental cutoffs, 1.5 MB size target, 32-char key rebalance, multi-draw usefulness, per-call GL cost at ~hundreds of calls) are starting points to benchmark, not findings.
- GLSL snippet follows iq's `sdRoundedBox`; it was written from memory of that formula and should be unit-tested with a golden image.
- `webglcontextlost`/`restored` event semantics and `getCoalescedEvents`/`getPredictedEvents`/`ctrlKey` pinch behavior are standard web-platform behavior stated from general knowledge (MDN's `isContextLost` page does not cover the events).
- Compositor findings come from reading repository source and docs at the commit above; I did not build or run it.
