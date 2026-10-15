// Flexma's scene shaders for the WebGPU backend: instanced SDF shapes, path
// meshes and textured quads (text, labels). A line-by-line port of the GLSL in
// gl.rs, so both backends draw the same pixels.

struct U {
  view0: vec4f,
  view1: vec4f,
  view2: vec4f,
  // 2D affine rows (a, c, e) and (b, d, f).
  m0: vec4f,
  m1: vec4f,
  // Mesh color, or a textured quad's rect (x, y, w, h).
  v: vec4f,
  // x: device px per unit, y: linear output, z: alpha.
  p: vec4f,
  // Textured quads: the texture region shown (x0, y0, x1, y1).
  uv: vec4f,
};

@group(0) @binding(0) var<uniform> u: U;
@group(1) @binding(0) var samp: sampler;
@group(1) @binding(1) var tex: texture_2d<f32>;

fn clip_of(world: vec2f) -> vec4f {
  let m = mat3x3f(u.view0.xyz, u.view1.xyz, u.view2.xyz);
  let c = m * vec3f(world, 1.0);
  return vec4f(c.xy, 0.0, 1.0);
}

fn srgb_to_linear(c: vec3f) -> vec3f {
  return select(pow((c + 0.055) / 1.055, vec3f(2.4)), c / 12.92, c < vec3f(0.04045));
}

// Linear blending: outputs go to an sRGB target in linear light.
fn to_output(c: vec4f) -> vec4f {
  if (u.p.y < 0.5 || c.a <= 0.0) {
    return c;
  }
  return vec4f(srgb_to_linear(clamp(c.rgb / c.a, vec3f(0.0), vec3f(1.0))) * c.a, c.a);
}

// ---------------------------------------------------------------- shapes

struct ShapeIn {
  @builtin(vertex_index) vi: u32,
  @location(0) m0: vec4f,
  @location(1) m1: vec4f,
  @location(2) size: vec4f,
  @location(3) radii: vec4f,
  @location(4) c0: vec4f,
  @location(5) c1: vec4f,
  @location(6) c2: vec4f,
  @location(7) c3: vec4f,
  @location(8) stops: vec4f,
  @location(9) paint: vec4f,
  @location(10) stroke: vec4f,
  @location(11) strokep: vec4f,
  @location(12) fx: vec4f,
};

struct ShapeOut {
  @builtin(position) pos: vec4f,
  @location(0) local: vec2f,
  @location(1) @interpolate(flat) size: vec4f,
  @location(2) @interpolate(flat) radii: vec4f,
  @location(3) @interpolate(flat) c0: vec4f,
  @location(4) @interpolate(flat) c1: vec4f,
  @location(5) @interpolate(flat) c2: vec4f,
  @location(6) @interpolate(flat) c3: vec4f,
  @location(7) @interpolate(flat) stops: vec4f,
  @location(8) @interpolate(flat) paint: vec4f,
  @location(9) @interpolate(flat) stroke: vec4f,
  @location(10) @interpolate(flat) strokep: vec4f,
  @location(11) @interpolate(flat) fx: vec4f,
};

@vertex
fn vs_shape(a: ShapeIn) -> ShapeOut {
  let corner = vec2f(f32(a.vi & 1u), f32((a.vi >> 1u) & 1u));
  // Expand by the stroke/shadow extent plus 1.5 device px for the AA fringe.
  let pad = a.strokep.z + 1.5 / u.p.x;
  let local = mix(vec2f(-pad), a.size.xy + vec2f(pad), corner);
  let world = vec2f(a.m0.x * local.x + a.m0.y * local.y + a.m0.z, a.m1.x * local.x + a.m1.y * local.y + a.m1.z);
  var o: ShapeOut;
  o.pos = clip_of(world);
  o.local = local;
  o.size = a.size;
  o.radii = a.radii;
  o.c0 = a.c0;
  o.c1 = a.c1;
  o.c2 = a.c2;
  o.c3 = a.c3;
  o.stops = a.stops;
  o.paint = a.paint;
  o.stroke = a.stroke;
  o.strokep = a.strokep;
  o.fx = a.fx;
  return o;
}

