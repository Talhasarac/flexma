//! Paint layers in the editor (see `flexma_doc::raster` for the model).
//!
//! Each paint layer is a grid of 256 px GPU tiles. A tile shows its base
//! snapshot (a PNG the server keeps) with every committed stroke since then
//! replayed on top in server order, then the strokes that aren't committed
//! yet: others' live strokes, our own strokes waiting for their
//! acknowledgement, and the one being painted. New dabs are stamped straight
//! onto a tile; when the server's order disagrees with what's on screen (a
//! stroke committed under something already drawn), or a stroke is undone,
//! the tile is rebuilt from its base.
//!
//! Snapshots keep loading fast: once a tile has collected enough strokes, the
//! client that painted the last one uploads a new base (leaving the newest
//! strokes out so they can still be undone), and after a quiet moment it
//! uploads the tile's latest look for exports and thumbnails.
//!
//! Textured brushes stamp built-in tip images and read grain and color
//! textures (loaded from `/brushes/` before any stroke that needs them is
//! drawn). Smudge strokes read the layer under each dab, so they run dab by
//! dab across tiles with a pickup buffer per stroke; a tile they touch is
//! only ever redrawn together with every other tile they touch.

use crate::backend::{Backend, DabBatch, DabStyle, ImageSource, PaintMode, SmudgeDab, SmudgeTile, TexId, DAB_FLOATS, SMUDGE_BUF};
use flexma_doc::raster::{self, Brush, Dab, DabCursor, Stroke, TILE};
use flexma_doc::{Document, NodeType};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::HtmlImageElement;

type Key = (i32, i32);

/// Strokes on a tile past its base before a new base is uploaded.
const BASE_EVERY: usize = 32;
/// The newest strokes per tile left out of a new base, so they stay undoable.
const UNDO_KEEP: usize = 12;
/// Quiet time before a tile's latest look is uploaded (for exports).
const VIEW_AFTER_MS: f64 = 2500.0;
/// Others' live strokes that stop updating are dropped after this long.
const LIVE_TIMEOUT_MS: f64 = 15_000.0;

/// A brush tip or texture image (see `raster::TIPS` and `raster::TEXTURES`).
struct Asset {
    tex: Option<TexId>,
    w: f32,
    h: f32,
    /// Loaded, or failed (then dabs fall back to a round tip, no texture).
    ready: bool,
}

/// Base snapshots wait until painting has paused this long (they replay
/// strokes, which would hitch a stroke in progress).
const BASE_AFTER_IDLE_MS: f64 = 1000.0;

/// Dab batches collected for one GPU submission. Tiles are independent, so
/// batches are grouped by tile (each tile's keep their order) and runs with
/// the same look merge into one draw.
#[derive(Default)]
struct Batches {
    list: Vec<(TexId, Vec<f32>, PaintMode, DabStyle)>,
}

impl Batches {
    fn push(&mut self, tex: TexId, data: &[f32], mode: PaintMode, style: DabStyle) {
        if let Some(last) = self.list.iter_mut().rev().find(|b| b.0 == tex) {
            if last.2 == mode && last.3 == style {
                last.1.extend_from_slice(data);
                return;
            }
        }
        self.list.push((tex, data.to_vec(), mode, style));
    }

    /// Send everything collected so far (before work that must come after it).
    fn flush(&mut self, gpu: &mut dyn Backend) {
        if self.list.is_empty() {
            return;
        }
        // A stable sort keeps each tile's batches in order.
        self.list.sort_by_key(|b| b.0 .0);
        let batches: Vec<DabBatch> = self.list.iter().map(|(tex, d, mode, style)| DabBatch { tex: *tex, dabs: d, mode: *mode, style: *style }).collect();
        gpu.paint_dabs(&batches);
        self.list.clear();
    }
}

/// GPU work queued as strokes come in, done in order by `apply_gpu`.
enum Work {
    /// Dabs for one tile.
    Dabs { layer: String, key: Key, data: Vec<f32>, brush: Rc<Brush>, mode: PaintMode },
    /// Smudge dabs (layer px), one after another across tiles. `keys`: the
    /// tiles they cover.
    Smudge { layer: String, id: String, brush: Rc<Brush>, alpha_lock: bool, dabs: Vec<Dab>, first: bool, keys: Vec<Key> },
}

#[derive(Clone)]
struct Committed {
    stroke: Rc<Stroke>,
    undone: bool,
}

#[derive(Default)]
struct Tile {
    tex: Option<TexId>,
    /// Strokes up to this number are baked into the base snapshot.
    base_seq: u64,
    /// The base snapshot (None: transparent).
    base_url: Option<String>,
    base_img: Option<HtmlImageElement>,
    /// The base image is loading for a rebuild (the tile is hidden meanwhile).
    loading: bool,
    /// The base image is loading for a snapshot (the tile stays visible).
    fetching: bool,
    needs_rebuild: bool,
    /// Committed strokes touching the tile past its base, by number.
    strokes: Vec<u64>,
    /// Uncommitted strokes drawn on the tile, in the order they started.
    provisional: Vec<String>,
    /// Two of them were painted at once, so their dabs are interleaved: the
    /// tile is redrawn in server order as they commit.
    interleaved: bool,
    /// Newest committed stroke on the tile, when it landed and whether it's ours.
    last_seq: u64,
    changed_at: f64,
    last_mine: bool,
    view_sent: u64,
    snapshotting: bool,
}

impl Tile {
    fn ready(&self) -> bool {
        self.tex.is_some() && !self.loading && !self.needs_rebuild
    }
}

struct Layer {
    w: f64,
    h: f64,
    tiles: HashMap<Key, Tile>,
    /// In the document. A deleted layer keeps its tile records (not their
    /// textures), so undoing the delete brings the painting back.
    present: bool,
}

impl Layer {
    fn tiles_of(&self, s: &Stroke) -> Vec<Key> {
        s.tiles(self.w, self.h)
    }
}

/// A stroke being painted (ours) or watched live (someone else's).
struct Live {
    stroke: Stroke,
    cursor: DabCursor,
    from: String,
    touched: HashSet<Key>,
    last_ms: f64,
}

pub struct SnapshotReady {
    pub layer: String,
    pub x: i32,
    pub y: i32,
    pub kind: &'static str,
    pub seq: u64,
    /// Straight (not premultiplied) RGBA, rows top first.
    pub pixels: Vec<u8>,
}

#[derive(Default)]
pub struct PaintState {
    layers: HashMap<String, Layer>,
    committed: BTreeMap<u64, Committed>,
    by_id: HashMap<String, u64>,
    /// Our strokes sent but not acknowledged, in order.
    pending: Vec<Stroke>,
    /// Our strokes undone before their acknowledgement arrived.
    undo_on_ack: HashSet<String>,
    active: Option<Live>,
    /// Points of the active stroke already sent live.
    live_sent: usize,
    live: HashMap<String, Live>,
    queue: Vec<Work>,
    /// Brush images by id, and loads that finished (id, image).
    assets: HashMap<String, Asset>,
    asset_loads: Rc<RefCell<Vec<(String, Key, String, Option<HtmlImageElement>)>>>,
    /// URL prefix of brush images (`{asset_base}{id}.png`).
    pub asset_base: String,
    /// Pickup buffers of smudge strokes in progress, by stroke id.
    buffers: HashMap<String, TexId>,
    /// Painting is possible (a paint tool is picked): get the GPU ready.
    pub warm: bool,
    warmed: bool,
    /// Tile readbacks are still in flight on the GPU.
    readbacks: bool,
    /// When a stroke was last being painted (base snapshots wait for a pause).
    last_paint_ms: f64,
    outbox: Vec<String>,
    loaded: bool,
    early: Vec<Value>,
    /// URL prefix of tile images: `{base}/{layer}/{x}/{y}?kind=base&v={seq}`.
    pub tile_base: String,
    images: Rc<RefCell<Vec<(String, Key, String, Option<HtmlImageElement>)>>>,
    snapshots: Rc<RefCell<Vec<SnapshotReady>>>,
    trash: Rc<RefCell<Vec<TexId>>>,
    freed: Vec<TexId>,
    pub wake: Rc<Cell<bool>>,
    /// Set by a timer when tile snapshots may be due (the editor then runs
    /// `apply_gpu` even if nothing else changed).
    pub due: Rc<Cell<bool>>,
    timer: Rc<Cell<bool>>,
    pub client_id: String,
    counter: u64,
    pub dirty: bool,
    doc_version: u64,
}

