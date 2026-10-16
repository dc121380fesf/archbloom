//! 路径追踪 v0 —— spike 转正：BVH 加速 + NEE 直接光 + 余弦重要性采样 + firefly 钳制
//! 已知边界：纹理/roughness 不参与（平涂 + 镜面）；Sun 光并入自动补光；确定性 = 每行独立种子。
use crate::{review_triangles, Tri};
use archcore_ir::Scene;
use image::{Rgb, RgbImage};
use rayon::prelude::*;
use std::f64::consts::PI;

type V3 = [f64; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn mulv(a: V3, b: V3) -> V3 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(v: V3) -> V3 {
    let l = dot(v, v).sqrt().max(1e-12);
    scale(v, 1.0 / l)
}

struct Rng(u32);
impl Rng {
    fn new(seed: u32) -> Self {
        Rng(seed | 1)
    }
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    fn next_f32(&mut self) -> f64 {
        (self.next_u32() >> 8) as f64 / (1u32 << 24) as f64
    }
}

#[derive(Clone, Copy)]
struct Ray {
    o: V3,
    d: V3,
}
impl Ray {
    fn at(&self, t: f64) -> V3 {
        add(self.o, scale(self.d, t))
    }
}

// ---------- BVH（中位分割） ----------

struct BvhNode {
    bmin: V3,
    bmax: V3,
    left: i32,
    right: i32,
    start: u32,
    count: u32,
}

pub struct Bvh {
    nodes: Vec<BvhNode>,
    order: Vec<u32>,
    tris: Vec<Tri>,
}

/// 扁平化 BVH（GPU/外部消费）：三角按叶序重排后叶区间连续，节点数组 root=0
pub struct FlatNode {
    pub bmin: V3,
    pub bmax: V3,
    pub left: i32,
    pub right: i32,
    pub start: u32,
    pub count: u32,
}
pub struct FlatBvh {
    pub tris: Vec<Tri>,
    pub nodes: Vec<FlatNode>,
}

impl Bvh {
    /// 按叶序重排三角 + 节点数组（与 closest/occluded 遍历语义一一对应）
    pub fn flatten(&self) -> FlatBvh {
        let tris = self.order.iter().map(|&i| self.tris[i as usize]).collect();
        let nodes = self
            .nodes
            .iter()
            .map(|n| FlatNode {
                bmin: n.bmin,
                bmax: n.bmax,
                left: n.left,
                right: n.right,
                start: n.start,
                count: n.count,
            })
            .collect();
        FlatBvh { tris, nodes }
    }
}

fn tri_bounds(t: &Tri) -> (V3, V3) {
    let mut lo = t.p[0];
    let mut hi = t.p[0];
    for p in &t.p {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

fn centroid(t: &Tri) -> V3 {
    scale(add(add(t.p[0], t.p[1]), t.p[2]), 1.0 / 3.0)
}

impl Bvh {
    pub fn build(tris: Vec<Tri>) -> Self {
        let n = tris.len();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let mut nodes = Vec::new();
        if n > 0 {
            build_rec(&tris, &mut order, 0, n, &mut nodes);
        }
        Bvh { nodes, order, tris }
    }

    fn closest(&self, ray: &Ray) -> Option<(f64, usize, f64, f64)> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best: Option<(f64, usize, f64, f64)> = None;
        let mut stack = vec![0i32];
        while let Some(ni) = stack.pop() {
            let n = &self.nodes[ni as usize];
            if !ray_aabb(ray, n.bmin, n.bmax, best.map(|b| b.0).unwrap_or(f64::MAX)) {
                continue;
            }
            if n.count > 0 {
                for i in n.start..n.start + n.count {
                    let ti = self.order[i as usize] as usize;
                    if let Some((t, u, v)) = ray_tri(ray, &self.tris[ti]) {
                        if best.is_none_or(|b| t < b.0) {
                            best = Some((t, ti, u, v));
                        }
                    }
                }
            } else {
                stack.push(n.left);
                stack.push(n.right);
            }
        }
        best
    }

    fn occluded(&self, ray: &Ray, max_t: f64) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let mut stack = vec![0i32];
        while let Some(ni) = stack.pop() {
            let n = &self.nodes[ni as usize];
            if !ray_aabb(ray, n.bmin, n.bmax, max_t) {
                continue;
            }
            if n.count > 0 {
                for i in n.start..n.start + n.count {
                    let ti = self.order[i as usize] as usize;
                    if let Some((t, _, _)) = ray_tri(ray, &self.tris[ti]) {
                        if t > 1e-4 && t < max_t {
                            return true;
                        }
                    }
                }
            } else {
                stack.push(n.left);
                stack.push(n.right);
            }
        }
        false
    }
}

fn build_rec(tris: &[Tri], order: &mut Vec<u32>, lo: usize, hi: usize, nodes: &mut Vec<BvhNode>) -> i32 {
    let (mut cmin, mut cmax) = ([f64::MAX; 3], [f64::MIN; 3]);
    for &i in &order[lo..hi] {
        let c = centroid(&tris[i as usize]);
        for k in 0..3 {
            cmin[k] = cmin[k].min(c[k]);
            cmax[k] = cmax[k].max(c[k]);
        }
    }
    let idx = nodes.len() as i32;
    nodes.push(BvhNode {
        bmin: [0.0; 3],
        bmax: [0.0; 3],
        left: -1,
        right: -1,
        start: lo as u32,
        count: (hi - lo) as u32,
    });
    if hi - lo <= 4 {
        let (mut fmin, mut fmax) = ([f64::MAX; 3], [f64::MIN; 3]);
        for &i in &order[lo..hi] {
            let (a, b) = tri_bounds(&tris[i as usize]);
            for k in 0..3 {
                fmin[k] = fmin[k].min(a[k]);
                fmax[k] = fmax[k].max(b[k]);
            }
        }
        nodes[idx as usize].bmin = fmin;
        nodes[idx as usize].bmax = fmax;
        return idx;
    }
    let axis = (0..3).max_by(|&a, &b| {
        (cmax[a] - cmin[a])
            .partial_cmp(&(cmax[b] - cmin[b]))
            .unwrap_or(std::cmp::Ordering::Equal)
    }).unwrap();
    order[lo..hi].sort_by(|&a, &b| {
        centroid(&tris[a as usize])[axis]
            .partial_cmp(&centroid(&tris[b as usize])[axis])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mid = (lo + hi) / 2;
    let l = build_rec(tris, order, lo, mid, nodes);
    let r = build_rec(tris, order, mid, hi, nodes);
    let (lb, rb) = (&nodes[l as usize], &nodes[r as usize]);
    let mut bmin = lb.bmin;
    let mut bmax = lb.bmax;
    for k in 0..3 {
        bmin[k] = bmin[k].min(rb.bmin[k]);
        bmax[k] = bmax[k].max(rb.bmax[k]);
    }
    let n = &mut nodes[idx as usize];
    n.left = l;
    n.right = r;
    n.count = 0;
    n.bmin = bmin;
    n.bmax = bmax;
    idx
}

fn ray_aabb(ray: &Ray, bmin: V3, bmax: V3, max_t: f64) -> bool {
    let mut t0 = 0.0f64;
    let mut t1 = max_t;
    for k in 0..3 {
        let inv = 1.0 / ray.d[k];
        let mut a = (bmin[k] - ray.o[k]) * inv;
        let mut b = (bmax[k] - ray.o[k]) * inv;
        if inv < 0.0 {
            std::mem::swap(&mut a, &mut b);
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t1 < t0 {
            return false;
        }
    }
    true
}

/// Möller–Trumbore（返回 t 与重心坐标 u,v）
fn ray_tri(ray: &Ray, t: &Tri) -> Option<(f64, f64, f64)> {
    let e1 = sub(t.p[1], t.p[0]);
    let e2 = sub(t.p[2], t.p[0]);
    let pv = cross(ray.d, e2);
    let det = dot(e1, pv);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let tv = sub(ray.o, t.p[0]);
    let u = dot(tv, pv) * inv;
    if !(-1e-9..=1.0 + 1e-9).contains(&u) {
        return None;
    }
    let qv = cross(tv, e1);
    let v = dot(ray.d, qv) * inv;
    if v < -1e-9 || u + v > 1.0 + 1e-9 {
        return None;
    }
    let t0 = dot(e2, qv) * inv;
    if t0 > 1e-4 {
        Some((t0, u, v))
    } else {
        None
    }
}

// ---------- 贴图（base_color 采样：双线性 + wrap 平铺） ----------

/// 贴图（RGB 浮点缓存；GPU 轨从 pub 字段直接上传）
pub struct Tex {
    pub w: u32,
    pub h: u32,
    pub data: Vec<[f64; 3]>,
}

impl Tex {
    fn load(path: &str) -> Result<Tex, String> {
        let img = image::open(path)
            .map_err(|e| format!("贴图加载失败 {path}: {e}"))?
            .to_rgb8();
        let (w, h) = img.dimensions();
        let data = img
            .pixels()
            .map(|p| [p[0] as f64 / 255.0, p[1] as f64 / 255.0, p[2] as f64 / 255.0])
            .collect();
        Ok(Tex { w, h, data })
    }

    fn pixel(&self, x: i64, y: i64) -> V3 {
        let xi = ((x % self.w as i64) + self.w as i64) % self.w as i64;
        let yi = ((y % self.h as i64) + self.h as i64) % self.h as i64;
        self.data[(yi * self.w as i64 + xi) as usize]
    }

    /// 双线性 + wrap；v 轴翻转对齐图像行序
    fn sample(&self, u: f64, v: f64) -> V3 {
        let x = (u.fract() + 1.0).fract() * self.w as f64 - 0.5;
        let y = (1.0 - (v.fract() + 1.0).fract()) * self.h as f64 - 0.5;
        let x0 = x.floor() as i64;
        let y0 = y.floor() as i64;
        let fx = x - x0 as f64;
        let fy = y - y0 as f64;
        let c00 = self.pixel(x0, y0);
        let c10 = self.pixel(x0 + 1, y0);
        let c01 = self.pixel(x0, y0 + 1);
        let c11 = self.pixel(x0 + 1, y0 + 1);
        let top = add(scale(c00, 1.0 - fx), scale(c10, fx));
        let bot = add(scale(c01, 1.0 - fx), scale(c11, fx));
        add(scale(top, 1.0 - fy), scale(bot, fy))
    }
}

// ---------- 灯光（IR area/point；无灯自动补光） ----------

pub struct AreaLight {
    pub p0: V3,
    pub u: V3,
    pub v: V3,
    pub n: V3,
    pub le: V3,
    pub area: f64,
}

fn rotate_quat(q: [f64; 4], v: V3) -> V3 {
    let qv = [q[0], q[1], q[2]];
    let t = scale(cross(qv, v), 2.0);
    add(add(v, scale(t, q[3])), cross(qv, t))
}

fn lights_from_scene(scene: &Scene) -> Vec<AreaLight> {
    let mut out = Vec::new();
    for o in &scene.objects {
        if let archcore_ir::ObjectKind::Light {
            light_kind,
            color,
            intensity,
            size,
            ..
        } = &o.kind
        {
            if matches!(light_kind, archcore_ir::LightKind::Sun) {
                continue; // v0：sun 并入自动补光
            }
            let pos = o.transform.translation;
            let n = norm(rotate_quat(o.transform.rotation, [0.0, 0.0, -1.0]));
            let s = match light_kind {
                archcore_ir::LightKind::Point => 0.05,
                _ => size.unwrap_or(1.0),
            };
            let hint = if n[2].abs() > 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 0.0, 1.0] };
            let right = norm(cross(hint, n));
            let upv = cross(n, right);
            let u = scale(right, s);
            let v = scale(upv, s);
            let le = scale(*color, *intensity);
            out.push(AreaLight {
                p0: add(add(pos, scale(u, -0.5)), scale(v, -0.5)),
                u,
                v,
                n,
                le,
                area: cross(u, v).iter().map(|x| x * x).sum::<f64>().sqrt(),
            });
        }
    }
    out
}

fn auto_rig(tris: &[Tri]) -> Vec<AreaLight> {
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for t in tris {
        let (a, b) = tri_bounds(t);
        for k in 0..3 {
            lo[k] = lo[k].min(a[k]);
            hi[k] = hi[k].max(b[k]);
        }
    }
    let c = if tris.is_empty() {
        [0.0; 3]
    } else {
        scale(add(lo, hi), 0.5)
    };
    let r = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]).max(1.0);
    let size = r * 1.4;
    let u = [size, 0.0, 0.0];
    let v = [0.0, size, 0.0];
    let p0 = [c[0] - size / 2.0, c[1] - size / 2.0, c[2] + r * 1.8];
    vec![AreaLight {
        p0,
        u,
        v,
        n: [0.0, 0.0, -1.0],
        le: [5.0, 5.0, 5.0], // 尺度不变曝光：E∝le·area/dist²，le 恒定则与场景大小无关（此前 le=r*2.2 随尺度放大 → 大场景死白）
        area: size * size,
    }]
}

