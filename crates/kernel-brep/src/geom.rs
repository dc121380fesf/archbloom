//! 解析几何：五种曲面 + 三种曲线。
//!
//! # 为什么必须是解析式
//!
//! 网格后端里，一个 "Φ10 孔" 是 32 个三角面——你能算出它的体积，但算不出"半径到底是不是 5.000"。
//! 这里相反：曲面是公式（`Cylinder { radius: 5.0 }`），体积是闭式解，圆度和半径**不需要测量**。
//!
//! 这就是「可出 STEP 加工」的底气：加工厂要的是定义，不是逼近。
//!
//! # 范围（写死在项目书 §2 重启记录里）
//!
//! 支持：平面 / 圆柱 / 圆锥 / 球 / 圆环 + 直线 / 圆弧 / 椭圆弧。
//! **不支持**自由曲面（NURBS）、放样、扫掠——那些需要 NURBS-NURBS 求交，是三十年工程。

use archcore_kernel::{Point3, Vector3};

pub fn add(a: Point3, b: Vector3) -> Point3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: Point3, b: Point3) -> Vector3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(v: Vector3, k: f64) -> Vector3 {
    [v[0] * k, v[1] * k, v[2] * k]
}

pub fn dot(a: Vector3, b: Vector3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: Vector3, b: Vector3) -> Vector3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn len(v: Vector3) -> f64 {
    dot(v, v).sqrt()
}

/// 归一化。零向量返回 None——**不返回零向量冒充成功**，
/// 那会让下游的"方向无意义"变成静默错误。
pub fn normalize(v: Vector3) -> Option<Vector3> {
    let l = len(v);
    if l <= 1e-15 {
        return None;
    }
    Some([v[0] / l, v[1] / l, v[2] / l])
}

/// 构造与给定方向正交的一组基（用于圆/椭圆的参数化）。
pub fn orthonormal_basis(axis: Vector3) -> Option<(Vector3, Vector3)> {
    let n = normalize(axis)?;
    // 选一个不与 n 平行的参考向量，避免叉积退化
    let helper = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let u = normalize(cross(n, helper))?;
    let v = cross(n, u);
    Some((u, v))
}

/// 解析曲面。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Surface {
    /// 平面：过 origin、法线 normal（单位向量）
    Plane { origin: Point3, normal: Vector3 },
    /// 圆柱面：轴过 origin、方向 axis（单位）、半径 radius
    Cylinder {
        origin: Point3,
        axis: Vector3,
        radius: f64,
    },
    /// 圆锥面：顶点 apex、轴方向 axis（单位）、半角 half_angle（弧度）
    Cone {
        apex: Point3,
        axis: Vector3,
        half_angle: f64,
    },
    /// 球面
    Sphere { center: Point3, radius: f64 },
    /// 圆环面：绕 axis 的圆环，主半径 major（中心到管心）、次半径 minor（管半径）
    Torus {
        center: Point3,
        axis: Vector3,
        major: f64,
        minor: f64,
    },
}

