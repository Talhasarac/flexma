//! The WebGL2 backend (the fallback when WebGPU isn't available), its helpers
//! and the GLSL shader sources.

use crate::backend::{Backend, DabBatch, DabStyle, FrameParams, ImageSource, MeshId, PaintMode, Rows, SmudgeDab, TexId, DAB_FLOATS, SMUDGE_BUF};
use std::collections::HashMap;
use wasm_bindgen::JsCast;
use web_sys::{
    WebGl2RenderingContext as GL, WebGlBuffer, WebGlFramebuffer, WebGlProgram, WebGlRenderbuffer, WebGlShader, WebGlTexture,
    WebGlUniformLocation, WebGlVertexArrayObject,
};

pub fn compile(gl: &GL, kind: u32, src: &str) -> Result<WebGlShader, String> {
    let sh = gl.create_shader(kind).ok_or("create_shader failed")?;
    gl.shader_source(&sh, src);
    gl.compile_shader(&sh);
    if gl.get_shader_parameter(&sh, GL::COMPILE_STATUS).as_bool().unwrap_or(false) {
        Ok(sh)
    } else {
        Err(gl.get_shader_info_log(&sh).unwrap_or_default())
    }
}

pub fn program(gl: &GL, vs: &str, fs: &str) -> Result<WebGlProgram, String> {
    let v = compile(gl, GL::VERTEX_SHADER, vs)?;
    let f = compile(gl, GL::FRAGMENT_SHADER, fs)?;
    let p = gl.create_program().ok_or("create_program failed")?;
    gl.attach_shader(&p, &v);
    gl.attach_shader(&p, &f);
    gl.link_program(&p);
    if gl.get_program_parameter(&p, GL::LINK_STATUS).as_bool().unwrap_or(false) {
        Ok(p)
    } else {
        Err(gl.get_program_info_log(&p).unwrap_or_default())
    }
}

pub struct Uniforms {
    pub locs: Vec<Option<WebGlUniformLocation>>,
}

impl Uniforms {
    pub fn new(gl: &GL, p: &WebGlProgram, names: &[&str]) -> Uniforms {
        Uniforms { locs: names.iter().map(|n| gl.get_uniform_location(p, n)).collect() }
    }
    pub fn get(&self, i: usize) -> Option<&WebGlUniformLocation> {
        self.locs[i].as_ref()
    }
}

/// Upload f32 data into the currently bound ARRAY_BUFFER without copying into a
/// new JS array (view over wasm memory, created right before use).
pub fn buffer_f32(gl: &GL, target: u32, data: &[f32], usage: u32) {
    unsafe {
        let view = js_sys::Float32Array::view(data);
        gl.buffer_data_with_array_buffer_view(target, &view, usage);
    }
}

const NO_WEBGL: &str = "WebGL2 is not available in this browser. Turn on hardware acceleration, or restart the browser (it disables WebGL after a graphics driver problem).";

/// Upload into a grow-only buffer: reallocate (with headroom) only when the data
/// outgrows it, otherwise update in place. Avoids per-frame GPU reallocations.
pub fn upload_growable(gl: &GL, target: u32, data: &[f32], capacity: &mut usize, usage: u32) {
    let bytes = data.len() * 4;
    if bytes > *capacity {
        let new_cap = (bytes + bytes / 2).max(64 * 1024);
        gl.buffer_data_with_i32(target, new_cap as i32, usage);
        *capacity = new_cap;
    }
    if bytes > 0 {
        unsafe {
            let view = js_sys::Float32Array::view(data);
            gl.buffer_sub_data_with_i32_and_array_buffer_view(target, 0, &view);
        }
    }
}

pub fn context(canvas: &web_sys::HtmlCanvasElement) -> Result<GL, String> {
    let attrs = js_sys::Object::new();
    let set = |k: &str, v: wasm_bindgen::JsValue| {
        let _ = js_sys::Reflect::set(&attrs, &k.into(), &v);
    };
    set("antialias", true.into());
    set("alpha", true.into());
    set("stencil", true.into());
    set("depth", false.into());
    set("premultipliedAlpha", true.into());
    set("preserveDrawingBuffer", false.into());
    // Default power preference and no `desynchronized`: both are safer inside
    // embedded webviews and on dual-GPU laptops.
    set("powerPreference", "default".into());
    let ctx = canvas
        .get_context_with_context_options("webgl2", &attrs)
        .map_err(|_| NO_WEBGL)?
        .ok_or(NO_WEBGL)?;
    ctx.dyn_into::<GL>().map_err(|_| "WebGL2 is not available".to_string())
}

/// Floats per shape instance (13 vec4 attributes).
pub const INST_FLOATS: usize = 52;

const STRIDE: i32 = (INST_FLOATS * 4) as i32;

/// A multisampled sRGB color + depth/stencil target and the texture it
/// resolves into. Blending into an sRGB attachment happens in linear light.
struct LinearTarget {
    w: i32,
    h: i32,
    msaa_fbo: WebGlFramebuffer,
    msaa_color: WebGlRenderbuffer,
    msaa_depth: WebGlRenderbuffer,
    resolve_fbo: WebGlFramebuffer,
    resolve_tex: WebGlTexture,
}

/// Background blur: the scene so far, and two smaller textures it's blurred
/// through (across, then down).
struct BackdropTargets {
    w: i32,
    h: i32,
    sw: i32,
    sh: i32,
    srgb: bool,
    src: (WebGlTexture, WebGlFramebuffer),
    a: (WebGlTexture, WebGlFramebuffer),
    b: (WebGlTexture, WebGlFramebuffer),
}

/// A color texture to render into, with its framebuffer.
fn render_texture(gl: &GL, w: i32, h: i32, srgb: bool) -> Option<(WebGlTexture, WebGlFramebuffer)> {
    let t = gl.create_texture()?;
    gl.bind_texture(GL::TEXTURE_2D, Some(&t));
    gl.tex_storage_2d(GL::TEXTURE_2D, 1, if srgb { GL::SRGB8_ALPHA8 } else { GL::RGBA8 }, w.max(1), h.max(1));
    gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MIN_FILTER, GL::LINEAR as i32);
    gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MAG_FILTER, GL::LINEAR as i32);
    gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_S, GL::CLAMP_TO_EDGE as i32);
    gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_T, GL::CLAMP_TO_EDGE as i32);
    let f = gl.create_framebuffer()?;
    gl.bind_framebuffer(GL::FRAMEBUFFER, Some(&f));
    gl.framebuffer_texture_2d(GL::FRAMEBUFFER, GL::COLOR_ATTACHMENT0, GL::TEXTURE_2D, Some(&t), 0);
    gl.bind_framebuffer(GL::FRAMEBUFFER, None);
    Some((t, f))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Prog {
    None,
    Shape,
    Mesh,
    Tex,
    Label,
}

pub struct GlBackend {
    gl: GL,
    shape_prog: WebGlProgram,
    shape_u: Uniforms,
    mesh_prog: WebGlProgram,
    mesh_u: Uniforms,
    tex_prog: WebGlProgram,
    tex_u: Uniforms,
    composite_prog: WebGlProgram,
    composite_u: Uniforms,
    blur_prog: WebGlProgram,
    blur_u: Uniforms,
    copy_prog: WebGlProgram,
    copy_u: Uniforms,
    backdrop: Option<BackdropTargets>,
    /// The frame's size in device px.
    frame_size: (i32, i32),
    dab_prog: WebGlProgram,
    dab_u: Uniforms,
    dab_vao: WebGlVertexArrayObject,
    dab_buf: WebGlBuffer,
    dab_cap: usize,
    /// Smudge: pick up the layer into a stroke's buffer, then lay it down.
    pick_prog: WebGlProgram,
    pick_u: Uniforms,
    stamp_prog: WebGlProgram,
    stamp_u: Uniforms,
    /// The tiles under a smudge dab, copied side by side, and a scratch
    /// buffer (created on first use).
    smudge_window: Option<(WebGlTexture, WebGlFramebuffer)>,
    smudge_scratch: Option<(WebGlTexture, WebGlFramebuffer)>,
    /// Tile readbacks in flight: pixels copy into a buffer on the GPU and are
    /// collected once its fence has passed, so reading never stalls a frame.
    readbacks: Vec<Readback>,
    paint_passes: u64,
    /// Framebuffers for paint tiles, by texture slot.
    paint_fbos: HashMap<u32, WebGlFramebuffer>,
    shape_vao: WebGlVertexArrayObject,
    mesh_vao: WebGlVertexArrayObject,
    empty_vao: WebGlVertexArrayObject,
    inst_buf: WebGlBuffer,
    inst_cap: usize,
    overlay_buf: WebGlBuffer,
    overlay_cap: usize,
    white: WebGlTexture,
    textures: Vec<Option<WebGlTexture>>,
    meshes: Vec<Option<(WebGlBuffer, WebGlBuffer, i32)>>,
    max_tex: u32,
    linear_target: Option<LinearTarget>,
    /// Shaders currently write linear light (into `linear_target`).
    linear_out: bool,
    depth: i32,
    current: Prog,
    view: [f32; 9],
    screen: [f32; 9],
    px: f32,
    draw_calls: u32,
}

struct Readback {
    buf: WebGlBuffer,
    sync: web_sys::WebGlSync,
    bytes: usize,
    done: Box<dyn FnOnce(Vec<u8>)>,
}

/// Reuse a freed slot in `v` or append.
fn slot<T>(v: &mut Vec<Option<T>>, item: T) -> u32 {
    match v.iter().position(Option::is_none) {
        Some(i) => {
            v[i] = Some(item);
            i as u32
        }
        None => {
            v.push(Some(item));
            (v.len() - 1) as u32
        }
    }
}