// ---------- 路径追踪核心（spike 蓝本） ----------

fn sample_light(l: &AreaLight, rng: &mut Rng) -> (V3, f64, V3) {
    let a = rng.next_f32();
    let b = rng.next_f32();
    let p = add(add(l.p0, scale(l.u, a)), scale(l.v, b));
    (p, 1.0 / l.area, l.le)
}

fn hit_light<'a>(lights: &'a [AreaLight], ray: &Ray) -> Option<(f64, &'a AreaLight)> {
    let mut best: Option<(f64, &AreaLight)> = None;
    for l in lights {
        let denom = dot(ray.d, l.n);
        if denom.abs() < 1e-9 {
            continue;
        }
        let t = dot(sub(l.p0, ray.o), l.n) / denom;
        if t < 1e-4 {
            continue;
        }
        let p = ray.at(t);
        let d = sub(p, l.p0);
        let a = dot(d, l.u) / dot(l.u, l.u);
        let b = dot(d, l.v) / dot(l.v, l.v);
        if (0.0..=1.0).contains(&a)
            && (0.0..=1.0).contains(&b)
            && best.is_none_or(|(bt, _)| t < bt)
        {
            best = Some((t, l));
        }
    }
    best
}

fn cosine_dir(n: V3, rng: &mut Rng) -> V3 {
    let r1 = rng.next_f32();
    let r2 = rng.next_f32();
    let phi = 2.0 * PI * r1;
    let t = r2.sqrt();
    let local = [t * phi.cos(), t * phi.sin(), (1.0 - r2).max(0.0).sqrt()];
    let w = n;
    let a = if w[0].abs() > 0.9 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let v = norm(cross(w, a));
    let u = cross(w, v);
    norm(add(add(scale(u, local[0]), scale(v, local[1])), scale(w, local[2])))
}