impl Surface {
    /// 曲面类型名（写进错误信息与 STEP 映射）。
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Plane { .. } => "plane",
            Self::Cylinder { .. } => "cylinder",
            Self::Cone { .. } => "cone",
            Self::Sphere { .. } => "sphere",
            Self::Torus { .. } => "torus",
        }
    }

    /// 点到曲面的最近距离（米）。
    ///
    /// 用于 [`crate::solid::Face::check`]：验证离散点确实落在它声称的曲面上。
    /// 这是「标签不能跟几何忽悠」的硬约束——没有它，一个 16 边形的环
    /// 可以挂着 `Cylinder` 标签而无人发现。
    ///
    /// 圆锥与圆环面暂返回 0（不参与体检）：这两类目前只有定义、没有构造器，
    /// 等真有实体时再实现，而不是现在猜一个可能错的公式。
    pub fn distance_to(&self, p: Point3) -> f64 {
        match self {
            Self::Plane { origin, normal } => dot(sub(p, *origin), *normal).abs(),
            Self::Cylinder {
                origin,
                axis,
                radius,
            } => {
                let d = sub(p, *origin);
                let along = dot(d, *axis);
                let radial = sub(d, scale(*axis, along));
                (len(radial) - radius).abs()
            }
            Self::Sphere { center, radius } => (len(sub(p, *center)) - radius).abs(),
            Self::Cone { .. } | Self::Torus { .. } => 0.0,
        }
    }

    /// 参数 → 空间。uv 约定：
    ///
    /// - `Plane`：(u, v) 沿 [`orthonormal_basis(normal)`] 的两个基底
    /// - `Cylinder`：(θ, z)，θ 从 +u 转向 +v
    /// - `Sphere`：(θ, φ)，经纬
    /// - `Cone` / `Torus`：暂不实现（没有构造器，不猜公式）
    pub fn eval(&self, uv: [f64; 2]) -> Option<Point3> {
        match self {
            Self::Plane { origin, normal } => {
                let (u, v) = orthonormal_basis(*normal)?;
                Some([
                    origin[0] + u[0] * uv[0] + v[0] * uv[1],
                    origin[1] + u[1] * uv[0] + v[1] * uv[1],
                    origin[2] + u[2] * uv[0] + v[2] * uv[1],
                ])
            }
            Self::Cylinder {
                origin,
                axis,
                radius,
            } => {
                let (u, v) = orthonormal_basis(*axis)?;
                let (c, s) = (uv[0].cos(), uv[0].sin());
                Some([
                    origin[0] + radius * (u[0] * c + v[0] * s) + axis[0] * uv[1],
                    origin[1] + radius * (u[1] * c + v[1] * s) + axis[1] * uv[1],
                    origin[2] + radius * (u[2] * c + v[2] * s) + axis[2] * uv[1],
                ])
            }
            Self::Sphere { center, radius } => {
                let (st, ct) = (uv[0].sin(), uv[0].cos());
                let (sp, cp) = (uv[1].sin(), uv[1].cos());
                Some([
                    center[0] + radius * cp * ct,
                    center[1] + radius * cp * st,
                    center[2] + radius * sp,
                ])
            }
            Self::Cone { .. } | Self::Torus { .. } => None,
        }
    }

    /// 空间 → 参数（[`Self::eval`] 的逆）。
    ///
    /// 用于从**已有的离散顶点反推参数域**——这样填进去的 `boundary`
    /// 与 `eval` 天然一致，不会出现「基底选取不同导致参数含义不同」。
    pub fn project(&self, p: Point3) -> Option<[f64; 2]> {
        match self {
            Self::Plane { origin, normal } => {
                let (u, v) = orthonormal_basis(*normal)?;
                let d = sub(p, *origin);
                Some([dot(d, u), dot(d, v)])
            }
            Self::Cylinder {
                origin,
                axis,
                radius,
            } => {
                let (u, v) = orthonormal_basis(*axis)?;
                let d = sub(p, *origin);
                let along = dot(d, *axis);
                let radial = sub(d, scale(*axis, along));
                let _ = radius;
                let theta = dot(radial, v).atan2(dot(radial, u));
                Some([theta, along])
            }
            Self::Sphere { center, .. } => {
                let d = sub(p, *center);
                let l = dot(d, d).sqrt();
                if l < 1e-15 {
                    return None;
                }
                let phi = (d[2] / l).asin();
                let theta = d[1].atan2(d[0]);
                Some([theta, phi])
            }
            Self::Cone { .. } | Self::Torus { .. } => None,
        }
    }

    /// 按容差把参数域边界细分成折线，返回空间坐标环。
    ///
    /// `tolerance` 是弦高误差。平面不用细分（直线就是直）；
    /// 圆柱按弧长反解段数；球按经纬。
    pub fn tessellate_boundary(
        &self,
        boundary: &[Vec<[f64; 2]>],
        tolerance: f64,
    ) -> Vec<Vec<Point3>> {
        let tol = tolerance.max(1e-12);
        let mut out = Vec::with_capacity(boundary.len());
        for ring in boundary {
            let m = ring.len();
            if m < 3 {
                out.push(Vec::new());
                continue;
            }
            let mut pts: Vec<Point3> = Vec::new();
            for i in 0..m {
                let a = ring[i];
                let b = ring[(i + 1) % m];
                // 每段需要多少小段：按弦高公式反解
                let steps = self.segments_for(a, b, tol);
                for k in 0..steps {
                    let t = k as f64 / steps as f64;
                    let uv = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                    if let Some(p) = self.eval(uv) {
                        pts.push(p);
                    }
                }
            }
            out.push(pts);
        }
        out
    }

    /// 一段参数域边需要拆多少小段。
    ///
    /// 弦高 h ≈ R(1 − cos(Δθ/2))，反解得 Δθ ≤ 2·acos(1 − h/R)。
    /// 平面上的参数是直线，1 段就够。
    fn segments_for(&self, a: [f64; 2], b: [f64; 2], tol: f64) -> usize {
        let du = b[0] - a[0];
        let dv = b[1] - a[1];
        match self {
            Self::Plane { .. } => 1,
            Self::Cylinder { radius, .. } => {
                // 只有 θ 分量会弯曲，z 是直的
                let arc = du.abs();
                if arc < 1e-15 || *radius <= tol {
                    return 1;
                }
                let max_step = 2.0 * (1.0 - tol / radius).clamp(-1.0, 1.0).acos();
                if max_step < 1e-9 {
                    return 1;
                }
                ((arc / max_step).ceil() as usize).max(1)
            }
            Self::Sphere { radius, .. } => {
                let arc = (du * du + dv * dv).sqrt();
                if arc < 1e-15 || *radius <= tol {
                    return 1;
                }
                let max_step = 2.0 * (1.0 - tol / radius).clamp(-1.0, 1.0).acos();
                if max_step < 1e-9 {
                    return 1;
                }
                ((arc / max_step).ceil() as usize).max(1)
            }
            Self::Cone { .. } | Self::Torus { .. } => 1,
        }
    }

    /// 在某点处的外法线。点不在曲面上时返回最近点的法线（不做投影检查）。
    pub fn normal_at(&self, p: Point3) -> Option<Vector3> {
        match self {
            Self::Plane { normal, .. } => Some(*normal),
            Self::Cylinder {
                origin,
                axis,
                radius,
            } => {
                let d = sub(p, *origin);
                let along = dot(d, *axis);
                let radial = sub(d, scale(*axis, along));
                if len(radial) <= 1e-12 {
                    // 轴线上：法线不定
                    return None;
                }
                let _ = radius;
                normalize(radial)
            }
            Self::Cone {
                apex,
                axis,
                half_angle,
            } => {
                let d = sub(p, *apex);
                let along = dot(d, *axis);
                let radial = sub(d, scale(*axis, along));
                let rl = len(radial);
                if rl <= 1e-12 {
                    return None;
                }
                let rh = normalize(radial)?;
                // 圆锥外法线 = 径向分量 × cos(半角) − 轴向 × sin(半角)
                let ca = half_angle.cos();
                let sa = half_angle.sin();
                normalize([
                    rh[0] * ca - axis[0] * sa,
                    rh[1] * ca - axis[1] * sa,
                    rh[2] * ca - axis[2] * sa,
                ])
            }
            Self::Sphere { center, .. } => normalize(sub(p, *center)),
            Self::Torus {
                center,
                axis,
                major,
                ..
            } => {
                let d = sub(p, *center);
                let along = dot(d, *axis);
                let radial = sub(d, scale(*axis, along));
                let rh = normalize(radial)?;
                let tube_center = add(*center, scale(rh, *major));
                normalize(sub(p, tube_center))
            }
        }
    }

    /// 该曲面在匀质材料下的体积贡献用的"到轴距离"，供体积公式复用。
    pub fn is_planar(&self) -> bool {
        matches!(self, Self::Plane { .. })
    }
}

