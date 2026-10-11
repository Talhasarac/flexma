//! WebGL2 renderer.
//!
//! The document is compiled into a camera-independent *display list*: runs of
//! instanced SDF shapes (rects, rounded rects, ellipses, shadows), path meshes
//! (lyon), text quads and image draws, in painter's order, with stencil-based
//! clipping for frames. Pan/zoom only changes a uniform and replays the list, so
//! camera motion never rebuilds geometry. Overlays (selection, handles, guides)
//! are rebuilt each frame into a small separate instance buffer.

use crate::camera::Camera;
use crate::gl::{self, Uniforms, INST_FLOATS};
use crate::scene::Scene;
use crate::text::{text_color, TextEngine};
use flexma_doc::path::{self, Seg};
use flexma_doc::{Affine, Color, Document, Effect, ImageFit, Node, NodeId, NodeType, Paint, StrokeAlign};
use lyon_tessellation as lt;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{HtmlImageElement, WebGl2RenderingContext as GL, WebGlBuffer, WebGlProgram, WebGlTexture, WebGlVertexArrayObject};

const STRIDE: i32 = (INST_FLOATS * 4) as i32;
const MAX_TEXT_RASTERS_PER_FRAME: usize = 24;
/// Device pixels of text we are willing to rasterize + upload in one frame.
const TEXT_PIXEL_BUDGET: usize = 2_000_000;
/// Longest side of a text raster; beyond this, extreme zoom shows slightly soft text.
const MAX_TEXT_TEXTURE: u32 = 2048;
/// GPU memory budget for cached text rasters; least-recently-drawn ones are evicted beyond it.
const TEXT_MEMORY_BUDGET: usize = 160 * 1024 * 1024;
/// Images are downscaled to at most this many pixels on the long side before upload.
const MAX_IMAGE_SIDE: u32 = 2048;
/// GPU memory budget for image fills; least-recently-drawn images are dropped (and reloaded on demand).
const IMAGE_MEMORY_BUDGET: usize = 384 * 1024 * 1024;

#[derive(Clone, Debug)]
enum Cmd {
    Shapes { start: u32, count: u32 },
    Image { inst: u32, asset: String },
    Mesh { id: NodeId, fill: Option<[f32; 4]>, stroke: Option<[f32; 4]> },
    Text { id: NodeId, opacity: f32 },
    /// Stencil clip drawn from one or more instances, each with its image (alpha masks).
    PushClip { parts: Vec<(u32, Option<String>)> },
    PopClip { parts: Vec<(u32, Option<String>)> },
}

struct FillParams {
    ptype: f32,
    angle: f32,
    nstops: f32,
    colors: [[f32; 4]; 4],
    stops: [f32; 4],
    image: Option<String>,
}

struct MeshEntry {
    key: u64,
    fill: Option<(WebGlBuffer, WebGlBuffer, i32)>,
    stroke: Option<(WebGlBuffer, WebGlBuffer, i32)>,
}

struct TextEntry {
    content: u64,
    bucket: f64,
    tex: WebGlTexture,
    rect: [f64; 4],
    /// GPU bytes held by the texture.
    bytes: usize,
    /// Frame number this texture was last drawn in (for LRU eviction).
    used: u64,
}

struct LabelEntry {
    tex: WebGlTexture,
    w: f64,
    h: f64,
    used: u64,
}

pub enum ImgState {
    Loading,
    Ready { tex: WebGlTexture, w: f64, h: f64, bytes: usize, used: u64 },
    Failed,
}

/// A label quad to draw in screen space (CSS px).
pub struct Label {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub weight: u16,
    pub fg: String,
    pub bg: Option<String>,
    /// Anchor: 0 = left/top at (x,y); 1 = centered horizontally.
    pub center: bool,
}

pub struct Renderer {
    pub gl: GL,
    shape_prog: WebGlProgram,
    shape_u: Uniforms,
    mesh_prog: WebGlProgram,
    mesh_u: Uniforms,
    tex_prog: WebGlProgram,
    tex_u: Uniforms,
    shape_vao: WebGlVertexArrayObject,
    mesh_vao: WebGlVertexArrayObject,
    empty_vao: WebGlVertexArrayObject,
    inst_buf: WebGlBuffer,
    inst_cap: usize,
    overlay_buf: WebGlBuffer,
    overlay_cap: usize,
    white: WebGlTexture,
    instances: Vec<f32>,
    cmds: Vec<Cmd>,
    pub built_version: u64,
    meshes: HashMap<NodeId, MeshEntry>,
    texts: HashMap<NodeId, TextEntry>,
    text_bytes: usize,
    labels: HashMap<String, LabelEntry>,
    frame_no: u64,
    images: HashMap<String, ImgState>,
    loaded: Rc<RefCell<Vec<(String, Option<HtmlImageElement>)>>>,
    pub wake: Rc<Cell<bool>>,
    pub asset_base: String,
    pub max_tex: u32,
    /// The last frame skipped work (text rasters / images) and wants another.
    pub wants_more: bool,
    /// Draw calls issued by the last frame (diagnostics).
    pub draw_calls: u32,
}

fn premul(c: Color, alpha: f32) -> [f32; 4] {
    let a = (c.a * alpha).clamp(0.0, 1.0);
    [c.r * a, c.g * a, c.b * a, a]
}

fn view_matrix(cam: &Camera, css_w: f64, css_h: f64) -> [f32; 9] {
    let sx = 2.0 * cam.zoom / css_w;
    let sy = -2.0 * cam.zoom / css_h;
    let tx = 2.0 * cam.x / css_w - 1.0;
    let ty = 1.0 - 2.0 * cam.y / css_h;
    [sx as f32, 0.0, 0.0, 0.0, sy as f32, 0.0, tx as f32, ty as f32, 1.0]
}

fn screen_matrix(css_w: f64, css_h: f64) -> [f32; 9] {
    [(2.0 / css_w) as f32, 0.0, 0.0, 0.0, (-2.0 / css_h) as f32, 0.0, -1.0, 1.0, 1.0]
}

#[allow(clippy::too_many_arguments)]
pub fn push_instance(
    buf: &mut Vec<f32>,
    m: &Affine,
    w: f64,
    h: f64,
    ellipse: bool,
    mode: f32,
    radii: [f64; 4],
    colors: [[f32; 4]; 4],
    stops: [f32; 4],
    paint: [f32; 4],
    stroke: [f32; 4],
    strokep: [f32; 4],
    fx: [f32; 4],
) {
    buf.extend_from_slice(&[m.a as f32, m.c as f32, m.e as f32, 0.0]);
    buf.extend_from_slice(&[m.b as f32, m.d as f32, m.f as f32, 0.0]);
    buf.extend_from_slice(&[w as f32, h as f32, if ellipse { 1.0 } else { 0.0 }, mode]);
    buf.extend_from_slice(&[radii[0] as f32, radii[1] as f32, radii[2] as f32, radii[3] as f32]);
    for c in colors {
        buf.extend_from_slice(&c);
    }
    buf.extend_from_slice(&stops);
    buf.extend_from_slice(&paint);
    buf.extend_from_slice(&stroke);
    buf.extend_from_slice(&strokep);
    buf.extend_from_slice(&fx);
}

