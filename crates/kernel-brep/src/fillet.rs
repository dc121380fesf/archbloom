//! 圆角（fillet）：把锐边换成圆弧过渡。
//!
//! # 按 SMLib 的流程重写
//!
//! 参考 NVIDIA SMLib 文档（<https://docs.nvidia.com/smlib/manual/smlib/fillets>）：
//!
//! > "**Rail Curves** are calculated first, before the fillet surface itself, and their
//! > intersections with other body topology **largely determine the overall filleting
//! > situation**."
//!
//! 所以顺序是：**轨线 → 圆角面 → 角部**。不是「每条边算一对切点」。
//!
//! # 三个核心概念
//!
//! ## 轨线（Rail Curve）
//!
//! 圆角面与原始面的交线。对一条边、一个侧面：
//!
//! ```text
//! 轨线 = 面上距边 R 的直线（平行于边）
//! ```
//!
//! 轨线一直延伸到边的两端，在角部与**球面相交**——那个交点才是真正的边界终点。
//!
//! ## 球心（顶点处）
//!
//! 盒子的角是 SMLib 分类里的 **NxN Same-Convexity Corner**，原文说
//! "the corner will be an analytical surface (**Sphere**)"。
//!
//! 球心要**精确求解**：到顶点周围每个面的距离都等于 R。
//!
//! ```text
//! 对每个相邻面 i：(c - v) · m_i = R      （m_i = -n_i 是内法线）
//! 三个面 → 三元一次方程组 → Cramer 法则
//! ```
//!
//! 盒子角 (0,0,0)、R=0.2 时解出 `c = (0.2, 0.2, 0.2)`——到三个面都是 R。
//!
//! ## 轨线端点 = 轨线 ∩ 球面
//!
//! 把轨线参数化成 `p(t) = p0 + t·e`，代入 `|p - c|² = R²` 解二次方程，
//! 取靠近边的那个根。
//!
//! # 为什么不能复用倒角
//!
//! 倒角的内缩点是「面内两个方向都退 R」的交点 `(0.2, 0.2, 0)`；
//! 圆角的轨线端点是「只退一个方向，再与球面求交」`(0.2, 0, 0)` 附近。
//! 两者不同——复用过一次，弧的半径从 0.2 变成 0.346，体积差 39%。

use std::collections::HashMap;

use archcore_kernel::{KernelError, Point3, Vector3};

use crate::chamfer::edge_table;
use crate::geom::{add, cross, dot, normalize, scale, sub, Surface};
use crate::solid::{Face, Solid};

/// 圆角参数。
#[derive(Clone, Copy, Debug)]
pub struct FilletOptions {
    pub radius: f64,
    /// 圆弧离散段数。
    pub segments: usize,
}

impl Default for FilletOptions {
    fn default() -> Self {
        Self {
            radius: 0.001,
            segments: 8,
        }
    }
}

/// 一条边的两条轨线（每侧一条）。
struct Rails {
    /// 面 f1 上的轨线：起点、方向（已延伸到球面端点）
    f1_start: Point3,
    f1_end: Point3,
    /// 面 f2 上
    f2_start: Point3,
    f2_end: Point3,
    /// 圆角轴心线两端（圆心）
    ca: Point3,
    cb: Point3,
    m1: Vector3,
    m2: Vector3,
}

