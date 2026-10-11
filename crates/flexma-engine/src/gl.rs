//! Thin WebGL2 helpers and the shader sources.

use wasm_bindgen::JsCast;
use web_sys::{WebGl2RenderingContext as GL, WebGlProgram, WebGlShader, WebGlUniformLocation};

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
// d(slice1)/dx, for explicit texture gradients.
float slice1d(float x, float size, float img, float a, float b, float da, float db) {
  if (x <= da) return da > 0.0 ? a / da / img : 0.0;
  if (x >= size - db) return db > 0.0 ? b / db / img : 0.0;
  return (img - a - b) / max(size - da - db, 1e-4) / img;
}
void main() {
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
  else if (ft == 3) {
    // Image: v_c0 = tint, v_c1 = (imgW, imgH, fit, alpha); 9-slice when v_c3.z is set.
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
      vec2 p = clamp(v_local, vec2(0.0), sz);
      vec2 tuv = vec2(slice1(p.x, sz.x, img.x, a.x, b.x, da.x, db.x), slice1(p.y, sz.y, img.y, a.y, b.y, da.y, db.y));
      vec2 f = vec2(slice1d(p.x, sz.x, img.x, a.x, b.x, da.x, db.x), slice1d(p.y, sz.y, img.y, a.y, b.y, da.y, db.y));
      bool center = all(greaterThan(p, da)) && all(lessThan(p, sz - db));
      if (v_c3.y > 0.5 || !center) fill = textureGrad(u_tex, tuv, dlx * f, dly * f) * v_c0 * v_c1.w;
    } else {
      float fit = v_c1.z;
      vec2 scale;
      if (fit < 0.5) { float s = max(sz.x / img.x, sz.y / img.y); scale = img * s; }
      else if (fit < 1.5) { float s = min(sz.x / img.x, sz.y / img.y); scale = img * s; }
      else { scale = sz; }
      vec2 off = (sz - scale) * 0.5;
      vec2 tuv = (v_local - off) / scale;
      if (tuv.x >= 0.0 && tuv.y >= 0.0 && tuv.x <= 1.0 && tuv.y <= 1.0) fill = textureGrad(u_tex, tuv, dlx / scale, dly / scale) * v_c0 * v_c1.w;
    }
  }
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
void main() { o = u_color; }
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
void main() { o = texture(u_tex, v_uv) * u_alpha; }
"#;
