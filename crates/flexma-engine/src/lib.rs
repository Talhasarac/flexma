//! # flexma-engine
//!
//! The Flexma editor engine compiled to WebAssembly. It owns the document
//! replica, the scene, interaction and a WebGL2 renderer. JavaScript forwards
//! input events and network messages in, and reads small JSON snapshots out
//! (only when version counters change), keeping React out of the hot path.

pub mod camera;
pub mod commands;
pub mod editor;
pub mod gl;
pub mod history;
pub mod keys;
pub mod overlay;
pub mod render;
pub mod scene;
pub mod sync;
pub mod text;
pub mod tools;

use editor::{Editor, PeerView};
use flexma_doc::{NodeId, Op, Vec2};
use serde_json::{Map, Value};
use tools::{Mods, Tool};
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

fn mods(shift: bool, alt: bool, meta: bool) -> Mods {
    Mods { shift, alt, meta }
}

fn parse_ids(json: &str) -> Vec<NodeId> {
    serde_json::from_str(json).unwrap_or_default()
}

#[wasm_bindgen]
pub struct FlexmaEditor {
    ed: Editor,
}

#[wasm_bindgen]
impl FlexmaEditor {
    #[wasm_bindgen(constructor)]
    pub fn new(canvas: HtmlCanvasElement) -> Result<FlexmaEditor, JsValue> {
        console_error_panic_hook::set_once();
        let gl = gl::context(&canvas).map_err(|e| JsValue::from_str(&e))?;
        let renderer = render::Renderer::new(gl).map_err(|e| JsValue::from_str(&e))?;
        let text = text::TextEngine::new()?;
        Ok(FlexmaEditor { ed: Editor::new(Some(renderer), Some(text)) })
    }

    /// Load (or reload after reconnect) the document snapshot from the server.
    pub fn load(&mut self, doc_json: &str, client_id: &str, can_edit: bool) -> Result<(), JsValue> {
        self.ed.load(doc_json, client_id, can_edit).map_err(|e| JsValue::from_str(&e))
    }

    pub fn set_asset_base(&mut self, base: &str) {
        if let Some(r) = self.ed.renderer.as_mut() {
            r.asset_base = base.to_string();
        }
    }

    pub fn resize(&mut self, css_w: f64, css_h: f64, dpr: f64) {
        let first = self.ed.css_w == 800.0 && self.ed.css_h == 600.0 && self.ed.dpr == 1.0;
        self.ed.css_w = css_w.max(1.0);
        self.ed.css_h = css_h.max(1.0);
        self.ed.dpr = dpr.max(0.5);
        if first {
            self.ed.zoom_to_fit(false);
        }
        self.ed.dirty = true;
    }

    /// Render if anything changed. Call every animation frame.
    pub fn frame(&mut self, now_ms: f64) -> bool {
        self.ed.now_ms = now_ms;
        self.ed.render()
    }

    pub fn zoom_to_fit_initial(&mut self) {
        self.ed.zoom_to_fit(false);
    }

    // ---------- input ----------

    pub fn pointer_down(&mut self, x: f64, y: f64, button: i32, shift: bool, alt: bool, meta: bool) {
        self.ed.pointer_down(Vec2::new(x, y), button, mods(shift, alt, meta));
        self.ed.dirty = true;
    }

    pub fn pointer_move(&mut self, x: f64, y: f64, shift: bool, alt: bool, meta: bool) {
        self.ed.pointer_move(Vec2::new(x, y), mods(shift, alt, meta));
    }

    pub fn pointer_up(&mut self, x: f64, y: f64, shift: bool, alt: bool, meta: bool) {
        self.ed.pointer_up(Vec2::new(x, y), mods(shift, alt, meta));
    }

    pub fn pointer_leave(&mut self) {
        if self.ed.hover.take().is_some() {
            self.ed.dirty = true;
        }
    }

    pub fn double_click(&mut self, x: f64, y: f64) {
        self.ed.double_click(Vec2::new(x, y));
        self.ed.dirty = true;
    }