impl GlBackend {
    pub fn new(gl: GL) -> Result<GlBackend, String> {
        let shape_prog = program(&gl, SHAPE_VS, SHAPE_FS)?;
        let mesh_prog = program(&gl, MESH_VS, MESH_FS)?;
        let tex_prog = program(&gl, TEX_VS, TEX_FS)?;
        let composite_prog = program(&gl, COMPOSITE_VS, COMPOSITE_FS)?;
        let blur_prog = program(&gl, COMPOSITE_VS, BLUR_FS)?;
        let blur_u = Uniforms::new(&gl, &blur_prog, &["u_tex", "u_step", "u_sigma"]);
        let copy_prog = program(&gl, COMPOSITE_VS, COPY_FS)?;
        let copy_u = Uniforms::new(&gl, &copy_prog, &["u_tex"]);
        let shape_u = Uniforms::new(&gl, &shape_prog, &["u_view", "u_px", "u_tex", "u_linear"]);
        let mesh_u = Uniforms::new(&gl, &mesh_prog, &["u_view", "u_m0", "u_m1", "u_color", "u_linear"]);
        let tex_u = Uniforms::new(&gl, &tex_prog, &["u_view", "u_m0", "u_m1", "u_rect", "u_tex", "u_alpha", "u_linear", "u_uv"]);
        let dab_prog = program(&gl, DAB_VS, &dab_fs())?;
        let dab_u = Uniforms::new(&gl, &dab_prog, &DAB_UNIFORMS);
        let pick_prog = program(&gl, COMPOSITE_VS, &pick_fs())?;
        let pick_u = Uniforms::new(&gl, &pick_prog, &PICK_UNIFORMS);
        let stamp_prog = program(&gl, STAMP_VS, &stamp_fs())?;
        let stamp_u = Uniforms::new(&gl, &stamp_prog, &STAMP_UNIFORMS);
        let dab_vao = gl.create_vertex_array().ok_or("vao")?;
        let dab_buf = gl.create_buffer().ok_or("buffer")?;
        gl.bind_vertex_array(Some(&dab_vao));
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&dab_buf));
        for i in 0..3u32 {
            gl.enable_vertex_attrib_array(i);
            gl.vertex_attrib_divisor(i, 1);
            gl.vertex_attrib_pointer_with_i32(i, 4, GL::FLOAT, false, (DAB_FLOATS * 4) as i32, (i * 16) as i32);
        }
        gl.bind_vertex_array(None);
        let composite_u = Uniforms::new(&gl, &composite_prog, &["u_tex"]);
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
        let max_tex = gl.get_parameter(GL::MAX_TEXTURE_SIZE).ok().and_then(|v| v.as_f64()).unwrap_or(4096.0).min(8192.0) as u32;
        gl.enable(GL::BLEND);
        gl.blend_func(GL::ONE, GL::ONE_MINUS_SRC_ALPHA);
        gl.pixel_storei(GL::UNPACK_PREMULTIPLY_ALPHA_WEBGL, 1);
        Ok(GlBackend {
            gl,
            shape_prog,
            shape_u,
            mesh_prog,
            mesh_u,
            tex_prog,
            tex_u,
            composite_prog,
            composite_u,
            blur_prog,
            blur_u,
            copy_prog,
            copy_u,
            backdrop: None,
            frame_size: (0, 0),
            dab_prog,
            dab_u,
            dab_vao,
            dab_buf,
            dab_cap: 0,
            pick_prog,
            pick_u,
            stamp_prog,
            stamp_u,
            smudge_window: None,
            smudge_scratch: None,
            readbacks: Vec::new(),
            paint_passes: 0,
            paint_fbos: HashMap::new(),
            shape_vao,
            mesh_vao,
            empty_vao,
            inst_buf,
            inst_cap: 0,
            overlay_buf,
            overlay_cap: 0,
            white,
            textures: Vec::new(),
            meshes: Vec::new(),
            max_tex,
            linear_target: None,
            linear_out: false,
            depth: 0,
            current: Prog::None,
            view: [0.0; 9],
            screen: [0.0; 9],
            px: 1.0,
            draw_calls: 0,
        })
    }

    fn tex(&self, t: Option<TexId>) -> &WebGlTexture {
        t.and_then(|t| self.textures.get(t.0 as usize)).and_then(Option::as_ref).unwrap_or(&self.white)
    }

    /// The framebuffer drawing into a paint tile's texture.
    fn paint_fbo(&mut self, tex: TexId) -> Option<WebGlFramebuffer> {
        if let Some(f) = self.paint_fbos.get(&tex.0) {
            return Some(f.clone());
        }
        let t = self.textures.get(tex.0 as usize)?.clone()?;
        let gl = &self.gl;
        let fbo = gl.create_framebuffer()?;
        gl.bind_framebuffer(GL::FRAMEBUFFER, Some(&fbo));
        gl.framebuffer_texture_2d(GL::FRAMEBUFFER, GL::COLOR_ATTACHMENT0, GL::TEXTURE_2D, Some(&t), 0);
        gl.bind_framebuffer(GL::FRAMEBUFFER, None);
        self.paint_fbos.insert(tex.0, fbo.clone());
        Some(fbo)
    }

    /// A render-target texture of `size`² (smudge window or scratch).
    fn target_texture(&self, size: u32) -> Option<(WebGlTexture, WebGlFramebuffer)> {
        let gl = &self.gl;
        let t = gl.create_texture()?;
        gl.bind_texture(GL::TEXTURE_2D, Some(&t));
        let _ = gl.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_u8_array(
            GL::TEXTURE_2D, 0, GL::RGBA as i32, size as i32, size as i32, 0, GL::RGBA, GL::UNSIGNED_BYTE, None,
        );
        for (k, v) in [(GL::TEXTURE_MIN_FILTER, GL::LINEAR), (GL::TEXTURE_MAG_FILTER, GL::LINEAR), (GL::TEXTURE_WRAP_S, GL::CLAMP_TO_EDGE), (GL::TEXTURE_WRAP_T, GL::CLAMP_TO_EDGE)] {
            gl.tex_parameteri(GL::TEXTURE_2D, k, v as i32);
        }
        let f = gl.create_framebuffer()?;
        gl.bind_framebuffer(GL::FRAMEBUFFER, Some(&f));
        gl.framebuffer_texture_2d(GL::FRAMEBUFFER, GL::COLOR_ATTACHMENT0, GL::TEXTURE_2D, Some(&t), 0);
        gl.bind_framebuffer(GL::FRAMEBUFFER, None);
        Some((t, f))
    }

    /// Bind the tip, grain and tint textures to units 1–3 and set the style
    /// uniforms shared by the dab and smudge shaders (`u` lists them first).
    fn bind_style(&self, u: &Uniforms, st: &DabStyle) {
        let gl = &self.gl;
        let flags = st.tip.is_some() as i32 | (st.tip_color as i32) << 1 | (st.grain.is_some() as i32) << 2 | (st.tint.is_some() as i32) << 3;
        gl.uniform1i(u.get(0), flags);
        gl.uniform2f(u.get(1), st.tip_extent[0], st.tip_extent[1]);
        gl.uniform2f(u.get(2), st.origin[0], st.origin[1]);
        gl.uniform4f(u.get(3), st.grain_size[0], st.grain_size[1], st.tint_size[0], st.tint_size[1]);
        gl.uniform2f(u.get(4), st.grain_strength, st.tint_strength);
        for (i, t) in [st.tip, st.grain, st.tint].into_iter().enumerate() {
            gl.active_texture(GL::TEXTURE1 + i as u32);
            gl.bind_texture(GL::TEXTURE_2D, Some(self.tex(t)));
            gl.uniform1i(u.get(5 + i), 1 + i as i32);
        }
        gl.active_texture(GL::TEXTURE0);
        let [g, t] = crate::backend::texture_lods(st);
        gl.uniform2f(u.get(8), g, t);
        gl.uniform2f(u.get(9), st.tip_px[0], st.tip_px[1]);
    }

    fn bind_shape_attribs(&self, start: u32) {
        let gl = &self.gl;
        let base = start as i32 * STRIDE;
        for i in 0..13u32 {
            gl.vertex_attrib_pointer_with_i32(i, 4, GL::FLOAT, false, STRIDE, base + (i as i32) * 16);
        }
    }

    fn use_shape(&mut self) {
        if self.current == Prog::Shape {
            return;
        }
        let gl = &self.gl;
        gl.use_program(Some(&self.shape_prog));
        gl.bind_vertex_array(Some(&self.shape_vao));
        gl.uniform_matrix3fv_with_f32_array(self.shape_u.get(0), false, &self.view);
        gl.uniform1f(self.shape_u.get(1), self.px);
        gl.uniform1i(self.shape_u.get(2), 0);
        gl.uniform1f(self.shape_u.get(3), if self.linear_out { 1.0 } else { 0.0 });
        gl.active_texture(GL::TEXTURE0);
        self.current = Prog::Shape;
    }

    /// Bind (creating or resizing) the linear-blending target.
    fn bind_linear_target(&mut self, w: i32, h: i32) -> bool {
        let gl = self.gl.clone();
        if self.linear_target.as_ref().is_some_and(|t| t.w != w || t.h != h) {
            if let Some(t) = self.linear_target.take() {
                gl.delete_framebuffer(Some(&t.msaa_fbo));
                gl.delete_renderbuffer(Some(&t.msaa_color));
                gl.delete_renderbuffer(Some(&t.msaa_depth));
                gl.delete_framebuffer(Some(&t.resolve_fbo));
                gl.delete_texture(Some(&t.resolve_tex));
            }
        }
        if self.linear_target.is_none() {
            let samples = gl.get_parameter(GL::MAX_SAMPLES).ok().and_then(|v| v.as_f64()).unwrap_or(0.0).min(4.0) as i32;
            let (Some(msaa_fbo), Some(msaa_color), Some(msaa_depth), Some(resolve_fbo), Some(resolve_tex)) =
                (gl.create_framebuffer(), gl.create_renderbuffer(), gl.create_renderbuffer(), gl.create_framebuffer(), gl.create_texture())
            else {
                return false;
            };
            gl.bind_renderbuffer(GL::RENDERBUFFER, Some(&msaa_color));
            gl.renderbuffer_storage_multisample(GL::RENDERBUFFER, samples, GL::SRGB8_ALPHA8, w, h);
            gl.bind_renderbuffer(GL::RENDERBUFFER, Some(&msaa_depth));
            gl.renderbuffer_storage_multisample(GL::RENDERBUFFER, samples, GL::DEPTH24_STENCIL8, w, h);
            gl.bind_framebuffer(GL::FRAMEBUFFER, Some(&msaa_fbo));
            gl.framebuffer_renderbuffer(GL::FRAMEBUFFER, GL::COLOR_ATTACHMENT0, GL::RENDERBUFFER, Some(&msaa_color));
            gl.framebuffer_renderbuffer(GL::FRAMEBUFFER, GL::DEPTH_STENCIL_ATTACHMENT, GL::RENDERBUFFER, Some(&msaa_depth));
            let ok_msaa = gl.check_framebuffer_status(GL::FRAMEBUFFER) == GL::FRAMEBUFFER_COMPLETE;
            gl.bind_texture(GL::TEXTURE_2D, Some(&resolve_tex));
            gl.tex_storage_2d(GL::TEXTURE_2D, 1, GL::SRGB8_ALPHA8, w, h);
            gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MIN_FILTER, GL::NEAREST as i32);
            gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MAG_FILTER, GL::NEAREST as i32);
            gl.bind_framebuffer(GL::FRAMEBUFFER, Some(&resolve_fbo));
            gl.framebuffer_texture_2d(GL::FRAMEBUFFER, GL::COLOR_ATTACHMENT0, GL::TEXTURE_2D, Some(&resolve_tex), 0);
            let ok_resolve = gl.check_framebuffer_status(GL::FRAMEBUFFER) == GL::FRAMEBUFFER_COMPLETE;
            gl.bind_framebuffer(GL::FRAMEBUFFER, None);
            let t = LinearTarget { w, h, msaa_fbo, msaa_color, msaa_depth, resolve_fbo, resolve_tex };
            if !(ok_msaa && ok_resolve) {
                gl.delete_framebuffer(Some(&t.msaa_fbo));
                gl.delete_renderbuffer(Some(&t.msaa_color));
                gl.delete_renderbuffer(Some(&t.msaa_depth));
                gl.delete_framebuffer(Some(&t.resolve_fbo));
                gl.delete_texture(Some(&t.resolve_tex));
                return false;
            }
            self.linear_target = Some(t);
        }
        let t = self.linear_target.as_ref().unwrap();
        gl.bind_framebuffer(GL::FRAMEBUFFER, Some(&t.msaa_fbo));
        true
    }

    /// Resolve the linear target and encode it onto the canvas.
    fn composite_linear_target(&mut self) {
        let gl = self.gl.clone();
        let Some(t) = self.linear_target.as_ref() else { return };
        gl.bind_framebuffer(GL::READ_FRAMEBUFFER, Some(&t.msaa_fbo));
        gl.bind_framebuffer(GL::DRAW_FRAMEBUFFER, Some(&t.resolve_fbo));
        gl.blit_framebuffer(0, 0, t.w, t.h, 0, 0, t.w, t.h, GL::COLOR_BUFFER_BIT, GL::NEAREST);
        gl.bind_framebuffer(GL::FRAMEBUFFER, None);
        gl.disable(GL::BLEND);
        gl.use_program(Some(&self.composite_prog));
        gl.bind_vertex_array(Some(&self.empty_vao));
        gl.active_texture(GL::TEXTURE0);
        gl.bind_texture(GL::TEXTURE_2D, Some(&t.resolve_tex));
        gl.uniform1i(self.composite_u.get(0), 0);
        gl.draw_arrays(GL::TRIANGLES, 0, 3);
        gl.enable(GL::BLEND);
        self.draw_calls += 1;
        self.current = Prog::None;
    }
}

