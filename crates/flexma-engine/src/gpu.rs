//! The WebGPU backend.
//!
//! Draws are recorded during the frame and encoded in `end_frame`, once every
//! draw's uniforms are known: they all go into one uniform buffer that draws
//! index with dynamic offsets. The scene renders into a 4x multisampled
//! target with a stencil buffer (nested clips) that resolves onto the canvas,
//! or, for pages with linear blending, into an sRGB texture that the final
//! pass encodes onto the canvas. Overlays and labels draw in that final pass.
//!
//! JavaScript creates the device and configures the canvas (both are async),
//! then hands them over; see `FlexmaEditor::with_webgpu`.

use crate::backend::{texture_lods, Backend, DabBatch, DabStyle, FrameParams, ImageSource, MeshId, PaintMode, Rows, SmudgeDab, SmudgeTile, TexId, DAB_FLOATS, SMUDGE_BUF};
use crate::gl::INST_FLOATS;
use js_sys::{Array, Object, Reflect};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use web_sys::{HtmlCanvasElement, HtmlImageElement};

#[wasm_bindgen]
extern "C" {
    #[derive(Clone)]
    pub type GpuDevice;
    #[wasm_bindgen(method, getter)]
    fn queue(this: &GpuDevice) -> GpuQueue;
    #[wasm_bindgen(method, getter)]
    fn limits(this: &GpuDevice) -> JsValue;
    #[wasm_bindgen(method, js_name = createBuffer)]
    fn create_buffer(this: &GpuDevice, d: &Object) -> GpuBuffer;
    #[wasm_bindgen(method, js_name = createTexture)]
    fn create_texture(this: &GpuDevice, d: &Object) -> GpuTexture;
    #[wasm_bindgen(method, js_name = createSampler)]
    fn create_sampler(this: &GpuDevice, d: &Object) -> JsValue;
    #[wasm_bindgen(method, js_name = createShaderModule)]
    fn create_shader_module(this: &GpuDevice, d: &Object) -> JsValue;
    #[wasm_bindgen(method, js_name = createBindGroupLayout)]
    fn create_bind_group_layout(this: &GpuDevice, d: &Object) -> JsValue;
    #[wasm_bindgen(method, js_name = createPipelineLayout)]
    fn create_pipeline_layout(this: &GpuDevice, d: &Object) -> JsValue;
    #[wasm_bindgen(method, js_name = createRenderPipeline)]
    fn create_render_pipeline(this: &GpuDevice, d: &Object) -> JsValue;
    #[wasm_bindgen(method, js_name = createBindGroup)]
    fn create_bind_group(this: &GpuDevice, d: &Object) -> JsValue;
    #[wasm_bindgen(method, js_name = createCommandEncoder)]
    fn create_command_encoder(this: &GpuDevice) -> GpuCommandEncoder;
    #[wasm_bindgen(method, js_name = pushErrorScope)]
    fn push_error_scope(this: &GpuDevice, filter: &str);
    #[wasm_bindgen(method, js_name = popErrorScope)]
    fn pop_error_scope(this: &GpuDevice) -> js_sys::Promise;

    #[derive(Clone)]
    pub type GpuQueue;
    #[wasm_bindgen(method, js_name = writeBuffer)]
    fn write_f32(this: &GpuQueue, b: &GpuBuffer, offset: f64, data: &[f32]);
    #[wasm_bindgen(method, js_name = writeBuffer)]
    fn write_u32(this: &GpuQueue, b: &GpuBuffer, offset: f64, data: &[u32]);
    #[wasm_bindgen(method, js_name = writeTexture)]
    fn write_texture(this: &GpuQueue, dst: &Object, data: &[u8], layout: &Object, size: &Array);
    #[wasm_bindgen(method, catch, js_name = copyExternalImageToTexture)]
    fn copy_external_image_to_texture(this: &GpuQueue, src: &Object, dst: &Object, size: &Array) -> Result<(), JsValue>;
    #[wasm_bindgen(method)]
    fn submit(this: &GpuQueue, buffers: &Array);
    #[wasm_bindgen(method, js_name = onSubmittedWorkDone)]
    fn on_submitted_work_done(this: &GpuQueue) -> js_sys::Promise;

    #[derive(Clone)]
    pub type GpuBuffer;
    #[wasm_bindgen(method)]
    fn destroy(this: &GpuBuffer);
    #[wasm_bindgen(method, js_name = mapAsync)]
    fn map_async(this: &GpuBuffer, mode: u32) -> js_sys::Promise;
    #[wasm_bindgen(method, js_name = getMappedRange)]
    fn get_mapped_range(this: &GpuBuffer) -> js_sys::ArrayBuffer;
    #[wasm_bindgen(method)]
    fn unmap(this: &GpuBuffer);

    #[derive(Clone)]
    pub type GpuTexture;
    #[wasm_bindgen(method, js_name = createView)]
    fn create_view(this: &GpuTexture) -> JsValue;
    #[wasm_bindgen(method, js_name = createView)]
    fn create_view_with(this: &GpuTexture, d: &Object) -> JsValue;
    #[wasm_bindgen(method, js_name = destroy)]
    fn destroy_texture(this: &GpuTexture);

    #[derive(Clone)]
    pub type GpuCanvasContext;
    #[wasm_bindgen(method)]
    fn configure(this: &GpuCanvasContext, d: &Object);
    #[wasm_bindgen(method, js_name = getCurrentTexture)]
    fn get_current_texture(this: &GpuCanvasContext) -> GpuTexture;

    pub type GpuCommandEncoder;
    #[wasm_bindgen(method, js_name = copyTextureToTexture)]
    fn copy_texture_to_texture(this: &GpuCommandEncoder, src: &Object, dst: &Object, size: &Array);
    #[wasm_bindgen(method, js_name = copyTextureToBuffer)]
    fn copy_texture_to_buffer(this: &GpuCommandEncoder, src: &Object, dst: &Object, size: &Array);
    #[wasm_bindgen(method, js_name = beginRenderPass)]
    fn begin_render_pass(this: &GpuCommandEncoder, d: &Object) -> GpuRenderPass;
    #[wasm_bindgen(method)]
    fn finish(this: &GpuCommandEncoder) -> JsValue;

    pub type GpuRenderPass;
    #[wasm_bindgen(method, js_name = setPipeline)]
    fn set_pipeline(this: &GpuRenderPass, p: &JsValue);
    #[wasm_bindgen(method, js_name = setBindGroup)]
    fn set_bind_group(this: &GpuRenderPass, index: u32, g: &JsValue);
    #[wasm_bindgen(method, js_name = setBindGroup)]
    fn set_bind_group_offsets(this: &GpuRenderPass, index: u32, g: &JsValue, offsets: &[u32]);
    #[wasm_bindgen(method, js_name = setVertexBuffer)]
    fn set_vertex_buffer(this: &GpuRenderPass, slot: u32, b: &GpuBuffer);
    #[wasm_bindgen(method, js_name = setIndexBuffer)]
    fn set_index_buffer(this: &GpuRenderPass, b: &GpuBuffer, format: &str);
    #[wasm_bindgen(method, js_name = setStencilReference)]
    fn set_stencil_reference(this: &GpuRenderPass, r: u32);
    #[wasm_bindgen(method)]
    fn draw(this: &GpuRenderPass, vertices: u32, instances: u32, first_vertex: u32, first_instance: u32);
    #[wasm_bindgen(method, js_name = drawIndexed)]
    fn draw_indexed(this: &GpuRenderPass, count: u32, instances: u32, first_index: u32, base_vertex: i32, first_instance: u32);
    #[wasm_bindgen(method)]
    fn end(this: &GpuRenderPass);
}

/// A plain JS object from key/value pairs.
macro_rules! obj {
    ($($k:literal : $v:expr),* $(,)?) => {{
        let o = Object::new();
        $( let _ = Reflect::set(&o, &JsValue::from_str($k), &JsValue::from($v)); )*
        o
    }};
}

fn arr(items: Vec<JsValue>) -> Array {
    items.into_iter().collect()
}

// GPUBufferUsage, GPUTextureUsage, GPUShaderStage and GPUColorWrite flags.
const BUF_MAP_READ: u32 = 0x1;
const BUF_COPY_DST: u32 = 0x8;
const BUF_INDEX: u32 = 0x10;
const BUF_VERTEX: u32 = 0x20;
const BUF_UNIFORM: u32 = 0x40;
const TEX_COPY_SRC: u32 = 0x1;
const TEX_COPY_DST: u32 = 0x2;
const TEX_BINDING: u32 = 0x4;
const TEX_RENDER: u32 = 0x10;
const STAGE_VERTEX: u32 = 0x1;
const STAGE_FRAGMENT: u32 = 0x2;
const WRITE_ALL: u32 = 0xF;

/// Floats per uniform slot (256 bytes, the dynamic-offset alignment).
const SLOT_FLOATS: usize = 64;
/// Bytes of the uniform struct the shaders read.
const UNIFORM_SIZE: u32 = 128;
const SAMPLES: u32 = 4;
const DEPTH_FORMAT: &str = "depth24plus-stencil8";
/// The linear-blending scene target: blending into it happens in linear light.
const LINEAR_FORMAT: &str = "rgba8unorm-srgb";
const TEX_FORMAT: &str = "rgba8unorm";