    pub fn wheel(&mut self, dx: f64, dy: f64, x: f64, y: f64, zoom: bool) {
        self.ed.wheel(dx, dy, Vec2::new(x, y), zoom);
    }

    /// Abort the current pointer gesture (e.g. a second finger turned it into a pinch).
    pub fn cancel_gesture(&mut self) {
        match std::mem::replace(&mut self.ed.gesture, tools::Gesture::None) {
            tools::Gesture::None | tools::Gesture::Pan { .. } | tools::Gesture::Pending { .. } | tools::Gesture::Marquee { .. } => {}
            tools::Gesture::Pen { .. } => self.ed.finish_pen(),
            _ => self.ed.end(),
        }
        self.ed.guides.clear();
        self.ed.drop_target = None;
        self.ed.insert_indicator = None;
        self.ed.dirty = true;
    }

    pub fn pinch(&mut self, scale: f64, x: f64, y: f64) {
        self.ed.pinch(scale, Vec2::new(x, y));
    }

    pub fn key_down(&mut self, key: &str, shift: bool, alt: bool, meta: bool) -> bool {
        self.ed.key_down(key, mods(shift, alt, meta))
    }

    pub fn key_up(&mut self, key: &str, shift: bool, alt: bool, meta: bool) {
        self.ed.key_up(key, mods(shift, alt, meta));
    }

    pub fn set_tool(&mut self, name: &str) {
        if let Some(t) = Tool::parse(name) {
            self.ed.set_tool(t);
        }
    }

    pub fn tool(&self) -> String {
        self.ed.tool.as_str().to_string()
    }

    pub fn cursor(&mut self) -> String {
        self.ed.cursor()
    }

    pub fn is_gesture_active(&self) -> bool {
        !matches!(self.ed.gesture, tools::Gesture::None)
    }

    // ---------- multiplayer ----------

    /// Handle an `ops` message from the server.
    pub fn receive_ops(&mut self, from: &str, batch: f64, ops_json: &str) {
        let ops: Vec<Value> = serde_json::from_str(ops_json).unwrap_or_default();
        let ops: Vec<Op> = ops.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect();
        self.ed.apply_remote(from, batch as u64, ops);
    }

    /// Next batch of local ops to send (JSON message), if any.
    pub fn take_outgoing(&mut self) -> Option<String> {
        self.ed.sync.flush()
    }

    pub fn has_outgoing(&self) -> bool {
        self.ed.sync.has_outgoing()
    }

    pub fn unacked(&self) -> u32 {
        self.ed.sync.inflight_count() as u32
    }

    /// `[{clientId, name, color, selection, page, isAgent}]`
    pub fn set_peers(&mut self, json: &str) {
        let v: Vec<Value> = serde_json::from_str(json).unwrap_or_default();
        self.ed.peers = v
            .into_iter()
            .map(|p| PeerView {
                client_id: p["clientId"].as_str().unwrap_or_default().to_string(),
                name: p["name"].as_str().unwrap_or_default().to_string(),
                color: p["color"].as_str().unwrap_or("#6d5dfc").to_string(),
                selection: serde_json::from_value(p["selection"].clone()).unwrap_or_default(),
                page: p["page"].as_str().map(str::to_string),
                is_agent: p["isAgent"].as_bool().unwrap_or(false),
            })
            .collect();
        self.ed.dirty = true;
    }

    // ---------- versions & snapshots for the UI ----------

    pub fn doc_version(&self) -> f64 {
        self.ed.doc_version as f64
    }
    pub fn structure_version(&self) -> f64 {
        self.ed.structure_version as f64
    }
    pub fn selection_version(&self) -> f64 {
        self.ed.selection_version as f64
    }
    pub fn ui_version(&self) -> f64 {
        self.ed.ui_version as f64
    }

