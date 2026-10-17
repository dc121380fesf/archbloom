//! BSP 树布尔运算（CSG）。
//!
//! # 算法选择
//!
//! 平面多面体的布尔有三条路：BSP 树、面片求交 + 拓扑重建、体素化。
//! 这里选 **BSP**，理由是它把「分类」这件事做得最干净：
//!
//! 布尔运算的本质是「哪些面片保留」，而 BSP 用分割平面把空间二分，
//! 每个面片落在前/后/共面三类之一——保不保留就是查表。
//!
//! 代价：结果是一堆**无拓扑连接的面片**，需要重新缝合顶点。
//! 这一步在 [`rebuild`](crate::rebuild) 里做。
//!
//! # 精度
//!
//! 一切距离比较用 `EPS = 1e-9`（米）——对建筑（米级）和机械（毫米级）都够用。
//! 更严格的做法是有理数精确算术（Blender Exact Boolean 的路子），
//! 但那要重写所有谓词；先用容差版把链路跑通。
//!
//! # 已知局限（E 阶段）
//!
//! - 只处理**平面**多边形：曲面（圆柱侧面）先离散成平面片再进来，
//!   所以圆柱参与布尔时得到的仍是离散结果；
//! - 共面重叠面片可能产生重合面（同一位置两个片）——真实性检查会暴露，
//!   但暂不做"去重合并"。

use archcore_kernel::{Point3, Vector3};

use crate::geom::{add, cross, dot, len, normalize, scale, sub};

/// 距离容差（米）。小于它的距离视为 0。
pub const EPS: f64 = 1e-9;

/// 平面：`normal · x = w`，normal 为单位向量。
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub normal: Vector3,
    pub w: f64,
}

impl Plane {
    pub fn from_normal_and_point(n: Vector3, p: Point3) -> Option<Self> {
        let normal = normalize(n)?;
        Some(Self {
            normal,
            w: dot(normal, p),
        })
    }

    /// 用 Newell 法从多边形顶点算平面（比取前三点叉积稳健）。
    pub fn from_polygon(verts: &[Point3]) -> Option<Self> {
        if verts.len() < 3 {
            return None;
        }
        let mut n = [0.0, 0.0, 0.0];
        for i in 0..verts.len() {
            let a = verts[i];
            let b = verts[(i + 1) % verts.len()];
            n[0] += (a[1] - b[1]) * (a[2] + b[2]);
            n[1] += (a[2] - b[2]) * (a[0] + b[0]);
            n[2] += (a[0] - b[0]) * (a[1] + b[1]);
        }
        let normal = normalize(n)?;
        Some(Self {
            normal,
            w: dot(normal, verts[0]),
        })
    }

    pub fn flip(&mut self) {
        self.normal = scale(self.normal, -1.0);
        self.w = -self.w;
    }

    pub fn distance(&self, p: Point3) -> f64 {
        dot(self.normal, p) - self.w
    }
}

/// 多边形：顶点环（逆时针）+ 所在平面。
#[derive(Clone, Debug)]
pub struct Polygon {
    pub verts: Vec<Point3>,
    pub plane: Plane,
}

impl Polygon {
    pub fn new(verts: Vec<Point3>) -> Option<Self> {
        let plane = Plane::from_polygon(&verts)?;
        Some(Self { verts, plane })
    }

    pub fn flip(&mut self) {
        self.verts.reverse();
        self.plane.flip();
    }

    /// 扇形三角化的面积（用于统计与检查）。
    pub fn area(&self) -> f64 {
        let mut sum = 0.0;
        for i in 1..self.verts.len().saturating_sub(1) {
            let u = sub(self.verts[i], self.verts[0]);
            let v = sub(self.verts[i + 1], self.verts[0]);
            sum += len(cross(u, v)) / 2.0;
        }
        sum
    }

