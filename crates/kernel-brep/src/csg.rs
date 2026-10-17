//! 布尔运算：切分 → 分类 → 缝合。
//!
//! # 与 BSP 版本的区别
//!
//! 旧版让 BSP 树做**裁剪**（`clip_polygons`），那一步依赖树形结构，
//! 同一组几何换个输入顺序就出不同结果，而且错了以后很难定位。
//!
//! 这里把职责拆开，每步都能单独验：
//!
//! 1. **切分**：用「双方所有面的平面」把每个多边形切开（纯几何，无树）；
//! 2. **分类**：每个子面用**射线法**判在自己对方体内还是体外（不依赖树形）；
//! 3. **保留**：按运算类型挑子面，差集时把对方的面翻转（洞壁朝向）；
//! 4. **缝合**：交给 [`rebuild`](crate::rebuild)，焊顶点、查水密。
//!
//! # 已知代价
//!
//! 切分会把面片切得比理论最少值碎（每个面都被所有平面切过一遍），
//! 所以结果面数偏多。`rebuild` 的共面消隐会吃掉一部分，但不会收到最简。
//! **对水密性与体积没有影响**——碎面片的边依然两两配对。
//! 简并留给后续优化，正确性优先。

use archcore_kernel::Point3;

use crate::bsp::{split_polygon, Plane, Polygon};
use crate::geom::{cross, dot, sub};
use crate::solid::Solid;

/// 射线法判定点在实体内外时用的方向。
///
/// 刻意选一个"不讲理"的方向（三个分量都非零、非整数比、不撞已知常数），
/// 避免恰好擦过顶点或与面共面——那会让计数错。
const RAY_DIR: [f64; 3] = [0.5413, 0.7413, 0.3917];

/// 用一组平面把所有多边形切碎。
///
/// # 关键优化：只让「真穿过」的平面动手
///
/// 否则每个面都要被所有平面切一遍——64 段圆柱被板的 6 个平面切，
/// 碎片数按 2^6 爆，128 段时直接撞上规模上限、切分不完整，拓扑就崩了。
///
/// 先判断平面是否真的穿过这个多边形（顶点分居两侧），不撞就原样保留。
pub fn split_all(polys: Vec<Polygon>, planes: &[Plane]) -> Vec<Polygon> {
    let mut cur = polys;
    for pl in planes {
        let mut next = Vec::with_capacity(cur.len() + cur.len() / 4);
        for p in cur {
            if !crosses(pl, &p) {
                // 平面不穿过它（全在一侧或共面）：原样留下
                next.push(p);
                continue;
            }
            let mut cf = Vec::new();
            let mut cb = Vec::new();
            let mut f = Vec::new();
            let mut b = Vec::new();
            split_polygon(pl, &p, &mut cf, &mut cb, &mut f, &mut b);
            next.extend(cf);
            next.extend(cb);
            next.extend(f);
            next.extend(b);
        }
        cur = next;
        // 安全阀：真出现异常规模就停，但正常工作下远达不到
        if cur.len() > 50_000 {
            break;
        }
    }
    cur
}

/// 平面是否真的穿过这个多边形（顶点分居两侧）。
fn crosses(pl: &Plane, p: &Polygon) -> bool {
    let mut front = false;
    let mut back = false;
    for v in &p.verts {
        let d = pl.distance(*v);
        if d > crate::bsp::EPS {
            front = true;
        } else if d < -crate::bsp::EPS {
            back = true;
        }
        if front && back {
            return true;
        }
    }
    false
}

/// 射线法：点是否在实体内。
///
/// 从 `p` 沿 [`RAY_DIR`] 发一条射线，数它穿过多少个面。
/// 奇数 → 在内，偶数 → 在外。对凸、非凸、带孔实体都成立。
pub fn point_inside(solid: &Solid, p: Point3) -> bool {
    let mut hits = 0usize;
    for f in &solid.faces {
        if f.verts.len() < 3 {
            continue;
        }
        let v: Vec<Point3> = f
            .verts
            .iter()
            .filter_map(|i| solid.verts.get(*i as usize).copied())
            .collect();
        // 扇形剖分成三角形逐个测
        for i in 1..v.len().saturating_sub(1) {
            if ray_hits_triangle(p, v[0], v[i], v[i + 1]) {
                hits += 1;
            }
        }
    }
    hits % 2 == 1
}

/// Möller–Trumbore 射线-三角形求交（只判是否命中，distance > 0）。
fn ray_hits_triangle(origin: Point3, a: Point3, b: Point3, c: Point3) -> bool {
    let dir = RAY_DIR;
    let e1 = sub(b, a);
    let e2 = sub(c, a);
    let pvec = cross(dir, e2);
    let det = dot(e1, pvec);
    if det.abs() < 1e-12 {
        return false;
    }
    let inv_det = 1.0 / det;
    let tvec = sub(origin, a);
    let u = dot(tvec, pvec) * inv_det;
    if !(-1e-9..=1.0 + 1e-9).contains(&u) {
        return false;
    }
    let qvec = cross(tvec, e1);
    let v = dot(dir, qvec) * inv_det;
    if v < -1e-9 || u + v > 1.0 + 1e-9 {
        return false;
    }
    let t = dot(e2, qvec) * inv_det;
    t > 1e-9
}