    pub fn layers(&self) -> String {
        self.ed.layers_json().to_string()
    }
    pub fn pages(&self) -> String {
        self.ed.pages_json().to_string()
    }
    pub fn selection(&self) -> String {
        self.ed.selection_json().to_string()
    }
    pub fn selection_ids(&self) -> String {
        serde_json::to_string(&self.ed.selection).unwrap_or_default()
    }
    pub fn node(&self, id: &str) -> Option<String> {
        self.ed.doc.get(id).map(|n| editor::full_json(n).to_string())
    }
    pub fn current_page(&self) -> String {
        self.ed.scene.page.clone()
    }
    pub fn can_undo(&self) -> bool {
        self.ed.history.can_undo()
    }
    pub fn can_redo(&self) -> bool {
        self.ed.history.can_redo()
    }
    pub fn can_edit(&self) -> bool {
        self.ed.can_edit
    }

    /// UI events since the last call (JSON array).
    pub fn take_events(&mut self) -> String {
        let ev = std::mem::take(&mut self.ed.events);
        Value::Array(ev).to_string()
    }

    pub fn stats(&self) -> String {
        serde_json::json!({
            "nodes": self.ed.doc.len(),
            "instances": self.ed.renderer.as_ref().map(|r| r.instance_count()).unwrap_or(0),
            "drawCalls": self.ed.renderer.as_ref().map(|r| r.draw_calls).unwrap_or(0),
            "textMemoryMB": self.ed.renderer.as_ref().map(|r| r.text_memory() as f64 / 1048576.0).unwrap_or(0.0),
            "zoom": self.ed.cam.zoom,
        })
        .to_string()
    }

    // ---------- selection & edits ----------

    pub fn set_selection(&mut self, ids_json: &str) {
        let ids = parse_ids(ids_json);
        self.ed.set_selection(ids);
    }

    pub fn select_all(&mut self) {
        self.ed.select_all();
    }

    /// Node a click at screen (x, y) would select, if any.
    pub fn hit_test(&mut self, x: f64, y: f64, deep: bool) -> Option<String> {
        self.ed.ensure_scene();
        let w = self.ed.cam.to_world(Vec2::new(x, y));
        self.ed.resolve_click(w, deep)
    }

    pub fn hover_node(&mut self, id: Option<String>) {
        if self.ed.hover != id {
            self.ed.hover = id;
            self.ed.dirty = true;
        }
    }

    /// `[{id, props}]` with canonical property names. Returns errors as JSON.
    pub fn update_nodes(&mut self, json: &str) -> String {
        let v: Vec<Value> = serde_json::from_str(json).unwrap_or_default();
        let updates: Vec<(NodeId, Map<String, Value>)> = v
            .into_iter()
            .filter_map(|u| {
                let id = u["id"].as_str()?.to_string();
                let props = u["props"].as_object()?.clone();
                Some((id, props))
            })
            .collect();
        let errs = self.ed.update_nodes(updates);
        serde_json::to_string(&errs).unwrap_or_default()
    }

    /// Apply the same props to every selected node.
    pub fn update_selection(&mut self, props_json: &str) -> String {
        let Ok(Value::Object(props)) = serde_json::from_str::<Value>(props_json) else { return "[]".into() };
        let updates = self.ed.selection.iter().map(|id| (id.clone(), props.clone())).collect();
        let errs = self.ed.update_nodes(updates);
        serde_json::to_string(&errs).unwrap_or_default()
    }

    /// Group several edits (e.g. a slider drag) into one undo step.
    pub fn begin(&mut self, label: &str) {
        self.ed.begin(label);
    }
    pub fn end(&mut self) {
        self.ed.end();
    }

    pub fn undo(&mut self) {
        self.ed.undo();
    }
    pub fn redo(&mut self) {
        self.ed.redo();
    }
    pub fn delete_selection(&mut self) {
        self.ed.delete_selection();
    }
    pub fn duplicate(&mut self) {
        self.ed.duplicate();
    }
    pub fn group(&mut self) {
        self.ed.wrap_selection(flexma_doc::NodeType::Group, false);
    }
    pub fn ungroup(&mut self) {
        self.ed.ungroup();
    }
    pub fn frame_selection(&mut self) {
        self.ed.wrap_selection(flexma_doc::NodeType::Frame, false);
    }
    // ---------- components ----------

    pub fn create_component(&mut self) -> Option<String> {
        self.ed.create_component()
    }

    // ---------- shared styles ----------