const SHADERS: &str = include_str!("shaders/flexma.wgsl");
const POST_SHADERS: &str = include_str!("shaders/post.wgsl");
const BLUR_SHADERS: &str = include_str!("shaders/blur.wgsl");
const BRUSH_SHADERS: &str = include_str!("shaders/brush.wgsl");
const DAB_SHADERS: &str = include_str!("shaders/dab.wgsl");
const SMUDGE_SHADERS: &str = include_str!("shaders/smudge.wgsl");
/// Bytes of the `Style` uniform (brush.wgsl) and of one `Smudge` uniform.
const STYLE_BYTES: u32 = 64;
const SMUDGE_BYTES: u32 = 176;
/// Uniform slots are 256-byte aligned (dynamic offsets).
const SLOT_BYTES: usize = 256;
/// The smudge window: the tiles under a dab, side by side.
const SMUDGE_WINDOW: u32 = 1024;
/// Most dabs drawn in one GPU submission, and most smudge dabs (each a few
/// passes): replaying a file's strokes goes out in many small submissions,
/// which the paint queue paces (see `GpuBackend::paint_cap`).
const DABS_PER_SUBMIT: u32 = 4096;
const SMUDGE_DABS_PER_SUBMIT: usize = 48;
/// Dab batches (about a pass each) per queued slice.
const DAB_BATCHES_PER_OP: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Stencil {
    /// Draw where the stencil equals the clip depth.
    Test,
    /// Clip push / pop: raise or lower the stencil, write no color.
    Incr,
    Decr,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Pipe {
    /// Scene pipelines (multisampled, stencil-tested); `true` = linear target.
    Shape(Stencil, bool),
    Mesh(bool),
    Tex(bool),
    /// Final-pass pipelines on the canvas.
    Overlay,
    Label,
    Composite,
    /// Mipmap generation.
    Blit,
    /// Brush dabs into a paint tile.
    Dab(PaintMode),
    /// Smudge: pick up into the stroke's buffer, lay it down onto a tile.
    SmudgePick,
    SmudgeStamp,
    /// Background blur: a blur pass, and the blurred copy onto the scene.
    Blur(bool),
    Backdrop(bool),
}

#[derive(Clone, Copy)]
enum Inst {
    Scene,
    Overlay,
}

enum Op {
    Shapes { inst: Inst, first: u32, count: u32, tex: Option<TexId>, stencil: Stencil, slot: u32 },
    Mesh { mesh: MeshId, slot: u32 },
    Tex { tex: TexId, slot: u32, label: bool },
    StencilRef(u32),
    Composite,
    /// Background blur here: the scene pass ends, what it drew is blurred
    /// (uniform slots across and down, at 1/`down` size), and the next pass
    /// starts with it drawn inside `parts`. `depth`: the clip depth.
    Backdrop { parts: Vec<(u32, Option<TexId>)>, depth: u32, slots: [u32; 2], down: u32 },
    /// The blurred copy (a texture bind group), full screen, stencil-tested.
    BackdropQuad(JsValue),
}

/// A texture to render into and sample from: texture, view, bind group.
struct RenderTex {
    tex: GpuTexture,
    view: JsValue,
    bind: JsValue,
}

struct TexEntry {
    tex: GpuTexture,
    bind: JsValue,
    w: u32,
    h: u32,
    mips: u32,
}

struct Target {
    w: u32,
    h: u32,
    linear: bool,
    msaa: GpuTexture,
    msaa_view: JsValue,
    depth: GpuTexture,
    depth_view: JsValue,
    /// Linear blending: the scene resolves here, then gets encoded onto the canvas.
    resolve: Option<(GpuTexture, JsValue, JsValue)>,
}

struct Grow {
    buf: GpuBuffer,
    cap: u64,
}

pub struct GpuBackend {
    device: GpuDevice,
    queue: GpuQueue,
    context: GpuCanvasContext,
    format: String,
    module: JsValue,
    post_module: JsValue,
    blur_module: JsValue,
    /// Background blur textures: the scene so far, and per small size the
    /// two blur passes' targets. Format: the scene's.
    backdrop_src: Option<(u32, u32, bool, RenderTex)>,
    backdrop_small: HashMap<(u32, u32, bool), (RenderTex, RenderTex)>,
    dab_module: JsValue,
    dab_bgl: JsValue,
    dab_layout: JsValue,
    dab_inst: Option<Grow>,
    dab_uniforms: Option<Grow>,
    smudge_module: JsValue,
    smudge_bgl: JsValue,
    smudge_layout: JsValue,
    smudge_uniform: Option<Grow>,
    /// Window and scratch textures (created on first use).
    smudge_window: Option<GpuTexture>,
    smudge_scratch: Option<GpuTexture>,
    smudge_blank: Option<GpuTexture>,
    repeat_sampler: JsValue,
    /// Paint-tile render passes and submits so far (diagnostics).
    paint_passes: u64,
    paint_submits: u64,
    uniform_bgl: JsValue,
    tex_bgl: JsValue,
    main_layout: JsValue,
    mesh_layout: JsValue,
    post_layout: JsValue,
    sampler: JsValue,
    pipelines: HashMap<Pipe, JsValue>,
    white: TexEntry,
    textures: Vec<Option<TexEntry>>,
    meshes: Vec<Option<(GpuBuffer, GpuBuffer, u32)>>,
    scene_inst: Option<Grow>,
    overlay_inst: Option<Grow>,
    uniforms: Option<(Grow, JsValue)>,
    target: Option<Target>,
    /// Canvas size the context was last configured for.
    configured: (u32, u32),
    max_tex: u32,
    // ---- the frame being recorded ----
    frame: Option<FrameParams>,
    scene_ops: Vec<Op>,
    post_ops: Vec<Op>,
    in_scene: bool,
    udata: Vec<f32>,
    scene_slot: u32,
    depth: u32,
    draw_calls: u32,
    /// Destroyed after the frame that may still use them is submitted.
    trash_tex: Vec<GpuTexture>,
    trash_buf: Vec<GpuBuffer>,
    /// Frames in a row that failed validation, and who to tell when it keeps happening.
    bad_frames: Rc<Cell<u32>>,
    /// Every rejected frame so far (diagnostics).
    errors: Rc<Cell<u64>>,
    on_error: js_sys::Function,
    wake: Rc<Cell<bool>>,
    /// Checks each frame's error scope (one closure, reused every frame).
    frame_check: Option<Closure<dyn FnMut(JsValue)>>,
    /// Most paint passes encoded that the GPU hasn't finished: a long replay
    /// (opening a painting) is encoded a slice at a time as the GPU finishes
    /// the last, so it neither freezes the page nor piles up in the GPU
    /// process (see `FlexmaEditor::with_webgpu`).
    paint_cap: u32,
    /// Paint work waiting for the GPU to catch up (see `paint_cap`).
    paint_queue: VecDeque<PaintOp>,
    /// Paint passes submitted that the GPU hasn't finished, each submit's
    /// share (submits finish in order), and the callback that settles them.
    paint_inflight: Rc<Cell<u32>>,
    paint_costs: Rc<RefCell<VecDeque<u32>>>,
    paint_done: Closure<dyn FnMut(JsValue)>,
    /// The most paint ops that have waited at once (stats).
    paint_queue_peak: usize,
}

/// Paint work that waits in `GpuBackend::paint_queue`, with its own copies
/// of the data.
enum PaintOp {
    Dabs(Vec<(TexId, Vec<f32>, PaintMode, DabStyle)>),
    Smudge(Vec<QueuedSmudge>),
    Load(TexId, QueuedImage, u32),
    Clear(TexId, u32),
    Copy(TexId, TexId, u32),
    Read(TexId, u32, Box<dyn FnOnce(Vec<u8>)>),
    Delete(TexId),
}

enum QueuedImage {
    Image(HtmlImageElement),
    Canvas(HtmlCanvasElement),
}

/// A `SmudgeDab` that owns its tiles.
struct QueuedSmudge {
    tiles: Vec<SmudgeTile>,
    dab: [f32; DAB_FLOATS],
    style: DabStyle,
    buffer: TexId,
    first: bool,
    keep: f32,
    paint: [f32; 4],
    mix: f32,
    alpha_lock: bool,
}

impl PaintOp {
    /// About how many passes (and copies) the op encodes.
    fn cost(&self) -> u32 {
        match self {
            PaintOp::Dabs(b) => b.len() as u32,
            PaintOp::Smudge(d) => smudge_cost(d.iter().map(|d| d.tiles.len())),
            PaintOp::Delete(_) => 0,
            _ => 1,
        }
    }
}

/// Passes and copies for smudge dabs over these numbers of tiles: a pick,
/// a buffer copy, and a copy and a stamp per tile.
fn smudge_cost(tiles: impl Iterator<Item = usize>) -> u32 {
    tiles.map(|n| 2 + 2 * n as u32).sum()
}

/// Consecutive failed frames before WebGPU counts as broken. A single failed
/// frame (e.g. the canvas's first texture right after it's configured) is
/// just drawn again.
const BAD_FRAMES_BEFORE_FAILURE: u32 = 3;

fn premul_blend() -> Object {
    let c = obj! { "srcFactor": "one", "dstFactor": "one-minus-src-alpha", "operation": "add" };
    let a = obj! { "srcFactor": "one", "dstFactor": "one-minus-src-alpha", "operation": "add" };
    obj! { "color": c, "alpha": a }
}

impl GpuBackend {
    pub fn new(device: JsValue, context: JsValue, format: String, on_error: js_sys::Function, paint_cap: u32) -> Result<GpuBackend, String> {
        let device: GpuDevice = device.unchecked_into();
        let context: GpuCanvasContext = context.unchecked_into();
        let queue = device.queue();
        let module = device.create_shader_module(&obj! { "label": "flexma", "code": SHADERS });
        let post_module = device.create_shader_module(&obj! { "label": "flexma-post", "code": POST_SHADERS });
        let blur_module = device.create_shader_module(&obj! { "label": "flexma-blur", "code": BLUR_SHADERS });
        let dab_module = device.create_shader_module(&obj! { "label": "flexma-dab", "code": format!("{BRUSH_SHADERS}\n{DAB_SHADERS}") });
        let smudge_module = device.create_shader_module(&obj! { "label": "flexma-smudge", "code": format!("{BRUSH_SHADERS}\n{SMUDGE_SHADERS}") });
        // Brush bindings: style uniform, clamped and repeating samplers, then
        // the tip, grain and tint textures; smudge adds the window and buffer.
        let brush_entries = |uniform: JsValue, extra: u32| {
            let mut v: Vec<JsValue> = vec![
                obj! { "binding": 0u32, "visibility": STAGE_VERTEX | STAGE_FRAGMENT, "buffer": uniform }.into(),
                obj! { "binding": 1u32, "visibility": STAGE_FRAGMENT, "sampler": obj! { "type": "filtering" } }.into(),
                obj! { "binding": 2u32, "visibility": STAGE_FRAGMENT, "sampler": obj! { "type": "filtering" } }.into(),
            ];
            for b in 3..6 + extra {
                v.push(obj! { "binding": b, "visibility": STAGE_FRAGMENT, "texture": obj! { "sampleType": "float" } }.into());
            }
            arr(v)
        };
        let dab_bgl = device.create_bind_group_layout(&obj! {
            "entries": brush_entries(obj! { "type": "uniform", "hasDynamicOffset": true, "minBindingSize": STYLE_BYTES }.into(), 0),
        });
        let smudge_bgl = device.create_bind_group_layout(&obj! {
            "entries": brush_entries(obj! { "type": "uniform", "hasDynamicOffset": true, "minBindingSize": SMUDGE_BYTES }.into(), 2),
        });
        let dab_layout = device.create_pipeline_layout(&obj! { "bindGroupLayouts": arr(vec![dab_bgl.clone()]) });
        let smudge_layout = device.create_pipeline_layout(&obj! { "bindGroupLayouts": arr(vec![smudge_bgl.clone()]) });
        let repeat_sampler = device.create_sampler(&obj! {
            "magFilter": "linear",
            "minFilter": "linear",
            "mipmapFilter": "linear",
            "addressModeU": "repeat",
            "addressModeV": "repeat",
        });
        let uniform_bgl = device.create_bind_group_layout(&obj! {
            "entries": arr(vec![obj! {
                "binding": 0u32,
                "visibility": STAGE_VERTEX | STAGE_FRAGMENT,
                "buffer": obj! { "type": "uniform", "hasDynamicOffset": true, "minBindingSize": UNIFORM_SIZE },
            }.into()]),
        });
        let tex_bgl = device.create_bind_group_layout(&obj! {
            "entries": arr(vec![
                obj! { "binding": 0u32, "visibility": STAGE_FRAGMENT, "sampler": obj! { "type": "filtering" } }.into(),
                obj! { "binding": 1u32, "visibility": STAGE_FRAGMENT, "texture": obj! { "sampleType": "float" } }.into(),
            ]),
        });
        let main_layout = device.create_pipeline_layout(&obj! { "bindGroupLayouts": arr(vec![uniform_bgl.clone(), tex_bgl.clone()]) });
        let mesh_layout = device.create_pipeline_layout(&obj! { "bindGroupLayouts": arr(vec![uniform_bgl.clone()]) });
        let post_layout = device.create_pipeline_layout(&obj! { "bindGroupLayouts": arr(vec![tex_bgl.clone()]) });
        let sampler = device.create_sampler(&obj! {
            "magFilter": "linear",
            "minFilter": "linear",
            "mipmapFilter": "linear",
            "addressModeU": "clamp-to-edge",
            "addressModeV": "clamp-to-edge",
        });
        let max_tex = Reflect::get(&device.limits(), &"maxTextureDimension2D".into())
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(8192.0)
            .min(8192.0) as u32;
        // A 1x1 white texture for untextured shapes.
        let tex = device.create_texture(&obj! {
            "label": "white",
            "size": arr(vec![1u32.into(), 1u32.into()]),
            "format": TEX_FORMAT,
            "usage": TEX_BINDING | TEX_COPY_DST,
        });
        queue.write_texture(
            &obj! { "texture": tex.clone() },
            &[255, 255, 255, 255],
            &obj! { "bytesPerRow": 4u32 },
            &arr(vec![1u32.into(), 1u32.into()]),
        );
        let bind = device.create_bind_group(&obj! {
            "layout": tex_bgl.clone(),
            "entries": arr(vec![
                obj! { "binding": 0u32, "resource": sampler.clone() }.into(),
                obj! { "binding": 1u32, "resource": tex.create_view() }.into(),
            ]),
        });
        let white = TexEntry { tex, bind, w: 1, h: 1, mips: 1 };
        let paint_inflight = Rc::new(Cell::new(0u32));
        let paint_costs: Rc<RefCell<VecDeque<u32>>> = Rc::default();
        let paint_done = {
            let (inflight, costs) = (paint_inflight.clone(), paint_costs.clone());
            Closure::new(move |_: JsValue| {
                if let Some(n) = costs.borrow_mut().pop_front() {
                    inflight.set(inflight.get().saturating_sub(n));
                }
            })
        };
        Ok(GpuBackend {
            device,
            queue,
            context,
            format,
            module,
            post_module,
            blur_module,
            backdrop_src: None,
            backdrop_small: HashMap::new(),
            dab_module,
            dab_bgl,
            dab_layout,
            dab_inst: None,
            dab_uniforms: None,
            smudge_module,
            smudge_bgl,
            smudge_layout,
            smudge_uniform: None,
            smudge_window: None,
            smudge_scratch: None,
            smudge_blank: None,
            repeat_sampler,
            paint_passes: 0,
            paint_submits: 0,
            uniform_bgl,
            tex_bgl,
            main_layout,
            mesh_layout,
            post_layout,
            sampler,
            pipelines: HashMap::new(),
            white,
            textures: Vec::new(),
            meshes: Vec::new(),
            scene_inst: None,
            overlay_inst: None,
            uniforms: None,
            target: None,
            configured: (0, 0),
            max_tex,
            frame: None,
            scene_ops: Vec::new(),
            post_ops: Vec::new(),
            in_scene: false,
            udata: Vec::new(),
            scene_slot: 0,
            depth: 0,
            draw_calls: 0,
            trash_tex: Vec::new(),
            trash_buf: Vec::new(),
            bad_frames: Rc::new(Cell::new(0)),
            errors: Rc::new(Cell::new(0)),
            on_error,
            wake: Rc::new(Cell::new(false)),
            frame_check: None,
            paint_cap: paint_cap.max(1),
            paint_queue: VecDeque::new(),
            paint_inflight,
            paint_costs,
            paint_done,
            paint_queue_peak: 0,
        })
    }

    fn pipeline(&mut self, p: Pipe) -> JsValue {
        if let Some(pl) = self.pipelines.get(&p) {
            return pl.clone();
        }
        let shape_buffers = || {
            let attrs: Vec<JsValue> = (0..13u32)
                .map(|i| obj! { "shaderLocation": i, "offset": i * 16, "format": "float32x4" }.into())
                .collect();
            arr(vec![obj! { "arrayStride": (INST_FLOATS * 4) as u32, "stepMode": "instance", "attributes": arr(attrs) }.into()])
        };
        let mesh_buffers = || {
            arr(vec![obj! {
                "arrayStride": 8u32,
                "stepMode": "vertex",
                "attributes": arr(vec![obj! { "shaderLocation": 0u32, "offset": 0u32, "format": "float32x2" }.into()]),
            }
            .into()])
        };
        let scene_target = |linear: bool, write: u32| -> JsValue {
            obj! { "format": if linear { LINEAR_FORMAT } else { self.format.as_str() }, "blend": premul_blend(), "writeMask": write }.into()
        };
        let stencil = |s: Stencil| -> Object {
            let pass = match s {
                Stencil::Test => "keep",
                Stencil::Incr => "increment-clamp",
                Stencil::Decr => "decrement-clamp",
            };
            let face = || obj! { "compare": "equal", "failOp": "keep", "depthFailOp": "keep", "passOp": pass };
            obj! {
                "format": DEPTH_FORMAT,
                "depthWriteEnabled": false,
                "depthCompare": "always",
                "stencilFront": face(),
                "stencilBack": face(),
                "stencilReadMask": 0xffu32,
                "stencilWriteMask": if s == Stencil::Test { 0u32 } else { 0xffu32 },
            }
        };
        let ms = || obj! { "count": SAMPLES };
        let desc = match p {
            Pipe::Shape(s, linear) => obj! {
                "label": format!("shape {s:?} {linear}"),
                "layout": self.main_layout.clone(),
                "vertex": obj! { "module": self.module.clone(), "entryPoint": "vs_shape", "buffers": shape_buffers() },
                "fragment": obj! {
                    "module": self.module.clone(),
                    "entryPoint": "fs_shape",
                    "targets": arr(vec![scene_target(linear, if s == Stencil::Test { WRITE_ALL } else { 0 })]),
                },
                "primitive": obj! { "topology": "triangle-strip" },
                "depthStencil": stencil(s),
                "multisample": ms(),
            },
            Pipe::Mesh(linear) => obj! {
                "label": "mesh",
                "layout": self.mesh_layout.clone(),
                "vertex": obj! { "module": self.module.clone(), "entryPoint": "vs_mesh", "buffers": mesh_buffers() },
                "fragment": obj! { "module": self.module.clone(), "entryPoint": "fs_mesh", "targets": arr(vec![scene_target(linear, WRITE_ALL)]) },
                "primitive": obj! { "topology": "triangle-list" },
                "depthStencil": stencil(Stencil::Test),
                "multisample": ms(),
            },
            Pipe::Tex(linear) => obj! {
                "label": "tex",
                "layout": self.main_layout.clone(),
                "vertex": obj! { "module": self.module.clone(), "entryPoint": "vs_tex", "buffers": arr(vec![]) },
                "fragment": obj! { "module": self.module.clone(), "entryPoint": "fs_tex", "targets": arr(vec![scene_target(linear, WRITE_ALL)]) },
                "primitive": obj! { "topology": "triangle-strip" },
                "depthStencil": stencil(Stencil::Test),
                "multisample": ms(),
            },
            Pipe::Overlay => obj! {
                "label": "overlay",
                "layout": self.main_layout.clone(),
                "vertex": obj! { "module": self.module.clone(), "entryPoint": "vs_shape", "buffers": shape_buffers() },
                "fragment": obj! {
                    "module": self.module.clone(),
                    "entryPoint": "fs_shape",
                    "targets": arr(vec![obj! { "format": self.format.as_str(), "blend": premul_blend() }.into()]),
                },
                "primitive": obj! { "topology": "triangle-strip" },
            },
            Pipe::Label => obj! {
                "label": "label",
                "layout": self.main_layout.clone(),
                "vertex": obj! { "module": self.module.clone(), "entryPoint": "vs_tex", "buffers": arr(vec![]) },
                "fragment": obj! {
                    "module": self.module.clone(),
                    "entryPoint": "fs_tex",
                    "targets": arr(vec![obj! { "format": self.format.as_str(), "blend": premul_blend() }.into()]),
                },
                "primitive": obj! { "topology": "triangle-strip" },
            },
            Pipe::Composite => obj! {
                "label": "composite",
                "layout": self.post_layout.clone(),
                "vertex": obj! { "module": self.post_module.clone(), "entryPoint": "vs_full", "buffers": arr(vec![]) },
                "fragment": obj! {
                    "module": self.post_module.clone(),
                    "entryPoint": "fs_composite",
                    "targets": arr(vec![obj! { "format": self.format.as_str() }.into()]),
                },
                "primitive": obj! { "topology": "triangle-list" },
            },
            Pipe::Dab(mode) => {
                let (src, dst) = match mode {
                    PaintMode::Paint => ("one", "one-minus-src-alpha"),
                    PaintMode::Erase => ("zero", "one-minus-src-alpha"),
                    PaintMode::AlphaLock => ("dst-alpha", "one-minus-src-alpha"),
                };
                let comp = || obj! { "srcFactor": src, "dstFactor": dst, "operation": "add" };
                obj! {
                    "label": format!("dab {mode:?}"),
                    "layout": self.dab_layout.clone(),
                    "vertex": obj! {
                        "module": self.dab_module.clone(),
                        "entryPoint": "vs_dab",
                        "buffers": arr(vec![obj! {
                            "arrayStride": (DAB_FLOATS * 4) as u32,
                            "stepMode": "instance",
                            "attributes": arr(vec![
                                obj! { "shaderLocation": 0u32, "offset": 0u32, "format": "float32x4" }.into(),
                                obj! { "shaderLocation": 1u32, "offset": 16u32, "format": "float32x4" }.into(),
                                obj! { "shaderLocation": 2u32, "offset": 32u32, "format": "float32x4" }.into(),
                            ]),
                        }.into()]),
                    },
                    "fragment": obj! {
                        "module": self.dab_module.clone(),
                        "entryPoint": "fs_dab",
                        "targets": arr(vec![obj! { "format": TEX_FORMAT, "blend": obj! { "color": comp(), "alpha": comp() } }.into()]),
                    },
                    "primitive": obj! { "topology": "triangle-strip" },
                }
            }
            Pipe::SmudgePick | Pipe::SmudgeStamp => {
                let (vs, fs) = if p == Pipe::SmudgePick { ("vs_full", "fs_pick") } else { ("vs_stamp", "fs_stamp") };
                obj! {
                    "label": fs,
                    "layout": self.smudge_layout.clone(),
                    "vertex": obj! { "module": self.smudge_module.clone(), "entryPoint": vs, "buffers": arr(vec![]) },
                    "fragment": obj! {
                        "module": self.smudge_module.clone(),
                        "entryPoint": fs,
                        "targets": arr(vec![obj! { "format": TEX_FORMAT }.into()]),
                    },
                    "primitive": obj! { "topology": if p == Pipe::SmudgePick { "triangle-list" } else { "triangle-strip" } },
                }
            }
            Pipe::Blur(linear) => obj! {
                "label": "backdrop blur",
                "layout": self.main_layout.clone(),
                "vertex": obj! { "module": self.blur_module.clone(), "entryPoint": "vs_full", "buffers": arr(vec![]) },
                "fragment": obj! {
                    "module": self.blur_module.clone(),
                    "entryPoint": "fs_blur",
                    "targets": arr(vec![obj! { "format": if linear { LINEAR_FORMAT } else { self.format.as_str() } }.into()]),
                },
                "primitive": obj! { "topology": "triangle-list" },
            },
            Pipe::Backdrop(linear) => obj! {
                "label": "backdrop",
                "layout": self.post_layout.clone(),
                "vertex": obj! { "module": self.post_module.clone(), "entryPoint": "vs_full", "buffers": arr(vec![]) },
                "fragment": obj! { "module": self.post_module.clone(), "entryPoint": "fs_blit", "targets": arr(vec![scene_target(linear, WRITE_ALL)]) },
                "primitive": obj! { "topology": "triangle-list" },
                "depthStencil": stencil(Stencil::Test),
                "multisample": ms(),
            },
            Pipe::Blit => obj! {
                "label": "mip blit",
                "layout": self.post_layout.clone(),
                "vertex": obj! { "module": self.post_module.clone(), "entryPoint": "vs_full", "buffers": arr(vec![]) },
                "fragment": obj! {
                    "module": self.post_module.clone(),
                    "entryPoint": "fs_blit",
                    "targets": arr(vec![obj! { "format": TEX_FORMAT }.into()]),
                },
                "primitive": obj! { "topology": "triangle-list" },
            },
        };
        let pl = self.device.create_render_pipeline(&desc);
        self.pipelines.insert(p, pl.clone());
        pl
    }

    fn tex_bind_group(&self, view: JsValue) -> JsValue {
        self.device.create_bind_group(&obj! {
            "layout": self.tex_bgl.clone(),
            "entries": arr(vec![
                obj! { "binding": 0u32, "resource": self.sampler.clone() }.into(),
                obj! { "binding": 1u32, "resource": view }.into(),
            ]),
        })
    }

    /// A view of a texture for a brush binding (white when none).
    fn brush_view(&self, t: Option<TexId>) -> JsValue {
        self.tex_entry(t).tex.create_view()
    }

    /// Bind group entries 1–5: samplers and the tip, grain and tint textures.
    fn brush_bindings(&self, st: &DabStyle) -> Vec<JsValue> {
        vec![
            obj! { "binding": 1u32, "resource": self.sampler.clone() }.into(),
            obj! { "binding": 2u32, "resource": self.repeat_sampler.clone() }.into(),
            obj! { "binding": 3u32, "resource": self.brush_view(st.tip) }.into(),
            obj! { "binding": 4u32, "resource": self.brush_view(st.grain) }.into(),
            obj! { "binding": 5u32, "resource": self.brush_view(st.tint) }.into(),
        ]
    }

    fn tex_entry(&self, t: Option<TexId>) -> &TexEntry {
        t.and_then(|t| self.textures.get(t.0 as usize)).and_then(Option::as_ref).unwrap_or(&self.white)
    }

    /// Fill each mip level from the one above with a linear downsample.
    fn generate_mips(&mut self, tex: &GpuTexture, levels: u32) {
        if levels <= 1 {
            return;
        }
        let pipe = self.pipeline(Pipe::Blit);
        let enc = self.device.create_command_encoder();
        for level in 1..levels {
            let src = tex.create_view_with(&obj! { "baseMipLevel": level - 1, "mipLevelCount": 1u32 });
            let dst = tex.create_view_with(&obj! { "baseMipLevel": level, "mipLevelCount": 1u32 });
            let bind = self.tex_bind_group(src);
            let pass = enc.begin_render_pass(&obj! {
                "colorAttachments": arr(vec![obj! { "view": dst, "loadOp": "clear", "storeOp": "store",
                    "clearValue": arr(vec![0u32.into(), 0u32.into(), 0u32.into(), 0u32.into()]) }.into()]),
            });
            pass.set_pipeline(&pipe);
            pass.set_bind_group(0, &bind);
            pass.draw(3, 1, 0, 0);
            pass.end();
        }
        self.queue.submit(&arr(vec![enc.finish()]));
    }

    /// Grow-only vertex buffer for instances.
    fn write_instances(device: &GpuDevice, queue: &GpuQueue, slot: &mut Option<Grow>, trash: &mut Vec<GpuBuffer>, data: &[f32]) {
        let bytes = (data.len() * 4) as u64;
        if bytes == 0 {
            return;
        }
        if slot.as_ref().is_none_or(|g| g.cap < bytes) {
            let cap = (bytes + bytes / 2).max(64 * 1024);
            if let Some(old) = slot.take() {
                trash.push(old.buf);
            }
            let buf = device.create_buffer(&obj! { "size": cap as f64, "usage": BUF_VERTEX | BUF_COPY_DST });
            *slot = Some(Grow { buf, cap });
        }
        queue.write_f32(&slot.as_ref().unwrap().buf, 0.0, data);
    }

    /// Write uniform data into a growable buffer; returns the buffer.
    fn write_uniforms(device: &GpuDevice, queue: &GpuQueue, slot: &mut Option<Grow>, trash: &mut Vec<GpuBuffer>, data: &[f32]) -> GpuBuffer {
        let bytes = (data.len() * 4) as u64;
        if slot.as_ref().is_none_or(|g| g.cap < bytes) {
            let cap = (bytes + bytes / 2).max(64 * 1024);
            if let Some(old) = slot.take() {
                trash.push(old.buf);
            }
            let buf = device.create_buffer(&obj! { "size": cap as f64, "usage": BUF_UNIFORM | BUF_COPY_DST });
            *slot = Some(Grow { buf, cap });
        }
        let buf = slot.as_ref().unwrap().buf.clone();
        queue.write_f32(&buf, 0.0, data);
        buf
    }

    /// Append a uniform slot: view (3 columns), affine rows, a vec4 and params.
    fn push_uniforms(&mut self, view: &[f32; 9], m: Rows, v: [f32; 4], p: [f32; 4], uv: [f32; 4]) -> u32 {
        let slot = (self.udata.len() / SLOT_FLOATS) as u32;
        let mut s = [0.0f32; SLOT_FLOATS];
        s[0..3].copy_from_slice(&view[0..3]);
        s[4..7].copy_from_slice(&view[3..6]);
        s[8..11].copy_from_slice(&view[6..9]);
        s[12..15].copy_from_slice(&m.0);
        s[16..19].copy_from_slice(&m.1);
        s[20..24].copy_from_slice(&v);
        s[24..28].copy_from_slice(&p);
        s[28..32].copy_from_slice(&uv);
        self.udata.extend_from_slice(&s);
        slot
    }

    fn ensure_target(&mut self, w: u32, h: u32, linear: bool) {
        if self.target.as_ref().is_some_and(|t| t.w == w && t.h == h && t.linear == linear) {
            return;
        }
        if let Some(old) = self.target.take() {
            self.trash_tex.push(old.msaa);
            self.trash_tex.push(old.depth);
            if let Some((t, _, _)) = old.resolve {
                self.trash_tex.push(t);
            }
        }
        let size = || arr(vec![w.into(), h.into()]);
        let color_format = if linear { LINEAR_FORMAT } else { self.format.as_str() };
        let msaa = self.device.create_texture(&obj! { "label": "msaa", "size": size(), "format": color_format, "sampleCount": SAMPLES, "usage": TEX_RENDER });
        let depth = self.device.create_texture(&obj! { "label": "depth", "size": size(), "format": DEPTH_FORMAT, "sampleCount": SAMPLES, "usage": TEX_RENDER });
        let resolve = linear.then(|| {
            let t = self.device.create_texture(&obj! { "label": "linear resolve", "size": size(), "format": LINEAR_FORMAT, "usage": TEX_RENDER | TEX_BINDING });
            let view = t.create_view();
            let bind = self.tex_bind_group(view.clone());
            (t, view, bind)
        });
        let msaa_view = msaa.create_view();
        let depth_view = depth.create_view();
        self.target = Some(Target { w, h, linear, msaa, msaa_view, depth, depth_view, resolve });
    }

    fn replay(&mut self, pass: &GpuRenderPass, ops: &[Op], scene: bool, linear: bool) {
        let ubind = self.uniforms.as_ref().map(|(_, b)| b.clone()).unwrap();
        let mut current: Option<Pipe> = None;
        let mut bound_tex: Option<Option<TexId>> = None;
        let mut bound_inst: Option<u8> = None;
        for op in ops {
            match op {
                Op::StencilRef(r) => pass.set_stencil_reference(*r),
                Op::Shapes { inst, first, count, tex, stencil, slot } => {
                    let pipe = if scene { Pipe::Shape(*stencil, linear) } else { Pipe::Overlay };
                    if current != Some(pipe) {
                        let pl = self.pipeline(pipe);
                        pass.set_pipeline(&pl);
                        current = Some(pipe);
                        bound_tex = None;
                        bound_inst = None;
                    }
                    pass.set_bind_group_offsets(0, &ubind, &[slot * (SLOT_FLOATS as u32) * 4]);
                    if bound_tex != Some(*tex) {
                        pass.set_bind_group(1, &self.tex_entry(*tex).bind);
                        bound_tex = Some(*tex);
                    }
                    let which = match inst {
                        Inst::Scene => 0u8,
                        Inst::Overlay => 1u8,
                    };
                    if bound_inst != Some(which) {
                        let buf = match inst {
                            Inst::Scene => self.scene_inst.as_ref(),
                            Inst::Overlay => self.overlay_inst.as_ref(),
                        };
                        let Some(g) = buf else { continue };
                        pass.set_vertex_buffer(0, &g.buf);
                        bound_inst = Some(which);
                    }
                    pass.draw(4, *count, 0, *first);
                }
                Op::Mesh { mesh, slot } => {
                    let Some(Some((vb, ib, count))) = self.meshes.get(mesh.0 as usize) else { continue };
                    let (vb, ib, count) = (vb.clone(), ib.clone(), *count);
                    let pipe = Pipe::Mesh(linear);
                    if current != Some(pipe) {
                        let pl = self.pipeline(pipe);
                        pass.set_pipeline(&pl);
                        current = Some(pipe);
                        bound_tex = None;
                        bound_inst = None;
                    }
                    pass.set_bind_group_offsets(0, &ubind, &[slot * (SLOT_FLOATS as u32) * 4]);
                    pass.set_vertex_buffer(0, &vb);
                    pass.set_index_buffer(&ib, "uint32");
                    pass.draw_indexed(count, 1, 0, 0, 0);
                }
                Op::Tex { tex, slot, label } => {
                    let pipe = if *label { Pipe::Label } else { Pipe::Tex(linear) };
                    if current != Some(pipe) {
                        let pl = self.pipeline(pipe);
                        pass.set_pipeline(&pl);
                        current = Some(pipe);
                        bound_tex = None;
                        bound_inst = None;
                    }
                    pass.set_bind_group_offsets(0, &ubind, &[slot * (SLOT_FLOATS as u32) * 4]);
                    if bound_tex != Some(Some(*tex)) {
                        pass.set_bind_group(1, &self.tex_entry(Some(*tex)).bind);
                        bound_tex = Some(Some(*tex));
                    }
                    pass.draw(4, 1, 0, 0);
                }
                Op::Backdrop { .. } => {}
                Op::BackdropQuad(bind) => {
                    let pl = self.pipeline(Pipe::Backdrop(linear));
                    pass.set_pipeline(&pl);
                    current = Some(Pipe::Backdrop(linear));
                    bound_tex = None;
                    bound_inst = None;
                    pass.set_bind_group(0, bind);
                    pass.draw(3, 1, 0, 0);
                }
                Op::Composite => {
                    let Some(bind) = self.target.as_ref().and_then(|t| t.resolve.as_ref()).map(|r| r.2.clone()) else { continue };
                    let pl = self.pipeline(Pipe::Composite);
                    pass.set_pipeline(&pl);
                    current = Some(Pipe::Composite);
                    bound_tex = None;
                    bound_inst = None;
                    pass.set_bind_group(0, &bind);
                    pass.draw(3, 1, 0, 0);
                }
            }
        }
    }

    /// A texture in the scene's format to render into and sample from.
    fn render_tex(&self, label: &str, w: u32, h: u32, linear: bool) -> RenderTex {
        let tex = self.device.create_texture(&obj! {
            "label": label,
            "size": arr(vec![w.max(1).into(), h.max(1).into()]),
            "format": if linear { LINEAR_FORMAT } else { self.format.as_str() },
            "usage": TEX_RENDER | TEX_BINDING,
        });
        let view = tex.create_view();
        let bind = self.tex_bind_group(view.clone());
        RenderTex { tex, view, bind }
    }

    /// The scene with background blurs: a pass up to each one, resolved into
    /// `backdrop_src`, blurred across then down into small textures, and the
    /// next pass (loading what the last one stored) starts by drawing the
    /// blurred copy inside the layer's shape.
    #[allow(clippy::too_many_arguments)]
    fn scene_with_backdrops(&mut self, enc: &GpuCommandEncoder, ops: &[Op], msaa: JsValue, depth: JsValue, resolve: JsValue, bg: flexma_doc::Color, size: (u32, u32), linear: bool) {
        let (w, h) = size;
        if !self.backdrop_src.as_ref().is_some_and(|(sw, sh, l, _)| (*sw, *sh, *l) == (w, h, linear)) {
            if let Some((_, _, _, old)) = self.backdrop_src.take() {
                self.trash_tex.push(old.tex);
            }
            let t = self.render_tex("backdrop", w, h, linear);
            self.backdrop_src = Some((w, h, linear, t));
        }
        let Some(ubind) = self.uniforms.as_ref().map(|(_, b)| b.clone()) else { return };
        let (src_view, src_bind) = self.backdrop_src.as_ref().map(|(_, _, _, t)| (t.view.clone(), t.bind.clone())).unwrap();
        let mut start = 0;
        let mut prev: Option<(Vec<(u32, Option<TexId>)>, u32, JsValue)> = None;
        loop {
            let end = ops[start..].iter().position(|o| matches!(o, Op::Backdrop { .. })).map(|p| start + p);
            let (first, last) = (start == 0, end.is_none());
            let load = |first: bool| if first { "clear" } else { "load" };
            let store = |last: bool| if last { "discard" } else { "store" };
            let pass = enc.begin_render_pass(&obj! {
                "colorAttachments": arr(vec![obj! {
                    "view": msaa.clone(),
                    "resolveTarget": if last { resolve.clone() } else { src_view.clone() },
                    "clearValue": arr(vec![(bg.r as f64).into(), (bg.g as f64).into(), (bg.b as f64).into(), 1.0f64.into()]),
                    "loadOp": load(first),
                    "storeOp": store(last),
                }.into()]),
                "depthStencilAttachment": obj! {
                    "view": depth.clone(),
                    "depthClearValue": 1.0f64,
                    "depthLoadOp": load(first),
                    "depthStoreOp": store(last),
                    "stencilClearValue": 0u32,
                    "stencilLoadOp": load(first),
                    "stencilStoreOp": store(last),
                },
            });
            pass.set_stencil_reference(prev.as_ref().map(|p| p.1).unwrap_or(0));
            if let Some((parts, d, blurred)) = prev.take() {
                let slot = self.scene_slot;
                let clip = |stencil| parts.iter().map(move |(inst, tex)| Op::Shapes { inst: Inst::Scene, first: *inst, count: 1, tex: *tex, stencil, slot });
                let mut copy: Vec<Op> = clip(Stencil::Incr).collect();
                copy.push(Op::StencilRef(d + 1));
                copy.push(Op::BackdropQuad(blurred));
                copy.extend(clip(Stencil::Decr));
                copy.push(Op::StencilRef(d));
                self.replay(&pass, &copy, true, linear);
            }
            self.replay(&pass, &ops[start..end.unwrap_or(ops.len())], true, linear);
            pass.end();
            let Some(e) = end else { break };
            let Op::Backdrop { parts, depth: d, slots, down } = &ops[e] else { break };
            let key = (w.div_ceil(*down), h.div_ceil(*down), linear);
            if !self.backdrop_small.contains_key(&key) {
                if self.backdrop_small.len() >= 4 {
                    for (_, (a, b)) in self.backdrop_small.drain() {
                        self.trash_tex.push(a.tex);
                        self.trash_tex.push(b.tex);
                    }
                }
                let pair = (self.render_tex("backdrop blur a", key.0, key.1, linear), self.render_tex("backdrop blur b", key.0, key.1, linear));
                self.backdrop_small.insert(key, pair);
            }
            let (a, b) = &self.backdrop_small[&key];
            let steps = [(a.view.clone(), src_bind.clone(), slots[0]), (b.view.clone(), a.bind.clone(), slots[1])];
            let blurred = b.bind.clone();
            let pl = self.pipeline(Pipe::Blur(linear));
            for (target, input, slot) in steps {
                let bp = enc.begin_render_pass(&obj! {
                    "colorAttachments": arr(vec![obj! {
                        "view": target,
                        "clearValue": arr(vec![0u32.into(), 0u32.into(), 0u32.into(), 0u32.into()]),
                        "loadOp": "clear",
                        "storeOp": "store",
                    }.into()]),
                });
                bp.set_pipeline(&pl);
                bp.set_bind_group_offsets(0, &ubind, &[slot * (SLOT_FLOATS as u32) * 4]);
                bp.set_bind_group(1, &input);
                bp.draw(3, 1, 0, 0);
                bp.end();
            }
            prev = Some((parts.clone(), *d, blurred));
            start = e + 1;
        }
    }

    fn record(&mut self, op: Op) {
        if matches!(op, Op::Shapes { .. } | Op::Mesh { .. } | Op::Tex { .. } | Op::Composite) {
            self.draw_calls += 1;
        }
        if self.in_scene {
            self.scene_ops.push(op);
        } else {
            self.post_ops.push(op);
        }
    }
}

impl Backend for GpuBackend {
    fn name(&self) -> &'static str {
        "webgpu"
    }

    fn max_texture_size(&self) -> u32 {
        self.max_tex
    }

    fn upload_texture(&mut self, reuse: Option<TexId>, src: ImageSource, w: u32, h: u32, mipmaps: bool) -> Option<TexId> {
        let (w, h) = (w.max(1), h.max(1));
        let mips = if mipmaps { 32 - w.max(h).leading_zeros() } else { 1 };
        let existing = reuse.and_then(|t| self.textures.get(t.0 as usize)).and_then(Option::as_ref);
        let fits = existing.is_some_and(|e| e.w == w && e.h == h && e.mips == mips);
        let tex = if fits {
            existing.unwrap().tex.clone()
        } else {
            self.device.create_texture(&obj! {
                "label": format!("upload {w}x{h} mips {mips}"),
                "size": arr(vec![w.into(), h.into()]),
                "format": TEX_FORMAT,
                "mipLevelCount": mips,
                "usage": TEX_BINDING | TEX_COPY_DST | TEX_RENDER,
            })
        };
        let source: JsValue = match src {
            ImageSource::Image(i) => i.clone().into(),
            ImageSource::Canvas(c) => c.clone().into(),
        };
        let copied = self.queue.copy_external_image_to_texture(
            &obj! { "source": source },
            &obj! { "texture": tex.clone(), "premultipliedAlpha": true },
            &arr(vec![w.into(), h.into()]),
        );
        if copied.is_err() {
            if !fits {
                tex.destroy_texture();
            }
            return None;
        }
        self.generate_mips(&tex, mips);
        if fits {
            return reuse;
        }
        let bind = self.tex_bind_group(tex.create_view());
        let entry = TexEntry { tex, bind, w, h, mips };
        match reuse.filter(|t| self.textures.get(t.0 as usize).is_some_and(Option::is_some)) {
            Some(t) => {
                if let Some(old) = self.textures[t.0 as usize].replace(entry) {
                    self.trash_tex.push(old.tex);
                }
                Some(t)
            }
            None => match self.textures.iter().position(Option::is_none) {
                Some(i) => {
                    self.textures[i] = Some(entry);
                    Some(TexId(i as u32))
                }
                None => {
                    self.textures.push(Some(entry));
                    Some(TexId((self.textures.len() - 1) as u32))
                }
            },
        }
    }


    fn upload_brush_texture(&mut self, src: ImageSource, w: u32, h: u32, _repeat: bool) -> Option<TexId> {
        // Repeating is the sampler's job here (grain and tint bind `repeat_sampler`).
        self.upload_texture(None, src, w, h, true)
    }

    fn create_mesh(&mut self, vertices: &[f32], indices: &[u32]) -> Option<MeshId> {
        if indices.is_empty() || vertices.is_empty() {
            return None;
        }
        let vb = self.device.create_buffer(&obj! { "size": (vertices.len() * 4) as f64, "usage": BUF_VERTEX | BUF_COPY_DST });
        let ib = self.device.create_buffer(&obj! { "size": (indices.len() * 4) as f64, "usage": BUF_INDEX | BUF_COPY_DST });
        self.queue.write_f32(&vb, 0.0, vertices);
        self.queue.write_u32(&ib, 0.0, indices);
        let item = (vb, ib, indices.len() as u32);
        Some(MeshId(match self.meshes.iter().position(Option::is_none) {
            Some(i) => {
                self.meshes[i] = Some(item);
                i as u32
            }
            None => {
                self.meshes.push(Some(item));
                (self.meshes.len() - 1) as u32
            }
        }))
    }

    fn delete_mesh(&mut self, m: MeshId) {
        if let Some(Some((vb, ib, _))) = self.meshes.get_mut(m.0 as usize).map(Option::take) {
            self.trash_buf.push(vb);
            self.trash_buf.push(ib);
        }
    }

    fn set_scene_instances(&mut self, data: &[f32]) {
        Self::write_instances(&self.device, &self.queue, &mut self.scene_inst, &mut self.trash_buf, data);
    }

    fn begin_frame(&mut self, p: &FrameParams) {
        self.scene_ops.clear();
        self.post_ops.clear();
        self.udata.clear();
        self.draw_calls = 0;
        self.depth = 0;
        self.in_scene = true;
        // Slot 0: the scene's shapes (view, AA width, linear output).
        self.scene_slot = self.push_uniforms(&p.view, ([0.0; 3], [0.0; 3]), [0.0; 4], [p.px, if p.linear { 1.0 } else { 0.0 }, 1.0, 0.0], [0.0; 4]);
        self.frame = Some(FrameParams {
            width: p.width,
            height: p.height,
            background: p.background,
            linear: p.linear,
            view: p.view,
            screen: p.screen,
            px: p.px,
        });
    }

    fn draw_shapes(&mut self, first: u32, count: u32, tex: Option<TexId>) {
        if count == 0 {
            return;
        }
        let slot = self.scene_slot;
        self.record(Op::Shapes { inst: Inst::Scene, first, count, tex, stencil: Stencil::Test, slot });
    }

    fn clip(&mut self, parts: &[(u32, Option<TexId>)], push: bool) {
        let slot = self.scene_slot;
        let stencil = if push { Stencil::Incr } else { Stencil::Decr };
        for (inst, tex) in parts {
            self.record(Op::Shapes { inst: Inst::Scene, first: *inst, count: 1, tex: *tex, stencil, slot });
        }
        self.depth = if push { self.depth + 1 } else { self.depth.saturating_sub(1) };
        self.record(Op::StencilRef(self.depth));
    }

    fn backdrop_blur(&mut self, parts: &[(u32, Option<TexId>)], sigma: f32) {
        let Some(f) = self.frame.as_ref() else { return };
        if sigma <= 0.0 || parts.is_empty() || f.width == 0 || f.height == 0 || !self.in_scene {
            return;
        }
        // Blur at a fraction of the size, so a few taps cover a big radius.
        let down = ((sigma / 4.0).ceil() as u32).clamp(1, 16);
        let (view, w, sh) = (f.view, f.width, f.height.div_ceil(down));
        let s = sigma / down as f32;
        let across = self.push_uniforms(&view, ([0.0; 3], [0.0; 3]), [down as f32 / w as f32, 0.0, s, 0.0], [0.0; 4], [0.0; 4]);
        let along = self.push_uniforms(&view, ([0.0; 3], [0.0; 3]), [0.0, 1.0 / sh as f32, s, 0.0], [0.0; 4], [0.0; 4]);
        self.draw_calls += 3;
        self.scene_ops.push(Op::Backdrop { parts: parts.to_vec(), depth: self.depth, slots: [across, along], down });
    }

    fn draw_mesh(&mut self, mesh: MeshId, m: Rows, color: [f32; 4]) {
        let Some(f) = self.frame.as_ref() else { return };
        let (view, linear) = (f.view, f.linear);
        let slot = self.push_uniforms(&view, m, color, [0.0, if linear { 1.0 } else { 0.0 }, 1.0, 0.0], [0.0; 4]);
        self.record(Op::Mesh { mesh, slot });
    }

    fn draw_texture(&mut self, tex: TexId, m: Rows, rect: [f32; 4], uv: [f32; 4], alpha: f32) {
        let Some(f) = self.frame.as_ref() else { return };
        let (view, linear) = (f.view, f.linear);
        let slot = self.push_uniforms(&view, m, rect, [0.0, if linear { 1.0 } else { 0.0 }, alpha, 0.0], uv);
        self.record(Op::Tex { tex, slot, label: false });
    }

    fn end_scene(&mut self) {
        self.in_scene = false;
        if self.frame.as_ref().is_some_and(|f| f.linear) {
            self.record(Op::Composite);
        }
    }

    fn draw_overlay(&mut self, data: &[f32]) {
        if data.is_empty() {
            return;
        }
        let Some(f) = self.frame.as_ref() else { return };
        let (view, px) = (f.view, f.px);
        Self::write_instances(&self.device, &self.queue, &mut self.overlay_inst, &mut self.trash_buf, data);
        let slot = self.push_uniforms(&view, ([0.0; 3], [0.0; 3]), [0.0; 4], [px, 0.0, 1.0, 0.0], [0.0; 4]);
        self.record(Op::Shapes { inst: Inst::Overlay, first: 0, count: (data.len() / INST_FLOATS) as u32, tex: None, stencil: Stencil::Test, slot });
    }

    fn draw_label(&mut self, tex: TexId, rect: [f32; 4]) {
        let Some(f) = self.frame.as_ref() else { return };
        let screen = f.screen;
        let slot = self.push_uniforms(&screen, ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), rect, [1.0, 0.0, 1.0, 0.0], [0.0, 0.0, 1.0, 1.0]);
        self.record(Op::Tex { tex, slot, label: true });
    }

    fn end_frame(&mut self) {
        let Some(f) = self.frame.take() else { return };
        if f.width == 0 || f.height == 0 {
            return;
        }
        // Uniforms: one buffer, grown as needed, bound with dynamic offsets.
        let bytes = (self.udata.len() * 4) as u64;
        if self.uniforms.as_ref().is_none_or(|(g, _)| g.cap < bytes) {
            let cap = (bytes + bytes / 2).max(64 * 1024);
            if let Some((old, _)) = self.uniforms.take() {
                self.trash_buf.push(old.buf);
            }
            let buf = self.device.create_buffer(&obj! { "size": cap as f64, "usage": BUF_UNIFORM | BUF_COPY_DST });
            let bind = self.device.create_bind_group(&obj! {
                "layout": self.uniform_bgl.clone(),
                "entries": arr(vec![obj! {
                    "binding": 0u32,
                    "resource": obj! { "buffer": buf.clone(), "offset": 0u32, "size": UNIFORM_SIZE },
                }.into()]),
            });
            self.uniforms = Some((Grow { buf, cap }, bind));
        }
        self.queue.write_f32(&self.uniforms.as_ref().unwrap().0.buf, 0.0, &self.udata);

        self.ensure_target(f.width, f.height, f.linear);
        // The frame's GPU work runs in an error scope: one bad frame is redrawn,
        // only repeated ones count as failure (see BAD_FRAMES_BEFORE_FAILURE).
        self.device.push_error_scope("validation");
        // Reconfigure when the canvas changes size.
        if self.configured != (f.width, f.height) {
            self.context.configure(&obj! { "device": self.device.clone(), "format": self.format.as_str(), "alphaMode": "premultiplied" });
            self.configured = (f.width, f.height);
        }
        let canvas_view = self.context.get_current_texture().create_view();
        let t = self.target.as_ref().unwrap();
        let bg = if f.linear { f.background.to_linear() } else { f.background };
        let resolve_view = match &t.resolve {
            Some((_, view, _)) => view.clone(),
            None => canvas_view.clone(),
        };
        let enc = self.device.create_command_encoder();
        let scene_ops = std::mem::take(&mut self.scene_ops);
        if scene_ops.iter().any(|o| matches!(o, Op::Backdrop { .. })) {
            let (msaa, depth) = (t.msaa_view.clone(), t.depth_view.clone());
            self.scene_with_backdrops(&enc, &scene_ops, msaa, depth, resolve_view, bg, (f.width, f.height), f.linear);
        } else {
        let scene = enc.begin_render_pass(&obj! {
            "colorAttachments": arr(vec![obj! {
                "view": t.msaa_view.clone(),
                "resolveTarget": resolve_view,
                "clearValue": arr(vec![(bg.r as f64).into(), (bg.g as f64).into(), (bg.b as f64).into(), 1.0f64.into()]),
                "loadOp": "clear",
                "storeOp": "discard",
            }.into()]),
            "depthStencilAttachment": obj! {
                "view": t.depth_view.clone(),
                "depthClearValue": 1.0f64,
                "depthLoadOp": "clear",
                "depthStoreOp": "discard",
                "stencilClearValue": 0u32,
                "stencilLoadOp": "clear",
                "stencilStoreOp": "discard",
            },
        });
        scene.set_stencil_reference(0);
        self.replay(&scene, &scene_ops, true, f.linear);
        scene.end();
        }
        let post_ops = std::mem::take(&mut self.post_ops);
        if !post_ops.is_empty() {
            let post = enc.begin_render_pass(&obj! {
                "colorAttachments": arr(vec![obj! {
                    "view": canvas_view,
                    "clearValue": arr(vec![0u32.into(), 0u32.into(), 0u32.into(), 1u32.into()]),
                    "loadOp": if f.linear { "clear" } else { "load" },
                    "storeOp": "store",
                }.into()]),
            });
            self.replay(&post, &post_ops, false, false);
            post.end();
        }
        self.queue.submit(&arr(vec![enc.finish()]));
        if self.frame_check.is_none() {
            let (bad, wake, on_error, errors) = (self.bad_frames.clone(), self.wake.clone(), self.on_error.clone(), self.errors.clone());
            self.frame_check = Some(Closure::new(move |err: JsValue| {
                if err.is_null() || err.is_undefined() {
                    bad.set(0);
                    return;
                }
                let n = bad.get() + 1;
                bad.set(n);
                errors.set(errors.get() + 1);
                wake.set(true);
                if n == BAD_FRAMES_BEFORE_FAILURE {
                    let msg = Reflect::get(&err, &"message".into()).ok().and_then(|m| m.as_string()).unwrap_or_default();
                    let _ = on_error.call1(&JsValue::NULL, &JsValue::from_str(&msg));
                }
            }));
        }
        let _ = self.device.pop_error_scope().then(self.frame_check.as_ref().unwrap());
        self.scene_ops = scene_ops;
        self.post_ops = post_ops;
        for t in self.trash_tex.drain(..) {
            t.destroy_texture();
        }
        for b in self.trash_buf.drain(..) {
            b.destroy();
        }
        // Paint work waiting for the GPU (opening a painting) goes on as it catches up.
        self.pump_paint();
    }

    fn draw_calls(&self) -> u32 {
        self.draw_calls
    }

    fn paint_counters(&self) -> [u64; 3] {
        [self.paint_passes, self.paint_submits, self.paint_queue_peak as u64]
    }

    fn gpu_errors(&self) -> u64 {
        self.errors.get()
    }

    fn attach_wake(&mut self, wake: Rc<Cell<bool>>) {
        self.wake = wake;
    }

    fn delete_texture(&mut self, t: TexId) {
        // Queued paint work may still use it.
        if self.paint_queue.is_empty() {
            self.delete_now(t);
        } else {
            self.queue_paint(PaintOp::Delete(t));
        }
    }

    fn load_paint_texture(&mut self, tex: TexId, src: ImageSource, size: u32) -> bool {
        if self.can_paint_now(1) {
            let ok = self.load_now(tex, src, size);
            self.paint_sent(1);
            return ok;
        }
        if !matches!(self.textures.get(tex.0 as usize), Some(Some(_))) {
            return false;
        }
        // Queued: a load that fails later clears the tile instead, as the
        // caller would have.
        let src = match src {
            ImageSource::Image(i) => QueuedImage::Image(i.clone()),
            ImageSource::Canvas(c) => QueuedImage::Canvas(c.clone()),
        };
        self.queue_paint(PaintOp::Load(tex, src, size));
        true
    }

    fn clear_paint_texture(&mut self, tex: TexId, size: u32) {
        if self.can_paint_now(1) {
            self.clear_now(tex, size);
            self.paint_sent(1);
        } else {
            self.queue_paint(PaintOp::Clear(tex, size));
        }
    }

    fn paint_dabs(&mut self, batches: &[DabBatch]) {
        for chunk in batches.chunks(DAB_BATCHES_PER_OP) {
            let cost = chunk.len() as u32;
            if self.can_paint_now(cost) {
                self.paint_dabs_now(chunk);
                self.paint_sent(cost);
            } else {
                self.queue_paint(PaintOp::Dabs(chunk.iter().map(|b| (b.tex, b.dabs.to_vec(), b.mode, b.style)).collect()));
            }
        }
    }

    fn smudge(&mut self, dabs: &[SmudgeDab]) {
        for chunk in dabs.chunks(SMUDGE_DABS_PER_SUBMIT) {
            let cost = smudge_cost(chunk.iter().map(|d| d.tiles.len()));
            if self.can_paint_now(cost) {
                self.smudge_now(chunk);
                self.paint_sent(cost);
            } else {
                let queued = chunk
                    .iter()
                    .map(|d| QueuedSmudge { tiles: d.tiles.to_vec(), dab: d.dab, style: d.style, buffer: d.buffer, first: d.first, keep: d.keep, paint: d.paint, mix: d.mix, alpha_lock: d.alpha_lock })
                    .collect();
                self.queue_paint(PaintOp::Smudge(queued));
            }
        }
    }

    fn copy_paint_texture(&mut self, src: TexId, dst: TexId, size: u32) {
        if self.can_paint_now(1) {
            self.copy_now(src, dst, size);
            self.paint_sent(1);
        } else {
            self.queue_paint(PaintOp::Copy(src, dst, size));
        }
    }

    fn read_paint_texture(&mut self, tex: TexId, size: u32, done: Box<dyn FnOnce(Vec<u8>)>) {
        if self.can_paint_now(1) {
            self.read_now(tex, size, done);
            self.paint_sent(1);
        } else {
            self.queue_paint(PaintOp::Read(tex, size, done));
        }
    }

    fn paint_pending(&self) -> bool {
        !self.paint_queue.is_empty()
    }

    fn create_paint_texture(&mut self, size: u32) -> Option<TexId> {
        // New textures start transparent.
        let tex = self.device.create_texture(&obj! {
            "label": "paint tile",
            "size": arr(vec![size.into(), size.into()]),
            "format": TEX_FORMAT,
            "usage": TEX_BINDING | TEX_COPY_DST | TEX_COPY_SRC | TEX_RENDER,
        });
        let bind = self.tex_bind_group(tex.create_view());
        let entry = TexEntry { tex, bind, w: size, h: size, mips: 1 };
        Some(TexId(match self.textures.iter().position(Option::is_none) {
            Some(i) => {
                self.textures[i] = Some(entry);
                i as u32
            }
            None => {
                self.textures.push(Some(entry));
                (self.textures.len() - 1) as u32
            }
        }))
    }





    fn prewarm_paint(&mut self) {
        if self.smudge_window.is_some() {
            return;
        }
        // Creating the pipelines now lets the GPU process compile them while
        // nothing is being painted yet.
        for p in [Pipe::Dab(PaintMode::Paint), Pipe::Dab(PaintMode::Erase), Pipe::Dab(PaintMode::AlphaLock), Pipe::SmudgePick, Pipe::SmudgeStamp] {
            self.pipeline(p);
        }
        let target = |label: &str, size: u32, usage: u32| {
            self.device.create_texture(&obj! { "label": label, "size": arr(vec![size.into(), size.into()]), "format": TEX_FORMAT, "usage": usage })
        };
        let tile = flexma_doc::raster::TILE;
        self.smudge_window = Some(target("smudge window", SMUDGE_WINDOW, TEX_BINDING | TEX_COPY_DST | TEX_RENDER));
        self.smudge_scratch = Some(target("smudge scratch", SMUDGE_BUF, TEX_COPY_SRC | TEX_RENDER));
        // New textures start transparent: unpainted tiles copy from this one.
        self.smudge_blank = Some(target("blank tile", tile, TEX_COPY_SRC));
    }


}

