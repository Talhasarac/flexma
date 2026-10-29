//! Editor core: document + scene + selection + history + sync. The wasm API in
//! `lib.rs` is a thin wrapper over this; interaction lives in `tools.rs`,
//! commands in `commands.rs` and overlay drawing in `overlay.rs`.

use crate::camera::Camera;
use crate::history::History;
use crate::render::Renderer;
use crate::scene::Scene;
use crate::sync::Sync;
use crate::text::TextEngine;
use crate::tools::{Gesture, Tool};
use flexma_doc::constraints::{self, Subtree};
use flexma_doc::{components, layout};
use flexma_doc::{Document, Node, NodeId, NodeType, Op, OpEffect, Rect, Vec2};
use serde_json::{json, Map, Value};
use std::collections::HashSet;

#[derive(Clone, Debug, Default)]
pub struct PeerView {
    pub client_id: String,
    pub name: String,
    pub color: String,
    pub selection: Vec<String>,
    pub page: Option<String>,
    pub is_agent: bool,
}

#[derive(Clone, Debug)]
pub struct Guide {
    /// Vertical guide at x (true) or horizontal guide at y (false).
    pub vertical: bool,
    pub pos: f64,
    pub from: f64,
    pub to: f64,
}

pub struct Editor {
    pub doc: Document,
    pub scene: Scene,
    pub renderer: Option<Renderer>,
    pub text: Option<TextEngine>,
    pub cam: Camera,
    pub css_w: f64,
    pub css_h: f64,
    pub dpr: f64,
    pub selection: Vec<NodeId>,
    pub hover: Option<NodeId>,
    pub tool: Tool,
    pub gesture: Gesture,
    pub history: History,
    pub sync: Sync,
    pub doc_version: u64,
    pub structure_version: u64,
    pub selection_version: u64,
    pub ui_version: u64,
    pub dirty: bool,
    pub editing_text: Option<NodeId>,
    pub peers: Vec<PeerView>,
    pub pointer: Vec2,
    pub space_down: bool,
    pub can_edit: bool,
    pub id_counter: u64,
    pub guides: Vec<Guide>,
    pub fonts_ready: bool,
    pub needs_measure: HashSet<NodeId>,
    /// Pages still streaming in after the first part (see `merge_page`).
    pub pending_pages: HashSet<NodeId>,
    /// A page asked for before it arrived; shown once it's merged.
    pub wanted_page: Option<NodeId>,
    pub last_camera_change: f64,
    pub frame_labels: Vec<(NodeId, Rect)>,
    pub drop_target: Option<NodeId>,
    pub insert_indicator: Option<(Vec2, Vec2)>,
    pub now_ms: f64,
    pub events: Vec<Value>,
    pub loaded: bool,
    /// Ids created during the current gesture (to select when it ends).
    pub pen_preview: Option<Vec2>,
    /// Timestamp of the last rendered frame (rate limit).
    pub last_frame_ms: f64,
    /// Paint layers: tiles, strokes and sync (see `paint.rs`).
    pub paint: crate::paint::PaintState,
    /// Brush and eraser settings.
    pub brush: flexma_doc::raster::Brush,
    pub eraser: flexma_doc::raster::Brush,
    /// Pen pressure of the latest pointer event (1 for mice).
    pub pressure: f32,
    /// Pen tilt of the latest pointer event: 0 upright … 1 flat, and the
    /// direction it leans on screen (radians, 0 = right, clockwise).
    pub tilt: [f32; 2],
    /// The stabilizer's smoothed position (layer px) while painting.
    pub smooth: Option<[f32; 3]>,
    /// The pointer is over the canvas (for the brush outline).
    pub pointer_in: bool,
    /// The workspace has painting turned on (B and E pick the brush and eraser).
    pub paint_tools: bool,
}

