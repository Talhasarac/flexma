// Brush dabs into a 256 px paint tile (the WebGPU twin of DAB_VS/DAB_FS in
// gl.rs): x, y, radius, hardness in texel space and a premultiplied color
// per instance.

const SIZE: f32 = 256.0;

struct DabIn {
  @builtin(vertex_index) vi: u32,
  @location(0) d0: vec4f,
  @location(1) col: vec4f,
};

struct DabOut {
  @builtin(position) pos: vec4f,
  @location(0) p: vec2f,
  @location(1) @interpolate(flat) d0: vec4f,
  @location(2) @interpolate(flat) col: vec4f,
};

@vertex
fn vs_dab(a: DabIn) -> DabOut {
  let corner = vec2f(f32(a.vi & 1u), f32((a.vi >> 1u) & 1u));
  let r = max(a.d0.z, 0.5) + 1.0;
  let p = a.d0.xy + (corner * 2.0 - 1.0) * r;
  let c = p / SIZE * 2.0 - 1.0;
  var o: DabOut;
  // Texel row 0 (the tile's top) is the top of a WebGPU render target.
  o.pos = vec4f(c.x, -c.y, 0.0, 1.0);
  o.p = p;
  o.d0 = a.d0;
  o.col = a.col;
  return o;
}

@fragment
fn fs_dab(i: DabOut) -> @location(0) vec4f {
  // Hardness sets where the soft falloff starts; the rim is antialiased, and
  // dabs under half a pixel fade by area instead of shrinking.
  let r = max(i.d0.z, 0.5);
  let d = length(i.p - i.d0.xy);
  let inner = r * clamp(i.d0.w, 0.0, 1.0);
  let soft = 1.0 - smoothstep(inner, max(r, inner + 0.001), d);
  let rim = clamp(r - d + 0.5, 0.0, 1.0);
  let area = min(1.0, i.d0.z * i.d0.z / 0.25);
  return i.col * (soft * rim * area);
}
