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

use crate::backend::{texture_lods, Backend, DabStyle, FrameParams, ImageSource, MeshId, PaintMode, Rows, SmudgeDab, TexId, DAB_FLOATS, SMUDGE_BUF};
use crate::gl::INST_FLOATS;
use js_sys::{Array, Object, Reflect};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

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
    dab_module: JsValue,
    dab_bgl: JsValue,
    dab_layout: JsValue,
    dab_inst: Option<Grow>,
    dab_uniform: GpuBuffer,
    smudge_module: JsValue,
    smudge_bgl: JsValue,
    smudge_layout: JsValue,
    smudge_uniform: Option<Grow>,
    /// Window and scratch textures (created on first use).
    smudge_window: Option<GpuTexture>,
    smudge_scratch: Option<GpuTexture>,
    repeat_sampler: JsValue,
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
    on_error: js_sys::Function,
    wake: Rc<Cell<bool>>,
    /// Checks each frame's error scope (one closure, reused every frame).
    frame_check: Option<Closure<dyn FnMut(JsValue)>>,
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
    pub fn new(device: JsValue, context: JsValue, format: String, on_error: js_sys::Function) -> Result<GpuBackend, String> {
        let device: GpuDevice = device.unchecked_into();
        let context: GpuCanvasContext = context.unchecked_into();
        let queue = device.queue();
        let module = device.create_shader_module(&obj! { "label": "flexma", "code": SHADERS });
        let post_module = device.create_shader_module(&obj! { "label": "flexma-post", "code": POST_SHADERS });
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
            "entries": brush_entries(obj! { "type": "uniform", "minBindingSize": STYLE_BYTES }.into(), 0),
        });
        let smudge_bgl = device.create_bind_group_layout(&obj! {
            "entries": brush_entries(obj! { "type": "uniform", "hasDynamicOffset": true, "minBindingSize": SMUDGE_BYTES }.into(), 2),
        });
        let dab_layout = device.create_pipeline_layout(&obj! { "bindGroupLayouts": arr(vec![dab_bgl.clone()]) });
        let smudge_layout = device.create_pipeline_layout(&obj! { "bindGroupLayouts": arr(vec![smudge_bgl.clone()]) });
        let dab_uniform = device.create_buffer(&obj! { "size": SLOT_BYTES as f64, "usage": BUF_UNIFORM | BUF_COPY_DST });
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
        Ok(GpuBackend {
            device,
            queue,
            context,
            format,
            module,
            post_module,
            dab_module,
            dab_bgl,
            dab_layout,
            dab_inst: None,
            dab_uniform,
            smudge_module,
            smudge_bgl,
            smudge_layout,
            smudge_uniform: None,
            smudge_window: None,
            smudge_scratch: None,
            repeat_sampler,
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
            on_error,
            wake: Rc::new(Cell::new(false)),
            frame_check: None,
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

    fn delete_texture(&mut self, t: TexId) {
        if let Some(Some(e)) = self.textures.get_mut(t.0 as usize).map(Option::take) {
            self.trash_tex.push(e.tex);
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
        let scene_ops = std::mem::take(&mut self.scene_ops);
        self.replay(&scene, &scene_ops, true, f.linear);
        scene.end();
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
            let (bad, wake, on_error) = (self.bad_frames.clone(), self.wake.clone(), self.on_error.clone());
            self.frame_check = Some(Closure::new(move |err: JsValue| {
                if err.is_null() || err.is_undefined() {
                    bad.set(0);
                    return;
                }
                let n = bad.get() + 1;
                bad.set(n);
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
    }

    fn draw_calls(&self) -> u32 {
        self.draw_calls
    }

    fn attach_wake(&mut self, wake: Rc<Cell<bool>>) {
        self.wake = wake;
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

    fn load_paint_texture(&mut self, tex: TexId, src: ImageSource, size: u32) -> bool {
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

    fn clear_paint_texture(&mut self, tex: TexId, _size: u32) {
        let Some(Some(e)) = self.textures.get(tex.0 as usize) else { return };
        let enc = self.device.create_command_encoder();
        let pass = enc.begin_render_pass(&obj! {
            "colorAttachments": arr(vec![obj! { "view": e.tex.create_view(), "loadOp": "clear", "storeOp": "store",
                "clearValue": arr(vec![0u32.into(), 0u32.into(), 0u32.into(), 0u32.into()]) }.into()]),
        });
        pass.end();
        self.queue.submit(&arr(vec![enc.finish()]));
    }

    fn paint_dabs(&mut self, tex: TexId, size: u32, dabs: &[f32], mode: PaintMode, style: &DabStyle) {
        if dabs.is_empty() {
            return;
        }
        let Some(view) = self.textures.get(tex.0 as usize).and_then(Option::as_ref).map(|e| e.tex.create_view()) else { return };
        Self::write_instances(&self.device, &self.queue, &mut self.dab_inst, &mut self.trash_buf, dabs);
        self.queue.write_f32(&self.dab_uniform, 0.0, &style_words(style, size as f32));
        let mut entries = vec![obj! { "binding": 0u32, "resource": obj! { "buffer": self.dab_uniform.clone(), "offset": 0u32, "size": STYLE_BYTES } }.into()];
        entries.extend(self.brush_bindings(style));
        let bind = self.device.create_bind_group(&obj! { "layout": self.dab_bgl.clone(), "entries": arr(entries) });
        let pipe = self.pipeline(Pipe::Dab(mode));
        let enc = self.device.create_command_encoder();
        let pass = enc.begin_render_pass(&obj! {
            "colorAttachments": arr(vec![obj! { "view": view, "loadOp": "load", "storeOp": "store" }.into()]),
        });
        pass.set_pipeline(&pipe);
        pass.set_bind_group(0, &bind);
        pass.set_vertex_buffer(0, &self.dab_inst.as_ref().unwrap().buf);
        pass.draw(4, (dabs.len() / DAB_FLOATS) as u32, 0, 0);
        pass.end();
        self.queue.submit(&arr(vec![enc.finish()]));
    }

    fn smudge(&mut self, dabs: &[SmudgeDab]) {
        if dabs.is_empty() {
            return;
        }
        let target = |label: &str, size: u32, usage: u32| {
            self.device.create_texture(&obj! { "label": label, "size": arr(vec![size.into(), size.into()]), "format": TEX_FORMAT, "usage": usage })
        };
        if self.smudge_window.is_none() {
            self.smudge_window = Some(target("smudge window", SMUDGE_WINDOW, TEX_BINDING | TEX_COPY_DST | TEX_RENDER));
            self.smudge_scratch = Some(target("smudge scratch", SMUDGE_BUF, TEX_COPY_SRC | TEX_RENDER));
        }
        let (window, scratch) = (self.smudge_window.clone().unwrap(), self.smudge_scratch.clone().unwrap());
        // One uniform slot per pass: the pick, then a stamp per written tile.
        let floats = SLOT_BYTES / 4;
        let mut data: Vec<f32> = Vec::new();
        let mut plans = Vec::new();
        for d in dabs {
            let Some(win) = crate::gl::window_of(d.tiles, SMUDGE_WINDOW) else { continue };
            let reach = crate::gl::smudge_reach(d);
            let tile = flexma_doc::raster::TILE as f32;
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
            plans.push((d, win, pick, stamps));
        }
        if plans.is_empty() {
            return;
        }
        let bytes = (data.len() * 4) as u64;
        if self.smudge_uniform.as_ref().is_none_or(|g| g.cap < bytes) {
            let cap = (bytes + bytes / 2).max(64 * 1024);
            if let Some(old) = self.smudge_uniform.take() {
                self.trash_buf.push(old.buf);
            }
            let buf = self.device.create_buffer(&obj! { "size": cap as f64, "usage": BUF_UNIFORM | BUF_COPY_DST });
            self.smudge_uniform = Some(Grow { buf, cap });
        }
        let ubuf = self.smudge_uniform.as_ref().unwrap().buf.clone();
        self.queue.write_f32(&ubuf, 0.0, &data);
        let (pick_pipe, stamp_pipe) = (self.pipeline(Pipe::SmudgePick), self.pipeline(Pipe::SmudgeStamp));
        let tile = flexma_doc::raster::TILE;
        let enc = self.device.create_command_encoder();
        for (d, win, pick, stamps) in plans {
            let Some(buf) = self.textures.get(d.buffer.0 as usize).and_then(Option::as_ref).map(|e| e.tex.clone()) else { continue };
            // 1. The tiles under the dab, side by side.
            enc.begin_render_pass(&obj! {
                "colorAttachments": arr(vec![obj! { "view": window.create_view(), "loadOp": "clear", "storeOp": "store",
                    "clearValue": arr(vec![0u32.into(), 0u32.into(), 0u32.into(), 0u32.into()]) }.into()]),
            })
            .end();
            for t in d.tiles {
                let Some(src) = t.tex.and_then(|x| self.textures.get(x.0 as usize)).and_then(Option::as_ref) else { continue };
                let (x, y) = ((t.origin[0] - win[0]) as u32, (t.origin[1] - win[1]) as u32);
                enc.copy_texture_to_texture(
                    &obj! { "texture": src.tex.clone() },
                    &obj! { "texture": window.clone(), "origin": arr(vec![x.into(), y.into()]) },
                    &arr(vec![tile.into(), tile.into()]),
                );
            }
            let mut entries = vec![obj! { "binding": 0u32, "resource": obj! { "buffer": ubuf.clone(), "offset": 0u32, "size": SMUDGE_BYTES } }.into()];
            entries.extend(self.brush_bindings(&d.style));
            entries.push(obj! { "binding": 6u32, "resource": window.create_view() }.into());
            entries.push(obj! { "binding": 7u32, "resource": buf.create_view() }.into());
            let bind = self.device.create_bind_group(&obj! { "layout": self.smudge_bgl.clone(), "entries": arr(entries) });
            // 2. Pick up into the scratch, which becomes the stroke's buffer.
            let pass = enc.begin_render_pass(&obj! {
                "colorAttachments": arr(vec![obj! { "view": scratch.create_view(), "loadOp": "clear", "storeOp": "store",
                    "clearValue": arr(vec![0u32.into(), 0u32.into(), 0u32.into(), 0u32.into()]) }.into()]),
            });
            pass.set_pipeline(&pick_pipe);
            pass.set_bind_group_offsets(0, &bind, &[pick * SLOT_BYTES as u32]);
            pass.draw(3, 1, 0, 0);
            pass.end();
            enc.copy_texture_to_texture(&obj! { "texture": scratch.clone() }, &obj! { "texture": buf.clone() }, &arr(vec![SMUDGE_BUF.into(), SMUDGE_BUF.into()]));
            // 3. Lay it down, tile by tile.
            for (slot, tex) in stamps {
                let Some(e) = self.textures.get(tex.0 as usize).and_then(Option::as_ref) else { continue };
                let pass = enc.begin_render_pass(&obj! {
                    "colorAttachments": arr(vec![obj! { "view": e.tex.create_view(), "loadOp": "load", "storeOp": "store" }.into()]),
                });
                pass.set_pipeline(&stamp_pipe);
                pass.set_bind_group_offsets(0, &bind, &[slot * SLOT_BYTES as u32]);
                pass.draw(4, 1, 0, 0);
                pass.end();
            }
        }
        self.queue.submit(&arr(vec![enc.finish()]));
    }

    fn copy_paint_texture(&mut self, src: TexId, dst: TexId, size: u32) {
        let (Some(Some(a)), Some(Some(b))) = (self.textures.get(src.0 as usize), self.textures.get(dst.0 as usize)) else { return };
        let enc = self.device.create_command_encoder();
        enc.copy_texture_to_texture(&obj! { "texture": a.tex.clone() }, &obj! { "texture": b.tex.clone() }, &arr(vec![size.into(), size.into()]));
        self.queue.submit(&arr(vec![enc.finish()]));
    }

    fn read_paint_texture(&mut self, tex: TexId, size: u32, done: Box<dyn FnOnce(Vec<u8>)>) {
        let Some(Some(e)) = self.textures.get(tex.0 as usize) else { return };
        let bytes = size * size * 4;
        let buf = self.device.create_buffer(&obj! { "size": bytes as f64, "usage": BUF_MAP_READ | BUF_COPY_DST });
        let enc = self.device.create_command_encoder();
        enc.copy_texture_to_buffer(
            &obj! { "texture": e.tex.clone() },
            &obj! { "buffer": buf.clone(), "bytesPerRow": size * 4, "rowsPerImage": size },
            &arr(vec![size.into(), size.into()]),
        );
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
