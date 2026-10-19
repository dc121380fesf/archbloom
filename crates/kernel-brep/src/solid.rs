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

    /// 参数域边界：外环 + 洞，点在 `surface` 的 (u,v) 参数空间里。
    ///
    /// **现在全是 `None`**。它是「面的真相」，`verts` 是从它派生出来的离散缓存；
    /// 两者一致由 [`Face::check`] 保证。
    ///
    /// 迁移计划见 `docs/face-param-domain.md`：步 0 只加字段不动逻辑，
    /// 步 2 开始给圆柱填，步 4 才让 `verts` 真正从它生成。
    pub boundary: Option<Vec<Vec<[f64; 2]>>>,

    /// 这个面承诺的弦高误差（米）。细分点数应由它反解，而不是硬编码 16/64/8。
    ///
    /// 同样现在是 `None`——没填就表示「沿用调用方给的离散精度」。
    pub tolerance: Option<f64>,
}

impl Face {
    /// 构造一个**没有参数域信息**的面（迁移期的默认形态）。
    ///
    /// 迁移完成后这个构造函数应该逐步消失，全部改成
    /// `Face::from_boundary(surface, boundary, tolerance)`。
    pub fn legacy(surface: Surface, verts: Vec<u32>, holes: Vec<Vec<u32>>) -> Self {
        Self {
            surface,
            verts,
            holes,
            boundary: None,
            tolerance: None,
        }
    }

