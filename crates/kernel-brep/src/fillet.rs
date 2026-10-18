//! 圆角（fillet）：把锐边换成圆弧过渡。
//!
//! # 与倒角的区别
//!
//! | | chamfer | fillet |
//! |---|---|---|
//! | 连接面 | 一个平面 | 一段圆弧 |
//! | 图纸 | `C0.5` | `R0.5` |
//! | 加工 | 铣刀斜切 | 球头刀走弧 |
//!
//! # 几何（凸边）
//!
//! ```text
//! m1 = -n1, m2 = -n2        （内法线）
//! bis = normalize(m1 + m2)  （角平分线）
//! θ  = π - acos(n1·n2)      （内部二面角）
//! c  = 边点 + bis * (R / sin(θ/2))   圆心
//! p1 = c - R*m1            切点，落在 f1 上
//! p2 = c - R*m2            切点，落在 f2 上
//! ```
//!
//! 90° 凸边 R=0.2 时 `c=(0.2,0.2)`，切点 `(0.2,0)` `(0,0.2)`——距边都是 R。
//!
//! # 当前实现：离散逼近
//!
//! 圆弧离散成 `segments` 段平面条，先验证几何（体积/水密/欧拉）。
//! 真圆柱面版本等离散版验收通过再做——STEP 那边已经能写
//! `CYLINDRICAL_SURFACE` 了，缺的是把这里每段平面换成一段弧。

use std::collections::HashMap;

use archcore_kernel::{KernelError, Point3, Vector3};

use crate::chamfer::edge_table;
use crate::geom::{add, cross, dot, normalize, scale, sub, Surface};
use crate::solid::{Face, Solid};

/// 圆角参数。
#[derive(Clone, Copy, Debug)]
pub struct FilletOptions {
    pub radius: f64,
    /// 每条边的圆弧用几段平面逼近。
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

/// 一条边的圆角几何。
struct Fillet {
    /// 只用于调试与后续的角部处理（当前实现未读）。
    #[allow(dead_code)]
    a: u32,
    #[allow(dead_code)]
    b: u32,
    #[allow(dead_code)]
    f1: usize,
    #[allow(dead_code)]
    f2: usize,
    m1: Vector3,
    m2: Vector3,
    ca: Point3,
    cb: Point3,
    /// 两端在 f1 上的切点
    a1: Point3,
    b1: Point3,
    /// 两端在 f2 上的切点（角部处理要用）
    #[allow(dead_code)]
    a2: Point3,
    #[allow(dead_code)]
    b2: Point3,
}

/// 对实体做圆角（离散逼近）。
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

    // ── 1. 每条选中边的圆角几何 ──
    let mut fillets: Vec<Fillet> = Vec::new();
    // (面, 顶点) → 该面上的切点
    let mut cuts: HashMap<(usize, u32), Vec<Point3>> = HashMap::new();

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
        let d = opts.radius / sin_half;
        let pa = solid.verts[e.a as usize];
        let pb = solid.verts[e.b as usize];
        let ca = add(pa, scale(bis, d));
        let cb = add(pb, scale(bis, d));
        let a1 = sub(ca, scale(m1, opts.radius));
        let b1 = sub(cb, scale(m1, opts.radius));
        let a2 = sub(ca, scale(m2, opts.radius));
        let b2 = sub(cb, scale(m2, opts.radius));

        cuts.entry((e.f1, e.a)).or_default().push(a1);
        cuts.entry((e.f1, e.b)).or_default().push(b1);
        cuts.entry((e.f2, e.a)).or_default().push(a2);
        cuts.entry((e.f2, e.b)).or_default().push(b2);