impl Backend for GlBackend {
    fn name(&self) -> &'static str {
        "webgl2"
    }

    fn max_texture_size(&self) -> u32 {
        self.max_tex
    }

    fn upload_texture(&mut self, reuse: Option<TexId>, src: ImageSource, _w: u32, _h: u32, mipmaps: bool) -> Option<TexId> {
        let gl = &self.gl;
        let existing = reuse.and_then(|t| self.textures.get(t.0 as usize).cloned().flatten());
        let tex = match existing {
            Some(t) => t,
            None => gl.create_texture()?,
        };
        gl.bind_texture(GL::TEXTURE_2D, Some(&tex));
        let ok = match src {
            ImageSource::Image(img) => gl
                .tex_image_2d_with_u32_and_u32_and_html_image_element(GL::TEXTURE_2D, 0, GL::RGBA as i32, GL::RGBA, GL::UNSIGNED_BYTE, img)
                .is_ok(),
            ImageSource::Canvas(c) => gl
                .tex_image_2d_with_u32_and_u32_and_html_canvas_element(GL::TEXTURE_2D, 0, GL::RGBA as i32, GL::RGBA, GL::UNSIGNED_BYTE, c)
                .is_ok(),
        };
        if mipmaps {
            gl.generate_mipmap(GL::TEXTURE_2D);
            gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MIN_FILTER, GL::LINEAR_MIPMAP_LINEAR as i32);
        } else {
            gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MIN_FILTER, GL::LINEAR as i32);
        }
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MAG_FILTER, GL::LINEAR as i32);
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_S, GL::CLAMP_TO_EDGE as i32);
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_T, GL::CLAMP_TO_EDGE as i32);
        if !ok {
            gl.delete_texture(Some(&tex));
            if let Some(t) = reuse {
                if let Some(s) = self.textures.get_mut(t.0 as usize) {
                    *s = None;
                }
            }
            return None;
        }
        match reuse.filter(|t| self.textures.get(t.0 as usize).is_some_and(Option::is_some)) {
            Some(t) => Some(t),
            None => Some(TexId(slot(&mut self.textures, tex))),
        }
    }

    fn delete_texture(&mut self, t: TexId) {
        if let Some(Some(tex)) = self.textures.get_mut(t.0 as usize).map(Option::take) {
            self.gl.delete_texture(Some(&tex));
        }
        if let Some(f) = self.paint_fbos.remove(&t.0) {
            self.gl.delete_framebuffer(Some(&f));
        }
    }

    fn upload_brush_texture(&mut self, src: ImageSource, w: u32, h: u32, repeat: bool) -> Option<TexId> {
        let t = self.upload_texture(None, src, w, h, true)?;
        let gl = &self.gl;
        gl.bind_texture(GL::TEXTURE_2D, Some(self.tex(Some(t))));
        let wrap = if repeat { GL::REPEAT } else { GL::CLAMP_TO_EDGE } as i32;
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_S, wrap);
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_T, wrap);
        Some(t)
    }

    fn create_mesh(&mut self, vertices: &[f32], indices: &[u32]) -> Option<MeshId> {
        if indices.is_empty() {
            return None;
        }
        let gl = &self.gl;
        let vb = gl.create_buffer()?;
        let ib = gl.create_buffer()?;
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&vb));
        buffer_f32(gl, GL::ARRAY_BUFFER, vertices, GL::STATIC_DRAW);
        gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, Some(&ib));
        unsafe {
            let view = js_sys::Uint32Array::view(indices);
            gl.buffer_data_with_array_buffer_view(GL::ELEMENT_ARRAY_BUFFER, &view, GL::STATIC_DRAW);
        }
        Some(MeshId(slot(&mut self.meshes, (vb, ib, indices.len() as i32))))
    }

    fn delete_mesh(&mut self, m: MeshId) {
        if let Some(Some((vb, ib, _))) = self.meshes.get_mut(m.0 as usize).map(Option::take) {
            self.gl.delete_buffer(Some(&vb));
            self.gl.delete_buffer(Some(&ib));
        }
    }

    fn set_scene_instances(&mut self, data: &[f32]) {
        self.gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.inst_buf));
        upload_growable(&self.gl, GL::ARRAY_BUFFER, data, &mut self.inst_cap, GL::DYNAMIC_DRAW);
    }

    fn begin_frame(&mut self, p: &FrameParams) {
        self.draw_calls = 0;
        self.current = Prog::None;
        self.depth = 0;
        self.view = p.view;
        self.screen = p.screen;
        self.px = p.px;
        let (pw, ph) = (p.width as i32, p.height as i32);
        self.frame_size = (pw, ph);
        // Linear blending: draw the scene into an sRGB target (blends happen in
        // linear light), then encode it onto the canvas before the overlays.
        self.linear_out = p.linear && pw > 0 && ph > 0 && self.bind_linear_target(pw, ph);
        let gl = &self.gl;
        if !self.linear_out {
            gl.bind_framebuffer(GL::FRAMEBUFFER, None);
        }
        gl.viewport(0, 0, pw, ph);
        // An sRGB target takes the clear color in linear light.
        let bg = if self.linear_out { p.background.to_linear() } else { p.background };
        gl.clear_color(bg.r, bg.g, bg.b, 1.0);
        gl.clear_stencil(0);
        gl.clear(GL::COLOR_BUFFER_BIT | GL::STENCIL_BUFFER_BIT);
        gl.enable(GL::STENCIL_TEST);
        gl.stencil_func(GL::EQUAL, 0, 0xff);
        gl.stencil_op(GL::KEEP, GL::KEEP, GL::KEEP);
    }

    fn draw_shapes(&mut self, first: u32, count: u32, tex: Option<TexId>) {
        self.use_shape();
        let gl = &self.gl;
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.inst_buf));
        self.bind_shape_attribs(first);
        gl.bind_texture(GL::TEXTURE_2D, Some(self.tex(tex)));
        gl.draw_arrays_instanced(GL::TRIANGLE_STRIP, 0, 4, count as i32);
        self.draw_calls += 1;
    }

    fn clip(&mut self, parts: &[(u32, Option<TexId>)], push: bool) {
        self.use_shape();
        let gl = self.gl.clone();
        gl.color_mask(false, false, false, false);
        gl.stencil_func(GL::EQUAL, self.depth, 0xff);
        // Overlapping parts only count once: a pixel already changed fails EQUAL.
        gl.stencil_op(GL::KEEP, GL::KEEP, if push { GL::INCR } else { GL::DECR });
        for (inst, tex) in parts {
            gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.inst_buf));
            self.bind_shape_attribs(*inst);
            gl.bind_texture(GL::TEXTURE_2D, Some(self.tex(*tex)));
            gl.draw_arrays_instanced(GL::TRIANGLE_STRIP, 0, 4, 1);
            self.draw_calls += 1;
        }
        self.depth = if push { self.depth + 1 } else { (self.depth - 1).max(0) };
        gl.color_mask(true, true, true, true);
        gl.stencil_op(GL::KEEP, GL::KEEP, GL::KEEP);
        gl.stencil_func(GL::EQUAL, self.depth, 0xff);
    }

    fn backdrop_blur(&mut self, parts: &[(u32, Option<TexId>)], sigma: f32) {
        let (w, h) = self.frame_size;
        if sigma <= 0.0 || w <= 0 || h <= 0 || parts.is_empty() {
            return;
        }
        // Blur at a fraction of the size, so a few taps cover a big radius.
        let down = ((sigma / 4.0).ceil() as i32).clamp(1, 16);
        let (sw, sh) = ((w + down - 1) / down, (h + down - 1) / down);
        let srgb = self.linear_out;
        let gl = self.gl.clone();
        if !self.backdrop.as_ref().is_some_and(|b| (b.w, b.h, b.sw, b.sh, b.srgb) == (w, h, sw, sh, srgb)) {
            if let Some(old) = self.backdrop.take() {
                for (t, f) in [old.src, old.a, old.b] {
                    gl.delete_texture(Some(&t));
                    gl.delete_framebuffer(Some(&f));
                }
            }
            let (Some(src), Some(a), Some(b)) = (render_texture(&gl, w, h, srgb), render_texture(&gl, sw, sh, srgb), render_texture(&gl, sw, sh, srgb)) else { return };
            self.backdrop = Some(BackdropTargets { w, h, sw, sh, srgb, src, a, b });
        }
        let scene_fbo = if self.linear_out { self.linear_target.as_ref().map(|t| t.msaa_fbo.clone()) } else { None };
        let t = self.backdrop.as_ref().unwrap();
        // What's been drawn so far.
        gl.bind_framebuffer(GL::READ_FRAMEBUFFER, scene_fbo.as_ref());
        gl.bind_framebuffer(GL::DRAW_FRAMEBUFFER, Some(&t.src.1));
        gl.blit_framebuffer(0, 0, w, h, 0, 0, w, h, GL::COLOR_BUFFER_BIT, GL::NEAREST);
        // Blurred across, then down.
        gl.disable(GL::BLEND);
        gl.disable(GL::STENCIL_TEST);
        gl.use_program(Some(&self.blur_prog));
        gl.bind_vertex_array(Some(&self.empty_vao));
        gl.active_texture(GL::TEXTURE0);
        gl.uniform1i(self.blur_u.get(0), 0);
        gl.uniform1f(self.blur_u.get(2), sigma / down as f32);
        gl.viewport(0, 0, sw, sh);
        gl.bind_framebuffer(GL::FRAMEBUFFER, Some(&t.a.1));
        gl.bind_texture(GL::TEXTURE_2D, Some(&t.src.0));
        gl.uniform2f(self.blur_u.get(1), down as f32 / w as f32, 0.0);
        gl.draw_arrays(GL::TRIANGLES, 0, 3);
        gl.bind_framebuffer(GL::FRAMEBUFFER, Some(&t.b.1));
        gl.bind_texture(GL::TEXTURE_2D, Some(&t.a.0));
        gl.uniform2f(self.blur_u.get(1), 0.0, 1.0 / sh as f32);
        gl.draw_arrays(GL::TRIANGLES, 0, 3);
        let blurred = t.b.0.clone();
        // Back on the scene: the blurred copy, inside the layer's shape.
        gl.bind_framebuffer(GL::FRAMEBUFFER, scene_fbo.as_ref());
        gl.viewport(0, 0, w, h);
        gl.enable(GL::STENCIL_TEST);
        gl.enable(GL::BLEND);
        self.current = Prog::None;
        self.clip(parts, true);
        gl.use_program(Some(&self.copy_prog));
        gl.bind_vertex_array(Some(&self.empty_vao));
        gl.active_texture(GL::TEXTURE0);
        gl.bind_texture(GL::TEXTURE_2D, Some(&blurred));
        gl.uniform1i(self.copy_u.get(0), 0);
        gl.draw_arrays(GL::TRIANGLES, 0, 3);
        self.current = Prog::None;
        self.clip(parts, false);
        self.draw_calls += 3;
    }

    fn draw_mesh(&mut self, mesh: MeshId, m: Rows, color: [f32; 4]) {
        let Some(Some((vb, ib, count))) = self.meshes.get(mesh.0 as usize) else { return };
        let gl = &self.gl;
        if self.current != Prog::Mesh {
            gl.use_program(Some(&self.mesh_prog));
            gl.bind_vertex_array(Some(&self.mesh_vao));
            gl.uniform_matrix3fv_with_f32_array(self.mesh_u.get(0), false, &self.view);
            gl.uniform1f(self.mesh_u.get(4), if self.linear_out { 1.0 } else { 0.0 });
            self.current = Prog::Mesh;
        }
        gl.uniform3f(self.mesh_u.get(1), m.0[0], m.0[1], m.0[2]);
        gl.uniform3f(self.mesh_u.get(2), m.1[0], m.1[1], m.1[2]);
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(vb));
        gl.vertex_attrib_pointer_with_i32(0, 2, GL::FLOAT, false, 8, 0);
        gl.bind_buffer(GL::ELEMENT_ARRAY_BUFFER, Some(ib));
        gl.uniform4f(self.mesh_u.get(3), color[0], color[1], color[2], color[3]);
        gl.draw_elements_with_i32(GL::TRIANGLES, *count, GL::UNSIGNED_INT, 0);
        self.draw_calls += 1;
    }

    fn draw_texture(&mut self, tex: TexId, m: Rows, rect: [f32; 4], uv: [f32; 4], alpha: f32) {
        let gl = &self.gl;
        if self.current != Prog::Tex {
            gl.use_program(Some(&self.tex_prog));
            gl.bind_vertex_array(Some(&self.empty_vao));
            gl.uniform_matrix3fv_with_f32_array(self.tex_u.get(0), false, &self.view);
            gl.uniform1f(self.tex_u.get(6), if self.linear_out { 1.0 } else { 0.0 });
            self.current = Prog::Tex;
        }
        gl.uniform3f(self.tex_u.get(1), m.0[0], m.0[1], m.0[2]);
        gl.uniform3f(self.tex_u.get(2), m.1[0], m.1[1], m.1[2]);
        gl.uniform4f(self.tex_u.get(3), rect[0], rect[1], rect[2], rect[3]);
        gl.uniform4f(self.tex_u.get(7), uv[0], uv[1], uv[2], uv[3]);
        gl.uniform1f(self.tex_u.get(5), alpha);
        gl.bind_texture(GL::TEXTURE_2D, Some(self.tex(Some(tex))));
        gl.draw_arrays(GL::TRIANGLE_STRIP, 0, 4);
        self.draw_calls += 1;
    }

    fn end_scene(&mut self) {
        self.gl.disable(GL::STENCIL_TEST);
        if self.linear_out {
            self.composite_linear_target();
            self.linear_out = false;
        }
        self.current = Prog::None;
    }

    fn draw_overlay(&mut self, data: &[f32]) {
        if data.is_empty() {
            return;
        }
        self.use_shape();
        let gl = &self.gl;
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.overlay_buf));
        upload_growable(gl, GL::ARRAY_BUFFER, data, &mut self.overlay_cap, GL::DYNAMIC_DRAW);
        self.bind_shape_attribs(0);
        gl.bind_texture(GL::TEXTURE_2D, Some(&self.white));
        gl.draw_arrays_instanced(GL::TRIANGLE_STRIP, 0, 4, (data.len() / INST_FLOATS) as i32);
        self.draw_calls += 1;
    }

    fn draw_label(&mut self, tex: TexId, rect: [f32; 4]) {
        let gl = &self.gl;
        if self.current != Prog::Label {
            gl.use_program(Some(&self.tex_prog));
            gl.bind_vertex_array(Some(&self.empty_vao));
            gl.uniform_matrix3fv_with_f32_array(self.tex_u.get(0), false, &self.screen);
            gl.uniform3f(self.tex_u.get(1), 1.0, 0.0, 0.0);
            gl.uniform3f(self.tex_u.get(2), 0.0, 1.0, 0.0);
            gl.uniform1f(self.tex_u.get(5), 1.0);
            gl.uniform1f(self.tex_u.get(6), 0.0);
            gl.uniform4f(self.tex_u.get(7), 0.0, 0.0, 1.0, 1.0);
            self.current = Prog::Label;
        }
        gl.uniform4f(self.tex_u.get(3), rect[0], rect[1], rect[2], rect[3]);
        gl.bind_texture(GL::TEXTURE_2D, Some(self.tex(Some(tex))));
        gl.draw_arrays(GL::TRIANGLE_STRIP, 0, 4);
        self.draw_calls += 1;
    }

    fn end_frame(&mut self) {
        self.gl.bind_vertex_array(None);
        self.current = Prog::None;
    }

    fn draw_calls(&self) -> u32 {
        self.draw_calls
    }

    fn create_paint_texture(&mut self, size: u32) -> Option<TexId> {
        let gl = &self.gl;
        let tex = gl.create_texture()?;
        gl.bind_texture(GL::TEXTURE_2D, Some(&tex));
        // WebGL zero-fills: a transparent tile.
        let _ = gl.tex_image_2d_with_i32_and_i32_and_i32_and_format_and_type_and_opt_u8_array(
            GL::TEXTURE_2D,
            0,
            GL::RGBA as i32,
            size as i32,
            size as i32,
            0,
            GL::RGBA,
            GL::UNSIGNED_BYTE,
            None,
        );
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MIN_FILTER, GL::LINEAR as i32);
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MAG_FILTER, GL::LINEAR as i32);
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_S, GL::CLAMP_TO_EDGE as i32);
        gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_T, GL::CLAMP_TO_EDGE as i32);
        Some(TexId(slot(&mut self.textures, tex)))
    }

    fn load_paint_texture(&mut self, tex: TexId, src: ImageSource, _size: u32) -> bool {
        let Some(Some(t)) = self.textures.get(tex.0 as usize) else { return false };
        let gl = &self.gl;
        gl.bind_texture(GL::TEXTURE_2D, Some(t));
        match src {
            ImageSource::Image(img) => gl.tex_image_2d_with_u32_and_u32_and_html_image_element(GL::TEXTURE_2D, 0, GL::RGBA as i32, GL::RGBA, GL::UNSIGNED_BYTE, img).is_ok(),
            ImageSource::Canvas(c) => gl.tex_image_2d_with_u32_and_u32_and_html_canvas_element(GL::TEXTURE_2D, 0, GL::RGBA as i32, GL::RGBA, GL::UNSIGNED_BYTE, c).is_ok(),
        }
    }

    fn clear_paint_texture(&mut self, tex: TexId, size: u32) {
        let Some(fbo) = self.paint_fbo(tex) else { return };
        let gl = &self.gl;
        gl.bind_framebuffer(GL::FRAMEBUFFER, Some(&fbo));
        gl.viewport(0, 0, size as i32, size as i32);
        gl.disable(GL::STENCIL_TEST);
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(GL::COLOR_BUFFER_BIT);
        gl.bind_framebuffer(GL::FRAMEBUFFER, None);
    }

    fn paint_dabs(&mut self, batches: &[DabBatch]) {
        if batches.iter().all(|b| b.dabs.is_empty()) {
            return;
        }
        let gl = self.gl.clone();
        // One upload for every batch; each batch points its attributes at its part.
        let mut inst: Vec<f32> = Vec::with_capacity(batches.iter().map(|b| b.dabs.len()).sum());
        let mut first = Vec::with_capacity(batches.len());
        for b in batches {
            first.push(inst.len());
            inst.extend_from_slice(b.dabs);
        }
        gl.bind_vertex_array(Some(&self.dab_vao));
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&self.dab_buf));
        upload_growable(&gl, GL::ARRAY_BUFFER, &inst, &mut self.dab_cap, GL::DYNAMIC_DRAW);
        gl.disable(GL::STENCIL_TEST);
        gl.use_program(Some(&self.dab_prog));
        gl.uniform1f(self.dab_u.get(10), flexma_doc::raster::TILE as f32);
        let size = flexma_doc::raster::TILE as i32;
        let (mut tex, mut style, mut mode) = (None, None, None);
        for (b, start) in batches.iter().zip(first) {
            if b.dabs.is_empty() {
                continue;
            }
            if tex != Some(b.tex) {
                let Some(fbo) = self.paint_fbo(b.tex) else { continue };
                gl.bind_framebuffer(GL::FRAMEBUFFER, Some(&fbo));
                gl.viewport(0, 0, size, size);
                tex = Some(b.tex);
                self.paint_passes += 1;
            }
            if mode != Some(b.mode) {
                match b.mode {
                    PaintMode::Paint => gl.blend_func(GL::ONE, GL::ONE_MINUS_SRC_ALPHA),
                    PaintMode::Erase => gl.blend_func(GL::ZERO, GL::ONE_MINUS_SRC_ALPHA),
                    PaintMode::AlphaLock => gl.blend_func(GL::DST_ALPHA, GL::ONE_MINUS_SRC_ALPHA),
                }
                mode = Some(b.mode);
            }
            if style != Some(b.style) {
                self.bind_style(&self.dab_u, &b.style);
                style = Some(b.style);
            }
            for i in 0..3u32 {
                gl.vertex_attrib_pointer_with_i32(i, 4, GL::FLOAT, false, (DAB_FLOATS * 4) as i32, (start * 4) as i32 + (i * 16) as i32);
            }
            gl.draw_arrays_instanced(GL::TRIANGLE_STRIP, 0, 4, (b.dabs.len() / DAB_FLOATS) as i32);
        }
        gl.blend_func(GL::ONE, GL::ONE_MINUS_SRC_ALPHA);
        gl.bind_vertex_array(None);
        gl.bind_framebuffer(GL::FRAMEBUFFER, None);
        self.current = Prog::None;
    }

    fn smudge(&mut self, dabs: &[SmudgeDab]) {
        if dabs.is_empty() {
            return;
        }
        const WIN: u32 = 1024;
        self.prewarm_paint();
        let (Some((win_t, win_f)), Some((_, scr_f))) = (self.smudge_window.clone(), self.smudge_scratch.clone()) else { return };
        let gl = self.gl.clone();
        gl.disable(GL::STENCIL_TEST);
        gl.disable(GL::BLEND);
        gl.bind_vertex_array(Some(&self.empty_vao));
        for d in dabs {
            let Some(buf_f) = self.paint_fbo(d.buffer) else { continue };
            let Some(win) = window_of(d.tiles, WIN) else { continue };
            // 1. Only the pixels under the dab (plus a texel for filtering)
            // go into the window; unpainted tiles' parts are cleared.
            let reach = smudge_reach(d);
            let tile = flexma_doc::raster::TILE as f32;
            let (bx0, by0) = ((d.dab[0] - reach - 2.0).floor(), (d.dab[1] - reach - 2.0).floor());
            let (bx1, by1) = ((d.dab[0] + reach + 2.0).ceil(), (d.dab[1] + reach + 2.0).ceil());
            for t in d.tiles {
                let (x0, y0) = (bx0.max(t.origin[0]), by0.max(t.origin[1]));
                let (x1, y1) = (bx1.min(t.origin[0] + tile), by1.min(t.origin[1] + tile));
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let (sx, sy, w, h) = ((x0 - t.origin[0]) as i32, (y0 - t.origin[1]) as i32, (x1 - x0) as i32, (y1 - y0) as i32);
                let (dx, dy) = ((x0 - win[0]) as i32, (y0 - win[1]) as i32);
                match t.tex.and_then(|x| self.paint_fbo(x)) {
                    Some(src) => {
                        gl.bind_framebuffer(GL::READ_FRAMEBUFFER, Some(&src));
                        gl.bind_framebuffer(GL::DRAW_FRAMEBUFFER, Some(&win_f));
                        gl.blit_framebuffer(sx, sy, sx + w, sy + h, dx, dy, dx + w, dy + h, GL::COLOR_BUFFER_BIT, GL::NEAREST);
                    }
                    None => {
                        gl.bind_framebuffer(GL::FRAMEBUFFER, Some(&win_f));
                        gl.enable(GL::SCISSOR_TEST);
                        gl.scissor(dx, dy, w, h);
                        gl.clear_color(0.0, 0.0, 0.0, 0.0);
                        gl.clear(GL::COLOR_BUFFER_BIT);
                        gl.disable(GL::SCISSOR_TEST);
                    }
                }
            }
            // 2. Pick up: blend the paint under the dab into the carried paint.
            gl.bind_framebuffer(GL::FRAMEBUFFER, Some(&scr_f));
            gl.viewport(0, 0, SMUDGE_BUF as i32, SMUDGE_BUF as i32);
            gl.use_program(Some(&self.pick_prog));
            let u = &self.pick_u;
            gl.uniform4f(u.get(0), d.dab[0], d.dab[1], d.dab[2], d.dab[8]);
            gl.uniform2f(u.get(1), d.style.tip_extent[0], d.style.tip_extent[1]);
            gl.uniform4f(u.get(2), win[0], win[1], WIN as f32, WIN as f32);
            gl.uniform4f(u.get(3), d.paint[0], d.paint[1], d.paint[2], d.paint[3]);
            gl.uniform4f(u.get(4), d.first as i32 as f32, d.keep, d.mix, SMUDGE_BUF as f32);
            gl.active_texture(GL::TEXTURE1);
            gl.bind_texture(GL::TEXTURE_2D, Some(&win_t));
            gl.uniform1i(u.get(5), 1);
            gl.active_texture(GL::TEXTURE2);
            gl.bind_texture(GL::TEXTURE_2D, Some(self.tex(Some(d.buffer))));
            gl.uniform1i(u.get(6), 2);
            gl.active_texture(GL::TEXTURE0);
            gl.draw_arrays(GL::TRIANGLES, 0, 3);
            gl.bind_framebuffer(GL::READ_FRAMEBUFFER, Some(&scr_f));
            gl.bind_framebuffer(GL::DRAW_FRAMEBUFFER, Some(&buf_f));
            let b = SMUDGE_BUF as i32;
            gl.blit_framebuffer(0, 0, b, b, 0, 0, b, b, GL::COLOR_BUFFER_BIT, GL::NEAREST);
            // 3. Lay it down through the tip, tile by tile.
            let targets: Vec<(WebGlFramebuffer, &crate::backend::SmudgeTile)> =
                d.tiles.iter().filter(|t| t.write).filter_map(|t| Some((t.tex.and_then(|x| self.paint_fbo(x))?, t))).collect();
            gl.use_program(Some(&self.stamp_prog));
            let u = &self.stamp_u;
            self.bind_style(u, &d.style);
            for (dst, t) in targets {
                gl.bind_framebuffer(GL::FRAMEBUFFER, Some(&dst));
                self.paint_passes += 1;
                let n = flexma_doc::raster::TILE;
                gl.viewport(0, 0, n as i32, n as i32);
                gl.uniform4f(u.get(10), d.dab[0], d.dab[1], d.dab[2], d.dab[8]);
                gl.uniform4f(u.get(11), d.dab[3], d.dab[7], d.alpha_lock as i32 as f32, n as f32);
                gl.uniform4f(u.get(12), win[0], win[1], t.origin[0], t.origin[1]);
                gl.uniform4f(u.get(13), d.dab[0] - t.origin[0] - reach, d.dab[1] - t.origin[1] - reach, d.dab[0] - t.origin[0] + reach, d.dab[1] - t.origin[1] + reach);
                gl.active_texture(GL::TEXTURE4);
                gl.bind_texture(GL::TEXTURE_2D, Some(&win_t));
                gl.uniform1i(u.get(14), 4);
                gl.active_texture(GL::TEXTURE5);
                gl.bind_texture(GL::TEXTURE_2D, Some(self.tex(Some(d.buffer))));
                gl.uniform1i(u.get(15), 5);
                gl.active_texture(GL::TEXTURE0);
                gl.draw_arrays(GL::TRIANGLE_STRIP, 0, 4);
            }
        }
        gl.enable(GL::BLEND);
        gl.blend_func(GL::ONE, GL::ONE_MINUS_SRC_ALPHA);
        gl.bind_vertex_array(None);
        gl.bind_framebuffer(GL::FRAMEBUFFER, None);
        self.current = Prog::None;
    }

    fn copy_paint_texture(&mut self, src: TexId, dst: TexId, size: u32) {
        let (Some(a), Some(b)) = (self.paint_fbo(src), self.paint_fbo(dst)) else { return };
        let gl = &self.gl;
        gl.bind_framebuffer(GL::READ_FRAMEBUFFER, Some(&a));
        gl.bind_framebuffer(GL::DRAW_FRAMEBUFFER, Some(&b));
        let s = size as i32;
        gl.blit_framebuffer(0, 0, s, s, 0, 0, s, s, GL::COLOR_BUFFER_BIT, GL::NEAREST);
        gl.bind_framebuffer(GL::FRAMEBUFFER, None);
    }

    fn read_paint_texture(&mut self, tex: TexId, size: u32, done: Box<dyn FnOnce(Vec<u8>)>) {
        let Some(fbo) = self.paint_fbo(tex) else { return };
        let gl = &self.gl;
        let Some(buf) = gl.create_buffer() else { return };
        let bytes = (size * size * 4) as usize;
        gl.bind_buffer(GL::PIXEL_PACK_BUFFER, Some(&buf));
        gl.buffer_data_with_i32(GL::PIXEL_PACK_BUFFER, bytes as i32, GL::STREAM_READ);
        gl.bind_framebuffer(GL::FRAMEBUFFER, Some(&fbo));
        // Into the pack buffer: returns at once, the copy happens on the GPU.
        let _ = gl.read_pixels_with_i32(0, 0, size as i32, size as i32, GL::RGBA, GL::UNSIGNED_BYTE, 0);
        gl.bind_framebuffer(GL::FRAMEBUFFER, None);
        gl.bind_buffer(GL::PIXEL_PACK_BUFFER, None);
        let Some(sync) = gl.fence_sync(GL::SYNC_GPU_COMMANDS_COMPLETE, 0) else {
            gl.delete_buffer(Some(&buf));
            return;
        };
        gl.flush();
        self.readbacks.push(Readback { buf, sync, bytes, done });
    }

    fn poll_readbacks(&mut self) -> bool {
        let gl = self.gl.clone();
        let pending = std::mem::take(&mut self.readbacks);
        for r in pending {
            let status = gl.client_wait_sync_with_u32(&r.sync, 0, 0);
            if status == GL::TIMEOUT_EXPIRED {
                self.readbacks.push(r);
                continue;
            }
            gl.delete_sync(Some(&r.sync));
            let mut px = vec![0u8; r.bytes];
            gl.bind_buffer(GL::PIXEL_PACK_BUFFER, Some(&r.buf));
            gl.get_buffer_sub_data_with_i32_and_u8_array(GL::PIXEL_PACK_BUFFER, 0, &mut px);
            gl.bind_buffer(GL::PIXEL_PACK_BUFFER, None);
            gl.delete_buffer(Some(&r.buf));
            if status != GL::WAIT_FAILED {
                (r.done)(px);
            }
        }
        !self.readbacks.is_empty()
    }

    fn prewarm_paint(&mut self) {
        // The programs compile at startup; the smudge targets are made here.
        if self.smudge_window.is_none() {
            self.smudge_window = self.target_texture(1024);
            self.smudge_scratch = self.target_texture(SMUDGE_BUF);
        }
    }

    fn paint_counters(&self) -> [u64; 3] {
        [self.paint_passes, 0, 0]
    }
}