/// Paint work: done now, or queued until the GPU catches up.
impl GpuBackend {
    /// Whether paint work of `cost` passes can be encoded now: nothing waits
    /// ahead of it and the GPU isn't too far behind.
    fn can_paint_now(&self, cost: u32) -> bool {
        let inflight = self.paint_inflight.get();
        self.paint_queue.is_empty() && (inflight == 0 || inflight + cost <= self.paint_cap)
    }

    /// Count work just submitted as in flight until the GPU finishes it.
    fn paint_sent(&mut self, cost: u32) {
        if cost == 0 {
            return;
        }
        self.paint_inflight.set(self.paint_inflight.get() + cost);
        self.paint_costs.borrow_mut().push_back(cost);
        let _ = self.queue.on_submitted_work_done().then(&self.paint_done);
    }

    fn queue_paint(&mut self, op: PaintOp) {
        self.paint_queue.push_back(op);
        self.paint_queue_peak = self.paint_queue_peak.max(self.paint_queue.len());
        self.pump_paint();
    }

    /// Encode queued paint work, in order, as far as the GPU keeps up.
    fn pump_paint(&mut self) {
        while let Some(cost) = self.paint_queue.front().map(PaintOp::cost) {
            let inflight = self.paint_inflight.get();
            if inflight > 0 && inflight + cost > self.paint_cap {
                break;
            }
            let op = self.paint_queue.pop_front().unwrap();
            self.run_paint(op);
            self.paint_sent(cost);
        }
    }