// The fragment's shape inputs, read by the helpers below (GLSL's v_* globals).
struct Vary {
  local: vec2f,
  size: vec4f,
  radii: vec4f,
  c0: vec4f,
  c1: vec4f,
  c2: vec4f,
  c3: vec4f,
  stops: vec4f,
  paint: vec4f,
  stroke: vec4f,
  strokep: vec4f,
  fx: vec4f,
};
var<private> V: Vary;

// GLSL's mod: x - y * floor(x / y) (WGSL's % truncates instead).
fn fmod(x: f32, y: f32) -> f32 {
  return x - y * floor(x / y);
}

fn sd_round_box(p: vec2f, b: vec2f, rad: vec4f) -> f32 {
  var r: f32;
  if (p.x > 0.0) {
    r = select(rad.y, rad.z, p.y > 0.0);
  } else {
    r = select(rad.x, rad.w, p.y > 0.0);
  }
  let q = abs(p) - b + r;
  return min(max(q.x, q.y), 0.0) + length(max(q, vec2f(0.0))) - r;
}

fn sd_ellipse(p: vec2f, ab: vec2f) -> f32 {
  let pa = p / ab;
  let k0 = length(pa);
  let k1 = length(pa / ab);
  if (k0 < 1e-5) {
    return -min(ab.x, ab.y);
  }
  return k0 * (k0 - 1.0) / k1;
}

fn gradient(uv: vec2f) -> vec4f {
  let dir = vec2f(cos(V.paint.y), sin(V.paint.y));
  let d = uv - 0.5;
  // Project onto the gradient axis spanning the box diagonal extent.
  let ext = abs(dir.x) * 0.5 + abs(dir.y) * 0.5;
  let t = clamp(dot(d, dir) / (2.0 * ext) + 0.5, 0.0, 1.0);
  let n = i32(V.paint.z);
  if (t <= V.stops.x || n < 2) { return V.c0; }
  if (t <= V.stops.y) { return mix(V.c0, V.c1, (t - V.stops.x) / max(V.stops.y - V.stops.x, 1e-5)); }
  if (n < 3) { return V.c1; }
  if (t <= V.stops.z) { return mix(V.c1, V.c2, (t - V.stops.y) / max(V.stops.z - V.stops.y, 1e-5)); }
  if (n < 4) { return V.c2; }
  if (t <= V.stops.w) { return mix(V.c2, V.c3, (t - V.stops.z) / max(V.stops.w - V.stops.z, 1e-5)); }
  return V.c3;
}

// One axis of a 9-slice: layer position x in [0, size] to a texture coordinate.
// a, b = source insets (image px) at the start/end; da, db = the same in layer px.
fn slice1(x: f32, size: f32, img: f32, a: f32, b: f32, da: f32, db: f32) -> f32 {
  if (x <= da) { return select(0.0, x / da, da > 0.0) * a / img; }
  if (x >= size - db) { return 1.0 - select(0.0, (size - x) / db, db > 0.0) * b / img; }
  return (a + (x - da) / max(size - da - db, 1e-4) * (img - a - b)) / img;
}

// d(slice1)/dx, for explicit texture gradients.
fn slice1d(x: f32, size: f32, img: f32, a: f32, b: f32, da: f32, db: f32) -> f32 {
  if (x <= da) { return select(0.0, a / da / img, da > 0.0); }
  if (x >= size - db) { return select(0.0, b / db / img, db > 0.0); }
  return (img - a - b) / max(size - da - db, 1e-4) / img;
}

