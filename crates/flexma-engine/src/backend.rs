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
    fn draw_mesh(&mut self, mesh: MeshId, m: Rows, color: [f32; 4]);
    /// A textured quad covering `rect` (x, y, w, h) in local space.
    fn draw_texture(&mut self, tex: TexId, m: Rows, rect: [f32; 4], alpha: f32);
    /// The scene is done: resolve linear blending; overlays come next.
    fn end_scene(&mut self);
    /// World-space overlay instances (selection, handles, guides).
    fn draw_overlay(&mut self, data: &[f32]);
    /// A screen-space label at `rect` in CSS px.
    fn draw_label(&mut self, tex: TexId, rect: [f32; 4]);
    fn end_frame(&mut self);
    /// Draw calls issued by the last frame (diagnostics).
    fn draw_calls(&self) -> u32;
    /// A flag the backend can raise to ask for another frame (e.g. after a
    /// frame failed on the GPU and should be redrawn).
    fn attach_wake(&mut self, _wake: Rc<Cell<bool>>) {}
}