    fn run_paint(&mut self, op: PaintOp) {
        match op {
            PaintOp::Dabs(b) => {
                let batches: Vec<DabBatch> = b.iter().map(|(tex, dabs, mode, style)| DabBatch { tex: *tex, dabs, mode: *mode, style: *style }).collect();
                self.paint_dabs_now(&batches);
            }
            PaintOp::Smudge(d) => {
                let dabs: Vec<SmudgeDab> = d
                    .iter()
                    .map(|q| SmudgeDab { tiles: &q.tiles, dab: q.dab, style: q.style, buffer: q.buffer, first: q.first, keep: q.keep, paint: q.paint, mix: q.mix, alpha_lock: q.alpha_lock })
                    .collect();
                self.smudge_now(&dabs);
            }
            PaintOp::Load(tex, img, size) => {
                let ok = match &img {
                    QueuedImage::Image(i) => self.load_now(tex, ImageSource::Image(i), size),
                    QueuedImage::Canvas(c) => self.load_now(tex, ImageSource::Canvas(c), size),
                };
                if !ok {
                    self.clear_now(tex, size);
                }
            }
            PaintOp::Clear(tex, size) => self.clear_now(tex, size),
            PaintOp::Copy(src, dst, size) => self.copy_now(src, dst, size),
            PaintOp::Read(tex, size, done) => self.read_now(tex, size, done),
            PaintOp::Delete(tex) => self.delete_now(tex),
        }
    }