pub const SHAPE_VS: &str = r#"#version 300 es
precision highp float;
layout(location=0) in vec4 a_m0;
layout(location=1) in vec4 a_m1;
layout(location=2) in vec4 a_size;
layout(location=3) in vec4 a_radii;
layout(location=4) in vec4 a_c0;
layout(location=5) in vec4 a_c1;
layout(location=6) in vec4 a_c2;
layout(location=7) in vec4 a_c3;
layout(location=8) in vec4 a_stops;
layout(location=9) in vec4 a_paint;
layout(location=10) in vec4 a_stroke;
layout(location=11) in vec4 a_strokep;
layout(location=12) in vec4 a_fx;
uniform mat3 u_view;
uniform float u_px;
out vec2 v_local;
flat out vec4 v_size;
flat out vec4 v_radii;
flat out vec4 v_c0;
flat out vec4 v_c1;
flat out vec4 v_c2;
flat out vec4 v_c3;
flat out vec4 v_stops;
flat out vec4 v_paint;
flat out vec4 v_stroke;
flat out vec4 v_strokep;
flat out vec4 v_fx;
void main() {
  vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
  // Expand by the stroke/shadow extent plus 1.5 device px for the AA fringe.
  float pad = a_strokep.z + 1.5 / u_px;
  vec2 local = mix(vec2(-pad), a_size.xy + vec2(pad), corner);
  vec2 world = vec2(a_m0.x * local.x + a_m0.y * local.y + a_m0.z,
                    a_m1.x * local.x + a_m1.y * local.y + a_m1.z);
  vec3 clip = u_view * vec3(world, 1.0);
  gl_Position = vec4(clip.xy, 0.0, 1.0);
  v_local = local;
  v_size = a_size; v_radii = a_radii; v_c0 = a_c0; v_c1 = a_c1; v_c2 = a_c2; v_c3 = a_c3;
  v_stops = a_stops; v_paint = a_paint; v_stroke = a_stroke; v_strokep = a_strokep; v_fx = a_fx;
}
"#;