fn premul(brush: &Brush, d: &Dab) -> [f32; 4] {
    let c = brush.color;
    if brush.erase {
        return [0.0, 0.0, 0.0, d.alpha];
    }
    let a = (c[3] * d.alpha).clamp(0.0, 1.0);
    [c[0] * a, c[1] * a, c[2] * a, a]
}

fn mode_of(brush: &Brush, alpha_lock: bool) -> PaintMode {
    if brush.erase {
        PaintMode::Erase
    } else if alpha_lock {
        PaintMode::AlphaLock
    } else {
        PaintMode::Paint
    }
}

/// How far a dab can reach from its center: a tip's longer side is the
/// diameter, and it may be rotated.
fn dab_reach(brush: &Brush, d: &Dab) -> f32 {
    if brush.tip.is_some() {
        d.radius * std::f32::consts::SQRT_2
    } else {
        d.radius
    }
}

/// Tiles of `layer` a dab covers.
fn dab_tiles(brush: &Brush, d: &Dab, layer: &Layer) -> impl Iterator<Item = Key> {
    let t = TILE as f32;
    let max_x = ((layer.w as f32 / t).ceil() as i32 - 1).max(0);
    let max_y = ((layer.h as f32 / t).ceil() as i32 - 1).max(0);
    let r = dab_reach(brush, d) + 1.5;
    let (x0, x1) = (((d.x - r) / t).floor() as i32, ((d.x + r) / t).floor() as i32);
    let (y0, y1) = (((d.y - r) / t).floor() as i32, ((d.y + r) / t).floor() as i32);
    (y0.max(0)..=y1.min(max_y)).flat_map(move |ty| (x0.max(0)..=x1.min(max_x)).map(move |tx| (tx, ty)))
}

/// Per-tile dab instance data for `dabs`, limited to `only` tiles.
fn bucket(brush: &Brush, dabs: &[Dab], layer: &Layer, only: Option<&HashSet<Key>>) -> HashMap<Key, Vec<f32>> {
    let t = TILE as f32;
    let mut out: HashMap<Key, Vec<f32>> = HashMap::new();
    for d in dabs {
        let col = premul(brush, d);
        for (tx, ty) in dab_tiles(brush, d, layer) {
            if only.is_some_and(|o| !o.contains(&(tx, ty))) {
                continue;
            }
            let v = out.entry((tx, ty)).or_default();
            v.extend_from_slice(&[d.x - tx as f32 * t, d.y - ty as f32 * t, d.radius, brush.hardness]);
            v.extend_from_slice(&col);
            v.extend_from_slice(&[d.angle, 0.0, 0.0, 0.0]);
        }
    }
    out
}

/// One smudge dab's instance data, in layer px (the color slot carries only
/// the flow).
fn smudge_dab(brush: &Brush, d: &Dab) -> [f32; DAB_FLOATS] {
    [d.x, d.y, d.radius, brush.hardness, 0.0, 0.0, 0.0, d.alpha, d.angle, 0.0, 0.0, 0.0]
}

/// The brush color, premultiplied (what mixer brushes add while smudging).
fn paint_color(brush: &Brush) -> [f32; 4] {
    let c = brush.color;
    [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]]
}

impl PaintState {
    pub fn new() -> PaintState {
        PaintState { asset_base: "/brushes/".into(), ..PaintState::default() }
    }

    /// Forget the file's paint (a reconnect reloads it), keeping our own
    /// strokes the server hasn't confirmed: `load` sends them again.
    pub fn reset(&mut self) {
        for l in self.layers.values() {
            for t in l.tiles.values() {
                if let Some(tex) = t.tex {
                    self.freed.push(tex);
                }
            }
        }
        let old = std::mem::take(self);
        *self = PaintState {
            tile_base: old.tile_base,
            wake: old.wake,
            due: old.due,
            timer: old.timer,
            client_id: old.client_id,
            freed: old.freed,
            pending: old.pending,
            undo_on_ack: old.undo_on_ack,
            active: old.active.map(|a| Live { touched: HashSet::new(), ..a }),
            counter: old.counter,
            images: old.images,
            snapshots: old.snapshots,
            trash: old.trash,
            assets: old.assets,
            asset_loads: old.asset_loads,
            asset_base: old.asset_base,
            buffers: old.buffers,
            warm: old.warm,
            warmed: old.warmed,
            ..PaintState::default()
        };
        self.dirty = true;
    }

    /// Keep layer sizes in step with the document; free removed layers' GPU tiles.
    pub fn sync_layers(&mut self, doc: &Document, version: u64) {
        if version == self.doc_version && !self.layers.is_empty() {
            return;
        }
        self.doc_version = version;
        let mut seen = HashSet::new();
        for n in doc.nodes() {
            if n.kind != NodeType::Paint || flexma_doc::node::is_derived_id(&n.id) {
                continue;
            }
            seen.insert(n.id.clone());
            let l = self.layers.entry(n.id.clone()).or_insert_with(|| Layer { w: n.width, h: n.height, tiles: HashMap::new(), present: true });
            l.w = n.width.clamp(1.0, raster::MAX_LAYER_SIDE);
            l.h = n.height.clamp(1.0, raster::MAX_LAYER_SIDE);
            if !l.present {
                l.present = true;
                self.dirty = true;
            }
        }
        for (id, l) in self.layers.iter_mut() {
            if l.present && !seen.contains(id) {
                l.present = false;
                for t in l.tiles.values_mut() {
                    self.freed.extend(t.tex.take());
                    t.base_img = None;
                    t.loading = false;
                    t.fetching = false;
                    t.snapshotting = false;
                    t.needs_rebuild = true;
                }
            }
        }
    }

    pub fn has_layer(&self, id: &str) -> bool {
        self.layers.contains_key(id)
    }

    /// Tiles to draw for a layer: (column, row, texture).
    pub fn tiles(&self, layer: &str) -> Vec<(i32, i32, TexId)> {
        let Some(l) = self.layers.get(layer).filter(|l| l.present) else { return Vec::new() };
        l.tiles.iter().filter(|(_, t)| !t.loading).filter_map(|(k, t)| t.tex.map(|tex| (k.0, k.1, tex))).collect()
    }

    pub fn layer_size(&self, layer: &str) -> Option<(f64, f64)> {
        self.layers.get(layer).map(|l| (l.w, l.h))
    }

    fn tile_url(&self, layer: &str, k: Key, seq: u64) -> String {
        let enc: String = layer
            .bytes()
            .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect();
        format!("{}/{enc}/{}/{}?kind=base&v={seq}", self.tile_base, k.0, k.1)
    }

    fn tile_mut(&mut self, layer: &str, k: Key) -> Option<&mut Tile> {
        self.layers.get_mut(layer).map(|l| l.tiles.entry(k).or_default())
    }

    /// Stamp `dabs` of a stroke (or mark tiles for rebuild when they can't
    /// take dabs yet). `provisional` records the stroke on the tiles it
    /// touches; `first`: the dabs start the stroke (smudge pickup).
    #[allow(clippy::too_many_arguments)]
    fn stamp(&mut self, id: &str, layer_id: &str, brush: &Brush, alpha_lock: bool, dabs: &[Dab], provisional: bool, first: bool) {
        let assets = self.ensure_assets(brush);
        let Some(layer) = self.layers.get(layer_id) else { return };
        let smudge = brush.smudges();
        let buckets = bucket(brush, dabs, layer, None);
        let keys: Vec<Key> = buckets.keys().copied().collect();
        let mode = mode_of(brush, alpha_lock);
        let shared = Rc::new(brush.clone());
        for (k, data) in buckets {
            let Some(t) = self.tile_mut(layer_id, k) else { continue };
            if provisional {
                match t.provisional.iter().position(|p| p == id) {
                    None => t.provisional.push(id.to_string()),
                    // Someone else's stroke went down since this one started here.
                    Some(i) if i + 1 != t.provisional.len() => t.interleaved = true,
                    Some(_) => {}
                }
            }
            if !assets || !(t.ready() || (smudge && t.tex.is_none() && !t.loading && t.base_url.is_none() && !t.needs_rebuild)) {
                t.needs_rebuild = true;
            } else if !smudge {
                self.queue.push(Work::Dabs { layer: layer_id.to_string(), key: k, data, brush: shared.clone(), mode });
            }
        }
        if smudge && assets && !dabs.is_empty() {
            self.queue.push(Work::Smudge {
                layer: layer_id.to_string(),
                id: id.to_string(),
                brush: shared,
                alpha_lock,
                dabs: dabs.to_vec(),
                first,
                keys,
            });
        }
        self.dirty = true;
    }