    pub fn styles(&self) -> String {
        self.ed.styles_json().to_string()
    }
    /// kind: "fill" | "stroke" | "text". Returns the new style's id.
    pub fn create_style(&mut self, kind: &str, name: &str) -> Option<String> {
        self.ed.create_style(kind, name)
    }
    pub fn apply_style(&mut self, kind: &str, style_id: &str) {
        self.ed.set_style(kind, Some(style_id));
    }
    pub fn detach_style(&mut self, kind: &str) {
        self.ed.set_style(kind, None);
    }
    /// props: JSON object of canonical props (name, fills, fontSize…).
    pub fn update_style(&mut self, style_id: &str, props_json: &str) {
        if let Ok(Value::Object(props)) = serde_json::from_str::<Value>(props_json) {
            self.ed.update_style(style_id, props);
        }
    }
    pub fn delete_style(&mut self, style_id: &str) {
        self.ed.delete_style(style_id);
    }
    pub fn detach_instances(&mut self) {
        self.ed.detach_instances();
    }
    pub fn insert_instance(&mut self, component_id: &str) -> Option<String> {
        self.ed.insert_instance(component_id)
    }
    pub fn go_to_main_component(&mut self, instance_id: &str) {
        self.ed.go_to_main_component(instance_id);
    }
    pub fn reset_overrides(&mut self, ids_json: &str) {
        let ids = parse_ids(ids_json);
        self.ed.reset_overrides(&ids);
    }
    /// Main components in the document (JSON array).
    pub fn components(&self) -> String {
        self.ed.components_json().to_string()
    }

    /// `[{ family, styles: [[weight, italic], …], count }]` for text in the file.
    pub fn used_fonts(&self) -> String {
        self.ed.used_fonts_json().to_string()
    }

    /// Replace a font family everywhere in the file (one undo step).
    pub fn replace_font(&mut self, from: &str, to: &str) -> u32 {
        self.ed.replace_font(from, to) as u32
    }

    pub fn add_auto_layout(&mut self) {
        self.ed.add_auto_layout();
    }
    pub fn reorder(&mut self, how: &str) {
        self.ed.reorder(how);
    }
    pub fn align(&mut self, how: &str) {
        self.ed.align(how);
    }
    pub fn distribute(&mut self, horizontal: bool) {
        self.ed.distribute(horizontal);
    }
    pub fn flip(&mut self, horizontal: bool) {
        self.ed.flip(horizontal);
    }
    pub fn toggle_visible(&mut self, ids_json: &str) {
        let ids = parse_ids(ids_json);
        self.ed.toggle_prop(&ids, "visible");
    }
    pub fn toggle_locked(&mut self, ids_json: &str) {
        let ids = parse_ids(ids_json);
        self.ed.toggle_prop(&ids, "locked");
    }
    pub fn rename(&mut self, id: &str, name: &str) {
        self.ed.apply_local(vec![Op::update1(id, "name", Value::String(name.to_string()))]);
    }
    pub fn move_layer(&mut self, id: &str, parent: &str, index: u32) {
        self.ed.move_layer(id, parent, index as usize);
    }

    pub fn copy(&mut self) -> Option<String> {
        self.ed.copy_json()
    }
    pub fn cut(&mut self) -> Option<String> {
        let s = self.ed.copy_json();
        if s.is_some() {
            self.ed.delete_selection();
        }
        s
    }
    pub fn paste(&mut self, json: &str) -> bool {
        self.ed.paste_json(json)
    }

    /// `[width, height]` of an image asset once it has loaded (for 9-slice insets).
    pub fn image_size(&self, asset: &str) -> Option<Vec<f64>> {
        self.ed.renderer.as_ref().and_then(|r| r.image_size(asset)).map(|(w, h)| vec![w, h])
    }

    /// Insert an uploaded image. Pass has_pos=false to center it in view.
    pub fn insert_image(&mut self, asset: &str, w: f64, h: f64, x: f64, y: f64, has_pos: bool) -> Option<String> {
        let at = if has_pos { Some(self.ed.cam.to_world(Vec2::new(x, y))) } else { None };
        self.ed.insert_image(asset, w, h, at)
    }

