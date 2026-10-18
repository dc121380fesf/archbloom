//! 实体表示：解析面 + 离散边界。
//!
//! # 表示取舍
//!
//! 严格 B-rep 里，圆柱侧面是**一个**面，边界是两条圆曲线。那需要「曲线环」的拓扑，
//! 复杂度立刻上一个台阶。
//!
//! 这里用**带解析标注的离散拓扑**：
//! - 每个面有一个解析曲面（[`Surface`]），但边界用顶点环（便于三角化与拓扑检查）；
//! - 圆柱侧面离散成 N 个四边形面，它们**共享同一个 `Cylinder` 标注**；
//! - 体积/面积走**闭式解**，不从三角化量。
//!
//! 好处：拓扑简单、tessellate 直接可用、体积依然精确。
//! 代价：STEP 输出时需要把共享同一圆柱标注的相邻面**合并回一个 CYLINDRICAL_SURFACE**
//! （这是 G 阶段的事，不是现在的）。
//!
//! # 为什么体积必须是闭式解
//!
//! 「Φ10 孔」的精确体积是 π·r²·h。如果从三角化量，结果依赖离散段数——
//! 那就不是定义，是逼近。K3 闸门要的就是这个区别。

use std::collections::HashMap;
use std::f64::consts::PI;

use archcore_kernel::{Point3, Vector3};

use crate::geom::{cross, dot, normalize, sub, Surface};

/// 一个面：解析曲面 + 环（逆时针为外法线朝外）。
///
/// `verts` 是外环，`holes` 是内环（每个内环也应围成逆时针的「洞」方向）。
/// 开孔板的顶面就是「外方形 + 内圆孔」——一个面两个环。
/// 面片合并（[`crate::merge`]）之后会产出这种面。
#[derive(Clone, Debug)]
pub struct Face {
    pub surface: Surface,
    pub verts: Vec<u32>,
    /// 内环（孔洞）。空表示简单多边形。
    pub holes: Vec<Vec<u32>>,
}

impl Face {
    /// 遍历这个面的所有环（外环在前，然后各个洞）。
    ///
    /// 拓扑检查（水密、欧拉、边计数）都必须走这里，只看 `verts` 会把洞的边漏掉。
    pub fn all_rings(&self) -> impl Iterator<Item = &Vec<u32>> {
        std::iter::once(&self.verts).chain(self.holes.iter())
    }
}

/// 一条有向半边。
#[derive(Clone, Copy, Debug)]
pub struct HalfEdge {
    /// 起点顶点。
    pub origin: u32,
    /// 反向半边在 `half_edges` 里的下标（无反向时是 `usize::MAX`）。
    pub twin: usize,
    /// 环上的下一条半边。
    pub next: usize,
    /// 所属面（调试用）。
    pub face: usize,
    /// 所属边在 `edges` 里的下标（未配对时为 `usize::MAX`）。
    pub edge: usize,
}

/// 一条无向边：两个互为 twin 的半边。
#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub he: (usize, usize),
}

/// 实体：顶点池 + 面表 + **显式半边拓扑**。
///
/// # 为什么要有半边
///
/// 边的身份不能靠「两个顶点号组成的有序对」推导——两个面如果不是同一次算出来的，
/// 索引就不保证相同，只能靠 weld 合并坐标，浮点一抖就合不上。
/// 倒角、面片合并、圆角三个功能都栽在这里（详见 docs/half-edge-refactor.md）。
#[derive(Clone, Debug, Default)]
pub struct Solid {
    pub verts: Vec<Point3>,
    pub faces: Vec<Face>,
    /// 半边表：每个面的每个环都展开成一条半边链。
    pub half_edges: Vec<HalfEdge>,
    /// 边表：每条无向边一对互为 twin 的半边。
    pub edges: Vec<Edge>,
    /// 构造器给出的闭式体积（立方米）。None 表示未知，需要三角化估算。
    pub analytic_volume: Option<f64>,
    /// 构造器给出的闭式面积。
    pub analytic_area: Option<f64>,
}