/// 对实体做圆角。返回新实体。
pub fn fillet_solid(
    solid: &Solid,
    edges: &[u64],
    opts: &FilletOptions,
) -> Result<Solid, KernelError> {
    if opts.radius <= 0.0 || !opts.radius.is_finite() {
        return Err(KernelError::Degenerate {
            at: [0.0, 0.0, 0.0],
            reason: format!("圆角半径必须为正，收到 {}", opts.radius),
            suggestion: "传入大于 0 的半径，例如 0.001（R1mm）".to_string(),
        });
    }
    let table = edge_table(solid)?;
    if table.is_empty() {
        return Err(KernelError::TopologyBroken {
            at: "fillet".to_string(),
            reason: "边表为空".to_string(),
            suggestion: "检查实体构造".to_string(),
        });
    }

    let selected: Vec<bool> = if edges.is_empty() {
        table.iter().map(|e| e.sharp).collect()
    } else {
        let mut want = vec![false; table.len()];
        for &id in edges {
            let idx = id as usize;
            if idx >= table.len() {
                return Err(KernelError::Degenerate {
                    at: [0.0, 0.0, 0.0],
                    reason: format!("边号 {id} 超出范围（共 {} 条）", table.len()),
                    suggestion: format!("边号范围是 0..{}", table.len()),
                });
            }
            want[idx] = table[idx].sharp;
        }
        want
    };
    if !selected.iter().any(|s| *s) {
        return Ok(solid.clone());
    }

    let r = opts.radius;

    // ── 1. 每个顶点的球心 ──
    // 顶点 → (球心, 相邻面列表)
    let mut spheres: HashMap<u32, (Point3, Vec<usize>)> = HashMap::new();
    let mut verts_of_edge: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, e) in table.iter().enumerate() {
        if !selected[i] {
            continue;
        }
        for v in [e.a, e.b] {
            let entry = verts_of_edge.entry(v).or_default();
            if !entry.contains(&i) {
                entry.push(i);
            }
        }
    }
    for (&v, edge_ids) in &verts_of_edge {
        let mut faces: Vec<usize> = Vec::new();
        for &ei in edge_ids {
            for fi in [table[ei].f1, table[ei].f2] {
                if !faces.contains(&fi) {
                    faces.push(fi);
                }
            }
        }
        if faces.len() < 3 {
            // 只有两个面参与（单边圆角）：角部是「轨线延伸到相邻边」，另算
            continue;
        }
        let mut ns: Vec<Vector3> = faces.iter().map(|&fi| face_normal(solid, fi)).collect();
        // 取前三个面解 3x3
        ns.truncate(3);
        let pv = solid.verts[v as usize];
        if let Some(c) = solve_sphere_center(pv, &ns, r) {
            spheres.insert(v, (c, faces));
        }
    }

    // ── 2. 每条边的两条轨线（端点与球面求交） ──
    let mut rails: HashMap<(u32, u32), Rails> = HashMap::new();
    for (i, e) in table.iter().enumerate() {
        if !selected[i] {
            continue;
        }
        let n1 = face_normal(solid, e.f1);
        let n2 = face_normal(solid, e.f2);
        let m1 = scale(n1, -1.0);
        let m2 = scale(n2, -1.0);
        let Some(bis) = normalize(add(m1, m2)) else {
            continue;
        };
        let alpha = dot(n1, n2).clamp(-1.0, 1.0).acos();
        let theta = std::f64::consts::PI - alpha;
        let sin_half = (theta * 0.5).sin();
        if sin_half.abs() < 1e-9 {
            continue;
        }
        let d = r / sin_half;
        let pa = solid.verts[e.a as usize];
        let pb = solid.verts[e.b as usize];
        let ca = add(pa, scale(bis, d));
        let cb = add(pb, scale(bis, d));

        // 轨线：面上距边 R、平行于边的直线。
        //
        // 方向靠「边点 → 圆心」再投影到面内得到。
        // **不能用 cross(n, edir)**——它的符号依赖环方向，盒子两个面一个对一个反，
        // 实测体积会超过原体积（8.01 / 8.65），说明弧朝外凸了。
        let edir = normalize(sub(pb, pa)).unwrap_or([1.0, 0.0, 0.0]);
        let dir1 = {
            let d = sub(ca, pa);
            let proj = sub(d, scale(n1, dot(d, n1)));
            normalize(proj).unwrap_or([0.0, 0.0, 1.0])
        };
        let dir2 = {
            let d = sub(cb, pb);
            let proj = sub(d, scale(n2, dot(d, n2)));
            normalize(proj).unwrap_or([0.0, 0.0, 1.0])
        };
        // 轨线起点：距边 R
        let rp1 = add(pa, scale(dir1, r));
        let rp2 = add(pa, scale(dir2, r));

        // 端点：与两端球面求交
        let a1 = trim_to_sphere(rp1, edir, spheres.get(&e.a), r).unwrap_or(rp1);
        let b1 = trim_to_sphere(
            add(pb, scale(dir1, r)),
            edir,
            spheres.get(&e.b),
            r,
        )
        .unwrap_or(add(pb, scale(dir1, r)));
        let a2 = trim_to_sphere(rp2, edir, spheres.get(&e.a), r).unwrap_or(rp2);
        let b2 = trim_to_sphere(
            add(pb, scale(dir2, r)),
            edir,
            spheres.get(&e.b),
            r,
        )
        .unwrap_or(add(pb, scale(dir2, r)));

        rails.insert(
            (e.a, e.b),
            Rails {
                f1_start: a1,
                f1_end: b1,
                f2_start: a2,
                f2_end: b2,
                ca,
                cb,
                m1,
                m2,
            },
        );
    }
    let _ = verts_of_edge;

    // ── 3. 组装 ──
    let mut out = Solid::default();
    let mut weld: HashMap<(i64, i64, i64), u32> = HashMap::new();
    macro_rules! put {
        ($p:expr) => {{
            let p: Point3 = $p;
            let k = (
                (p[0] / 1e-9).round() as i64,
                (p[1] / 1e-9).round() as i64,
                (p[2] / 1e-9).round() as i64,
            );
            if let Some(&i) = weld.get(&k) {
                i
            } else {
                let i = out.push_vert(p);
                weld.insert(k, i);
                i
            }
        }};
    }

    // 3a) 原面：顶点被圆角边连着的，换成轨线端点
    // (面, 原顶点) → 该面上的轨线端点
    let mut cut: HashMap<(usize, u32), Point3> = HashMap::new();
    for (i, e) in table.iter().enumerate() {
        if !selected[i] {
            continue;
        }
        let Some(rl) = rails.get(&(e.a, e.b)) else {
            continue;
        };
        cut.insert((e.f1, e.a), rl.f1_start);
        cut.insert((e.f1, e.b), rl.f1_end);
        cut.insert((e.f2, e.a), rl.f2_start);
        cut.insert((e.f2, e.b), rl.f2_end);
    }
    for (fi, f) in solid.faces.iter().enumerate() {
        let n = f.verts.len();
        let mut ring: Vec<u32> = Vec::with_capacity(n);
        for i in 0..n {
            let cur = f.verts[i];
            match cut.get(&(fi, cur)) {
                Some(p) => ring.push(put!(*p)),
                None => ring.push(put!(solid.verts[cur as usize])),
            }
        }
        let ring = dedup(ring);
        if ring.len() >= 3 {
            out.faces.push(Face {
                surface: f.surface,
                verts: ring,
                holes: Vec::new(),
            });
        }
    }

    // 3b) 每条边：segments 段弧
    for (i, e) in table.iter().enumerate() {
        if !selected[i] {
            continue;
        }
        let Some(rl) = rails.get(&(e.a, e.b)) else {
            continue;
        };
        let seg = opts.segments.max(2);
        let axis = normalize(sub(rl.cb, rl.ca)).unwrap_or([0.0, 0.0, 1.0]);
        let from_dir = normalize(sub(rl.f1_start, rl.ca)).unwrap_or(rl.m1);
        let to_dir = normalize(sub(rl.f2_start, rl.ca)).unwrap_or(rl.m2);
        let total = dot(from_dir, to_dir).clamp(-1.0, 1.0).acos();
        let sign = if dot(cross(from_dir, to_dir), axis) >= 0.0 {
            1.0
        } else {
            -1.0
        };
        let mut prev_a = put!(rl.f1_start);
        let mut prev_b = put!(rl.f1_end);
        for s in 1..=seg {
            let t = s as f64 / seg as f64;
            let ang = total * t * sign;
            let dir = rotate(from_dir, axis, ang);
            let cur_a = add(rl.ca, scale(dir, r));
            let cur_b = add(rl.cb, scale(dir, r));
            let va = if s == seg {
                put!(rl.f2_start)
            } else {
                put!(cur_a)
            };
            let vb = if s == seg {
                put!(rl.f2_end)
            } else {
                put!(cur_b)
            };
            let quad = dedup(vec![prev_a, prev_b, vb, va]);
            if quad.len() >= 3 {
                let nrm = plane_normal(&out, &quad);
                out.faces.push(Face {
                    surface: Surface::Plane {
                        origin: out.verts[quad[0] as usize],
                        normal: nrm,
                    },
                    verts: quad,
                    holes: Vec::new(),
                });
            }
            prev_a = va;
            prev_b = vb;
        }
    }

    // 3c) 角部：球面片
    // 每个顶点，收集它周围所有「轨线端点」，按绕内法线角度排序，
    // 以球心为顶点做扇形三角（球面三角的离散）。
    for (&v, (c, faces_at_v)) in &spheres {
        let pv = solid.verts[v as usize];
        let mut ring_pts: Vec<Point3> = Vec::new();
        for (i, e) in table.iter().enumerate() {
            if !selected[i] || (e.a != v && e.b != v) {
                continue;
            }
            let Some(rl) = rails.get(&(e.a, e.b)) else {
                continue;
            };
            let (p1, p2) = if e.a == v {
                (rl.f1_start, rl.f2_start)
            } else {
                (rl.f1_end, rl.f2_end)
            };
            ring_pts.push(p1);
            ring_pts.push(p2);
        }
        // 去重
        let mut uniq: Vec<Point3> = Vec::new();
        for p in ring_pts {
            if !uniq.iter().any(|q| dist2(*q, p) < 1e-18) {
                uniq.push(p);
            }
        }
        if uniq.len() < 3 {
            continue;
        }
        // 绕内法线排序
        let mut inner = [0.0; 3];
        for &fi in faces_at_v {
            let n = face_normal(solid, fi);
            for k in 0..3 {
                inner[k] -= n[k];
            }
        }
        let Some(in_n) = normalize(inner) else { continue };
        let Some((u, w)) = crate::geom::orthonormal_basis(in_n) else {
            continue;
        };
        let mut keyed: Vec<(f64, Point3)> = uniq
            .into_iter()
            .map(|p| {
                let d = sub(p, pv);
                (dot(d, w).atan2(dot(d, u)), p)
            })
            .collect();
        keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        // 角部：**不能用球心当顶点**——球心在实体内部，
        // 把它当三角顶点会让三角面穿进实体（实测体积从 7.8 掉到 5.4）。
        //
        // 球面片的正确做法：边界就是轨道端点在球面上的那段弧，
        // 面本身是球面的一部分。这里先用“球面上端点组成的多边形”近似（一个平面面），
        // 弓高误差在 R 量级，比穿进实体好得多。
        let ring: Vec<u32> = keyed
            .iter()
            .map(|(_, p)| {
                let q = normalize(sub(*p, *c))
                    .map(|d| add(*c, scale(d, r)))
                    .unwrap_or(*p);
                put!(q)
            })
            .collect();
        let ring = dedup(ring);
        if ring.len() >= 3 {
            let nrm = plane_normal(&out, &ring);
            out.faces.push(Face {
                surface: Surface::Plane {
                    origin: out.verts[ring[0] as usize],
                    normal: nrm,
                },
                verts: ring,
                holes: Vec::new(),
            });
        }
    }

    out.analytic_volume = None;
    out.analytic_area = None;
    Ok(out)
}