impl Editor {
    pub fn new(renderer: Option<Renderer>, text: Option<TextEngine>) -> Editor {
        let doc = Document::new_empty();
        let page = doc.pages()[0].clone();
        // Paint has its own wake: its events need a redraw, never a rebuild of
        // the whole document's display list.
        let paint = crate::paint::PaintState::new();
        Editor {
            doc,
            scene: Scene { page, dirty: true, ..Default::default() },
            renderer,
            text,
            cam: Camera::default(),
            css_w: 800.0,
            css_h: 600.0,
            dpr: 1.0,
            selection: vec![],
            hover: None,
            tool: Tool::Select,
            gesture: Gesture::None,
            history: History::default(),
            sync: Sync::new("local"),
            doc_version: 1,
            structure_version: 1,
            selection_version: 1,
            ui_version: 1,
            dirty: true,
            editing_text: None,
            peers: vec![],
            pointer: Vec2::ZERO,
            space_down: false,
            can_edit: true,
            id_counter: 0,
            guides: vec![],
            fonts_ready: false,
            needs_measure: HashSet::new(),
            pending_pages: HashSet::new(),
            wanted_page: None,
            last_camera_change: -1e9,
            frame_labels: vec![],
            drop_target: None,
            insert_indicator: None,
            now_ms: 0.0,
            events: vec![],
            loaded: false,
            pen_preview: None,
            last_frame_ms: f64::NEG_INFINITY,
            paint,
            brush: flexma_doc::raster::Brush::default(),
            eraser: flexma_doc::raster::Brush { size: 30.0, hardness: 0.6, erase: true, ..Default::default() },
            pressure: 1.0,
            tilt: [0.0, 0.0],
            smooth: None,
            pointer_in: false,
            paint_tools: false,
        }
    }

    // ---------------- ids ----------------

    pub fn new_id(&mut self) -> NodeId {
        self.id_counter += 1;
        format!("{}:{}", self.sync.client_id, self.id_counter)
    }

    // ---------------- loading & remote ----------------

    pub fn load(&mut self, doc_json: &str, client_id: &str, can_edit: bool) -> Result<(), String> {
        let unacked = if self.loaded { self.sync.take_unacked() } else { vec![] };
        let mut doc = Document::from_json(doc_json)?;
        components::sync_all(&mut doc);
        flexma_doc::styles::sync_all(&mut doc);
        layout::relayout_all(&mut doc);
        self.doc = doc;
        self.sync.reset(client_id);
        // The paint state is fetched again after (re)connecting.
        self.paint.client_id = client_id.to_string();
        self.paint.reset();
        self.can_edit = can_edit;
        if self.doc.get(&self.scene.page).map(|n| n.kind) != Some(NodeType::Page) {
            self.scene.page = self.doc.pages()[0].clone();
        }
        self.selection.retain(|id| self.doc.contains(id));
        self.loaded = true;
        self.pending_pages.clear();
        self.wanted_page = None;
        // Re-apply our edits that the server never acknowledged (reconnect).
        if !unacked.is_empty() && can_edit {
            let mut changed = Vec::new();
            for op in unacked {
                if let Ok(eff) = self.doc.apply(&op) {
                    changed.extend(eff.changed);
                    self.sync.push(op);
                }
            }
            let synced = components::sync_instances(&mut self.doc, &changed, &[]);
            changed.extend(synced);
            let styled = flexma_doc::styles::sync_styles(&mut self.doc, &changed);
            changed.extend(styled);
            layout::relayout(&mut self.doc, &changed);
            self.doc.clear_fresh();
        }
        self.doc.walk(flexma_doc::ROOT_ID, &mut |n| {
            if n.kind == NodeType::Text && n.needs_measure {
                self.needs_measure.insert(n.id.clone());
            }
        });
        self.bump_all();
        Ok(())
    }

    /// After loading a first part (`flexma_doc::parts::first_part`): the
    /// other pages stream in through `merge_page`, `first` is here.
    pub fn begin_stream(&mut self, first: &str) {
        self.pending_pages = self.doc.pages().into_iter().filter(|p| p != first).collect();
        self.set_page(first);
        self.ui_version += 1;
    }

