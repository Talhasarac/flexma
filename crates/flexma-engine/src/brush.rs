//! The brush and eraser tools: which paint layer a stroke goes on, turning
//! pointer input (with pen pressure and the stabilizer) into stroke points,
//! and creating paint layers.

use crate::editor::Editor;
use crate::tools::{Gesture, Tool};
use flexma_doc::raster::{self, Stroke};
use flexma_doc::{Node, NodeId, NodeType, Op, Vec2};

/// Side of a paint layer created on the bare canvas.
const NEW_LAYER_SIDE: f64 = 2048.0;

impl Editor {
    fn active_brush(&self) -> raster::Brush {
        if self.tool == Tool::Eraser {
            raster::Brush { erase: true, ..self.eraser.clone() }
        } else {
            raster::Brush { erase: false, ..self.brush.clone() }
        }
    }

    /// The selected paint layer, if exactly one is selected and editable.
    pub fn selected_paint_layer(&self) -> Option<NodeId> {
        match self.selection.as_slice() {
            [id] => self
                .doc
                .get(id)
                .filter(|n| n.kind == NodeType::Paint && !n.locked && n.visible && !flexma_doc::node::is_derived_id(&n.id))
                .map(|n| n.id.clone()),
            _ => None,
        }
    }

    /// Add a paint layer: sized to the frame under `world` (for painting over
    /// a screen), else a large square around it. Selects it.
    pub fn create_paint_layer(&mut self, world: Vec2) -> Option<NodeId> {
        if !self.can_edit {
            return None;
        }
        self.ensure_scene();
        let frame = self.frame_at_point(world);
        let (parent, x, y, w, h) = match frame.and_then(|f| self.doc.get(&f).cloned()) {
            Some(f) => (f.id.clone(), 0.0, 0.0, f.width.max(1.0), f.height.max(1.0)),
            None => {
                let page = self.scene.page.clone();
                (page, (world.x - NEW_LAYER_SIDE / 2.0).round(), (world.y - NEW_LAYER_SIDE / 2.0).round(), NEW_LAYER_SIDE, NEW_LAYER_SIDE)
            }
        };
        let id = self.new_id();
        let mut n = Node::new(&id, NodeType::Paint);
        n.name = self.next_paint_name();
        n.parent = Some(parent.clone());
        n.x = x;
        n.y = y;
        n.width = w.min(raster::MAX_LAYER_SIDE).ceil();
        n.height = h.min(raster::MAX_LAYER_SIDE).ceil();
        n.fills = vec![];
        let last = self.doc.children(&parent).last().and_then(|c| self.doc.get(c)).map(|c| c.index.clone());
        n.index = flexma_doc::fractional::key_between(last.as_deref(), None);
        self.begin("Add paint layer");
        self.apply_local(vec![Op::Create { node: n }]);
        self.end();
        self.set_selection(vec![id.clone()]);
        Some(id)
    }

    fn next_paint_name(&self) -> String {
        let n = self.doc.nodes().filter(|n| n.kind == NodeType::Paint && !flexma_doc::node::is_derived_id(&n.id)).count();
        format!("Paint layer {}", n + 1)
    }

    fn frame_at_point(&self, world: Vec2) -> Option<NodeId> {
        self.scene.order.iter().rev().find_map(|it| {
            let n = self.doc.get(&it.id)?;
            let frame = matches!(n.kind, NodeType::Frame | NodeType::Component) && !flexma_doc::node::is_derived_id(&n.id) && !n.locked;
            (frame && it.clip.as_ref().is_none_or(|c| c.contains(world)) && self.scene.hit_shape(n, world, 0.0)).then(|| n.id.clone())
        })
    }

    /// `world` in a layer's pixels.
    fn layer_point(&self, layer: &str, world: Vec2) -> Option<[f32; 2]> {
        let inv = self.doc.world_transform(layer).invert()?;
        let p = inv.apply(world);
        Some([p.x as f32, p.y as f32])
    }

    /// The pen's tilt in a layer's frame (its direction turned by the
    /// layer's rotation).
    fn layer_tilt(&self, layer: &str) -> [f32; 2] {
        let m = self.doc.world_transform(layer);
        [self.tilt[0], self.tilt[1] - (m.b.atan2(m.a)) as f32]
    }

    pub fn paint_down(&mut self, world: Vec2) {
        if !self.can_edit {
            return;
        }
        let layer = match self.selected_paint_layer() {
            Some(l) => l,
            // Erasing never creates a layer.
            None if self.tool == Tool::Eraser => return,
            None => match self.create_paint_layer(world) {
                Some(l) => l,
                None => return,
            },
        };
        self.paint.sync_layers(&self.doc, self.doc_version);
        let Some(p) = self.layer_point(&layer, world) else { return };
        let alpha_lock = self.doc.get(&layer).is_some_and(|n| n.alpha_lock);
        let first = [p[0], p[1], self.pressure];
        self.smooth = Some(first);
        let brush = self.active_brush().sanitized();
        // Tilt is recorded only for brushes that use it.
        let tilt = if brush.uses_tilt() { vec![self.layer_tilt(&layer)] } else { Vec::new() };
        let stroke = Stroke { id: self.paint.next_stroke_id(), layer: layer.clone(), brush, points: vec![first], tilt, alpha_lock };
        self.paint.begin(stroke, self.now_ms);
        self.gesture = Gesture::Paint { layer };
        self.dirty = true;
    }

    pub fn paint_move(&mut self, layer: &str, world: Vec2) {
        let Some(p) = self.layer_point(layer, world) else { return };
        let raw = [p[0], p[1], self.pressure];
        // Stabilizer: follow the pointer with a lag; pressure stays live.
        let s = self.active_brush().smoothing.clamp(0.0, 0.95);
        let prev = self.smooth.unwrap_or(raw);
        let k = 1.0 - s;
        let sm = [prev[0] + (raw[0] - prev[0]) * k, prev[1] + (raw[1] - prev[1]) * k, raw[2]];
        // Skip sub-pixel jitter.
        if (sm[0] - prev[0]).abs() < 0.2 && (sm[1] - prev[1]).abs() < 0.2 {
            return;
        }
        self.smooth = Some(sm);
        let tilt = self.layer_tilt(layer);
        self.paint.add_point(sm, tilt);
        self.dirty = true;
    }

    pub fn paint_up(&mut self, layer: &str, world: Vec2) {
        // Finish at the pointer, so the stabilizer's lag doesn't cut the end off.
        if let Some(p) = self.layer_point(layer, world) {
            if self.smooth.is_some_and(|s| (s[0] - p[0]).abs() > 0.2 || (s[1] - p[1]).abs() > 0.2) {
                let tilt = self.layer_tilt(layer);
                self.paint.add_point([p[0], p[1], self.pressure], tilt);
            }
        }
        self.smooth = None;
        if let Some(id) = self.paint.end() {
            let sel = self.selection.clone();
            self.history.push_stroke(id, &sel);
            self.ui_version += 1;
        }
        self.dirty = true;
    }
}
