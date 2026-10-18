//! 倒角（chamfer）：把锐边切成平面斜面。
//!
//! # 与 fillet 的区别（写死在这里，避免以后混用）
//!
//! * **chamfer（本模块）**：一条棱切一刀，得到一个平面。工程图纸上标 `C0.5` 或 `0.5×45°`。
//! * **fillet**：一条棱换成一段圆弧面。图纸上标 `R0.5`。需要圆柱面 + 圆弧边界，是另一档工作量。
//!
//! 两者都能叫「倒角」，但加工方式和验收标准完全不同——所以能力和方法都分开。
//!
//! # 算法（三步）
//!
//! 1. **边表**：把隐式边（顶点环的相邻对）落成显式记录，每条无向边找到它的两个面。
//! 2. **面内缩**：面上每条锐边向内平移 `width`，相邻平移线求交得到该顶点的新位置；
//!    非锐边平移量取 0——**这正是「只倒一部分边」不撕裂的原因**。
//! 3. **缝合**：原面用内缩后的环，每条锐边补一个斜面，每个多锐边顶点补一个角面。
//!
//! # 为什么不能简单「把所有顶点往里挪」
//!
//! 顶点被多个面共享。如果按面各自挪，同一个顶点在不同面上会挪到不同位置，实体就裂了。
//! 所以内缩必须在**每个面内部**独立计算，再由斜面和角面把它们重新缝起来。

use std::collections::HashMap;

use archcore_kernel::{KernelError, Point3, Vector3};

use crate::geom::{add, cross, dot, normalize, scale, sub, Surface};
use crate::solid::{Face, Solid};

/// 顶点合并容差。**不要放大**：倒角产生的斜面很窄，容差大了会把两侧的面吞掉。
const MERGE_TOL: f64 = 1e-9;

/// 倒角参数。
#[derive(Clone, Copy, Debug)]
pub struct ChamferOptions {
    /// 倒角宽度：从原棱起算，向两侧面各切进多少（米）。
    pub width: f64,
    /// 只倒两面夹角大于此值的边（度）。
    ///
    /// 默认 30°：小于这个角度视为「同一曲面被离散出来的折线」，
    /// 给它们倒角会把圆柱面切成锯齿。
    pub angle_threshold_deg: f64,
}

impl Default for ChamferOptions {
    fn default() -> Self {
        Self {
            width: 0.001,
            angle_threshold_deg: 30.0,
        }
    }
}

/// 一条无向边的完整信息。
pub struct EdgeRec {
    pub a: u32,
    pub b: u32,
    /// 两个相邻面。水密实体必有恰好两个。
    pub f1: usize,
    pub f2: usize,
    /// 是否达到倒角条件。
    pub sharp: bool,
    /// 两面法线夹角（度）。给边查询用；倒角自身的判据是 [`Self::sharp`]。
    pub angle_deg: f64,
}