    /// 校验 `verts` 与 `surface` 是否自洽：每个离散点到曲面的距离必须在 `tol` 内。
    ///
    /// 没有 `boundary` 或 `tolerance` 时直接通过（迁移期的面还没填）。
    /// 平面上的点永远在曲面上，所以平面面天然通过。
    pub fn check(&self, solid: &Solid, tol: f64) -> Result<(), String> {
        if self.boundary.is_none() {
            return Ok(());
        }
        for ring in self.all_rings() {
            for &v in ring {
                let p = solid.verts[v as usize];
                let d = self.surface.distance_to(p);
                if d > tol {
                    return Err(format!(
                        "顶点 {v} 距曲面 {d:.3e} 米，超过容差 {tol:.3e}"
                    ));
                }
            }
        }
        Ok(())
    }
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
    /// 实体体检：报告「同一个曲面被切成多少片」。
    ///
    /// # 为什么需要它
    ///
    /// `Face::check` 只能验证「离散点是否落在声称的曲面上」——而现在的顶点
    /// **确实都在曲面上**（它们就是按曲面参数生成的）。真正的债是别的：
    /// 一个圆柱侧面本该是 **1 个面**，实际被切成 16 片共享同一个 `Cylinder`
    /// 标注。顶点没错，**面的划分错了**。
    ///
    /// 这个函数把这种债量出来：返回 `(曲面描述, 面数)`，只列面数 > 1 的。
    /// 它不改任何东西，只让债显形——设计稿步 1 的全部目的就是这个。
    pub fn audit(&self) -> Vec<(String, usize)> {
        let mut groups: Vec<(Surface, usize)> = Vec::new();
        for f in &self.faces {
            // 平面按法线 + 过点归组，曲面按参数归组
            match groups.iter_mut().find(|(s, _)| same_surface_key(s, &f.surface)) {
                Some((_, n)) => *n += 1,
                None => groups.push((f.surface, 1)),
            }
        }
        let mut out: Vec<(String, usize)> = groups
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(s, n)| (describe_surface(&s), n))
            .collect();
        out.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        out
    }


    pub fn push_vert(&mut self, p: Point3) -> u32 {
        self.verts.push(p);
        (self.verts.len() - 1) as u32
    }

    pub fn push_face(&mut self, surface: Surface, verts: Vec<u32>) -> usize {
        self.faces.push(Face::legacy(surface, verts, Vec::new()));
        self.faces.len() - 1
    }

    /// 推一个**带参数域**的面。
    ///
    /// `boundary` 是曲面参数空间里的边界（外环在前，然后是洞），
    /// `tolerance` 是这个面承诺的弦高误差。
    ///
    /// 现在 `verts` 仍然是调用方给的（步 2 不改结构），`boundary` 只是记下来。
    /// 到 `docs/face-param-domain.md` 的步 4，`verts` 会改成由它生成。
    pub fn push_face_with_boundary(
        &mut self,
        surface: Surface,
        verts: Vec<u32>,
        boundary: Vec<Vec<[f64; 2]>>,
        tolerance: f64,
    ) -> usize {
        self.faces.push(Face {
            surface,
            verts,
            holes: Vec::new(),
            boundary: Some(boundary),
            tolerance: Some(tolerance),
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
    /// 欧拉示性数（严格说返回的是 `2(S - G)`）。
    ///
    /// # 为什么不是 `V - E + F`
    ///
    /// 那个简化式默认**每个面同胚于圆盘**。带洞面不是：顶面「外方框 + 内圆孔」
    /// 拓扑上是个环带（annulus），有 2 个边界环。拿 `V - E + F` 算会虚增。
    ///
    /// 实测开孔板：V=136、E=204、F=70 → 136-204+70 = 2，但它是环面（带一通孔），
    /// 应为 0。原因就是两个带洞面各有 2 个环，而 F 只数了 1。
    ///
    /// # 欧拉-庞加莱公式
    ///
    /// ```text
    /// V - E + F - (L - F) = 2(S - G)
    /// ```
    ///
    /// 其中 L 是环总数（外环 + 洞环），S 是壳数，G 是亏格。
    /// 化简即 `V - E + 2F - L`：
    /// - 盒子（球面，G=0）：V=8、E=12、F=6、L=6 → 8-12+12-6 = **2** ✓
    /// - 开孔板（环面，G=1）：136-204+140-72 = **0** ✓
    pub fn euler_characteristic(&self) -> i64 {
        use std::collections::HashSet;
        let mut edges: HashSet<(u32, u32)> = HashSet::new();
        let mut used: HashSet<u32> = HashSet::new();
        let mut rings = 0i64;
        for f in &self.faces {
            for ring in f.all_rings() {
                rings += 1;
                let n = ring.len();
                for i in 0..n {
                    let a = ring[i];
                    let b = ring[(i + 1) % n];
                    edges.insert(if a < b { (a, b) } else { (b, a) });
                    used.insert(a);
                }
            }
        }
        // 只数真正参与面的顶点。`verts` 里可能残留孤立点
        // （面片合并切分后尤多），拿 `verts.len()` 会把欧拉数虚增。
        used.len() as i64 - edges.len() as i64 + 2 * self.faces.len() as i64 - rings
    }

    /// 三角化：每个面按扇形剖分。
    ///
    /// `tol` 用于 B-rep 后端的曲面细分；这里的面边界已经是离散折线，
    /// 所以容差只影响"这个实体是否已经够细"的判断，不做再细分（见 [`super::kernel`]）。
    pub fn triangulate(&self) -> (Vec<Point3>, Vec<Vector3>, Vec<u32>) {
        let mut pos = self.verts.clone();
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

            // 三角化：凸面用扇形（快）；带洞面走桥接 + 耳切（否则洞会被扇形剖分「填上」）。
            //
            // **非凸的无洞面也必须走耳切**。扇形只对凸多边形成立，
            // 非凸时拉出的三角形会互相重叠、而且没有任何报错——
            // 这是典型的静默错误。布尔件的碎片面里非凸很常见。
            if f.holes.is_empty() {
                if let crate::geom::Surface::Sphere { center, radius } = &f.surface {
                    // 球面片不能用纯扇形。
                    //
                    // 扇形从 ring[0] 拉出一排细长三角形，它们全是**弦平面**，
                    // 内接在球面内侧，且弓高损失与「三角形的跨度」相关，
                    // 与边界点多密无关——实测圆角盒子的球面片体积贡献
                    // 0.218191，FreeCAD 是 0.234527，差 7%，且 seg 2→32 收敛不去。
                    //
                    // 每层把三角形拆四、新顶点投回球面，才真正贴向球面。
                    let mut tris: Vec<[u32; 3]> = Vec::new();
                    for i in 1..n - 1 {
                        tris.push([f.verts[0], f.verts[i], f.verts[i + 1]]);
                    }
                    // 两层细分（边界点已有 seg 段，再细一层就够）
                    for _ in 0..2 {
                        let mut next: Vec<[u32; 3]> = Vec::new();
                        for t in &tris {
                            let a = pos[t[0] as usize];
                            let b = pos[t[1] as usize];
                            let c = pos[t[2] as usize];
                            let mut mid = |p: Point3, q: Point3| -> u32 {
                                let m = [
                                    (p[0] + q[0]) * 0.5,
                                    (p[1] + q[1]) * 0.5,
                                    (p[2] + q[2]) * 0.5,
                                ];
                                let d = sub(m, *center);
                                let l = dot(d, d).sqrt();
                                let s = if l > 1e-15 { *radius / l } else { 1.0 };
                                let on = [
                                    center[0] + d[0] * s,
                                    center[1] + d[1] * s,
                                    center[2] + d[2] * s,
                                ];
                                // 复用已有点（避免顶点数爆）——扫一遍区间内的点
                                for k in 0..n {
                                    let v = pos[f.verts[k] as usize];
                                    if (v[0] - on[0]).abs() < 1e-12
                                        && (v[1] - on[1]).abs() < 1e-12
                                        && (v[2] - on[2]).abs() < 1e-12
                                    {
                                        return f.verts[k];
                                    }
                                }
                                pos.push(on);
                                (pos.len() - 1) as u32
                            };
                            let ab = mid(a, b);
                            let bc = mid(b, c);
                            let ca = mid(c, a);
                            next.push([t[0], ab, ca]);
                            next.push([ab, t[1], bc]);
                            next.push([ca, bc, t[2]]);
                            next.push([ab, bc, ca]);
                        }
                        tris = next;
                    }
                    for t in tris {
                        idx.extend_from_slice(&t);
                    }
                } else if ring_is_convex(&pos, &f.verts, face_n) {
                    for i in 1..n - 1 {
                        idx.extend_from_slice(&[f.verts[0], f.verts[i], f.verts[i + 1]]);
                    }
                } else {
                    // 非凸：交给耳切（holes 为空时它会走纯 ear_clip）
                    let tris = crate::poly2d::triangulate_face(&pos, &f.verts, &[], face_n);
                    for t in tris {
                        idx.extend_from_slice(&t);
                    }
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
        if nrm.len() < pos.len() {
            nrm.resize(pos.len(), [0.0, 0.0, 0.0]);
        }
        (pos, nrm, idx)
    }
}

/// 环在给定法线朝向下是否是凸多边形。
///
/// 只要出现一个「凹角」（相邻两点的叉积与法线反向），就判非凸。
/// 这是**扇形剖分能不能用**的前提检查——扇形对凹多边形会切出重叠三角形，
/// 而且不会报错，属于静默错误。
fn ring_is_convex(verts: &[Point3], ring: &[u32], n: Vector3) -> bool {
    let m = ring.len();
    if m < 3 {
        return false;
    }
    let mut sign = 0.0_f64;
    for i in 0..m {
        let a = verts[ring[i] as usize];
        let b = verts[ring[(i + 1) % m] as usize];
        let c = verts[ring[(i + 2) % m] as usize];
        let u = sub(b, a);
        let v = sub(c, b);
        let cr = cross(u, v);
        let d = dot(cr, n);
        if d.abs() < 1e-15 {
            continue; // 共线：跳过
        }
        if sign == 0.0 {
            sign = d;
        } else if d * sign < 0.0 {
            return false;
        }
    }
    true
}

/// 两个曲面是否「同族」：同类且参数相同。
fn same_surface_key(a: &Surface, b: &Surface) -> bool {
    match (a, b) {
        (
            Surface::Plane {
                normal: n1,
                origin: o1,
            },
            Surface::Plane {
                normal: n2,
                origin: o2,
            },
        ) => {
            // **法线必须同向**。用 abs 会把「顶面」和「底面」判成同一个曲面
            // （法线 ±z，点积 -1，取绝对值就变 1 了）——那是两个不同的面。
            dot(*n1, *n2) > 1.0 - 1e-9 && dot(sub(*o2, *o1), *n1).abs() < 1e-9
        }
        (
            Surface::Cylinder {
                axis: a1, radius: r1, ..
            },
            Surface::Cylinder {
                axis: a2, radius: r2, ..
            },
        ) => (r1 - r2).abs() < 1e-12 && dot(*a1, *a2).abs() > 1.0 - 1e-9,
        (
            Surface::Sphere {
                center: c1, radius: r1, ..
            },
            Surface::Sphere {
                center: c2, radius: r2, ..
            },
        ) => {
            (r1 - r2).abs() < 1e-12 && dot(sub(*c2, *c1), sub(*c2, *c1)) < 1e-18
        }
        _ => false,
    }
}

fn describe_surface(s: &Surface) -> String {
    match s {
        Surface::Plane { normal, .. } => {
            format!("平面 n=({:.2},{:.2},{:.2})", normal[0], normal[1], normal[2])
        }
        Surface::Cylinder { radius, .. } => format!("圆柱面 r={radius:.6}"),
        Surface::Sphere { radius, .. } => format!("球面 r={radius:.6}"),
        Surface::Cone { .. } => "圆锥面".to_string(),
        Surface::Torus { .. } => "圆环面".to_string(),
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
    // 参数域（平面）：**用 project 从实际顶点反推**，保证与 eval 同一套基底。
    // 直接写 (R cosθ, R sinθ) 会错——eval 用的基底是 orthonormal_basis(normal)，
    // 对 +Z 来说是 u=(0,1,0)、v=(-1,0,0)，不是一个单位阵。
    let disc_uv: Vec<[f64; 2]> = (0..n)
        .map(|i| {
            let a = TAU * (i as f64) / (n as f64);
            let p = [base[0] + radius * a.cos(), base[1] + radius * a.sin(), base[2]];
            Surface::Plane { origin: base, normal: [0.0, 0.0, -1.0] }
                .project(p)
                .unwrap_or([radius * a.cos(), radius * a.sin()])
        })
        .collect();
    // 弦高误差：离散多边形与真圆的偏差
    let sagitta = radius * (1.0 - (PI / n as f64).cos());
    s.push_face_with_boundary(
        Surface::Plane {
            origin: base,
            normal: [0.0, 0.0, -1.0],
        },
        bot_ring,
        vec![disc_uv.clone()],
        sagitta,
    );
    // 顶面（法线 +Z）
    s.push_face_with_boundary(
        Surface::Plane {
            origin: [base[0], base[1], base[2] + height],
            normal: [0.0, 0.0, 1.0],
        },
        top.clone(),
        vec![disc_uv],
        sagitta,
    );

    // 侧面：n 个四边形，共享同一圆柱标注
    let cyl = Surface::Cylinder {
        origin: base,
        axis: [0.0, 0.0, 1.0],
        radius,
    };
    // 参数域（圆柱）：uv = (θ, z)，整个侧面就是**一个矩形**。
    //
    // 现在仍然切成 n 片输出（步 2 不改结构），但每片记下自己的 θ 区间。
    // 步 4 会用这个矩形把 n 片合成 1 个面。
    let dtheta = TAU / n as f64;
    for i in 0..n {
        let j = (i + 1) % n;
        let t0 = i as f64 * dtheta;
        let t1 = (i + 1) as f64 * dtheta;
        s.push_face_with_boundary(
            cyl,
            vec![bottom[i], bottom[j], top[j], top[i]],
            vec![vec![
                [t0, 0.0],
                [t1, 0.0],
                [t1, height],
                [t0, height],
            ]],
            sagitta,
        );
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

    /// 步 2 验收：圆柱的三类面都填了参数域，且 `check()` 能查实。
    #[test]
    fn 圆柱面都带参数域且自洽() {
        let s = cylinder_solid([0.0, 0.0, 0.0], 0.005, 0.02, 16);
        let with_boundary = s.faces.iter().filter(|f| f.boundary.is_some()).count();
        assert_eq!(
            with_boundary,
            s.faces.len(),
            "圆柱的每个面都应带参数域（顶/底/侧面），实际 {with_boundary}/{}",
            s.faces.len()
        );
        // 所有离散点都应落在它们声称的曲面上
        for (i, f) in s.faces.iter().enumerate() {
            let tol = f.tolerance.expect("应有容差");
            assert!(tol > 0.0, "面 {i} 容差应为正");
            f.check(&s, tol + 1e-12)
                .unwrap_or_else(|e| panic!("面 {i} 不自洽：{e}"));
        }
    }

    /// 参数域的形状要对：侧面是矩形（θ×z），端面是圆盘。
    #[test]
    fn 圆柱参数域形状正确() {
        let n = 16usize;
        let r = 0.005;
        let h = 0.02;
        let s = cylinder_solid([0.0, 0.0, 0.0], r, h, n);
        // 侧面共 n 个面，每个的边界是 4 个点的矩形（ θ 区间 × 高度）
        let sides: Vec<_> = s
            .faces
            .iter()
            .filter(|f| matches!(f.surface, Surface::Cylinder { .. }))
            .collect();
        assert_eq!(sides.len(), n, "侧面应有 {n} 片");
        for f in &sides {
            let b = f.boundary.as_ref().expect("侧面应有参数域");
            assert_eq!(b.len(), 1, "侧面只有外环");
            assert_eq!(b[0].len(), 4, "侧面参数域是矩形");
            for p in &b[0] {
                assert!(p[1].abs() < 1e-12 || (p[1] - h).abs() < 1e-12, "z 只能是 0 或 H");
            }
        }
        // 端面：圆盘，n 个点的多边形
        let planes: Vec<_> = s
            .faces
            .iter()
            .filter(|f| matches!(f.surface, Surface::Plane { .. }))
            .collect();
        assert_eq!(planes.len(), 2, "端面只有 2 个");
        for f in &planes {
            let b = f.boundary.as_ref().expect("端面应有参数域");
            assert_eq!(b[0].len(), n, "端面边界是 {n} 边形");
            for p in &b[0] {
                let rad = (p[0] * p[0] + p[1] * p[1]).sqrt();
                assert!((rad - r).abs() < 1e-12, "端面参数域点应在半径 {r} 上");
            }
        }
    }

    /// 盒子不填参数域（步 2 只动圆柱），`check()` 直接通过。
    #[test]
    fn 盒子暂未填参数域() {
        let s = box_solid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        assert!(s.faces.iter().all(|f| f.boundary.is_none()));
        for f in &s.faces {
            assert!(f.check(&s, 1e-9).is_ok());
        }
    }

    /// 步 3：`eval` 与 `project` 必须互逆。
    ///
    /// 这是参数域能不能用的前提——反推出来的 uv 再 eval 回去，
    /// 必须回到原处。基底选得不对，这里立刻会暴露。
    #[test]
    fn 曲面参数往返一致() {
        use crate::geom::{len, sub};
        let cases: Vec<(Surface, Vec<[f64; 2]>)> = vec![
            (
                Surface::Plane {
                    origin: [1.0, 2.0, 3.0],
                    normal: [0.0, 0.0, 1.0],
                },
                vec![[0.0, 0.0], [1.5, 0.0], [1.5, 2.5], [0.0, 2.5]],
            ),
            (
                Surface::Cylinder {
                    origin: [0.0, 0.0, 0.0],
                    axis: [0.0, 0.0, 1.0],
                    radius: 0.005,
                },
                vec![[0.0, 0.0], [1.0, 0.0], [1.0, 0.02], [0.0, 0.02]],
            ),
            (
                Surface::Sphere {
                    center: [1.0, 1.0, 1.0],
                    radius: 0.2,
                },
                vec![[0.0, 0.0], [1.0, 0.5], [-2.0, 0.3]],
            ),
        ];
        for (s, uvs) in cases {
            for uv in uvs {
                let p = s.eval(uv).expect("eval");
                let back = s.project(p).expect("project");
                let p2 = s.eval(back).expect("eval2");
                let d = len(sub(p, p2));
                assert!(d < 1e-12, "参数往返偏差 {d:.3e}（uv={uv:?}）");
            }
        }
    }

    /// 步 3：细分段数必须跟容差走——tol 越小，段数越多。
    #[test]
    fn 细分段数随容差变化() {
        let cyl = Surface::Cylinder {
            origin: [0.0, 0.0, 0.0],
            axis: [0.0, 0.0, 1.0],
            radius: 0.005,
        };
        let ring = vec![vec![
            [0.0, 0.0],
            [2.0 * std::f64::consts::PI, 0.0],
            [2.0 * std::f64::consts::PI, 0.02],
            [0.0, 0.02],
        ]];
        let coarse = cyl.tessellate_boundary(&ring, 1e-3);
        let fine = cyl.tessellate_boundary(&ring, 1e-6);
        let n_coarse = coarse[0].len();
        let n_fine = fine[0].len();
        assert!(
            n_fine > n_coarse * 10,
            "容差缩 1000 倍，段数应显著增加：{n_coarse} → {n_fine}"
        );
        // 所有细分点都应在圆柱面上
        for p in &fine[0] {
            let d = (p[0] * p[0] + p[1] * p[1]).sqrt();
            assert!((d - 0.005).abs() < 1e-12, "细分点不在圆柱面上");
        }
    }

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