    /// Create a frame of a preset size (e.g. "iPhone 16") centered in view.
    pub fn create_frame_preset(&mut self, name: &str, w: f64, h: f64) -> Option<String> {
        if !self.ed.can_edit {
            return None;
        }
        self.ed.ensure_scene();
        let page = self.ed.scene.page.clone();
        // Next to existing work (never on top of it), else centered in view.
        let content = self.ed.scene.content_bounds();
        let (x, y) = if content.is_empty() {
            let c = self.ed.cam.visible_world(self.ed.css_w, self.ed.css_h).center();
            ((c.x - w / 2.0).round(), (c.y - h / 2.0).round())
        } else {
            ((content.right() + 100.0).round(), content.y.round())
        };
        let mut props = Map::new();
        props.insert("x".into(), Value::from(x));
        props.insert("y".into(), Value::from(y));
        props.insert("width".into(), Value::from(w));
        props.insert("height".into(), Value::from(h));
        self.ed.begin("frame");
        let mut node = self.ed.make_node(flexma_doc::NodeType::Frame, &page, &props);
        node.name = name.to_string();
        let id = node.id.clone();
        self.ed.apply_local(vec![Op::Create { node }]);
        self.ed.end();
        self.ed.set_selection(vec![id.clone()]);
        self.ed.set_tool(Tool::Select);
        self.ed.zoom_to_fit(true);
        Some(id)
    }

    // ---------- pages ----------

    pub fn set_page(&mut self, id: &str) {
        self.ed.set_page(id);
    }
    pub fn add_page(&mut self, name: &str) -> String {
        self.ed.add_page(name)
    }
    pub fn rename_page(&mut self, id: &str, name: &str) {
        self.ed.apply_local(vec![Op::update1(id, "name", Value::String(name.to_string()))]);
    }
    pub fn delete_page(&mut self, id: &str) {
        self.ed.delete_page(id);
    }
    pub fn duplicate_page(&mut self, id: &str) -> Option<String> {
        self.ed.duplicate_page(id)
    }

    // ---------- camera ----------

    pub fn zoom(&self) -> f64 {
        self.ed.cam.zoom
    }
    pub fn camera(&self) -> Vec<f64> {
        vec![self.ed.cam.x, self.ed.cam.y, self.ed.cam.zoom]
    }
    pub fn set_zoom(&mut self, z: f64) {
        let c = Vec2::new(self.ed.css_w / 2.0, self.ed.css_h / 2.0);
        self.ed.cam.set_zoom_at(z, c);
        self.ed.camera_changed();
    }
    pub fn zoom_in(&mut self) {
        let z = self.ed.cam.step(true);
        self.set_zoom(z);
    }
    pub fn zoom_out(&mut self) {
        let z = self.ed.cam.step(false);
        self.set_zoom(z);
    }
    pub fn zoom_to_fit(&mut self) {
        self.ed.zoom_to_fit(false);
    }
    pub fn zoom_to_selection(&mut self) {
        self.ed.zoom_to_fit(true);
    }
    pub fn world_to_screen(&self, x: f64, y: f64) -> Vec<f64> {
        let p = self.ed.cam.to_screen(Vec2::new(x, y));
        vec![p.x, p.y]
    }
    pub fn screen_to_world(&self, x: f64, y: f64) -> Vec<f64> {
        let p = self.ed.cam.to_world(Vec2::new(x, y));
        vec![p.x, p.y]
    }
    /// World-space bounds `[x, y, w, h]` of a node on the current page (empty if not visible).
    pub fn node_bounds(&mut self, id: &str) -> Vec<f64> {
        self.ed.ensure_scene();
        match self.ed.scene.aabb.get(id) {
            Some(b) => vec![b.x, b.y, b.width, b.height],
            None => vec![],
        }
    }

    /// Pan so a world point is centered.
    pub fn center_on(&mut self, x: f64, y: f64) {
        self.ed.cam.x = self.ed.css_w / 2.0 - x * self.ed.cam.zoom;
        self.ed.cam.y = self.ed.css_h / 2.0 - y * self.ed.cam.zoom;
        self.ed.camera_changed();
    }