/// 对实体做平面倒角，返回新实体。
///
/// `edges` 为空表示「所有锐边」；否则只倒指定的边（按 [`Solid::edge_table`] 的顺序编号）。
pub fn chamfer_solid(
    solid: &Solid,
    edges: &[u64],
    opts: &ChamferOptions,
) -> Result<Solid, KernelError> {
    if opts.width <= 0.0 || !opts.width.is_finite() {
        return Err(KernelError::Degenerate {
            at: [0.0, 0.0, 0.0],
            reason: format!("倒角宽度必须为正有限值，收到 {}", opts.width),
            suggestion: "传入大于 0 的宽度，例如 0.001（1mm）".to_string(),
        });
    }
    if solid.faces.is_empty() {
        return Err(KernelError::TopologyBroken {
            at: String::new(),
            reason: "实体没有任何面，无法倒角".to_string(),
            suggestion: "先创建实体再倒角".to_string(),
        });
    }

    let table = edge_table(solid)?;
    if table.is_empty() {
        return Err(KernelError::TopologyBroken {
            at: String::new(),
            reason: "实体的边表为空".to_string(),
            suggestion: "检查实体是否由 push_face 正确构造".to_string(),
        });
    }

    // 选定要倒的边
    let selected: Vec<bool> = if edges.is_empty() {
        table.iter().map(|e| e.sharp).collect()
    } else {
        let mut want = vec![false; table.len()];
        for &id in edges {
            let idx = id as usize;
            if idx >= table.len() {
                return Err(KernelError::Degenerate {
                    at: [0.0, 0.0, 0.0],
                    reason: format!("边号 {id} 超出范围（共 {} 条边）", table.len()),
                    suggestion: format!("边号范围是 0..{}", table.len()),
                });
            }
            want[idx] = table[idx].sharp;
        }
        want
    };

    if !selected.iter().any(|s| *s) {
        // 没有符合条件的边：原样返回（幂等，不报错——调用方可能对平滑实体无脑调用）
        return Ok(solid.clone());
    }

    // 每条边一个内缩量：选中的边切 width，其余不动
    let mut edge_width: HashMap<(u32, u32), f64> = HashMap::new();
    for (i, e) in table.iter().enumerate() {
        let k = key(e.a, e.b);
        edge_width.insert(k, if selected[i] { opts.width } else { 0.0 });
        // 反向也登记，方便按有向查询
        edge_width.insert((e.b, e.a), if selected[i] { opts.width } else { 0.0 });
    }

    let mut out = Solid::default();
    let mut weld: HashMap<(i64, i64, i64), u32> = HashMap::new();
    // (面索引, 原顶点号) → 内缩后的新顶点号。
    // **必须记账，不能靠几何反查**：圆柱上一个点距 0.5mm、倒角宽度也是 0.5mm 时，
    // 「最近点」会把相邻顶点认成目标顶点，缝合直接错位。
    let mut inset_map: HashMap<(usize, u32), u32> = HashMap::new();

    // ---- 第一步：每个面算出内缩后的环 ----
    let mut inset_rings: Vec<Vec<u32>> = Vec::with_capacity(solid.faces.len());
    for (fi, f) in solid.faces.iter().enumerate() {
        let n = f.verts.len();
        if n < 3 {
            return Err(KernelError::TopologyBroken {
                at: String::new(),
                reason: format!("面 {fi} 只有 {n} 个顶点"),
                suggestion: "检查实体构造过程".to_string(),
            });
        }
        let face_n = face_normal(solid, f, fi);
        let mut ring: Vec<u32> = Vec::with_capacity(n);
        for i in 0..n {
            let prev = f.verts[(i + n - 1) % n];
            let cur = f.verts[i];
            let next = f.verts[(i + 1) % n];
            let d_prev = *edge_width.get(&(prev, cur)).unwrap_or(&0.0);
            let d_cur = *edge_width.get(&(cur, next)).unwrap_or(&0.0);
            let p = inset_at_vertex(
                solid.verts[prev as usize],
                solid.verts[cur as usize],
                solid.verts[next as usize],
                face_n,
                d_prev,
                d_cur,
            )?;
            let idx = push_weld(&mut out, &mut weld, p);
            inset_map.insert((fi, cur), idx);
            ring.push(idx);
        }
        // 相邻重复点会形成退化边（零长度），去掉
        let ring = dedup_ring(ring);
        if ring.len() < 3 {
            return Err(KernelError::Degenerate {
                at: [0.0, 0.0, 0.0],
                reason: format!(
                    "面 {fi} 内缩后只剩 {} 个顶点（倒角宽度 {} 相对面尺寸过大）",
                    ring.len(),
                    opts.width
                ),
                suggestion: "减小倒角宽度，或不要倒这个面上的全部边".to_string(),
            });
        }
        // 宽度大于面的“内切半径”时，平移线交点会跨过中心、环直接翻面。
        // 注意：单看有符号面积方向拦不住——四边形翻过去再绕回来，符号不变。
        // 可靠的判据是 **内缩只会让面积变小**，变大就说明越过了中心。
        let area_before = signed_area(&solid.verts, &f.verts, face_n).abs();
        let area_after = signed_area(&out.verts, &ring, face_n).abs();
        if area_after <= 1e-15 || area_after >= area_before {
            return Err(KernelError::Degenerate {
                at: [0.0, 0.0, 0.0],
                reason: format!(
                    "面 {fi} 内缩后面积异常（{:.3e} → {:.3e}，宽度 {}）",
                    area_before, area_after, opts.width
                ),
                suggestion: "把倒角宽度减到面最小边长的 1/4 以下".to_string(),
            });
        }
        inset_rings.push(ring);
    }

    // ---- 第二步：原面换用内缩环 ----
    for (fi, f) in solid.faces.iter().enumerate() {
        out.faces.push(Face {
            surface: f.surface,
            verts: inset_rings[fi].clone(),
        });
    }

    // ---- 第三步：每条选中的边补一个斜面 ----
    for (i, e) in table.iter().enumerate() {
        if !selected[i] {
            continue;
        }
        let (qa1, qb1) = (
            *inset_map.get(&(e.f1, e.a)).ok_or_else(|| KernelError::TopologyBroken {
                at: format!("face {}", e.f1),
                reason: format!("面 {} 缺少顶点 {} 的内缩记录", e.f1, e.a),
                suggestion: "这是内核缺陷，请带上实体参数报 issue".to_string(),
            })?,
            *inset_map.get(&(e.f1, e.b)).ok_or_else(|| KernelError::TopologyBroken {
                at: format!("face {}", e.f1),
                reason: format!("面 {} 缺少顶点 {} 的内缩记录", e.f1, e.b),
                suggestion: "这是内核缺陷，请带上实体参数报 issue".to_string(),
            })?,
        );
        let (qa2, qb2) = (
            *inset_map.get(&(e.f2, e.a)).ok_or_else(|| KernelError::TopologyBroken {
                at: format!("face {}", e.f2),
                reason: format!("面 {} 缺少顶点 {} 的内缩记录", e.f2, e.a),
                suggestion: "这是内核缺陷，请带上实体参数报 issue".to_string(),
            })?,
            *inset_map.get(&(e.f2, e.b)).ok_or_else(|| KernelError::TopologyBroken {
                at: format!("face {}", e.f2),
                reason: format!("面 {} 缺少顶点 {} 的内缩记录", e.f2, e.b),
                suggestion: "这是内核缺陷，请带上实体参数报 issue".to_string(),
            })?,
        );
        let quad = vec![qa1, qb1, qb2, qa2];
        let n1 = face_normal(solid, &solid.faces[e.f1], e.f1);
        let n2 = face_normal(solid, &solid.faces[e.f2], e.f2);
        let refn = normalize(add(n1, n2)).unwrap_or([0.0, 0.0, 1.0]);
        let quad = orient_outward(&out, quad, refn);
        // 斜面是平面：用三点定法线
        let nrm = plane_normal(&out, &quad);
        out.push_face(
            Surface::Plane {
                origin: out.verts[quad[0] as usize],
                normal: nrm,
            },
            quad,
        );
    }

    // ---- 第四步：多锐边顶点补角面 ----
    let corners = corner_faces(solid, &table, &selected, &inset_map, &mut out)?;
    for (ring, nrm, origin) in corners {
        out.push_face(
            Surface::Plane {
                origin,
                normal: nrm,
            },
            ring,
        );
    }

    out.analytic_volume = None;
    out.analytic_area = None;
    Ok(out)
}

