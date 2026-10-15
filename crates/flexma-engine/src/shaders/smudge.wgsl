// Smudge dabs (the WebGPU twin of pick_fs / STAMP_VS / stamp_fs in gl.rs).
// Appended to brush.wgsl. The tiles under a dab are copied side by side into
// a window first; then
//   pick:  the paint under the dab, in the tip's frame, is blended into what
//          the stroke carries (a SMUDGE_BUF² buffer);
//   stamp: each tile pixel becomes the carried paint by the tip's coverage.
//          Untouched pixels are copied exactly from the window (no blending).

struct Smudge {
  st: Style,
  // x, y (layer px), radius, angle.
  dab: vec4f,
  // hardness, flow, lock alpha, tile size.
  look: vec4f,
  // window x, y; tile x, y (layer px).
  place: vec4f,
  // The quad to draw, tile px.
  quad: vec4f,
  // window x, y, w, h.
  win: vec4f,
  // Premultiplied brush color mixed in.
  paint: vec4f,
  // first dab, keep, mix, buffer size.
  params: vec4f,
};

@group(0) @binding(0) var<uniform> sm: Smudge;
@group(0) @binding(6) var window_t: texture_2d<f32>;
@group(0) @binding(7) var buf_t: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4f {
  let p = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u));
  return vec4f(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_pick(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  let uv = pos.xy / sm.params.w;
  let q = (uv * 2.0 - 1.0) * sm.st.extent * max(sm.dab.z, 0.5);
  let c = cos(sm.dab.w);
  let s = sin(sm.dab.w);
  let p = sm.dab.xy + vec2f(c * q.x - s * q.y, s * q.x + c * q.y);
  let under = textureSampleLevel(window_t, clamp_s, (p - sm.win.xy) / sm.win.zw, 0.0);
  var carried = under;
  if sm.params.x <= 0.5 {
    carried = mix(under, textureSampleLevel(buf_t, clamp_s, uv, 0.0), sm.params.y);
  }
  return mix(carried, sm.paint, sm.params.z);
}

@vertex
fn vs_stamp(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4f {
  let corner = vec2f(f32(vi & 1u), f32((vi >> 1u) & 1u));
  let p = mix(sm.quad.xy, sm.quad.zw, corner);
  let c = p / sm.look.w * 2.0 - 1.0;
  return vec4f(c.x, -c.y, 0.0, 1.0);
}

@fragment
fn fs_stamp(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  let p = sm.place.zw + pos.xy;
  let dst = textureLoad(window_t, vec2i(p - sm.place.xy), 0);
  let cv = cover(sm.st, p, sm.dab.xy, sm.dab.z, sm.dab.w, sm.look.x);
  let k = cv.k * sm.look.y * grain(sm.st, p);
  var r = mix(dst, textureSampleLevel(buf_t, clamp_s, cv.uv, 0.0), clamp(k, 0.0, 1.0));
  if sm.look.z > 0.5 {
    r = select(vec4f(0.0), r * (dst.a / r.a), r.a > 0.0);
  }
  return r;
}