/// 天空环境光：出射线 miss 时的天穹辐射（地平线→天顶渐变 + 地面反射光）
fn sky(d: V3) -> V3 {
    let horizon = [0.58, 0.68, 0.82];
    let zenith = [0.16, 0.32, 0.70];
    let ground = [0.26, 0.24, 0.20];
    if d[2] >= 0.0 {
        let t = d[2].powf(0.55);
        [
            horizon[0] + (zenith[0] - horizon[0]) * t,
            horizon[1] + (zenith[1] - horizon[1]) * t,
            horizon[2] + (zenith[2] - horizon[2]) * t,
        ]
    } else {
        let t = (-d[2]).min(1.0).powf(0.7);
        [
            horizon[0] + (ground[0] - horizon[0]) * t,
            horizon[1] + (ground[1] - horizon[1]) * t,
            horizon[2] + (ground[2] - horizon[2]) * t,
        ]
    }
}

fn trace(bvh: &Bvh, lights: &[AreaLight], textures: &[Option<Tex>], ray: &Ray, rng: &mut Rng) -> V3 {
    let mut throughput = [1.0; 3];
    let mut acc = [0.0; 3];
    let mut ray = *ray;
    let mut specular = true;
    for _ in 0..5 {
        let hit_tri = bvh.closest(&ray);
        let hit_lamp = hit_light(lights, &ray);
        // 光源面片先于几何？
        if let Some((tl, l)) = hit_lamp {
            if hit_tri.is_none_or(|(tt, _, _, _)| tl < tt) {
                if specular {
                    acc = add(acc, mulv(throughput, l.le));
                }
                break;
            }
        }
        let Some((t, ti, bu, bv)) = hit_tri else {
            acc = add(acc, mulv(throughput, sky(ray.d)));
            break;
        };
        let tri = &bvh.tris[ti];
        let p = ray.at(t);
        // 贴图采样：重心插值 UV + 双线性（无贴图/无 UV 回退平涂）
        let albedo = match (tri.tex, tri.uv) {
            (Some(idx), Some(uvs)) => {
                let w0 = 1.0 - bu - bv;
                let u = w0 * uvs[0][0] + bu * uvs[1][0] + bv * uvs[2][0];
                let vv = w0 * uvs[0][1] + bu * uvs[1][1] + bv * uvs[2][1];
                textures
                    .get(idx as usize)
                    .and_then(|t| t.as_ref())
                    .map(|t| mulv(t.sample(u, vv), tri.color))
                    .unwrap_or(tri.color)
            }
            _ => tri.color,
        };
        let mut n = norm(cross(sub(tri.p[1], tri.p[0]), sub(tri.p[2], tri.p[0])));
        if dot(n, ray.d) > 0.0 {
            n = scale(n, -1.0);
        }
        // NEE：逐光源求和
        for l in lights {
            let (lp, lpdf, le) = sample_light(l, rng);
            let to_l = sub(lp, p);
            let dist2 = dot(to_l, to_l);
            let dist = dist2.sqrt();
            let wi = scale(to_l, 1.0 / dist);
            let cos_s = dot(wi, n).max(0.0);
            let cos_l = dot(wi, l.n).min(0.0).abs();
            if cos_s > 0.0
                && cos_l > 0.0
                && !bvh.occluded(&Ray { o: add(p, scale(n, 1e-3)), d: wi }, dist - 1e-3)
            {
                let brdf = scale(albedo, 1.0 / PI);
                let g = cos_s * cos_l / dist2;
                acc = add(acc, scale(mulv(mulv(throughput, brdf), le), g / lpdf));
            }
        }
        // 下一条光线
        if tri.mirror {
            let d = sub(ray.d, scale(n, 2.0 * dot(ray.d, n)));
            ray = Ray { o: add(p, scale(n, 1e-3)), d: norm(d) };
            throughput = mulv(throughput, albedo);
            specular = true;
        } else {
            ray = Ray { o: add(p, scale(n, 1e-3)), d: cosine_dir(n, rng) };
            throughput = mulv(throughput, albedo);
            specular = false;
        }
    }
    // firefly 钳制
    [
        acc[0].min(30.0),
        acc[1].min(30.0),
        acc[2].min(30.0),
    ]
}