type Key = (u32, u32);

fn key(a: u32, b: u32) -> Key {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// 把隐式边落成显式表。
///
/// 顺带做水密检查：每条边必须恰好两个面。两个后端（网格/B-rep）都靠这一条
/// 判断「这个实体还能不能做拓扑运算」。
pub fn edge_table(solid: &Solid) -> Result<Vec<EdgeRec>, KernelError> {
    let mut map: HashMap<Key, Vec<usize>> = HashMap::new();
    for (fi, f) in solid.faces.iter().enumerate() {
        let n = f.verts.len();
        for i in 0..n {
            let a = f.verts[i];
            let b = f.verts[(i + 1) % n];
            if a == b {
                return Err(KernelError::TopologyBroken {
                    at: String::new(),
                    reason: format!("面 {fi} 含零长度边（顶点 {a} 重复）"),
                    suggestion: "检查实体构造".to_string(),
                });
            }
            map.entry(key(a, b)).or_default().push(fi);
        }
    }

    let mut out = Vec::with_capacity(map.len());
    for ((a, b), faces) in map {
        if faces.len() != 2 {
            return Err(KernelError::TopologyBroken {
                at: String::new(),
                reason: format!(
                    "边 ({a},{b}) 被 {} 个面共享（水密实体应为 2）",
                    faces.len()
                ),
                suggestion: "先修复实体水密性（is_watertight 应返回 true）".to_string(),
            });
        }
        let (f1, f2) = (faces[0], faces[1]);
        let n1 = face_normal(solid, &solid.faces[f1], f1);
        let n2 = face_normal(solid, &solid.faces[f2], f2);
        let ang = dot(n1, n2).clamp(-1.0, 1.0).acos().to_degrees();
        out.push(EdgeRec {
            a,
            b,
            f1,
            f2,
            sharp: ang > 30.0,
            angle_deg: ang,
        });
    }
    // 确定性排序：按顶点号，保证同一实体两次得到的边号一致
    out.sort_by_key(|e| (e.a, e.b));
    Ok(out)
}

/// 面法线：优先解析曲面，退化时用多边形叉积。
fn face_normal(solid: &Solid, f: &crate::solid::Face, fi: usize) -> Vector3 {
    let n = f.verts.len();
    let mut c = [0.0; 3];
    for v in &f.verts {
        for (ci, pi) in c.iter_mut().zip(solid.verts[*v as usize].iter()) {
            *ci += *pi;
        }
    }
    for ci in c.iter_mut() {
        *ci /= n as f64;
    }
    if let Some(nv) = f.surface.normal_at(c) {
        return nv;
    }
    // 叉积兜底
    let a = solid.verts[f.verts[0] as usize];
    let b = solid.verts[f.verts[1] as usize];
    let cc = solid.verts[f.verts[2] as usize];
    normalize(cross(sub(b, a), sub(cc, a))).unwrap_or_else(|| {
        // 完全退化的面：给个默认值，让后续能报出具体问题而不是 panic
        let _ = fi;
        [0.0, 0.0, 1.0]
    })
}

/// 顶点内缩：相邻两条边的平移线求交。
///
/// 返回该顶点在面内平移后的位置。两条边都平移 0 时原样返回。
fn inset_at_vertex(
    prev: Point3,
    cur: Point3,
    next: Point3,
    n: Vector3,
    d_prev: f64,
    d_cur: f64,
) -> Result<Point3, KernelError> {
    if d_prev == 0.0 && d_cur == 0.0 {
        return Ok(cur);
    }
    let e1 = normalize(sub(cur, prev)).ok_or_else(|| KernelError::Degenerate {
        at: cur,
        reason: "前一条边长度为零".to_string(),
        suggestion: "检查实体是否有重复顶点".to_string(),
    })?;
    let e2 = normalize(sub(next, cur)).ok_or_else(|| KernelError::Degenerate {
        at: cur,
        reason: "后一条边长度为零".to_string(),
        suggestion: "检查实体是否有重复顶点".to_string(),
    })?;

    // 面内法线：cross(n, e) 指向环的内侧（环为从外看的逆时针）
    let m1 = normalize(cross(n, e1)).ok_or_else(|| KernelError::Degenerate {
        at: cur,
        reason: "边方向与面法线平行，无法确定内缩方向".to_string(),
        suggestion: "检查面法线是否正确".to_string(),
    })?;
    let m2 = normalize(cross(n, e2)).ok_or_else(|| KernelError::Degenerate {
        at: cur,
        reason: "边方向与面法线平行，无法确定内缩方向".to_string(),
        suggestion: "检查面法线是否正确".to_string(),
    })?;

    // 面内二维叉积：dot(cross(a,b), n)
    let cross2 = |a: Vector3, b: Vector3| dot(cross(a, b), n);
    let denom = cross2(e1, e2);
    if denom.abs() < 1e-12 {
        return Err(KernelError::Degenerate {
            at: cur,
            reason: "顶点处两条边共线（180° 折返），内缩方向无解".to_string(),
            suggestion: "把这条边从倒角选择中排除，或先修复实体".to_string(),
        });
    }
    // 解：d_prev*m1 + t*e1 = d_cur*m2 + s*e2
    //
    // 与 e2 叉积消掉 s：
    //   d_prev*(m1×e2) + t*(e1×e2) = d_cur*(m2×e2) + 0
    // 注意 **右侧不为零**——m2⊥e2 不等于 m2×e2=0；
    // 而 cross2(m2, e2) = (cross(n,e2))×e2 = −n，投影到 n 上恒为 −1（单位向量下）。
    // 最早就是因为漏了右边这一项，顶点只沿一个方向内缩，底面中心凹成了“尖角”。
    let t = (d_cur * cross2(m2, e2) - d_prev * cross2(m1, e2)) / denom;
    Ok(add(add(cur, scale(m1, d_prev)), scale(e1, t)))
}

/// 环的有符号面积（Newell 法），沿给定法线取符号。
fn signed_area(verts: &[Point3], ring: &[u32], n: Vector3) -> f64 {
    let m = ring.len();
    let mut acc = [0.0; 3];
    for i in 0..m {
        let a = verts[ring[i] as usize];
        let b = verts[ring[(i + 1) % m] as usize];
        acc[0] += (a[1] - b[1]) * (a[2] + b[2]);
        acc[1] += (a[2] - b[2]) * (a[0] + b[0]);
        acc[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    dot(acc, n) * 0.5
}

/// 让多边形朝向正确：法线应与参考方向同向。
///
/// 参考方向由调用方给：斜面用两侧面外法线之和（凸/凹边都成立），
/// 角面用顶点周围面法线之和。**不要**用「多边形质心相对原棱」——
/// 倒角面本来就在棱的内侧，那个判据会把每个面都判反。
fn orient_outward(out: &Solid, mut ring: Vec<u32>, ref_normal: Vector3) -> Vec<u32> {
    let nrm = plane_normal(out, &ring);
    if dot(nrm, ref_normal) < 0.0 {
        ring.reverse();
    }
    ring
}

fn plane_normal(out: &Solid, ring: &[u32]) -> Vector3 {
    let a = out.verts[ring[0] as usize];
    let b = out.verts[ring[1] as usize];
    let c = out.verts[ring[2] as usize];
    normalize(cross(sub(b, a), sub(c, a))).unwrap_or([0.0, 0.0, 1.0])
}

/// 去重：连续重复的顶点（零长度边）合并。
fn dedup_ring(ring: Vec<u32>) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::with_capacity(ring.len());
    for &v in ring.iter() {
        if out.last() != Some(&v) {
            out.push(v);
        }
    }
    // 首尾也要去重
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

/// 带合并的顶点压入。
fn push_weld(out: &mut Solid, weld: &mut HashMap<(i64, i64, i64), u32>, p: Point3) -> u32 {
    let k = (
        (p[0] / MERGE_TOL).round() as i64,
        (p[1] / MERGE_TOL).round() as i64,
        (p[2] / MERGE_TOL).round() as i64,
    );
    if let Some(&idx) = weld.get(&k) {
        return idx;
    }
    let idx = out.push_vert(p);
    weld.insert(k, idx);
    idx
}

/// 顶点补角面：一个顶点若连着 >= 3 条倒角边，那里会留下一个缺口。
fn corner_faces(
    solid: &Solid,
    table: &[EdgeRec],
    selected: &[bool],
    inset_map: &HashMap<(usize, u32), u32>,
    out: &mut Solid,
) -> Result<Vec<(Vec<u32>, Vector3, Point3)>, KernelError> {
    // 顶点 -> 参与的倒角边数 + 相关面
    let mut per_vert: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, e) in table.iter().enumerate() {
        if !selected[i] {
            continue;
        }
        per_vert.entry(e.a).or_default().push(i);
        per_vert.entry(e.b).or_default().push(i);
    }

    let mut result = Vec::new();
    for (v, edges) in per_vert {
        if edges.len() < 3 {
            // 两条边共享的顶点，缺口已被两个斜面各自的四个点覆盖，无需补
            continue;
        }
        // 收集该顶点周围所有面
        let mut faces: Vec<usize> = Vec::new();
        for &ei in &edges {
            if !faces.contains(&table[ei].f1) {
                faces.push(table[ei].f1);
            }
            if !faces.contains(&table[ei].f2) {
                faces.push(table[ei].f2);
            }
        }
        if faces.len() < 3 {
            continue;
        }
        // 每个面在 v 处的内缩点
        let pv = solid.verts[v as usize];
        let mut cands: Vec<u32> = Vec::new();
        for &fi in &faces {
            if let Some(&idx) = inset_map.get(&(fi, v)) {
                cands.push(idx);
            }
        }
        cands.sort_unstable();
        cands.dedup();
        if cands.len() < 3 {
            continue;
        }
        // 按绕顶点平均法线的角度排序
        let mut navg = [0.0; 3];
        for &fi in &faces {
            let nf = face_normal(solid, &solid.faces[fi], fi);
            for (ai, ni) in navg.iter_mut().zip(nf.iter()) {
                *ai += *ni;
            }
        }
        let navg = normalize(navg).unwrap_or([0.0, 0.0, 1.0]);
        let (u, w) = crate::geom::orthonormal_basis(navg).ok_or_else(|| {
            KernelError::Degenerate {
                at: pv,
                reason: "顶点处平均法线退化".to_string(),
                suggestion: "检查该顶点周围的面".to_string(),
            }
        })?;
        let mut keyed: Vec<(f64, u32)> = cands
            .into_iter()
            .map(|idx| {
                let d = sub(out.verts[idx as usize], pv);
                let ang = dot(d, w).atan2(dot(d, u));
                (ang, idx)
            })
            .collect();
        keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let ring: Vec<u32> = keyed.into_iter().map(|(_, i)| i).collect();
        let ring = orient_outward(out, ring, navg);
        let nrm = plane_normal(out, &ring);
        let origin = out.verts[ring[0] as usize];
        result.push((ring, nrm, origin));
    }
    Ok(result)
}
