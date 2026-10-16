// 产品 GPU 路径追踪内核：与 CPU trace() 同语义（5 弹射、逐光源 NEE、镜面、
// 光源可见性只算镜面路、firefly 钳制、ACES+exposure）。浮点/调度差异 => 非确定性，
// CPU 轨保持确定性参考（宪法双轨制）。
struct Tri {
  p0: vec4<f32>,
  p1: vec4<f32>,
  p2: vec4<f32>,
  col: vec4<f32>,
  nor: vec4<f32>,
  uv01: vec4<f32>, // u0,v0,u1,v1
  uv2m: vec4<f32>, // u2,v2, tex(+1)/0, mirror(0/1)
};
struct Node {
  bmin: vec4<f32>,
  bmax: vec4<f32>,
  data: vec4<i32>, // left,right,start,count(count>0 叶)
};
struct Light {
  p0: vec4<f32>,
  u: vec4<f32>,
  v: vec4<f32>,
  n: vec4<f32>,
  le: vec4<f32>,
};
struct Uni {
  eye: vec4<f32>,
  right: vec4<f32>,
  up: vec4<f32>,
  fwd: vec4<f32>,
  cam: vec4<f32>,    // fov_scale, aspect, exposure, unused
  dims: vec4<u32>,   // w, h, spp, bounces
  counts: vec4<u32>, // nlights, ntex, 0, 0
};

@group(0) @binding(0) var<storage, read> tris: array<Tri>;
@group(0) @binding(1) var<storage, read> nodes: array<Node>;
@group(0) @binding(2) var<storage, read> lights: array<Light>;
@group(0) @binding(3) var<storage, read> tex_hdr: array<vec4<u32>>; // offset, w, h, pad
@group(0) @binding(4) var<storage, read> tex_data: array<vec4<f32>>;
@group(0) @binding(5) var<uniform> u: Uni;
@group(0) @binding(6) var<storage, read_write> outpx: array<u32>;

fn next_rand(r: ptr<function, u32>) -> f32 {
  *r = *r * 747796405u + 2891336453u;
  var word = ((*r >> ((*r >> 28u) + 4u)) ^ *r) * 277803737u;
  word = (word >> 22u) ^ word;
  return f32(word) * (1.0 / 4294967296.0);
}

fn safe_inv(d: vec3<f32>) -> vec3<f32> {
  let s = select(vec3(-1.0), vec3(1.0), d >= vec3(0.0));
  let m = max(abs(d), vec3(1e-9));
  return s / m;
}

fn hit_tri_at(o: vec3<f32>, d: vec3<f32>, a: vec3<f32>, b: vec3<f32>, c: vec3<f32>, tmax: f32) -> vec4<f32> {
  let e1 = b - a;
  let e2 = c - a;
  let p = cross(d, e2);
  let det = dot(e1, p);
  if (abs(det) < 1e-10) { return vec4(-1.0, 0.0, 0.0, 0.0); }
  let inv = 1.0 / det;
  let tv = o - a;
  let bu = dot(tv, p) * inv;
  if (bu < 0.0 || bu > 1.0) { return vec4(-1.0, 0.0, 0.0, 0.0); }
  let q = cross(tv, e1);
  let bv = dot(d, q) * inv;
  if (bv < 0.0 || bu + bv > 1.0) { return vec4(-1.0, 0.0, 0.0, 0.0); }
  let t = dot(e2, q) * inv;
  if (t <= 1e-4 || t >= tmax) { return vec4(-1.0, 0.0, 0.0, 0.0); }
  return vec4(t, bu, bv, 1.0);
}

fn hit_box(o: vec3<f32>, di: vec3<f32>, n: u32, tmax: f32) -> f32 {
  let N = nodes[n];
  let t0 = (N.bmin.xyz - o) * di;
  let t1 = (N.bmax.xyz - o) * di;
  let tn = min(t0, t1);
  let tf = max(t0, t1);
  let tmin = max(max(tn.x, tn.y), max(tn.z, 0.0));
  let tm = min(min(tf.x, tf.y), min(tf.z, tmax));
  return select(-1.0, tmin, tmin <= tm);
}

fn closest(o: vec3<f32>, d: vec3<f32>) -> vec4<f32> { // t, u, v, triIdx(+1)/0=miss
  var stack: array<u32, 64>;
  var sp = 1;
  stack[0] = 0u; // 产品 Bvh root=0（与 CPU closest 同源）
  var best_t = 1e30;
  var best_i = -1.0;
  var best_uv = vec2(0.0);
  let di = safe_inv(d);
  while (sp > 0) {
    sp -= 1;
    let ni = stack[sp];
    if (hit_box(o, di, ni, best_t) < 0.0) { continue; }
    let N = nodes[ni];
    if (N.data.w > 0) {
      for (var k = 0; k < N.data.w; k = k + 1) {
        let ti = u32(N.data.z + k);
        let T = tris[ti];
        let h = hit_tri_at(o, d, T.p0.xyz, T.p1.xyz, T.p2.xyz, best_t);
        if (h.w > 0.5) {
          best_t = h.x;
          best_i = f32(ti);
          best_uv = h.yz;
        }
      }
    } else {
      stack[sp] = u32(N.data.x); sp = sp + 1;
      stack[sp] = u32(N.data.y); sp = sp + 1;
    }
  }
  return vec4(best_t, best_uv, best_i);
}

