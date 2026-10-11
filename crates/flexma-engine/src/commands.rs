//! Editor commands: delete, duplicate, group/ungroup, frame selection, auto
//! layout, z-order, align/distribute, flip, nudge, clipboard, images and pages.

use crate::editor::Editor;
use flexma_doc::fractional;
use flexma_doc::geom::local_transform;
use flexma_doc::{Affine, LayoutMode, Node, NodeId, NodeType, Op, Paint, Rect, Vec2};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

fn props(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

impl Editor {
    /// Bounding box of a node in its parent's coordinate space.
    fn local_bounds(&self, n: &Node) -> Rect {
        local_transform(n.x, n.y, n.width, n.height, n.rotation, n.flip_x, n.flip_y).map_rect(&Rect::new(0.0, 0.0, n.width, n.height))
    }

    pub fn delete_selection(&mut self) {
        let ids = self.top_selection();
        if ids.is_empty() {
            return;
        }
        let parents: Vec<NodeId> = ids.iter().filter_map(|i| self.doc.parent_of(i).map(str::to_string)).collect();
        self.begin("delete");
        self.apply_local(ids.into_iter().map(|id| Op::Delete { id }).collect());
        self.fit_groups(&parents);
        self.end();
        self.set_selection(vec![]);
    }

    /// Deep-copy nodes as ops with fresh ids. Returns (ops, new root ids).
    fn clone_subtrees(&mut self, roots: &[NodeId], dx: f64, dy: f64, place_after: bool) -> (Vec<Op>, Vec<NodeId>) {
        let mut ops = Vec::new();
        let mut new_roots = Vec::new();
        for root in roots {
            let Some(rn) = self.doc.get(root).cloned() else { continue };
            let Some(parent) = rn.parent.clone() else { continue };
            let mut nodes = Vec::new();
            self.doc.walk(root, &mut |n| nodes.push(n.clone()));
            let mut map: HashMap<NodeId, NodeId> = HashMap::new();
            for n in &nodes {
                let id = self.new_id();
                map.insert(n.id.clone(), id);
            }
            let index = if place_after {
                let sibs = self.doc.children(&parent);
                let pos = sibs.iter().position(|s| s == root).unwrap_or(0);
                let after = sibs.get(pos + 1).and_then(|s| self.doc.get(s)).map(|s| s.index.clone());
                fractional::key_between(Some(&rn.index), after.as_deref())
            } else {
                self.doc.index_at_end(&parent)
            };
            for n in nodes {
                let mut c = n.clone();
                c.id = map[&n.id].clone();
                if n.id == *root {
                    c.index = index.clone();
                    c.x += dx;
                    c.y += dy;
                } else if let Some(p) = &n.parent {
                    c.parent = map.get(p).cloned().or(Some(p.clone()));
                }
                ops.push(Op::Create { node: c });
            }
            new_roots.push(map[root].clone());
        }
        (ops, new_roots)
    }

    pub fn duplicate(&mut self) {
        let ids = self.top_selection();
        if ids.is_empty() || !self.can_edit {
            return;
        }
        let in_al = ids.iter().any(|id| {
            self.doc.parent_of(id).and_then(|p| self.doc.get(p)).map(|p| p.layout_mode != LayoutMode::None).unwrap_or(false)
        });
        let off = if in_al { 0.0 } else { 10.0 };
        self.begin("duplicate");
        let (ops, roots) = self.clone_subtrees(&ids, off, off, true);
        self.apply_local(ops);
        self.end();
        self.set_selection(roots);
    }

    /// Alt-drag: duplicate in place (originals stay, copies get selected and move).
    pub fn duplicate_in_place(&mut self) {
        let ids = self.top_selection();
        let (ops, roots) = self.clone_subtrees(&ids, 0.0, 0.0, true);
        self.begin("duplicate");
        self.apply_local(ops);
        self.end();
        self.set_selection(roots);
    }

    /// Wrap the selection in a new GROUP or FRAME. Returns the new node id.
    pub fn wrap_selection(&mut self, kind: NodeType, auto_layout: bool) -> Option<NodeId> {
        let ids = self.top_selection();
        if ids.is_empty() || !self.can_edit {
            return None;
        }
        let parent = self.doc.parent_of(&ids[0])?.to_string();
        let parent_inv = self.doc.world_transform(&parent).invert().unwrap_or_default();
        // Bounds in parent space (via world AABBs for nodes from other parents).
        self.ensure_scene();
        let mut b = Rect::empty();
        for id in &ids {
            let wb = self.scene.aabb.get(id).copied().unwrap_or_default();
            b = b.union(&parent_inv.map_rect(&wb));
        }
        let front = ids
            .iter()
            .filter(|id| self.doc.parent_of(id) == Some(parent.as_str()))
            .filter_map(|id| self.doc.get(id).map(|n| n.index.clone()))
            .max()
            .unwrap_or_else(|| self.doc.index_at_end(&parent));
        self.begin(if kind == NodeType::Group { "group" } else { "frame" });
        let gid = self.new_id();
        let mut g = Node::new(gid.clone(), kind);
        g.parent = Some(parent.clone());
        g.index = front;
        g.x = b.x.round();
        g.y = b.y.round();
        g.width = b.width.round().max(1.0);
        g.height = b.height.round().max(1.0);
        g.name = if kind == NodeType::Group { "Group".into() } else { "Frame".into() };
        if kind == NodeType::Frame {
            g.fills = vec![];
            g.clips_content = false;
        }
        // Order children by position for auto layout.
        let mut ordered = ids.clone();
        if auto_layout {
            let horizontal = {
                let mut spread_x = 0.0;
                let mut spread_y = 0.0;
                let centers: Vec<Vec2> = ids.iter().filter_map(|id| self.scene.aabb.get(id).map(|r| r.center())).collect();
                for w in centers.windows(2) {
                    spread_x += (w[1].x - w[0].x).abs();
                    spread_y += (w[1].y - w[0].y).abs();
                }
                spread_x > spread_y
            };
            ordered.sort_by(|a, b| {
                let ra = self.scene.aabb.get(a).copied().unwrap_or_default();
                let rb = self.scene.aabb.get(b).copied().unwrap_or_default();
                let (ka, kb) = if horizontal { (ra.x, rb.x) } else { (ra.y, rb.y) };
                ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
            });
            // Average gap between neighbours.
            let rects: Vec<Rect> = ordered.iter().filter_map(|id| self.scene.aabb.get(id).copied()).collect();
            let mut gaps = Vec::new();
            for w in rects.windows(2) {
                gaps.push(if horizontal { w[1].x - w[0].right() } else { w[1].y - w[0].bottom() });
            }
            let gap = if gaps.is_empty() { 10.0 } else { (gaps.iter().sum::<f64>() / gaps.len() as f64).max(0.0).round() };
            g.layout_mode = if horizontal { LayoutMode::Horizontal } else { LayoutMode::Vertical };
            g.item_spacing = gap;
            g.layout_sizing_horizontal = flexma_doc::SizingMode::Hug;
            g.layout_sizing_vertical = flexma_doc::SizingMode::Hug;
        } else {
            // Keep the existing stacking order.
            ordered.sort_by(|a, b| {
                let ia = self.scene.index.get(a).copied().unwrap_or(0);
                let ib = self.scene.index.get(b).copied().unwrap_or(0);
                ia.cmp(&ib)
            });
        }
        let gx = g.x;
        let gy = g.y;
        let mut ops = vec![Op::Create { node: g }];
        let keys = fractional::keys_between(None, None, ordered.len());
        for (id, key) in ordered.iter().zip(keys) {
            let n = self.doc.get(id).unwrap();
            let world = self.doc.world_transform(id);
            let lr = local_transform(0.0, 0.0, n.width, n.height, n.rotation, n.flip_x, n.flip_y);
            let origin = parent_inv.apply(world.mul(&lr.invert().unwrap_or_default()).apply(Vec2::ZERO));
            ops.push(Op::Update {
                id: id.clone(),
                props: props(&[
                    ("parent", json!(gid)),
                    ("index", json!(key)),
                    ("x", json!((origin.x - gx).round())),
                    ("y", json!((origin.y - gy).round())),
                ]),
            });
        }
        let old_parents: Vec<NodeId> = ids.iter().filter_map(|i| self.doc.parent_of(i).map(str::to_string)).collect();
        self.apply_local(ops);
        self.fit_groups(&old_parents);
        self.end();
        self.set_selection(vec![gid.clone()]);
        Some(gid)
    }

    pub fn ungroup(&mut self) {
        let ids = self.top_selection();
        let mut new_sel = Vec::new();
        self.begin("ungroup");
        for id in ids {
            let Some(g) = self.doc.get(&id).cloned() else { continue };
            if !matches!(g.kind, NodeType::Group | NodeType::Frame) {
                continue;
            }
            let Some(parent) = g.parent.clone() else { continue };
            let gt = local_transform(g.x, g.y, g.width, g.height, g.rotation, g.flip_x, g.flip_y);
            let kids = self.doc.children(&id).to_vec();
            let after = {
                let sibs = self.doc.children(&parent);
                let pos = sibs.iter().position(|s| *s == id).unwrap_or(0);
                sibs.get(pos + 1).and_then(|s| self.doc.get(s)).map(|s| s.index.clone())
            };
            let keys = fractional::keys_between(Some(&g.index), after.as_deref(), kids.len());
            let mut ops = Vec::new();
            for (k, key) in kids.iter().zip(keys) {
                let Some(c) = self.doc.get(k) else { continue };
                let p = gt.apply(Vec2::new(c.x, c.y));
                let mut pr = props(&[("parent", json!(parent)), ("index", json!(key)), ("x", json!(p.x.round())), ("y", json!(p.y.round()))]);
                if g.rotation != 0.0 {
                    pr.insert("rotation".into(), json!(c.rotation + g.rotation));
                }
                ops.push(Op::Update { id: k.clone(), props: pr });
                new_sel.push(k.clone());
            }
            ops.push(Op::Delete { id: id.clone() });
            self.apply_local(ops);
        }
        self.end();
        self.set_selection(new_sel);
    }

    /// Shift+A: add auto layout to a frame, or wrap the selection in one.
    pub fn add_auto_layout(&mut self) {
        let ids = self.top_selection();
        if ids.len() == 1 {
            let n = self.doc.get(&ids[0]).cloned();
            if let Some(n) = n {
                if n.kind == NodeType::Frame {
                    if n.layout_mode != LayoutMode::None {
                        self.update_nodes(vec![(n.id.clone(), props(&[("layoutMode", json!("NONE"))]))]);
                        return;
                    }
                    let kids = self.doc.children(&n.id).to_vec();
                    self.ensure_scene();
                    let mut sorted = kids.clone();
                    sorted.sort_by(|a, b| {
                        let ra = self.scene.aabb.get(a).copied().unwrap_or_default();
                        let rb = self.scene.aabb.get(b).copied().unwrap_or_default();
                        ra.y.partial_cmp(&rb.y).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    self.begin("auto layout");
                    let keys = fractional::keys_between(None, None, sorted.len());
                    let mut ops: Vec<Op> = sorted.iter().zip(keys).map(|(id, k)| Op::update1(id.clone(), "index", json!(k))).collect();
                    ops.push(Op::Update {
                        id: n.id.clone(),
                        props: props(&[
                            ("layoutMode", json!("VERTICAL")),
                            ("itemSpacing", json!(10)),
                            ("padding", json!(10)),
                            ("layoutSizingHorizontal", json!(if kids.is_empty() { "FIXED" } else { "HUG" })),
                            ("layoutSizingVertical", json!(if kids.is_empty() { "FIXED" } else { "HUG" })),
                        ]),
                    });
                    self.apply_local(ops);
                    self.end();
                    return;
                }
            }
        }
        self.wrap_selection(NodeType::Frame, true);
    }

    /// Keep GROUP nodes sized to their children.
    pub fn fit_groups(&mut self, ids: &[NodeId]) {
        let mut seen = HashSet::new();
        for id in ids {
            let mut cur = Some(id.clone());
            while let Some(gid) = cur {
                if !seen.insert(gid.clone()) {
                    break;
                }
                let Some(g) = self.doc.get(&gid).cloned() else { break };
                if g.kind != NodeType::Group {
                    break;
                }
                let kids = self.doc.children(&gid).to_vec();
                if kids.is_empty() {
                    self.apply_local(vec![Op::Delete { id: gid.clone() }]);
                    cur = g.parent.clone();
                    continue;
                }
                let mut b = Rect::empty();
                for k in &kids {
                    if let Some(c) = self.doc.get(k) {
                        b = b.union(&self.local_bounds(c));
                    }
                }
                if b.x.abs() > 0.01 || b.y.abs() > 0.01 || (b.width - g.width).abs() > 0.01 || (b.height - g.height).abs() > 0.01 {
                    let gt = local_transform(g.x, g.y, g.width, g.height, g.rotation, g.flip_x, g.flip_y);
                    let new_origin = gt.apply(Vec2::new(b.x, b.y));
                    let mut ops = vec![Op::Update {
                        id: gid.clone(),
                        props: props(&[
                            ("x", json!(new_origin.x)),
                            ("y", json!(new_origin.y)),
                            ("width", json!(b.width)),
                            ("height", json!(b.height)),
                        ]),
                    }];
                    for k in &kids {
                        if let Some(c) = self.doc.get(k) {
                            ops.push(Op::Update { id: k.clone(), props: props(&[("x", json!(c.x - b.x)), ("y", json!(c.y - b.y))]) });
                        }
                    }
                    self.apply_local(ops);
                }
                cur = g.parent.clone();
            }
        }
    }

    pub fn reorder(&mut self, how: &str) {
        let ids = self.top_selection();
        let mut ops = Vec::new();
        for id in &ids {
            let Some(parent) = self.doc.parent_of(id).map(str::to_string) else { continue };
            let sibs: Vec<NodeId> = self.doc.children(&parent).to_vec();
            let pos = sibs.iter().position(|s| s == id).unwrap_or(0);
            let idx = |i: usize| self.doc.get(&sibs[i]).map(|n| n.index.clone());
            let key = match how {
                "front" => fractional::key_between(sibs.last().and_then(|s| self.doc.get(s)).map(|n| n.index.as_str()), None),
                "back" => fractional::key_between(None, sibs.first().and_then(|s| self.doc.get(s)).map(|n| n.index.as_str())),
                "forward" => {
                    if pos + 1 >= sibs.len() {
                        continue;
                    }
                    let a = idx(pos + 1);
                    let b = if pos + 2 < sibs.len() { idx(pos + 2) } else { None };
                    fractional::key_between(a.as_deref(), b.as_deref())
                }
                _ => {
                    if pos == 0 {
                        continue;
                    }
                    let b = idx(pos - 1);
                    let a = if pos >= 2 { idx(pos - 2) } else { None };
                    fractional::key_between(a.as_deref(), b.as_deref())
                }
            };
            ops.push(Op::update1(id.clone(), "index", json!(key)));
        }
        self.apply_local(ops);
    }

    /// Move a layer from the layers panel: `index` counts from the back (0).
    pub fn move_layer(&mut self, id: &str, parent: &str, index: usize) {
        let Some(pn) = self.doc.get(parent) else { return };
        if !pn.kind.is_container() || id == parent || self.doc.is_ancestor(id, parent) {
            return;
        }
        let key = self.doc.index_at(parent, index, &[id.to_string()]);
        if self.doc.parent_of(id) == Some(parent) {
            self.apply_local(vec![Op::update1(id, "index", json!(key))]);
            return;
        }
        let old_parent = self.doc.parent_of(id).map(str::to_string);
        let n = self.doc.get(id).unwrap().clone();
        let world = self.doc.world_transform(id);
        let lr = local_transform(0.0, 0.0, n.width, n.height, n.rotation, n.flip_x, n.flip_y);
        let origin = world.mul(&lr.invert().unwrap_or_default()).apply(Vec2::ZERO);
        let p = self.doc.world_transform(parent).invert().unwrap_or_default().apply(origin);
        self.begin("move layer");
        self.apply_local(vec![Op::Update {
            id: id.to_string(),
            props: props(&[("parent", json!(parent)), ("index", json!(key)), ("x", json!(p.x.round())), ("y", json!(p.y.round()))]),
        }]);
        if let Some(op) = old_parent {
            self.fit_groups(&[op, parent.to_string()]);
        }
        self.end();
    }

    /// Translate nodes by a world-space delta.
    pub fn nudge(&mut self, dx: f64, dy: f64) {
        let ids = self.top_selection();
        let mut ops = Vec::new();
        for id in &ids {
            let Some(n) = self.doc.get(id) else { continue };
            if n.locked {
                continue;
            }
            let mut inv = n.parent.as_deref().map(|p| self.doc.world_transform(p)).unwrap_or_default().invert().unwrap_or_default();
            inv.e = 0.0;
            inv.f = 0.0;
            let d = inv.apply_vector(Vec2::new(dx, dy));
            ops.push(Op::Update { id: id.clone(), props: props(&[("x", json!(n.x + d.x)), ("y", json!(n.y + d.y))]) });
        }
        let parents: Vec<NodeId> = ids.iter().filter_map(|i| self.doc.parent_of(i).map(str::to_string)).collect();
        self.begin("nudge");
        self.apply_local(ops);
        self.fit_groups(&parents);
        self.end();
    }

    pub fn align(&mut self, how: &str) {
        self.ensure_scene();
        let ids = self.top_selection();
        if ids.is_empty() {
            return;
        }
        let target = if ids.len() == 1 {
            match self.doc.parent_of(&ids[0]).and_then(|p| self.scene.aabb.get(p)) {
                Some(b) => *b,
                None => return,
            }
        } else {
            self.selection_bounds()
        };
        self.begin("align");
        for id in &ids {
            let Some(b) = self.scene.aabb.get(id).copied() else { continue };
            let (dx, dy) = match how {
                "left" => (target.x - b.x, 0.0),
                "hcenter" => (target.center().x - b.center().x, 0.0),
                "right" => (target.right() - b.right(), 0.0),
                "top" => (0.0, target.y - b.y),
                "vcenter" => (0.0, target.center().y - b.center().y),
                "bottom" => (0.0, target.bottom() - b.bottom()),
                _ => (0.0, 0.0),
            };
            if dx != 0.0 || dy != 0.0 {
                let keep = self.selection.clone();
                self.selection = vec![id.clone()];
                self.nudge(dx.round(), dy.round());
                self.selection = keep;
            }
        }
        self.end();
        self.selection_version += 1;
    }

    pub fn distribute(&mut self, horizontal: bool) {
        self.ensure_scene();
        let mut ids = self.top_selection();
        if ids.len() < 3 {
            return;
        }
        let rect = |e: &Editor, id: &NodeId| e.scene.aabb.get(id).copied().unwrap_or_default();
        ids.sort_by(|a, b| {
            let (ra, rb) = (rect(self, a), rect(self, b));
            let (ka, kb) = if horizontal { (ra.x, rb.x) } else { (ra.y, rb.y) };
            ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
        });
        let first = rect(self, &ids[0]);
        let last = rect(self, &ids[ids.len() - 1]);
        let total: f64 = ids.iter().map(|i| if horizontal { rect(self, i).width } else { rect(self, i).height }).sum();
        let span = if horizontal { last.right() - first.x } else { last.bottom() - first.y };
        let gap = (span - total) / (ids.len() - 1) as f64;
        let mut cursor = if horizontal { first.x } else { first.y };
        self.begin("distribute");
        let keep = self.selection.clone();
        for id in &ids {
            let r = rect(self, id);
            let d = cursor - if horizontal { r.x } else { r.y };
            if d.abs() > 0.01 {
                self.selection = vec![id.clone()];
                if horizontal {
                    self.nudge(d.round(), 0.0);
                } else {
                    self.nudge(0.0, d.round());
                }
            }
            cursor += (if horizontal { r.width } else { r.height }) + gap;
        }
        self.selection = keep;
        self.end();
    }

    pub fn flip(&mut self, horizontal: bool) {
        let ids = self.top_selection();
        let ops = ids
            .iter()
            .filter_map(|id| {
                let n = self.doc.get(id)?;
                Some(if horizontal { Op::update1(id.clone(), "flipX", json!(!n.flip_x)) } else { Op::update1(id.clone(), "flipY", json!(!n.flip_y)) })
            })
            .collect();
        self.apply_local(ops);
    }

    pub fn toggle_prop(&mut self, ids: &[NodeId], key: &str) {
        let target = ids.iter().filter_map(|id| self.doc.get(id)).any(|n| match key {
            "visible" => !n.visible,
            _ => !n.locked,
        });
        let ops = ids.iter().map(|id| Op::update1(id.clone(), key, json!(target))).collect();
        self.apply_local(ops);
    }

    pub fn select_all(&mut self) {
        let parent = self
            .selection
            .first()
            .and_then(|id| self.doc.parent_of(id))
            .map(str::to_string)
            .unwrap_or_else(|| self.scene.page.clone());
        let ids: Vec<NodeId> = self.doc.children(&parent).iter().filter(|id| self.doc.get(id).map(|n| n.visible && !n.locked).unwrap_or(false)).cloned().collect();
        self.set_selection(ids);
    }

    pub fn select_parent(&mut self) {
        let Some(first) = self.selection.first() else { return };
        if let Some(p) = self.doc.parent_of(first).map(str::to_string) {
            if self.doc.get(&p).map(|n| n.kind.is_scene()).unwrap_or(false) {
                self.set_selection(vec![p]);
                return;
            }
        }
        self.set_selection(vec![]);
    }

    pub fn select_children(&mut self) {
        let mut out = Vec::new();
        for id in &self.selection {
            out.extend(self.doc.children(id).iter().cloned());
        }
        if !out.is_empty() {
            self.set_selection(out);
        }
    }

    // ---------------- clipboard ----------------

    pub fn copy_json(&mut self) -> Option<String> {
        let roots = self.top_selection();
        if roots.is_empty() {
            return None;
        }
        self.ensure_scene();
        let mut nodes = Vec::new();
        let mut origins = Map::new();
        for r in &roots {
            self.doc.walk(r, &mut |n| nodes.push(n.clone()));
            let n = self.doc.get(r).unwrap();
            let world = self.doc.world_transform(r);
            let lr = local_transform(0.0, 0.0, n.width, n.height, n.rotation, n.flip_x, n.flip_y);
            let o = world.mul(&lr.invert().unwrap_or_default()).apply(Vec2::ZERO);
            origins.insert(r.clone(), json!([o.x, o.y]));
        }
        Some(json!({ "flexma": 1, "roots": roots, "nodes": nodes, "origins": origins }).to_string())
    }

    pub fn paste_json(&mut self, json_str: &str) -> bool {
        let Ok(v) = serde_json::from_str::<Value>(json_str) else { return false };
        if v.get("flexma").is_none() {
            return false;
        }
        let nodes: Vec<Node> = serde_json::from_value(v["nodes"].clone()).unwrap_or_default();
        let roots: Vec<NodeId> = serde_json::from_value(v["roots"].clone()).unwrap_or_default();
        if nodes.is_empty() || roots.is_empty() || !self.can_edit {
            return false;
        }
        self.ensure_scene();
        // Target: selected container, else the parent of the selection, else the page.
        let target = match self.selection.as_slice() {
            [one] => {
                let n = self.doc.get(one).unwrap();
                if matches!(n.kind, NodeType::Frame | NodeType::Group) && !roots.contains(one) {
                    one.clone()
                } else {
                    n.parent.clone().unwrap_or_else(|| self.scene.page.clone())
                }
            }
            [first, ..] => self.doc.parent_of(first).map(str::to_string).unwrap_or_else(|| self.scene.page.clone()),
            [] => self.scene.page.clone(),
        };
        let inv = self.doc.world_transform(&target).invert().unwrap_or_default();
        // Keep the original position if it's visible, otherwise center in view.
        let view = self.cam.visible_world(self.css_w, self.css_h);
        let mut bounds = Rect::empty();
        for r in &roots {
            if let Some(o) = v["origins"][r].as_array() {
                let n = nodes.iter().find(|n| &n.id == r);
                let (w, h) = n.map(|n| (n.width, n.height)).unwrap_or((0.0, 0.0));
                bounds = bounds.union(&Rect::new(o[0].as_f64().unwrap_or(0.0), o[1].as_f64().unwrap_or(0.0), w, h));
            }
        }
        let shift = if !bounds.is_empty() && !view.intersects(&bounds) {
            view.center() - bounds.center()
        } else {
            Vec2::ZERO
        };
        let mut map: HashMap<NodeId, NodeId> = HashMap::new();
        for n in &nodes {
            let id = self.new_id();
            map.insert(n.id.clone(), id);
        }
        let keys = {
            let last = self.doc.children(&target).last().and_then(|i| self.doc.get(i)).map(|n| n.index.clone());
            fractional::keys_between(last.as_deref(), None, roots.len())
        };
        let mut ops = Vec::new();
        let mut new_roots = Vec::new();
        let mut ki = 0;
        for n in &nodes {
            let mut c = n.clone();
            c.id = map[&n.id].clone();
            if roots.contains(&n.id) {
                let o = v["origins"][&n.id].as_array().map(|a| Vec2::new(a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0))).unwrap_or_default();
                let p = inv.apply(o + shift);
                c.parent = Some(target.clone());
                c.index = keys[ki.min(keys.len() - 1)].clone();
                ki += 1;
                c.x = p.x.round();
                c.y = p.y.round();
                new_roots.push(c.id.clone());
            } else if let Some(p) = &n.parent {
                c.parent = map.get(p).cloned();
            }
            ops.push(Op::Create { node: c });
        }
        self.begin("paste");
        self.apply_local(ops);
        self.end();
        self.set_selection(new_roots);
        true
    }

    /// Insert an uploaded image as a rectangle with an image fill.
    pub fn insert_image(&mut self, asset: &str, w: f64, h: f64, at: Option<Vec2>) -> Option<NodeId> {
        if !self.can_edit {
            return None;
        }
        self.ensure_scene();
        let view = self.cam.visible_world(self.css_w, self.css_h);
        let max = (view.width.min(view.height) * 0.6).max(50.0);
        let s = (max / w.max(h).max(1.0)).min(1.0);
        let (w, h) = ((w * s).round().max(1.0), (h * s).round().max(1.0));
        let center = at.unwrap_or_else(|| view.center());
        let target = self.frame_drop_target(center).unwrap_or_else(|| self.scene.page.clone());
        let inv = self.doc.world_transform(&target).invert().unwrap_or_default();
        let p = inv.apply(Vec2::new(center.x - w / 2.0, center.y - h / 2.0));
        let mut pr = Map::new();
        pr.insert("x".into(), json!(p.x.round()));
        pr.insert("y".into(), json!(p.y.round()));
        pr.insert("width".into(), json!(w));
        pr.insert("height".into(), json!(h));
        let mut node = self.make_node(NodeType::Rectangle, &target, &pr);
        node.name = "Image".into();
        node.fills = vec![Paint::Image { asset: asset.to_string(), fit: flexma_doc::ImageFit::Fill, opacity: 1.0, visible: true }];
        let id = node.id.clone();
        self.begin("image");
        self.apply_local(vec![Op::Create { node }]);
        self.end();
        self.set_selection(vec![id.clone()]);
        Some(id)
    }

    fn frame_drop_target(&self, world: Vec2) -> Option<NodeId> {
        for it in self.scene.order.iter().rev() {
            let n = self.doc.get(&it.id)?;
            if n.kind == NodeType::Frame && !n.locked && self.scene.aabb.get(&it.id).map(|b| b.contains(world)).unwrap_or(false) {
                return Some(n.id.clone());
            }
        }
        None
    }

    // ---------------- pages ----------------

    pub fn add_page(&mut self, name: &str) -> NodeId {
        let id = self.new_id();
        let mut p = Node::new(id.clone(), NodeType::Page);
        p.name = name.to_string();
        p.parent = Some(flexma_doc::ROOT_ID.into());
        p.index = self.doc.index_at_end(flexma_doc::ROOT_ID);
        self.apply_local(vec![Op::Create { node: p }]);
        self.set_page(&id);
        id
    }

    pub fn duplicate_page(&mut self, page: &str) -> Option<NodeId> {
        let src = self.doc.get(page)?.clone();
        let mut nodes = Vec::new();
        self.doc.walk(page, &mut |n| nodes.push(n.clone()));
        let mut map: HashMap<NodeId, NodeId> = HashMap::new();
        for n in &nodes {
            let id = self.new_id();
            map.insert(n.id.clone(), id);
        }
        let after = {
            let pages = self.doc.pages();
            let pos = pages.iter().position(|p| p == page).unwrap_or(0);
            pages.get(pos + 1).and_then(|p| self.doc.get(p)).map(|p| p.index.clone())
        };
        let mut ops = Vec::new();
        for n in nodes {
            let mut c = n.clone();
            c.id = map[&n.id].clone();
            if n.id == page {
                c.name = format!("{} copy", src.name);
                c.index = fractional::key_between(Some(&src.index), after.as_deref());
            } else if let Some(p) = &n.parent {
                c.parent = map.get(p).cloned();
            }
            ops.push(Op::Create { node: c });
        }
        let nid = map[page].clone();
        self.begin("duplicate page");
        self.apply_local(ops);
        self.end();
        self.set_page(&nid);
        Some(nid)
    }

    pub fn delete_page(&mut self, page: &str) {
        if self.doc.pages().len() <= 1 {
            return;
        }
        let next = self.doc.pages().iter().find(|p| p.as_str() != page).cloned();
        if self.scene.page == page {
            if let Some(n) = next {
                self.set_page(&n);
            }
        }
        self.apply_local(vec![Op::Delete { id: page.to_string() }]);
    }

    pub fn _unused(&self) -> Affine {
        Affine::IDENTITY
    }
}
