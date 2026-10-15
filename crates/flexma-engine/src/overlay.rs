//! Per-frame editor chrome: hover/selection outlines, resize handles, size
//! badge, marquee, snap guides, drop targets, collaborator selections, frame
//! titles and pen-tool previews. Built in world space (sizes divided by zoom so
//! they stay constant on screen) plus screen-space labels.

use crate::editor::Editor;
use crate::render::{overlay_rect, Label};
use crate::tools::Gesture;
use flexma_doc::geom::fmt_num;
use flexma_doc::{Affine, Color, NodeType, Rect, Vec2};

const ACCENT: [f32; 4] = [0.427, 0.365, 0.988, 1.0];
const GUIDE: [f32; 4] = [0.949, 0.306, 0.118, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// Components and instances get Figma's purple.
const COMPONENT: [f32; 4] = [0.592, 0.278, 1.0, 1.0];
const NONE: [f32; 4] = [0.0; 4];

fn rgba(c: [f32; 4], a: f32) -> [f32; 4] {
    [c[0] * a, c[1] * a, c[2] * a, a]
}

fn parse_color(s: &str) -> [f32; 4] {
    let c = Color::parse(s).unwrap_or(Color::rgba(0.427, 0.365, 0.988, 1.0));
    [c.r, c.g, c.b, 1.0]
}

fn line(buf: &mut Vec<f32>, a: Vec2, b: Vec2, width: f64, color: [f32; 4]) {
    let d = b - a;
    let len = d.len();
    if len < 1e-9 {
        return;
    }
    let ang = d.y.atan2(d.x).to_degrees();
    let m = Affine::translate(a.x, a.y).mul(&Affine::rotate_deg(ang)).mul(&Affine::translate(0.0, -width / 2.0));
    overlay_rect(buf, &m, len, width, color, NONE, 0.0, 0.0, false);
}

impl Editor {
    fn outline_node(&self, buf: &mut Vec<f32>, id: &str, color: [f32; 4], width_px: f64) {
        let Some(n) = self.doc.get(id) else { return };
        let z = self.cam.zoom;
        let m = self.scene.world_of(id);
        let sw = width_px / z;
        match n.kind {
            NodeType::Ellipse => overlay_rect(buf, &m, n.width, n.height, NONE, color, sw, 0.0, true),
            NodeType::Line => {
                let a = m.apply(Vec2::ZERO);
                let b = m.apply(Vec2::new(n.width, 0.0));
                line(buf, a, b, sw, color);
            }
            _ => {
                let r = if n.kind == NodeType::Rectangle || n.kind.is_frame_like() { n.radii()[0].min(n.width / 2.0) } else { 0.0 };
                overlay_rect(buf, &m, n.width, n.height, NONE, color, sw, r, false);
            }
        }
    }

    /// Components, instances and layers inside instances are drawn in purple.
    fn is_component_related(&self, id: &str) -> bool {
        flexma_doc::node::is_derived_id(id)
            || self.doc.get(id).map(|n| matches!(n.kind, NodeType::Component | NodeType::Instance)).unwrap_or(false)
    }

    pub fn build_overlay(&mut self) -> (Vec<f32>, Vec<Label>) {
        let mut buf: Vec<f32> = Vec::new();
        let mut labels: Vec<Label> = Vec::new();
        let z = self.cam.zoom;
        self.frame_labels.clear();
        let dragging = matches!(self.gesture, Gesture::Move { .. } | Gesture::Marquee { .. });

        // Frame titles for top-level frames.
        let page = self.scene.page.clone();
        for c in self.doc.children(&page).to_vec() {
            let Some(n) = self.doc.get(&c) else { continue };
            if !n.kind.is_frame_like() || !n.visible {
                continue;
            }
            let Some(b) = self.scene.aabb.get(&c) else { continue };
            let sb = self.cam.rect_to_screen(b);
            if sb.width < 24.0 || sb.right() < 0.0 || sb.x > self.css_w || sb.bottom() < 0.0 || sb.y - 20.0 > self.css_h {
                continue;
            }
            let selected = self.selection.contains(&c);
            let max_chars = ((sb.width / 6.5) as usize).max(3);
            let mut name: String = n.name.chars().take(max_chars).collect();
            if n.name.chars().count() > max_chars {
                name.pop();
                name.push('…');
            }
            let is_comp = matches!(n.kind, NodeType::Component | NodeType::Instance);
            let prefix = match n.kind {
                NodeType::Component => "❖ ",
                NodeType::Instance => "◇ ",
                _ => "",
            };
            labels.push(Label {
                text: format!("{prefix}{name}"),
                x: sb.x,
                y: sb.y - 18.0,
                size: 11.0,
                weight: if selected { 600 } else { 500 },
                fg: if is_comp { "#9747ff".into() } else if selected { "#6d5dfc".into() } else { "#8a8a8f".into() },
                bg: None,
                center: false,
            });
            let w = (n.name.chars().count().min(max_chars) as f64 * 6.5 + 8.0).min(sb.width);
            self.frame_labels.push((c.clone(), Rect::new(sb.x, sb.y - 20.0, w, 18.0)));
        }

        // Collaborators' selections.
        for p in self.peers.clone() {
            if p.page.as_deref() != Some(page.as_str()) {
                continue;
            }
            let color = parse_color(&p.color);
            let mut bounds = Rect::empty();
            for id in &p.selection {
                if self.scene.index.contains_key(id) {
                    self.outline_node(&mut buf, id, color, if p.is_agent { 2.0 } else { 1.5 });
                    if let Some(b) = self.scene.aabb.get(id) {
                        bounds = bounds.union(b);
                    }
                }
            }
            if p.is_agent && !bounds.is_empty() {
                let sb = self.cam.rect_to_screen(&bounds);
                labels.push(Label {
                    text: format!("✦ {}", p.name),
                    x: sb.x,
                    y: sb.y - 20.0,
                    size: 11.0,
                    weight: 600,
                    fg: "#ffffff".into(),
                    bg: Some(p.color.clone()),
                    center: false,
                });
            }
        }

        // Hover.
        if let Some(h) = self.hover.clone() {
            if !self.selection.contains(&h) && !dragging && self.scene.index.contains_key(&h) {
                let color = if self.is_component_related(&h) { COMPONENT } else { ACCENT };
                self.outline_node(&mut buf, &h, color, 1.5);
            }
        }

        // Drop target / auto-layout insertion.
        if let Some(t) = self.drop_target.clone() {
            if self.scene.index.contains_key(&t) {
                self.outline_node(&mut buf, &t, ACCENT, 2.0);
            }
        }
        if let Some((a, b)) = self.insert_indicator {
            line(&mut buf, a, b, 2.0 / z, ACCENT);
        }

        // Selection.
        let sel = self.selection.clone();
        let editing = self.editing_text.clone();
        for id in &sel {
            if self.scene.index.contains_key(id) {
                let color = if self.is_component_related(id) { COMPONENT } else { ACCENT };
                self.outline_node(&mut buf, id, color, 1.0);
            }
        }
        if let Some(t) = &editing {
            if self.scene.index.contains_key(t) {
                self.outline_node(&mut buf, t, ACCENT, 1.0);
            }
        }
        let show_handles = self.can_edit && editing.is_none() && !matches!(self.gesture, Gesture::Move { .. } | Gesture::Marquee { .. } | Gesture::Pen { .. });
        if let Some((m, w, h, single)) = self.selection_box() {
            if single.is_none() {
                overlay_rect(&mut buf, &m, w, h, NONE, ACCENT, 1.0 / z, 0.0, false);
            }
            let is_line = single.as_ref().and_then(|id| self.doc.get(id)).map(|n| n.kind == NodeType::Line).unwrap_or(false);
            if show_handles {
                let hs = 8.0 / z;
                let pts: Vec<(f64, f64)> = if is_line {
                    vec![(0.0, 0.0), (1.0, 0.0)]
                } else {
                    let sw = w * z * m.a.hypot(m.b);
                    let sh = h * z * m.c.hypot(m.d);
                    if sw < 12.0 && sh < 12.0 {
                        vec![(1.0, 1.0)]
                    } else {
                        vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
                    }
                };
                for (fx, fy) in pts {
                    let c = Vec2::new(fx * w, if is_line { 0.0 } else { fy * h });
                    let hm = m.mul(&Affine::translate(c.x - hs / 2.0, c.y - hs / 2.0));
                    overlay_rect(&mut buf, &hm, hs, hs, WHITE, ACCENT, 1.0 / z, if is_line { hs / 2.0 } else { 1.0 / z }, is_line);
                }
            }
            // Size badge under the selection.
            if editing.is_none() && !matches!(self.gesture, Gesture::Marquee { .. }) {
                let b = self.selection_bounds();
                if !b.is_empty() {
                    let sb = self.cam.rect_to_screen(&b);
                    let (bw, bh) = if let Some(id) = &single {
                        let n = self.doc.get(id).unwrap();
                        (n.width, n.height)
                    } else {
                        (b.width, b.height)
                    };
                    let text = if is_line { fmt_num((bw * 100.0).round() / 100.0) } else { format!("{} × {}", fmt_num((bw * 100.0).round() / 100.0), fmt_num((bh * 100.0).round() / 100.0)) };
                    labels.push(Label {
                        text,
                        x: sb.x + sb.width / 2.0,
                        y: sb.bottom() + 8.0,
                        size: 11.0,
                        weight: 500,
                        fg: "#ffffff".into(),
                        bg: Some("#6d5dfc".into()),
                        center: true,
                    });
                }
            }
        }

        // Marquee.
        if let Gesture::Marquee { start, .. } = &self.gesture {
            let r = Rect::from_points(*start, self.world_at(self.pointer));
            overlay_rect(&mut buf, &Affine::translate(r.x, r.y), r.width, r.height, rgba(ACCENT, 0.08), ACCENT, 1.0 / z, 0.0, false);
        }

        // Snap guides.
        for g in &self.guides {
            if g.vertical {
                line(&mut buf, Vec2::new(g.pos, g.from), Vec2::new(g.pos, g.to), 1.0 / z, GUIDE);
            } else {
                line(&mut buf, Vec2::new(g.from, g.pos), Vec2::new(g.to, g.pos), 1.0 / z, GUIDE);
            }
        }

        // Pen tool.
        if let Some((points, closed)) = self.pen_points() {
            let hs = 7.0 / z;
            if let (Some(last), Some(prev)) = (points.last(), self.pen_preview) {
                if !closed {
                    line(&mut buf, last.p, prev, 1.0 / z, ACCENT);
                }
            }
            for (i, p) in points.iter().enumerate() {
                if p.cout != p.p {
                    line(&mut buf, p.cin, p.cout, 1.0 / z, ACCENT);
                    for c in [p.cin, p.cout] {
                        overlay_rect(&mut buf, &Affine::translate(c.x - hs / 3.0, c.y - hs / 3.0), hs / 1.5, hs / 1.5, WHITE, ACCENT, 1.0 / z, hs, true);
                    }
                }
                let fill = if i == 0 { ACCENT } else { WHITE };
                overlay_rect(&mut buf, &Affine::translate(p.p.x - hs / 2.0, p.p.y - hs / 2.0), hs, hs, fill, ACCENT, 1.0 / z, 0.0, false);
            }
        } else if self.tool == crate::tools::Tool::Pen {
            let p = self.world_at(self.pointer);
            let hs = 7.0 / z;
            overlay_rect(&mut buf, &Affine::translate(p.x - hs / 2.0, p.y - hs / 2.0), hs, hs, WHITE, ACCENT, 1.0 / z, 0.0, false);
        }
        // Brush / eraser outline at the pointer: its size on screen, ringed
        // light and dark so it shows on any color, with a dot for tiny brushes.
        if self.tool.paints() && self.pointer_in && self.can_edit && !self.space_down {
            let b = if self.tool == crate::tools::Tool::Eraser { &self.eraser } else { &self.brush };
            let scale = self.selected_paint_layer().map(|l| self.doc.world_transform(&l).a.abs().max(1e-6)).unwrap_or(1.0);
            let r = (b.size as f64 * scale / 2.0).max(0.5 / z);
            let c = self.world_at(self.pointer);
            let ring = |buf: &mut Vec<f32>, rad: f64, color: [f32; 4], sw: f64| {
                let m = flexma_doc::Affine::translate(c.x - rad, c.y - rad);
                overlay_rect(buf, &m, rad * 2.0, rad * 2.0, [0.0; 4], color, sw, 0.0, true);
            };
            ring(&mut buf, r + 0.5 / z, [1.0, 1.0, 1.0, 0.9], 1.0 / z);
            ring(&mut buf, r - 0.5 / z, [0.0, 0.0, 0.0, 0.75], 1.0 / z);
            if r * z < 4.0 {
                let d = 1.5 / z;
                let m = flexma_doc::Affine::translate(c.x - d, c.y - d);
                overlay_rect(&mut buf, &m, d * 2.0, d * 2.0, [0.0, 0.0, 0.0, 0.85], [1.0, 1.0, 1.0, 0.9], 0.75 / z, 0.0, true);
            }
        }
        (buf, labels)
    }
}