    /// Merge a page that streamed in after the first part. Its instances
    /// expand, styles apply and auto layout runs, as when a file loads.
    pub fn merge_page(&mut self, json: &str) -> Result<NodeId, String> {
        let part: flexma_doc::parts::PagePart = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let page = part.page;
        let mut changed = self.doc.merge_nodes(part.nodes);
        let synced = components::sync_instances(&mut self.doc, &changed, &[]);
        changed.extend(synced);
        let styled = flexma_doc::styles::sync_styles(&mut self.doc, &changed);
        changed.extend(styled);
        layout::relayout(&mut self.doc, &changed);
        self.doc.clear_fresh();
        for id in &changed {
            if self.doc.get(id).is_some_and(|n| n.kind == NodeType::Text && n.needs_measure) {
                self.needs_measure.insert(id.clone());
            }
        }
        self.pending_pages.remove(&page);
        self.bump_all();
        if self.wanted_page.as_deref() == Some(page.as_str()) {
            self.set_page(&page);
        }
        Ok(page)
    }

    pub fn bump_all(&mut self) {
        self.doc_version += 1;
        self.structure_version += 1;
        self.selection_version += 1;
        self.ui_version += 1;
        self.scene.dirty = true;
        self.dirty = true;
    }

    /// Apply a server message `{t:"ops", from, batch, ops}`.
    pub fn apply_remote(&mut self, from: &str, batch: u64, ops: Vec<Op>) {
        if from == self.sync.client_id {
            self.sync.ack(batch);
            return;
        }
        let page_was_empty = self.doc.children(&self.scene.page).is_empty();
        let mut eff = OpEffect::default();
        let trusted = from == "server";
        for op in ops {
            let op = if trusted { Some(op) } else { self.sync.filter_remote(op) };
            let Some(op) = op else { continue };
            // A create for a node we already have (e.g. correction) replaces it.
            if let Op::Create { node } = &op {
                if self.doc.contains(&node.id) {
                    let props: Map<String, Value> = match node.to_json() {
                        Value::Object(m) => m.into_iter().filter(|(k, _)| k != "id" && k != "type").collect(),
                        _ => Map::new(),
                    };
                    if let Ok(e) = self.doc.apply(&Op::Update { id: node.id.clone(), props }) {
                        eff.merge(e);
                    }
                    continue;
                }
            }
            if let Ok(e) = self.doc.apply(&op) {
                eff.merge(e);
            }
        }
        for id in &eff.changed {
            if let Some(n) = self.doc.get(id) {
                if n.kind == NodeType::Text && n.needs_measure {
                    self.needs_measure.insert(id.clone());
                }
            }
        }
        let laid = self.derive(&mut eff);
        self.after_effect(&eff, &laid);
        // Someone (often an agent) just put the first content on the page we're
        // looking at: bring it into view.
        if page_was_empty && !self.doc.children(&self.scene.page).is_empty() && matches!(self.gesture, crate::tools::Gesture::None) {
            self.zoom_to_fit(false);
        }
    }

    /// Derived state after ops: re-expand affected instances, then auto layout.
    /// Returns nodes whose derived geometry/content changed.
    fn derive(&mut self, eff: &mut OpEffect) -> Vec<NodeId> {
        let synced = components::sync_instances(&mut self.doc, &eff.changed, &eff.removed);
        if !synced.is_empty() {
            eff.structure = true;
        }
        let mut changed = eff.changed.clone();
        changed.extend(synced.iter().cloned());
        // Shared styles: a changed style updates every layer using it.
        if changed.iter().any(|id| self.doc.get(id).map(|n| n.kind == NodeType::Style).unwrap_or(false)) {
            eff.structure = true;
        }
        let styled = flexma_doc::styles::sync_styles(&mut self.doc, &changed);
        for id in &styled {
            if self.doc.get(id).map(|n| n.kind == NodeType::Text && n.needs_measure).unwrap_or(false) {
                self.needs_measure.insert(id.clone());
            }
        }
        changed.extend(styled);
        let mut laid = layout::relayout(&mut self.doc, &changed);
        // Text that layout rewrapped at a new width.
        for id in &laid {
            if self.doc.get(id).is_some_and(|n| n.kind == NodeType::Text && n.needs_measure) {
                self.needs_measure.insert(id.clone());
            }
        }
        laid.extend(synced);
        self.doc.clear_fresh();
        laid
    }

