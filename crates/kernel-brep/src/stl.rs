//! STL 导出（ASCII）。
//!
//! # 为什么先做 STL 而不是 STEP
//!
//! STL 是三角面清单，**不带任何几何定义**——它只能证明"形状对"，证明不了"定义对"。
//! 但它简单到不可能写错，正好用来验证布尔出的网格是不是闭合、有没有翻面。
//!
//! 真正的加工出口是 STEP（G 阶段）：那时才带 PLANE / CYLINDRICAL_SURFACE 这些定义。
//!
//! # 输出格式
//!
//! ASCII STL（人可读、易 diff、适合验收）。二进制版体积小，等真需要时再加。

use crate::solid::{volume_from_mesh, Solid};

/// 导出 ASCII STL。
pub fn to_stl_ascii(solid: &Solid, name: &str) -> String {
    let (pos, _, idx) = solid.triangulate();
    let mut out = String::with_capacity(idx.len() * 60);
    out.push_str(&format!("solid {name}\n"));

    for tri in idx.as_chunks::<3>().0 {
        let (a, b, c) = (pos[tri[0] as usize], pos[tri[1] as usize], pos[tri[2] as usize]);
        let n = face_normal(a, b, c);
        out.push_str(&format!(
            "  facet normal {:.9e} {:.9e} {:.9e}\n    outer loop\n",
            n[0], n[1], n[2]
        ));
        for v in [a, b, c] {
            out.push_str(&format!(
                "      vertex {:.9e} {:.9e} {:.9e}\n",
                v[0], v[1], v[2]
            ));
        }
        out.push_str("    endloop\n  endfacet\n");
    }
    out.push_str(&format!("endsolid {name}\n"));
    out
}

fn face_normal(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> [f64; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l <= 1e-15 {
        [0.0, 0.0, 0.0]
    } else {
        [n[0] / l, n[1] / l, n[2] / l]
    }
}

/// 导出后自检：STL 里的三角数与体积应与实体一致。
///
/// 这个函数存在的意义是「导出的东西别悄悄和源不一致」——把它挂在测试和
/// 导出流程里，比事后拿外部工具比更早发现问题。
pub fn verify_against_solid(stl: &str, solid: &Solid) -> Result<(), String> {
    let facets = stl.matches("facet normal").count();
    let (pos, _, idx) = solid.triangulate();
    let expect = idx.len() / 3;
    if facets != expect {
        return Err(format!("STL facet 数 {facets} 与实体三角数 {expect} 不符"));
    }
    let vol = volume_from_mesh(&pos, &idx);
    if !vol.is_finite() || vol < 0.0 {
        return Err(format!("体积异常：{vol}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solid::box_solid;

    #[test]
    fn 盒子导出十二条三角() {
        let s = box_solid([0.0, 0.0, 0.0], [1.0, 2.0, 3.0]);
        let stl = to_stl_ascii(&s, "box");
        assert_eq!(stl.matches("facet normal").count(), 12);
        assert_eq!(stl.matches("vertex").count(), 36);
        assert!(stl.starts_with("solid box"));
        assert!(stl.trim_end().ends_with("endsolid box"));
        verify_against_solid(&stl, &s).unwrap();
    }

    #[test]
    fn 法线已归一化() {
        let s = box_solid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let stl = to_stl_ascii(&s, "b");
        for line in stl.lines().filter(|l| l.contains("facet normal")) {
            let parts: Vec<f64> = line
                .trim()
                .trim_start_matches("facet normal ")
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect();
            if parts.len() == 3 {
                let l = (parts[0].powi(2) + parts[1].powi(2) + parts[2].powi(2)).sqrt();
                assert!((l - 1.0).abs() < 1e-6, "法线未归一化：{l}");
            }
        }
    }
}