// Unity's Filled image: coverage of layer position p in a box of size sz.
// V.stops = (method: 1 horizontal, 2 vertical, 3/4/5 radial 90/180/360,
// amount, origin as Unity's fillOrigin, clockwise).
fn filled_coverage(p: vec2f, sz: vec2f, aa: f32) -> f32 {
  let m = i32(V.stops.x + 0.5);
  if (m == 0) { return 1.0; }
  let a = clamp(V.stops.y, 0.0, 1.0);
  if (a <= 0.0) { return 0.0; }
  if (a >= 1.0) { return 1.0; }
  let o = i32(V.stops.z + 0.5);
  let cw = V.stops.w > 0.5;
  if (m == 1) {
    return clamp(0.5 + select(p.x - (1.0 - a) * sz.x, a * sz.x - p.x, o == 0) / aa, 0.0, 1.0);
  }
  if (m == 2) {
    return clamp(0.5 + select(a * sz.y - p.y, p.y - (1.0 - a) * sz.y, o == 0) / aa, 0.0, 1.0);
  }
  // Radial: a pivot, the direction a clockwise sweep starts at (degrees
  // clockwise from 12 o'clock) and the span of directions it covers.
  var pivot: vec2f;
  var start: f32;
  var span: f32;
  if (m == 3) {
    // Corner: bottom-left, top-left, top-right, bottom-right.
    if (o == 0) { pivot = vec2f(0.0, sz.y); } else if (o == 1) { pivot = vec2f(0.0); } else if (o == 2) { pivot = vec2f(sz.x, 0.0); } else { pivot = sz; }
    start = f32(o) * 90.0;
    span = 90.0;
  } else if (m == 4) {
    // Edge middle: bottom, left, top, right.
    if (o == 0) { pivot = vec2f(sz.x * 0.5, sz.y); } else if (o == 1) { pivot = vec2f(0.0, sz.y * 0.5); } else if (o == 2) { pivot = vec2f(sz.x * 0.5, 0.0); } else { pivot = vec2f(sz.x, sz.y * 0.5); }
    if (o == 0) { start = 270.0; } else if (o == 1) { start = 0.0; } else if (o == 2) { start = 90.0; } else { start = 180.0; }
    span = 180.0;
  } else {
    // Center, starting at the bottom, right, top or left.
    pivot = sz * 0.5;
    if (o == 0) { start = 180.0; } else if (o == 1) { start = 90.0; } else if (o == 2) { start = 0.0; } else { start = 270.0; }
    span = 360.0;
  }
  // The shown sweep [lo, hi] within the span. Counter-clockwise fills grow
  // from the span's far end (for 360°, back from the start).
  let lo = select(span * (1.0 - a), 0.0, cw);
  let hi = select(span, span * a, cw);
  let hw = (hi - lo) * 0.5;
  let d = p - pivot;
  let r = length(d);
  let ang = degrees(atan2(d.x, -d.y));
  let rel = fmod(ang - start - lo - hw + 540.0, 360.0) - 180.0;
  // Signed px distance to the cut edges (positive inside); a span edge that
  // is also the box edge isn't a cut.
  let full = span > 359.0;
  let sd_lo = select(1e6, r * sin(radians(clamp(rel + hw, -90.0, 90.0))), full || !cw);
  let sd_hi = select(1e6, r * sin(radians(clamp(hw - rel, -90.0, 90.0))), full || cw);
  return clamp(0.5 + min(sd_lo, sd_hi) / aa, 0.0, 1.0);
}

// An image lookup at tuv in [0, 1] over the image, or over its crop when
// V.fx = crop rect in texture space (x, y, w, h). Lookups stay half a texel
// inside the crop so neighbouring sprites in an atlas don't bleed in.
fn sample_image(tuv: vec2f, gx: vec2f, gy: vec2f) -> vec4f {
  if (V.fx.z <= 0.0) {
    return textureSampleGrad(tex, samp, tuv, gx, gy);
  }
  let half_texel = 0.5 / vec2f(textureDimensions(tex, 0));
  let lo = V.fx.xy + half_texel;
  let hi = max(lo, V.fx.xy + V.fx.zw - half_texel);
  return textureSampleGrad(tex, samp, clamp(V.fx.xy + tuv * V.fx.zw, lo, hi), gx * V.fx.zw, gy * V.fx.zw);
}