    pub fn after_effect(&mut self, eff: &OpEffect, laid: &[NodeId]) {
        // Ops that set values nodes already had (often another editor's).
        if !eff.structure && eff.changed.is_empty() && eff.removed.is_empty() && laid.is_empty() {
            return;
        }
        self.doc_version += 1;
        if eff.structure {
            self.structure_version += 1;
        }
        if !eff.removed.is_empty() {
            let before = self.selection.len();
            self.selection.retain(|id| self.doc.contains(id));
            if self.selection.len() != before {
                self.selection_version += 1;
            }
            if let Some(h) = &self.hover {
                if !self.doc.contains(h) {
                    self.hover = None;
                }
            }
            if let Some(t) = &self.editing_text {
                if !self.doc.contains(t) {
                    self.editing_text = None;
                    self.ui_version += 1;
                }
            }
        }
        if eff.changed.iter().chain(laid.iter()).any(|id| self.selection.contains(id)) {
            self.selection_version += 1;
        }
        // Renames / visibility changes show in the layers panel.
        if !eff.structure && eff.changed.len() <= 64 {
            self.structure_version += 1;
        }
        if self.doc.get(&self.scene.page).is_none() {
            if let Some(p) = self.doc.pages().first() {
                self.scene.page = p.clone();
                self.ui_version += 1;
            }
        }
        self.scene.dirty = true;
        self.dirty = true;
    }

    // ---------------- local edits ----------------

    /// Apply local ops: record undo, queue for sync, re-run auto layout.
    pub fn apply_local(&mut self, ops: Vec<Op>) -> Vec<String> {
        if !self.can_edit || ops.is_empty() {
            return vec![];
        }
        // Edits aimed inside instances become overrides on the instance.
        let ops = components::route_ops(&self.doc, flexma_doc::styles::route_ops(&self.doc, ops));
        let auto_txn = !self.history.is_open();
        if auto_txn {
            let sel = self.selection.clone();
            self.history.begin("edit", &sel);
        }
        let mut eff = OpEffect::default();
        let mut errors = Vec::new();
        for op in ops {
            let inv = self.doc.inverse_of(&op);
            match self.doc.apply(&op) {
                // It set values the node already had: nothing to sync or undo.
                Ok(e) if e.changed.is_empty() && e.removed.is_empty() && !e.structure => {}
                Ok(e) => {
                    self.history.record(inv);
                    self.sync.push(op);
                    eff.merge(e);
                }
                Err(e) => errors.push(e),
            }
        }
        let laid = self.derive(&mut eff);
        self.after_effect(&eff, &laid);
        if auto_txn {
            self.history.end();
        }
        errors
    }

    /// Local ops that are not user actions (measurement fixes): synced, not undoable.
    pub fn apply_system(&mut self, ops: Vec<Op>) {
        if !self.can_edit {
            return;
        }
        let ops = components::route_ops(&self.doc, flexma_doc::styles::route_ops(&self.doc, ops));
        let mut eff = OpEffect::default();
        for op in ops {
            match self.doc.apply(&op) {
                Ok(e) if e.changed.is_empty() && e.removed.is_empty() && !e.structure => {}
                Ok(e) => {
                    self.sync.push(op);
                    eff.merge(e);
                }
                Err(_) => {}
            }
        }
        let laid = self.derive(&mut eff);
        self.after_effect(&eff, &laid);
    }

    pub fn begin(&mut self, label: &str) {
        let sel = self.selection.clone();
        self.history.begin(label, &sel);
    }

    pub fn end(&mut self) {
        self.history.end();
        self.ui_version += 1;
    }

    pub fn undo(&mut self) {
        if !self.can_edit {
            return;
        }
        let Some(txn) = self.history.pop_undo() else { return };
        if let Some(id) = &txn.stroke {
            // A stroke merged into a snapshot can't be undone any more; skip it.
            if self.paint.undo(id, self.now_ms) {
                self.history.push_redo(txn.clone());
            }
            self.dirty = true;
            self.ui_version += 1;
            return;
        }
        let redo = self.apply_inverse(&txn.inverse);
        let mut r = txn.clone();
        r.inverse = redo;
        r.selection = self.selection.clone();
        self.history.push_redo(r);
        self.set_selection(txn.selection.into_iter().filter(|id| self.doc.contains(id)).collect());
        self.ui_version += 1;
    }