    // ---------------------------------------------------------------- brush images

    /// Whether every image `brush` uses is loaded; starts loading the rest.
    fn ensure_assets(&mut self, brush: &Brush) -> bool {
        let mut ready = true;
        for id in [&brush.tip, &brush.grain, &brush.tint].into_iter().flatten() {
            match self.assets.get(id) {
                Some(a) => ready &= a.ready,
                None => {
                    self.assets.insert(id.clone(), Asset { tex: None, w: 0.0, h: 0.0, ready: false });
                    let url = format!("{}{id}.png", self.asset_base);
                    start_image_load(url, id.clone(), (0, 0), self.asset_loads.clone(), self.wake.clone());
                    ready = false;
                }
            }
        }
        ready
    }

    /// Start loading a brush's images (before its first stroke).
    pub fn preload(&mut self, brush: &Brush) {
        self.ensure_assets(brush);
    }

    /// How `brush` draws on tile `k`.
    fn style_for(&self, brush: &Brush, k: Key) -> DabStyle {
        let loaded = |id: &Option<String>| id.as_ref().and_then(|i| self.assets.get(i)).filter(|a| a.ready && a.tex.is_some());
        let tip = loaded(&brush.tip);
        let grain = loaded(&brush.grain).filter(|_| brush.grain_strength > 0.0);
        let tint = loaded(&brush.tint).filter(|_| brush.tint_strength > 0.0);
        let (tip_extent, tip_px) = match tip {
            Some(a) => {
                let m = a.w.max(a.h).max(1.0);
                ([a.w / m, a.h / m], [a.w, a.h])
            }
            None => ([1.0, 1.0], [0.0, 0.0]),
        };
        DabStyle {
            tip: tip.and_then(|a| a.tex),
            tip_extent,
            tip_px,
            tip_color: brush.tip_color && tip.is_some(),
            grain: grain.and_then(|a| a.tex),
            grain_size: grain.map(|a| [a.w * brush.grain_scale, a.h * brush.grain_scale]).unwrap_or_default(),
            grain_px: grain.map(|a| [a.w, a.h]).unwrap_or_default(),
            grain_strength: brush.grain_strength,
            tint: tint.and_then(|a| a.tex),
            tint_size: tint.map(|a| [a.w * brush.tint_scale, a.h * brush.tint_scale]).unwrap_or_default(),
            tint_px: tint.map(|a| [a.w, a.h]).unwrap_or_default(),
            tint_strength: brush.tint_strength,
            origin: [k.0 as f32 * TILE as f32, k.1 as f32 * TILE as f32],
        }
    }

    fn rebuild(&mut self, layer: &str, keys: impl IntoIterator<Item = Key>) {
        for k in keys {
            if let Some(t) = self.tile_mut(layer, k) {
                t.needs_rebuild = true;
            }
        }
        self.dirty = true;
    }

    // ---------------------------------------------------------------- loading

    /// The file's paint state from `GET /paint`: per layer, tile bases and the
    /// strokes on top of them.
    pub fn load(&mut self, state: &Value) {
        let layers = state.get("layers").and_then(Value::as_object).cloned().unwrap_or_default();
        for (id, l) in layers {
            if !self.layers.contains_key(&id) {
                continue;
            }
            for t in l.get("tiles").and_then(Value::as_array).into_iter().flatten() {
                let (Some(x), Some(y)) = (t.get("x").and_then(Value::as_i64), t.get("y").and_then(Value::as_i64)) else { continue };
                let base = t.get("base").and_then(Value::as_u64);
                let view = t.get("view").and_then(Value::as_u64);
                let k = (x as i32, y as i32);
                let url = base.map(|b| self.tile_url(&id, k, b));
                if let Some(tile) = self.tile_mut(&id, k) {
                    tile.base_seq = base.unwrap_or(0);
                    tile.base_url = url;
                    tile.view_sent = view.unwrap_or(0).max(tile.base_seq);
                    tile.needs_rebuild = true;
                }
            }
            for s in l.get("strokes").and_then(Value::as_array).into_iter().flatten() {
                let Some(seq) = s.get("seq").and_then(Value::as_u64) else { continue };
                let Ok(stroke) = serde_json::from_value::<Stroke>(s.get("stroke").cloned().unwrap_or(Value::Null)) else { continue };
                let undone = s.get("undone").and_then(Value::as_bool).unwrap_or(false);
                self.add_committed(seq, stroke, undone, false, 0.0, false);
            }
        }
        // Tiles whose saved look is behind their strokes (the painter left
        // before uploading it): this client uploads it once the tile is drawn.
        for l in self.layers.values_mut() {
            for t in l.tiles.values_mut() {
                if t.last_seq > t.view_sent {
                    t.last_mine = true;
                    t.changed_at = f64::NEG_INFINITY;
                }
            }
        }
        self.loaded = true;
        self.resume_ours();
        let max = state.get("seq").and_then(Value::as_u64).unwrap_or(0);
        for m in std::mem::take(&mut self.early) {
            if m.get("t").and_then(Value::as_str) == Some("stroke") && m.get("seq").and_then(Value::as_u64).is_some_and(|s| s <= max) {
                continue;
            }
            self.receive(&m, 0.0);
        }
        self.dirty = true;
    }

    /// After (re)loading: our strokes the server already has are done; the
    /// rest are sent again and drawn on top, like the one being painted.
    fn resume_ours(&mut self) {
        for id in std::mem::take(&mut self.undo_on_ack) {
            if self.by_id.get(&id).and_then(|s| self.committed.get(s)).is_some_and(|c| !c.undone) {
                self.set_undone(&id, true, Some(0.0));
                self.outbox.push(json!({ "t": "strokeUndo", "id": id, "undone": true }).to_string());
            }
        }
        let mut ours: Vec<Stroke> = Vec::new();
        for s in std::mem::take(&mut self.pending) {
            if self.by_id.contains_key(&s.id) || !self.layers.contains_key(&s.layer) {
                continue;
            }
            self.outbox.push(json!({ "t": "stroke", "stroke": s }).to_string());
            ours.push(s.clone());
            self.pending.push(s);
        }
        if let Some(a) = self.active.as_mut() {
            if self.layers.contains_key(&a.stroke.layer) {
                // Sent live again from the start.
                self.live_sent = 0;
                ours.push(a.stroke.clone());
            } else {
                self.active = None;
            }
        }
        for s in ours {
            let Some(keys) = self.layers.get(&s.layer).map(|l| l.tiles_of(&s)) else { continue };
            for &k in &keys {
                if let Some(t) = self.tile_mut(&s.layer, k).filter(|t| !t.provisional.contains(&s.id)) {
                    t.provisional.push(s.id.clone());
                }
            }
            if let Some(a) = self.active.as_mut().filter(|a| a.stroke.id == s.id) {
                a.touched.extend(keys.iter().copied());
            }
            self.rebuild(&s.layer, keys);
        }
    }