// ---------- 渲染入口 ----------

fn auto_camera(tris: &[Tri]) -> (V3, V3, V3, V3) {
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for t in tris {
        let (a, b) = tri_bounds(t);
        for k in 0..3 {
            lo[k] = lo[k].min(a[k]);
            hi[k] = hi[k].max(b[k]);
        }
    }
    let c = if tris.is_empty() {
        [0.0, 0.0, 0.5]
    } else {
        scale(add(lo, hi), 0.5)
    };
    let r = dot(sub(hi, lo), sub(hi, lo)).sqrt() / 2.0;
    let dir = norm([1.0, 0.65, 0.9]);
    let eye = add(c, scale(dir, r.max(0.5) * 2.4));
    let fwd = norm(sub(c, eye));
    let right = norm(cross(fwd, [0.0, 0.0, 1.0]));
    let up = cross(right, fwd);
    (eye, fwd, right, up)
}

/// 渲染 IR 场景 → PNG 字节（默认曝光 1.0）
pub fn render_pathtraced(scene: &Scene, width: u32, height: u32, spp: usize) -> Result<Vec<u8>, String> {
    render_pathtraced_exposed(scene, width, height, spp, 1.0)
}

/// 场景准备（CPU/GPU 双轨同源）：BVH、灯光、贴图、相机一次抽取两轨共用
pub struct Prepared {
    pub bvh: Bvh,
    pub lights: Vec<AreaLight>,
    pub textures: Vec<Option<Tex>>,
    pub cam: (V3, V3, V3, V3, f64), // eye, fwd, right, up, fov
}