/// Simple overlay instance helpers (solid fill and/or stroke, world space).
pub fn overlay_rect(buf: &mut Vec<f32>, m: &Affine, w: f64, h: f64, fill: [f32; 4], stroke: [f32; 4], sw: f64, radius: f64, ellipse: bool) {
    push_instance(
        buf,
        m,
        w,
        h,
        ellipse,
        0.0,
        [radius; 4],
        [fill, [0.0; 4], [0.0; 4], [0.0; 4]],
        [0.0; 4],
        [if fill[3] > 0.0 { 1.0 } else { 0.0 }, 0.0, 0.0, 1.0],
        stroke,
        [sw as f32, 0.0, (sw / 2.0) as f32, 0.0],
        [0.0; 4],
    );
}

/// Do a frame's descendants (with strokes and shadows) lie inside its rounded
/// box, so clipping them would change nothing? Saves two stencil draws per frame.
fn content_fits(doc: &Document, scene: &Scene, i: usize, span: usize, frame: &Node, world: &Affine) -> bool {
    // Only for axis-aligned frames.
    if world.b.abs() > 1e-9 || world.c.abs() > 1e-9 || world.a <= 0.0 || world.d <= 0.0 {
        return false;
    }
    let Some(fb) = scene.aabb.get(&frame.id) else { return false };
    let scale = world.a;
    let [tl, tr, br, bl] = frame.radii().map(|r| r * scale);
    let eps = 0.01;
    let inside_point = |x: f64, y: f64| -> bool {
        let corner = |cx: f64, cy: f64, r: f64, in_region: bool| !in_region || (x - cx).hypot(y - cy) <= r + eps;
        x >= fb.x - eps
            && y >= fb.y - eps
            && x <= fb.right() + eps
            && y <= fb.bottom() + eps
            && corner(fb.x + tl, fb.y + tl, tl, x < fb.x + tl && y < fb.y + tl)
            && corner(fb.right() - tr, fb.y + tr, tr, x > fb.right() - tr && y < fb.y + tr)
            && corner(fb.right() - br, fb.bottom() - br, br, x > fb.right() - br && y > fb.bottom() - br)
            && corner(fb.x + bl, fb.bottom() - bl, bl, x < fb.x + bl && y > fb.bottom() - bl)
    };
    for it in &scene.order[i + 1..=i + span] {
        let Some(n) = doc.get(&it.id) else { continue };
        if n.effects.iter().any(|e| matches!(e, Effect::DropShadow { visible: true, .. } | Effect::LayerBlur { visible: true, .. })) {
            return false;
        }
        let Some(b) = scene.aabb.get(&it.id) else { continue };
        let b = if n.has_visible_stroke() { b.inflate(n.stroke_weight) } else { *b };
        if !(inside_point(b.x, b.y) && inside_point(b.right(), b.y) && inside_point(b.right(), b.bottom()) && inside_point(b.x, b.bottom())) {
            return false;
        }
    }
    true
}

/// Draw an image into a fresh canvas at a smaller size.
fn downscale(img: &HtmlImageElement, w: u32, h: u32) -> Option<web_sys::HtmlCanvasElement> {
    let doc = web_sys::window()?.document()?;
    let canvas: web_sys::HtmlCanvasElement = doc.create_element("canvas").ok()?.dyn_into().ok()?;
    canvas.set_width(w);
    canvas.set_height(h);
    let ctx: web_sys::CanvasRenderingContext2d = canvas.get_context("2d").ok()??.dyn_into().ok()?;
    let _ = js_sys::Reflect::set(&ctx, &"imageSmoothingQuality".into(), &"high".into());
    ctx.draw_image_with_html_image_element_and_dw_and_dh(img, 0.0, 0.0, w as f64, h as f64).ok()?;
    Some(canvas)
}

fn hash_of<T: Hash>(v: &T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

fn f(v: f64) -> u64 {
    v.to_bits()
}

fn text_content_hash(n: &Node) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    n.characters.hash(&mut h);
    n.font_family.hash(&mut h);
    f(n.font_size).hash(&mut h);
    n.font_weight.hash(&mut h);
    n.italic.hash(&mut h);
    f(n.width).hash(&mut h);
    f(n.height).hash(&mut h);
    format!("{:?}{:?}{:?}{:?}{:?}", n.text_align, n.text_align_vertical, n.text_auto_resize, n.text_decoration, n.text_overflow).hash(&mut h);
    n.text_wrap.hash(&mut h);
    n.line_height.map(f).hash(&mut h);
    f(n.letter_spacing).hash(&mut h);
    let c = text_color(n);
    (c.r.to_bits(), c.g.to_bits(), c.b.to_bits(), c.a.to_bits()).hash(&mut h);
    if let Some((c, w, align)) = crate::text::text_stroke(n) {
        (c.r.to_bits(), c.g.to_bits(), c.b.to_bits(), c.a.to_bits(), f(w)).hash(&mut h);
        format!("{align:?}").hash(&mut h);
    }
    h.finish()
}

fn mesh_key(n: &Node, tol_bucket: i32) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    n.kind.hash(&mut h);
    n.path_data.hash(&mut h);
    f(n.width).hash(&mut h);
    f(n.height).hash(&mut h);
    n.point_count.hash(&mut h);
    f(n.inner_radius).hash(&mut h);
    f(n.stroke_weight).hash(&mut h);
    n.has_visible_stroke().hash(&mut h);
    n.fills.iter().any(|p| p.is_visible()).hash(&mut h);
    n.fill_rule.hash(&mut h);
    n.stroke_cap.hash(&mut h);
    n.stroke_join.hash(&mut h);
    tol_bucket.hash(&mut h);
    h.finish()
}

fn lyon_path(segs: &[Seg]) -> lt::path::Path {
    let mut b = lt::path::Path::builder();
    let mut open = false;
    let pt = |v: flexma_doc::Vec2| lt::math::point(v.x as f32, v.y as f32);
    let mut last = flexma_doc::Vec2::ZERO;
    for s in segs {
        match *s {
            Seg::Move(p) => {
                if open {
                    b.end(false);
                }
                b.begin(pt(p));
                open = true;
                last = p;
            }
            Seg::Line(p) => {
                if !open {
                    b.begin(pt(last));
                    open = true;
                }
                b.line_to(pt(p));
                last = p;
            }
            Seg::Cubic(c1, c2, p) => {
                if !open {
                    b.begin(pt(last));
                    open = true;
                }
                b.cubic_bezier_to(pt(c1), pt(c2), pt(p));
                last = p;
            }
            Seg::Close => {
                if open {
                    b.end(true);
                    open = false;
                }
            }
        }
    }
    if open {
        b.end(false);
    }
    b.build()
}