    /// Record a committed stroke. `draw`: stamp it now if the tiles allow it.
    fn add_committed(&mut self, seq: u64, stroke: Stroke, undone: bool, mine: bool, now: f64, draw: bool) {
        if self.by_id.contains_key(&stroke.id) {
            return;
        }
        let Some(layer) = self.layers.get(&stroke.layer) else { return };
        let keys = layer.tiles_of(&stroke);
        let stroke = Rc::new(stroke);
        self.by_id.insert(stroke.id.clone(), seq);
        self.committed.insert(seq, Committed { stroke: stroke.clone(), undone });
        let mut needs = Vec::new();
        for &k in &keys {
            let Some(t) = self.tile_mut(&stroke.layer, k) else { continue };
            if seq <= t.base_seq {
                continue;
            }
            if let Err(i) = t.strokes.binary_search(&seq) {
                t.strokes.insert(i, seq);
            }
            if seq > t.last_seq {
                t.last_seq = seq;
                t.changed_at = now;
                t.last_mine = mine;
            }
            // Already drawn as ours or as a live stroke: it's in the right place
            // unless something uncommitted was drawn before it.
            let pos = t.provisional.iter().position(|p| *p == stroke.id);
            match pos {
                Some(i) if t.interleaved => {
                    t.provisional.remove(i);
                    needs.push(k);
                }
                None if t.interleaved => needs.push(k),
                Some(0) => {
                    t.provisional.remove(0);
                }
                Some(i) => {
                    t.provisional.remove(i);
                    needs.push(k);
                }
                None if !t.provisional.is_empty() || !draw || undone => needs.push(k),
                None => {}
            }
        }
        if draw && !undone {
            let drawn: HashSet<Key> = keys.iter().copied().filter(|k| !needs.contains(k)).collect();
            let fresh: Vec<Key> = drawn
                .iter()
                .copied()
                .filter(|k| {
                    // Tiles where this stroke wasn't on screen at all yet.
                    !self.was_provisional(&stroke.id, &stroke.layer, *k)
                })
                .collect();
            // A smudge stroke reads what's under it as it goes: only a
            // replay in order gets it right.
            if !fresh.is_empty() && (stroke.brush.smudges() || !self.ensure_assets(&stroke.brush)) {
                needs.extend(fresh);
            } else if !fresh.is_empty() {
                let only: HashSet<Key> = fresh.into_iter().collect();
                let dabs = raster::all_dabs(&stroke);
                if let Some(layer) = self.layers.get(&stroke.layer) {
                    let mode = mode_of(&stroke.brush, stroke.alpha_lock);
                    let brush = Rc::new(stroke.brush.clone());
                    for (k, data) in bucket(&stroke.brush, &dabs, layer, Some(&only)) {
                        let ready = self.layers.get(&stroke.layer).and_then(|l| l.tiles.get(&k)).is_some_and(Tile::ready);
                        if ready {
                            self.queue.push(Work::Dabs { layer: stroke.layer.clone(), key: k, data, brush: brush.clone(), mode });
                        } else {
                            needs.push(k);
                        }
                    }
                }
            }
        }
        self.rebuild(&stroke.layer, needs);
    }

    fn was_provisional(&self, id: &str, layer: &str, k: Key) -> bool {
        // Ours (pending / active) or a live stroke we watched: already stamped.
        let ours = self.pending.iter().any(|s| s.id == id) || self.active.as_ref().is_some_and(|a| a.stroke.id == id);
        let watched = self.live.get(id).is_some_and(|l| l.touched.contains(&k));
        let _ = layer;
        ours || watched
    }

    // ---------------------------------------------------------------- messages