fn any_hit(o: vec3<f32>, d: vec3<f32>, max_t: f32) -> bool {
  var stack: array<u32, 64>;
  var sp = 1;
  stack[0] = 0u;
  let di = safe_inv(d);
  while (sp > 0) {
    sp -= 1;
    let ni = stack[sp];
    if (hit_box(o, di, ni, max_t) < 0.0) { continue; }
    let N = nodes[ni];
    if (N.data.w > 0) {
      for (var k = 0; k < N.data.w; k = k + 1) {
        let ti = u32(N.data.z + k);
        let T = tris[ti];
        let h = hit_tri_at(o, d, T.p0.xyz, T.p1.xyz, T.p2.xyz, max_t);
        if (h.w > 0.5) { return true; }
      }
    } else {
      stack[sp] = u32(N.data.x); sp = sp + 1;
      stack[sp] = u32(N.data.y); sp = sp + 1;
    }
  }
  return false;
}

fn wrapi(x: i32, m: i32) -> i32 {
  return ((x % m) + m) % m;
}

/// 双线性 + wrap + v 翻转（与 CPU Tex::sample 逐行同义）
fn tex_sample(idx: u32, uu: f32, vv: f32) -> vec3<f32> {
  let h = tex_hdr[idx];
  let off = h.x;
  let wi = i32(h.y);
  let hi = i32(h.z);
  let tw = f32(h.y);
  let th = f32(h.z);
  let x = (uu - floor(uu)) * tw - 0.5;
  let y = (1.0 - (vv - floor(vv))) * th - 0.5;
  let x0 = i32(floor(x));
  let y0 = i32(floor(y));
  let fx = x - f32(x0);
  let fy = y - f32(y0);
  let ix0 = wrapi(x0, wi);
  let ix1 = wrapi(x0 + 1, wi);
  let iy0 = wrapi(y0, hi);
  let iy1 = wrapi(y0 + 1, hi);
  let c00 = tex_data[off + u32(iy0 * wi + ix0)].rgb;
  let c10 = tex_data[off + u32(iy0 * wi + ix1)].rgb;
  let c01 = tex_data[off + u32(iy1 * wi + ix0)].rgb;
  let c11 = tex_data[off + u32(iy1 * wi + ix1)].rgb;
  let top = c00 * (1.0 - fx) + c10 * fx;
  let bot = c01 * (1.0 - fx) + c11 * fx;
  return top * (1.0 - fy) + bot * fy;
}

fn cosine_hemi(n: vec3<f32>, r: ptr<function, u32>) -> vec3<f32> {
  let r1 = next_rand(r);
  let r2 = next_rand(r);
  let sq = sqrt(r1);
  let phi = 6.2831853 * r2;
  let s = sqrt(max(0.0, 1.0 - r1));
  let upv = select(vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0), abs(n.z) < 0.9);
  let tx = normalize(cross(upv, n));
  let ty = cross(n, tx);
  return normalize(tx * (sq * cos(phi)) + ty * (sq * sin(phi)) + n * s);
}

/// 光源四边形最近命中（两三角），返回 t（<0 = 未中）
fn hit_light_quad(o: vec3<f32>, d: vec3<f32>, L: Light, tmax: f32) -> f32 {
  let a = L.p0.xyz;
  let b = L.p0.xyz + L.u.xyz;
  let c = L.p0.xyz + L.u.xyz + L.v.xyz;
  let e = L.p0.xyz + L.v.xyz;
  let h1 = hit_tri_at(o, d, a, b, c, tmax);
  let h2 = hit_tri_at(o, d, a, c, e, tmax);
  var t = -1.0;
  if (h1.w > 0.5) { t = h1.x; }
  if (h2.w > 0.5 && (t < 0.0 || h2.x < t)) { t = h2.x; }
  return t;
}

fn sky(d: vec3<f32>) -> vec3<f32> {
  let horizon = vec3<f32>(0.58, 0.68, 0.82);
  let zenith = vec3<f32>(0.16, 0.32, 0.70);
  let ground = vec3<f32>(0.26, 0.24, 0.20);
  if (d.z >= 0.0) {
    let t = pow(d.z, 0.55);
    return mix(horizon, zenith, vec3<f32>(t));
  }
  let t = pow(min(-d.z, 1.0), 0.7);
  return mix(horizon, ground, vec3<f32>(t));
}