impl Solid {
    pub fn push_vert(&mut self, p: Point3) -> u32 {
        self.verts.push(p);
        (self.verts.len() - 1) as u32
    }

    pub fn push_face(&mut self, surface: Surface, verts: Vec<u32>) -> usize {
        self.faces.push(Face {
            surface,
            verts,
            holes: Vec::new(),
        });
        self.faces.len() - 1
    }

    /// 轴向包围盒。
    pub fn bbox(&self) -> (Point3, Point3) {
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        for p in &self.verts {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        if self.verts.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        (lo, hi)
    }

    /// 从面表构建半边与边拓扑。
    ///
    /// 唯一入口：任何几何操作后重建一次，然后跑 [`Self::check_topology`] 自检。
    ///
    /// 配对规则：有向边 `(a,b)` 与 `(b,a)` 互为 twin。
    /// 找不到反向的称为**边界半边**（非闭合壳），不算错误，但要能看出来。
    pub fn build_topology(&mut self) {
        self.half_edges.clear();
        self.edges.clear();

        // 1. 每个面的每个环展开成半边链
        for (fi, f) in self.faces.iter().enumerate() {
            for ring in f.all_rings() {
                let n = ring.len();
                if n < 3 {
                    continue;
                }
                let start = self.half_edges.len();
                for (i, &v) in ring.iter().enumerate() {
                    self.half_edges.push(HalfEdge {
                        origin: v,
                        twin: usize::MAX,
                        next: start + (i + 1) % n,
                        face: fi,
                        edge: usize::MAX,
                    });
                }
            }
        }

        // 2. 配对：用 (起点, 终点) 查反向
        let mut by_pair: HashMap<(u32, u32), usize> = HashMap::new();
        for (i, he) in self.half_edges.iter().enumerate() {
            let a = he.origin;
            let b = self.half_edges[he.next].origin;
            by_pair.insert((a, b), i);
        }
        for i in 0..self.half_edges.len() {
            if self.half_edges[i].twin != usize::MAX {
                continue;
            }
            let a = self.half_edges[i].origin;
            let b = self.half_edges[self.half_edges[i].next].origin;
            if let Some(&j) = by_pair.get(&(b, a)) {
                if j != i && self.half_edges[j].twin == usize::MAX {
                    self.half_edges[i].twin = j;
                    self.half_edges[j].twin = i;
                    let e = self.edges.len();
                    self.half_edges[i].edge = e;
                    self.half_edges[j].edge = e;
                    self.edges.push(Edge { he: (i, j) });
                }
            }
        }
    }

    /// 拓扑自检。返回问题列表，空表示健康。
    pub fn check_topology(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (i, he) in self.half_edges.iter().enumerate() {
            if he.next >= self.half_edges.len() {
                problems.push(format!("半边 {i} 的 next 越界"));
                continue;
            }
            if he.twin != usize::MAX {
                if he.twin >= self.half_edges.len() {
                    problems.push(format!("半边 {i} 的 twin 越界"));
                } else if self.half_edges[he.twin].twin != i {
                    problems.push(format!("半边 {i} 的 twin 不对称"));
                }
            }
        }
        // 每条边必须恰好两个半边
        if !self.half_edges.len().is_multiple_of(2) {
            problems.push(format!(
                "半边数 {} 为奇数，必有未配对的边界半边",
                self.half_edges.len()
            ));
        }
        if self.edges.len() * 2 != self.half_edges.len() {
            problems.push(format!(
                "边数 {} × 2 ≠ 半边数 {}",
                self.edges.len(),
                self.half_edges.len()
            ));
        }
        problems
    }

    /// 统计未配对的边界半边数（闭合实体应为 0）。
    pub fn boundary_half_edges(&self) -> usize {
        self.half_edges
            .iter()
            .filter(|h| h.twin == usize::MAX)
            .count()
    }

    ///
    /// **内环（洞）的边也要算**。合并共面面片之后，顶面的内圆边只出现在 `holes` 里，
    /// 只看 `verts` 会把这些边当成孤立边，误报「不水密」。
    pub fn is_watertight(&self) -> bool {
        use std::collections::HashMap;
        let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
        for f in &self.faces {
            for ring in f.all_rings() {
                let n = ring.len();
                if n < 3 {
                    return false;
                }
                for i in 0..n {
                    let a = ring[i];
                    let b = ring[(i + 1) % n];
                    if a == b {
                        return false;
                    }
                    let k = if a < b { (a, b) } else { (b, a) };
                    *edges.entry(k).or_insert(0) += 1;
                }
            }
        }
        !edges.is_empty() && edges.values().all(|c| *c == 2)
    }

    /// 欧拉示性数 V − E + F。
    ///
    /// 闭合单连通实体为 2；带 g 个通孔的实体为 2 − 2g（开孔板带一个通孔 → 0）。
    pub fn euler_characteristic(&self) -> i64 {
        use std::collections::HashSet;
        let mut edges: HashSet<(u32, u32)> = HashSet::new();
        for f in &self.faces {
            for ring in f.all_rings() {
                let n = ring.len();
                for i in 0..n {
                    let a = ring[i];
                    let b = ring[(i + 1) % n];
                    edges.insert(if a < b { (a, b) } else { (b, a) });
                }
            }
        }
        self.verts.len() as i64 - edges.len() as i64 + self.faces.len() as i64
    }

    /// 三角化：每个面按扇形剖分。
    ///
    /// `tol` 用于 B-rep 后端的曲面细分；这里的面边界已经是离散折线，
    /// 所以容差只影响"这个实体是否已经够细"的判断，不做再细分（见 [`super::kernel`]）。
    pub fn triangulate(&self) -> (Vec<Point3>, Vec<Vector3>, Vec<u32>) {
        let pos = self.verts.clone();
        let mut nrm: Vec<Vector3> = vec![[0.0, 0.0, 0.0]; pos.len()];
        let mut idx: Vec<u32> = Vec::new();

        for f in &self.faces {
            let n = f.verts.len();
            if n < 3 {
                continue;
            }
            // 面法线：优先用解析曲面在质心处的外法线，退化时用顶点叉积
            let centroid = centroid_of(&self.verts, &f.verts);
            let face_n = f
                .surface
                .normal_at(centroid)
                .or_else(|| polygon_normal(&self.verts, &f.verts))
                .unwrap_or([0.0, 0.0, 1.0]);

            // 三角化：简单面沿用扇形（快，且凸面结果一样）；
            // 带洞面必须走桥接 + 耳切，否则洞会被扇形剖分「填上」。
            if f.holes.is_empty() {
                for i in 1..n - 1 {
                    idx.extend_from_slice(&[f.verts[0], f.verts[i], f.verts[i + 1]]);
                }
            } else {
                let tris = crate::poly2d::triangulate_face(&self.verts, &f.verts, &f.holes, face_n);
                for t in tris {
                    idx.extend_from_slice(&t);
                }
                for h in &f.holes {
                    for v in h {
                        if let Some(slot) = nrm.get_mut(*v as usize) {
                            *slot = face_n;
                        }
                    }
                }
            }
            for v in &f.verts {
                if let Some(slot) = nrm.get_mut(*v as usize) {
                    *slot = face_n;
                }
            }
        }
        (pos, nrm, idx)
    }
}

fn centroid_of(verts: &[Point3], ring: &[u32]) -> Point3 {
    let mut s = [0.0, 0.0, 0.0];
    let mut n = 0.0;
    for i in ring {
        if let Some(p) = verts.get(*i as usize) {
            s[0] += p[0];
            s[1] += p[1];
            s[2] += p[2];
            n += 1.0;
        }
    }
    if n == 0.0 {
        return [0.0; 3];
    }
    [s[0] / n, s[1] / n, s[2] / n]
}

fn polygon_normal(verts: &[Point3], ring: &[u32]) -> Option<Vector3> {
    if ring.len() < 3 {
        return None;
    }
    let a = *verts.get(ring[0] as usize)?;
    let b = *verts.get(ring[1] as usize)?;
    let c = *verts.get(ring[2] as usize)?;
    normalize(cross(sub(b, a), sub(c, a)))
}

/// 长方体（轴对齐）。6 个平面面，逆时针朝外。
pub fn box_solid(min: Point3, max: Point3) -> Solid {
    let mut s = Solid::default();
    let v = [
        s.push_vert([min[0], min[1], min[2]]),
        s.push_vert([max[0], min[1], min[2]]),
        s.push_vert([max[0], max[1], min[2]]),
        s.push_vert([min[0], max[1], min[2]]),
        s.push_vert([min[0], min[1], max[2]]),
        s.push_vert([max[0], min[1], max[2]]),
        s.push_vert([max[0], max[1], max[2]]),
        s.push_vert([min[0], max[1], max[2]]),
    ];
    let plane = |n: Vector3| Surface::Plane {
        origin: [0.0, 0.0, 0.0],
        normal: n,
    };
    // 逆时针（从外看）
    s.push_face(plane([0.0, 0.0, -1.0]), vec![v[0], v[3], v[2], v[1]]); // -Z
    s.push_face(plane([0.0, 0.0, 1.0]), vec![v[4], v[5], v[6], v[7]]); // +Z
    s.push_face(plane([0.0, -1.0, 0.0]), vec![v[0], v[1], v[5], v[4]]); // -Y
    s.push_face(plane([0.0, 1.0, 0.0]), vec![v[2], v[3], v[7], v[6]]); // +Y
    s.push_face(plane([-1.0, 0.0, 0.0]), vec![v[0], v[4], v[7], v[3]]); // -X
    s.push_face(plane([1.0, 0.0, 0.0]), vec![v[1], v[2], v[6], v[5]]); // +X

    let dx = max[0] - min[0];
    let dy = max[1] - min[1];
    let dz = max[2] - min[2];
    s.analytic_volume = Some(dx * dy * dz);
    s.analytic_area = Some(2.0 * (dx * dy + dy * dz + dz * dx));
    s
}

/// 圆柱体（轴平行 Z），侧面离散成 `segments` 个四边形面，全部标注为同一个圆柱面。
///
/// 为什么侧面不合成一个面：那需要「曲线环」拓扑（边是圆而非折线）。
/// 这里用离散边界 + 共享解析标注，STEP 输出时再合并（G 阶段）。
pub fn cylinder_solid(base: Point3, radius: f64, height: f64, segments: usize) -> Solid {
    let n = segments.max(3);
    let mut s = Solid::default();

    let bottom: Vec<u32> = (0..n)
        .map(|i| {
            let a = TAU * (i as f64) / (n as f64);
            s.push_vert([base[0] + radius * a.cos(), base[1] + radius * a.sin(), base[2]])
        })
        .collect();
    let top: Vec<u32> = (0..n)
        .map(|i| {
            let a = TAU * (i as f64) / (n as f64);
            s.push_vert([
                base[0] + radius * a.cos(),
                base[1] + radius * a.sin(),
                base[2] + height,
            ])
        })
        .collect();

    // 底面（法线 -Z）：环反向
    let mut bot_ring: Vec<u32> = bottom.clone();
    bot_ring.reverse();
    s.push_face(
        Surface::Plane {
            origin: base,
            normal: [0.0, 0.0, -1.0],
        },
        bot_ring,
    );
    // 顶面（法线 +Z）
    s.push_face(
        Surface::Plane {
            origin: [base[0], base[1], base[2] + height],
            normal: [0.0, 0.0, 1.0],
        },
        top.clone(),
    );

    // 侧面：n 个四边形，共享同一圆柱标注
    let cyl = Surface::Cylinder {
        origin: base,
        axis: [0.0, 0.0, 1.0],
        radius,
    };
    for i in 0..n {
        let j = (i + 1) % n;
        s.push_face(cyl, vec![bottom[i], bottom[j], top[j], top[i]]);
    }

    s.analytic_volume = Some(PI * radius * radius * height);
    // 侧面积 + 两个端面
    s.analytic_area = Some(TAU * radius * height + 2.0 * PI * radius * radius);
    s
}

const TAU: f64 = std::f64::consts::TAU;

/// 按容差算圆柱侧面需要多少段：弦高 ≤ tol。
///
/// 弦高 = r(1 − cos(θ/2))，取 θ = 2π/n → n ≥ π / acos(1 − tol/r)。
pub fn segments_for_tolerance(radius: f64, tol: f64) -> usize {
    if radius <= 0.0 || tol <= 0.0 || tol >= radius {
        return 8;
    }
    let theta = 2.0 * (1.0 - tol / radius).acos();
    if theta <= 0.0 {
        return 256;
    }
    ((TAU / theta).ceil() as usize).clamp(8, 256)
}

/// 三角化后的体积（散度定理），用于与闭式解交叉验证。
pub fn volume_from_mesh(positions: &[Point3], indices: &[u32]) -> f64 {
    let mut sum = 0.0;
    for tri in indices.as_chunks::<3>().0 {
        let (Some(a), Some(b), Some(c)) = (
            positions.get(tri[0] as usize),
            positions.get(tri[1] as usize),
            positions.get(tri[2] as usize),
        ) else {
            continue;
        };
        sum += dot(*a, cross(*b, *c));
    }
    (sum / 6.0).abs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::len;

    #[test]
    fn 长方体体积面积精确() {
        let s = box_solid([0.0, 0.0, 0.0], [2.0, 3.0, 4.0]);
        assert!((s.analytic_volume.unwrap() - 24.0).abs() < 1e-12);
        assert!((s.analytic_area.unwrap() - 2.0 * (6.0 + 12.0 + 8.0)).abs() < 1e-12);
    }

    #[test]
    fn 长方体水密且欧拉数为二() {
        let s = box_solid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        assert!(s.is_watertight());
        assert_eq!(s.euler_characteristic(), 2);
    }

    #[test]
    fn 圆柱体积走闭式解() {
        let r = 0.005; // Φ10mm
        let h = 0.020;
        let s = cylinder_solid([0.0, 0.0, 0.0], r, h, 64);
        let exact = PI * r * r * h;
        let got = s.analytic_volume.unwrap();
        assert!((got - exact).abs() / exact < 1e-12, "闭式解必须精确，got={got}");
    }

    #[test]
    fn 圆柱离散网格体积随段数逼近闭式解() {
        let r = 0.005;
        let h = 0.020;
        let exact = PI * r * r * h;
        let mut prev_err = f64::MAX;
        for segs in [8usize, 16, 32, 64] {
            let s = cylinder_solid([0.0, 0.0, 0.0], r, h, segs);
            let (pos, _, idx) = s.triangulate();
            let mesh_vol = volume_from_mesh(&pos, &idx);
            let err = (mesh_vol - exact).abs() / exact;
            assert!(
                err < prev_err,
                "{segs} 段的误差 {err:.6} 应小于上一档 {prev_err:.6}（离散应收敛）"
            );
            prev_err = err;
        }
    }

    #[test]
    fn 圆柱水密() {
        let s = cylinder_solid([0.0, 0.0, 0.0], 1.0, 2.0, 16);
        assert!(s.is_watertight());
    }

    #[test]
    fn 容差越小段数越多() {
        let coarse = segments_for_tolerance(1.0, 0.1);
        let fine = segments_for_tolerance(1.0, 0.001);
        assert!(fine > coarse, "细容差应给更多段：{fine} vs {coarse}");
        assert!(coarse >= 8 && fine <= 256);
    }

    #[test]
    fn 非法容差回落到默认段数() {
        assert_eq!(segments_for_tolerance(1.0, 0.0), 8);
        assert_eq!(segments_for_tolerance(0.0, 0.1), 8);
        assert_eq!(segments_for_tolerance(1.0, 2.0), 8);
    }

    #[test]
    fn 三角化顶点法线非零() {
        let s = box_solid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let (_, nrm, _) = s.triangulate();
        for n in &nrm {
            assert!(len(*n) > 0.9, "法线应已归一化，实际 {n:?}");
        }
    }
}