    /// 质心（顶点平均，够用）。
    pub fn centroid(&self) -> Point3 {
        let n = self.verts.len() as f64;
        let mut s = [0.0, 0.0, 0.0];
        for v in &self.verts {
            s[0] += v[0];
            s[1] += v[1];
            s[2] += v[2];
        }
        [s[0] / n, s[1] / n, s[2] / n]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Coplanar,
    Front,
    Back,
    Spanning,
}

fn classify(v: f64) -> Side {
    if v < -EPS {
        Side::Back
    } else if v > EPS {
        Side::Front
    } else {
        Side::Coplanar
    }
}

/// 用平面切一个多边形，把结果分到四个桶里。
///
/// 这是整个 BSP 的基础：分割平面把多边形切成前/后两段，落在平面上的归共面。
pub(crate) fn split_polygon(
    plane: &Plane,
    poly: &Polygon,
    coplanar_front: &mut Vec<Polygon>,
    coplanar_back: &mut Vec<Polygon>,
    front: &mut Vec<Polygon>,
    back: &mut Vec<Polygon>,
) {
    let mut poly_type = Side::Coplanar;
    let mut types: Vec<Side> = Vec::with_capacity(poly.verts.len());

    for v in &poly.verts {
        let t = classify(plane.distance(*v));
        poly_type = match (poly_type, t) {
            (Side::Coplanar, ty) => ty,
            (Side::Front, Side::Back) | (Side::Back, Side::Front) => Side::Spanning,
            (Side::Front, Side::Coplanar) => Side::Front,
            (Side::Back, Side::Coplanar) => Side::Back,
            (a, _) => a,
        };
        types.push(t);
    }

    match poly_type {
        Side::Coplanar => {
            // 同向归前面，反向归后面
            if dot(plane.normal, poly.plane.normal) > 0.0 {
                coplanar_front.push(poly.clone());
            } else {
                coplanar_back.push(poly.clone());
            }
        }
        Side::Front => front.push(poly.clone()),
        Side::Back => back.push(poly.clone()),
        Side::Spanning => {
            let n = poly.verts.len();
            let mut f: Vec<Point3> = Vec::new();
            let mut b: Vec<Point3> = Vec::new();
            for i in 0..n {
                let j = (i + 1) % n;
                let ti = types[i];
                let tj = types[j];
                let vi = poly.verts[i];
                let vj = poly.verts[j];

                if ti != Side::Back {
                    f.push(vi);
                }
                if ti != Side::Front {
                    b.push(vi);
                }
                if (ti == Side::Front && tj == Side::Back)
                    || (ti == Side::Back && tj == Side::Front)
                {
                    // 边穿过平面：插交点（两侧各加一份）
                    let di = plane.distance(vi);
                    let dj = plane.distance(vj);
                    let denom = di - dj;
                    if denom.abs() > f64::EPSILON {
                        let t = di / denom;
                        let p = add(vi, scale(sub(vj, vi), t));
                        f.push(p);
                        b.push(p);
                    }
                }
            }
            if f.len() >= 3 {
                if let Some(p) = Polygon::new(f) {
                    front.push(p);
                }
            }
            if b.len() >= 3 {
                if let Some(p) = Polygon::new(b) {
                    back.push(p);
                }
            }
        }
    }
}

/// BSP 节点。
#[derive(Clone, Debug, Default)]
pub struct Node {
    plane: Option<Plane>,
    front: Option<Box<Node>>,
    back: Option<Box<Node>>,
    polygons: Vec<Polygon>,
}

impl Node {
    pub fn new(polygons: Vec<Polygon>) -> Self {
        let mut n = Self::default();
        n.build(polygons);
        n
    }

    /// 构建：取首个多边形所在平面切分，递归前后。
    pub fn build(&mut self, polygons: Vec<Polygon>) {
        if polygons.is_empty() {
            return;
        }
        let plane = match self.plane {
            Some(p) => p,
            None => polygons[0].plane,
        };
        self.plane = Some(plane);

        let mut front: Vec<Polygon> = Vec::new();
        let mut back: Vec<Polygon> = Vec::new();
        let mut cf: Vec<Polygon> = Vec::new();
        let mut cb: Vec<Polygon> = Vec::new();

        for p in &polygons {
            split_polygon(&plane, p, &mut cf, &mut cb, &mut front, &mut back);
        }
        self.polygons.extend(cf);
        self.polygons.extend(cb);

        if !front.is_empty() {
            self.front.get_or_insert_with(Box::default).build(front);
        }
        if !back.is_empty() {
            self.back.get_or_insert_with(Box::default).build(back);
        }
    }