    pub fn redo(&mut self) {
        if !self.can_edit {
            return;
        }
        let Some(txn) = self.history.pop_redo() else { return };
        if let Some(id) = &txn.stroke {
            if self.paint.redo(id, self.now_ms) {
                self.history.push_undo_keep_redo(txn.clone());
            }
            self.dirty = true;
            self.ui_version += 1;
            return;
        }
        let undo = self.apply_inverse(&txn.inverse);
        let mut u = txn.clone();
        u.inverse = undo;
        u.selection = self.selection.clone();
        self.history.push_undo_keep_redo(u);
        self.set_selection(txn.selection.into_iter().filter(|id| self.doc.contains(id)).collect());
        self.ui_version += 1;
    }

    /// Apply inverse ops (in reverse order); returns the ops that undo *that*.
    fn apply_inverse(&mut self, inverse: &[Op]) -> Vec<Op> {
        let mut back = Vec::new();
        let mut eff = OpEffect::default();
        for op in inverse.iter().rev() {
            let inv = self.doc.inverse_of(op);
            if let Ok(e) = self.doc.apply(op) {
                back.push(inv);
                self.sync.push(op.clone());
                eff.merge(e);
            }
        }
        let laid = self.derive(&mut eff);
        self.after_effect(&eff, &laid);
        // `back` holds per-op inverse lists in application order; flatten so that
        // applying it reversed restores the state before this call.
        back.into_iter().rev().flatten().collect::<Vec<_>>().into_iter().rev().collect()
    }

    // ---------------- selection ----------------

    pub fn set_selection(&mut self, ids: Vec<NodeId>) {
        let mut ids: Vec<NodeId> = ids.into_iter().filter(|id| self.doc.get(id).map(|n| n.kind.is_scene()).unwrap_or(false)).collect();
        ids.dedup();
        if ids != self.selection {
            self.selection = ids;
            self.selection_version += 1;
            self.dirty = true;
            self.events.push(json!({ "type": "selection" }));
        }
    }

    /// Selected nodes without those whose ancestor is also selected.
    pub fn top_selection(&self) -> Vec<NodeId> {
        let set: HashSet<&NodeId> = self.selection.iter().collect();
        self.selection
            .iter()
            .filter(|id| !self.doc.ancestors(id).iter().any(|a| set.contains(a)))
            .filter(|id| self.doc.contains(id))
            .cloned()
            .collect()
    }

    pub fn ensure_scene(&mut self) {
        if self.scene.dirty {
            self.scene.rebuild(&self.doc);
        }
    }

    /// World AABB of the selection.
    pub fn selection_bounds(&mut self) -> Rect {
        self.ensure_scene();
        let mut r = Rect::empty();
        for id in &self.selection {
            if let Some(b) = self.scene.aabb.get(id) {
                r = r.union(b);
            }
        }
        r
    }

    // ---------------- text measurement ----------------

    /// Measure and fix the box of text nodes that need it.
    pub fn process_measurements(&mut self) {
        if !self.fonts_ready || self.needs_measure.is_empty() || self.text.is_none() || !self.can_edit {
            return;
        }
        let ids: Vec<NodeId> = self.needs_measure.drain().collect();
        let mut ops = Vec::new();
        for id in ids {
            let Some(n) = self.doc.get(&id).cloned() else { continue };
            if n.kind != NodeType::Text {
                continue;
            }
            let (w, h) = self.text.as_mut().unwrap().measure_box(&n);
            let mut props = Map::new();
            if (w - n.width).abs() > 0.5 {
                props.insert("width".into(), json!(w));
            }
            if (h - n.height).abs() > 0.5 {
                props.insert("height".into(), json!(h));
            }
            if n.text_auto_resize == flexma_doc::TextAutoResize::Height {
                props.insert("measuredWidth".into(), json!(n.width));
            }
            props.insert("needsMeasure".into(), json!(false));
            ops.push(Op::Update { id, props });
        }
        self.apply_system(ops);
    }