fn trace(ro0: vec3<f32>, d0: vec3<f32>, r: ptr<function, u32>) -> vec3<f32> {
  var thr = vec3(1.0);
  var acc = vec3(0.0);
  var ro = ro0;
  var d = d0;
  var specular = 1.0;
  for (var b = 0; b < i32(u.dims.w); b = b + 1) {
    let h = closest(ro, d);
    // 光源面片先于几何？（可见性只算镜面路，与 CPU 同）
    var lt = 1e30;
    var lle = vec3(0.0);
    var lhit = false;
    for (var i = 0u; i < u.counts.x; i = i + 1u) {
      let t = hit_light_quad(ro, d, lights[i], min(h.x, lt));
      if (t > 0.0 && t < lt) {
        lt = t;
        lle = lights[i].le.xyz;
        lhit = true;
      }
    }
    if (lhit && lt < h.x) {
      if (specular > 0.5) {
        acc = acc + thr * lle;
      }
      break;
    }
    if (h.w < 0.0) {
      acc = acc + thr * sky(d);
      break;
    }
    let T = tris[u32(h.w)];
    let p = ro + d * h.x;
    // albedo：重心插值 UV + 双线性 × base_color（无贴图回退平涂）
    var albedo = T.col.rgb;
    if (T.uv2m.z > 0.5) {
      let w0 = 1.0 - h.y - h.z;
      let uu = w0 * T.uv01.x + h.y * T.uv01.z + h.z * T.uv2m.x;
      let vv = w0 * T.uv01.y + h.y * T.uv01.w + h.z * T.uv2m.y;
      albedo = tex_sample(u32(T.uv2m.z) - 1u, uu, vv) * T.col.rgb;
    }
    var n = T.nor.xyz;
    if (dot(n, d) > 0.0) { n = -n; }
    // NEE：逐光源均匀采样（pdf = 1/area）
    for (var i = 0u; i < u.counts.x; i = i + 1u) {
      let L = lights[i];
      let sa = next_rand(r);
      let sb = next_rand(r);
      let lp = L.p0.xyz + L.u.xyz * sa + L.v.xyz * sb;
      let to_l = lp - p;
      let dist2 = dot(to_l, to_l);
      let dist = sqrt(dist2);
      let wi = to_l / max(dist, 1e-9);
      let cos_s = max(dot(wi, n), 0.0);
      let cos_l = abs(min(dot(wi, L.n.xyz), 0.0));
      if (cos_s > 0.0 && cos_l > 0.0 && !any_hit(p + n * 1e-3, wi, dist - 1e-3)) {
        let area = length(cross(L.u.xyz, L.v.xyz));
        let brdf = albedo / 3.14159265;
        let g = cos_s * cos_l / dist2;
        acc = acc + thr * brdf * L.le.xyz * (g * area);
      }
    }
    // 下一条光线
    if (T.uv2m.w > 0.5) {
      d = normalize(d - n * 2.0 * dot(d, n));
      ro = p + n * 1e-3;
      thr = thr * albedo;
      specular = 1.0;
    } else {
      d = cosine_hemi(n, r);
      ro = p + n * 1e-3;
      thr = thr * albedo;
      specular = 0.0;
    }
  }
  return min(acc, vec3(30.0));
}

fn aces(x: vec3<f32>) -> vec3<f32> {
  let xx = max(x, vec3(0.0));
  return (xx * (2.51 * xx + vec3(0.03))) / (xx * (2.43 * xx + vec3(0.59)) + vec3(0.14));
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (gid.x >= u.dims.x || gid.y >= u.dims.y) { return; }
  var rng = gid.x * 1973u + gid.y * 9277u + 26699u;
  var acc = vec3(0.0);
  for (var s = 0u; s < u.dims.z; s = s + 1u) {
    let jx = next_rand(&rng);
    let jy = next_rand(&rng);
    var d: vec3<f32>;
    if (u.cam.w > 0.5) {
      // 等距柱状全景（与 CPU Equirect 同数学）
      let theta = ((f32(gid.x) + jx) / f32(u.dims.x) - 0.5) * 6.2831853;
      let phi = (0.5 - (f32(gid.y) + jy) / f32(u.dims.y)) * 3.14159265;
      let cp = cos(phi);
      d = normalize(u.fwd.xyz * (cos(theta) * cp) + u.right.xyz * (sin(theta) * cp) + u.up.xyz * sin(phi));
    } else {
      let px = ((f32(gid.x) + jx) / f32(u.dims.x) * 2.0 - 1.0) * u.cam.x * u.cam.y;
      let py = (1.0 - (f32(gid.y) + jy) / f32(u.dims.y) * 2.0) * u.cam.x;
      d = normalize(u.fwd.xyz + u.right.xyz * px + u.up.xyz * py);
    }
    acc = acc + trace(u.eye.xyz, d, &rng);
  }
  var c = acc / f32(u.dims.z) * u.cam.z;
  c = clamp(aces(c), vec3(0.0), vec3(1.0));
  c = pow(c, vec3(1.0 / 2.2));
  outpx[gid.y * u.dims.x + gid.x] = pack4x8unorm(vec4(c, 1.0));
}
