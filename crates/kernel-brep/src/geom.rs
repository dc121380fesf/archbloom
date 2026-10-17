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