    /// Props to add so a text node's box follows its content after `props` apply.
    pub fn text_box_props(&mut self, n: &Node, props: &Map<String, Value>) -> Map<String, Value> {
        let mut out = Map::new();
        if n.kind != NodeType::Text || self.text.is_none() {
            return out;
        }
        let mut probe = n.clone();
        for (k, v) in props {
            let _ = probe.set_prop(k, v);
        }
        let (w, h) = self.text.as_mut().unwrap().measure_box(&probe);
        if probe.text_auto_resize == flexma_doc::TextAutoResize::WidthAndHeight && (w - probe.width).abs() > 0.01 {
            out.insert("width".into(), json!(w));
        }
        if probe.text_auto_resize != flexma_doc::TextAutoResize::None && (h - probe.height).abs() > 0.01 {
            out.insert("height".into(), json!(h));
        }
        if probe.text_auto_resize == flexma_doc::TextAutoResize::Height && probe.measured_width != Some(probe.width) {
            out.insert("measuredWidth".into(), json!(probe.width));
        }
        if probe.needs_measure {
            out.insert("needsMeasure".into(), json!(false));
        }
        out
    }

    /// Child updates that keep a captured frame's or group's descendants
    /// following their constraints when it becomes `w`×`h`. Text boxes that
    /// change width are measured for real.
    pub fn follow_resize(&mut self, tree: &Subtree, w: f64, h: f64) -> Vec<Op> {
        let mut ops = constraints::resize_ops(tree, w, h);
        for op in &mut ops {
            let Op::Update { id, props } = op else { continue };
            if props.get("needsMeasure") != Some(&json!(true)) {
                continue;
            }
            let Some(n) = tree.get(id).cloned() else { continue };
            let exact = self.text_box_props(&n, props);
            if exact.is_empty() {
                // No text engine yet: measure once fonts are ready.
                self.needs_measure.insert(id.clone());
            }
            props.extend(exact);
        }
        ops
    }

    /// Update nodes from the properties panel: `[{id, props}]` with canonical
    /// prop names. Text boxes are re-measured, and children of resized frames
    /// follow their constraints.
    pub fn update_nodes(&mut self, updates: Vec<(NodeId, Map<String, Value>)>) -> Vec<String> {
        let mut ops = Vec::new();
        let mut follow = Vec::new();
        for (id, mut props) in updates {
            let Some(n) = self.doc.get(&id).cloned() else { continue };
            // Resizing a hugging auto-layout frame makes that axis fixed.
            if n.kind.is_frame_like() {
                if props.contains_key("width") && !props.contains_key("layoutSizingHorizontal") && n.layout_sizing_horizontal == flexma_doc::SizingMode::Hug {
                    props.insert("layoutSizingHorizontal".into(), json!("FIXED"));
                }
                if props.contains_key("height") && !props.contains_key("layoutSizingVertical") && n.layout_sizing_vertical == flexma_doc::SizingMode::Hug {
                    props.insert("layoutSizingVertical".into(), json!("FIXED"));
                }
            }
            if n.kind == NodeType::Text {
                if props.contains_key("width") && !props.contains_key("textAutoResize") && n.text_auto_resize == flexma_doc::TextAutoResize::WidthAndHeight {
                    props.insert("textAutoResize".into(), json!("HEIGHT"));
                }
                if props.contains_key("height") && !props.contains_key("textAutoResize") && n.text_auto_resize != flexma_doc::TextAutoResize::None {
                    props.insert("textAutoResize".into(), json!("NONE"));
                }
                let extra = self.text_box_props(&n, &props);
                for (k, v) in extra {
                    props.entry(k).or_insert(v);
                }
            }
            if n.kind == NodeType::Vector && (props.contains_key("width") || props.contains_key("height")) && !props.contains_key("pathData") {
                let nw = props.get("width").and_then(Value::as_f64).unwrap_or(n.width);
                let nh = props.get("height").and_then(Value::as_f64).unwrap_or(n.height);
                if n.width > 0.0 && n.height > 0.0 {
                    let segs = flexma_doc::path::scale(&flexma_doc::path::parse(&n.path_data), nw / n.width, nh / n.height);
                    props.insert("pathData".into(), json!(flexma_doc::path::to_svg_d(&segs)));
                }
            }
            if props.contains_key("width") || props.contains_key("height") {
                let w = props.get("width").and_then(Value::as_f64).unwrap_or(n.width);
                let h = props.get("height").and_then(Value::as_f64).unwrap_or(n.height);
                let tree = Subtree::capture(&self.doc, &id);
                follow.extend(self.follow_resize(&tree, w, h));
            }
            ops.push(Op::Update { id, props });
        }
        // Explicit edits go last so they win over constraint results.
        follow.extend(ops);
        self.apply_local(follow)
    }