    fn delete_now(&mut self, t: TexId) {
        if let Some(Some(e)) = self.textures.get_mut(t.0 as usize).map(Option::take) {
            self.trash_tex.push(e.tex);
        }
    }

    fn load_now(&mut self, tex: TexId, src: ImageSource, size: u32) -> bool {
        let Some(Some(e)) = self.textures.get(tex.0 as usize) else { return false };
        let source: JsValue = match src {
            ImageSource::Image(i) => i.clone().into(),
            ImageSource::Canvas(c) => c.clone().into(),
        };
        self.queue
            .copy_external_image_to_texture(
                &obj! { "source": source },
                &obj! { "texture": e.tex.clone(), "premultipliedAlpha": true },
                &arr(vec![size.into(), size.into()]),
            )
            .is_ok()
    }

    fn clear_now(&mut self, tex: TexId, _size: u32) {
        let Some(Some(e)) = self.textures.get(tex.0 as usize) else { return };
        let enc = self.device.create_command_encoder();
        self.paint_passes += 1;
        let pass = enc.begin_render_pass(&obj! {
            "colorAttachments": arr(vec![obj! { "view": e.tex.create_view(), "loadOp": "clear", "storeOp": "store",
                "clearValue": arr(vec![0u32.into(), 0u32.into(), 0u32.into(), 0u32.into()]) }.into()]),
        });
        pass.end();
        self.paint_submits += 1;
        self.queue.submit(&arr(vec![enc.finish()]));
    }