/// 解球心：`(c - v) · m_i = R`，三个面 → 三元一次方程组。
///
/// 用 Cramer 法则解 3x3。行列式太小时返回 None（三面近乎共面）。
fn solve_sphere_center(v: Point3, normals: &[Vector3], r: f64) -> Option<Point3> {
    if normals.len() < 3 {
        return None;
    }
    let m: Vec<Vector3> = normals.iter().map(|n| scale(*n, -1.0)).collect();
    let (a1, a2, a3) = (m[0], m[1], m[2]);
    let det = dot(a1, cross(a2, a3));
    if det.abs() < 1e-12 {
        return None;
    }
    // RHS 全为 r（因为 c·m_i - v·m_i = R → c·m_i = R + v·m_i）
    let b: Vec<f64> = (0..3).map(|i| r + dot(v, m[i])).collect();
    // Cramer
    let x = dot([b[0], b[1], b[2]], cross(a2, a3)) / det;
    let y = dot(a1, cross([b[0], b[1], b[2]], a3)) / det;
    let z = dot(a1, cross(a2, [b[0], b[1], b[2]])) / det;
    Some([x, y, z])
}

/// 把轨线起点沿边方向推到球面上（取靠边的根）。
fn trim_to_sphere(
    p0: Point3,
    dir: Vector3,
    sphere: Option<&(Point3, Vec<usize>)>,
    r: f64,
) -> Option<Point3> {
    let (c, _) = sphere?;
    // |p0 + t*dir - c|² = r²
    let d = sub(p0, *c);
    let a = dot(dir, dir);
    let b = 2.0 * dot(d, dir);
    let cc = dot(d, d) - r * r;
    let disc = b * b - 4.0 * a * cc;
    if disc < 0.0 {
        return None;
    }
    let sq = disc.sqrt();
    let t1 = (-b + sq) / (2.0 * a);
    let t2 = (-b - sq) / (2.0 * a);
    // 取较小的正根（离边最近）
    let t = if t1.abs() < t2.abs() {
        t1
    } else {
        t2
    };
    Some(add(p0, scale(dir, t)))
}