    // ---------------- queries for the UI ----------------

    pub fn layers_json(&self) -> Value {
        fn rec(doc: &Document, id: &str) -> Value {
            let n = doc.get(id).unwrap();
            // Front-most first, like every layers panel.
            let kids: Vec<Value> = doc.children(id).iter().rev().map(|c| rec(doc, c)).collect();
            json!({
                "id": n.id,
                "name": n.name,
                "type": n.kind,
                "visible": n.visible,
                "locked": n.locked,
                "layout": n.layout_mode != flexma_doc::LayoutMode::None,
                "derived": flexma_doc::node::is_derived_id(&n.id),
                "children": kids,
            })
        }
        let page = &self.scene.page;
        let kids: Vec<Value> = self.doc.children(page).iter().rev().map(|c| rec(&self.doc, c)).collect();
        Value::Array(kids)
    }

    pub fn pages_json(&self) -> Value {
        let pages: Vec<Value> = self
            .doc
            .pages()
            .iter()
            .filter_map(|id| self.doc.get(id))
            .map(|p| json!({ "id": p.id, "name": p.name, "loading": self.pending_pages.contains(&p.id) }))
            .collect();
        json!({ "pages": pages, "current": self.scene.page })
    }

    pub fn selection_json(&self) -> Value {
        let nodes: Vec<Value> = self
            .selection
            .iter()
            .filter_map(|id| self.doc.get(id))
            .map(|n| {
                let mut j = full_json(n);
                let parent = n.parent.as_deref().and_then(|p| self.doc.get(p));
                j["parentType"] = json!(parent.map(|p| p.kind));
                j["parentLayout"] = json!(parent.map(|p| p.layout_mode).unwrap_or_default());
                j["derived"] = json!(flexma_doc::node::is_derived_id(&n.id));
                if n.kind == NodeType::Instance {
                    let comp = n.component_id.as_deref().and_then(|c| self.doc.get(c));
                    j["componentName"] = json!(comp.map(|c| c.name.clone()));
                    j["overrideCount"] = json!(n.overrides.len());
                    if let Some(c) = comp {
                        if !c.variant.is_empty() {
                            j["componentVariant"] = json!(c.variant);
                            j["variantOptions"] = json!(flexma_doc::components::variant_options(&self.doc, &c.id));
                        }
                        if !c.component_properties.is_empty() {
                            j["componentPropertyValues"] = json!(flexma_doc::components::property_values(&self.doc, n));
                        }
                    }
                }
                if n.kind == NodeType::Component {
                    j["instanceCount"] = json!(flexma_doc::components::instances_of(&self.doc, &n.id).len());
                }
                j
            })
            .collect();
        Value::Array(nodes)
    }

    pub fn set_page(&mut self, page: &str) {
        if self.pending_pages.contains(page) {
            // Still streaming in: switch once it's merged.
            self.wanted_page = Some(page.to_string());
            self.ui_version += 1;
            self.dirty = true;
            return;
        }
        if self.wanted_page.take().is_some() {
            self.ui_version += 1;
        }
        if self.doc.get(page).map(|n| n.kind) == Some(NodeType::Page) && self.scene.page != page {
            self.scene.page = page.to_string();
            self.scene.dirty = true;
            self.selection.clear();
            self.selection_version += 1;
            self.structure_version += 1;
            self.ui_version += 1;
            self.hover = None;
            self.editing_text = None;
            self.dirty = true;
            if let Some(r) = self.renderer.as_mut() {
                r.invalidate();
            }
            self.zoom_to_fit(false);
        }
    }