pub fn prepare_scene(scene: &Scene) -> Result<Prepared, String> {
    let (tris, tex_paths) = review_triangles(scene);
    let mut textures: Vec<Option<Tex>> = Vec::new();
    for pth in &tex_paths {
        textures.push(Some(Tex::load(pth)?));
    }
    let bvh = Bvh::build(tris.clone());
    let lights = {
        let l = lights_from_scene(scene);
        if l.is_empty() {
            auto_rig(&tris)
        } else {
            l
        }
    };

    // 相机：IR 相机优先，否则自动取景
    let cam = scene.objects.iter().find_map(|o| match &o.kind {
        archcore_ir::ObjectKind::Camera {
            lens_mm, sensor_mm, ..
        } => {
            let fwd = norm(rotate_quat(o.transform.rotation, [0.0, 0.0, -1.0]));
            let right = norm(cross(fwd, [0.0, 0.0, 1.0]));
            let up = cross(right, fwd);
            let fov = 2.0 * ((sensor_mm / 2.0) / lens_mm).atan();
            Some((o.transform.translation, fwd, right, up, fov))
        }
        _ => None,
    });
    let (eye, fwd, right, up, fov) = match cam {
        Some(c) => c,
        None => {
            let (eye, fwd, right, up) = auto_camera(&tris);
            (eye, fwd, right, up, 50f64.to_radians())
        }
    };
    Ok(Prepared {
        bvh,
        lights,
        textures,
        cam: (eye, fwd, right, up, fov),
    })
}

