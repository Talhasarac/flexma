//! Pointer interaction: tools, gestures, handles, snapping, drawing and the pen.

use crate::editor::{Editor, Guide};
use flexma_doc::constraints::Subtree;
use flexma_doc::geom::local_transform;
use flexma_doc::path::Seg;
use flexma_doc::{Affine, Color, LayoutMode, Node, NodeId, NodeType, Op, Paint, Rect, Vec2};
use serde_json::{json, Map, Value};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Hand,
    Frame,
    Rectangle,
    Ellipse,
    Line,
    Text,
    Pen,
    Polygon,
    Star,
    Brush,
    Eraser,
}

impl Tool {
    pub fn paints(self) -> bool {
        matches!(self, Tool::Brush | Tool::Eraser)
    }

    pub fn parse(s: &str) -> Option<Tool> {
        Some(match s {
            "select" => Tool::Select,
            "hand" => Tool::Hand,
            "frame" => Tool::Frame,
            "rectangle" => Tool::Rectangle,
            "ellipse" => Tool::Ellipse,
            "line" => Tool::Line,
            "text" => Tool::Text,
            "pen" => Tool::Pen,
            "polygon" => Tool::Polygon,
            "star" => Tool::Star,
            "brush" => Tool::Brush,
            "eraser" => Tool::Eraser,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Tool::Select => "select",
            Tool::Hand => "hand",
            Tool::Frame => "frame",
            Tool::Rectangle => "rectangle",
            Tool::Ellipse => "ellipse",
            Tool::Line => "line",
            Tool::Text => "text",
            Tool::Pen => "pen",
            Tool::Polygon => "polygon",
            Tool::Star => "star",
            Tool::Brush => "brush",
            Tool::Eraser => "eraser",
        }
    }

    fn node_type(self) -> Option<NodeType> {
        Some(match self {
            Tool::Frame => NodeType::Frame,
            Tool::Rectangle => NodeType::Rectangle,
            Tool::Ellipse => NodeType::Ellipse,
            Tool::Line => NodeType::Line,
            Tool::Text => NodeType::Text,
            Tool::Polygon => NodeType::Polygon,
            Tool::Star => NodeType::Star,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Handle {
    /// Box handle at fractional position (fx, fy) in {0, .5, 1}.
    Box(f64, f64),
    Rotate,
    LineStart,
    LineEnd,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Clone)]
pub struct MoveItem {
    id: NodeId,
    x0: f64,
    y0: f64,
    parent_inv: Affine,
}

#[derive(Clone)]
pub struct ResizeItem {
    id: NodeId,
    node0: Node,
    /// Frames and groups: the descendants that follow the resize, as they were at gesture start.
    tree: Option<Subtree>,
    world0: Affine,
    parent_inv: Affine,
    aabb0: Rect,
}

#[derive(Clone, Copy)]
pub struct PenPoint {
    pub p: Vec2,
    pub cin: Vec2,
    pub cout: Vec2,
}

#[derive(Clone)]
pub enum Gesture {
    None,
    Pending { down: Vec2, world: Vec2, target: Option<NodeId>, reselect: Option<NodeId> },
    Marquee { start: Vec2, base: Vec<NodeId> },
    Move { start: Vec2, items: Vec<MoveItem>, bbox0: Rect, xs: Vec<(f64, Rect)>, ys: Vec<(f64, Rect)>, reorder: Option<NodeId> },
    Resize { handle: Handle, items: Vec<ResizeItem>, bbox0: Rect },
    Rotate { id: NodeId, center: Vec2, a0: f64, rot0: f64 },
    Draw { kind: NodeType, start: Vec2, parent: NodeId, id: Option<NodeId> },
    Pan { last: Vec2 },
    Pen { id: Option<NodeId>, parent: NodeId, points: Vec<PenPoint>, dragging: bool, closed: bool },
    /// A brush or eraser stroke on a paint layer.
    Paint { layer: NodeId },
}

const DRAG_THRESHOLD: f64 = 3.0;
const SNAP_PX: f64 = 6.0;

fn round_px(v: f64) -> f64 {
    v.round()
}

impl Editor {
    pub fn world_at(&self, screen: Vec2) -> Vec2 {
        self.cam.to_world(screen)
    }

    pub fn set_tool(&mut self, t: Tool) {
        if self.paint.painting() && !t.paints() {
            self.paint.cancel();
            self.gesture = Gesture::None;
        }
        if let Gesture::Pen { .. } = self.gesture {
            self.finish_pen();
        }
        if t.paints() {
            // Get painting's GPU programs and brush images ready now.
            self.paint.warm = true;
        }
        if self.tool != t {
            self.tool = t;
            self.ui_version += 1;
            self.dirty = true;
        }
    }

    fn frame_at(&self, world: Vec2, exclude: &HashSet<NodeId>) -> Option<NodeId> {
        for it in self.scene.order.iter().rev() {
            let Some(n) = self.doc.get(&it.id) else { continue };
            // Drop targets: frames and main components (never instances or their layers).
            if !matches!(n.kind, NodeType::Frame | NodeType::Component) || flexma_doc::node::is_derived_id(&n.id) || n.locked || exclude.contains(&n.id) {
                continue;
            }
            if self.doc.ancestors(&n.id).iter().any(|a| exclude.contains(a)) {
                continue;
            }
            if let Some(c) = &it.clip {
                if !c.contains(world) {
                    continue;
                }
            }
            if self.scene.hit_shape(n, world, 0.0) {
                return Some(n.id.clone());
            }
        }
        None
    }

    /// Which node a click at `world` selects, Figma style.
    pub fn resolve_click(&self, world: Vec2, deep: bool) -> Option<NodeId> {
        let tol = 3.0 / self.cam.zoom;
        let hits = self.scene.hits_at(&self.doc, world, tol);
        let leaf = hits.into_iter().find(|id| {
            let n = self.doc.get(id).unwrap();
            !n.locked && !self.doc.ancestors(id).iter().any(|a| self.doc.get(a).map(|a| a.locked).unwrap_or(false))
        })?;
        if deep {
            return Some(leaf);
        }
        // Chain from the page's child down to the leaf.
        let mut chain: Vec<NodeId> = self.doc.ancestors(&leaf).into_iter().rev().collect();
        chain.push(leaf.clone());
        let page_pos = chain.iter().position(|id| *id == self.scene.page).map(|p| p + 1).unwrap_or(0);
        let chain: Vec<NodeId> = chain[page_pos..].to_vec();
        // Selection scope: if a selected node's parent contains the leaf, pick that level.
        for s in &self.selection {
            if let Some(p) = self.doc.parent_of(s) {
                if let Some(i) = chain.iter().position(|c| c == p) {
                    if i + 1 < chain.len() {
                        return Some(chain[i + 1].clone());
                    }
                }
            }
            if chain.contains(s) {
                return Some(s.clone());
            }
        }
        let first = chain.first()?.clone();
        let fnode = self.doc.get(&first)?;
        let top_frame_with_kids = matches!(fnode.kind, NodeType::Frame | NodeType::Component) && !self.doc.children(&first).is_empty();
        if top_frame_with_kids {
            if chain.len() > 1 {
                return Some(chain[1].clone());
            }
            // Clicking the empty background of an artboard starts a marquee.
            return None;
        }
        Some(first)
    }

    /// Local→world transform and size of the selection box.
    pub fn selection_box(&mut self) -> Option<(Affine, f64, f64, Option<NodeId>)> {
        if self.selection.is_empty() {
            return None;
        }
        self.ensure_scene();
        if self.selection.len() == 1 {
            let id = self.selection[0].clone();
            let n = self.doc.get(&id)?;
            if self.scene.index.contains_key(&id) {
                return Some((self.scene.world_of(&id), n.width, n.height, Some(id)));
            }
            return None;
        }
        let b = self.selection_bounds();
        if b.is_empty() {
            return None;
        }
        Some((Affine::translate(b.x, b.y), b.width, b.height, None))
    }

    pub fn handle_at(&mut self, screen: Vec2) -> Option<Handle> {
        if !self.can_edit {
            return None;
        }
        let (m, w, h, single) = self.selection_box()?;
        let to_screen = |p: Vec2| self.cam.to_screen(m.apply(p));
        if let Some(id) = &single {
            let n = self.doc.get(id)?;
            if n.kind == NodeType::Line {
                if to_screen(Vec2::ZERO).dist(screen) < 8.0 {
                    return Some(Handle::LineStart);
                }
                if to_screen(Vec2::new(w, 0.0)).dist(screen) < 8.0 {
                    return Some(Handle::LineEnd);
                }
                return None;
            }
        }
        let sw = (w * self.cam.zoom * m.a.hypot(m.b)).abs();
        let sh = (h * self.cam.zoom * m.c.hypot(m.d)).abs();
        let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        if sw > 6.0 && sh > 6.0 {
            for (fx, fy) in corners {
                if to_screen(Vec2::new(fx * w, fy * h)).dist(screen) <= 7.0 {
                    return Some(Handle::Box(fx, fy));
                }
            }
        }
        // Edges: anywhere along the edge.
        if sw > 4.0 && sh > 4.0 {
            let inv = m.invert()?;
            let l = inv.apply(self.cam.to_world(screen));
            let tol = 4.0 / (self.cam.zoom * m.a.hypot(m.b)).max(1e-6);
            let in_x = l.x > 0.0 && l.x < w;
            let in_y = l.y > 0.0 && l.y < h;
            if in_x && l.y.abs() < tol {
                return Some(Handle::Box(0.5, 0.0));
            }
            if in_x && (l.y - h).abs() < tol {
                return Some(Handle::Box(0.5, 1.0));
            }
            if in_y && l.x.abs() < tol {
                return Some(Handle::Box(0.0, 0.5));
            }
            if in_y && (l.x - w).abs() < tol {
                return Some(Handle::Box(1.0, 0.5));
            }
        }
        // Rotation zone just outside the corners (single selection).
        if single.is_some() {
            for (fx, fy) in corners {
                let d = to_screen(Vec2::new(fx * w, fy * h)).dist(screen);
                if d > 7.0 && d < 20.0 {
                    let inv = m.invert()?;
                    let l = inv.apply(self.cam.to_world(screen));
                    let outside = l.x < 0.0 || l.y < 0.0 || l.x > w || l.y > h;
                    if outside {
                        return Some(Handle::Rotate);
                    }
                }
            }
        }
        None
    }

    pub fn cursor(&mut self) -> String {
        if self.space_down || self.tool == Tool::Hand {
            return if matches!(self.gesture, Gesture::Pan { .. }) { "grabbing".into() } else { "grab".into() };
        }
        match &self.gesture {
            Gesture::Pan { .. } => return "grabbing".into(),
            Gesture::Move { .. } => return "default".into(),
            Gesture::Rotate { .. } => return "crosshair".into(),
            _ => {}
        }
        match self.tool {
            Tool::Select => {}
            Tool::Text => return "text".into(),
            Tool::Hand => return "grab".into(),
            // The brush draws its own outline.
            Tool::Brush | Tool::Eraser if self.can_edit => return "none".into(),
            _ => return "crosshair".into(),
        }
        let h = match &self.gesture {
            Gesture::Resize { handle, .. } => Some(*handle),
            _ => self.handle_at(self.pointer),
        };
        match h {
            Some(Handle::Box(fx, fy)) => {
                let base = if (fx - 0.5).abs() < 0.1 {
                    90.0
                } else if (fy - 0.5).abs() < 0.1 {
                    0.0
                } else if (fx - fy).abs() < 0.1 {
                    45.0
                } else {
                    135.0
                };
                let rot = self.selection_box().map(|(m, ..)| m.rotation_deg()).unwrap_or(0.0);
                let a = ((base + rot) % 180.0 + 180.0) % 180.0;
                if !(22.5..157.5).contains(&a) {
                    "ew-resize".into()
                } else if a < 67.5 {
                    "nwse-resize".into()
                } else if a < 112.5 {
                    "ns-resize".into()
                } else {
                    "nesw-resize".into()
                }
            }
            Some(Handle::Rotate) => "crosshair".into(),
            Some(_) => "move".into(),
            None => "default".into(),
        }
    }

    // ---------------- pointer events ----------------

    pub fn pointer_down(&mut self, screen: Vec2, button: i32, mods: Mods) {
        self.pointer = screen;
        self.ensure_scene();
        let world = self.world_at(screen);
        if button == 1 || self.space_down || self.tool == Tool::Hand {
            self.gesture = Gesture::Pan { last: screen };
            return;
        }
        if button != 0 {
            return;
        }
        if self.editing_text.is_some() {
            self.end_text_edit();
        }
        if !self.can_edit && self.tool != Tool::Select {
            self.tool = Tool::Select;
        }
        match self.tool {
            Tool::Select => self.select_down(screen, world, mods),
            Tool::Pen => self.pen_down(world),
            Tool::Brush | Tool::Eraser => self.paint_down(world),
            t => {
                if let Some(kind) = t.node_type() {
                    let exclude = HashSet::new();
                    let parent = self.frame_at(world, &exclude).unwrap_or_else(|| self.scene.page.clone());
                    self.gesture = Gesture::Draw { kind, start: world, parent, id: None };
                }
            }
        }
    }

    fn select_down(&mut self, screen: Vec2, world: Vec2, mods: Mods) {
        if let Some(h) = self.handle_at(screen) {
            self.start_handle_drag(h, world);
            return;
        }
        // Frame title labels.
        let label = self.frame_labels.iter().find(|(_, r)| r.contains(screen)).map(|(id, _)| id.clone());
        let target = match label {
            Some(id) => Some(id),
            None => self.resolve_click(world, mods.meta),
        };
        match target {
            Some(t) => {
                let mut reselect = None;
                if mods.shift {
                    let mut sel = self.selection.clone();
                    if let Some(i) = sel.iter().position(|s| *s == t) {
                        sel.remove(i);
                    } else {
                        sel.push(t.clone());
                    }
                    self.set_selection(sel);
                } else if !self.selection.contains(&t) {
                    self.set_selection(vec![t.clone()]);
                } else if self.selection.len() > 1 {
                    reselect = Some(t.clone());
                }
                let target = if self.selection.contains(&t) { Some(t) } else { None };
                self.gesture = Gesture::Pending { down: screen, world, target, reselect };
            }
            None => {
                // Inside a multi-selection's box: drag moves the whole selection.
                let b = self.selection_bounds();
                if self.selection.len() > 1 && !b.is_empty() && b.contains(world) && !mods.shift {
                    let t = self.selection[0].clone();
                    self.gesture = Gesture::Pending { down: screen, world, target: Some(t), reselect: None };
                    return;
                }
                if !mods.shift {
                    self.set_selection(vec![]);
                }
                self.gesture = Gesture::Pending { down: screen, world, target: None, reselect: None };
            }
        }
    }

    fn start_handle_drag(&mut self, h: Handle, _world: Vec2) {
        let items: Vec<ResizeItem> = self
            .top_selection()
            .into_iter()
            .filter_map(|id| {
                let n = self.doc.get(&id)?.clone();
                if n.locked {
                    return None;
                }
                let parent_inv = n.parent.as_deref().map(|p| self.doc.world_transform(p)).unwrap_or_default().invert().unwrap_or_default();
                let tree = (n.kind.is_frame_like() || n.kind == NodeType::Group).then(|| Subtree::capture(&self.doc, &id));
                Some(ResizeItem {
                    world0: self.scene.world_of(&id),
                    aabb0: self.scene.aabb.get(&id).copied().unwrap_or_default(),
                    id,
                    node0: n,
                    tree,
                    parent_inv,
                })
            })
            .collect();
        if items.is_empty() {
            return;
        }
        self.begin("resize");
        if h == Handle::Rotate {
            let it = &items[0];
            let c = it.world0.apply(Vec2::new(it.node0.width / 2.0, it.node0.height / 2.0));
            let p = self.world_at(self.pointer);
            self.gesture = Gesture::Rotate {
                id: it.id.clone(),
                center: c,
                a0: (p.y - c.y).atan2(p.x - c.x),
                rot0: it.node0.rotation,
            };
            return;
        }
        let bbox0 = items.iter().fold(Rect::empty(), |r, i| r.union(&i.aabb0));
        self.gesture = Gesture::Resize { handle: h, items, bbox0 };
    }

    pub fn pointer_move(&mut self, screen: Vec2, mods: Mods) {
        let prev = self.pointer;
        self.pointer = screen;
        let world = self.world_at(screen);
        self.pointer_in = true;
        if self.tool.paints() {
            // The brush outline follows the pointer.
            self.dirty = true;
        }
        match self.gesture.clone() {
            Gesture::Paint { layer } => {
                self.paint_move(&layer, world);
            }
            Gesture::None => {
                if self.tool == Tool::Select && !self.interacting() {
                    self.ensure_scene();
                    let h = self.resolve_click(world, mods.meta);
                    if h != self.hover {
                        self.hover = h;
                        self.dirty = true;
                    }
                }
                if self.tool == Tool::Pen {
                    self.dirty = true;
                }
            }
            Gesture::Pending { down, world: w0, target, .. } => {
                if down.dist(screen) > DRAG_THRESHOLD {
                    match target {
                        Some(_) if self.can_edit => {
                            if mods.alt {
                                self.duplicate_in_place();
                            }
                            self.start_move(w0);
                            self.pointer_move(screen, mods);
                        }
                        _ => {
                            let base = if mods.shift { self.selection.clone() } else { vec![] };
                            self.gesture = Gesture::Marquee { start: w0, base };
                            self.pointer_move(screen, mods);
                        }
                    }
                }
            }
            Gesture::Marquee { start, base } => {
                let r = Rect::from_points(start, world);
                let mut sel = base.clone();
                for id in self.marquee_candidates(start) {
                    if let Some(b) = self.scene.aabb.get(&id) {
                        if b.intersects(&r) && !sel.contains(&id) {
                            sel.push(id);
                        }
                    }
                }
                self.set_selection(sel);
                self.dirty = true;
            }
            Gesture::Move { .. } => self.update_move(world, mods),
            Gesture::Resize { .. } => self.update_resize(world, mods),
            Gesture::Rotate { id, center, a0, rot0 } => {
                let a = (world.y - center.y).atan2(world.x - center.x);
                let mut deg = rot0 + (a - a0).to_degrees();
                if mods.shift {
                    deg = (deg / 15.0).round() * 15.0;
                }
                deg = ((deg + 180.0).rem_euclid(360.0)) - 180.0;
                let deg = (deg * 100.0).round() / 100.0;
                self.apply_local(vec![Op::update1(id, "rotation", json!(deg))]);
            }
            Gesture::Draw { .. } => self.update_draw(world, mods),
            Gesture::Pan { last } => {
                self.cam.x += screen.x - last.x;
                self.cam.y += screen.y - last.y;
                self.gesture = Gesture::Pan { last: screen };
                self.camera_changed();
            }
            Gesture::Pen { .. } => self.pen_move(world),
        }
        let _ = prev;
    }

    pub fn pointer_up(&mut self, screen: Vec2, mods: Mods) {
        self.pointer = screen;
        let world = self.world_at(screen);
        let g = std::mem::replace(&mut self.gesture, Gesture::None);
        match g {
            Gesture::Paint { layer } => self.paint_up(&layer, world),
            Gesture::Pending { target: Some(_), reselect: Some(t), .. } if !mods.shift => {
                self.set_selection(vec![t]);
            }
            Gesture::Move { items, reorder, .. } => {
                self.finish_move(&items, reorder, world);
                self.end();
            }
            Gesture::Resize { items, .. } => {
                let parents: Vec<NodeId> = items.iter().filter_map(|i| self.doc.parent_of(&i.id).map(str::to_string)).collect();
                self.fit_groups(&parents);
                self.end();
            }
            Gesture::Rotate { .. } => self.end(),
            Gesture::Draw { kind, start, parent, id } => self.finish_draw(kind, start, world, parent, id, mods),
            Gesture::Pen { id, parent, points, closed, .. } => {
                self.gesture = Gesture::Pen { id, parent, points, dragging: false, closed };
            }
            _ => {}
        }
        self.guides.clear();
        self.drop_target = None;
        self.insert_indicator = None;
        self.dirty = true;
    }

    /// Re-run the active drag at the last pointer position (modifier changed).
    pub fn modifiers_changed(&mut self, mods: Mods) {
        match self.gesture {
            Gesture::Move { .. } | Gesture::Resize { .. } | Gesture::Draw { .. } | Gesture::Rotate { .. } => {
                let p = self.pointer;
                self.pointer_move(p, mods);
            }
            _ => {}
        }
    }

    pub fn double_click(&mut self, screen: Vec2) {
        let world = self.world_at(screen);
        if self.tool == Tool::Pen {
            self.finish_pen();
            return;
        }
        if self.selection.len() == 1 {
            let id = self.selection[0].clone();
            let Some(n) = self.doc.get(&id) else { return };
            if n.kind == NodeType::Text && self.can_edit {
                self.start_text_edit(&id);
                return;
            }
            if n.kind.is_container() {
                // Step into the container: select the child under the pointer.
                let deep = self.resolve_click(world, true);
                if let Some(leaf) = deep {
                    let mut chain: Vec<NodeId> = self.doc.ancestors(&leaf).into_iter().rev().collect();
                    chain.push(leaf);
                    if let Some(i) = chain.iter().position(|c| *c == id) {
                        if let Some(child) = chain.get(i + 1) {
                            let c = child.clone();
                            if self.doc.get(&c).map(|n| n.kind == NodeType::Text).unwrap_or(false) && chain.len() == i + 2 {
                                self.set_selection(vec![c.clone()]);
                                self.start_text_edit(&c);
                            } else {
                                self.set_selection(vec![c]);
                            }
                        }
                    }
                }
            }
        } else if let Some(t) = self.resolve_click(world, true) {
            if self.doc.get(&t).map(|n| n.kind == NodeType::Text).unwrap_or(false) && self.can_edit {
                self.set_selection(vec![t.clone()]);
                self.start_text_edit(&t);
            }
        }
    }

    pub fn wheel(&mut self, dx: f64, dy: f64, screen: Vec2, zoom: bool) {
        if zoom {
            let factor = (-dy * 0.01).exp().clamp(0.5, 2.0);
            self.cam.zoom_at(factor, screen);
        } else {
            self.cam.x -= dx;
            self.cam.y -= dy;
        }
        self.hover = None;
        self.camera_changed();
        // Keep dragging gestures in sync with the moving canvas.
        if matches!(self.gesture, Gesture::Move { .. } | Gesture::Resize { .. } | Gesture::Marquee { .. }) {
            let p = self.pointer;
            self.pointer_move(p, Mods::default());
        }
    }

    pub fn pinch(&mut self, scale: f64, screen: Vec2) {
        self.cam.zoom_at(scale, screen);
        self.camera_changed();
    }

    fn marquee_candidates(&self, start: Vec2) -> Vec<NodeId> {
        // Inside an artboard, the marquee selects its children; otherwise top-level nodes.
        let container = self
            .scene
            .order
            .iter()
            .rev()
            .filter(|it| it.depth == 0)
            .find(|it| {
                let n = self.doc.get(&it.id).unwrap();
                matches!(n.kind, NodeType::Frame | NodeType::Component) && self.scene.aabb.get(&it.id).map(|b| b.contains(start)).unwrap_or(false)
            })
            .map(|it| it.id.clone());
        let parent = container.unwrap_or_else(|| self.scene.page.clone());
        self.doc
            .children(&parent)
            .iter()
            .filter(|id| self.doc.get(id).map(|n| n.visible && !n.locked).unwrap_or(false))
            .cloned()
            .collect()
    }

    // ---------------- move ----------------

    fn start_move(&mut self, start: Vec2) {
        self.begin("move");
        let items: Vec<MoveItem> = self
            .top_selection()
            .into_iter()
            .filter_map(|id| {
                let n = self.doc.get(&id)?;
                // Layers inside instances stay put (they follow the main component).
                if n.locked || flexma_doc::node::is_derived_id(&id) {
                    return None;
                }
                let pw = n.parent.as_deref().map(|p| self.doc.world_transform(p)).unwrap_or_default();
                let mut inv = pw.invert().unwrap_or_default();
                inv.e = 0.0;
                inv.f = 0.0;
                Some(MoveItem { id: id.clone(), x0: n.x, y0: n.y, parent_inv: inv })
            })
            .collect();
        let bbox0 = items.iter().fold(Rect::empty(), |r, i| r.union(&self.scene.subtree_aabb(&i.id)));
        // Auto-layout children reorder instead of moving freely.
        let reorder = if items.len() == 1 {
            let p = self.doc.parent_of(&items[0].id).map(str::to_string);
            p.filter(|p| {
                let pn = self.doc.get(p).unwrap();
                matches!(pn.kind, NodeType::Frame | NodeType::Component) && pn.layout_mode != LayoutMode::None && !self.doc.get(&items[0].id).unwrap().layout_absolute
            })
        } else {
            None
        };
        let (xs, ys) = self.snap_targets(&items.iter().map(|i| i.id.clone()).collect());
        self.gesture = Gesture::Move { start, items, bbox0, xs, ys, reorder };
    }

    fn snap_targets(&self, moving: &HashSet<NodeId>) -> (Vec<(f64, Rect)>, Vec<(f64, Rect)>) {
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        let view = self.cam.visible_world(self.css_w, self.css_h).inflate(200.0 / self.cam.zoom);
        let mut parents: HashSet<String> = HashSet::new();
        for id in moving {
            if let Some(p) = self.doc.parent_of(id) {
                parents.insert(p.to_string());
            }
        }
        for it in &self.scene.order {
            if moving.contains(&it.id) {
                continue;
            }
            let p = self.doc.parent_of(&it.id).unwrap_or("");
            let relevant = it.depth == 0 || parents.contains(p) || parents.contains(&it.id);
            if !relevant || self.doc.ancestors(&it.id).iter().any(|a| moving.contains(a)) {
                continue;
            }
            let Some(b) = self.scene.aabb.get(&it.id) else { continue };
            if !b.intersects(&view) {
                continue;
            }
            for x in [b.x, b.x + b.width / 2.0, b.right()] {
                xs.push((x, *b));
            }
            for y in [b.y, b.y + b.height / 2.0, b.bottom()] {
                ys.push((y, *b));
            }
            if xs.len() > 6000 {
                break;
            }
        }
        (xs, ys)
    }

    fn snap_rect(&mut self, r: Rect, xs: &[(f64, Rect)], ys: &[(f64, Rect)]) -> (f64, f64) {
        let tol = SNAP_PX / self.cam.zoom;
        let mut best_x: Option<(f64, f64, Rect)> = None;
        for cand in [r.x, r.x + r.width / 2.0, r.right()] {
            for (t, tr) in xs {
                let d = t - cand;
                if d.abs() <= tol && best_x.map(|b| d.abs() < b.0.abs()).unwrap_or(true) {
                    best_x = Some((d, *t, *tr));
                }
            }
        }
        let mut best_y: Option<(f64, f64, Rect)> = None;
        for cand in [r.y, r.y + r.height / 2.0, r.bottom()] {
            for (t, tr) in ys {
                let d = t - cand;
                if d.abs() <= tol && best_y.map(|b| d.abs() < b.0.abs()).unwrap_or(true) {
                    best_y = Some((d, *t, *tr));
                }
            }
        }
        self.guides.clear();
        let dx = best_x.map(|b| b.0).unwrap_or(0.0);
        let dy = best_y.map(|b| b.0).unwrap_or(0.0);
        let snapped = Rect::new(r.x + dx, r.y + dy, r.width, r.height);
        if let Some((_, x, tr)) = best_x {
            self.guides.push(Guide { vertical: true, pos: x, from: snapped.y.min(tr.y), to: snapped.bottom().max(tr.bottom()) });
        }
        if let Some((_, y, tr)) = best_y {
            self.guides.push(Guide { vertical: false, pos: y, from: snapped.x.min(tr.x), to: snapped.right().max(tr.right()) });
        }
        (dx, dy)
    }

    fn update_move(&mut self, world: Vec2, mods: Mods) {
        let Gesture::Move { start, items, bbox0, xs, ys, reorder } = self.gesture.clone() else { return };
        let mut d = world - start;
        if mods.shift {
            if d.x.abs() > d.y.abs() {
                d.y = 0.0;
            } else {
                d.x = 0.0;
            }
        }
        if let Some(parent) = &reorder {
            // Reorder within auto layout: show an insertion indicator; commit on drop.
            let pb = self.scene.aabb.get(parent).copied().unwrap_or_default();
            if pb.contains(world) {
                self.insert_indicator = self.al_insertion(parent, &items[0].id, world).map(|(_, a, b)| (a, b));
                self.drop_target = None;
            } else {
                self.insert_indicator = None;
                let ex: HashSet<NodeId> = items.iter().map(|i| i.id.clone()).collect();
                self.drop_target = Some(self.frame_at(world, &ex).unwrap_or_else(|| self.scene.page.clone()));
            }
            self.dirty = true;
            return;
        }
        let moved = Rect::new(bbox0.x + d.x, bbox0.y + d.y, bbox0.width, bbox0.height);
        let (sx, sy) = self.snap_rect(moved, &xs, &ys);
        d.x += sx;
        d.y += sy;
        let mut ops = Vec::new();
        for it in &items {
            let ld = it.parent_inv.apply_vector(d);
            let mut props = Map::new();
            props.insert("x".into(), json!(round_px(it.x0 + ld.x)));
            props.insert("y".into(), json!(round_px(it.y0 + ld.y)));
            ops.push(Op::Update { id: it.id.clone(), props });
        }
        self.apply_local(ops);
        // Highlight where the selection would land.
        let ex: HashSet<NodeId> = items.iter().map(|i| i.id.clone()).collect();
        let target = self.frame_at(world, &ex);
        let cur_parent = items.first().and_then(|i| self.doc.parent_of(&i.id).map(str::to_string));
        self.drop_target = match target {
            Some(t) if Some(&t) != cur_parent.as_ref() => Some(t),
            _ => None,
        };
        if let Some(t) = self.drop_target.clone() {
            let tn = self.doc.get(&t).unwrap();
            if tn.layout_mode != LayoutMode::None {
                self.insert_indicator = self.al_insertion(&t, "", world).map(|(_, a, b)| (a, b));
            } else {
                self.insert_indicator = None;
            }
        } else {
            self.insert_indicator = None;
        }
    }

    /// Insertion index in an auto-layout frame for a pointer position, plus an
    /// indicator line in world space.
    fn al_insertion(&self, parent: &str, moving: &str, world: Vec2) -> Option<(usize, Vec2, Vec2)> {
        let pn = self.doc.get(parent)?;
        let horizontal = pn.layout_mode == LayoutMode::Horizontal;
        let kids: Vec<&NodeId> = self.doc.children(parent).iter().filter(|c| c.as_str() != moving).collect();
        let pb = self.scene.aabb.get(parent)?;
        let mut idx = kids.len();
        for (i, k) in kids.iter().enumerate() {
            if let Some(b) = self.scene.aabb.get(*k) {
                let mid = if horizontal { b.x + b.width / 2.0 } else { b.y + b.height / 2.0 };
                let v = if horizontal { world.x } else { world.y };
                if v < mid {
                    idx = i;
                    break;
                }
            }
        }
        let pos = if kids.is_empty() {
            if horizontal { pb.x + pn.padding.left } else { pb.y + pn.padding.top }
        } else if idx < kids.len() {
            let b = self.scene.aabb.get(kids[idx]).copied().unwrap_or_default();
            (if horizontal { b.x } else { b.y }) - pn.item_spacing / 2.0
        } else {
            let b = self.scene.aabb.get(kids[kids.len() - 1]).copied().unwrap_or_default();
            (if horizontal { b.right() } else { b.bottom() }) + pn.item_spacing / 2.0
        };
        let (a, b) = if horizontal {
            (Vec2::new(pos, pb.y + pn.padding.top), Vec2::new(pos, pb.bottom() - pn.padding.bottom))
        } else {
            (Vec2::new(pb.x + pn.padding.left, pos), Vec2::new(pb.right() - pn.padding.right, pos))
        };
        Some((idx, a, b))
    }

    fn finish_move(&mut self, items: &[MoveItem], reorder: Option<NodeId>, world: Vec2) {
        let ids: Vec<NodeId> = items.iter().map(|i| i.id.clone()).collect();
        if ids.is_empty() {
            return;
        }
        let ex: HashSet<NodeId> = ids.iter().cloned().collect();
        if let Some(parent) = reorder {
            let pb = self.scene.aabb.get(&parent).copied().unwrap_or_default();
            if pb.contains(world) {
                if let Some((idx, _, _)) = self.al_insertion(&parent, &ids[0], world) {
                    let key = self.doc.index_at(&parent, idx, &ids);
                    self.apply_local(vec![Op::update1(ids[0].clone(), "index", json!(key))]);
                }
                return;
            }
            // Dragged out of the auto-layout frame: drop at the pointer.
            let target = self.frame_at(world, &ex).unwrap_or_else(|| self.scene.page.clone());
            let n = self.doc.get(&ids[0]).unwrap().clone();
            let start_b = self.scene.aabb.get(&ids[0]).copied().unwrap_or_default();
            let grab = match self.gesture_start_world(items) {
                Some(s) => s - Vec2::new(start_b.x, start_b.y),
                None => Vec2::new(n.width / 2.0, n.height / 2.0),
            };
            let world_tl = world - grab;
            self.reparent(&ids, &target, Some(world_tl));
            return;
        }
        let target = self.frame_at(world, &ex).unwrap_or_else(|| self.scene.page.clone());
        let cur = self.doc.parent_of(&ids[0]).map(str::to_string);
        if Some(&target) != cur.as_ref() {
            self.reparent(&ids, &target, None);
        } else {
            let parents: Vec<NodeId> = cur.into_iter().collect();
            self.fit_groups(&parents);
        }
        let _ = world;
    }

    fn gesture_start_world(&self, _items: &[MoveItem]) -> Option<Vec2> {
        None
    }

    /// Move nodes into a new parent keeping their position on the canvas.
    pub fn reparent(&mut self, ids: &[NodeId], target: &str, world_top_left: Option<Vec2>) {
        let Some(tn) = self.doc.get(target) else { return };
        let al = tn.layout_mode != LayoutMode::None;
        let inv = self.doc.world_transform(target).invert().unwrap_or_default();
        let old_parents: Vec<NodeId> = ids.iter().filter_map(|i| self.doc.parent_of(i).map(str::to_string)).collect();
        let keys = if al {
            let idx = self.al_insertion(target, "", self.world_at(self.pointer)).map(|x| x.0).unwrap_or(usize::MAX);
            let sibs = self.doc.children(target).len();
            let pos = idx.min(sibs);
            let before = if pos == 0 { None } else { self.doc.get(&self.doc.children(target)[pos - 1]).map(|n| n.index.clone()) };
            let after = self.doc.children(target).get(pos).and_then(|i| self.doc.get(i)).map(|n| n.index.clone());
            flexma_doc::fractional::keys_between(before.as_deref(), after.as_deref(), ids.len())
        } else {
            let last = self.doc.children(target).last().and_then(|i| self.doc.get(i)).map(|n| n.index.clone());
            flexma_doc::fractional::keys_between(last.as_deref(), None, ids.len())
        };
        let mut ops = Vec::new();
        for (id, key) in ids.iter().zip(keys) {
            let Some(n) = self.doc.get(id) else { continue };
            if id == target || self.doc.is_ancestor(id, target) {
                continue;
            }
            let world = self.doc.world_transform(id);
            let lr = local_transform(0.0, 0.0, n.width, n.height, n.rotation, n.flip_x, n.flip_y);
            let origin = match world_top_left {
                Some(p) => p,
                None => world.mul(&lr.invert().unwrap_or_default()).apply(Vec2::ZERO),
            };
            let p = inv.apply(origin);
            let mut props = Map::new();
            props.insert("parent".into(), json!(target));
            props.insert("index".into(), json!(key));
            props.insert("x".into(), json!(round_px(p.x)));
            props.insert("y".into(), json!(round_px(p.y)));
            ops.push(Op::Update { id: id.clone(), props });
        }
        self.apply_local(ops);
        self.fit_groups(&old_parents);
    }

    // ---------------- resize ----------------

    fn update_resize(&mut self, world: Vec2, mods: Mods) {
        let Gesture::Resize { handle, items, bbox0 } = self.gesture.clone() else { return };
        let mut ops = Vec::new();
        if items.len() == 1 && matches!(handle, Handle::LineStart | Handle::LineEnd) {
            let it = &items[0];
            let n = &it.node0;
            let a0 = it.world0.apply(Vec2::ZERO);
            let b0 = it.world0.apply(Vec2::new(n.width, 0.0));
            let (fixed, mut moving) = if handle == Handle::LineStart { (b0, world) } else { (a0, world) };
            if mods.shift {
                let d = moving - fixed;
                let ang = (d.y.atan2(d.x) / std::f64::consts::FRAC_PI_4).round() * std::f64::consts::FRAC_PI_4;
                let len = d.len();
                moving = fixed + Vec2::new(ang.cos() * len, ang.sin() * len);
            }
            let (a, b) = if handle == Handle::LineStart { (moving, fixed) } else { (fixed, moving) };
            let la = it.parent_inv.apply(a);
            let lb = it.parent_inv.apply(b);
            let len = la.dist(lb).max(1.0);
            let ang = (lb.y - la.y).atan2(lb.x - la.x).to_degrees();
            let mid = Vec2::new((la.x + lb.x) / 2.0, (la.y + lb.y) / 2.0);
            let mut props = Map::new();
            props.insert("width".into(), json!(len));
            props.insert("rotation".into(), json!((ang * 100.0).round() / 100.0));
            props.insert("x".into(), json!(mid.x - len / 2.0));
            props.insert("y".into(), json!(mid.y));
            ops.push(Op::Update { id: it.id.clone(), props });
            self.apply_local(ops);
            return;
        }
        let Handle::Box(fx, fy) = handle else { return };
        if items.len() == 1 {
            let it = &items[0];
            let n0 = &it.node0;
            let (w0, h0) = (n0.width.max(0.01), n0.height.max(0.01));
            // Pointer in the node's axes, relative to the anchor.
            let anchor_f = if mods.alt { Vec2::new(0.5, 0.5) } else { Vec2::new(1.0 - fx, 1.0 - fy) };
            let anchor_world = it.world0.apply(Vec2::new(anchor_f.x * w0, anchor_f.y * h0));
            let mut lin = it.world0;
            lin.e = 0.0;
            lin.f = 0.0;
            let inv = lin.invert().unwrap_or_default();
            let d = inv.apply(world - anchor_world);
            let mut w = w0;
            let mut h = h0;
            if (fx - 0.5).abs() > 0.1 {
                let sign = if fx > 0.5 { 1.0 } else { -1.0 };
                w = d.x * sign * if mods.alt { 2.0 } else { 1.0 };
            }
            if (fy - 0.5).abs() > 0.1 {
                let sign = if fy > 0.5 { 1.0 } else { -1.0 };
                h = d.y * sign * if mods.alt { 2.0 } else { 1.0 };
            }
            if mods.shift {
                let ratio = w0 / h0;
                if (fx - 0.5).abs() < 0.1 {
                    w = h * ratio;
                } else if (fy - 0.5).abs() < 0.1 {
                    h = w / ratio;
                } else if (w / w0).abs() > (h / h0).abs() {
                    h = w / ratio;
                } else {
                    w = h * ratio;
                }
            }
            let w = round_px(w.max(1.0));
            let h = if n0.kind == NodeType::Line { 0.0 } else { round_px(h.max(1.0)) };
            // Solve for x, y so the anchor stays put.
            let lt = local_transform(0.0, 0.0, w, h, n0.rotation, n0.flip_x, n0.flip_y);
            let q = lt.apply(Vec2::new(anchor_f.x * w, anchor_f.y * h));
            let target = it.parent_inv.apply(anchor_world);
            let mut props = Map::new();
            props.insert("x".into(), json!(round_px(target.x - q.x)));
            props.insert("y".into(), json!(round_px(target.y - q.y)));
            if (fx - 0.5).abs() > 0.1 || mods.shift {
                props.insert("width".into(), json!(w));
            }
            if (fy - 0.5).abs() > 0.1 || mods.shift {
                props.insert("height".into(), json!(h));
            }
            self.resize_extras(n0, &mut props, w, h);
            let fw = if props.contains_key("width") { w } else { n0.width };
            let fh = if props.contains_key("height") { h } else { n0.height };
            ops.extend(self.follow_gesture(it, fw, fh, mods));
            ops.insert(0, Op::Update { id: it.id.clone(), props });
            self.apply_local(ops);
            return;
        }
        // Multi-selection: scale the bounding box.
        let b0 = bbox0;
        let anchor = Vec2::new(b0.x + (1.0 - fx) * b0.width, b0.y + (1.0 - fy) * b0.height);
        let mut nb = b0;
        if (fx - 0.5).abs() > 0.1 {
            let nx = world.x;
            nb.x = nx.min(anchor.x);
            nb.width = (nx - anchor.x).abs().max(1.0);
        }
        if (fy - 0.5).abs() > 0.1 {
            let ny = world.y;
            nb.y = ny.min(anchor.y);
            nb.height = (ny - anchor.y).abs().max(1.0);
        }
        if mods.shift {
            let s = (nb.width / b0.width).max(nb.height / b0.height);
            nb.width = b0.width * s;
            nb.height = b0.height * s;
            if fx < 0.5 {
                nb.x = anchor.x - nb.width;
            }
            if fy < 0.5 {
                nb.y = anchor.y - nb.height;
            }
        }
        let sx = nb.width / b0.width.max(1e-6);
        let sy = nb.height / b0.height.max(1e-6);
        for it in &items {
            let n0 = &it.node0;
            let a = it.aabb0;
            let new_tl = Vec2::new(nb.x + (a.x - b0.x) * sx, nb.y + (a.y - b0.y) * sy);
            let w = round_px((n0.width * sx).max(1.0));
            let h = if n0.kind == NodeType::Line { 0.0 } else { round_px((n0.height * sy).max(1.0)) };
            // Keep the node's AABB top-left at new_tl.
            let lt = local_transform(0.0, 0.0, w, h, n0.rotation, n0.flip_x, n0.flip_y);
            let lb = lt.map_rect(&Rect::new(0.0, 0.0, w, h));
            let p = it.parent_inv.apply(new_tl);
            let mut props = Map::new();
            props.insert("x".into(), json!(round_px(p.x - lb.x)));
            props.insert("y".into(), json!(round_px(p.y - lb.y)));
            props.insert("width".into(), json!(w));
            props.insert("height".into(), json!(h));
            self.resize_extras(n0, &mut props, w, h);
            ops.push(Op::Update { id: it.id.clone(), props });
            ops.extend(self.follow_gesture(it, w, h, mods));
        }
        self.apply_local(ops);
    }

    /// Children of a frame or group being resized to `w`×`h` follow their
    /// constraints (group children scale). Hold Cmd/Ctrl to leave a frame's
    /// children in place, as in Figma.
    fn follow_gesture(&mut self, it: &ResizeItem, w: f64, h: f64, mods: Mods) -> Vec<Op> {
        let Some(t) = &it.tree else { return Vec::new() };
        let mut ops = if mods.meta && it.node0.kind != NodeType::Group { Vec::new() } else { self.follow_resize(t, w, h) };
        let restore = t.restore_ops(&self.doc, &ops);
        ops.extend(restore);
        ops
    }

    /// Type-specific side effects of a resize (text auto-resize, auto-layout
    /// sizing, vector path scaling).
    fn resize_extras(&mut self, n0: &Node, props: &mut Map<String, Value>, w: f64, h: f64) {
        match n0.kind {
            NodeType::Text => {
                if props.contains_key("height") {
                    props.insert("textAutoResize".into(), json!("NONE"));
                } else if props.contains_key("width") && n0.text_auto_resize == flexma_doc::TextAutoResize::WidthAndHeight {
                    props.insert("textAutoResize".into(), json!("HEIGHT"));
                }
                let extra = self.text_box_props(n0, props);
                for (k, v) in extra {
                    if k == "height" && props.get("textAutoResize").and_then(Value::as_str) == Some("NONE") {
                        continue;
                    }
                    props.insert(k, v);
                }
            }
            NodeType::Frame | NodeType::Component | NodeType::Instance => {
                if props.contains_key("width") && n0.layout_sizing_horizontal == flexma_doc::SizingMode::Hug {
                    props.insert("layoutSizingHorizontal".into(), json!("FIXED"));
                }
                if props.contains_key("height") && n0.layout_sizing_vertical == flexma_doc::SizingMode::Hug {
                    props.insert("layoutSizingVertical".into(), json!("FIXED"));
                }
            }
            NodeType::Vector => {
                if n0.width > 0.0 && n0.height > 0.0 {
                    let segs = flexma_doc::path::scale(&flexma_doc::path::parse(&n0.path_data), w / n0.width, h / n0.height.max(1e-6));
                    props.insert("pathData".into(), json!(flexma_doc::path::to_svg_d(&segs)));
                }
            }
            _ => {}
        }
    }

    // ---------------- drawing ----------------

    fn update_draw(&mut self, world: Vec2, mods: Mods) {
        let Gesture::Draw { kind, start, parent, id } = self.gesture.clone() else { return };
        if self.cam.to_screen(start).dist(self.cam.to_screen(world)) < DRAG_THRESHOLD && id.is_none() {
            return;
        }
        let inv = self.doc.world_transform(&parent).invert().unwrap_or_default();
        let (ls, mut lc) = (inv.apply(start), inv.apply(world));
        let mut props = Map::new();
        if kind == NodeType::Line {
            if mods.shift {
                let d = lc - ls;
                let ang = (d.y.atan2(d.x) / std::f64::consts::FRAC_PI_4).round() * std::f64::consts::FRAC_PI_4;
                let len = d.len();
                lc = ls + Vec2::new(ang.cos() * len, ang.sin() * len);
            }
            let len = ls.dist(lc).max(1.0);
            let ang = (lc.y - ls.y).atan2(lc.x - ls.x).to_degrees();
            let mid = Vec2::new((ls.x + lc.x) / 2.0, (ls.y + lc.y) / 2.0);
            props.insert("x".into(), json!(mid.x - len / 2.0));
            props.insert("y".into(), json!(mid.y));
            props.insert("width".into(), json!(round_px(len)));
            props.insert("height".into(), json!(0));
            props.insert("rotation".into(), json!((ang * 100.0).round() / 100.0));
        } else {
            let mut d = lc - ls;
            if mods.shift {
                let m = d.x.abs().max(d.y.abs());
                d = Vec2::new(m * d.x.signum(), m * d.y.signum());
            }
            let (a, b) = if mods.alt { (ls - d, ls + d) } else { (ls, ls + d) };
            let r = Rect::from_points(a, b);
            props.insert("x".into(), json!(round_px(r.x)));
            props.insert("y".into(), json!(round_px(r.y)));
            props.insert("width".into(), json!(round_px(r.width).max(1.0)));
            props.insert("height".into(), json!(round_px(r.height).max(1.0)));
            if kind == NodeType::Text {
                props.insert("textAutoResize".into(), json!("HEIGHT"));
            }
        }
        match id {
            Some(id) => {
                self.apply_local(vec![Op::Update { id, props }]);
            }
            None => {
                self.begin("draw");
                let node = self.make_node(kind, &parent, &props);
                let nid = node.id.clone();
                self.apply_local(vec![Op::Create { node }]);
                self.set_selection(vec![nid.clone()]);
                self.gesture = Gesture::Draw { kind, start, parent, id: Some(nid) };
            }
        }
    }

    /// A new node with default styling for its type.
    pub fn make_node(&mut self, kind: NodeType, parent: &str, props: &Map<String, Value>) -> Node {
        let id = self.new_id();
        let mut n = Node::new(id, kind);
        n.parent = Some(parent.to_string());
        n.index = self.doc.index_at_end(parent);
        let count = self.doc.children(parent).iter().filter(|c| self.doc.get(c).map(|x| x.kind == kind).unwrap_or(false)).count();
        n.name = format!("{} {}", kind.default_name(), count + 1);
        match kind {
            NodeType::Rectangle | NodeType::Ellipse | NodeType::Polygon | NodeType::Star | NodeType::Vector => {
                n.fills = vec![Paint::solid(Color::rgba(0.851, 0.851, 0.851, 1.0))];
            }
            NodeType::Line => {
                n.strokes = vec![Paint::solid(Color::BLACK)];
                n.stroke_weight = 1.0;
            }
            NodeType::Text => {
                n.fills = vec![Paint::solid(Color::BLACK)];
                n.font_size = 16.0;
                n.name = "Text".into();
            }
            NodeType::Frame => {
                n.name = format!("Frame {}", count + 1);
            }
            _ => {}
        }
        for (k, v) in props {
            let _ = n.set_prop(k, v);
        }
        n
    }

    fn finish_draw(&mut self, kind: NodeType, start: Vec2, world: Vec2, parent: NodeId, id: Option<NodeId>, _mods: Mods) {
        let created = match id {
            Some(id) => {
                self.end();
                id
            }
            None => {
                // Click without drag: default-sized node at the click.
                let inv = self.doc.world_transform(&parent).invert().unwrap_or_default();
                let p = inv.apply(start);
                let mut props = Map::new();
                props.insert("x".into(), json!(round_px(p.x)));
                props.insert("y".into(), json!(round_px(p.y)));
                if kind == NodeType::Text {
                    props.insert("characters".into(), json!(""));
                    props.insert("width".into(), json!(1));
                    props.insert("height".into(), json!(20));
                } else if kind == NodeType::Line {
                    props.insert("width".into(), json!(100));
                } else {
                    props.insert("width".into(), json!(100));
                    props.insert("height".into(), json!(100));
                }
                self.begin("draw");
                let node = self.make_node(kind, &parent, &props);
                let nid = node.id.clone();
                self.apply_local(vec![Op::Create { node }]);
                self.end();
                nid
            }
        };
        let _ = world;
        self.set_selection(vec![created.clone()]);
        self.tool = Tool::Select;
        self.ui_version += 1;
        if kind == NodeType::Text {
            self.start_text_edit(&created);
        }
    }

    // ---------------- pen ----------------

    fn pen_down(&mut self, world: Vec2) {
        match self.gesture.clone() {
            Gesture::Pen { id, parent, mut points, closed, .. } => {
                if points.len() >= 2 && self.cam.to_screen(points[0].p).dist(self.cam.to_screen(world)) < 8.0 {
                    self.gesture = Gesture::Pen { id, parent, points, dragging: false, closed: true };
                    self.update_pen_node();
                    self.finish_pen();
                    return;
                }
                points.push(PenPoint { p: world, cin: world, cout: world });
                self.gesture = Gesture::Pen { id, parent, points, dragging: true, closed };
                self.update_pen_node();
            }
            _ => {
                let parent = self.frame_at(world, &HashSet::new()).unwrap_or_else(|| self.scene.page.clone());
                self.begin("pen");
                self.gesture = Gesture::Pen {
                    id: None,
                    parent,
                    points: vec![PenPoint { p: world, cin: world, cout: world }],
                    dragging: true,
                    closed: false,
                };
            }
        }
        self.dirty = true;
    }

    fn pen_move(&mut self, world: Vec2) {
        self.pen_preview = Some(world);
        if let Gesture::Pen { id, parent, mut points, dragging, closed } = self.gesture.clone() {
            if dragging {
                if let Some(last) = points.last_mut() {
                    last.cout = world;
                    last.cin = last.p * 2.0 - world;
                }
                self.gesture = Gesture::Pen { id, parent, points, dragging, closed };
                self.update_pen_node();
            }
        }
        self.dirty = true;
    }

    fn pen_segments(points: &[PenPoint], closed: bool) -> Vec<Seg> {
        let mut segs = Vec::new();
        for (i, p) in points.iter().enumerate() {
            if i == 0 {
                segs.push(Seg::Move(p.p));
                continue;
            }
            let prev = points[i - 1];
            if prev.cout == prev.p && p.cin == p.p {
                segs.push(Seg::Line(p.p));
            } else {
                segs.push(Seg::Cubic(prev.cout, p.cin, p.p));
            }
        }
        if closed && points.len() > 2 {
            let last = points[points.len() - 1];
            let first = points[0];
            if last.cout == last.p && first.cin == first.p {
                segs.push(Seg::Line(first.p));
            } else {
                segs.push(Seg::Cubic(last.cout, first.cin, first.p));
            }
            segs.push(Seg::Close);
        }
        segs
    }

    fn update_pen_node(&mut self) {
        let Gesture::Pen { id, parent, points, dragging, closed } = self.gesture.clone() else { return };
        if points.len() < 2 {
            return;
        }
        let inv = self.doc.world_transform(&parent).invert().unwrap_or_default();
        let local: Vec<PenPoint> = points
            .iter()
            .map(|p| PenPoint { p: inv.apply(p.p), cin: inv.apply(p.cin), cout: inv.apply(p.cout) })
            .collect();
        let segs = Self::pen_segments(&local, closed);
        let b = flexma_doc::path::bounds(&segs);
        let rel = flexma_doc::path::translate(&segs, Vec2::new(-b.x, -b.y));
        let mut props = Map::new();
        props.insert("x".into(), json!(b.x));
        props.insert("y".into(), json!(b.y));
        props.insert("width".into(), json!(b.width.max(1.0)));
        props.insert("height".into(), json!(b.height.max(1.0)));
        props.insert("pathData".into(), json!(flexma_doc::path::to_svg_d(&rel)));
        match id {
            Some(id) => {
                self.apply_local(vec![Op::Update { id, props }]);
            }
            None => {
                let mut node = self.make_node(NodeType::Vector, &parent, &props);
                node.fills = if closed { vec![Paint::solid(Color::rgba(0.851, 0.851, 0.851, 1.0))] } else { vec![] };
                node.strokes = vec![Paint::solid(Color::BLACK)];
                node.stroke_weight = 1.0;
                node.name = "Vector".into();
                let nid = node.id.clone();
                self.apply_local(vec![Op::Create { node }]);
                self.gesture = Gesture::Pen { id: Some(nid), parent, points, dragging, closed };
            }
        }
        if closed {
            if let Gesture::Pen { id: Some(id), .. } = &self.gesture {
                let id = id.clone();
                if self.doc.get(&id).map(|n| n.fills.is_empty()).unwrap_or(false) {
                    self.apply_local(vec![Op::update1(id, "fills", json!([{ "type": "SOLID", "color": "#d9d9d9" }]))]);
                }
            }
        }
    }

    pub fn finish_pen(&mut self) {
        if let Gesture::Pen { id, .. } = std::mem::replace(&mut self.gesture, Gesture::None) {
            self.end();
            if let Some(id) = id {
                self.set_selection(vec![id]);
            }
            self.tool = Tool::Select;
            self.pen_preview = None;
            self.ui_version += 1;
            self.dirty = true;
        }
    }

    pub fn pen_points(&self) -> Option<(Vec<PenPoint>, bool)> {
        match &self.gesture {
            Gesture::Pen { points, closed, .. } => Some((points.clone(), *closed)),
            _ => None,
        }
    }

    // ---------------- text editing ----------------

    pub fn start_text_edit(&mut self, id: &str) {
        if !self.can_edit {
            return;
        }
        self.editing_text = Some(id.to_string());
        self.begin("text");
        self.ui_version += 1;
        self.dirty = true;
        self.events.push(json!({ "type": "text-edit", "id": id }));
    }

    pub fn set_editing_text(&mut self, text: &str) {
        let Some(id) = self.editing_text.clone() else { return };
        let Some(n) = self.doc.get(&id).cloned() else { return };
        let mut props = Map::new();
        props.insert("characters".into(), json!(text));
        flexma_doc::text::keep_runs(&n, &mut props);
        if n.name.is_empty() || n.name == "Text" || n.name == n.characters.chars().take(40).collect::<String>() {
            props.insert("name".into(), json!(text.chars().take(40).collect::<String>()));
        }
        let extra = self.text_box_props(&n, &props);
        props.extend(extra);
        self.apply_local(vec![Op::Update { id, props }]);
        self.ui_version += 1;
    }

    /// Bold, italic or underline the characters `start..end` of the text
    /// being edited (on unless they all have it).
    pub fn toggle_text_style(&mut self, start: usize, end: usize, what: &str) {
        let Some(id) = self.editing_text.clone() else { return };
        let Some(n) = self.doc.get(&id).cloned() else { return };
        if start >= end {
            return;
        }
        let runs = flexma_doc::text::toggle_runs(&n, start, end, what);
        let mut props = Map::new();
        props.insert("textRuns".into(), json!(runs));
        let extra = self.text_box_props(&n, &props);
        props.extend(extra);
        self.apply_local(vec![Op::Update { id, props }]);
        self.ui_version += 1;
    }

    pub fn end_text_edit(&mut self) {
        let Some(id) = self.editing_text.take() else { return };
        let empty = self.doc.get(&id).map(|n| n.characters.trim().is_empty()).unwrap_or(false);
        if empty {
            self.apply_local(vec![Op::Delete { id: id.clone() }]);
            self.selection.retain(|s| *s != id);
            self.selection_version += 1;
        }
        self.end();
        self.ui_version += 1;
        self.dirty = true;
    }

    pub fn editing_text_json(&mut self) -> Value {
        let Some(id) = self.editing_text.clone() else { return Value::Null };
        self.ensure_scene();
        let Some(n) = self.doc.get(&id) else { return Value::Null };
        let m = self.scene.world_of(&id);
        let p = self.cam.to_screen(m.apply(Vec2::ZERO));
        let c = crate::text::text_color(n);
        // Edit at the size it's drawn at.
        let n = match self.text.as_mut() {
            Some(t) => t.fitted(n),
            None => std::borrow::Cow::Borrowed(n),
        };
        json!({
            "id": id,
            "x": p.x, "y": p.y,
            "width": n.width, "height": n.height,
            "rotation": m.rotation_deg(),
            "zoom": self.cam.zoom,
            "characters": n.characters,
            "fontFamily": n.font_family,
            "fontSize": n.font_size,
            "fontWeight": n.font_weight,
            "italic": n.italic,
            "lineHeight": n.effective_line_height(),
            "letterSpacing": n.letter_spacing,
            "textAlign": n.text_align,
            "autoResize": n.text_auto_resize,
            "wrap": n.text_wrap,
            "color": crate::text::css_color(c),
        })
    }
}

#[cfg(test)]
mod constraint_tests {
    use super::*;
    use flexma_doc::{ConstraintType, Constraints, Document};

    fn editor() -> Editor {
        let mut doc = Document::new_empty();
        let mut f = Node::new("f", NodeType::Frame);
        f.parent = Some("0:1".into());
        (f.width, f.height) = (200.0, 100.0);
        doc.apply(&Op::Create { node: f }).unwrap();
        for (id, x, w, h) in [("right", 170.0, 20.0, ConstraintType::Max), ("wide", 10.0, 180.0, ConstraintType::Stretch)] {
            let mut r = Node::new(id, NodeType::Rectangle);
            r.parent = Some("f".into());
            (r.x, r.width, r.height) = (x, w, 20.0);
            r.constraints = Constraints { horizontal: h, vertical: ConstraintType::Min };
            doc.apply(&Op::Create { node: r }).unwrap();
        }
        let mut ed = Editor::new(None, None);
        ed.load(&doc.to_json(), "c1", true).unwrap();
        ed
    }

    fn xw(ed: &Editor, id: &str) -> (f64, f64) {
        let n = ed.doc.get(id).unwrap();
        (n.x, n.width)
    }

    #[test]
    fn resize_handle_moves_children_by_constraints() {
        let mut ed = editor();
        ed.set_selection(vec!["f".into()]);
        ed.ensure_scene();
        ed.start_handle_drag(Handle::Box(1.0, 0.5), Vec2::ZERO);
        let none = Mods::default();
        ed.pointer_move(Vec2::new(300.0, 50.0), none);
        assert_eq!(xw(&ed, "right"), (270.0, 20.0));
        assert_eq!(xw(&ed, "wide"), (10.0, 280.0));
        // Back to the start size: children return too.
        ed.pointer_move(Vec2::new(200.0, 50.0), none);
        assert_eq!(xw(&ed, "right"), (170.0, 20.0));
        assert_eq!(xw(&ed, "wide"), (10.0, 180.0));
        // Cmd/Ctrl resizes the frame alone.
        ed.pointer_move(Vec2::new(260.0, 50.0), Mods { meta: true, ..none });
        assert_eq!(ed.doc.get("f").unwrap().width, 260.0);
        assert_eq!(xw(&ed, "right"), (170.0, 20.0));
        ed.pointer_move(Vec2::new(260.0, 50.0), none);
        assert_eq!(xw(&ed, "right"), (230.0, 20.0));
        ed.pointer_up(Vec2::new(260.0, 50.0), none);
        // One undo step restores the frame and its children.
        ed.undo();
        assert_eq!(ed.doc.get("f").unwrap().width, 200.0);
        assert_eq!(xw(&ed, "right"), (170.0, 20.0));
        // The properties panel follows constraints too.
        ed.update_nodes(vec![("f".into(), Map::from_iter([("width".to_string(), json!(250))]))]);
        assert_eq!(xw(&ed, "right"), (220.0, 20.0));
        assert_eq!(xw(&ed, "wide"), (10.0, 230.0));
    }
}

#[cfg(test)]
mod click_tests {
    use super::*;
    use flexma_doc::{Color, Document, Paint};

    /// A screen with a pill on the left, a screen-wide wrapper without fill in
    /// front of it holding a panel on the right, and an empty placeholder frame.
    fn editor(wrapper_fill: bool) -> Editor {
        let mut doc = Document::new_empty();
        let mut screen = Node::new("screen", NodeType::Frame);
        screen.parent = Some("0:1".into());
        (screen.width, screen.height) = (1000.0, 500.0);
        let mut pill = Node::new("pill", NodeType::Rectangle);
        pill.parent = Some("screen".into());
        (pill.x, pill.y, pill.width, pill.height) = (20.0, 20.0, 100.0, 40.0);
        pill.index = "a".into();
        let mut wrapper = Node::new("wrapper", NodeType::Frame);
        wrapper.parent = Some("screen".into());
        (wrapper.width, wrapper.height) = (1000.0, 100.0);
        wrapper.index = "b".into();
        wrapper.fills = if wrapper_fill { vec![Paint::solid(Color::WHITE)] } else { vec![] };
        let mut panel = Node::new("panel", NodeType::Rectangle);
        panel.parent = Some("wrapper".into());
        (panel.x, panel.y, panel.width, panel.height) = (800.0, 20.0, 180.0, 60.0);
        let mut empty = Node::new("empty", NodeType::Frame);
        empty.parent = Some("screen".into());
        (empty.x, empty.y, empty.width, empty.height) = (20.0, 300.0, 100.0, 100.0);
        empty.index = "c".into();
        empty.fills = vec![];
        for node in [screen, pill, wrapper, panel, empty] {
            doc.apply(&Op::Create { node }).unwrap();
        }
        let mut ed = Editor::new(None, None);
        ed.load(&doc.to_json(), "c1", true).unwrap();
        ed.ensure_scene();
        ed
    }

    fn click(ed: &Editor, x: f64, y: f64) -> Option<String> {
        ed.resolve_click(Vec2::new(x, y), false)
    }

    #[test]
    fn clicks_go_through_empty_frames() {
        let ed = editor(false);
        // The pill under the wrapper's empty area.
        assert_eq!(click(&ed, 50.0, 40.0).as_deref(), Some("pill"));
        // The wrapper's content still selects the wrapper.
        assert_eq!(click(&ed, 850.0, 50.0).as_deref(), Some("wrapper"));
        // Empty area over the screen's background: nothing (marquee).
        assert_eq!(click(&ed, 500.0, 50.0), None);
        // A frame with no children keeps its whole box.
        assert_eq!(click(&ed, 50.0, 350.0).as_deref(), Some("empty"));
        // Deep select reaches the pill too.
        assert_eq!(ed.resolve_click(Vec2::new(50.0, 40.0), true).as_deref(), Some("pill"));
    }

    #[test]
    fn painted_frames_catch_clicks_everywhere() {
        let ed = editor(true);
        assert_eq!(click(&ed, 50.0, 40.0).as_deref(), Some("wrapper"));
        assert_eq!(click(&ed, 500.0, 50.0).as_deref(), Some("wrapper"));
    }
}
