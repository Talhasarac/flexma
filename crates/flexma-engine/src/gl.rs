//! The WebGL2 backend (the fallback when WebGPU isn't available), its helpers
//! and the GLSL shader sources.

use crate::backend::{Backend, FrameParams, ImageSource, MeshId, Rows, TexId};
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
        let shape_u = Uniforms::new(&gl, &shape_prog, &["u_view", "u_px", "u_tex", "u_linear"]);
        let mesh_u = Uniforms::new(&gl, &mesh_prog, &["u_view", "u_m0", "u_m1", "u_color", "u_linear"]);
        let tex_u = Uniforms::new(&gl, &tex_prog, &["u_view", "u_m0", "u_m1", "u_rect", "u_tex", "u_alpha", "u_linear"]);
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

    fn draw_texture(&mut self, tex: TexId, m: Rows, rect: [f32; 4], alpha: f32) {
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
out vec2 v_uv;
void main() {
  vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
  vec2 local = u_rect.xy + corner * u_rect.zw;
  vec2 world = vec2(dot(u_m0, vec3(local, 1.0)), dot(u_m1, vec3(local, 1.0)));
  vec3 clip = u_view * vec3(world, 1.0);
  gl_Position = vec4(clip.xy, 0.0, 1.0);
  v_uv = corner;
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
    #[test]
    fn shaders_avoid_glsl_es_reserved_words() {
        const RESERVED: &[&str] = &[
            "attribute", "varying", "coherent", "volatile", "restrict", "readonly", "writeonly", "resource", "atomic_uint",
            "noperspective", "patch", "sample", "subroutine", "common", "partition", "active", "asm", "class", "union", "enum",
            "typedef", "template", "this", "goto", "inline", "noinline", "public", "static", "extern", "external", "interface",
            "long", "short", "double", "half", "fixed", "unsigned", "superp", "input", "output", "filter", "sizeof", "cast",
            "namespace", "using",
        ];
        for (name, src) in [("SHAPE_VS", SHAPE_VS), ("SHAPE_FS", SHAPE_FS), ("MESH_VS", MESH_VS), ("MESH_FS", MESH_FS), ("TEX_VS", TEX_VS), ("TEX_FS", TEX_FS), ("COMPOSITE_VS", COMPOSITE_VS), ("COMPOSITE_FS", COMPOSITE_FS)] {
            for line in src.lines() {
                let code = line.split("//").next().unwrap_or("");
                for word in code.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                    assert!(!RESERVED.contains(&word), "{name} uses the reserved word '{word}': {line}");
                }
            }
        }
    }
}
