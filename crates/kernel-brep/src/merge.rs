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

use std::collections::{HashMap, VecDeque};

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

    // 反复拼接：用**槽位 + 增量边表 + 优先队列**，避免每轮全量重扫。
    //
    // 队列按**共享边长度降序**：长边优先，让面先沿大边界（外框）向外长，
    // 最后才在孔壁附近的小边处收口。
    // 早先用普通 VecDeque，拼接顺序被 HashMap 遍历顺序支配，
    // 局部碎片先自己闭成环，结果形成 79 个**小洞**（理论应该只有 2 个大洞）。
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

    let mut queue: VecDeque<(u32, u32)> = VecDeque::new();
    {
        // 初始顺序必须**确定**：HashMap 遍历顺序随机，不排序的话同输入每次跑出
        // 不同面数（467 / 476 / 484 波动）。拼接是贪心的，顺序一变结果就变。
        let mut init: Vec<(u32, u32)> = edge_slots
            .iter()
            .filter(|(_, users)| users.len() == 2)
            .map(|(e, _)| *e)
            .collect();
        init.sort_unstable();
        for e in init {
            queue.push_back(e);
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
        // 法线必须用**这两个面自己**的，不能拿面 0 的（否则二维投影基全错）
        let nrm = fi
            .surface
            .normal_at(solid.verts[fi.verts[0] as usize])
            .unwrap_or([0.0, 0.0, 1.0]);
        let Some(new_face) = try_splice(solid, &fi, &fj, nrm, true) else {
            continue;
        };
        let area = poly_area(solid, &new_face.verts, &new_face.holes, nrm);
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
        // 也把 i 共边的边入队
        for edge in ring_edges(&new_face.verts) {
            queue.push_back(edge);
        }

        // 关键补充：把**邻居的所有边**也入队。
        // 只 push 新面自己的边时，那些「本来因邻居被并掉而变回单面」的边不会被重试，
        // 合并会提前收敛。
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
///
/// # 带洞拼接
///
/// 拼接后如果环里出现重复顶点，说明两段路径**首尾相遇**。这有两种可能：
///
/// * **形成洞**：分出的两个环一个完全包含另一个；
/// * **自交**：两个环交叉（拼错了，必须拒掉）。
///
/// # 为什么要分两轮（allow_holes）
///
/// 如果一开始就允许产生洞，相邻两块刚拼上就围出一个小圈，结果是
/// **47 个 6 点的碎片洞**而不是一个完整圆孔——顶面孔口写不成圆边，STEP 壳不闭合。
///
/// 所以第一轮 `allow_holes = false`：让面先长成大片；
/// 第二轮才允许闭合。这样洞会在拼接接近尾声时一次性形成。
fn try_splice(
    solid: &Solid,
    a: &Face,
    b: &Face,
    normal: Vector3,
    allow_holes: bool,
) -> Option<Face> {
    // 找共享边：a 有向边 (x→y)，b 必须是 (y→x)
    let ra = &a.verts;
    let rb = &b.verts;
    let ka = ra.len();
    for ia in 0..ka {
        let x = ra[ia];
        let y = ra[(ia + 1) % ka];
        let kb = rb.len();
        let mut ib = None;
        for j in 0..kb {
            if rb[j] == y && rb[(j + 1) % kb] == x {
                ib = Some(j);
                break;
            }
        }
        let Some(ib) = ib else { continue };

        // 拼：从 a 的 y 绕到 x（不含 x），再接 b 从 x 绕到 y（不含 y）
        let mut ring: Vec<u32> = Vec::with_capacity(ka + kb);
        let mut k = (ia + 1) % ka;
        while ra[k] != x {
            ring.push(ra[k]);
            k = (k + 1) % ka;
        }
        let mut m = (ib + 1) % kb;
        while rb[m] != y {
            ring.push(rb[m]);
            m = (m + 1) % kb;
        }
        if ring.len() < 3 {
            return None;
        }

        // 带洞面不参与拼接（简化：洞的合并需要另一套边界匹配）
        if !a.holes.is_empty() || !b.holes.is_empty() {
            return None;
        }

        // 找重复顶点
        let mut seen: HashMap<u32, usize> = HashMap::new();
        let mut dup: Option<(usize, usize)> = None;
        for (i, &v) in ring.iter().enumerate() {
            if let Some(&p) = seen.get(&v) {
                dup = Some((p, i));
                break;
            }
            seen.insert(v, i);
        }

        let Some((p1, p2)) = dup else {
            return Some(Face {
                surface: a.surface,
                verts: ring,
                holes: Vec::new(),
            });
        };

        // 第一轮不允许产生洞：提前闭合会在孔口周围留下大量碎片洞
        if !allow_holes {
            return None;
        }

        // 分成两环
        let ring_a: Vec<u32> = ring[p1..p2].to_vec();
        let mut ring_b: Vec<u32> = ring[p2..].to_vec();
        ring_b.extend_from_slice(&ring[..p1]);
        if ring_a.len() < 3 || ring_b.len() < 3 {
            return None;
        }

        // 投影到面内二维，判断包含关系
        let (u, w) = crate::geom::orthonormal_basis(normal)?;
        let origin = solid.verts[ring_a[0] as usize];
        let to2 = |i: u32| -> [f64; 2] {
            let d = sub(solid.verts[i as usize], origin);
            [dot(d, u), dot(d, w)]
        };
        let pa2: Vec<[f64; 2]> = ring_a.iter().map(|&i| to2(i)).collect();
        let pb2: Vec<[f64; 2]> = ring_b.iter().map(|&i| to2(i)).collect();
        let aa = poly_area2(&pa2).abs();
        let ab = poly_area2(&pb2).abs();
        if aa <= 0.0 || ab <= 0.0 {
            return None;
        }

        // **洞太小的就拒掉**：那是提前闭合留下的碎片，不是真孔。
        //
        // 实测：不卡这一条时，顶面会形成 47 个 6 点的碎片洞（半径差 55%），
        // 孔口写不成圆边，STEP 壳不闭合。真实的孔（Φ10 在 20×20 板上）
        // 面积占比 ~20%，碎片则远小于 1%。
        let small = aa.min(ab);
        let big = aa.max(ab);
        if small / big < 0.02 {
            return None;
        }

        let (outer, inner, inner2) = if aa > ab {
            (ring_a, ring_b, pb2)
        } else {
            (ring_b, ring_a, pa2)
        };
        let outer2: Vec<[f64; 2]> = outer.iter().map(|&i| to2(i)).collect();

        // 小环必须完全在大环内部，否则是自交
        if !inner2.iter().all(|&p| point_in_poly(p, &outer2)) {
            return None;
        }

        return Some(Face {
            surface: a.surface,
            verts: outer,
            holes: vec![inner],
        });
    }
    None
}

fn poly_area2(poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    let mut s = 0.0;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        s += a[0] * b[1] - b[0] * a[1];
    }
    s * 0.5
}

/// 点在多边形内（射线法，含边界）。
fn point_in_poly(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let n = poly.len();
    let mut inside = false;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let t = (p[1] - a[1]) / (b[1] - a[1]);
            let x = a[0] + t * (b[0] - a[0]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
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
