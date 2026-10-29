//! The GPU interface the renderer draws through: WebGPU when the browser has
//! it (`gpu.rs`), WebGL2 otherwise (`gl.rs`).
//!
//! The renderer decides *what* to draw (display list, caches, memory
//! budgets). A backend only owns GPU resources and replays a frame's draws:
//! scene shapes, meshes and text in painter's order with nested stencil clips,
//! then (outside the clips and any linear-blending target) the overlay and
//! screen-space labels.

use flexma_doc::Color;
use std::cell::Cell;
use std::rc::Rc;
use web_sys::{HtmlCanvasElement, HtmlImageElement};

/// A texture owned by the backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TexId(pub u32);

/// A triangle mesh (positions + u32 indices) owned by the backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshId(pub u32);

/// Pixels to upload. Both are premultiplied on upload.
pub enum ImageSource<'a> {
    Image(&'a HtmlImageElement),
    Canvas(&'a HtmlCanvasElement),
}

/// The two rows of a 2D affine transform: (a, c, e) and (b, d, f).
pub type Rows = ([f32; 3], [f32; 3]);

/// Floats per brush dab: x, y, radius, hardness (texel space), the
/// premultiplied color, then the tip angle (radians) and three spare floats.
pub const DAB_FLOATS: usize = 12;

/// Side of a smudge stroke's pickup buffer, in texels (it covers the tip).
pub const SMUDGE_BUF: u32 = 128;

/// How dabs look beyond a plain round stamp. Grain and tint textures are
/// fixed to the layer; `origin` is the tile's top-left in layer px.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DabStyle {
    pub tip: Option<TexId>,
    /// The tip's half-extents as a fraction of the dab radius (longer side 1),
    /// and its size in texels (for picking a mip level).
    pub tip_extent: [f32; 2],
    pub tip_px: [f32; 2],
    /// Stamp the tip's colors instead of the brush color.
    pub tip_color: bool,
    pub grain: Option<TexId>,
    /// The grain texture's size in layer px (it repeats).
    pub grain_size: [f32; 2],
    pub grain_strength: f32,
    /// Grain and tint texture sizes in texels (mip levels).
    pub grain_px: [f32; 2],
    pub tint_px: [f32; 2],
    pub tint: Option<TexId>,
    pub tint_size: [f32; 2],
    pub tint_strength: f32,
    pub origin: [f32; 2],
}

/// Dabs for one paint tile (`Backend::paint_dabs`).
pub struct DabBatch<'a> {
    pub tex: TexId,
    /// `DAB_FLOATS` per dab, texel space.
    pub dabs: &'a [f32],
    pub mode: PaintMode,
    pub style: DabStyle,
}

/// A layer tile under a smudge dab.
#[derive(Clone, Copy, Debug)]
pub struct SmudgeTile {
    /// None: not painted yet (reads as transparent, never written).
    pub tex: Option<TexId>,
    /// Top-left in layer px.
    pub origin: [f32; 2],
    pub write: bool,
}

/// One smudge dab: the layer under the dab is blended into the stroke's
/// pickup buffer, which is then laid down through the tip.
pub struct SmudgeDab<'a> {
    pub tiles: &'a [SmudgeTile],
    /// One dab (`DAB_FLOATS`), in layer px.
    pub dab: [f32; DAB_FLOATS],
    /// Tip and grain (`origin` unused: each tile has its own).
    pub style: DabStyle,
    /// The stroke's pickup buffer (a `SMUDGE_BUF` paint texture).
    pub buffer: TexId,
    /// The stroke's first dab: the buffer starts as the paint under it.
    pub first: bool,
    /// How much of the carried paint survives each dab.
    pub keep: f32,
    /// Premultiplied brush color mixed into the carried paint, and how much.
    pub paint: [f32; 4],
    pub mix: f32,
    pub alpha_lock: bool,
}

/// How brush dabs blend into a paint tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaintMode {
    Paint,
    Erase,
    /// Paint only where the tile already has alpha (lock transparency).
    AlphaLock,
}

pub struct FrameParams {
    /// Drawing buffer size in device pixels.
    pub width: u32,
    pub height: u32,
    pub background: Color,
    /// Blend the scene in linear light (a Unity project in Linear color space).
    pub linear: bool,
    /// World → clip space (column-major 3x3).
    pub view: [f32; 9],
    /// CSS px → clip space, for labels.
    pub screen: [f32; 9],
    /// Device px per world unit (antialiasing width).
    pub px: f32,
}

pub trait Backend {
    /// "webgpu" or "webgl2" (diagnostics).
    fn name(&self) -> &'static str;
    fn max_texture_size(&self) -> u32;

