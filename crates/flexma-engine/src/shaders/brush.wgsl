// Shared by the dab and smudge shaders: how a brush tip covers a pixel, paper
// grain and color variation. The WebGPU twin of STYLE_GLSL in gl.rs; keep
// them in step (strokes must look the same on both).

struct Style {
  // The tip's half-extents per unit of radius (longer side 1).
  extent: vec2f,
  // The tile's top-left in layer px.
  origin: vec2f,
  // Grain and tint texture sizes in layer px (they repeat).
  tex_size: vec4f,
  // Grain and tint strength.
  strength: vec2f,
  // Grain and tint mip levels.
  lods: vec2f,
  // The tip's size in texels.
  tip_px: vec2f,
  // 1 tip, 2 the tip's own colors, 4 grain, 8 tint.
  flags: u32,
  size: f32,
};

@group(0) @binding(1) var clamp_s: sampler;
@group(0) @binding(2) var repeat_s: sampler;
@group(0) @binding(3) var tip_t: texture_2d<f32>;
@group(0) @binding(4) var grain_t: texture_2d<f32>;
@group(0) @binding(5) var tint_t: texture_2d<f32>;

struct Cover {
  k: f32,
  // The tip's color (premultiplied) where it has one.
  tipc: vec4f,
  // Where the pixel falls in the tip, 0…1.
  uv: vec2f,
};

// Coverage of a dab (center c, radius r, angle a, hardness h) at layer point p.
fn cover(st: Style, p: vec2f, c: vec2f, r: f32, a: f32, h: f32) -> Cover {
  var o: Cover;
  o.tipc = vec4f(1.0);
  let area = min(1.0, r * r / 0.25);
  let d = p - c;
  let q = vec2f(cos(a) * d.x + sin(a) * d.y, -sin(a) * d.x + cos(a) * d.y);
  o.uv = q / (st.extent * max(r, 0.5)) * 0.5 + 0.5;
  if (st.flags & 1u) == 0u {
    let rr = max(r, 0.5);
    let dist = length(d);
    let inner = rr * clamp(h, 0.0, 1.0);
    let soft = 1.0 - smoothstep(inner, max(rr, inner + 0.001), dist);
    let rim = clamp(rr - dist + 0.5, 0.0, 1.0);
    o.k = soft * rim * area;
    return o;
  }
  let inside = step(vec2f(0.0), o.uv) * step(o.uv, vec2f(1.0));
  // The mip level where a texel is about a pixel (the same on every GPU).
  let span = st.tip_px / (2.0 * st.extent * max(r, 0.5));
  o.tipc = textureSampleLevel(tip_t, clamp_s, o.uv, max(log2(max(span.x, span.y)), 0.0));
  let k = select(o.tipc.r, o.tipc.a, (st.flags & 2u) != 0u);
  o.k = k * inside.x * inside.y * area;
  return o;
}

// Paper grain: paint catches on the paper's high spots.
fn grain(st: Style, p: vec2f) -> f32 {
  if (st.flags & 4u) == 0u {
    return 1.0;
  }
  let g = textureSampleLevel(grain_t, repeat_s, p / st.tex_size.xy, st.lods.x).r;
  return clamp(1.0 - st.strength.x * (1.0 - g) * 1.6, 0.0, 1.0);
}

// Color variation around the brush color (premultiplied in and out).
fn tint(st: Style, c: vec4f, p: vec2f) -> vec4f {
  if (st.flags & 8u) == 0u || c.a <= 0.0 {
    return c;
  }
  let v = textureSampleLevel(tint_t, repeat_s, p / st.tex_size.zw, st.lods.y).rgb - 0.5;
  return vec4f(clamp(c.rgb / c.a + v * 2.0 * st.strength.y, vec3f(0.0), vec3f(1.0)) * c.a, c.a);
}