pub const SHAPE_FS: &str = r#"#version 300 es
precision highp float;
in vec2 v_local;
flat in vec4 v_size;
flat in vec4 v_radii;
flat in vec4 v_c0;
flat in vec4 v_c1;
flat in vec4 v_c2;
flat in vec4 v_c3;
flat in vec4 v_stops;
flat in vec4 v_paint;
flat in vec4 v_stroke;
flat in vec4 v_strokep;
flat in vec4 v_fx;
uniform float u_px;
uniform sampler2D u_tex;
out vec4 o;
// Linear blending (u_linear): outputs go to an sRGB target in linear light.
uniform float u_linear;
vec3 srgbToLinear(vec3 c) { return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c)); }
vec4 toOutput(vec4 c) {
  if (u_linear < 0.5 || c.a <= 0.0) return c;
  return vec4(srgbToLinear(clamp(c.rgb / c.a, 0.0, 1.0)) * c.a, c.a);
}

float sdRoundBox(vec2 p, vec2 b, vec4 rad) {
  float r = p.x > 0.0 ? (p.y > 0.0 ? rad.z : rad.y) : (p.y > 0.0 ? rad.w : rad.x);
  vec2 q = abs(p) - b + r;
  return min(max(q.x, q.y), 0.0) + length(max(q, 0.0)) - r;
}
float sdEllipse(vec2 p, vec2 ab) {
  vec2 pa = p / ab;
  float k0 = length(pa);
  float k1 = length(pa / ab);
  if (k0 < 1e-5) return -min(ab.x, ab.y);
  return k0 * (k0 - 1.0) / k1;
}
vec4 gradient(vec2 uv) {
  vec2 dir = vec2(cos(v_paint.y), sin(v_paint.y));
  vec2 d = uv - 0.5;
  // Project onto the gradient axis spanning the box diagonal extent.
  float ext = abs(dir.x) * 0.5 + abs(dir.y) * 0.5;
  float t = clamp(dot(d, dir) / (2.0 * ext) + 0.5, 0.0, 1.0);
  int n = int(v_paint.z);
  if (t <= v_stops.x || n < 2) return v_c0;
  if (t <= v_stops.y) return mix(v_c0, v_c1, (t - v_stops.x) / max(v_stops.y - v_stops.x, 1e-5));
  if (n < 3) return v_c1;
  if (t <= v_stops.z) return mix(v_c1, v_c2, (t - v_stops.y) / max(v_stops.z - v_stops.y, 1e-5));
  if (n < 4) return v_c2;
  if (t <= v_stops.w) return mix(v_c2, v_c3, (t - v_stops.z) / max(v_stops.w - v_stops.z, 1e-5));
  return v_c3;
}
// One axis of a 9-slice: layer position x in [0, size] to a texture coordinate.
// a, b = source insets (image px) at the start/end; da, db = the same in layer px.
float slice1(float x, float size, float img, float a, float b, float da, float db) {
  if (x <= da) return (da > 0.0 ? x / da : 0.0) * a / img;
  if (x >= size - db) return 1.0 - (db > 0.0 ? (size - x) / db : 0.0) * b / img;
  return (a + (x - da) / max(size - da - db, 1e-4) * (img - a - b)) / img;
}
// Unity's Filled image: coverage of layer position p in a box of size sz.
// v_stops = (method: 1 horizontal, 2 vertical, 3/4/5 radial 90/180/360,
// amount, origin as Unity's fillOrigin, clockwise).
float filledCoverage(vec2 p, vec2 sz, float aa) {
  int m = int(v_stops.x + 0.5);
  if (m == 0) return 1.0;
  float a = clamp(v_stops.y, 0.0, 1.0);
  if (a <= 0.0) return 0.0;
  if (a >= 1.0) return 1.0;
  int o = int(v_stops.z + 0.5);
  bool cw = v_stops.w > 0.5;
  if (m == 1) return clamp(0.5 + (o == 0 ? a * sz.x - p.x : p.x - (1.0 - a) * sz.x) / aa, 0.0, 1.0);
  if (m == 2) return clamp(0.5 + (o == 0 ? p.y - (1.0 - a) * sz.y : a * sz.y - p.y) / aa, 0.0, 1.0);
  // Radial: a pivot, the direction a clockwise sweep starts at (degrees
  // clockwise from 12 o'clock) and the span of directions it covers.
  vec2 pivot;
  float start;
  float span;
  if (m == 3) {
    // Corner: bottom-left, top-left, top-right, bottom-right.
    pivot = o == 0 ? vec2(0.0, sz.y) : o == 1 ? vec2(0.0) : o == 2 ? vec2(sz.x, 0.0) : sz;
    start = float(o) * 90.0;
    span = 90.0;
  } else if (m == 4) {
    // Edge middle: bottom, left, top, right.
    pivot = o == 0 ? vec2(sz.x * 0.5, sz.y) : o == 1 ? vec2(0.0, sz.y * 0.5) : o == 2 ? vec2(sz.x * 0.5, 0.0) : vec2(sz.x, sz.y * 0.5);
    start = o == 0 ? 270.0 : o == 1 ? 0.0 : o == 2 ? 90.0 : 180.0;
    span = 180.0;
  } else {
    // Center, starting at the bottom, right, top or left.
    pivot = sz * 0.5;
    start = o == 0 ? 180.0 : o == 1 ? 90.0 : o == 2 ? 0.0 : 270.0;
    span = 360.0;
  }
  // The shown sweep [lo, hi] within the span. Counter-clockwise fills grow
  // from the span's far end (for 360°, back from the start).
  float lo = cw ? 0.0 : span * (1.0 - a);
  float hi = cw ? span * a : span;
  float hw = (hi - lo) * 0.5;
  vec2 d = p - pivot;
  float r = length(d);
  float ang = degrees(atan(d.x, -d.y));
  float rel = mod(ang - start - lo - hw + 540.0, 360.0) - 180.0;
  // Signed px distance to the cut edges (positive inside); a span edge that
  // is also the box edge isn't a cut.
  bool full = span > 359.0;
  float sdLo = (full || !cw) ? r * sin(radians(clamp(rel + hw, -90.0, 90.0))) : 1e6;
  float sdHi = (full || cw) ? r * sin(radians(clamp(hw - rel, -90.0, 90.0))) : 1e6;
  return clamp(0.5 + min(sdLo, sdHi) / aa, 0.0, 1.0);
}
// An image lookup at tuv in [0, 1] over the image, or over its crop when
// v_fx = crop rect in texture space (x, y, w, h). Lookups stay half a texel
// inside the crop so neighbouring sprites in an atlas don't bleed in.
vec4 sampleImage(vec2 tuv, vec2 gx, vec2 gy) {
  if (v_fx.z <= 0.0) return textureGrad(u_tex, tuv, gx, gy);
  vec2 half_texel = 0.5 / vec2(textureSize(u_tex, 0));
  vec2 lo = v_fx.xy + half_texel;
  vec2 hi = max(lo, v_fx.xy + v_fx.zw - half_texel);
  return textureGrad(u_tex, clamp(v_fx.xy + tuv * v_fx.zw, lo, hi), gx * v_fx.zw, gy * v_fx.zw);
}
// d(slice1)/dx, for explicit texture gradients.
float slice1d(float x, float size, float img, float a, float b, float da, float db) {
  if (x <= da) return da > 0.0 ? a / da / img : 0.0;
  if (x >= size - db) return db > 0.0 ? b / db / img : 0.0;
  return (img - a - b) / max(size - da - db, 1e-4) / img;
}
// An image fill at layer position `local`: v_c0 = tint, v_c1 = (imgW, imgH,
// fit, alpha), 9-slice when v_c3.z is set. gx, gy: layer-space screen
// derivatives, which pick the mip level.
vec4 imageFill(vec2 local, vec2 gx, vec2 gy, float aa) {
  vec4 fill = vec4(0.0);
  vec2 img = max(v_c1.xy, vec2(1.0));
  vec2 sz = max(v_size.xy, vec2(1e-4));
  if (v_c3.z > 0.5) {
    // v_c2 = source insets (top, right, bottom, left) in image px, v_c3.x = layer px per image px.
    vec2 a = v_c2.wx;
    vec2 b = v_c2.yz;
    vec2 da = a * v_c3.x;
    vec2 db = b * v_c3.x;
    vec2 k = min(vec2(1.0), sz / max(da + db, vec2(1e-4)));
    da *= k;
    db *= k;
    vec2 p = clamp(local, vec2(0.0), sz);
    vec2 tuv = vec2(slice1(p.x, sz.x, img.x, a.x, b.x, da.x, db.x), slice1(p.y, sz.y, img.y, a.y, b.y, da.y, db.y));
    vec2 f = vec2(slice1d(p.x, sz.x, img.x, a.x, b.x, da.x, db.x), slice1d(p.y, sz.y, img.y, a.y, b.y, da.y, db.y));
    bool center = all(greaterThan(p, da)) && all(lessThan(p, sz - db));
    if (v_c3.y > 0.5 || !center) fill = sampleImage(tuv, gx * f, gy * f) * v_c0 * v_c1.w;
  } else {
    float fit = v_c1.z;
    if (fit > 2.5) {
      // Tile: repeat at v_c3.w layer px per image px from the bottom-left, like Unity.
      vec2 t = img * max(v_c3.w, 1e-4);
      vec2 q = vec2(local.x, sz.y - local.y) / t;
      fill = sampleImage(vec2(fract(q.x), 1.0 - fract(q.y)), gx / t, gy / t) * v_c0 * v_c1.w;
    } else {
      vec2 scale;
      if (fit < 0.5) { float s = max(sz.x / img.x, sz.y / img.y); scale = img * s; }
      else if (fit < 1.5) { float s = min(sz.x / img.x, sz.y / img.y); scale = img * s; }
      else { scale = sz; }
      vec2 off = (sz - scale) * 0.5;
      vec2 tuv = (local - off) / scale;
      if (tuv.x >= 0.0 && tuv.y >= 0.0 && tuv.x <= 1.0 && tuv.y <= 1.0) fill = sampleImage(tuv, gx / scale, gy / scale) * v_c0 * v_c1.w;
    }
  }
  fill *= filledCoverage(local, sz, aa);
  return fill;
}
void shade();
void main() {
  shade();
  o = toOutput(o);
}
void shade() {
  // Screen derivatives of the layer position, taken in uniform control flow:
  // image lookups below sit in per-pixel branches, where implicit derivatives
  // are undefined and pick a blurry mip at the edges (which leaks through alpha masks).
  vec2 dlx = dFdx(v_local);
  vec2 dly = dFdy(v_local);
  vec2 hs = max(v_size.xy * 0.5, vec2(1e-4));
  vec2 p = v_local - hs;
  float d = v_size.z > 0.5 ? sdEllipse(p, hs) : sdRoundBox(p, hs, v_radii);
  float aa = 1.0 / u_px;
  float mode = v_size.w;
  if (mode > 3.5) {
    // Drop shadow of an image: its alpha silhouette, grown by spread
    // (v_strokep.y, the most of 9 taps around) and blurred (v_strokep.x, by
    // sampling a blurrier mip), in the shadow color v_stroke.
    float bl = max(v_strokep.x, 0.0);
    float sp = v_strokep.y;
    vec2 g1 = vec2(max(bl * 0.5, length(dlx)), 0.0);
    vec2 g2 = vec2(0.0, max(bl * 0.5, length(dly)));
    float a = sp < 0.0 ? 1.0 : 0.0;
    for (int i = 0; i < 9; i++) {
      float t = float(i) * 0.785398;
      vec2 q = v_local + (i == 0 ? vec2(0.0) : abs(sp) * vec2(cos(t), sin(t)));
      // Past the layer the silhouette fades out over the blur.
      vec2 qc = clamp(q, vec2(0.0), v_size.xy);
      float fade = 1.0 - smoothstep(0.0, bl + aa, length(q - qc));
      float inside = (v_size.z > 0.5 ? sdEllipse(qc - hs, hs) : sdRoundBox(qc - hs, hs, v_radii)) <= 0.5 ? 1.0 : 0.0;
      float ta = imageFill(qc, g1, g2, aa).a * fade * inside;
      a = sp < 0.0 ? min(a, ta) : max(a, ta);
    }
    o = v_stroke * a * v_paint.w;
    return;
  }
  bool alphaClip = mode > 2.5;
  if (mode > 1.5 && !alphaClip) {
    // Stencil clip shape.
    if (d > 0.0) discard;
    o = vec4(0.0);
    return;
  }
  if (mode > 0.5 && !alphaClip) {
    // Drop shadow: blurred SDF.
    float blur = max(v_fx.z, aa);
    float sd = d - v_fx.w;
    float a = 1.0 - smoothstep(-blur * 0.5, blur * 0.5, sd);
    o = v_c0 * a * v_paint.w;
    return;
  }
  vec4 fill = vec4(0.0);
  int ft = int(v_paint.x + 0.5);
  vec2 uv = v_local / max(v_size.xy, vec2(1e-4));
  if (ft == 1) fill = v_c0;
  else if (ft == 2) fill = gradient(uv);
  else if (ft == 3) fill = imageFill(v_local, dlx, dly, aa);
  if (alphaClip) {
    // Stencil alpha mask: keep the pixels where the fill isn't transparent
    // (above Unity's Mask cutoff of 0.001).
    if (d > 0.0 || fill.a * v_paint.w <= 0.001) discard;
    o = vec4(0.0);
    return;
  }
  float cov = clamp(0.5 - d / aa, 0.0, 1.0);
  vec4 col = fill * cov;
  float sw = v_strokep.x;
  if (sw > 0.0 && v_stroke.a > 0.0) {
    float align = v_strokep.y;
    float sd;
    if (align < 0.5) sd = abs(d) - sw * 0.5;
    else if (align < 1.5) sd = abs(d + sw * 0.5) - sw * 0.5;
    else sd = abs(d - sw * 0.5) - sw * 0.5;
    float scov;
    if (sw < aa) {
      // Hairline: keep a 1px line, fade by thickness.
      float center = align < 0.5 ? d : (align < 1.5 ? d + sw * 0.5 : d - sw * 0.5);
      scov = clamp(1.0 - abs(center) / aa, 0.0, 1.0) * (sw / aa);
    } else {
      scov = clamp(0.5 - sd / aa, 0.0, 1.0);
    }
    vec4 s = v_stroke * scov;
    col = s + col * (1.0 - s.a);
  }
  o = col * v_paint.w;
}
"#;