    /// A message from the room: `strokeLive`, `stroke`, `strokeUndo`, `tile`
    /// or `paintReject`.
    pub fn receive(&mut self, m: &Value, now: f64) {
        if !self.loaded {
            self.early.push(m.clone());
            return;
        }
        match m.get("t").and_then(Value::as_str) {
            Some("strokeLive") => self.on_live(m, now),
            Some("stroke") => {
                let (Some(seq), Some(s)) = (m.get("seq").and_then(Value::as_u64), m.get("stroke")) else { return };
                let Ok(stroke) = serde_json::from_value::<Stroke>(s.clone()) else { return };
                let mine = m.get("from").and_then(Value::as_str) == Some(self.client_id.as_str());
                let id = stroke.id.clone();
                // A live stroke we watched: stamp what the live chunks missed
                // (it stays "live" until committed below, so it counts as drawn).
                let mut rest = Vec::new();
                let mut first = false;
                if let Some(l) = self.live.get_mut(&id) {
                    first = l.cursor.count() == 0;
                    rest = raster::dabs(&l.stroke.brush, &stroke.points, &stroke.tilt, &mut l.cursor);
                    l.stroke.points = stroke.points.clone();
                    l.stroke.tilt = stroke.tilt.clone();
                    if let Some(layer) = self.layers.get(&stroke.layer) {
                        for d in &rest {
                            l.touched.extend(dab_tiles(&l.stroke.brush, d, layer));
                        }
                    }
                }
                if !rest.is_empty() {
                    let brush = self.live.get(&id).map(|l| l.stroke.brush.clone()).unwrap_or_else(|| stroke.brush.clone());
                    self.stamp(&id, &stroke.layer, &brush, stroke.alpha_lock, &rest, true, first);
                }
                let ours = mine || self.pending.iter().any(|p| p.id == id);
                let undone_early = self.undo_on_ack.remove(&id);
                self.add_committed(seq, stroke, undone_early, ours, now, true);
                self.pending.retain(|p| p.id != id);
                self.live.remove(&id);
                if undone_early {
                    self.outbox.push(json!({ "t": "strokeUndo", "id": id, "undone": true }).to_string());
                }
                self.dirty = true;
            }
            Some("strokeUndo") => {
                let (Some(id), undone) = (m.get("id").and_then(Value::as_str), m.get("undone").and_then(Value::as_bool).unwrap_or(true)) else { return };
                self.set_undone(id, undone, None);
            }
            Some("tile") => {
                let (Some(layer), Some(x), Some(y), Some(seq)) = (
                    m.get("layer").and_then(Value::as_str),
                    m.get("x").and_then(Value::as_i64),
                    m.get("y").and_then(Value::as_i64),
                    m.get("seq").and_then(Value::as_u64),
                ) else {
                    return;
                };
                let k = (x as i32, y as i32);
                let url = self.tile_url(layer, k, seq);
                if let Some(t) = self.layers.get_mut(layer).and_then(|l| l.tiles.get_mut(&k)) {
                    // Someone saved this look already; no need to upload it again.
                    t.view_sent = t.view_sent.max(seq);
                    if m.get("kind").and_then(Value::as_str) != Some("base") {
                        return;
                    }
                    if seq > t.base_seq {
                        // What's on screen already shows it; future rebuilds start there.
                        t.base_seq = seq;
                        t.base_url = Some(url);
                        t.base_img = None;
                        t.strokes.retain(|s| *s > seq);
                        t.snapshotting = false;
                    }
                }
            }
            Some("paintReject") => {
                if let Some(id) = m.get("id").and_then(Value::as_str) {
                    // A refused undo or redo: back to what everyone else sees.
                    if let Some(c) = self.by_id.get(id).and_then(|seq| self.committed.get(seq)) {
                        let undone = !c.undone;
                        self.set_undone(id, undone, None);
                        return;
                    }
                    if let Some(i) = self.pending.iter().position(|p| p.id == id) {
                        let s = self.pending.remove(i);
                        if let Some(l) = self.layers.get(&s.layer) {
                            let keys = l.tiles_of(&s);
                            self.forget_provisional(&s.layer, &s.id, &keys);
                            self.rebuild(&s.layer, keys);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn forget_provisional(&mut self, layer: &str, id: &str, keys: &[Key]) {
        if let Some(l) = self.layers.get_mut(layer) {
            for k in keys {
                if let Some(t) = l.tiles.get_mut(k) {
                    t.provisional.retain(|p| p != id);
                }
            }
        }
    }

    fn on_live(&mut self, m: &Value, now: f64) {
        let (Some(id), Some(layer), Some(from)) =
            (m.get("id").and_then(Value::as_str), m.get("layer").and_then(Value::as_str), m.get("from").and_then(Value::as_str))
        else {
            return;
        };
        if m.get("cancel").and_then(Value::as_bool) == Some(true) {
            self.drop_live(id);
            return;
        }
        if self.by_id.contains_key(id) || !self.layers.contains_key(layer) {
            return;
        }
        let points: Vec<[f32; 3]> = m
            .get("points")
            .and_then(|p| serde_json::from_value(p.clone()).ok())
            .unwrap_or_default();
        if !self.live.contains_key(id) {
            let Some(brush) = m.get("brush").and_then(|b| serde_json::from_value::<raster::Brush>(b.clone()).ok()) else { return };
            let alpha_lock = m.get("alphaLock").and_then(Value::as_bool).unwrap_or(false);
            let stroke = Stroke { id: id.to_string(), layer: layer.to_string(), brush: brush.sanitized(), points: Vec::new(), tilt: Vec::new(), alpha_lock };
            self.live.insert(id.to_string(), Live { stroke, cursor: DabCursor::new(id), from: from.to_string(), touched: HashSet::new(), last_ms: now });
        }
        let Some(mut l) = self.live.remove(id) else { return };
        let tilt: Vec<[f32; 2]> = m.get("tilt").and_then(|p| serde_json::from_value(p.clone()).ok()).unwrap_or_default();
        // Tilt comes with each chunk's points; a chunk without it (or a bad
        // one) leaves the stroke untilted.
        let tilted = l.stroke.brush.uses_tilt() && tilt.len() == points.len() && l.stroke.tilt.len() == l.stroke.points.len();
        let room = raster::MAX_POINTS.saturating_sub(l.stroke.points.len());
        let good: Vec<usize> = (0..points.len()).filter(|&i| points[i].iter().chain(tilt.get(i).into_iter().flatten()).all(|v| v.is_finite())).take(room).collect();
        l.stroke.points.extend(good.iter().map(|&i| points[i]));
        if tilted {
            l.stroke.tilt.extend(good.iter().map(|&i| tilt[i]));
        } else {
            l.stroke.tilt.clear();
        }
        l.last_ms = now;
        let first = l.cursor.count() == 0;
        let dabs = raster::dabs(&l.stroke.brush, &l.stroke.points, &l.stroke.tilt, &mut l.cursor);
        if let Some(layer) = self.layers.get(&l.stroke.layer) {
            for d in &dabs {
                l.touched.extend(dab_tiles(&l.stroke.brush, d, layer));
            }
        }
        let (layer, brush, alpha_lock) = (l.stroke.layer.clone(), l.stroke.brush.clone(), l.stroke.alpha_lock);
        self.live.insert(id.to_string(), l);
        self.stamp(id, &layer, &brush, alpha_lock, &dabs, true, first);
    }

    /// Someone left: drop their unfinished strokes.
    pub fn peer_left(&mut self, client_id: &str) {
        let ids: Vec<String> = self.live.iter().filter(|(_, l)| l.from == client_id).map(|(k, _)| k.clone()).collect();
        for id in ids {
            self.drop_live(&id);
        }
    }

    fn drop_live(&mut self, id: &str) {
        if let Some(l) = self.live.remove(id) {
            let keys: Vec<Key> = l.touched.iter().copied().collect();
            self.forget_provisional(&l.stroke.layer, id, &keys);
            self.rebuild(&l.stroke.layer, keys);
        }
    }

    /// Show or hide a committed stroke. `ours`: we undid it (at this time),
    /// so we also upload the tiles' new look.
    fn set_undone(&mut self, id: &str, undone: bool, ours: Option<f64>) {
        let Some(seq) = self.by_id.get(id).copied() else { return };
        let Some(c) = self.committed.get_mut(&seq) else { return };
        if c.undone == undone {
            return;
        }
        c.undone = undone;
        let stroke = c.stroke.clone();
        let Some(l) = self.layers.get_mut(&stroke.layer) else { return };
        let keys = l.tiles_of(&stroke);
        if let Some(now) = ours {
            for k in &keys {
                if let Some(t) = l.tiles.get_mut(k).filter(|t| t.last_seq > 0) {
                    t.last_mine = true;
                    t.changed_at = now;
                    t.view_sent = 0;
                }
            }
        }
        self.rebuild(&stroke.layer, keys);
    }

    /// Messages to send to the room (JSON strings), oldest first.
    /// Our strokes still on their way: being painted, unsent or unacknowledged.
    pub fn in_flight(&self) -> bool {
        self.active.is_some() || !self.pending.is_empty() || !self.outbox.is_empty()
    }

    pub fn take_outgoing(&mut self) -> Vec<String> {
        if let Some(a) = &self.active {
            if a.stroke.points.len() > self.live_sent {
                let first = self.live_sent == 0;
                let pts = &a.stroke.points[self.live_sent..];
                let mut m = json!({ "t": "strokeLive", "id": a.stroke.id, "layer": a.stroke.layer, "points": pts, "start": first });
                if a.stroke.tilt.len() == a.stroke.points.len() && !a.stroke.tilt.is_empty() {
                    m["tilt"] = json!(&a.stroke.tilt[self.live_sent..]);
                }
                if first {
                    m["brush"] = serde_json::to_value(&a.stroke.brush).unwrap_or(Value::Null);
                    m["alphaLock"] = json!(a.stroke.alpha_lock);
                }
                self.live_sent = a.stroke.points.len();
                // Live chunks go before anything queued after them.
                self.outbox.insert(0, m.to_string());
            }
        }
        std::mem::take(&mut self.outbox)
    }

    // ---------------------------------------------------------------- painting

    pub fn next_stroke_id(&mut self) -> String {
        self.counter += 1;
        format!("{}-{}-{}", self.client_id, js_sys::Date::now() as u64 % 100_000_000, self.counter)
    }

    pub fn painting(&self) -> bool {
        self.active.is_some()
    }

    pub fn begin(&mut self, stroke: Stroke, now: f64) {
        self.live_sent = 0;
        let mut l = Live { cursor: DabCursor::new(&stroke.id), stroke, from: self.client_id.clone(), touched: HashSet::new(), last_ms: now };
        let dabs = raster::dabs(&l.stroke.brush, &l.stroke.points, &l.stroke.tilt, &mut l.cursor);
        if let Some(layer) = self.layers.get(&l.stroke.layer) {
            for d in &dabs {
                l.touched.extend(dab_tiles(&l.stroke.brush, d, layer));
            }
        }
        let (id, layer, brush, alpha_lock) = (l.stroke.id.clone(), l.stroke.layer.clone(), l.stroke.brush.clone(), l.stroke.alpha_lock);
        self.active = Some(l);
        self.stamp(&id, &layer, &brush, alpha_lock, &dabs, true, true);
    }

    /// Add a point (and the pen's tilt there, for brushes that record it).
    pub fn add_point(&mut self, p: [f32; 3], tilt: [f32; 2]) {
        let Some(mut a) = self.active.take() else { return };
        if a.stroke.points.len() >= raster::MAX_POINTS {
            self.active = Some(a);
            return;
        }
        a.stroke.points.push(p);
        if !a.stroke.tilt.is_empty() {
            a.stroke.tilt.push(tilt);
        }
        let dabs = raster::dabs(&a.stroke.brush, &a.stroke.points, &a.stroke.tilt, &mut a.cursor);
        if let Some(layer) = self.layers.get(&a.stroke.layer) {
            for d in &dabs {
                a.touched.extend(dab_tiles(&a.stroke.brush, d, layer));
            }
        }
        let (id, layer, brush, alpha_lock) = (a.stroke.id.clone(), a.stroke.layer.clone(), a.stroke.brush.clone(), a.stroke.alpha_lock);
        self.active = Some(a);
        if !dabs.is_empty() {
            self.stamp(&id, &layer, &brush, alpha_lock, &dabs, true, false);
        }
    }

    /// Finish the active stroke and send it. Returns its id.
    pub fn end(&mut self) -> Option<String> {
        // Flush the last live chunk first.
        let live = self.take_outgoing();
        let a = self.active.take()?;
        self.outbox.extend(live);
        let id = a.stroke.id.clone();
        self.outbox.push(json!({ "t": "stroke", "stroke": a.stroke }).to_string());
        self.pending.push(a.stroke);
        self.live_sent = 0;
        Some(id)
    }

    /// Abandon the active stroke (nothing is sent).
    pub fn cancel(&mut self) {
        if let Some(a) = self.active.take() {
            let keys: Vec<Key> = a.touched.iter().copied().collect();
            self.forget_provisional(&a.stroke.layer, &a.stroke.id, &keys);
            self.rebuild(&a.stroke.layer, keys);
            if self.live_sent > 0 {
                // Others saw it live: let them drop it.
                self.outbox.push(json!({ "t": "strokeLive", "id": a.stroke.id, "layer": a.stroke.layer, "points": [], "cancel": true }).to_string());
            }
            self.live_sent = 0;
        }
    }

    /// Undo one of our strokes. False if it can't be undone any more (it's
    /// baked into a snapshot).
    pub fn undo(&mut self, id: &str, now: f64) -> bool {
        if let Some(i) = self.pending.iter().position(|p| p.id == id) {
            let s = self.pending.remove(i);
            self.undo_on_ack.insert(id.to_string());
            if let Some(l) = self.layers.get(&s.layer) {
                let keys = l.tiles_of(&s);
                self.forget_provisional(&s.layer, id, &keys);
                self.rebuild(&s.layer, keys);
            }
            return true;
        }
        let Some(&seq) = self.by_id.get(id) else { return false };
        let Some(c) = self.committed.get(&seq) else { return false };
        if c.undone || !self.undoable(seq, &c.stroke) {
            return false;
        }
        self.set_undone(id, true, Some(now));
        self.outbox.push(json!({ "t": "strokeUndo", "id": id, "undone": true }).to_string());
        true
    }

    pub fn redo(&mut self, id: &str, now: f64) -> bool {
        if self.undo_on_ack.remove(id) {
            return true;
        }
        let Some(&seq) = self.by_id.get(id) else { return false };
        if !self.committed.get(&seq).is_some_and(|c| c.undone) {
            return false;
        }
        self.set_undone(id, false, Some(now));
        self.outbox.push(json!({ "t": "strokeUndo", "id": id, "undone": false }).to_string());
        true
    }

    fn undoable(&self, seq: u64, s: &Stroke) -> bool {
        let Some(l) = self.layers.get(&s.layer) else { return false };
        l.tiles_of(s).iter().all(|k| l.tiles.get(k).is_none_or(|t| t.base_seq < seq))
    }

    // ---------------------------------------------------------------- GPU

    /// Run pending GPU work: brush images, base images, rebuilds, queued
    /// dabs, snapshots.
    pub fn apply_gpu(&mut self, gpu: &mut dyn Backend, now: f64) {
        self.readbacks = gpu.poll_readbacks();
        for tex in self.freed.drain(..).chain(self.trash.borrow_mut().drain(..)) {
            gpu.delete_texture(tex);
        }
        if !self.warmed && (self.warm || !self.layers.is_empty()) {
            // Compile painting's GPU programs and load every brush image now,
            // not halfway through the first stroke.
            self.warmed = true;
            gpu.prewarm_paint();
            for id in raster::TIPS.iter().chain(raster::TEXTURES) {
                self.ensure_assets(&Brush { tip: Some(id.to_string()), ..Brush::default() });
            }
        }
        // Brush images that finished loading (a failed one counts as ready:
        // its strokes fall back to a round tip).
        let loaded: Vec<_> = self.asset_loads.borrow_mut().drain(..).collect();
        for (id, _, _, img) in loaded {
            let Some(a) = self.assets.get_mut(&id) else { continue };
            if let Some(img) = img {
                let (w, h) = (img.natural_width(), img.natural_height());
                let repeat = raster::TEXTURES.contains(&id.as_str());
                a.tex = gpu.upload_brush_texture(ImageSource::Image(&img), w, h, repeat);
                (a.w, a.h) = (w as f32, h as f32);
            }
            a.ready = true;
            self.dirty = true;
        }
        // Base images that finished loading.
        let loaded: Vec<_> = self.images.borrow_mut().drain(..).collect();
        for (layer, k, url, img) in loaded {
            if let Some(t) = self.layers.get_mut(&layer).and_then(|l| l.tiles.get_mut(&k)) {
                if t.base_url.as_deref() == Some(url.as_str()) {
                    t.base_img = img;
                    t.fetching = false;
                    if t.loading {
                        t.loading = false;
                        t.needs_rebuild = true;
                    }
                }
            }
        }
        // Others' strokes that went quiet.
        let stale: Vec<String> = self.live.iter().filter(|(_, l)| now - l.last_ms > LIVE_TIMEOUT_MS && now > 0.0).map(|(k, _)| k.clone()).collect();
        for id in stale {
            self.drop_live(&id);
        }
        // Rebuilds, grouped per layer so each stroke's dabs are computed once.
        let mut rebuild: HashMap<String, HashSet<Key>> = HashMap::new();
        for (lid, l) in self.layers.iter_mut().filter(|(_, l)| l.present) {
            for (k, t) in l.tiles.iter_mut() {
                if !t.needs_rebuild || t.loading {
                    continue;
                }
                if t.base_url.is_some() && t.base_img.is_none() {
                    t.loading = true;
                    start_image_load(t.base_url.clone().unwrap(), lid.clone(), *k, self.images.clone(), self.wake.clone());
                    continue;
                }
                rebuild.entry(lid.clone()).or_default().insert(*k);
            }
        }
        let mut rebuilt: HashMap<String, HashSet<Key>> = HashMap::new();
        for (lid, mut keys) in rebuild {
            // Smudge strokes read neighboring tiles: redraw all of theirs together.
            self.smudge_closure(&lid, &mut keys);
            if self.wait_for_bases(&lid, &keys) || !self.rebuild_tiles(gpu, &lid, &keys) {
                continue;
            }
            rebuilt.insert(lid, keys);
        }
        let covered = |lid: &str, k: &Key| rebuilt.get(lid).is_some_and(|r| r.contains(k));
        // Everything queued this frame goes to the GPU in as few submissions as
        // the order allows: dabs batch up until a smudge needs them done.
        let mut batches = Batches::default();
        let mut queue = std::mem::take(&mut self.queue).into_iter().peekable();
        while let Some(work) = queue.next() {
            match work {
                Work::Dabs { layer, key, data, brush, mode } => {
                    if covered(&layer, &key) {
                        continue;
                    }
                    let style = self.style_for(&brush, key);
                    let Some(t) = self.layers.get_mut(&layer).filter(|l| l.present).and_then(|l| l.tiles.get_mut(&key)) else { continue };
                    if t.tex.is_none() && !t.loading && t.base_url.is_none() {
                        t.tex = gpu.create_paint_texture(TILE);
                    }
                    match t.tex {
                        Some(tex) if !t.loading && !t.needs_rebuild => batches.push(tex, &data, mode, style),
                        _ => t.needs_rebuild = true,
                    }
                }
                Work::Smudge { layer, id, brush, alpha_lock, mut dabs, first, mut keys } => {
                    // The stroke's next chunks this frame go in the same call.
                    while let Some(Work::Smudge { id: next, .. }) = queue.peek() {
                        if *next != id {
                            break;
                        }
                        if let Some(Work::Smudge { dabs: more, keys: k, .. }) = queue.next() {
                            dabs.extend(more);
                            keys.extend(k);
                        }
                    }
                    // A rebuild that touched this stroke already replayed it whole.
                    if keys.iter().any(|k| covered(&layer, k)) {
                        continue;
                    }
                    batches.flush(gpu);
                    let Some(buffer) = self.buffer_for(gpu, &id) else { continue };
                    self.run_smudge(gpu, &layer, &brush, alpha_lock, &dabs, first, buffer, None, None);
                }
            }
        }
        batches.flush(gpu);
        // Pickup buffers of strokes no longer being painted.
        let painting: HashSet<&String> = self.active.iter().map(|a| &a.stroke.id).chain(self.live.keys()).collect();
        let done: Vec<String> = self.buffers.keys().filter(|id| !painting.contains(id)).cloned().collect();
        for id in done {
            if let Some(t) = self.buffers.remove(&id) {
                gpu.delete_texture(t);
            }
        }
        self.snapshot(gpu, now);
        // Readbacks started just now keep frames coming until they're collected.
        self.readbacks = gpu.poll_readbacks();
    }

    /// The pickup buffer of a smudge stroke being painted.
    fn buffer_for(&mut self, gpu: &mut dyn Backend, id: &str) -> Option<TexId> {
        if let Some(t) = self.buffers.get(id) {
            return Some(*t);
        }
        let t = gpu.create_paint_texture(SMUDGE_BUF)?;
        self.buffers.insert(id.to_string(), t);
        Some(t)
    }

    /// Smudge `dabs` across the layer's tiles in order. Writes go to tiles in
    /// `only` (default: every tile that can take dabs); `scratch` stands in
    /// for one tile (base snapshots render a tile off screen).
    #[allow(clippy::too_many_arguments)]
    fn run_smudge(
        &mut self,
        gpu: &mut dyn Backend,
        layer_id: &str,
        brush: &Brush,
        alpha_lock: bool,
        dabs: &[Dab],
        first: bool,
        buffer: TexId,
        only: Option<&HashSet<Key>>,
        scratch: Option<(Key, TexId)>,
    ) {
        let style = self.style_for(brush, (0, 0));
        let Some(layer) = self.layers.get_mut(layer_id) else { return };
        let t = TILE as f32;
        let mut plans: Vec<(Vec<SmudgeTile>, &Dab)> = Vec::with_capacity(dabs.len());
        for d in dabs {
            let mut tiles = Vec::new();
            for k in dab_tiles(brush, d, layer) {
                let origin = [k.0 as f32 * t, k.1 as f32 * t];
                if let Some((sk, tex)) = scratch.filter(|(sk, _)| *sk == k) {
                    tiles.push(SmudgeTile { tex: Some(tex), origin, write: only.is_none_or(|o| o.contains(&sk)) });
                    continue;
                }
                let writable = only.is_none_or(|o| o.contains(&k));
                let tile = layer.tiles.entry(k).or_default();
                // Unpainted tiles get a texture when the smudge reaches them.
                if writable && only.is_none() && scratch.is_none() && tile.tex.is_none() && !tile.loading && tile.base_url.is_none() && !tile.needs_rebuild {
                    tile.tex = gpu.create_paint_texture(TILE);
                }
                let tex = tile.tex.filter(|_| !tile.loading);
                let write = writable && tex.is_some() && (only.is_some() || !tile.needs_rebuild);
                tiles.push(SmudgeTile { tex, origin, write });
            }
            plans.push((tiles, d));
        }
        let paint = paint_color(brush);
        let list: Vec<SmudgeDab> = plans
            .iter()
            .enumerate()
            .map(|(i, (tiles, d))| SmudgeDab {
                tiles,
                dab: smudge_dab(brush, d),
                style,
                buffer,
                first: first && i == 0,
                keep: brush.smudge,
                paint,
                mix: brush.mix,
                alpha_lock,
            })
            .collect();
        gpu.smudge(&list);
    }

    /// Grow `keys` to every tile of each smudge stroke touching one of them,
    /// until nothing changes: a smudge stroke must be replayed whole.
    fn smudge_closure(&mut self, layer_id: &str, keys: &mut HashSet<Key>) {
        let Some(layer) = self.layers.get(layer_id) else { return };
        let mut sets: Vec<Vec<Key>> = Vec::new();
        let smudging = |s: &Stroke| s.layer == layer_id && s.brush.smudges();
        for c in self.committed.values().filter(|c| !c.undone && smudging(&c.stroke)) {
            sets.push(layer.tiles_of(&c.stroke));
        }
        for s in self.pending.iter().filter(|s| smudging(s)) {
            sets.push(layer.tiles_of(s));
        }
        for l in self.live.values().chain(self.active.iter()).filter(|l| smudging(&l.stroke)) {
            sets.push(l.touched.iter().copied().collect());
        }
        let mut grew = true;
        while grew {
            grew = false;
            for set in &sets {
                if set.iter().any(|k| keys.contains(k)) {
                    for k in set {
                        grew |= keys.insert(*k);
                    }
                }
            }
        }
    }

    /// Start loading the base images `keys` still need. True while any is loading.
    fn wait_for_bases(&mut self, layer_id: &str, keys: &HashSet<Key>) -> bool {
        let (images, wake) = (self.images.clone(), self.wake.clone());
        let Some(layer) = self.layers.get_mut(layer_id) else { return true };
        let mut waiting = false;
        for k in keys {
            let t = layer.tiles.entry(*k).or_default();
            t.needs_rebuild = true;
            if t.base_url.is_some() && t.base_img.is_none() {
                if !t.loading && !t.fetching {
                    t.fetching = true;
                    start_image_load(t.base_url.clone().unwrap(), layer_id.to_string(), *k, images.clone(), wake.clone());
                }
                waiting = true;
            }
        }
        waiting
    }

    /// Redraw tiles from their bases. False (nothing done) while a brush
    /// image they need is loading.
    fn rebuild_tiles(&mut self, gpu: &mut dyn Backend, layer_id: &str, keys: &HashSet<Key>) -> bool {
        let Some(layer) = self.layers.get(layer_id) else { return false };
        let bases: HashMap<Key, u64> = keys.iter().map(|k| (*k, layer.tiles.get(k).map_or(0, |t| t.base_seq))).collect();
        let min_base = bases.values().copied().min().unwrap_or(0);
        // Committed strokes in order, then the uncommitted ones: others' live
        // strokes, our pending ones, ours in progress.
        let mut strokes: Vec<(Option<u64>, Rc<Stroke>)> = self
            .committed
            .range(min_base + 1..)
            .filter(|(_, c)| !c.undone && c.stroke.layer == layer_id)
            .map(|(s, c)| (Some(*s), c.stroke.clone()))
            .collect();
        let mut live: Vec<&Live> = self.live.values().filter(|l| l.stroke.layer == layer_id).collect();
        live.sort_by(|a, b| a.last_ms.partial_cmp(&b.last_ms).unwrap_or(std::cmp::Ordering::Equal));
        strokes.extend(live.into_iter().map(|l| (None, Rc::new(l.stroke.clone()))));
        strokes.extend(self.pending.iter().filter(|s| s.layer == layer_id).map(|s| (None, Rc::new(s.clone()))));
        if let Some(a) = self.active.as_ref().filter(|a| a.stroke.layer == layer_id) {
            strokes.push((None, Rc::new(a.stroke.clone())));
        }
        let brushes: Vec<Brush> = strokes.iter().map(|(_, s)| s.brush.clone()).collect();
        let mut ready = true;
        for b in &brushes {
            ready &= self.ensure_assets(b);
        }
        if !ready {
            return false;
        }
        let Some(layer) = self.layers.get_mut(layer_id) else { return false };
        for k in keys {
            let t = layer.tiles.entry(*k).or_default();
            if t.tex.is_none() {
                t.tex = gpu.create_paint_texture(TILE);
            }
            let Some(tex) = t.tex else { continue };
            match &t.base_img {
                Some(img) if gpu.load_paint_texture(tex, ImageSource::Image(img), TILE) => {}
                _ => gpu.clear_paint_texture(tex, TILE),
            }
            t.needs_rebuild = false;
            t.loading = false;
        }
        // A scratch buffer for committed smudge strokes; ones in progress
        // keep theirs (they go on from where the replay leaves it).
        let mut temp: Option<TexId> = None;
        let mut batches = Batches::default();
        // The uncommitted strokes each tile now shows, in drawing order.
        let mut drawn: HashMap<Key, Vec<String>> = keys.iter().map(|k| (*k, Vec::new())).collect();
        for (seq, s) in strokes {
            let only: HashSet<Key> = keys.iter().copied().filter(|k| seq.is_none_or(|q| q > bases[k])).collect();
            if only.is_empty() {
                continue;
            }
            let dabs = raster::all_dabs(&s);
            if s.brush.smudges() {
                let Some(layer) = self.layers.get(layer_id) else { break };
                let touched: HashSet<Key> = dabs.iter().flat_map(|d| dab_tiles(&s.brush, d, layer)).collect();
                if touched.is_disjoint(&only) {
                    continue;
                }
                let buffer = if seq.is_none() {
                    self.buffer_for(gpu, &s.id)
                } else {
                    if temp.is_none() {
                        temp = gpu.create_paint_texture(SMUDGE_BUF);
                    }
                    temp
                };
                let Some(buffer) = buffer else { continue };
                batches.flush(gpu);
                self.run_smudge(gpu, layer_id, &s.brush, s.alpha_lock, &dabs, true, buffer, Some(&only), None);
                if seq.is_none() {
                    for k in touched.intersection(&only) {
                        drawn.entry(*k).or_default().push(s.id.clone());
                    }
                }
                continue;
            }
            let mode = mode_of(&s.brush, s.alpha_lock);
            let Some(layer) = self.layers.get(layer_id) else { break };
            for (k, data) in bucket(&s.brush, &dabs, layer, Some(&only)) {
                if let Some(tex) = layer.tiles.get(&k).and_then(|t| t.tex) {
                    batches.push(tex, &data, mode, self.style_for(&s.brush, k));
                }
                if seq.is_none() {
                    drawn.entry(k).or_default().push(s.id.clone());
                }
            }
        }
        batches.flush(gpu);
        if let Some(t) = temp {
            gpu.delete_texture(t);
        }
        if let Some(layer) = self.layers.get_mut(layer_id) {
            for (k, ids) in drawn {
                if let Some(t) = layer.tiles.get_mut(&k) {
                    t.provisional = ids;
                    t.interleaved = false;
                }
            }
        }
        self.dirty = true;
        true
    }

    /// Start base and view snapshots for tiles whose last stroke is ours.
    fn snapshot(&mut self, gpu: &mut dyn Backend, now: f64) {
        let Some(_) = self.layers.values().next() else { return };
        if self.active.is_some() {
            self.last_paint_ms = now;
        }
        let pausing = now - self.last_paint_ms < BASE_AFTER_IDLE_MS;
        let mut base_jobs: Vec<(String, Key, u64)> = Vec::new();
        let mut view_jobs: Vec<(String, Key, u64, TexId)> = Vec::new();
        let mut fetch: Vec<(String, Key, String)> = Vec::new();
        // The soonest a view snapshot that's still settling is due.
        let mut next_view = f64::INFINITY;
        for (lid, l) in self.layers.iter().filter(|(_, l)| l.present) {
            for (k, t) in &l.tiles {
                if !t.last_mine || !t.ready() || t.snapshotting {
                    continue;
                }
                if t.last_seq > t.view_sent && now - t.changed_at <= VIEW_AFTER_MS {
                    next_view = next_view.min(t.changed_at + VIEW_AFTER_MS - now);
                }
                // A new base is drawn from the current one's pixels.
                let base_ready = t.base_url.is_none() || t.base_img.is_some();
                if t.strokes.len() >= BASE_EVERY && !base_ready {
                    if !t.fetching {
                        fetch.push((lid.clone(), *k, t.base_url.clone().unwrap()));
                    }
                    continue;
                }
                if t.strokes.len() >= BASE_EVERY && pausing {
                    // Later, when painting pauses.
                    next_view = next_view.min(self.last_paint_ms + BASE_AFTER_IDLE_MS - now);
                } else if t.strokes.len() >= BASE_EVERY {
                    let cut = t.strokes[t.strokes.len() - UNDO_KEEP - 1];
                    base_jobs.push((lid.clone(), *k, cut));
                } else if t.provisional.is_empty() && t.last_seq > t.view_sent && now - t.changed_at > VIEW_AFTER_MS {
                    view_jobs.push((lid.clone(), *k, t.last_seq, t.tex.unwrap()));
                }
            }
        }
        if next_view.is_finite() {
            self.wake_in(next_view + 50.0);
        }
        for (lid, k, url) in fetch {
            if let Some(t) = self.layers.get_mut(&lid).and_then(|l| l.tiles.get_mut(&k)) {
                t.fetching = true;
            }
            start_image_load(url, lid, k, self.images.clone(), self.wake.clone());
        }
        for (lid, k, seq, tex) in view_jobs {
            if let Some(t) = self.layers.get_mut(&lid).and_then(|l| l.tiles.get_mut(&k)) {
                t.view_sent = seq;
            }
            let out = self.snapshots.clone();
            let wake = self.wake.clone();
            gpu.read_paint_texture(
                tex,
                TILE,
                Box::new(move |px| {
                    out.borrow_mut().push(SnapshotReady { layer: lid, x: k.0, y: k.1, kind: "view", seq, pixels: unpremultiply(px) });
                    wake.set(true);
                }),
            );
        }
        for (lid, k, cut) in base_jobs {
            let Some(base_seq) = self.layers.get(&lid).and_then(|l| l.tiles.get(&k)).map(|t| t.base_seq) else { continue };
            let replay: Vec<Rc<Stroke>> = self
                .committed
                .range(base_seq + 1..=cut)
                .filter(|(_, c)| !c.undone && c.stroke.layer == lid)
                .map(|(_, c)| c.stroke.clone())
                .collect();
            let mut ready = true;
            for s in &replay {
                ready &= self.ensure_assets(&s.brush);
            }
            if !ready {
                continue;
            }
            // Render the tile as of `cut` into a scratch texture and read it back.
            let Some(scratch) = gpu.create_paint_texture(TILE) else { continue };
            let Some(layer) = self.layers.get_mut(&lid) else { continue };
            let t = layer.tiles.get_mut(&k).unwrap();
            t.snapshotting = true;
            match &t.base_img {
                Some(img) if gpu.load_paint_texture(scratch, ImageSource::Image(img), TILE) => {}
                _ => gpu.clear_paint_texture(scratch, TILE),
            }
            let only: HashSet<Key> = [k].into_iter().collect();
            let mut temp: Option<TexId> = None;
            let mut batches = Batches::default();
            for s in replay {
                let dabs = raster::all_dabs(&s);
                if s.brush.smudges() {
                    // Neighbors are read as they are now (close enough for a
                    // snapshot; the painter's own view is exact).
                    if temp.is_none() {
                        temp = gpu.create_paint_texture(SMUDGE_BUF);
                    }
                    if let Some(buffer) = temp {
                        batches.flush(gpu);
                        self.run_smudge(gpu, &lid, &s.brush, s.alpha_lock, &dabs, true, buffer, Some(&only), Some((k, scratch)));
                    }
                    continue;
                }
                let Some(layer) = self.layers.get(&lid) else { break };
                if let Some(data) = bucket(&s.brush, &dabs, layer, Some(&only)).remove(&k) {
                    batches.push(scratch, &data, mode_of(&s.brush, s.alpha_lock), self.style_for(&s.brush, k));
                }
            }
            batches.flush(gpu);
            if let Some(t) = temp {
                gpu.delete_texture(t);
            }
            let (out, trash, wake) = (self.snapshots.clone(), self.trash.clone(), self.wake.clone());
            gpu.read_paint_texture(
                scratch,
                TILE,
                Box::new(move |px| {
                    out.borrow_mut().push(SnapshotReady { layer: lid, x: k.0, y: k.1, kind: "base", seq: cut, pixels: unpremultiply(px) });
                    trash.borrow_mut().push(scratch);
                    wake.set(true);
                }),
            );
        }
    }

    /// Snapshots read back and ready to upload.
    pub fn take_snapshots(&mut self) -> Vec<SnapshotReady> {
        std::mem::take(&mut *self.snapshots.borrow_mut())
    }

    /// An upload failed: try again in a while.
    pub fn snapshot_failed(&mut self, layer: &str, x: i32, y: i32) {
        if let Some(t) = self.layers.get_mut(layer).and_then(|l| l.tiles.get_mut(&(x, y))) {
            t.snapshotting = false;
            t.view_sent = 0;
        }
        self.wake_in(10_000.0);
    }

    /// Have the editor run `apply_gpu` after `ms`, once (one timer at a time).
    fn wake_in(&self, ms: f64) {
        if self.timer.replace(true) {
            return;
        }
        let (timer, due) = (self.timer.clone(), self.due.clone());
        let cb = Closure::once_into_js(move || {
            timer.set(false);
            due.set(true);
        });
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), ms.ceil() as i32);
        }
    }

    /// Whether anything is waiting on the GPU.
    pub fn busy(&self) -> bool {
        !self.queue.is_empty()
            || !self.freed.is_empty()
            || !self.images.borrow().is_empty()
            || !self.asset_loads.borrow().is_empty()
            || self.readbacks
            || self.layers.values().filter(|l| l.present).any(|l| l.tiles.values().any(|t| t.needs_rebuild && !t.loading))
    }
}

fn unpremultiply(mut px: Vec<u8>) -> Vec<u8> {
    for p in px.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    px
}

fn start_image_load(url: String, layer: String, k: Key, queue: Rc<RefCell<Vec<(String, Key, String, Option<HtmlImageElement>)>>>, wake: Rc<Cell<bool>>) {
    let Ok(img) = HtmlImageElement::new() else { return };
    let (q2, w2, l2, u2, i2) = (queue.clone(), wake.clone(), layer.clone(), url.clone(), img.clone());
    let onload = Closure::once(Box::new(move || {
        q2.borrow_mut().push((l2, k, u2, Some(i2)));
        w2.set(true);
    }) as Box<dyn FnOnce()>);
    let (q3, l3, u3) = (queue, layer, url.clone());
    let onerror = Closure::once(Box::new(move || {
        q3.borrow_mut().push((l3, k, u3, None));
        wake.set(true);
    }) as Box<dyn FnOnce()>);
    img.set_onload(Some(onload.as_ref().unchecked_ref()));
    img.set_onerror(Some(onerror.as_ref().unchecked_ref()));
    onload.forget();
    onerror.forget();
    img.set_src(&url);
}

/// JS-facing snapshot payload.
pub fn snapshot_js(s: &SnapshotReady) -> (JsValue, js_sys::Uint8Array) {
    let meta = json!({ "layer": s.layer, "x": s.x, "y": s.y, "kind": s.kind, "seq": s.seq }).to_string();
    (JsValue::from_str(&meta), js_sys::Uint8Array::from(&s.pixels[..]))
}