impl Renderer {
    pub fn new(gl: GL) -> Result<Renderer, String> {
        let shape_prog = gl::program(&gl, gl::SHAPE_VS, gl::SHAPE_FS)?;
        let mesh_prog = gl::program(&gl, gl::MESH_VS, gl::MESH_FS)?;
        let tex_prog = gl::program(&gl, gl::TEX_VS, gl::TEX_FS)?;
        let shape_u = Uniforms::new(&gl, &shape_prog, &["u_view", "u_px", "u_tex"]);
        let mesh_u = Uniforms::new(&gl, &mesh_prog, &["u_view", "u_m0", "u_m1", "u_color"]);
        let tex_u = Uniforms::new(&gl, &tex_prog, &["u_view", "u_m0", "u_m1", "u_rect", "u_tex", "u_alpha"]);
        let shape_vao = gl.create_vertex_array().ok_or("vao")?;
        let mesh_vao = gl.create_vertex_array().ok_or("vao")?;
        let empty_vao = gl.create_vertex_array().ok_or("vao")?;
        let inst_buf = gl.create_buffer().ok_or("buffer")?;
        let overlay_buf = gl.create_buffer().ok_or("buffer")?;
        gl.bind_vertex_array(Some(&shape_vao));
        for i in 0..13 {
            gl.enable_vertex_attrib_array(i);
            gl.vertex_attrib_divisor(i, 1);
        }
        gl.bind_vertex_array(Some(&mesh_vao));
        gl.enable_vertex_attrib_array(0);
        gl.bind_vertex_array(None);

        let white = gl.create_texture().ok_or("texture")?;
        gl.bind_texture(GL::TEXTURE_2D, Some(&white));
        let px = [255u8, 255, 255, 255];
        let _ = gl.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_u8_array(
            GL::TEXTURE_2D,
            0,
            GL::RGBA as i32,
            1,
            1,
            0,
            GL::RGBA,
            GL::UNSIGNED_BYTE,
            Some(&px),
        );
        let max_tex = gl
            .get_parameter(GL::MAX_TEXTURE_SIZE)
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(4096.0)
            .min(8192.0) as u32;
        gl.enable(GL::BLEND);
        gl.blend_func(GL::ONE, GL::ONE_MINUS_SRC_ALPHA);
        gl.pixel_storei(GL::UNPACK_PREMULTIPLY_ALPHA_WEBGL, 1);
        Ok(Renderer {
            gl,
            shape_prog,
            shape_u,
            mesh_prog,
            mesh_u,
            tex_prog,
            tex_u,
            shape_vao,
            mesh_vao,
            empty_vao,
            inst_buf,
            inst_cap: 0,
            overlay_buf,
            overlay_cap: 0,
            white,
            instances: Vec::new(),
            cmds: Vec::new(),
            built_version: u64::MAX,
            meshes: HashMap::new(),
            texts: HashMap::new(),
            text_bytes: 0,
            labels: HashMap::new(),
            frame_no: 0,
            images: HashMap::new(),
            loaded: Rc::new(RefCell::new(Vec::new())),
            wake: Rc::new(Cell::new(false)),
            asset_base: "/api/assets/".into(),
            max_tex,
            wants_more: false,
            draw_calls: 0,
        })
    }

    pub fn invalidate(&mut self) {
        self.built_version = u64::MAX;
    }

    /// Forget all text rasters (fonts changed).
    pub fn reset_text(&mut self) {
        for (_, e) in self.texts.drain() {
            self.gl.delete_texture(Some(&e.tex));
        }
        self.text_bytes = 0;
        for (_, e) in self.labels.drain() {
            self.gl.delete_texture(Some(&e.tex));
        }
    }

    // ---------------- images ----------------

    pub fn image(&mut self, asset: &str) -> Option<(&WebGlTexture, f64, f64)> {
        if !self.images.contains_key(asset) {
            self.images.insert(asset.to_string(), ImgState::Loading);
            if let Ok(img) = HtmlImageElement::new() {
                let queue = self.loaded.clone();
                let wake = self.wake.clone();
                let id = asset.to_string();
                let img2 = img.clone();
                let onload = Closure::once(Box::new(move || {
                    queue.borrow_mut().push((id, Some(img2)));
                    wake.set(true);
                }) as Box<dyn FnOnce()>);
                let queue = self.loaded.clone();
                let wake = self.wake.clone();
                let id = asset.to_string();
                let onerror = Closure::once(Box::new(move || {
                    queue.borrow_mut().push((id, None));
                    wake.set(true);
                }) as Box<dyn FnOnce()>);
                img.set_onload(Some(onload.as_ref().unchecked_ref()));
                img.set_onerror(Some(onerror.as_ref().unchecked_ref()));
                onload.forget();
                onerror.forget();
                img.set_src(&format!("{}{}", self.asset_base, asset));
            }
        }
        let frame = self.frame_no;
        match self.images.get_mut(asset) {
            Some(ImgState::Ready { tex, w, h, used, .. }) => {
                *used = frame;
                Some((&*tex, *w, *h))
            }
            _ => None,
        }
    }

