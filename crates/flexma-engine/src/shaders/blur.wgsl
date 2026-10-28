// Background blur for the WebGPU backend: one direction of a Gaussian over a
// full-screen pass. Mirrors BLUR_FS in gl.rs.

struct U {
  view0: vec4f,
  view1: vec4f,
  view2: vec4f,
  m0: vec4f,
  m1: vec4f,
  // xy: the step between taps (texture coordinates), z: σ in steps.
  v: vec4f,
  p: vec4f,
  uv: vec4f,
};

@group(0) @binding(0) var<uniform> u: U;
@group(1) @binding(0) var samp: sampler;
@group(1) @binding(1) var tex: texture_2d<f32>;

struct FullOut {
  @builtin(position) pos: vec4f,
  @location(0) uv: vec2f,
};

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> FullOut {
  let p = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u));
  var o: FullOut;
  o.pos = vec4f(p * 2.0 - 1.0, 0.0, 1.0);
  o.uv = vec2f(p.x, 1.0 - p.y);
  return o;
}

@fragment
fn fs_blur(i: FullOut) -> @location(0) vec4f {
  let s = max(u.v.z, 0.5);
  var acc = vec4f(0.0);
  var total = 0.0;
  for (var k = -16; k <= 16; k++) {
    let x = f32(k);
    let wt = exp(-0.5 * x * x / (s * s));
    acc += textureSampleLevel(tex, samp, i.uv + u.v.xy * x, 0.0) * wt;
    total += wt;
  }
  return acc / total;
}
