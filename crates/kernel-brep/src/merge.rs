//! 共面面片合并：把布尔切碎的碎片重新合成整面。
//!
//! # 为什么需要它
//!
//! 布尔运算的 `split_all` 会把每个面按所有相交平面切开。一块 20×20mm 的板开 Φ10 孔，
//! 理论只有 ~70 个面，实际输出 3148 个——每片最大 0.077×0.077mm。
//! 后果不只是「不好看」：
//!
//! - 倒角没法做（任何宽度都大过碎片）
//! - STEP 文件 944KB，加工厂读进去是一堆碎面
//! - 三角化后顶点数暴涨，渲染变慢
//!
//! # 算法：逐对面拼接（不是全局边消除）
//!
//! 一开始写的是「收集全部有向边 → 删内边 → 串环」，结果在 T 型节点上串错——
//! 开孔板顶面的内圆被串进了外环（132 点、面积偏大 60%）。全局边消除要求
//! 每个顶点出度恰好为 1，而布尔切分经常造出度 2 的顶点。
//!
//! 现在改成**每次只拼一对共享边的面**：
//!
//! 1. 两个面的环在共享边处各自断开；
//! 2. 把两段路径接成一个环；
//! 3. 检查新环不自交、面积等于两者之和（±容差），通过才接受；
//! 4. 反复直到没有可拼的。
//!
//! 慢一些，但每一步都能验证，出错也不会污染整块。
//!
//! # 不变量
//!
//! 合并**不改变几何**：顶点池原样保留，只重排面的组织方式。
//! 所以体积、水密性必须与合并前一致——这是验收口径。

use std::collections::HashMap;

use archcore_kernel::{KernelError, Point3, Vector3};

use crate::geom::{dot, normalize, sub, Surface};
use crate::solid::{Face, Solid};

/// 合并结果统计，便于调用方判断效果。
#[derive(Clone, Copy, Debug, Default)]
pub struct MergeStats {
    pub faces_before: usize,
    pub faces_after: usize,
    /// 合并后带洞的面数。
    pub holed_faces: usize,
    /// 面数减少比例（0.0 ~ 1.0）。
    pub reduction: f64,
}