fn rotate(v: Vector3, axis: Vector3, ang: f64) -> Vector3 {
    let k = normalize(axis).unwrap_or([0.0, 0.0, 1.0]);
    let c = ang.cos();
    let s = ang.sin();
    let kv = cross(k, v);
    let kd = dot(k, v);
    normalize([
        v[0] * c + kv[0] * s + k[0] * kd * (1.0 - c),
        v[1] * c + kv[1] * s + k[1] * kd * (1.0 - c),
        v[2] * c + kv[2] * s + k[2] * kd * (1.0 - c),
    ])
    .unwrap_or(v)
}

fn dist2(a: Point3, b: Point3) -> f64 {
    let d = sub(a, b);
    dot(d, d)
}

fn dedup(ring: Vec<u32>) -> Vec<u32> {
    let mut o: Vec<u32> = Vec::with_capacity(ring.len());
    for v in ring {
        if o.last() != Some(&v) {
            o.push(v);
        }
    }
    while o.len() > 1 && o.first() == o.last() {
        o.pop();
    }
    o
}

fn face_normal(solid: &Solid, fi: usize) -> Vector3 {
    let f = &solid.faces[fi];
    if f.verts.is_empty() {
        return [0.0, 0.0, 1.0];
    }
    f.surface
        .normal_at(solid.verts[f.verts[0] as usize])
        .or_else(|| polygon_normal(solid, &f.verts))
        .unwrap_or([0.0, 0.0, 1.0])
}

fn polygon_normal(solid: &Solid, ring: &[u32]) -> Option<Vector3> {
    if ring.len() < 3 {
        return None;
    }
    let a = solid.verts[ring[0] as usize];
    let b = solid.verts[ring[1] as usize];
    let c = solid.verts[ring[2] as usize];
    normalize(cross(sub(b, a), sub(c, a)))
}

fn plane_normal(out: &Solid, ring: &[u32]) -> Vector3 {
    let a = out.verts[ring[0] as usize];
    let b = out.verts[ring[1] as usize];
    let c = out.verts[ring[2] as usize];
    normalize(cross(sub(b, a), sub(c, a))).unwrap_or([0.0, 0.0, 1.0])
}
