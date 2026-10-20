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

use crate::edges::edge_table;
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

        // 端点：**面与球面的切点** c + R·n_f
        //
        // 原来走的是「沿棱方向推到球面」，得到的是**棱上的点**，不是切点。
        // 两条棱沿不同方向内缩，端点间距 = 2R·sin(θ/2)（弦长）：
        //   盒子 90°      → 1.4142R   实测 1.4142R
        //   圆柱 16 段    157.5°    → 1.9616R   实测 1.9616R
        //   圆柱 128 段   177.19°   → 1.9994R   实测 1.9994R
        // N→∞ 时两棱趋于反向，弦→直径 2R，所以间隙「不随 N 缩小」是必然。
        //
        // 而切点 c + R·n_f 同时满足两条恒等式：
        //   |p − c| = R              → 在球面上
        //   (p − c)·e = R·(n_f·e) = 0 → 在柱面上（n_f ⊥ e）
        // 也就是说**切点本来就是大圆上的点**，同一个面上两条棱给出的端点
        // 必须是同一个点——「顶点唯一」不需要任何拼接手段来维护。
        let a1 = corner_point(spheres.get(&e.a), n1, r).unwrap_or(rp1);
        let b1 = corner_point(spheres.get(&e.b), n1, r).unwrap_or(add(pb, scale(dir1, r)));
        let a2 = corner_point(spheres.get(&e.a), n2, r).unwrap_or(rp2);
        let b2 = corner_point(spheres.get(&e.b), n2, r).unwrap_or(add(pb, scale(dir2, r)));

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

    // ── 索引共享（关键重构）──
    //
    // 端点的坐标只算一次、索引只分配一次。
    // 之后原面、圆角面、角部面**全部引用同一批索引**，不靠坐标合并。
    //
    // 之前是每个面各自 put!(坐标)，靠 weld 按量化坐标合并——
    // 浮点一抖就合不上，实测 234 条孤边 / 333 条总边。
    let mut corner_idx: HashMap<(usize, u32), u32> = HashMap::new();
    for (i, e) in table.iter().enumerate() {
        if !selected[i] {
            continue;
        }
        let Some(rl) = rails.get(&(e.a, e.b)) else {
            continue;
        };
        corner_idx.insert((e.f1, e.a), put!(rl.f1_start));
        corner_idx.insert((e.f1, e.b), put!(rl.f1_end));
        corner_idx.insert((e.f2, e.a), put!(rl.f2_start));
        corner_idx.insert((e.f2, e.b), put!(rl.f2_end));
    }

    // ── 端圆点位预生成（角部面与圆角面共享）──
    //
    // 圆角面是按**端圆**分段的：第 s 段的点绕 ca（或 cb）转 ang*t。
    // 因此它的短边就是端圆上的一小段弧。
    //
    // 角部面如果只拿 3 个「轨线端点」连三角，就只共享到端点那条，
    // 剩下 seg-1 段的弧全部悬空——实测 240 半边里 120 条没配对。
    //
    // 这里把端圆上每个中间点也建成索引（key = 棱 + 端的哪一侧 + 第几步），
    // 圆角面与角部面都从这里取，同一个 s 就是同一个顶点。
    //
    // `arcs[(a, b, at_b)]` 存 seg+1 个索引：第 0 步是轨线端点 f1，
    // 第 seg 步是 f2（另一端），中间是弧点。
    let seg_n = opts.segments.max(2);
    let mut arcs: HashMap<(u32, u32, bool), Vec<u32>> = HashMap::new();
    for (i, e) in table.iter().enumerate() {
        if !selected[i] {
            continue;
        }
        let Some(rl) = rails.get(&(e.a, e.b)) else {
            continue;
        };
        let axis = normalize(sub(rl.cb, rl.ca)).unwrap_or([0.0, 0.0, 1.0]);
        // ── 统一的 u/w 基 ──
        //
        // 弧的参数化**不能用「各自归一化到自己的圆心」再插角度**：
        // 两端圆心 c_a、c_b 不同，基准方向就差一点，转出来的两条弧不对应，
        // 四边形的母线被扭歪（twisted），柱面片就不精确了。
        //
        // 改用一套垂直下轴的公用基底 (u, w)，两端绕各自球心转**同一组角度**，
        // 母线就严格平行于轴。
        let u0 = {
            let raw = sub(rl.f1_start, rl.ca);
            let along = dot(raw, axis);
            normalize(sub(raw, scale(axis, along))).unwrap_or(rl.m1)
        };
        let w0 = normalize(cross(axis, u0)).unwrap_or(rl.m2);
        let ca_center = spheres.get(&e.a).map(|(c, _)| *c);
        let cb_center = spheres.get(&e.b).map(|(c, _)| *c);
        // 从 u 到 f2 所在位置的角度（带符号）
        let to0 = {
            let raw = sub(rl.f2_start, rl.ca);
            let along = dot(raw, axis);
            normalize(sub(raw, scale(axis, along))).unwrap_or(rl.m2)
        };
        let total = dot(u0, to0).clamp(-1.0, 1.0).acos();
        let sign_ref = if dot(cross(u0, to0), axis) >= 0.0 {
            1.0
        } else {
            -1.0
        };
        for at_b in [false, true] {
            let sc = if at_b { cb_center } else { ca_center };
            let c0 = if at_b { rl.cb } else { rl.ca };
            // 弧心：球心优先；顶点没有球时（单边圆角）退回 ca/cb
            let center = sc.unwrap_or(c0);
            let _ = center;
            let mut chain: Vec<u32> = Vec::with_capacity(seg_n + 1);
            // 第 0 步固定用 corner_idx 的端点（保证与原面一致）
            let start_key = if at_b { (e.f1, e.b) } else { (e.f1, e.a) };
            chain.push(corner_idx[&start_key]);
            for s in 1..seg_n {
                let t = s as f64 / seg_n as f64;
                let ang = total * t * sign_ref;
                // 用公用基底 (u0, w0) 构造方向，两端严格对应
                let dir = [
                    u0[0] * ang.cos() + w0[0] * ang.sin(),
                    u0[1] * ang.cos() + w0[1] * ang.sin(),
                    u0[2] * ang.cos() + w0[2] * ang.sin(),
                ];
                chain.push(put!(add(center, scale(dir, r))));
            }
            let end_key = if at_b { (e.f2, e.b) } else { (e.f2, e.a) };
            chain.push(corner_idx[&end_key]);
            arcs.insert((e.a, e.b, at_b), chain);
        }
    }

    // 3a) 原面：边界顶点直接用 corner_idx 的索引
    for (fi, f) in solid.faces.iter().enumerate() {
        let n = f.verts.len();
        let mut ring: Vec<u32> = Vec::with_capacity(n);
        for i in 0..n {
            let cur = f.verts[i];
            match corner_idx.get(&(fi, cur)) {
                Some(&idx) => ring.push(idx),
                None => ring.push(put!(solid.verts[cur as usize])),
            }
        }
        let ring = dedup(ring);
        if ring.len() >= 3 {
            out.faces.push(Face {
                surface: f.surface,
                verts: ring,
                holes: Vec::new(),
            boundary: None,
            tolerance: None,
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
        let seg = seg_n;
        let _ = &rl;
        // 弧点直接取端圆链（与角部面共享索引）
        let Some(chain_a) = arcs.get(&(e.a, e.b, false)).cloned() else {
            continue;
        };
        let Some(chain_b) = arcs.get(&(e.a, e.b, true)).cloned() else {
            continue;
        };
        // 圆角面：两端用**共享索引**（端圆链）
        let mut prev_a = corner_idx[&(e.f1, e.a)];
        let mut prev_b = corner_idx[&(e.f1, e.b)];
        for s in 1..=seg {
            let va = chain_a[s];
            let vb = chain_b[s];
            let quad = dedup(vec![prev_a, prev_b, vb, va]);
            if quad.len() >= 3 {
                // 圆角面的朝向必须「背离轴线」（圆柱外侧）。
                // 不判的话一半面朝内，体积会被多减一遍。
                let mut quad = quad;
                let mid_axis = scale(add(rl.ca, rl.cb), 0.5);
                let centroid = {
                    let mut s = [0.0; 3];
                    for &i in &quad {
                        let p = out.verts[i as usize];
                        for k in 0..3 {
                            s[k] += p[k];
                        }
                    }
                    let m = quad.len() as f64;
                    [s[0] / m, s[1] / m, s[2] / m]
                };
                if dot(plane_normal(&out, &quad), sub(centroid, mid_axis)) < 0.0 {
                    quad.reverse();
                }
                let nrm = plane_normal(&out, &quad);
                let _ = nrm;
                // 圆角面是**圆柱面**（轴 = 两端圆心连线，半径 R），不是平面。
                //
                // 之前标成 Plane，后果有两个：
                // 1. STEP 导不出圆柱面（全退化成平面碎片）
                // 2. `normal_at` 拿平面法线，三角化方向不准，体积偏小（实测 7.53 vs 理论 7.80）
                let axis = normalize(sub(rl.cb, rl.ca)).unwrap_or([0.0, 0.0, 1.0]);
                out.faces.push(Face {
                    surface: Surface::Cylinder {
                        origin: rl.ca,
                        axis,
                        radius: r,
                    },
                    verts: quad,
                    holes: Vec::new(),
                boundary: None,
                tolerance: None,
                });
            }
            prev_a = va;
            prev_b = vb;
        }
    }

    // 3c) 角部：球面片
    //
    // 具体缝合逻辑已抽到 [`super::corner::corner_blend`]——因为倒角也有
    // 同一件事（顶点处若干棱相交要补面），抽出来才能按选中棱数分类。
    super::corner::corner_blend(&mut out, &table, &selected, &spheres, &arcs, r);

    out.analytic_volume = None;
    out.analytic_area = None;
    // 半边拓扑自检：把断裂点直接暴露出来
    out.build_topology();
    if std::env::var("ARCHCORE_DEBUG_FILLET").is_ok() {
        let problems = out.check_topology();
        eprintln!(
            "[fillet] 半边 {} 边 {} 边界半边 {} 问题 {} 条",
            out.half_edges.len(),
            out.edges.len(),
            out.boundary_half_edges(),
            problems.len()
        );
        for p in problems.iter().take(6) {
            eprintln!("  {p}");
        }
    }
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
    let b = [
        r + dot(v, m[0]),
        r + dot(v, m[1]),
        r + dot(v, m[2]),
    ];

    // Cramer 法则：**矩阵的行为方程系数**（a1/a2/a3），
    // 所以求 x 时替换的是**第 1 列**，而不是把 a2/a3 当行向量做叉乘。
    //
    // 早先写成 dot(b, cross(a2, a3))，把行当成了列——
    // 盒子角解出了 (-1.8, 0.2, -0.2)（正确答案是 1.8, 0.2, 0.2），符号全反。
    let col2 = [a1[1], a2[1], a3[1]];
    let col3 = [a1[2], a2[2], a3[2]];
    let col1 = [a1[0], a2[0], a3[0]];
    let x = dot(b, cross(col2, col3)) / det;
    let y = dot(b, cross(col3, col1)) / det;
    let z = dot(b, cross(col1, col2)) / det;
    Some([x, y, z])
}

/// 面与球面的**切点**：`c + R·n_f`。
///
/// 这是角部唯一的真值。同一个面上两条棱都必须得到它——它们共用同一个
/// 球心 `c` 和同一个面法线 `n_f`，结果按构造成立，不靠容差去合。
///
/// 两条恒等式（这就是为什么它同时也是大圆上的点）：
/// ```text
/// |p − c| = |R·n_f| = R                   → 在球面上
/// (p − c)·e = R·(n_f·e) = 0               → 在柱面上（n_f ⊥ e）
/// ```
///
/// 顶点没有球（单边圆角）时返回 `None`，调用方退回旧做法。
fn corner_point(sphere: Option<&(Point3, Vec<usize>)>, n_f: Vector3, r: f64) -> Option<Point3> {
    let (c, _) = sphere?;
    Some(add(*c, scale(n_f, r)))
}

pub(super) fn centroid_of(out: &Solid, ring: &[u32]) -> Point3 {
    let mut s = [0.0; 3];
    for &i in ring {
        let p = out.verts[i as usize];
        for k in 0..3 {
            s[k] += p[k];
        }
    }
    let m = ring.len() as f64;
    [s[0] / m, s[1] / m, s[2] / m]
}

/// 把两点的中点投影到球面上（预留：球面细分会用）。
#[allow(dead_code)]
fn project_to_sphere(a: Point3, b: Point3, center: Point3, r: f64) -> Point3 {
    let mid = scale(add(a, b), 0.5);
    normalize(sub(mid, center))
        .map(|d| add(center, scale(d, r)))
        .unwrap_or(mid)
}

pub(super) fn dedup(ring: Vec<u32>) -> Vec<u32> {
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