    /// 翻转子树（内外互换）。
    pub fn invert(&mut self) {
        for p in &mut self.polygons {
            p.flip();
        }
        if let Some(pl) = &mut self.plane {
            pl.flip();
        }
        if let Some(f) = &mut self.front {
            f.invert();
        }
        if let Some(b) = &mut self.back {
            b.invert();
        }
        std::mem::swap(&mut self.front, &mut self.back);
    }

    /// 裁掉落在本树「内部」的多边形。
    pub fn clip_polygons(&self, polygons: Vec<Polygon>) -> Vec<Polygon> {
        let Some(plane) = self.plane else {
            return polygons;
        };
        let mut front: Vec<Polygon> = Vec::new();
        let mut back: Vec<Polygon> = Vec::new();
        let mut cf: Vec<Polygon> = Vec::new();
        let mut cb: Vec<Polygon> = Vec::new();

        for p in &polygons {
            split_polygon(&plane, p, &mut cf, &mut cb, &mut front, &mut back);
        }
        // 共面朝前保留；共面朝后（背对本树）在内部，直接丢弃。
        // 曾试过让它进 back 再判，结果把该消隐的共享面保留下来（面积 10→12），
        // 因为 back 子树在边界处会把它当外部。这条路不对。
        front.extend(cf);

        let mut out = match &self.front {
            Some(f) => f.clip_polygons(front),
            None => front,
        };
        // 落在后面且本树没有后子树 → 在内部，丢弃
        if let Some(b) = &self.back {
            back = b.clip_polygons(back);
        } else {
            back.clear();
        }
        out.extend(back);
        out
    }

    pub fn clip_to(&mut self, other: &Node) {
        self.polygons = other.clip_polygons(std::mem::take(&mut self.polygons));
        if let Some(f) = &mut self.front {
            f.clip_to(other);
        }
        if let Some(b) = &mut self.back {
            b.clip_to(other);
        }
    }