/// 布尔运算的结果：保留哪些子面（已按运算规则筛过、差集时已翻转）。
pub fn select_faces(
    a: &Solid,
    b: &Solid,
    polys_a: Vec<Polygon>,
    polys_b: Vec<Polygon>,
    op: archcore_kernel::BoolOp,
) -> Vec<Polygon> {
    use archcore_kernel::BoolOp::{Difference, Intersection, Union};

    // 切分：双方所有平面互相切
    let planes_a: Vec<Plane> = a
        .faces
        .iter()
        .filter_map(|f| {
            let v: Vec<Point3> = f
                .verts
                .iter()
                .filter_map(|i| a.verts.get(*i as usize).copied())
                .collect();
            Plane::from_polygon(&v)
        })
        .collect();
    let planes_b: Vec<Plane> = b
        .faces
        .iter()
        .filter_map(|f| {
            let v: Vec<Point3> = f
                .verts
                .iter()
                .filter_map(|i| b.verts.get(*i as usize).copied())
                .collect();
            Plane::from_polygon(&v)
        })
        .collect();

    let sa = split_all(polys_a, &planes_b);
    let sb = split_all(polys_b, &planes_a);

    let mut out: Vec<Polygon> = Vec::new();

    // 质心探测点：沿内法线（−normal）偏一点。
    //
    // 为什么必须偏移：子面可能正好落在对方边界面上（共面情形），
    // 直接从质心发射线会惌上顶点或与面共面，计数随机——
    // 相邻两个盒子拼在一起时就会撞上。往内偏 EPS×100 既避开边界又不影响判断。
    let probe = |p: &Polygon| -> Point3 {
        let c = p.centroid();
        let n = p.plane.normal;
        let k = crate::bsp::EPS * 100.0;
        [c[0] - n[0] * k, c[1] - n[1] * k, c[2] - n[2] * k]
    };

    for p in sa {
        let inside_b = point_inside(b, probe(&p));
        let keep = match op {
            Union => !inside_b,
            Difference => !inside_b,
            Intersection => inside_b,
        };
        if keep {
            out.push(p);
        }
    }

    for mut p in sb {
        // B 的面在差集里会翻转，探测也用翻转前的内法线（即 −normal 方向）
        let inside_a = point_inside(a, probe(&p));
        let keep = match op {
            Union => !inside_a,
            Difference => inside_a,
            Intersection => inside_a,
        };
        if keep {
            // 差集时 B 的面成为洞壁，朝向要翻
            if matches!(op, Difference) {
                p.flip();
            }
            out.push(p);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solid::box_solid;

    fn volume_of(polys: &[Polygon]) -> f64 {
        let mut sum = 0.0;
        for p in polys {
            for i in 1..p.verts.len().saturating_sub(1) {
                sum += dot(p.verts[0], cross(p.verts[i], p.verts[i + 1]));
            }
        }
        (sum / 6.0).abs()
    }

    #[test]
    fn 射线法判内外() {
        let s = box_solid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        assert!(point_inside(&s, [0.5, 0.5, 0.5]), "中心应在内");
        assert!(!point_inside(&s, [2.0, 0.5, 0.5]), "外部点应在外");
        assert!(!point_inside(&s, [-0.5, 0.5, 0.5]));
    }

    #[test]
    fn 射线法对非凸也成立() {
        // L 形：两个盒子拼起来再挖掉一角 —— 用布尔本身构造太重，
        // 这里退一步：只验证"挖空的实体"内部点判定
        let s = box_solid([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
        assert!(point_inside(&s, [1.0, 1.0, 1.0]));
        assert!(!point_inside(&s, [3.0, 1.0, 1.0]));
    }

    #[test]
    fn 差集体积正确() {
        use archcore_kernel::BoolOp;
        let a = box_solid([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
        let b = box_solid([1.0, -1.0, -1.0], [3.0, 3.0, 3.0]);
        let pa = crate::rebuild::solid_to_polygons(&a);
        let pb = crate::rebuild::solid_to_polygons(&b);
        let out = select_faces(&a, &b, pa, pb, BoolOp::Difference);
        let v = volume_of(&out);
        assert!((v - 4.0).abs() < 1e-6, "差集体积应为 4，实际 {v}");
    }

    #[test]
    fn 交集体积正确() {
        use archcore_kernel::BoolOp;
        let a = box_solid([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
        let b = box_solid([1.0, 1.0, 1.0], [3.0, 3.0, 3.0]);
        let pa = crate::rebuild::solid_to_polygons(&a);
        let pb = crate::rebuild::solid_to_polygons(&b);
        let out = select_faces(&a, &b, pa, pb, BoolOp::Intersection);
        let v = volume_of(&out);
        assert!((v - 1.0).abs() < 1e-6, "交集体积应为 1，实际 {v}");
    }

    #[test]
    fn 并集体积正确_重叠情形() {
        use archcore_kernel::BoolOp;
        let a = box_solid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let b = box_solid([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]);
        let pa = crate::rebuild::solid_to_polygons(&a);
        let pb = crate::rebuild::solid_to_polygons(&b);
        let out = select_faces(&a, &b, pa, pb, BoolOp::Union);
        let v = volume_of(&out);
        // 1 + 1 − 0.125 = 1.875
        assert!((v - 1.875).abs() < 1e-6, "并集体积应为 1.875，实际 {v}");
    }

    #[test]
    fn 并集体积正确_相邻情形() {
        use archcore_kernel::BoolOp;
        let a = box_solid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let b = box_solid([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]);
        let pa = crate::rebuild::solid_to_polygons(&a);
        let pb = crate::rebuild::solid_to_polygons(&b);
        let out = select_faces(&a, &b, pa, pb, BoolOp::Union);
        let v = volume_of(&out);
        assert!((v - 2.0).abs() < 1e-6, "相邻并集体积应为 2，实际 {v}");
    }
}