pub const MESH_VS: &str = r#"#version 300 es
precision highp float;
layout(location=0) in vec2 a_pos;
uniform mat3 u_view;
uniform vec3 u_m0;
uniform vec3 u_m1;
void main() {
  vec2 world = vec2(dot(u_m0, vec3(a_pos, 1.0)), dot(u_m1, vec3(a_pos, 1.0)));
  vec3 clip = u_view * vec3(world, 1.0);
  gl_Position = vec4(clip.xy, 0.0, 1.0);
}
"#;

pub const MESH_FS: &str = r#"#version 300 es
precision highp float;
uniform vec4 u_color;
out vec4 o;
// Linear blending (u_linear): outputs go to an sRGB target in linear light.
uniform float u_linear;
vec3 srgbToLinear(vec3 c) { return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c)); }
vec4 toOutput(vec4 c) {
  if (u_linear < 0.5 || c.a <= 0.0) return c;
  return vec4(srgbToLinear(clamp(c.rgb / c.a, 0.0, 1.0)) * c.a, c.a);
}
void main() { o = toOutput(u_color); }
"#;

pub const TEX_VS: &str = r#"#version 300 es
precision highp float;
uniform mat3 u_view;
uniform vec3 u_m0;
uniform vec3 u_m1;
uniform vec4 u_rect;
// The texture region shown: (x0, y0, x1, y1).
uniform vec4 u_uv;
out vec2 v_uv;
void main() {
  vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
  vec2 local = u_rect.xy + corner * u_rect.zw;
  vec2 world = vec2(dot(u_m0, vec3(local, 1.0)), dot(u_m1, vec3(local, 1.0)));
  vec3 clip = u_view * vec3(world, 1.0);
  gl_Position = vec4(clip.xy, 0.0, 1.0);
  v_uv = mix(u_uv.xy, u_uv.zw, corner);
}
"#;