    pub fn all_polygons(&self) -> Vec<Polygon> {
        let mut out = self.polygons.clone();
        if let Some(f) = &self.front {
            out.extend(f.all_polygons());
        }
        if let Some(b) = &self.back {
            out.extend(b.all_polygons());
        }
        out
    }
}

/// 并集。
pub fn union(a: &Node, b: &Node) -> Vec<Polygon> {
    let mut a = a.clone();
    let mut b = b.clone();
    a.clip_to(&b);
    b.clip_to(&a);
    b.invert();
    b.clip_to(&a);
    b.invert();
    a.build(b.all_polygons());
    a.all_polygons()
}

/// 差集（a − b）。
pub fn difference(a: &Node, b: &Node) -> Vec<Polygon> {
    let mut a = a.clone();
    let mut b = b.clone();
    a.invert();
    a.clip_to(&b);
    b.clip_to(&a);
    b.invert();
    b.clip_to(&a);
    b.invert();
    a.build(b.all_polygons());
    a.invert();
    a.all_polygons()
}

/// 交集。
pub fn intersection(a: &Node, b: &Node) -> Vec<Polygon> {
    let mut a = a.clone();
    let mut b = b.clone();
    a.invert();
    b.clip_to(&a);
    b.invert();
    a.clip_to(&b);
    b.clip_to(&a);
    a.build(b.all_polygons());
    a.invert();
    a.all_polygons()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(min: f64, max: f64) -> Node {
        box_node([min, min, min], [max, max, max])
    }

    /// 任意尺寸长方体——布尔测试需要「只切一刀」而不是「切个小立方体」
    fn box_node(min: Point3, max: Point3) -> Node {
        let mut polys = Vec::new();
        let v = [
            [min[0], min[1], min[2]],
            [max[0], min[1], min[2]],
            [max[0], max[1], min[2]],
            [min[0], max[1], min[2]],
            [min[0], min[1], max[2]],
            [max[0], min[1], max[2]],
            [max[0], max[1], max[2]],
            [min[0], max[1], max[2]],
        ];
        // 六个面，逆时针朝外
        for ring in [
            [0usize, 3, 2, 1], // -Z
            [4, 5, 6, 7],      // +Z
            [0, 1, 5, 4],      // -Y
            [2, 3, 7, 6],      // +Y
            [0, 4, 7, 3],      // -X
            [1, 2, 6, 5],      // +X
        ] {
            let verts: Vec<Point3> = ring.iter().map(|i| v[*i]).collect();
            if let Some(p) = Polygon::new(verts) {
                polys.push(p);
            }
        }
        Node::new(polys)
    }

    fn total_area(polys: &[Polygon]) -> f64 {
        polys.iter().map(|p| p.area()).sum()
    }

    #[test]
    fn 并集_两个相邻立方体合成一个长方体() {
        let a = cube(0.0, 1.0);
        let b = cube(1.0, 2.0); // 沿 x 相接
        let out = union(&a, &b);
        // 结果表面积 = 长方体 2×1×1 的 10
        let area = total_area(&out);
        assert!((area - 10.0).abs() < 1e-6, "并集表面积应为 10，实际 {area}");
    }

    #[test]
    fn 差集_一刀切掉一半() {
        let a = cube(0.0, 1.0);
        // 厚板：x 从 0.5 开始，y/z 宽宽地包绕——只切 x 方向
        let b = box_node([0.5, -1.0, -1.0], [2.0, 2.0, 2.0]);
        let out = difference(&a, &b);
        let vol = volume_of(&out);
        assert!((vol - 0.5).abs() < 1e-6, "差集体积应为 0.5，实际 {vol}");
    }

    #[test]
    fn 交集_一刀切出一半() {
        let a = cube(0.0, 1.0);
        let b = box_node([0.5, -1.0, -1.0], [2.0, 2.0, 2.0]);
        let out = intersection(&a, &b);
        let vol = volume_of(&out);
        assert!((vol - 0.5).abs() < 1e-6, "交集体积应为 0.5，实际 {vol}");
    }

    #[test]
    fn 交集_两立方体只重叠一角() {
        // [0,1]³ 与 [0.5,1.5]³ 的重叠只有 [0.5,1]³ = 0.125
        let a = cube(0.0, 1.0);
        let c = cube(0.5, 1.5);
        let out = intersection(&a, &c);
        let vol = volume_of(&out);
        assert!((vol - 0.125).abs() < 1e-6, "重叠角体积应为 0.125，实际 {vol}");
    }

    #[test]
    fn 不接触时交集为空() {
        let a = cube(0.0, 1.0);
        let b = cube(5.0, 6.0);
        let out = intersection(&a, &b);
        let vol = volume_of(&out);
        assert!(vol < 1e-9, "不相交时交集体积应≈0，实际 {vol}");
    }

    #[test]
    fn 差集_完全包含时为空() {
        let a = cube(0.0, 1.0);
        let b = cube(-1.0, 2.0); // b 完全包住 a
        let out = difference(&a, &b);
        let vol = volume_of(&out);
        assert!(vol < 1e-9, "被完全吞掉时差集应为空，实际 {vol}");
    }

    fn volume_of(polys: &[Polygon]) -> f64 {
        // 散度定理：Σ 顶点扇形三角的 a·(b×c)/6
        let mut sum = 0.0;
        for p in polys {
            for i in 1..p.verts.len().saturating_sub(1) {
                let a = p.verts[0];
                let b = p.verts[i];
                let c = p.verts[i + 1];
                sum += dot(a, cross(b, c));
            }
        }
        (sum / 6.0).abs()
    }
}