// An image fill at layer position `local`: V.c0 = tint, V.c1 = (imgW, imgH,
// fit, alpha), 9-slice when V.c3.z is set. gx, gy: layer-space screen
// derivatives, which pick the mip level.
fn image_fill(local: vec2f, gx: vec2f, gy: vec2f, aa: f32) -> vec4f {
  var fill = vec4f(0.0);
  let img = max(V.c1.xy, vec2f(1.0));
  let sz = max(V.size.xy, vec2f(1e-4));
  if (V.c3.z > 0.5) {
    // V.c2 = source insets (top, right, bottom, left) in image px, V.c3.x = layer px per image px.
    let a = V.c2.wx;
    let b = V.c2.yz;
    var da = a * V.c3.x;
    var db = b * V.c3.x;
    let k = min(vec2f(1.0), sz / max(da + db, vec2f(1e-4)));
    da = da * k;
    db = db * k;
    let p = clamp(local, vec2f(0.0), sz);
    let tuv = vec2f(slice1(p.x, sz.x, img.x, a.x, b.x, da.x, db.x), slice1(p.y, sz.y, img.y, a.y, b.y, da.y, db.y));
    let f = vec2f(slice1d(p.x, sz.x, img.x, a.x, b.x, da.x, db.x), slice1d(p.y, sz.y, img.y, a.y, b.y, da.y, db.y));
    let center = all(p > da) && all(p < sz - db);
    if (V.c3.y > 0.5 || !center) {
      fill = sample_image(tuv, gx * f, gy * f) * V.c0 * V.c1.w;
    }
  } else {
    let fit = V.c1.z;
    if (fit > 2.5) {
      // Tile: repeat at V.c3.w layer px per image px from the bottom-left, like Unity.
      let t = img * max(V.c3.w, 1e-4);
      let q = vec2f(local.x, sz.y - local.y) / t;
      fill = sample_image(vec2f(fract(q.x), 1.0 - fract(q.y)), gx / t, gy / t) * V.c0 * V.c1.w;
    } else {
      var scale: vec2f;
      if (fit < 0.5) {
        let s = max(sz.x / img.x, sz.y / img.y);
        scale = img * s;
      } else if (fit < 1.5) {
        let s = min(sz.x / img.x, sz.y / img.y);
        scale = img * s;
      } else {
        scale = sz;
      }
      let off = (sz - scale) * 0.5;
      let tuv = (local - off) / scale;
      if (tuv.x >= 0.0 && tuv.y >= 0.0 && tuv.x <= 1.0 && tuv.y <= 1.0) {
        fill = sample_image(tuv, gx / scale, gy / scale) * V.c0 * V.c1.w;
      }
    }
  }
  fill = fill * filled_coverage(local, sz, aa);
  return fill;
}

fn shape_sd(p: vec2f, hs: vec2f) -> f32 {
  if (V.size.z > 0.5) {
    return sd_ellipse(p, hs);
  }
  return sd_round_box(p, hs, V.radii);
}