pub const TEX_FS: &str = r#"#version 300 es
precision highp float;
in vec2 v_uv;
uniform sampler2D u_tex;
uniform float u_alpha;
out vec4 o;
// Linear blending (u_linear): outputs go to an sRGB target in linear light.
uniform float u_linear;
vec3 srgbToLinear(vec3 c) { return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c)); }
vec4 toOutput(vec4 c) {
  if (u_linear < 0.5 || c.a <= 0.0) return c;
  return vec4(srgbToLinear(clamp(c.rgb / c.a, 0.0, 1.0)) * c.a, c.a);
}
void main() { o = toOutput(texture(u_tex, v_uv) * u_alpha); }
"#;

/// The window (x, y in layer px) a smudge dab's tiles are copied into.
pub fn window_of(tiles: &[crate::backend::SmudgeTile], max: u32) -> Option<[f32; 2]> {
    let x = tiles.iter().map(|t| t.origin[0]).fold(f32::MAX, f32::min);
    let y = tiles.iter().map(|t| t.origin[1]).fold(f32::MAX, f32::min);
    let fits = tiles.iter().all(|t| t.origin[0] - x + flexma_doc::raster::TILE as f32 <= max as f32 && t.origin[1] - y + flexma_doc::raster::TILE as f32 <= max as f32);
    (!tiles.is_empty() && fits).then_some([x, y])
}

/// How far a smudge dab reaches from its center (px).
pub fn smudge_reach(d: &SmudgeDab) -> f32 {
    let e = d.style.tip_extent;
    let k = if d.style.tip.is_some() { (e[0] * e[0] + e[1] * e[1]).sqrt() } else { 1.0 };
    d.dab[2].max(0.5) * k + 1.5
}

/// Uniforms shared by the dab and stamp shaders (`bind_style` sets 0–9).
const STYLE_GLSL: &str = r#"
uniform int u_flags;
uniform vec2 u_extent;
uniform vec2 u_origin;
uniform vec4 u_tex_size;
uniform vec2 u_strength;
uniform sampler2D u_tip;
uniform sampler2D u_grain;
uniform sampler2D u_tint;
uniform vec2 u_lods;
uniform vec2 u_tip_px;

// Coverage of a dab (center c, radius r, angle a, hardness h) at layer point
// p, with the tip color in `tipc` (premultiplied) when the tip has one.
float cover(vec2 p, vec2 c, float r, float a, float h, out vec4 tipc, out vec2 uv) {
  tipc = vec4(1.0);
  float area = min(1.0, r * r / 0.25);
  vec2 d = p - c;
  vec2 q = vec2(cos(a) * d.x + sin(a) * d.y, -sin(a) * d.x + cos(a) * d.y);
  uv = q / (u_extent * max(r, 0.5)) * 0.5 + 0.5;
  if ((u_flags & 1) == 0) {
    float rr = max(r, 0.5);
    float dist = length(d);
    float inner = rr * clamp(h, 0.0, 1.0);
    float soft = 1.0 - smoothstep(inner, max(rr, inner + 0.001), dist);
    float rim = clamp(rr - dist + 0.5, 0.0, 1.0);
    return soft * rim * area;
  }
  vec2 inside = step(vec2(0.0), uv) * step(uv, vec2(1.0));
  // The mip level where a texel is about a pixel (the same on every GPU).
  vec2 span = u_tip_px / (2.0 * u_extent * max(r, 0.5));
  tipc = textureLod(u_tip, uv, max(log2(max(span.x, span.y)), 0.0));
  float k = (u_flags & 2) != 0 ? tipc.a : tipc.r;
  return k * inside.x * inside.y * area;
}

