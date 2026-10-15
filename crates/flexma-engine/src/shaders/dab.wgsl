// Brush dabs into a 256 px paint tile (the WebGPU twin of DAB_VS / dab_fs in
// gl.rs): x, y, radius, hardness in texel space, a premultiplied color and
// the tip angle per instance. Appended to brush.wgsl.

const SIZE: f32 = 256.0;

@group(0) @binding(0) var<uniform> dab_st: Style;

struct DabIn {
  @builtin(vertex_index) vi: u32,
  @location(0) d0: vec4f,
  @location(1) col: vec4f,
  @location(2) d2: vec4f,
};

struct DabOut {
  @builtin(position) pos: vec4f,
  @location(0) p: vec2f,
  @location(1) @interpolate(flat) d0: vec4f,
  @location(2) @interpolate(flat) col: vec4f,
  @location(3) @interpolate(flat) d2: vec4f,
};

@vertex
fn vs_dab(a: DabIn) -> DabOut {
  let corner = vec2f(f32(a.vi & 1u), f32((a.vi >> 1u) & 1u));
  let tipped = (dab_st.flags & 1u) != 0u;
  let r = select(max(a.d0.z, 0.5) + 1.0, max(a.d0.z, 0.5) * length(dab_st.extent) + 1.0, tipped);
  let p = a.d0.xy + (corner * 2.0 - 1.0) * r;
  let c = p / SIZE * 2.0 - 1.0;
  var o: DabOut;
  // Texel row 0 (the tile's top) is the top of a WebGPU render target.
  o.pos = vec4f(c.x, -c.y, 0.0, 1.0);
  o.p = p;
  o.d0 = a.d0;
  o.col = a.col;
  o.d2 = a.d2;
  return o;
}

@fragment
fn fs_dab(i: DabOut) -> @location(0) vec4f {
  let st = dab_st;
  if st.flags == 0u {
    // A plain round dab. Hardness sets where the soft falloff starts; the
    // rim is antialiased, and dabs under half a pixel fade by area instead
    // of shrinking.
    let r = max(i.d0.z, 0.5);
    let d = length(i.p - i.d0.xy);
    let inner = r * clamp(i.d0.w, 0.0, 1.0);
    let soft = 1.0 - smoothstep(inner, max(r, inner + 0.001), d);
    let rim = clamp(r - d + 0.5, 0.0, 1.0);
    let area = min(1.0, i.d0.z * i.d0.z / 0.25);
    return i.col * (soft * rim * area);
  }
  let lp = st.origin + i.p;
  let cv = cover(st, i.p, i.d0.xy, i.d0.z, i.d2.x, i.d0.w);
  let k = cv.k * grain(st, lp);
  if (st.flags & 2u) != 0u {
    // The tip's own colors (a photo stamp), at the dab's flow.
    let a = k * i.col.a;
    return vec4f(cv.tipc.rgb / max(cv.tipc.a, 1e-4) * a, a);
  }
  return tint(st, i.col, lp) * k;
}