/// 渲染 IR 场景 → PNG 字节（确定性：每行独立种子；贴图加载失败显式报错；exposure 可调）
pub fn render_pathtraced_exposed(
    scene: &Scene,
    width: u32,
    height: u32,
    spp: usize,
    exposure: f64,
) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err(format!("渲染尺寸必须 > 0：{width}x{height}"));
    }
    if width > 16384 || height > 16384 {
        return Err(format!("渲染尺寸超上限 16384：{width}x{height}"));
    }
    if spp == 0 {
        return Err("spp 必须 > 0".to_string());
    }
    let prep = prepare_scene(scene)?;
    let (eye, fwd, right, up, fov) = prep.cam;
    let img = render_core(
        &prep.bvh,
        &prep.lights,
        &prep.textures,
        width,
        height,
        spp,
        eye,
        fwd,
        right,
        up,
        fov,
        exposure,
        Projection::Perspective,
    );
    Ok(encode_png(&img))
}

/// 投影模型：透视（fov）或等距柱状全景（图心=前方，上缘=天顶，x 向右扫 360°）
#[derive(Clone, Copy)]
pub enum Projection {
    Perspective,
    Equirect,
}

/// 全景渲染参数（等距柱状；stereo 时左右眼 SBS 并排，width 为总宽且须为偶数）
pub struct PanoramaOpts {
    pub width: u32,
    pub height: u32,
    pub spp: usize,
    pub exposure: f64,
    pub stereo: bool,
    pub ipd: f64,
}

