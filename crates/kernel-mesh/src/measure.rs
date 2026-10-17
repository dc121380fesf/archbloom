//! 三角网格的几何测量：体积、面积、水密性。
//!
//! # 为什么独立成文件
//!
//! 这三样是 `SolidFacts` 的来源，也是契约测试的断言锚点。它们是纯函数，
//! 不碰内核状态，所以可以单独测透——将来 B-rep 后端算体积要用解析解，
//! 这里的散度定理版本正好当交叉验证的对照。
//!
//! # 口径
//!
//! 单位随输入（IR 用米）。体积对**闭合网格**才有意义；开放网格算出来的是
//! 「以原点为顶点、面片为底的锥体积之和」，没有物理含义，所以水密性要单独报。

use archcore_kernel::Point3;

/// 三角形网格的测量结果。
#[derive(Clone, Copy, Debug, Default)]
pub struct Measure {
    pub volume: f64,
    pub area: f64,
    pub watertight: bool,
    pub triangle_count: usize,
    pub vertex_count: usize,
}

fn sub(a: Point3, b: Point3) -> Point3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: Point3, b: Point3) -> Point3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: Point3, b: Point3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// 有符号体积（散度定理）：Σ a·(b×c)/6。
///
/// 返回**带符号**值——调用方要判断朝向时用得上；`volume_abs` 才是给人看的。
pub fn signed_volume(positions: &[Point3], indices: &[u32]) -> f64 {
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
    sum / 6.0
}

/// 表面积：Σ |(b−a)×(c−a)|/2。
pub fn surface_area(positions: &[Point3], indices: &[u32]) -> f64 {
    let mut sum = 0.0;
    for tri in indices.as_chunks::<3>().0 {
        let (Some(a), Some(b), Some(c)) = (
            positions.get(tri[0] as usize),
            positions.get(tri[1] as usize),
            positions.get(tri[2] as usize),
        ) else {
            continue;
        };
        sum += norm(cross(sub(*b, *a), sub(*c, *a))) / 2.0;
    }
    sum
}

fn norm(v: Point3) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// 是否水密：每条无向边恰好被两个三角形共享。
///
/// 用无向边（小索引在前）计数，避免同一三角形内三对方向各算一次。
/// 退化三角形（两个索引相同）直接判不水密——它们不构成合法面片。
pub fn is_watertight(indices: &[u32]) -> bool {
    use std::collections::HashMap;
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for tri in indices.as_chunks::<3>().0 {
        let (a, b, c) = (tri[0], tri[1], tri[2]);
        if a == b || b == c || a == c {
            return false;
        }
        for (x, y) in [(a, b), (b, c), (c, a)] {
            let key = if x < y { (x, y) } else { (y, x) };
            *edges.entry(key).or_insert(0) += 1;
        }
    }
    !edges.is_empty() && edges.values().all(|n| *n == 2)
}

/// 一次算全。
pub fn measure(positions: &[Point3], indices: &[u32]) -> Measure {
    Measure {
        volume: signed_volume(positions, indices).abs(),
        area: surface_area(positions, indices),
        watertight: is_watertight(indices),
        triangle_count: indices.len() / 3,
        vertex_count: positions.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单位立方体（边长 2，中心在原点→体积 8）。
    fn unit_cube() -> (Vec<Point3>, Vec<u32>) {
        let p = vec![
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ];
        let i = vec![
            // 底面 (-z)
            0, 2, 1, 0, 3, 2, // 顶面 (+z)
            4, 5, 6, 4, 6, 7, // 前面 (-y)
            0, 1, 5, 0, 5, 4, // 后面 (+y)
            3, 7, 6, 3, 6, 2, // 左面 (-x)
            0, 4, 7, 0, 7, 3, // 右面 (+x)
            1, 2, 6, 1, 6, 5,
        ];
        (p, i)
    }

    #[test]
    fn 立方体体积为八() {
        let (p, i) = unit_cube();
        let v = signed_volume(&p, &i).abs();
        assert!((v - 8.0).abs() < 1e-9, "期望 8.0，得到 {v}");
    }

    #[test]
    fn 立方体表面积为二十四() {
        let (p, i) = unit_cube();
        let a = surface_area(&p, &i);
        assert!((a - 24.0).abs() < 1e-9, "期望 24.0，得到 {a}");
    }

    #[test]
    fn 立方体水密() {
        let (_, i) = unit_cube();
        assert!(is_watertight(&i));
    }

    #[test]
    fn 少一个面就不水密() {
        let (_, mut i) = unit_cube();
        i.truncate(i.len() - 6); // 去掉右面
        assert!(!is_watertight(&i));
    }

    #[test]
    fn 退化三角形判不水密() {
        assert!(!is_watertight(&[0, 0, 1]));
    }

    #[test]
    fn 空网格不算水密() {
        assert!(!is_watertight(&[]));
    }

    #[test]
    fn 索引越界不panic() {
        let p = vec![[0.0, 0.0, 0.0]];
        let i = vec![0, 1, 2];
        // 越界三元组被跳过，不得 panic
        let _ = signed_volume(&p, &i);
        let _ = surface_area(&p, &i);
    }

    #[test]
    fn measure一次算全且口径一致() {
        let (p, i) = unit_cube();
        let m = measure(&p, &i);
        assert!((m.volume - 8.0).abs() < 1e-9);
        assert!((m.area - 24.0).abs() < 1e-9);
        assert!(m.watertight);
        assert_eq!(m.triangle_count, 12);
        assert_eq!(m.vertex_count, 8);
    }
}