// Paper grain: paint catches on the paper's high spots.
float grain(vec2 p) {
  if ((u_flags & 4) == 0) return 1.0;
  float g = textureLod(u_grain, p / u_tex_size.xy, u_lods.x).r;
  return clamp(1.0 - u_strength.x * (1.0 - g) * 1.6, 0.0, 1.0);
}

// Color variation around the brush color (premultiplied in and out).
vec4 tint(vec4 c, vec2 p) {
  if ((u_flags & 8) == 0 || c.a <= 0.0) return c;
  vec3 v = textureLod(u_tint, p / u_tex_size.zw, u_lods.y).rgb - 0.5;
  return vec4(clamp(c.rgb / c.a + v * 2.0 * u_strength.y, 0.0, 1.0) * c.a, c.a);
}
"#;

const DAB_UNIFORMS: [&str; 11] = ["u_flags", "u_extent", "u_origin", "u_tex_size", "u_strength", "u_tip", "u_grain", "u_tint", "u_lods", "u_tip_px", "u_size"];

/// Brush dabs into a paint tile: x, y, radius, hardness (texel space), a
/// premultiplied color and the tip angle per instance. GL framebuffers start
/// at the bottom row, which is the tile's top row (row 0), so no flip.
pub const DAB_VS: &str = r#"#version 300 es
precision highp float;
precision highp int;
layout(location=0) in vec4 a_d0;
layout(location=1) in vec4 a_col;
layout(location=2) in vec4 a_d2;
uniform float u_size;
uniform int u_flags;
uniform vec2 u_extent;
out vec2 v_p;
flat out vec4 v_d0;
flat out vec4 v_col;
flat out vec4 v_d2;
void main() {
  vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
  float r = (u_flags & 1) != 0 ? max(a_d0.z, 0.5) * length(u_extent) + 1.0 : max(a_d0.z, 0.5) + 1.0;
  vec2 p = a_d0.xy + (corner * 2.0 - 1.0) * r;
  v_p = p;
  v_d0 = a_d0;
  v_col = a_col;
  v_d2 = a_d2;
  gl_Position = vec4(p / u_size * 2.0 - 1.0, 0.0, 1.0);
}
"#;

fn dab_fs() -> String {
    format!(
        r#"#version 300 es
precision highp float;
precision highp int;
{STYLE_GLSL}
in vec2 v_p;
flat in vec4 v_d0;
flat in vec4 v_col;
flat in vec4 v_d2;
out vec4 o;
void main() {{
  if (u_flags == 0) {{
    // A plain round dab. Hardness sets where the soft falloff starts; the
    // rim is antialiased, and dabs under half a pixel fade by area instead
    // of shrinking.
    float r = max(v_d0.z, 0.5);
    float d = length(v_p - v_d0.xy);
    float inner = r * clamp(v_d0.w, 0.0, 1.0);
    float soft = 1.0 - smoothstep(inner, max(r, inner + 0.001), d);
    float rim = clamp(r - d + 0.5, 0.0, 1.0);
    float area = min(1.0, v_d0.z * v_d0.z / 0.25);
    o = v_col * (soft * rim * area);
    return;
  }}
  vec4 tipc;
  vec2 uv;
  vec2 lp = u_origin + v_p;
  float k = cover(v_p, v_d0.xy, v_d0.z, v_d2.x, v_d0.w, tipc, uv) * grain(lp);
  if ((u_flags & 2) != 0) {{
    // The tip's own colors (a photo stamp), at the dab's flow.
    float a = k * v_col.a;
    o = vec4(tipc.rgb / max(tipc.a, 1e-4) * a, a);
  }} else {{
    o = tint(v_col, lp) * k;
  }}
}}
"#
    )
}

const PICK_UNIFORMS: [&str; 7] = ["u_dab", "u_extent", "u_win", "u_paint", "u_params", "u_window", "u_prev"];

/// Smudge pick-up, into the scratch buffer: the paint under the dab (in the
/// tip's frame) blended into what the stroke carries.
fn pick_fs() -> String {
    r#"#version 300 es
precision highp float;
uniform vec4 u_dab;
uniform vec2 u_extent;
uniform vec4 u_win;
uniform vec4 u_paint;
uniform vec4 u_params;
uniform sampler2D u_window;
uniform sampler2D u_prev;
out vec4 o;
void main() {
  vec2 uv = gl_FragCoord.xy / u_params.w;
  vec2 q = (uv * 2.0 - 1.0) * u_extent * max(u_dab.z, 0.5);
  float c = cos(u_dab.w), s = sin(u_dab.w);
  vec2 p = u_dab.xy + vec2(c * q.x - s * q.y, s * q.x + c * q.y);
  vec4 under = texture(u_window, (p - u_win.xy) / u_win.zw);
  vec4 carried = u_params.x > 0.5 ? under : mix(under, texture(u_prev, uv), u_params.y);
  o = mix(carried, u_paint, u_params.z);
}
"#
    .to_string()
}

const STAMP_UNIFORMS: [&str; 16] = [
    "u_flags", "u_extent", "u_origin", "u_tex_size", "u_strength", "u_tip", "u_grain", "u_tint", "u_lods", "u_tip_px", "u_dab", "u_look", "u_where", "u_quad", "u_window", "u_buf",
];

/// Smudge lay-down: a quad over the dab within one tile (`u_quad`, tile px).
pub const STAMP_VS: &str = r#"#version 300 es
precision highp float;
uniform vec4 u_quad;
uniform vec4 u_look;
void main() {
  vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
  vec2 p = mix(u_quad.xy, u_quad.zw, corner);
  gl_Position = vec4(p / u_look.w * 2.0 - 1.0, 0.0, 1.0);
}
"#;

/// Each pixel becomes the carried paint, by the tip's coverage; everything
/// else is copied exactly (no blending: the old pixel comes from the window).
fn stamp_fs() -> String {
    format!(
        r#"#version 300 es
precision highp float;
precision highp int;
{STYLE_GLSL}
uniform vec4 u_dab;
uniform vec4 u_look;
uniform vec4 u_where;
uniform sampler2D u_window;
uniform sampler2D u_buf;
out vec4 o;
void main() {{
  vec2 p = u_where.zw + gl_FragCoord.xy;
  vec4 dst = texelFetch(u_window, ivec2(p - u_where.xy), 0);
  vec4 tipc;
  vec2 uv;
  float k = cover(p, u_dab.xy, u_dab.z, u_dab.w, u_look.x, tipc, uv) * u_look.y * grain(p);
  vec4 r = mix(dst, textureLod(u_buf, uv, 0.0), clamp(k, 0.0, 1.0));
  if (u_look.z > 0.5) r = r.a > 0.0 ? r * (dst.a / r.a) : vec4(0.0);
  o = r;
}}
"#
    )
}

/// Background blur: one direction of a Gaussian, `u_step` apart (σ in steps).
pub const BLUR_FS: &str = r#"#version 300 es
precision highp float;
in vec2 v_uv;
uniform sampler2D u_tex;
uniform vec2 u_step;
uniform float u_sigma;
out vec4 o;
void main() {
  float s = max(u_sigma, 0.5);
  vec4 acc = vec4(0.0);
  float total = 0.0;
  for (int i = -16; i <= 16; i++) {
    float x = float(i);
    float wt = exp(-0.5 * x * x / (s * s));
    acc += texture(u_tex, v_uv + u_step * x) * wt;
    total += wt;
  }
  o = acc / total;
}
"#;

/// A texture as is (the blurred backdrop onto the scene).
pub const COPY_FS: &str = r#"#version 300 es
precision highp float;
in vec2 v_uv;
uniform sampler2D u_tex;
out vec4 o;
void main() {
  o = texture(u_tex, v_uv);
}
"#;

/// Full-screen pass that encodes a linear-light sRGB target for the canvas.
pub const COMPOSITE_VS: &str = r#"#version 300 es
out vec2 v_uv;
void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  v_uv = p;
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

pub const COMPOSITE_FS: &str = r#"#version 300 es
precision highp float;
in vec2 v_uv;
uniform sampler2D u_tex;
out vec4 o;
vec3 linearToSrgb(vec3 c) { return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c)); }
void main() {
  // The sRGB texture samples as linear light; the canvas wants sRGB.
  vec4 c = texture(u_tex, v_uv);
  o = vec4(linearToSrgb(clamp(c.rgb, 0.0, 1.0)), 1.0);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// A reserved word in a shader fails to compile on every GPU, which
    /// stops the editor from opening any file.
    /// Vertex and fragment shaders default to different int precisions, so a
    /// program sharing an int uniform fails to link unless both declare it.
    #[test]
    fn int_uniforms_declare_precision() {
        for (name, src) in [("DAB_VS", DAB_VS.to_string()), ("DAB_FS", dab_fs()), ("STAMP_FS", stamp_fs()), ("PICK_FS", pick_fs()), ("STAMP_VS", STAMP_VS.to_string())] {
            if src.contains("uniform int") {
                assert!(src.contains("precision highp int;"), "{name} has an int uniform without `precision highp int`");
            }
        }
    }

    #[test]
    fn shaders_avoid_glsl_es_reserved_words() {
        const RESERVED: &[&str] = &[
            "attribute", "varying", "coherent", "volatile", "restrict", "readonly", "writeonly", "resource", "atomic_uint",
            "noperspective", "patch", "sample", "subroutine", "common", "partition", "active", "asm", "class", "union", "enum",
            "typedef", "template", "this", "goto", "inline", "noinline", "public", "static", "extern", "external", "interface",
            "long", "short", "double", "half", "fixed", "unsigned", "superp", "input", "output", "filter", "sizeof", "cast",
            "namespace", "using",
        ];
        for (name, src) in [("SHAPE_VS", SHAPE_VS), ("SHAPE_FS", SHAPE_FS), ("MESH_VS", MESH_VS), ("MESH_FS", MESH_FS), ("TEX_VS", TEX_VS), ("TEX_FS", TEX_FS), ("COMPOSITE_VS", COMPOSITE_VS), ("COMPOSITE_FS", COMPOSITE_FS), ("BLUR_FS", BLUR_FS), ("COPY_FS", COPY_FS), ("DAB_VS", DAB_VS), ("DAB_FS", &dab_fs()), ("PICK_FS", &pick_fs()), ("STAMP_VS", STAMP_VS), ("STAMP_FS", &stamp_fs())] {
            for line in src.lines() {
                let code = line.split("//").next().unwrap_or("");
                for word in code.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                    assert!(!RESERVED.contains(&word), "{name} uses the reserved word '{word}': {line}");
                }
            }
        }
    }
}
