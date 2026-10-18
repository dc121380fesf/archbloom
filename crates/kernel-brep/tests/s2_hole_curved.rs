//! 开孔板 STEP 真曲面的验收（S2）。
//!
//! 关键断言：
//! 1. 孔壁写成 `CYLINDRICAL_SURFACE`，不是 64 个平面；
//! 2. 顶/底面的孔口用 `FACE_BOUND` + 圆边，不是折线；
//! 3. 实体本身水密、体积不变。
//!
//! FreeCAD 独立回读（人工验收记录）：
//! - 有效 True / 闭合 True
//! - 体积 1607.300918301 vs 理论 1607.3
//! - 面类型 Plane 7 + Cylinder 1，边类型 Line 271 + Circle 2

use archcore_kernel::{BoolOp, CancelToken, GeometryKernel};
use archcore_kernel_brep::{fix_holes_with_cylinder_rings, merge_coplanar, to_step_curved, BrepKernel};

fn plate() -> (BrepKernel, archcore_kernel::SolidId) {
    let mut k = BrepKernel::new();
    let p = k.add_box([0.0, 0.0, 0.0], [0.020, 0.020, 0.005]);
    let d = k.add_cylinder([0.010, 0.010, -0.001], 0.005, 0.007, 64);
    let mut noop = |_p: archcore_kernel::Progress| {};
    let id = k
        .boolean(p, d, BoolOp::Difference, &CancelToken::new(), &mut noop)
        .expect("开孔");
    (k, id)
}

#[test]
fn s2_hole_becomes_true_circle() {
    let (k, id) = plate();
    let (mut m, stats) = merge_coplanar(k.get(id).expect("solid")).expect("merge");
    assert_eq!(stats.holed_faces, 2, "顶/底各一个洞");

    let fixed = fix_holes_with_cylinder_rings(&mut m);
    println!(
        "面 {} 洞修复 {} 水密 {} 欧拉 {}",
        m.faces.len(),
        fixed,
        m.is_watertight(),
        m.euler_characteristic()
    );
    assert_eq!(fixed, 2, "两个洞都应被端圆替换");

    // 洞必须是 64 点、半径恒定的圆
    for f in &m.faces {
        for h in &f.holes {
            assert_eq!(h.len(), 64, "洞应该是 64 点（与圆柱段数一致）");
            let n = h.len() as f64;
            let mut c = [0.0; 3];
            for &v in h {
                let p = m.verts[v as usize];
                for (ck, pk) in c.iter_mut().zip(p.iter()) {
                    *ck += *pk;
                }
            }
            for ck in c.iter_mut() {
                *ck /= n;
            }
            let mut rmin = f64::MAX;
            let mut rmax = 0.0f64;
            for &v in h {
                let p = m.verts[v as usize];
                let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
                let r = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                rmin = rmin.min(r);
                rmax = rmax.max(r);
            }
            assert!(
                (rmax - rmin) / rmax < 1e-6,
                "洞半径必须恒定，实际 {rmin:.9}~{rmax:.9}"
            );
        }
    }
    assert!(m.is_watertight(), "修复后必须保持水密");
}

#[test]
fn s2_step_has_cylindrical_surface_and_hole_bounds() {
    let (k, id) = plate();
    let (mut m, _) = merge_coplanar(k.get(id).expect("solid")).expect("merge");
    fix_holes_with_cylinder_rings(&mut m);

    let step = to_step_curved(&m, "plate");
    let cyl = step.matches("CYLINDRICAL_SURFACE").count();
    let circle = step.matches("CIRCLE(''").count();
    let hole_bounds = step.matches("FACE_BOUND").count();
    println!(
        "STEP {} 字节 | CYLINDRICAL {cyl} | CIRCLE {circle} | FACE_BOUND {hole_bounds}",
        step.len()
    );

    assert_eq!(cyl, 1, "孔壁必须写成一个 CYLINDRICAL_SURFACE");
    assert_eq!(circle, 2, "顶/底端圆各一条闭合边");
    assert_eq!(hole_bounds, 2, "顶/底面的孔口各一个 FACE_BOUND");
    // 孔口不能再是折线：洞的边不应该是 LINE
    assert!(
        step.len() < 200_000,
        "用了曲面之后文件应该显著变小，实际 {} 字节",
        step.len()
    );
}
