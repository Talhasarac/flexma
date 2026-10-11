//! Derived, per-page scene data: render order, world transforms, world bounds
//! and clip regions. Rebuilt lazily when the document changes.

use flexma_doc::path;
use flexma_doc::{Affine, Document, Node, NodeId, NodeType, Rect, Vec2};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Item {
    pub id: NodeId,
    pub depth: u16,
    /// Number of descendants in `order` right after this item (subtree span).
    pub span: u32,
    /// Accumulated opacity of ancestors (not including this node).
    pub parent_opacity: f32,
    /// World-space clip rect from clipping ancestors (AABB approximation).
    pub clip: Option<Rect>,
}

#[derive(Default)]
pub struct Scene {
    pub page: NodeId,
    pub order: Vec<Item>,
    pub index: HashMap<NodeId, usize>,
    pub world: HashMap<NodeId, Affine>,
    pub aabb: HashMap<NodeId, Rect>,
    pub dirty: bool,
    /// Bumped on every rebuild (lets the renderer know its display list is stale).
    pub rev: u64,
}

impl Scene {
    pub fn rebuild(&mut self, doc: &Document) {
        self.order.clear();
        self.index.clear();
        self.world.clear();
        self.aabb.clear();
        if doc.get(&self.page).is_none() {
            if let Some(p) = doc.pages().first() {
                self.page = p.clone();
            }
        }
        let page = self.page.clone();
        for c in doc.children(&page).to_vec() {
            self.walk(doc, &c, &Affine::IDENTITY, 0, 1.0, None);
        }
        self.dirty = false;
        self.rev += 1;
    }

    fn walk(&mut self, doc: &Document, id: &str, parent: &Affine, depth: u16, opacity: f32, clip: Option<Rect>) {
        let Some(n) = doc.get(id) else { return };
        if !n.visible {
            return;
        }
        let world = parent.mul(&doc.local_transform(id));
        let bounds = world.map_rect(&Rect::new(0.0, 0.0, n.width, n.height));
        self.world.insert(n.id.clone(), world);
        self.aabb.insert(n.id.clone(), bounds);
        let slot = self.order.len();
        self.index.insert(n.id.clone(), slot);
        self.order.push(Item { id: n.id.clone(), depth, span: 0, parent_opacity: opacity, clip });
        let child_clip = if n.kind == NodeType::Frame && n.clips_content {
            Some(match clip {
                Some(c) => intersect(&c, &bounds),
                None => bounds,
            })
        } else {
            clip
        };
        let op = opacity * n.opacity as f32;
        for c in doc.children(id).to_vec() {
            self.walk(doc, &c, &world, depth + 1, op, child_clip);
        }
        self.order[slot].span = (self.order.len() - slot - 1) as u32;
    }

    pub fn world_of(&self, id: &str) -> Affine {
        self.world.get(id).copied().unwrap_or_default()
    }

    /// World bounds of a node including visible descendants.
    pub fn subtree_aabb(&self, id: &str) -> Rect {
        let Some(&i) = self.index.get(id) else { return Rect::empty() };
        let item = &self.order[i];
        let mut r = self.aabb.get(id).copied().unwrap_or(Rect::empty());
        for it in &self.order[i + 1..=i + item.span as usize] {
            if let Some(b) = self.aabb.get(&it.id) {
                r = r.union(b);
            }
        }
        r
    }

    /// Bounds of all content on the page.
    pub fn content_bounds(&self) -> Rect {
        let mut r = Rect::empty();
        for it in &self.order {
            if it.depth == 0 {
                r = r.union(&self.subtree_aabb(&it.id));
            }
        }
        r
    }

    /// All nodes whose own shape is under `p`, front-most first.
    pub fn hits_at(&self, doc: &Document, p: Vec2, tol: f64) -> Vec<NodeId> {
        let mut out = Vec::new();
        for it in self.order.iter().rev() {
            let Some(b) = self.aabb.get(&it.id) else { continue };
            if !b.inflate(tol).contains(p) {
                continue;
            }
            if let Some(c) = &it.clip {
                if !c.contains(p) {
                    continue;
                }
            }
            let Some(n) = doc.get(&it.id) else { continue };
            if self.hit_shape(n, p, tol) {
                out.push(it.id.clone());
            }
        }
        out
    }

    pub fn hit_shape(&self, n: &Node, p: Vec2, tol: f64) -> bool {
        let Some(inv) = self.world_of(&n.id).invert() else { return false };
        let l = inv.apply(p);
        let (w, h) = (n.width, n.height);
        match n.kind {
            NodeType::Group => false,
            NodeType::Ellipse => {
                let rx = (w / 2.0).max(1e-6) + tol;
                let ry = (h / 2.0).max(1e-6) + tol;
                let dx = (l.x - w / 2.0) / rx;
                let dy = (l.y - h / 2.0) / ry;
                dx * dx + dy * dy <= 1.0
            }
            NodeType::Line => {
                let half = (n.stroke_weight / 2.0).max(0.0) + tol;
                l.x >= -half && l.x <= w + half && l.y.abs() <= half
            }
            NodeType::Vector | NodeType::Polygon | NodeType::Star => {
                if !(l.x >= -tol && l.y >= -tol && l.x <= w + tol && l.y <= h + tol) {
                    return false;
                }
                let segs = path::node_path(n);
                let polys = path::flatten(&segs, 0.5);
                let filled = n.fills.iter().any(|f| f.is_visible());
                if filled && polys.iter().any(|(pts, _)| point_in_poly(pts, l)) {
                    return true;
                }
                let half = (n.stroke_weight / 2.0).max(0.0) + tol.max(2.0);
                polys.iter().any(|(pts, closed)| near_polyline(pts, *closed, l, half))
            }
            _ => l.x >= -tol && l.y >= -tol && l.x <= w + tol && l.y <= h + tol,
        }
    }
}

pub fn intersect(a: &Rect, b: &Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.right().min(b.right());
    let y1 = a.bottom().min(b.bottom());
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

fn point_in_poly(pts: &[Vec2], p: Vec2) -> bool {
    let mut inside = false;
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (pts[i], pts[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub fn dist_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let len2 = ab.x * ab.x + ab.y * ab.y;
    if len2 < 1e-12 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).clamp(0.0, 1.0);
    p.dist(a + ab * t)
}

fn near_polyline(pts: &[Vec2], closed: bool, p: Vec2, d: f64) -> bool {
    for w in pts.windows(2) {
        if dist_to_segment(p, w[0], w[1]) <= d {
            return true;
        }
    }
    if closed && pts.len() > 2 {
        return dist_to_segment(p, pts[pts.len() - 1], pts[0]) <= d;
    }
    false
}