        fillets.push(Fillet {
            a: e.a,
            b: e.b,
            f1: e.f1,
            f2: e.f2,
            m1,
            m2,
            ca,
            cb,
            a1,
            b1,
            a2,
            b2,
        });
    }
    if fillets.is_empty() {
        return Ok(solid.clone());
    }

    // ── 2. 组装 ──
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

    // 2a) 原面：顶点被圆角的，换成切点
    for (fi, f) in solid.faces.iter().enumerate() {
        let n = f.verts.len();
        let mut ring: Vec<u32> = Vec::with_capacity(n);
        for i in 0..n {
            let cur = f.verts[i];
            match cuts.get(&(fi, cur)) {
                Some(ps) if !ps.is_empty() => {
                    // 一个顶点可能有多条圆角边交汇：取第一个切点，保证拓扑连通
                    for p in ps {
                        ring.push(put!(*p));
                    }
                }
                _ => ring.push(put!(solid.verts[cur as usize])),
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

    // 2b) 每条边：segments 段平面条逼近圆弧
    for ft in &fillets {
        let seg = opts.segments.max(2);
        let axis = match normalize(sub(ft.cb, ft.ca)) {
            Some(x) => x,
            None => continue,
        };
        // 圆弧从 m1 转到 m2，总角 = π - acos(m1·m2)
        // 圆弧从切点1 转向切点2。
        //
        // 关键是**方向从圆心指向切点**，也就是 -m1 → -m2，
        // 而不是 m1 → m2（那是反的，弧会被甩到实体外面，实测 z 跑到 0.38）。
        let from_dir = scale(ft.m1, -1.0);
        let to_dir = scale(ft.m2, -1.0);
        let total = dot(from_dir, to_dir).clamp(-1.0, 1.0).acos();
        // 绕边方向的转向：用叉积符号判断正负
        let sign = if dot(cross(from_dir, to_dir), axis) >= 0.0 {
            1.0
        } else {
            -1.0
        };
        let mut prev_a = ft.a1;
        let mut prev_b = ft.b1;
        for s in 1..=seg {
            let t = s as f64 / seg as f64;
            let ang = total * t * sign;
            let dir = rotate(from_dir, to_dir, axis, ang);
            let cur_a = add(ft.ca, scale(dir, opts.radius));
            let cur_b = add(ft.cb, scale(dir, opts.radius));
            let quad = vec![
                put!(prev_a),
                put!(prev_b),
                put!(cur_b),
                put!(cur_a),
            ];
            let quad = dedup(quad);
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
            prev_a = cur_a;
            prev_b = cur_b;
        }
    }

    // ── 3. 角部补面 ──
    //
    // 一个顶点周围有 3 条圆角边 → 产生 6 个切点（每条边在两侧面各一个），
    // 它们围成一圈，但没有任何面把它们连起来 → 壳在角部漏气。
    //
    // 处理：每个顶点收集自己的所有切点，按绕顶点法线的角度排好，补一个多边形。
    // 真实圆角这里是个**球面三角**（三块圆角面交汇），
    // 先用平面多边形逼近——体量误差在 R 量级，比漏水好得多。
    let mut vertex_cuts: HashMap<u32, Vec<Point3>> = HashMap::new();
    for ((fi, v), ps) in &cuts {
        let _ = fi;
        for p in ps {
            vertex_cuts.entry(*v).or_default().push(*p);
        }
    }
    for (v, pts) in vertex_cuts {
        // 去重
        let mut uniq: Vec<Point3> = Vec::new();
        for p in &pts {
            if !uniq.iter().any(|q| dist2(*q, *p) < 1e-18) {
                uniq.push(*p);
            }
        }
        if uniq.len() < 3 {
            continue;
        }
        // 绕顶点平均法线排序
        let pv = solid.verts[v as usize];
        let mut navg = [0.0; 3];
        for e in table.iter() {
            if e.a == v || e.b == v {
                for fi in [e.f1, e.f2] {
                    let nn = face_normal(solid, fi);
                    for k in 0..3 {
                        navg[k] += nn[k];
                    }
                }
            }
        }
        let Some(nrm) = normalize(navg) else { continue };
        let Some((u, w)) = crate::geom::orthonormal_basis(nrm) else {
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
        let ring: Vec<u32> = keyed.into_iter().map(|(_, p)| put!(p)).collect();
        let ring = dedup(ring);
        if ring.len() >= 3 {
            let n = plane_normal(&out, &ring);
            out.faces.push(Face {
                surface: Surface::Plane {
                    origin: out.verts[ring[0] as usize],
                    normal: n,
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

fn dist2(a: Point3, b: Point3) -> f64 {
    let d = sub(a, b);
    dot(d, d)
}

/// 把方向 m1 绕 axis 转到 m2 方向上的 ang 弧度。
fn rotate(m1: Vector3, m2: Vector3, axis: Vector3, ang: f64) -> Vector3 {
    // Rodrigues：v' = v cosθ + (k×v) sinθ + k(k·v)(1-cosθ)
    let k = normalize(axis).unwrap_or([0.0, 0.0, 1.0]);
    let _ = m2;
    let c = ang.cos();
    let s = ang.sin();
    let kv = cross(k, m1);
    let kd = dot(k, m1);
    let r = [
        m1[0] * c + kv[0] * s + k[0] * kd * (1.0 - c),
        m1[1] * c + kv[1] * s + k[1] * kd * (1.0 - c),
        m1[2] * c + kv[2] * s + k[2] * kd * (1.0 - c),
    ];
    normalize(r).unwrap_or(m1)
}

fn dedup(ring: Vec<u32>) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::with_capacity(ring.len());
    for v in ring {
        if out.last() != Some(&v) {
            out.push(v);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
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
