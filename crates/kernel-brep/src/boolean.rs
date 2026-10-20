//! 布尔的**基础数据结构**：平面、带朝向的多边形、平面切割。
//!
//! # 这个文件里有什么、没有什么
//!
//! 只有三样东西：
//!
//! - `Plane`：带法线的平面，负责点分类（前 / 后 / 面上）
//! - `Polygon`：带朝向的多边形（环 + 所属平面 + 曲面标注）
//! - `split_polygon`：用平面把一个多边形切成前后两片
//!
//! **没有 BSP 树**。早期版本用过 BSP，后来发现它的 `clip_polygons` 依赖树形结构，
//! 同一个几何换个输入顺序结果就不同，于是正式路径改成了
//! [`csg`](crate::csg) 的「双方所有面平面切分 + 射线法分类 + 缝合」。
//! BSP 树的代码连同它的测试已经删除，文件也从 `bsp.rs` 改名为 `boolean.rs`——
//! 因为它现在装的是布尔的基础件，而不是 BSP。
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

use crate::geom::{add, cross, dot, len, normalize, scale, sub, Surface};

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
    /// 这个多边形所属的**解析曲面**。
    ///
    /// 布尔切分时子多边形继承它，重建时再用它当面的标注。
    /// 早先没有这个字段，布尔一跑就把圆柱面的身份退成 `Plane` ——
    /// 实测开孔板 169 个面全是 plane，真曲面导出变成了“只对没做过布尔的圆柱生效”。
    pub surface: Surface,

    /// 内环（洞）。空表示简单多边形。
    ///
    /// # 为什么必须有
    ///
    /// 布尔代数的封闭性要求中间态支持带洞面：挖一个盲孔，顶面立刻有洞。
    /// 早先没这个字段，第一次布尔的输出（顶面 = 外环 132 点 + 孔环 64 点）
    /// 进第二次布尔时洞被丢掉，顶面变实心 —— 实测「1 孔 + 1 槽」体积比光板还大 34%。
    ///
    /// # 定向约定
    ///
    /// 依 `plane.normal`：外环逆时针，每个洞顺时针（面积相减）。
    pub holes: Vec<Vec<Point3>>,
}

impl Polygon {
    pub fn new(verts: Vec<Point3>) -> Option<Self> {
        let plane = Plane::from_polygon(&verts)?;
        let surface = Surface::Plane {
            origin: verts[0],
            normal: plane.normal,
        };
        Some(Self {
            verts,
            plane,
            surface,
            holes: Vec::new(),
        })
    }

    /// 带曲面标注构造（布尔链里传递原始曲面用）。
    pub fn with_surface(verts: Vec<Point3>, surface: Surface) -> Option<Self> {
        let plane = Plane::from_polygon(&verts)?;
        Some(Self {
            verts,
            plane,
            surface,
            holes: Vec::new(),
        })
    }

    /// 带洞构造。洞必须在外环内，且与 `plane.normal` 反向（顺时针）。
    pub fn with_holes(
        verts: Vec<Point3>,
        holes: Vec<Vec<Point3>>,
        surface: Surface,
    ) -> Option<Self> {
        let plane = Plane::from_polygon(&verts)?;
        Some(Self {
            verts,
            plane,
            surface,
            holes,
        })
    }

    pub fn flip(&mut self) {
        self.verts.reverse();
        self.plane.flip();
        for h in &mut self.holes {
            h.reverse();
        }
    }

    /// 面积 = 外环 − Σ 洞。
    ///
    /// 早先只算外环、不认洞——带洞面会被当成实心，体积直接变大。
    pub fn area(&self) -> f64 {
        fn fan_area(verts: &[Point3]) -> f64 {
            let mut sum = 0.0;
            for i in 1..verts.len().saturating_sub(1) {
                let u = sub(verts[i], verts[0]);
                let v = sub(verts[i + 1], verts[0]);
                sum += len(cross(u, v)) / 2.0;
            }
            sum
        }
        let mut a = fan_area(&self.verts);
        for h in &self.holes {
            a -= fan_area(h);
        }
        a
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
                // 子多边形**继承父面的曲面标注**，否则布尔一跑圆柱就退成平面
                if let Some(p) = Polygon::with_surface(f, poly.surface) {
                    front.push(p);
                }
            }
            if b.len() >= 3 {
                if let Some(p) = Polygon::with_surface(b, poly.surface) {
                    back.push(p);
                }
            }
        }
    }
}