pub(crate) fn panorama_check(opts: &PanoramaOpts) -> Result<(), String> {
    if opts.width == 0 || opts.height == 0 {
        return Err(format!("全景尺寸必须 > 0：{}x{}", opts.width, opts.height));
    }
    if opts.width > 16384 || opts.height > 16384 {
        return Err(format!("全景尺寸超上限 16384：{}x{}", opts.width, opts.height));
    }
    if opts.spp == 0 {
        return Err("spp 必须 > 0".to_string());
    }
    if opts.stereo && !opts.width.is_multiple_of(2) {
        return Err("立体全景宽度须为偶数（SBS 左右等宽）".to_string());
    }
    Ok(())
}

pub(crate) fn render_panorama_pixels(
    prep: &Prepared,
    opts: &PanoramaOpts,
) -> Result<RgbImage, String> {
    panorama_check(opts)?;
    let (eye0, fwd, right, up, fov) = prep.cam;
    let render_one = |eye: V3, w: u32| {
        render_core(
            &prep.bvh,
            &prep.lights,
            &prep.textures,
            w,
            opts.height,
            opts.spp,
            eye,
            fwd,
            right,
            up,
            fov,
            opts.exposure,
            Projection::Equirect,
        )
    };
    if !opts.stereo {
        return Ok(render_one(eye0, opts.width));
    }
    let half = opts.width / 2;
    let off = scale(right, opts.ipd / 2.0);
    let l = render_one(sub(eye0, off), half);
    let r = render_one(add(eye0, off), half);
    let mut img = RgbImage::new(opts.width, opts.height);
    for y in 0..opts.height {
        for x in 0..half {
            img.put_pixel(x, y, *l.get_pixel(x, y));
            img.put_pixel(x + half, y, *r.get_pixel(x, y));
        }
    }
    Ok(img)
}

/// 全景渲染 → PNG 字节（CPU 确定性轨）
pub fn render_panorama(scene: &Scene, opts: &PanoramaOpts) -> Result<Vec<u8>, String> {
    let prep = prepare_scene(scene)?;
    Ok(encode_png(&render_panorama_pixels(&prep, opts)?))
}

pub(crate) fn encode_png(img: &RgbImage) -> Vec<u8> {
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("PNG 编码失败");
    out
}