    fn upload_loaded_images(&mut self) {
        let items: Vec<(String, Option<HtmlImageElement>)> = self.loaded.borrow_mut().drain(..).collect();
        for (id, img) in items {
            let Some(img) = img else {
                self.images.insert(id, ImgState::Failed);
                continue;
            };
            let gl = &self.gl;
            let Some(tex) = gl.create_texture() else { continue };
            gl.bind_texture(GL::TEXTURE_2D, Some(&tex));
            let (nw, nh) = (img.natural_width(), img.natural_height());
            let long = nw.max(nh);
            let ok = if long > MAX_IMAGE_SIDE {
                // Downscale big photos before upload to bound GPU memory.
                let s = MAX_IMAGE_SIDE as f64 / long as f64;
                let (w, h) = (((nw as f64) * s).round().max(1.0) as u32, ((nh as f64) * s).round().max(1.0) as u32);
                match downscale(&img, w, h) {
                    Some(c) => gl
                        .tex_image_2d_with_u32_and_u32_and_html_canvas_element(GL::TEXTURE_2D, 0, GL::RGBA as i32, GL::RGBA, GL::UNSIGNED_BYTE, &c)
                        .is_ok(),
                    None => false,
                }
            } else {
                gl.tex_image_2d_with_u32_and_u32_and_html_image_element(
                    GL::TEXTURE_2D,
                    0,
                    GL::RGBA as i32,
                    GL::RGBA,
                    GL::UNSIGNED_BYTE,
                    &img,
                )
                .is_ok()
            };
            gl.generate_mipmap(GL::TEXTURE_2D);
            gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MIN_FILTER, GL::LINEAR_MIPMAP_LINEAR as i32);
            gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MAG_FILTER, GL::LINEAR as i32);
            gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_S, GL::CLAMP_TO_EDGE as i32);
            gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_T, GL::CLAMP_TO_EDGE as i32);
            if ok {
                let (w, h) = (img.natural_width() as f64, img.natural_height() as f64);
                let s = (MAX_IMAGE_SIDE as f64 / w.max(h).max(1.0)).min(1.0);
                // RGBA + ~1/3 for the mip chain.
                let bytes = ((w * s) * (h * s) * 4.0 * 1.34) as usize;
                self.images.insert(id, ImgState::Ready { tex, w, h, bytes, used: self.frame_no });
                self.invalidate();
                self.evict_images();
            } else {
                self.images.insert(id, ImgState::Failed);
            }
        }
    }

    pub fn image_size(&self, asset: &str) -> Option<(f64, f64)> {
        match self.images.get(asset) {
            Some(ImgState::Ready { w, h, .. }) => Some((*w, *h)),
            _ => None,
        }
    }

    fn evict_images(&mut self) {
        let total: usize = self.images.values().map(|i| if let ImgState::Ready { bytes, .. } = i { *bytes } else { 0 }).sum();
        if total <= IMAGE_MEMORY_BUDGET {
            return;
        }
        let mut by_age: Vec<(u64, String, usize)> = self
            .images
            .iter()
            .filter_map(|(k, i)| match i {
                ImgState::Ready { used, bytes, .. } if *used + 2 < self.frame_no => Some((*used, k.clone(), *bytes)),
                _ => None,
            })
            .collect();
        by_age.sort();
        let mut total = total;
        for (_, id, bytes) in by_age {
            if total <= IMAGE_MEMORY_BUDGET * 3 / 4 {
                break;
            }
            if let Some(ImgState::Ready { tex, .. }) = self.images.remove(&id) {
                self.gl.delete_texture(Some(&tex));
                total -= bytes;
            }
        }
    }

    // ---------------- display list ----------------

    pub fn build(&mut self, doc: &Document, scene: &Scene, hidden: &HashSet<NodeId>, version: u64) {
        self.upload_loaded_images();
        let mut inst: Vec<f32> = Vec::with_capacity(scene.order.len() * INST_FLOATS * 2);
        let mut cmds: Vec<Cmd> = Vec::new();
        let mut clips: Vec<(usize, Vec<(u32, Option<String>)>)> = Vec::new();
        let push_shape = |cmds: &mut Vec<Cmd>, idx: u32| match cmds.last_mut() {
            Some(Cmd::Shapes { start, count }) if *start + *count == idx => *count += 1,
            _ => cmds.push(Cmd::Shapes { start: idx, count: 1 }),
        };
        for (i, item) in scene.order.iter().enumerate() {
            while let Some((end, _)) = clips.last() {
                if *end < i {
                    let (_, parts) = clips.pop().unwrap();
                    cmds.push(Cmd::PopClip { parts });
                } else {
                    break;
                }
            }
            let Some(n) = doc.get(&item.id) else { continue };
            let opacity = item.parent_opacity * n.opacity as f32;
            let world = scene.world_of(&n.id);
            let is_hidden = hidden.contains(&n.id) || opacity <= 0.0;
            if !is_hidden {
                match n.kind {
                    NodeType::Rectangle | NodeType::Frame | NodeType::Ellipse | NodeType::Component | NodeType::Instance => {
                        self.build_box(n, &world, opacity, &mut inst, &mut cmds, &push_shape);
                    }
                    NodeType::Text => {
                        if !n.characters.is_empty() {
                            cmds.push(Cmd::Text { id: n.id.clone(), opacity });
                        }
                    }
                    NodeType::Vector | NodeType::Polygon | NodeType::Star | NodeType::Line => {
                        let fill = n
                            .fills
                            .iter()
                            .rev()
                            .find(|p| p.is_visible())
                            .filter(|_| n.kind != NodeType::Line)
                            .map(|p| premul(p.approx_color(), opacity));
                        let stroke = n
                            .strokes
                            .iter()
                            .rev()
                            .find(|p| p.is_visible())
                            .filter(|_| n.stroke_weight > 0.0)
                            .map(|p| premul(p.approx_color(), opacity));
                        if fill.is_some() || stroke.is_some() {
                            cmds.push(Cmd::Mesh { id: n.id.clone(), fill, stroke });
                        }
                    }
                    _ => {}
                }
            }
            if n.kind.is_frame_like() && n.clips_content && item.span > 0 && n.clip_alpha {
                // Alpha mask: every visible fill stencils where it isn't transparent.
                let ellipse = n.kind == NodeType::Ellipse;
                let mut parts = Vec::new();
                for p in n.fills.iter().filter(|p| p.is_visible()) {
                    let FillParams { ptype, angle, nstops, colors, stops, image } = self.fill_params(Some(p));
                    let ci = (inst.len() / INST_FLOATS) as u32;
                    push_instance(
                        &mut inst,
                        &world,
                        n.width,
                        n.height,
                        ellipse,
                        3.0,
                        n.radii(),
                        colors,
                        stops,
                        [ptype, angle, nstops, 1.0],
                        [0.0; 4],
                        [0.0; 4],
                        [0.0; 4],
                    );
                    parts.push((ci, image));
                }
                cmds.push(Cmd::PushClip { parts: parts.clone() });
                clips.push((i + item.span as usize, parts));
            } else if n.kind.is_frame_like() && n.clips_content && item.span > 0 && !content_fits(doc, scene, i, item.span as usize, n, &world) {
                let ci = (inst.len() / INST_FLOATS) as u32;
                push_instance(
                    &mut inst,
                    &world,
                    n.width,
                    n.height,
                    false,
                    2.0,
                    n.radii(),
                    [[0.0; 4]; 4],
                    [0.0; 4],
                    [0.0, 0.0, 0.0, 1.0],
                    [0.0; 4],
                    [0.0; 4],
                    [0.0; 4],
                );
                cmds.push(Cmd::PushClip { parts: vec![(ci, None)] });
                clips.push((i + item.span as usize, vec![(ci, None)]));
            }
        }
        while let Some((_, parts)) = clips.pop() {
            cmds.push(Cmd::PopClip { parts });
        }
        let gl = &self.gl;
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.inst_buf));
        gl::upload_growable(gl, GL::ARRAY_BUFFER, &inst, &mut self.inst_cap, GL::DYNAMIC_DRAW);
        self.instances = inst;
        self.cmds = cmds;
        self.built_version = version;
        // Drop caches for nodes that no longer exist.
        if self.meshes.len() > scene.order.len() * 2 + 64 {
            let keep: HashSet<&NodeId> = scene.order.iter().map(|i| &i.id).collect();
            let gl = self.gl.clone();
            self.meshes.retain(|k, e| {
                let k2 = keep.contains(k);
                if !k2 {
                    for (a, b, _) in [e.fill.take(), e.stroke.take()].into_iter().flatten() {
                        gl.delete_buffer(Some(&a));
                        gl.delete_buffer(Some(&b));
                    }
                }
                k2
            });
        }
        if self.texts.len() > scene.order.len() + 64 {
            let keep: HashSet<&NodeId> = scene.order.iter().map(|i| &i.id).collect();
            let gl = self.gl.clone();
            let mut freed = 0;
            self.texts.retain(|k, e| {
                let k2 = keep.contains(k);
                if !k2 {
                    gl.delete_texture(Some(&e.tex));
                    freed += e.bytes;
                }
                k2
            });
            self.text_bytes = self.text_bytes.saturating_sub(freed);
        }
    }

    /// Shader parameters for one fill paint (`None`: no fill).
    fn fill_params(&mut self, paint: Option<&Paint>) -> FillParams {
        let mut colors = [[0.0f32; 4]; 4];
        let mut stops = [0.0f32; 4];
        let mut ptype = 0.0;
        let mut angle = 0.0;
        let mut nstops = 0.0;
        let mut image: Option<String> = None;
        match paint {
            Some(Paint::Solid { color, opacity: po, .. }) => {
                ptype = 1.0;
                colors[0] = premul(*color, *po);
            }
            Some(Paint::LinearGradient { stops: st, angle: a, opacity: po, .. }) => {
                ptype = 2.0;
                angle = a.to_radians();
                nstops = st.len().min(4) as f32;
                let mut sorted: Vec<_> = st.iter().collect();
                sorted.sort_by(|a, b| a.position.partial_cmp(&b.position).unwrap_or(std::cmp::Ordering::Equal));
                for (k, s) in sorted.iter().take(4).enumerate() {
                    colors[k] = premul(s.color, *po);
                    stops[k] = s.position;
                }
            }
            Some(Paint::Image { asset, fit, opacity: po, tint, slice, .. }) => {
                ptype = 3.0;
                let fit_v = match fit {
                    ImageFit::Fill => 0.0,
                    ImageFit::Fit => 1.0,
                    ImageFit::Stretch => 2.0,
                };
                match self.image(asset).map(|(_, w, h)| (w, h)) {
                    Some((w, h)) => {
                        // c0 = tint, c1 = (imgW, imgH, fit, alpha),
                        // c2 = 9-slice source insets (top, right, bottom, left) in image px,
                        // c3 = (layer px per image px, fill center, sliced, 0).
                        colors[0] = premul(tint.unwrap_or(Color::WHITE), 1.0);
                        colors[1] = [w as f32, h as f32, fit_v, *po];
                        if let Some(s) = slice {
                            colors[2] = s.source_insets(w, h).map(|v| v as f32);
                            colors[3] = [s.scale.max(0.0), if s.fill_center { 1.0 } else { 0.0 }, 1.0, 0.0];
                        }
                        image = Some(asset.clone());
                    }
                    None => {
                        // Placeholder while loading.
                        ptype = 1.0;
                        colors[0] = premul(Color::rgba(0.9, 0.9, 0.92, 1.0), 1.0);
                    }
                }
            }
            None => {}
        }
        FillParams { ptype, angle, nstops, colors, stops, image }
    }

    fn build_box(
        &mut self,
        n: &Node,
        world: &Affine,
        opacity: f32,
        inst: &mut Vec<f32>,
        cmds: &mut Vec<Cmd>,
        push_shape: &dyn Fn(&mut Vec<Cmd>, u32),
    ) {
        let ellipse = n.kind == NodeType::Ellipse;
        let radii = if ellipse { [0.0; 4] } else { n.radii() };
        // Shadows below the shape.
        for e in &n.effects {
            if let Effect::DropShadow { color, x, y, blur, spread, visible } = e {
                if !*visible || color.a <= 0.0 {
                    continue;
                }
                let m = Affine::translate(*x as f64, *y as f64).mul(world);
                let idx = (inst.len() / INST_FLOATS) as u32;
                let pad = *blur + spread.max(0.0) + 2.0;
                push_instance(
                    inst,
                    &m,
                    n.width,
                    n.height,
                    ellipse,
                    1.0,
                    radii,
                    [premul(*color, 1.0), [0.0; 4], [0.0; 4], [0.0; 4]],
                    [0.0; 4],
                    [0.0, 0.0, 0.0, opacity],
                    [0.0; 4],
                    [0.0, 0.0, pad, 0.0],
                    [*x, *y, *blur, *spread],
                );
                push_shape(cmds, idx);
            }
        }
        let fills: Vec<&Paint> = n.fills.iter().filter(|p| p.is_visible() && !n.hide_fill).collect();
        let strokes: Vec<&Paint> = if n.stroke_weight > 0.0 { n.strokes.iter().filter(|p| p.is_visible()).collect() } else { vec![] };
        let align = match n.stroke_align {
            StrokeAlign::Center => 0.0,
            StrokeAlign::Inside => 1.0,
            StrokeAlign::Outside => 2.0,
        };
        let sw = n.stroke_weight as f32;
        let stroke_pad = match n.stroke_align {
            StrokeAlign::Center => sw / 2.0,
            StrokeAlign::Inside => 0.0,
            StrokeAlign::Outside => sw,
        };
        let stroke_color = |p: &Paint| premul(p.approx_color(), 1.0);
        let total = fills.len().max(1);
        for i in 0..total {
            let paint = fills.get(i).copied();
            let last = i + 1 == total;
            let (stroke, strokep) = if last {
                match strokes.first() {
                    Some(s) => (stroke_color(s), [sw, align, stroke_pad, 0.0]),
                    None => ([0.0; 4], [0.0; 4]),
                }
            } else {
                ([0.0; 4], [0.0; 4])
            };
            let FillParams { ptype, angle, nstops, colors, stops, image } = self.fill_params(paint);
            if ptype == 0.0 && stroke[3] == 0.0 {
                continue;
            }
            let idx = (inst.len() / INST_FLOATS) as u32;
            push_instance(
                inst,
                world,
                n.width,
                n.height,
                ellipse,
                0.0,
                radii,
                colors,
                stops,
                [ptype, angle, nstops, opacity],
                stroke,
                strokep,
                [0.0; 4],
            );
            match image {
                Some(asset) => cmds.push(Cmd::Image { inst: idx, asset }),
                None => push_shape(cmds, idx),
            }
        }
        // Extra strokes.
        for s in strokes.iter().skip(1) {
            let idx = (inst.len() / INST_FLOATS) as u32;
            push_instance(
                inst,
                world,
                n.width,
                n.height,
                ellipse,
                0.0,
                radii,
                [[0.0; 4]; 4],
                [0.0; 4],
                [0.0, 0.0, 0.0, opacity],
                stroke_color(s),
                [sw, align, stroke_pad, 0.0],
                [0.0; 4],
            );
            push_shape(cmds, idx);
        }
    }

    // ---------------- drawing ----------------

    fn draw_clip_parts(&mut self, parts: &[(u32, Option<String>)]) {
        let gl = self.gl.clone();
        for (inst, asset) in parts {
            let tex = asset.as_deref().and_then(|a| self.image(a).map(|(t, _, _)| t.clone()));
            gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.inst_buf));
            self.bind_shape_attribs(*inst);
            gl.bind_texture(GL::TEXTURE_2D, Some(tex.as_ref().unwrap_or(&self.white)));
            gl.draw_arrays_instanced(GL::TRIANGLE_STRIP, 0, 4, 1);
            self.draw_calls += 1;
        }
    }

    fn bind_shape_attribs(&self, start: u32) {
        let gl = &self.gl;
        let base = start as i32 * STRIDE;
        for i in 0..13u32 {
            gl.vertex_attrib_pointer_with_i32(i, 4, GL::FLOAT, false, STRIDE, base + (i as i32) * 16);
        }
    }

    fn mesh_for(&mut self, n: &Node, zoom_px: f64) -> Option<&MeshEntry> {
        let bucket = zoom_px.max(0.01).log2().ceil() as i32;
        let key = mesh_key(n, bucket);
        let fresh = self.meshes.get(&n.id).map(|e| e.key == key).unwrap_or(false);
        if !fresh {
            let tol = (0.3 / 2f64.powi(bucket)).clamp(0.005, 2.0) as f32;
            let segs = path::node_path(n);
            let lp = lyon_path(&segs);
            let gl = &self.gl;
            let upload = |geom: &lt::VertexBuffers<[f32; 2], u32>| -> Option<(WebGlBuffer, WebGlBuffer, i32)> {
                if geom.indices.is_empty() {
                    return None;
                }
                let vb = gl.create_buffer()?;
                let ib = gl.create_buffer()?;
                gl.bind_buffer(GL::ARRAY_BUFFER, Some(&vb));
                let flat: Vec<f32> = geom.vertices.iter().flat_map(|v| [v[0], v[1]]).collect();
                gl::buffer_f32(gl, GL::ARRAY_BUFFER, &flat, GL::STATIC_DRAW);
                gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, Some(&ib));
                unsafe {
                    let view = js_sys::Uint32Array::view(&geom.indices);
                    gl.buffer_data_with_array_buffer_view(GL::ELEMENT_ARRAY_BUFFER, &view, GL::STATIC_DRAW);
                }
                Some((vb, ib, geom.indices.len() as i32))
            };
            let mut fill = None;
            if n.kind != NodeType::Line && n.fills.iter().any(|p| p.is_visible()) {
                let mut geom: lt::VertexBuffers<[f32; 2], u32> = lt::VertexBuffers::new();
                let ok = lt::FillTessellator::new()
                    .tessellate_path(
                        &lp,
                        &lt::FillOptions::tolerance(tol).with_fill_rule(match n.fill_rule {
                            flexma_doc::FillRule::Nonzero => lt::FillRule::NonZero,
                            flexma_doc::FillRule::Evenodd => lt::FillRule::EvenOdd,
                        }),
                        &mut lt::BuffersBuilder::new(&mut geom, |v: lt::FillVertex| v.position().to_array()),
                    )
                    .is_ok();
                if ok {
                    fill = upload(&geom);
                }
            }
            let mut stroke = None;
            if n.has_visible_stroke() {
                let mut geom: lt::VertexBuffers<[f32; 2], u32> = lt::VertexBuffers::new();
                let ok = lt::StrokeTessellator::new()
                    .tessellate_path(
                        &lp,
                        &lt::StrokeOptions::tolerance(tol)
                            .with_line_width(n.stroke_weight as f32)
                            .with_line_join(match n.stroke_join {
                                flexma_doc::StrokeJoin::Miter => lt::LineJoin::Miter,
                                flexma_doc::StrokeJoin::Round => lt::LineJoin::Round,
                                flexma_doc::StrokeJoin::Bevel => lt::LineJoin::Bevel,
                            })
                            .with_line_cap(match n.stroke_cap {
                                Some(flexma_doc::StrokeCap::Round) => lt::LineCap::Round,
                                Some(flexma_doc::StrokeCap::Square) => lt::LineCap::Square,
                                Some(flexma_doc::StrokeCap::None) => lt::LineCap::Butt,
                                None if n.kind == NodeType::Line => lt::LineCap::Round,
                                None => lt::LineCap::Butt,
                            }),
                        &mut lt::BuffersBuilder::new(&mut geom, |v: lt::StrokeVertex| v.position().to_array()),
                    )
                    .is_ok();
                if ok {
                    stroke = upload(&geom);
                }
            }
            if let Some(old) = self.meshes.insert(n.id.clone(), MeshEntry { key, fill, stroke }) {
                for (a, b, _) in [old.fill, old.stroke].into_iter().flatten() {
                    gl.delete_buffer(Some(&a));
                    gl.delete_buffer(Some(&b));
                }
            }
        }
        self.meshes.get(&n.id)
    }

    fn text_texture(&mut self, n: &Node, text: &mut TextEngine, bucket: f64, allow_raster: bool, budget: &mut (usize, usize)) -> Option<(WebGlTexture, [f64; 4])> {
        let content = text_content_hash(n);
        let frame = self.frame_no;
        if let Some(e) = self.texts.get_mut(&n.id) {
            if e.content == content && (e.bucket == bucket || !allow_raster) {
                e.used = frame;
                return Some((e.tex.clone(), e.rect));
            }
        }
        if budget.0 == 0 || budget.1 == 0 {
            self.wants_more = true;
            return self.texts.get(&n.id).filter(|e| e.content == content).map(|e| (e.tex.clone(), e.rect));
        }
        budget.0 -= 1;
        let r = text.rasterize(n, bucket, self.max_tex.min(MAX_TEXT_TEXTURE))?;
        budget.1 = budget.1.saturating_sub((r.px_w as usize) * (r.px_h as usize));
        let gl = &self.gl;
        let tex = match self.texts.remove(&n.id) {
            Some(e) => {
                self.text_bytes = self.text_bytes.saturating_sub(e.bytes);
                e.tex
            }
            None => gl.create_texture()?,
        };
        gl.bind_texture(GL::TEXTURE_2D, Some(&tex));
        let _ = gl.tex_image_2d_with_u32_and_u32_and_html_canvas_element(
            GL::TEXTURE_2D,
            0,
            GL::RGBA as i32,
            GL::RGBA,
            GL::UNSIGNED_BYTE,
            &text.canvas,
        );
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MIN_FILTER, GL::LINEAR as i32);
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MAG_FILTER, GL::LINEAR as i32);
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_S, GL::CLAMP_TO_EDGE as i32);
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_T, GL::CLAMP_TO_EDGE as i32);
        let bytes = (r.px_w as usize) * (r.px_h as usize) * 4;
        self.text_bytes += bytes;
        self.texts.insert(n.id.clone(), TextEntry { content, bucket, tex: tex.clone(), rect: r.rect, bytes, used: frame });
        Some((tex, r.rect))
    }

    /// Free least-recently-drawn text rasters once over the memory budget.
    fn evict_text(&mut self) {
        if self.text_bytes <= TEXT_MEMORY_BUDGET {
            return;
        }
        let target = TEXT_MEMORY_BUDGET * 3 / 4;
        let mut by_age: Vec<(u64, NodeId)> =
            self.texts.iter().filter(|(_, e)| e.used != self.frame_no).map(|(k, e)| (e.used, k.clone())).collect();
        by_age.sort();
        for (_, id) in by_age {
            if self.text_bytes <= target {
                break;
            }
            if let Some(e) = self.texts.remove(&id) {
                self.gl.delete_texture(Some(&e.tex));
                self.text_bytes = self.text_bytes.saturating_sub(e.bytes);
            }
        }
    }

    /// Texture memory held by text rasters (for diagnostics).
    pub fn text_memory(&self) -> usize {
        self.text_bytes
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        doc: &Document,
        scene: &Scene,
        text: &mut TextEngine,
        cam: &Camera,
        css_w: f64,
        css_h: f64,
        dpr: f64,
        background: Color,
        overlay: &[f32],
        labels: &[Label],
        interacting: bool,
    ) {
        self.frame_no += 1;
        self.wants_more = false;
        self.draw_calls = 0;
        self.upload_loaded_images();
        let gl = self.gl.clone();
        let pw = (css_w * dpr).round() as i32;
        let ph = (css_h * dpr).round() as i32;
        gl.viewport(0, 0, pw, ph);
        gl.clear_color(background.r, background.g, background.b, 1.0);
        gl.clear_stencil(0);
        gl.clear(GL::COLOR_BUFFER_BIT | GL::STENCIL_BUFFER_BIT);
        let view = view_matrix(cam, css_w, css_h);
        let px = (cam.zoom * dpr) as f32;
        let visible = cam.visible_world(css_w, css_h);

        gl.enable(GL::STENCIL_TEST);
        gl.stencil_func(GL::EQUAL, 0, 0xff);
        gl.stencil_op(GL::KEEP, GL::KEEP, GL::KEEP);
        let mut depth = 0i32;

        // Text resolution: power-of-two buckets of device px per unit.
        let bucket = 2f64.powf((cam.zoom * dpr).max(0.05).log2().ceil()).clamp(0.125, 16.0);
        let mut budget = (MAX_TEXT_RASTERS_PER_FRAME, TEXT_PIXEL_BUDGET);

        let cmds = std::mem::take(&mut self.cmds);
        let mut current: u8 = 0; // 1 shape, 2 mesh, 3 tex
        // Contiguous shape runs are merged into one instanced draw as long as
        // nothing in between actually draws (e.g. tiny or off-screen text).
        let mut pending: Option<(u32, u32)> = None;
        macro_rules! flush_shapes {
            () => {
                if let Some((s0, e0)) = pending.take() {
                    if current != 1 {
                        self.use_shape(&view, px);
                        current = 1;
                    }
                    gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.inst_buf));
                    self.bind_shape_attribs(s0);
                    gl.bind_texture(GL::TEXTURE_2D, Some(&self.white));
                    gl.draw_arrays_instanced(GL::TRIANGLE_STRIP, 0, 4, (e0 - s0) as i32);
                    self.draw_calls += 1;
                }
            };
        }
        for cmd in &cmds {
            match cmd {
                Cmd::Shapes { start, count } => {
                    pending = match pending {
                        Some((s0, e0)) if e0 == *start => Some((s0, start + count)),
                        Some(_) => {
                            flush_shapes!();
                            Some((*start, start + count))
                        }
                        None => Some((*start, start + count)),
                    };
                }
                Cmd::Image { inst, asset } => {
                    flush_shapes!();
                    if current != 1 {
                        self.use_shape(&view, px);
                        current = 1;
                    }
                    let tex = self.image(asset).map(|(t, _, _)| t.clone());
                    gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.inst_buf));
                    self.bind_shape_attribs(*inst);
                    gl.bind_texture(GL::TEXTURE_2D, Some(tex.as_ref().unwrap_or(&self.white)));
                    gl.draw_arrays_instanced(GL::TRIANGLE_STRIP, 0, 4, 1);
                    self.draw_calls += 1;
                }
                Cmd::PushClip { parts } => {
                    flush_shapes!();
                    if current != 1 {
                        self.use_shape(&view, px);
                        current = 1;
                    }
                    gl.color_mask(false, false, false, false);
                    gl.stencil_func(GL::EQUAL, depth, 0xff);
                    gl.stencil_op(GL::KEEP, GL::KEEP, GL::INCR);
                    // Overlapping parts only count once: a pixel already raised fails EQUAL.
                    self.draw_clip_parts(parts);
                    depth += 1;
                    gl.color_mask(true, true, true, true);
                    gl.stencil_op(GL::KEEP, GL::KEEP, GL::KEEP);
                    gl.stencil_func(GL::EQUAL, depth, 0xff);
                }
                Cmd::PopClip { parts } => {
                    flush_shapes!();
                    if current != 1 {
                        self.use_shape(&view, px);
                        current = 1;
                    }
                    gl.color_mask(false, false, false, false);
                    gl.stencil_func(GL::EQUAL, depth, 0xff);
                    gl.stencil_op(GL::KEEP, GL::KEEP, GL::DECR);
                    self.draw_clip_parts(parts);
                    depth = (depth - 1).max(0);
                    gl.color_mask(true, true, true, true);
                    gl.stencil_op(GL::KEEP, GL::KEEP, GL::KEEP);
                    gl.stencil_func(GL::EQUAL, depth, 0xff);
                }
                Cmd::Mesh { id, fill, stroke } => {
                    let Some(n) = doc.get(id) else { continue };
                    let world = scene.world_of(id);
                    if !world.map_rect(&flexma_doc::Rect::new(0.0, 0.0, n.width, n.height)).inflate(n.stroke_weight + 2.0).intersects(&visible) {
                        continue;
                    }
                    let n = n.clone();
                    let zoom_px = cam.zoom * dpr;
                    let (fill_buf, stroke_buf) = match self.mesh_for(&n, zoom_px) {
                        Some(e) => (e.fill.clone(), e.stroke.clone()),
                        None => continue,
                    };
                    flush_shapes!();
                    if current != 2 {
                        gl.use_program(Some(&self.mesh_prog));
                        gl.bind_vertex_array(Some(&self.mesh_vao));
                        gl.uniform_matrix3fv_with_f32_array(self.mesh_u.get(0), false, &view);
                        current = 2;
                    }
                    gl.uniform3f(self.mesh_u.get(1), world.a as f32, world.c as f32, world.e as f32);
                    gl.uniform3f(self.mesh_u.get(2), world.b as f32, world.d as f32, world.f as f32);
                    for (buf, color) in [(fill_buf, fill), (stroke_buf, stroke)] {
                        if let (Some((vb, ib, count)), Some(c)) = (buf, color) {
                            gl.bind_buffer(GL::ARRAY_BUFFER, Some(&vb));
                            gl.vertex_attrib_pointer_with_i32(0, 2, GL::FLOAT, false, 8, 0);
                            gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, Some(&ib));
                            gl.uniform4f(self.mesh_u.get(3), c[0], c[1], c[2], c[3]);
                            gl.draw_elements_with_i32(GL::TRIANGLES, count, GL::UNSIGNED_INT, 0);
                            self.draw_calls += 1;
                        }
                    }
                }
                Cmd::Text { id, opacity } => {
                    let Some(n) = doc.get(id) else { continue };
                    let world = scene.world_of(id);
                    // The last raster's extent covers text overflowing its box.
                    let r = self.texts.get(id).map(|e| e.rect).unwrap_or([0.0, 0.0, n.width, n.height]);
                    let b = world.map_rect(&flexma_doc::Rect::new(r[0], r[1], r[2], r[3])).union(&world.map_rect(&flexma_doc::Rect::new(0.0, 0.0, n.width, n.height))).inflate(n.font_size);
                    if !b.intersects(&visible) {
                        continue;
                    }
                    // Tiny text: skip rasterizing when it would be < 1.5 px tall.
                    if n.font_size * cam.zoom * dpr < 1.5 {
                        continue;
                    }
                    let n = n.clone();
                    let Some((tex, rect)) = self.text_texture(&n, text, bucket, !interacting, &mut budget) else { continue };
                    flush_shapes!();
                    if current != 3 {
                        gl.use_program(Some(&self.tex_prog));
                        gl.bind_vertex_array(Some(&self.empty_vao));
                        gl.uniform_matrix3fv_with_f32_array(self.tex_u.get(0), false, &view);
                        current = 3;
                    }
                    gl.uniform3f(self.tex_u.get(1), world.a as f32, world.c as f32, world.e as f32);
                    gl.uniform3f(self.tex_u.get(2), world.b as f32, world.d as f32, world.f as f32);
                    gl.uniform4f(self.tex_u.get(3), rect[0] as f32, rect[1] as f32, rect[2] as f32, rect[3] as f32);
                    gl.uniform1f(self.tex_u.get(5), *opacity);
                    gl.bind_texture(GL::TEXTURE_2D, Some(&tex));
                    gl.draw_arrays(GL::TRIANGLE_STRIP, 0, 4);
                    self.draw_calls += 1;
                }
            }
        }
        flush_shapes!();
        let _ = current;
        self.cmds = cmds;
        self.evict_text();
        gl.disable(GL::STENCIL_TEST);

        // Overlay (world space instances).
        if !overlay.is_empty() {
            self.use_shape(&view, px);
            gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.overlay_buf));
            gl::upload_growable(&gl, GL::ARRAY_BUFFER, overlay, &mut self.overlay_cap, GL::DYNAMIC_DRAW);
            self.bind_shape_attribs(0);
            gl.bind_texture(GL::TEXTURE_2D, Some(&self.white));
            gl.draw_arrays_instanced(GL::TRIANGLE_STRIP, 0, 4, (overlay.len() / INST_FLOATS) as i32);
        }

        // Labels (screen space).
        if !labels.is_empty() {
            let sview = screen_matrix(css_w, css_h);
            gl.use_program(Some(&self.tex_prog));
            gl.bind_vertex_array(Some(&self.empty_vao));
            gl.uniform_matrix3fv_with_f32_array(self.tex_u.get(0), false, &sview);
            gl.uniform3f(self.tex_u.get(1), 1.0, 0.0, 0.0);
            gl.uniform3f(self.tex_u.get(2), 0.0, 1.0, 0.0);
            gl.uniform1f(self.tex_u.get(5), 1.0);
            for l in labels {
                let key = format!("{}|{}|{}|{}|{:?}|{}", l.text, l.size, l.weight, l.fg, l.bg, dpr);
                if !self.labels.contains_key(&key) {
                    let (pw, ph) = text.rasterize_label(&l.text, l.size, l.weight, &l.fg, l.bg.as_deref(), dpr);
                    let Some(tex) = gl.create_texture() else { continue };
                    gl.bind_texture(GL::TEXTURE_2D, Some(&tex));
                    let _ = gl.tex_image_2d_with_u32_and_u32_and_html_canvas_element(
                        GL::TEXTURE_2D,
                        0,
                        GL::RGBA as i32,
                        GL::RGBA,
                        GL::UNSIGNED_BYTE,
                        &text.canvas,
                    );
                    gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MIN_FILTER, GL::LINEAR as i32);
                    gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MAG_FILTER, GL::LINEAR as i32);
                    gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_S, GL::CLAMP_TO_EDGE as i32);
                    gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_T, GL::CLAMP_TO_EDGE as i32);
                    self.labels.insert(key.clone(), LabelEntry { tex, w: pw as f64 / dpr, h: ph as f64 / dpr, used: self.frame_no });
                }
                let e = self.labels.get_mut(&key).unwrap();
                e.used = self.frame_no;
                let x = if l.center { (l.x - e.w / 2.0).round() } else { l.x.round() };
                gl.uniform4f(self.tex_u.get(3), x as f32, l.y.round() as f32, e.w as f32, e.h as f32);
                gl.bind_texture(GL::TEXTURE_2D, Some(&e.tex));
                gl.draw_arrays(GL::TRIANGLE_STRIP, 0, 4);
            }
            if self.labels.len() > 400 {
                let now = self.frame_no;
                let gl2 = gl.clone();
                self.labels.retain(|_, e| {
                    let keep = now - e.used < 120;
                    if !keep {
                        gl2.delete_texture(Some(&e.tex));
                    }
                    keep
                });
            }
        }
        gl.bind_vertex_array(None);
        if self.wake.get() {
            self.wants_more = true;
        }
    }

    fn use_shape(&self, view: &[f32; 9], px: f32) {
        let gl = &self.gl;
        gl.use_program(Some(&self.shape_prog));
        gl.bind_vertex_array(Some(&self.shape_vao));
        gl.uniform_matrix3fv_with_f32_array(self.shape_u.get(0), false, view);
        gl.uniform1f(self.shape_u.get(1), px);
        gl.uniform1i(self.shape_u.get(2), 0);
        gl.active_texture(GL::TEXTURE0);
    }

    pub fn image_cache_len(&self) -> usize {
        self.images.len()
    }

    pub fn instance_count(&self) -> usize {
        self.instances.len() / INST_FLOATS
    }

    pub fn _hash(&self) -> u64 {
        hash_of(&self.cmds.len())
    }
}