    /// Pan so a node is centered.
    pub fn focus_node(&mut self, id: &str) {
        self.ed.ensure_scene();
        if let Some(b) = self.ed.scene.aabb.get(id).copied() {
            let c = b.center();
            self.ed.cam.x = self.ed.css_w / 2.0 - c.x * self.ed.cam.zoom;
            self.ed.cam.y = self.ed.css_h / 2.0 - c.y * self.ed.cam.zoom;
            self.ed.camera_changed();
        }
    }

    // ---------- text editing ----------

    pub fn editing_text(&mut self) -> String {
        self.ed.editing_text_json().to_string()
    }
    pub fn set_editing_text(&mut self, text: &str) {
        self.ed.set_editing_text(text);
    }
    pub fn end_text_edit(&mut self) {
        self.ed.end_text_edit();
    }
    pub fn edit_selected_text(&mut self) {
        if let Some(id) = self.ed.selection.first().cloned() {
            if self.ed.doc.get(&id).map(|n| n.kind == flexma_doc::NodeType::Text).unwrap_or(false) {
                self.ed.start_text_edit(&id);
            }
        }
    }

    /// Web fonts finished loading: re-measure and re-rasterize text.
    /// Web fonts are loading: hold off measuring text until `fonts_ready`, so
    /// boxes aren't sized for a fallback font.
    pub fn fonts_loading(&mut self) {
        self.ed.fonts_ready = false;
    }

    /// Re-measure auto-sized text in these families (JSON array), e.g. after
    /// their font files loaded. Only real size changes become edits.
    pub fn remeasure_fonts(&mut self, families_json: &str) {
        let families: Vec<String> = serde_json::from_str(families_json).unwrap_or_default();
        let ids: Vec<NodeId> = self
            .ed
            .doc
            .nodes()
            .filter(|n| {
                n.kind == flexma_doc::NodeType::Text
                    && n.text_auto_resize != flexma_doc::TextAutoResize::None
                    && !flexma_doc::node::is_derived_id(&n.id)
                    && families.iter().any(|f| f.eq_ignore_ascii_case(&n.font_family))
            })
            .map(|n| n.id.clone())
            .collect();
        self.ed.needs_measure.extend(ids);
        self.ed.dirty = true;
    }

    pub fn fonts_ready(&mut self) {
        self.ed.fonts_ready = true;
        if let Some(t) = self.ed.text.as_mut() {
            t.reset();
        }
        if let Some(r) = self.ed.renderer.as_mut() {
            r.reset_text();
            r.invalidate();
        }
        self.ed.dirty = true;
    }

    // ---------- export ----------

    pub fn export_svg(&mut self, ids_json: &str) -> Option<String> {
        let mut ids = parse_ids(ids_json);
        if ids.is_empty() {
            ids = vec![self.ed.scene.page.clone()];
        }
        let bg = if ids.len() == 1 && ids[0] == self.ed.scene.page { Some(self.ed.page_background()) } else { None };
        let text = std::cell::RefCell::new(self.ed.text.take());
        let measure = |n: &flexma_doc::Node, s: &str| -> f64 {
            match text.borrow_mut().as_mut() {
                Some(t) => t.measure(n, s),
                None => flexma_doc::text::estimate_width(s, n.font_size, n.letter_spacing, n.font_weight),
            }
        };
        let base = self.ed.renderer.as_ref().map(|r| r.asset_base.clone()).unwrap_or_default();
        let resolve = move |id: &str| Some(format!("{base}{id}"));
        let renderer = self.ed.renderer.as_ref();
        let image_size = |id: &str| renderer.and_then(|r| r.image_size(id));
        let opts = flexma_doc::svg::SvgOptions { padding: 0.0, background: bg, resolve_asset: &resolve, image_size: &image_size, measure_text: Some(&measure), resolve_variant: &|_, _, _| None, linear: false };
        let out = flexma_doc::svg::export(&self.ed.doc, &ids, &opts).map(|(s, _)| s);
        self.ed.text = text.into_inner();
        out
    }
}