/// 把同一解析曲面上相邻的面片合并成整面。返回新实体与统计。
pub fn merge_coplanar(solid: &Solid) -> Result<(Solid, MergeStats), KernelError> {
    let n = solid.faces.len();
    if n == 0 {
        return Ok((solid.clone(), MergeStats::default()));
    }

    // 曲面分组键：只有同键才可能合并
    let keys: Vec<Option<SurfaceKey>> = solid
        .faces
        .iter()
        .map(|f| {
            if matches!(f.surface, Surface::Plane { .. }) {
                Some(SurfaceKey::of(&f.surface))
            } else {
                None
            }
        })
        .collect();

    // 当前工作集：面 + 它的曲面键（None 表示不参与合并）
    struct Work {
        face: Face,
        key: Option<SurfaceKey>,
        // 累积面积（用于拼接后校验）
        area: f64,
    }

    let mut work: Vec<Work> = Vec::with_capacity(n);
    for (i, f) in solid.faces.iter().enumerate() {
        let normal = face_normal(solid, i);
        work.push(Work {
            area: poly_area(solid, &f.verts, &f.holes, normal),
            face: f.clone(),
            key: keys[i].clone(),
        });
    }

    // 反复拼接：用**槽位 + 增量边表 + 候选队列**，避免每轮全量重扫。
    //
    // 上一版每轮都重建 edge_map 并遍历所有共享边配对，3148 面要跑 415 秒。
    // 这里改成：合并只更新受影响的两面边记录，新产生的配对入队。
    let mut slots: Vec<Option<Work>> = work.into_iter().map(Some).collect();

    // 边 → 占用它的槽位
    let mut edge_slots: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, w) in slots.iter().enumerate() {
        let Some(w) = w else { continue };
        if w.key.is_none() {
            continue;
        }
        for e in ring_edges(&w.face.verts) {
            edge_slots.entry(e).or_default().push(i);
        }
    }

    // 候选配对队列（边上有两个可合并面）
    let mut queue: std::collections::VecDeque<(u32, u32)> = std::collections::VecDeque::new();
    for (e, users) in &edge_slots {
        if users.len() == 2 {
            queue.push_back(*e);
        }
    }

    let mut guard = 0usize;
    let max_ops = n * 8 + 64;
    while let Some(e) = queue.pop_front() {
        guard += 1;
        if guard > max_ops {
            break;
        }
        let users = match edge_slots.get(&e) {
            Some(u) if u.len() == 2 => u.clone(),
            _ => continue,
        };
        let (i, j) = (users[0], users[1]);
        if slots[i].is_none() || slots[j].is_none() {
            continue;
        }
        if slots[i].as_ref().unwrap().key != slots[j].as_ref().unwrap().key {
            continue;
        }

        let (fi, fj) = (
            slots[i].as_ref().unwrap().face.clone(),
            slots[j].as_ref().unwrap().face.clone(),
        );
        let Some(new_face) = try_splice(solid, &fi, &fj) else {
            continue;
        };
        let normal = face_normal(solid, 0);
        let area = poly_area(solid, &new_face.verts, &new_face.holes, normal);
        let expect = slots[i].as_ref().unwrap().area + slots[j].as_ref().unwrap().area;
        if (area - expect).abs() > expect.max(1e-18) * 1e-6 + 1e-18 {
            continue;
        }

        // 从边表里摸掉 i、j 的旧边
        for old in [&fi, &fj] {
            for edge in ring_edges(&old.verts) {
                if let Some(v) = edge_slots.get_mut(&edge) {
                    v.retain(|&s| s != i && s != j);
                    if v.is_empty() {
                        edge_slots.remove(&edge);
                    }
                }
            }
        }
        // 写回新面，腾空 j
        slots[i] = Some(Work {
            face: new_face.clone(),
            key: slots[i].as_ref().unwrap().key.clone(),
            area,
        });
        slots[j] = None;

        // 新面的边入表，并把新产生的配对入队
        for edge in ring_edges(&new_face.verts) {
            let v = edge_slots.entry(edge).or_default();
            if !v.contains(&i) {
                v.push(i);
            }
            if v.len() == 2 {
                queue.push_back(edge);
            }
        }
        // 也把 j 腾空后可能变“可配对”的邻居重新入队（保守做法：把所有与 i 共边的边入队）
        for edge in ring_edges(&new_face.verts) {
            queue.push_back(edge);
        }

        // 关键补充：把**邻居的所有边**也入队。
        // 只 push 新面自己的边时，那些「本来因邻居被并掉而变回单面」的边不会被重试，
        // 合并会提前收敛（实测 509 面而非 256）。
        let mut neighbors: Vec<usize> = Vec::new();
        for edge in ring_edges(&new_face.verts) {
            if let Some(v) = edge_slots.get(&edge) {
                for &s in v {
                    if s != i && !neighbors.contains(&s) {
                        neighbors.push(s);
                    }
                }
            }
        }
        for nb in neighbors {
            if let Some(w) = &slots[nb] {
                for edge in ring_edges(&w.face.verts) {
                    queue.push_back(edge);
                }
            }
        }
    }

    let work: Vec<Work> = slots.into_iter().flatten().collect();

    let mut holed = 0usize;
    let mut out_faces = Vec::with_capacity(work.len());
    for w in &work {
        if !w.face.holes.is_empty() {
            holed += 1;
        }
        out_faces.push(w.face.clone());
    }

    let out = Solid {
        verts: solid.verts.clone(),
        faces: out_faces,
        analytic_volume: solid.analytic_volume,
        analytic_area: solid.analytic_area,
    };
    let stats = MergeStats {
        faces_before: n,
        faces_after: out.faces.len(),
        holed_faces: holed,
        reduction: if n > 0 {
            1.0 - out.faces.len() as f64 / n as f64
        } else {
            0.0
        },
    };
    Ok((out, stats))
}

/// 尝试沿共享边拼接两个面。失败返回 None（不改变任何东西）。
fn try_splice(_solid: &Solid, a: &Face, b: &Face) -> Option<Face> {
    // 找共享边：a 有向边 (x→y)，b 必须是 (y→x)
    let ra = &a.verts;
    let rb = &b.verts;
    let ka = ra.len();
    for ia in 0..ka {
        let x = ra[ia];
        let y = ra[(ia + 1) % ka];
        // b 里找 y→x
        let kb = rb.len();
        let mut ib = None;
        for j in 0..kb {
            if rb[j] == y && rb[(j + 1) % kb] == x {
                ib = Some(j);
                break;
            }
        }
        let Some(ib) = ib else { continue };

        // 拼：从 a 的 y 出发绕到 x（不含 x），再接 b 从 x 绕到 y（不含 y）
        let mut ring: Vec<u32> = Vec::with_capacity(ka + kb);
        // a: y .. a 末尾 .. a 开头 .. x 之前
        let mut k = (ia + 1) % ka;
        while ra[k] != x {
            ring.push(ra[k]);
            k = (k + 1) % ka;
        }
        // 此时 ra[k] == x
        // b: x 之后 .. b 末尾 .. b 开头 .. y 之前
        let mut m = (ib + 1) % kb;
        while rb[m] != y {
            ring.push(rb[m]);
            m = (m + 1) % kb;
        }

        if ring.len() < 3 {
            return None;
        }

        // 只在**没有洞**的情形下拼接（带洞面拼接属未支持场景，宁可不合）
        if !a.holes.is_empty() || !b.holes.is_empty() {
            return None;
        }

        // 校验：新环自身不能有重复顶点（自交的典型征兆）
        let mut seen: HashMap<u32, ()> = HashMap::new();
        for &v in &ring {
            if seen.insert(v, ()).is_some() {
                return None;
            }
        }

        return Some(Face {
            surface: a.surface,
            verts: ring,
            holes: Vec::new(),
        });
    }
    None
}