#[allow(clippy::too_many_arguments)]
fn render_core(
    bvh: &Bvh,
    lights: &[AreaLight],
    textures: &[Option<Tex>],
    width: u32,
    height: u32,
    spp: usize,
    eye: V3,
    fwd: V3,
    right: V3,
    up: V3,
    fov: f64,
    exposure: f64,
    proj: Projection,
) -> RgbImage {
    let fov_scale = (fov / 2.0).tan();
    let aspect = width as f64 / height as f64;
    let rows: Vec<Vec<[u8; 3]>> = (0..height)
        .into_par_iter()
        .map(|y| {
            let mut rng = Rng::new(y.wrapping_mul(9781).wrapping_add(12345));
            (0..width)
                .map(|x| {
                    let mut acc = [0.0; 3];
                    for _ in 0..spp {
                        let jx = rng.next_f32();
                        let jy = rng.next_f32();
                        let d = match proj {
                            Projection::Perspective => {
                                let px = (x as f64 + jx) / width as f64 * 2.0 - 1.0;
                                let py = 1.0 - (y as f64 + jy) / height as f64 * 2.0;
                                norm(add(
                                    add(fwd, scale(right, px * fov_scale * aspect)),
                                    scale(up, py * fov_scale),
                                ))
                            }
                            Projection::Equirect => {
                                let theta =
                                    ((x as f64 + jx) / width as f64 - 0.5) * std::f64::consts::TAU;
                                let phi =
                                    (0.5 - (y as f64 + jy) / height as f64) * std::f64::consts::PI;
                                let cp = phi.cos();
                                norm(add(
                                    add(scale(fwd, theta.cos() * cp), scale(right, theta.sin() * cp)),
                                    scale(up, phi.sin()),
                                ))
                            }
                        };
                        acc = add(acc, trace(bvh, lights, textures, &Ray { o: eye, d }, &mut rng));
                    }
                    let c = scale(acc, 1.0 / spp as f64);
                    [
                        to8(c[0], exposure),
                        to8(c[1], exposure),
                        to8(c[2], exposure),
                    ]
                })
                .collect()
        })
        .collect();
    let mut img = RgbImage::new(width, height);
    for (y, row) in rows.iter().enumerate() {
        for (x, px) in row.iter().enumerate() {
            img.put_pixel(x as u32, y as u32, Rgb(*px));
        }
    }
    img
}

/// ACES 近似色调映射 + gamma（曝光债的治本：高光优雅压缩，不再死白）
fn to8(c: f64, exposure: f64) -> u8 {
    let x = (c * exposure).max(0.0);
    let y = (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14);
    let v = y.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0;
    (v + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use archcore_ir::{Geometry, Object, ObjectKind, Transform, Units, UpAxis};
    use std::collections::BTreeMap;

    fn quad_scene(metallic: f64) -> Scene {
        let mat = archcore_ir::Material {
            id: "mat-0".into(),
            name: "M".into(),
            shading_model: archcore_ir::ShadingModel::PbrMetallicRoughness,
            base_color: [0.8, 0.3, 0.3, 1.0],
            metallic,
            roughness: 0.5,
            opacity: 1.0,
            textures: Default::default(),
        };
        Scene {
            schema_version: "0.1".into(),
            units: Units::Mm,
            up_axis: UpAxis::ZUp,
            name: "t".into(),
            objects: vec![Object {
                id: "obj-1".into(),
                name: "板".into(),
                kind: ObjectKind::Mesh {
                    geometry: Geometry {
                        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
                        indices: vec![0, 2, 1, 0, 3, 2],
                        normals: None,
                        uv_sets: vec![],
                    },
                    material_slots: vec!["mat-0".into()],
                },
                parent: None,
                transform: Transform::default(),
                visible: true,
                meta: BTreeMap::new(),
            }],
            materials: vec![mat],
            assets: vec![],
            meta: BTreeMap::new(),
        }
    }

    #[test]
    fn pathtrace_renders_non_trivial_png() {
        let png = render_pathtraced(&quad_scene(0.0), 48, 36, 4).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgb8();
        let mut colors = std::collections::BTreeSet::new();
        for p in img.pixels() {
            colors.insert((p[0] / 16, p[1] / 16, p[2] / 16));
        }
        assert!(colors.len() > 1);
    }

    /// 确定性承诺：同输入两次渲染字节一致（每行独立种子）
    #[test]
    fn pathtrace_is_deterministic() {
        let a = render_pathtraced(&quad_scene(0.0), 32, 24, 3).unwrap();
        let b = render_pathtraced(&quad_scene(0.0), 32, 24, 3).unwrap();
        assert_eq!(a, b);
    }

    /// 镜面材质路径不炸（metallic ≥ 0.5 走镜面反弹）
    #[test]
    fn mirror_material_renders() {
        let png = render_pathtraced(&quad_scene(1.0), 32, 24, 3).unwrap();
        assert!(image::load_from_memory(&png).is_ok());
    }
}