fn shade(dlx: vec2f, dly: vec2f) -> vec4f {
  let hs = max(V.size.xy * 0.5, vec2f(1e-4));
  let p = V.local - hs;
  let d = shape_sd(p, hs);
  let aa = 1.0 / u.p.x;
  let mode = V.size.w;
  if (mode > 3.5) {
    // Drop shadow of an image: its alpha silhouette, grown by spread
    // (V.strokep.y, the most of 9 taps around) and blurred (V.strokep.x, by
    // sampling a blurrier mip), in the shadow color V.stroke.
    let bl = max(V.strokep.x, 0.0);
    let sp = V.strokep.y;
    let g1 = vec2f(max(bl * 0.5, length(dlx)), 0.0);
    let g2 = vec2f(0.0, max(bl * 0.5, length(dly)));
    var a = select(0.0, 1.0, sp < 0.0);
    for (var i = 0; i < 9; i++) {
      let t = f32(i) * 0.785398;
      var q = V.local;
      if (i != 0) {
        q = V.local + abs(sp) * vec2f(cos(t), sin(t));
      }
      // Past the layer the silhouette fades out over the blur.
      let qc = clamp(q, vec2f(0.0), V.size.xy);
      let fade = 1.0 - smoothstep(0.0, bl + aa, length(q - qc));
      let inside = select(0.0, 1.0, shape_sd(qc - hs, hs) <= 0.5);
      let ta = image_fill(qc, g1, g2, aa).a * fade * inside;
      if (sp < 0.0) {
        a = min(a, ta);
      } else {
        a = max(a, ta);
      }
    }
    return V.stroke * a * V.paint.w;
  }
  let alpha_clip = mode > 2.5;
  if (mode > 1.5 && !alpha_clip) {
    // Stencil clip shape.
    if (d > 0.0) {
      discard;
    }
    return vec4f(0.0);
  }
  if (mode > 0.5 && !alpha_clip) {
    // Drop shadow: blurred SDF.
    let blur = max(V.fx.z, aa);
    let sd = d - V.fx.w;
    let a = 1.0 - smoothstep(-blur * 0.5, blur * 0.5, sd);
    return V.c0 * a * V.paint.w;
  }
  var fill = vec4f(0.0);
  let ft = i32(V.paint.x + 0.5);
  let uv = V.local / max(V.size.xy, vec2f(1e-4));
  if (ft == 1) {
    fill = V.c0;
  } else if (ft == 2) {
    fill = gradient(uv);
  } else if (ft == 3) {
    fill = image_fill(V.local, dlx, dly, aa);
  }
  if (alpha_clip) {
    // Stencil alpha mask: keep the pixels where the fill isn't transparent
    // (above Unity's Mask cutoff of 0.001).
    if (d > 0.0 || fill.a * V.paint.w <= 0.001) {
      discard;
    }
    return vec4f(0.0);
  }
  let cov = clamp(0.5 - d / aa, 0.0, 1.0);
  var col = fill * cov;
  let sw = V.strokep.x;
  if (sw > 0.0 && V.stroke.a > 0.0) {
    let align = V.strokep.y;
    var sd: f32;
    if (align < 0.5) {
      sd = abs(d) - sw * 0.5;
    } else if (align < 1.5) {
      sd = abs(d + sw * 0.5) - sw * 0.5;
    } else {
      sd = abs(d - sw * 0.5) - sw * 0.5;
    }
    var scov: f32;
    if (sw < aa) {
      // Hairline: keep a 1px line, fade by thickness.
      var center: f32;
      if (align < 0.5) {
        center = d;
      } else if (align < 1.5) {
        center = d + sw * 0.5;
      } else {
        center = d - sw * 0.5;
      }
      scov = clamp(1.0 - abs(center) / aa, 0.0, 1.0) * (sw / aa);
    } else {
      scov = clamp(0.5 - sd / aa, 0.0, 1.0);
    }
    let s = V.stroke * scov;
    col = s + col * (1.0 - s.a);
  }
  return col * V.paint.w;
}

@fragment
fn fs_shape(i: ShapeOut) -> @location(0) vec4f {
  V = Vary(i.local, i.size, i.radii, i.c0, i.c1, i.c2, i.c3, i.stops, i.paint, i.stroke, i.strokep, i.fx);
  // Screen derivatives of the layer position, taken in uniform control flow:
  // image lookups below sit in per-pixel branches.
  let dlx = dpdx(i.local);
  let dly = dpdy(i.local);
  return to_output(shade(dlx, dly));
}

// ---------------------------------------------------------------- meshes

@vertex
fn vs_mesh(@location(0) pos: vec2f) -> @builtin(position) vec4f {
  let world = vec2f(dot(u.m0.xyz, vec3f(pos, 1.0)), dot(u.m1.xyz, vec3f(pos, 1.0)));
  return clip_of(world);
}

@fragment
fn fs_mesh() -> @location(0) vec4f {
  return to_output(u.v);
}

// ---------------------------------------------------------------- textured quads

struct TexOut {
  @builtin(position) pos: vec4f,
  @location(0) uv: vec2f,
};

@vertex
fn vs_tex(@builtin(vertex_index) vi: u32) -> TexOut {
  let corner = vec2f(f32(vi & 1u), f32((vi >> 1u) & 1u));
  let local = u.v.xy + corner * u.v.zw;
  let world = vec2f(dot(u.m0.xyz, vec3f(local, 1.0)), dot(u.m1.xyz, vec3f(local, 1.0)));
  var o: TexOut;
  o.pos = clip_of(world);
  o.uv = mix(u.uv.xy, u.uv.zw, corner);
  return o;
}

@fragment
fn fs_tex(i: TexOut) -> @location(0) vec4f {
  return to_output(textureSample(tex, samp, i.uv) * u.p.z);
}