/// 曲面分组键：把浮点量化成整数。
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
struct SurfaceKey(String);

impl SurfaceKey {
    fn of(s: &Surface) -> Self {
        fn q(v: f64) -> i64 {
            (v / 1e-9).round() as i64
        }
        let body = match s {
            // 平面用「法线 + 到原点距离」做键，**不能用 origin 坐标**：
            // 布尔切分后每片碎片的 origin 是它自己的首个顶点，同一平面会被当成不同平面，
            // 合并永远不会发生（实测 3148 → 3148）。
            Surface::Plane { origin, normal } => {
                let d = normal[0] * origin[0] + normal[1] * origin[1] + normal[2] * origin[2];
                format!(
                    "P|{},{},{}|{}",
                    q(normal[0]),
                    q(normal[1]),
                    q(normal[2]),
                    q(d)
                )
            }
            Surface::Cylinder {
                origin,
                axis,
                radius,
            } => format!(
                "C|{},{},{}|{},{},{}|{}",
                q(origin[0]),
                q(origin[1]),
                q(origin[2]),
                q(axis[0]),
                q(axis[1]),
                q(axis[2]),
                q(*radius)
            ),
            Surface::Cone {
                apex,
                axis,
                half_angle,
            } => format!(
                "N|{},{},{}|{},{},{}|{}",
                q(apex[0]),
                q(apex[1]),
                q(apex[2]),
                q(axis[0]),
                q(axis[1]),
                q(axis[2]),
                q(*half_angle)
            ),
            Surface::Sphere { center, radius } => format!(
                "S|{},{},{}|{}",
                q(center[0]),
                q(center[1]),
                q(center[2]),
                q(*radius)
            ),
            Surface::Torus {
                center,
                axis,
                major,
                minor,
            } => format!(
                "T|{},{},{}|{},{},{}|{}|{}",
                q(center[0]),
                q(center[1]),
                q(center[2]),
                q(axis[0]),
                q(axis[1]),
                q(axis[2]),
                q(*major),
                q(*minor)
            ),
        };
        SurfaceKey(body)
    }
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

/// 面的面积（外环减洞）。
fn poly_area(solid: &Solid, ring: &[u32], holes: &[Vec<u32>], n: Vector3) -> f64 {
    let mut a = signed_area(solid, ring, n).abs();
    for h in holes {
        a -= signed_area(solid, h, n).abs();
    }
    a.max(0.0)
}

fn signed_area(solid: &Solid, ring: &[u32], n: Vector3) -> f64 {
    let m = ring.len();
    if m < 3 {
        return 0.0;
    }
    let mut acc = [0.0; 3];
    for i in 0..m {
        let a = solid.verts[ring[i] as usize];
        let b = solid.verts[ring[(i + 1) % m] as usize];
        acc[0] += (a[1] - b[1]) * (a[2] + b[2]);
        acc[1] += (a[2] - b[2]) * (a[0] + b[0]);
        acc[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    dot(acc, n) * 0.5
}

fn polygon_normal(solid: &Solid, ring: &[u32]) -> Option<Vector3> {
    if ring.len() < 3 {
        return None;
    }
    let a = solid.verts[ring[0] as usize];
    let b = solid.verts[ring[1] as usize];
    let c = solid.verts[ring[2] as usize];
    let ab = sub(b, a);
    let ac = sub(c, a);
    normalize([
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ])
}

#[allow(dead_code)]
fn unused(_p: Point3) {}

/// 面的无向边集合。
fn ring_edges(ring: &[u32]) -> Vec<(u32, u32)> {
    let k = ring.len();
    let mut out = Vec::with_capacity(k);
    for j in 0..k {
        let a = ring[j];
        let b = ring[(j + 1) % k];
        out.push(if a < b { (a, b) } else { (b, a) });
    }
    out
}