    /// Upload `w`×`h` pixels as a texture, replacing `reuse` if given. Image
    /// fills get mipmaps.
    fn upload_texture(&mut self, reuse: Option<TexId>, src: ImageSource, w: u32, h: u32, mipmaps: bool) -> Option<TexId>;
    fn delete_texture(&mut self, t: TexId);
    /// A brush tip or texture (mipmapped); `repeat` tiles it (grain, tint).
    fn upload_brush_texture(&mut self, src: ImageSource, w: u32, h: u32, repeat: bool) -> Option<TexId>;
    fn create_mesh(&mut self, vertices: &[f32], indices: &[u32]) -> Option<MeshId>;
    fn delete_mesh(&mut self, m: MeshId);
    /// The scene's shape instances (`gl::INST_FLOATS` floats each), drawn by index below.
    fn set_scene_instances(&mut self, data: &[f32]);

    fn begin_frame(&mut self, p: &FrameParams);
    /// Instanced shapes `first..first + count`, sampling `tex` (white when none).
    fn draw_shapes(&mut self, first: u32, count: u32, tex: Option<TexId>);
    /// Push (or pop) a stencil clip drawn from scene instances, each with its
    /// texture (alpha masks). Overlapping parts count once.
    fn clip(&mut self, parts: &[(u32, Option<TexId>)], push: bool);
    /// Background blur: blur what's been drawn so far (Gaussian σ in device
    /// px) and draw it inside the shape of `parts` (as for `clip`), under the
    /// layer that's drawn next.
    fn backdrop_blur(&mut self, parts: &[(u32, Option<TexId>)], sigma: f32);
    fn draw_mesh(&mut self, mesh: MeshId, m: Rows, color: [f32; 4]);
    /// A textured quad covering `rect` (x, y, w, h) in local space, showing
    /// the texture region `uv` (x0, y0, x1, y1).
    fn draw_texture(&mut self, tex: TexId, m: Rows, rect: [f32; 4], uv: [f32; 4], alpha: f32);
    /// The scene is done: resolve linear blending; overlays come next.
    fn end_scene(&mut self);
    /// World-space overlay instances (selection, handles, guides).
    fn draw_overlay(&mut self, data: &[f32]);
    /// A screen-space label at `rect` in CSS px.
    fn draw_label(&mut self, tex: TexId, rect: [f32; 4]);
    fn end_frame(&mut self);
    /// Draw calls issued by the last frame (diagnostics).
    fn draw_calls(&self) -> u32;
    /// Paint-tile GPU work so far: (render passes, queue submits, most ops
    /// that waited for the GPU at once).
    fn paint_counters(&self) -> [u64; 3] {
        [0, 0, 0]
    }
    /// Frames the GPU rejected so far (each is redrawn; see `attach_wake`).
    fn gpu_errors(&self) -> u64 {
        0
    }
    /// A flag the backend can raise to ask for another frame (e.g. after a
    /// frame failed on the GPU and should be redrawn).
    fn attach_wake(&mut self, _wake: Rc<Cell<bool>>) {}

    // ---- paint tiles (outside frames) ----

    /// A transparent `size`×`size` texture that brush dabs can be drawn into
    /// and that can be read back.
    fn create_paint_texture(&mut self, size: u32) -> Option<TexId>;
    /// Replace a paint texture's pixels with an image (a tile snapshot).
    fn load_paint_texture(&mut self, tex: TexId, src: ImageSource, size: u32) -> bool;
    fn clear_paint_texture(&mut self, tex: TexId, size: u32);
    /// Stamp batches of dabs into `TILE`-sized paint textures, in order (one
    /// GPU submission for all of them).
    fn paint_dabs(&mut self, batches: &[DabBatch]);
    /// Smudge dabs, in order (each one reads what the ones before it left).
    fn smudge(&mut self, dabs: &[SmudgeDab]);
    fn copy_paint_texture(&mut self, src: TexId, dst: TexId, size: u32);
    /// Read a paint texture back: premultiplied RGBA rows, top first. `done`
    /// runs later, without stalling the GPU (see `poll_readbacks`).
    fn read_paint_texture(&mut self, tex: TexId, size: u32, done: Box<dyn FnOnce(Vec<u8>)>);
    /// Finish readbacks the GPU is done with. True while some are pending.
    fn poll_readbacks(&mut self) -> bool {
        false
    }
    /// Get painting's GPU programs ready before the first stroke (they would
    /// otherwise compile mid-stroke).
    fn prewarm_paint(&mut self) {}

    /// Paint work is waiting for the GPU to catch up (the editor keeps
    /// drawing frames until it's done).
    fn paint_pending(&self) -> bool {
        false
    }
}

/// Mip levels for the grain and tint textures: where a texel is about a layer
/// pixel. Computed once here so both backends sample the same level.
pub fn texture_lods(st: &DabStyle) -> [f32; 2] {
    let lod = |px: [f32; 2], size: [f32; 2]| if size[0] > 0.0 && px[0] > 0.0 { (px[0] / size[0]).log2().max(0.0) } else { 0.0 };
    [lod(st.grain_px, st.grain_size), lod(st.tint_px, st.tint_size)]
}