    pub fn zoom_to_fit(&mut self, selection_only: bool) {
        self.ensure_scene();
        let r = if selection_only && !self.selection.is_empty() {
            let mut r = Rect::empty();
            for id in self.selection.clone() {
                r = r.union(&self.scene.subtree_aabb(&id));
            }
            r
        } else {
            self.scene.content_bounds()
        };
        if r.is_empty() {
            self.cam = Camera { x: self.css_w / 2.0, y: self.css_h / 2.0, zoom: 1.0 };
        } else {
            self.cam.fit(&r, self.css_w, self.css_h, 64.0, if selection_only { 8.0 } else { 1.0 });
        }
        self.camera_changed();
    }

    pub fn camera_changed(&mut self) {
        self.last_camera_change = self.now_ms;
        self.dirty = true;
        self.ui_version += 1;
    }

    pub fn interacting(&self) -> bool {
        self.now_ms - self.last_camera_change < 180.0
    }

    pub fn page_background(&self) -> flexma_doc::Color {
        self.doc
            .get(&self.scene.page)
            .and_then(|p| p.background)
            .unwrap_or(flexma_doc::Color::rgba(0.961, 0.961, 0.961, 1.0))
    }

    // ---------------- rendering ----------------

    pub fn render(&mut self) -> bool {
        // Never render more often than a fast display refreshes, however often
        // we're called; this keeps the GPU queue from piling up.
        if self.now_ms >= self.last_frame_ms && self.now_ms - self.last_frame_ms < 4.0 {
            return false;
        }
        self.process_measurements();
        let wake = self.renderer.as_ref().map(|r| r.wake.replace(false)).unwrap_or(false);
        let settled_text = !self.interacting() && self.now_ms - self.last_camera_change < 400.0;
        let paint = std::mem::take(&mut self.paint.dirty) || self.paint.busy() || self.paint.due.replace(false) || self.paint.wake.replace(false);
        if !self.dirty && !wake && !settled_text && !paint {
            return false;
        }
        self.paint.sync_layers(&self.doc, self.doc_version);
        self.ensure_scene();
        let (overlay, labels) = self.build_overlay();
        let mut hidden = HashSet::new();
        if let Some(t) = &self.editing_text {
            hidden.insert(t.clone());
        }
        let version = self.doc_version ^ (self.scene.rev << 32) ^ (hidden.len() as u64) << 60;
        let bg = self.page_background();
        let interacting = self.interacting();
        let (Some(r), Some(text)) = (self.renderer.as_mut(), self.text.as_mut()) else {
            self.dirty = false;
            return false;
        };
        if r.built_version != version || wake {
            r.build(&self.doc, &self.scene, &hidden, version);
        }
        let linear = self.doc.get(&self.scene.page).is_some_and(|p| p.linear_blending);
        r.draw(&self.doc, &self.scene, text, &mut self.paint, self.now_ms, &self.cam, self.css_w, self.css_h, self.dpr, bg, linear, &overlay, &labels, interacting);
        self.last_frame_ms = self.now_ms;
        self.dirty = r.wants_more;
        true
    }
}

/// Node JSON with every property (not just non-defaults) for the properties panel.
pub fn full_json(n: &Node) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(n.id));
    m.insert("type".into(), json!(n.kind));
    for k in Node::PROPS {
        if let Some(v) = n.get_prop(k) {
            m.insert((*k).to_string(), v);
        }
    }
    Value::Object(m)
}

pub fn effect_visible(e: &flexma_doc::Effect) -> bool {
    match e {
        flexma_doc::Effect::DropShadow { visible, .. } | flexma_doc::Effect::LayerBlur { visible, .. } | flexma_doc::Effect::BackgroundBlur { visible, .. } => *visible,
    }
}