    fn paint_dabs_now(&mut self, batches: &[DabBatch]) {
        let batches: Vec<&DabBatch> = batches.iter().filter(|b| !b.dabs.is_empty()).collect();
        if batches.is_empty() {
            return;
        }
        // One instance buffer for every batch (each draws from its first
        // instance), one uniform slot per batch, one submit.
        let mut inst: Vec<f32> = Vec::with_capacity(batches.iter().map(|b| b.dabs.len()).sum());
        let mut first = Vec::with_capacity(batches.len());
        let floats = SLOT_BYTES / 4;
        let mut uni = vec![0.0f32; batches.len() * floats];
        for (i, b) in batches.iter().enumerate() {
            first.push((inst.len() / DAB_FLOATS) as u32);
            inst.extend_from_slice(b.dabs);
            uni[i * floats..i * floats + 16].copy_from_slice(&style_words(&b.style, flexma_doc::raster::TILE as f32));
        }
        Self::write_instances(&self.device, &self.queue, &mut self.dab_inst, &mut self.trash_buf, &inst);
        let ubuf = Self::write_uniforms(&self.device, &self.queue, &mut self.dab_uniforms, &mut self.trash_buf, &uni);
        let pipes = [PaintMode::Paint, PaintMode::Erase, PaintMode::AlphaLock].map(|m| (m, self.pipeline(Pipe::Dab(m))));
        let mut binds: HashMap<[Option<TexId>; 3], JsValue> = HashMap::new();
        let mut enc = self.device.create_command_encoder();
        let inst_buf = self.dab_inst.as_ref().unwrap().buf.clone();
        // Dabs drawn since the last submit (see DABS_PER_SUBMIT).
        let mut in_submit = 0u32;
        let mut i = 0;
        while i < batches.len() {
            let tex = batches[i].tex;
            let Some(view) = self.textures.get(tex.0 as usize).and_then(Option::as_ref).map(|e| e.tex.create_view()) else {
                i += 1;
                continue;
            };
            // Consecutive batches for the same tile share a render pass.
            let begin = |enc: &GpuCommandEncoder| {
                let pass = enc.begin_render_pass(&obj! {
                    "colorAttachments": arr(vec![obj! { "view": view.clone(), "loadOp": "load", "storeOp": "store" }.into()]),
                });
                pass.set_vertex_buffer(0, &inst_buf);
                pass
            };
            self.paint_passes += 1;
            let mut pass = begin(&enc);
            while i < batches.len() && batches[i].tex == tex {
                let b = batches[i];
                let key = [b.style.tip, b.style.grain, b.style.tint];
                if !binds.contains_key(&key) {
                    let mut entries = vec![obj! { "binding": 0u32, "resource": obj! { "buffer": ubuf.clone(), "offset": 0u32, "size": STYLE_BYTES } }.into()];
                    entries.extend(self.brush_bindings(&b.style));
                    binds.insert(key, self.device.create_bind_group(&obj! { "layout": self.dab_bgl.clone(), "entries": arr(entries) }));
                }
                let pipe = &pipes.iter().find(|(m, _)| *m == b.mode).unwrap().1;
                // In order, split across submissions when the batch is long.
                let total = (b.dabs.len() / DAB_FLOATS) as u32;
                let mut done = 0;
                while done < total {
                    if in_submit >= DABS_PER_SUBMIT {
                        pass.end();
                        self.paint_submits += 1;
                        self.queue.submit(&arr(vec![enc.finish()]));
                        enc = self.device.create_command_encoder();
                        in_submit = 0;
                        self.paint_passes += 1;
                        pass = begin(&enc);
                    }
                    let n = (total - done).min(DABS_PER_SUBMIT - in_submit);
                    pass.set_pipeline(pipe);
                    pass.set_bind_group_offsets(0, &binds[&key], &[(i * SLOT_BYTES) as u32]);
                    pass.draw(4, n, 0, first[i] + done);
                    done += n;
                    in_submit += n;
                }
                i += 1;
            }
            pass.end();
        }
        self.paint_submits += 1;
        self.queue.submit(&arr(vec![enc.finish()]));
    }