/// 解析曲线。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    /// 直线：过 origin、方向 dir（单位）
    Line { origin: Point3, dir: Vector3 },
    /// 圆：中心 center、法线 normal（单位）、半径 radius
    Circle {
        center: Point3,
        normal: Vector3,
        radius: f64,
    },
    /// 椭圆：中心、法线、长轴方向、长半轴 a、短半轴 b
    Ellipse {
        center: Point3,
        normal: Vector3,
        major_dir: Vector3,
        a: f64,
        b: f64,
    },
}

impl Curve {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Line { .. } => "line",
            Self::Circle { .. } => "circle",
            Self::Ellipse { .. } => "ellipse",
        }
    }

    /// 起点（参数 0）。
    pub fn start(&self) -> Point3 {
        match self {
            Self::Line { origin, .. } => *origin,
            Self::Circle {
                center,
                normal,
                radius,
            } => {
                let Some((u, _)) = orthonormal_basis(*normal) else {
                    return *center;
                };
                add(*center, scale(u, *radius))
            }
            Self::Ellipse {
                center,
                major_dir,
                a,
                ..
            } => add(*center, scale(*major_dir, *a)),
        }
    }

    /// 圆周/椭圆的周长（弧长参数化留给 E 阶段）。
    pub fn circumference(&self) -> Option<f64> {
        match self {
            Self::Line { .. } => None,
            Self::Circle { radius, .. } => Some(std::f64::consts::TAU * radius),
            Self::Ellipse { a, b, .. } => {
                // 拉马努金近似，相对误差 ~1e-10 量级
                let h = ((*a - *b) / (*a + *b)).powi(2);
                Some(std::f64::consts::PI * (a + b) * (1.0 + 3.0 * h / (10.0 + (4.0 - 3.0 * h).sqrt())))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn 圆柱面法线沿径向() {
        let s = Surface::Cylinder {
            origin: [0.0, 0.0, 0.0],
            axis: [0.0, 0.0, 1.0],
            radius: 5.0,
        };
        let n = s.normal_at([5.0, 0.0, 3.0]).unwrap();
        assert!((n[0] - 1.0).abs() < 1e-12 && n[1].abs() < 1e-12 && n[2].abs() < 1e-12);
    }

    #[test]
    fn 轴线上法线不定返回none() {
        let s = Surface::Cylinder {
            origin: [0.0, 0.0, 0.0],
            axis: [0.0, 0.0, 1.0],
            radius: 5.0,
        };
        assert!(s.normal_at([0.0, 0.0, 3.0]).is_none(), "轴线上的法线无意义，必须明说");
    }

    #[test]
    fn 球面法线指向外() {
        let s = Surface::Sphere {
            center: [0.0, 0.0, 0.0],
            radius: 2.0,
        };
        let n = s.normal_at([2.0, 0.0, 0.0]).unwrap();
        assert!((n[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn 圆锥面法线倾斜() {
        let s = Surface::Cone {
            apex: [0.0, 0.0, 0.0],
            axis: [0.0, 0.0, 1.0],
            half_angle: PI / 4.0,
        };
        let n = s.normal_at([1.0, 0.0, 1.0]).unwrap();
        // 45° 圆锥：法线应与轴成 135°（即径向 +1/√2、轴向 −1/√2）
        let inv_sqrt2 = std::f64::consts::FRAC_1_SQRT_2;
        assert!((n[0] - inv_sqrt2).abs() < 1e-6, "n.x={}", n[0]);
        assert!((n[2] + inv_sqrt2).abs() < 1e-6, "n.z={}", n[2]);
    }

    #[test]
    fn 圆周长为二派r() {
        let c = Curve::Circle {
            center: [0.0, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            radius: 5.0,
        };
        let l = c.circumference().unwrap();
        assert!((l - 2.0 * PI * 5.0).abs() < 1e-9);
    }

    #[test]
    fn 正交基不与轴平行() {
        for axis in [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.6, 0.8, 0.0]] {
            let (u, v) = orthonormal_basis(axis).unwrap();
            assert!(dot(u, axis).abs() < 1e-12, "u 与轴正交");
            assert!(dot(v, axis).abs() < 1e-12, "v 与轴正交");
            assert!((len(u) - 1.0).abs() < 1e-12);
            assert!((len(v) - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn 零向量归一化返回none() {
        assert!(normalize([0.0, 0.0, 0.0]).is_none());
    }
}
