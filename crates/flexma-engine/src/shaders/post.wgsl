// Full-screen passes for the WebGPU backend: encoding the linear-blending
// target onto the canvas, and downsampling mip levels.

@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var tex: texture_2d<f32>;

struct FullOut {
  @builtin(position) pos: vec4f,
  @location(0) uv: vec2f,
};

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> FullOut {
  let p = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u));
  var o: FullOut;
  o.pos = vec4f(p * 2.0 - 1.0, 0.0, 1.0);
  // Texture rows start at the top; clip space y points up.
  o.uv = vec2f(p.x, 1.0 - p.y);
  return o;
}

fn linear_to_srgb(c: vec3f) -> vec3f {
  return select(1.055 * pow(c, vec3f(1.0 / 2.4)) - 0.055, c * 12.92, c < vec3f(0.0031308));
}

@fragment
fn fs_composite(i: FullOut) -> @location(0) vec4f {
  // The sRGB texture samples as linear light; the canvas wants sRGB.
  let c = textureSample(tex, samp, i.uv);
  return vec4f(linear_to_srgb(clamp(c.rgb, vec3f(0.0), vec3f(1.0))), 1.0);
}

@fragment
fn fs_blit(i: FullOut) -> @location(0) vec4f {
  return textureSample(tex, samp, i.uv);
}