    fn smudge_now(&mut self, dabs: &[SmudgeDab]) {
        if dabs.is_empty() {
            return;
        }
        self.prewarm_paint();
        let (window, scratch, blank) = (self.smudge_window.clone().unwrap(), self.smudge_scratch.clone().unwrap(), self.smudge_blank.clone().unwrap());
        // One uniform slot per pass: the pick, then a stamp per written tile.
        let floats = SLOT_BYTES / 4;
        let mut data: Vec<f32> = Vec::new();
        let mut plans = Vec::new();
        let tile = flexma_doc::raster::TILE as f32;
        for d in dabs {
            let Some(win) = crate::gl::window_of(d.tiles, SMUDGE_WINDOW) else { continue };
            let reach = crate::gl::smudge_reach(d);
            let mut slot = |place: [f32; 4], quad: [f32; 4]| {
                let mut w = vec![0.0f32; floats];
                w[..16].copy_from_slice(&style_words(&d.style, tile));
                w[16..20].copy_from_slice(&[d.dab[0], d.dab[1], d.dab[2], d.dab[8]]);
                w[20..24].copy_from_slice(&[d.dab[3], d.dab[7], d.alpha_lock as i32 as f32, tile]);
                w[24..28].copy_from_slice(&place);
                w[28..32].copy_from_slice(&quad);
                w[32..36].copy_from_slice(&[win[0], win[1], SMUDGE_WINDOW as f32, SMUDGE_WINDOW as f32]);
                w[36..40].copy_from_slice(&d.paint);
                w[40..44].copy_from_slice(&[d.first as i32 as f32, d.keep, d.mix, SMUDGE_BUF as f32]);
                data.extend_from_slice(&w);
                (data.len() / floats - 1) as u32
            };
            let pick = slot([win[0], win[1], 0.0, 0.0], [0.0; 4]);
            let stamps: Vec<(u32, TexId)> = d
                .tiles
                .iter()
                .filter(|t| t.write)
                .filter_map(|t| {
                    let tex = t.tex?;
                    let (x, y) = (d.dab[0] - t.origin[0], d.dab[1] - t.origin[1]);
                    Some((slot([win[0], win[1], t.origin[0], t.origin[1]], [x - reach, y - reach, x + reach, y + reach]), tex))
                })
                .collect();
            plans.push((d, win, reach, pick, stamps));
        }
        if plans.is_empty() {
            return;
        }
        let ubuf = Self::write_uniforms(&self.device, &self.queue, &mut self.smudge_uniform, &mut self.trash_buf, &data);
        let (pick_pipe, stamp_pipe) = (self.pipeline(Pipe::SmudgePick), self.pipeline(Pipe::SmudgeStamp));
        let mut binds: HashMap<[Option<TexId>; 4], JsValue> = HashMap::new();
        let mut enc = self.device.create_command_encoder();
        for (n, (d, win, reach, pick, stamps)) in plans.into_iter().enumerate() {
            // Long smudge replays go out in several submissions (see DABS_PER_SUBMIT).
            if n > 0 && n % SMUDGE_DABS_PER_SUBMIT == 0 {
                self.paint_submits += 1;
                self.queue.submit(&arr(vec![enc.finish()]));
                enc = self.device.create_command_encoder();
            }
            let Some(buf) = self.textures.get(d.buffer.0 as usize).and_then(Option::as_ref).map(|e| e.tex.clone()) else { continue };
            // 1. Only the pixels under the dab (plus a texel for filtering)
            // are copied into the window; unpainted tiles copy as transparent.
            let (bx0, by0) = ((d.dab[0] - reach - 2.0).floor(), (d.dab[1] - reach - 2.0).floor());
            let (bx1, by1) = ((d.dab[0] + reach + 2.0).ceil(), (d.dab[1] + reach + 2.0).ceil());
            for t in d.tiles {
                let (x0, y0) = (bx0.max(t.origin[0]), by0.max(t.origin[1]));
                let (x1, y1) = (bx1.min(t.origin[0] + tile), by1.min(t.origin[1] + tile));
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let src = t.tex.and_then(|x| self.textures.get(x.0 as usize)).and_then(Option::as_ref).map(|e| e.tex.clone()).unwrap_or_else(|| blank.clone());
                enc.copy_texture_to_texture(
                    &obj! { "texture": src, "origin": arr(vec![((x0 - t.origin[0]) as u32).into(), ((y0 - t.origin[1]) as u32).into()]) },
                    &obj! { "texture": window.clone(), "origin": arr(vec![((x0 - win[0]) as u32).into(), ((y0 - win[1]) as u32).into()]) },
                    &arr(vec![((x1 - x0) as u32).into(), ((y1 - y0) as u32).into()]),
                );
            }
            let key = [Some(d.buffer), d.style.tip, d.style.grain, d.style.tint];
            if !binds.contains_key(&key) {
                let mut entries = vec![obj! { "binding": 0u32, "resource": obj! { "buffer": ubuf.clone(), "offset": 0u32, "size": SMUDGE_BYTES } }.into()];
                entries.extend(self.brush_bindings(&d.style));
                entries.push(obj! { "binding": 6u32, "resource": window.create_view() }.into());
                entries.push(obj! { "binding": 7u32, "resource": buf.create_view() }.into());
                binds.insert(key, self.device.create_bind_group(&obj! { "layout": self.smudge_bgl.clone(), "entries": arr(entries) }));
            }
            let bind = &binds[&key];
            // 2. Pick up into the scratch, which becomes the stroke's buffer.
            self.paint_passes += 1;
            let pass = enc.begin_render_pass(&obj! {
                "colorAttachments": arr(vec![obj! { "view": scratch.create_view(), "loadOp": "clear", "storeOp": "store",
                    "clearValue": arr(vec![0u32.into(), 0u32.into(), 0u32.into(), 0u32.into()]) }.into()]),
            });
            pass.set_pipeline(&pick_pipe);
            pass.set_bind_group_offsets(0, bind, &[pick * SLOT_BYTES as u32]);
            pass.draw(3, 1, 0, 0);
            pass.end();
            enc.copy_texture_to_texture(&obj! { "texture": scratch.clone() }, &obj! { "texture": buf.clone() }, &arr(vec![SMUDGE_BUF.into(), SMUDGE_BUF.into()]));
            // 3. Lay it down, tile by tile.
            for (slot, tex) in stamps {
                let Some(e) = self.textures.get(tex.0 as usize).and_then(Option::as_ref) else { continue };
                self.paint_passes += 1;
                let pass = enc.begin_render_pass(&obj! {
                    "colorAttachments": arr(vec![obj! { "view": e.tex.create_view(), "loadOp": "load", "storeOp": "store" }.into()]),
                });
                pass.set_pipeline(&stamp_pipe);
                pass.set_bind_group_offsets(0, bind, &[slot * SLOT_BYTES as u32]);
                pass.draw(4, 1, 0, 0);
                pass.end();
            }
        }
        self.paint_submits += 1;
        self.queue.submit(&arr(vec![enc.finish()]));
    }

    fn copy_now(&mut self, src: TexId, dst: TexId, size: u32) {
        let (Some(Some(a)), Some(Some(b))) = (self.textures.get(src.0 as usize), self.textures.get(dst.0 as usize)) else { return };
        let enc = self.device.create_command_encoder();
        enc.copy_texture_to_texture(&obj! { "texture": a.tex.clone() }, &obj! { "texture": b.tex.clone() }, &arr(vec![size.into(), size.into()]));
        self.paint_submits += 1;
        self.queue.submit(&arr(vec![enc.finish()]));
    }

    fn read_now(&mut self, tex: TexId, size: u32, done: Box<dyn FnOnce(Vec<u8>)>) {
        let Some(Some(e)) = self.textures.get(tex.0 as usize) else { return };
        let bytes = size * size * 4;
        let buf = self.device.create_buffer(&obj! { "size": bytes as f64, "usage": BUF_MAP_READ | BUF_COPY_DST });
        let enc = self.device.create_command_encoder();
        enc.copy_texture_to_buffer(
            &obj! { "texture": e.tex.clone() },
            &obj! { "buffer": buf.clone(), "bytesPerRow": size * 4, "rowsPerImage": size },
            &arr(vec![size.into(), size.into()]),
        );
        self.paint_submits += 1;
        self.queue.submit(&arr(vec![enc.finish()]));
        let b2 = buf.clone();
        let mapped = Closure::once(move |_: JsValue| {
            let data = js_sys::Uint8Array::new(&b2.get_mapped_range()).to_vec();
            b2.unmap();
            b2.destroy();
            done(data);
        });
        let _ = buf.map_async(1).then(&mapped);
        // Snapshots are rare; the closure lives until it has run.
        mapped.forget();
    }
}

/// The `Style` uniform (brush.wgsl): 16 words.
fn style_words(st: &DabStyle, size: f32) -> [f32; 16] {
    let flags = st.tip.is_some() as u32 | (st.tip_color as u32) << 1 | (st.grain.is_some() as u32) << 2 | (st.tint.is_some() as u32) << 3;
    let [grain_lod, tint_lod] = texture_lods(st);
    [
        st.tip_extent[0],
        st.tip_extent[1],
        st.origin[0],
        st.origin[1],
        st.grain_size[0],
        st.grain_size[1],
        st.tint_size[0],
        st.tint_size[1],
        st.grain_strength,
        st.tint_strength,
        grain_lod,
        tint_lod,
        st.tip_px[0],
        st.tip_px[1],
        f32::from_bits(flags),
        size,
    ]
}
