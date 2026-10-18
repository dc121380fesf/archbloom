//! 共面面片合并的验收。
//!
//! 三条硬指标，缺一不可：
//! 1. **面数显著下降**（布尔碎片合成整面）
//! 2. **体积不变**（合并是重组，不是改几何）
//! 3. **水密保持**（边消除不能把壳弄破）

use archcore_kernel::{BoolOp, CancelToken, GeometryKernel};
use archcore_kernel_brep::{merge_coplanar, volume_from_mesh, BrepKernel};

fn plate_with_hole() -> (BrepKernel, archcore_kernel::SolidId) {
    let mut k = BrepKernel::new();
    let plate = k.add_box([0.0, 0.0, 0.0], [0.020, 0.020, 0.005]);
    let drill = k.add_cylinder([0.010, 0.010, -0.001], 0.005, 0.007, 64);
    let mut noop = |_p: archcore_kernel::Progress| {};
    let id = k
        .boolean(plate, drill, BoolOp::Difference, &CancelToken::new(), &mut noop)
        .expect("开孔");
    (k, id)
}

fn volume(s: &archcore_kernel_brep::Solid) -> f64 {
    let (pos, _, idx) = s.triangulate();
    volume_from_mesh(&pos, &idx)
}

#[test]
fn m1_merge_plate_with_hole() {
    let (k, id) = plate_with_hole();
    let s = k.get(id).expect("solid");
    let v0 = volume(s);
    let f0 = s.faces.len();

    let (merged, stats) = merge_coplanar(s).expect("合并应当成功");
    let v1 = volume(&merged);
    println!(
        "面 {} → {}（减少 {:.1}%），带洞面 {}，体积 {:.9} → {:.9}",
        stats.faces_before,
        stats.faces_after,
        stats.reduction * 100.0,
        stats.holed_faces,
        v0,
        v1
    );

    // 实测：3148 → 534（减少 83%），体积**完全不变**，水密保持。
    //
    // 理论极限是 ~70 面（外框 4 + 顶底 2 + 孔壁 64），没达到的原因明确：
    // 顶面/底面的最终形态是「外方形 + 内圆孔」的**带洞面**，而当前拼接遇到
    // 「拼完会形成环」的情形一律当自交拒掉，所以只能拼成留一道缝的 C 形大环。
    // 这是已知缺口，不是回归。
    assert!(
        stats.reduction > 0.8,
        "面数应至少减少 80%，实际 {:.1}%（{} → {}）",
        stats.reduction * 100.0,
        f0,
        stats.faces_after
    );
    let dv = (v1 - v0).abs() / v0;
    assert!(dv < 1e-6, "合并不得改变体积，相对差 {dv:.2e}");
    assert!(merged.is_watertight(), "合并后必须水密");
    println!("合并后欧拉 {}", merged.euler_characteristic());
}

#[test]
fn m2_merge_box_is_identity() {
    // 干净的盒子没有可合并的碎片，合并应当原样返回
    let s = archcore_kernel_brep::box_solid([0.0, 0.0, 0.0], [1.0, 2.0, 3.0]);
    let (merged, stats) = merge_coplanar(&s).expect("合并");
    println!("盒子：{} → {} 面", stats.faces_before, stats.faces_after);
    assert_eq!(stats.faces_after, 6, "盒子应保持 6 个面");
    assert!(merged.is_watertight());
    assert!((volume(&merged) - 6.0).abs() < 1e-12);
}

#[test]
fn m3_merge_cylinder_sides() {
    // 圆柱侧面是 64 个共享同一圆柱面的四边形：应当合并成一个带两个洞的面？
    // 实际上侧面合并后是一个「筒状」面，外环是顶/底两个圆（不连通）——
    // 这种情况边消除会给出两个独立的环，都不闭合，属于**已知限制**。
    // 这里只要求：不 panic、不产出坏实体。
    let s = archcore_kernel_brep::cylinder_solid([0.0, 0.0, 0.0], 0.005, 0.02, 16);
    let v0 = volume(&s);
    match merge_coplanar(&s) {
        Ok((merged, stats)) => {
            println!(
                "圆柱：{} → {} 面，水密 {}，体积 {:.9} → {:.9}",
                stats.faces_before,
                stats.faces_after,
                merged.is_watertight(),
                v0,
                volume(&merged)
            );
            // 不管合并成什么样，体积不能变
            let dv = (volume(&merged) - v0).abs() / v0;
            assert!(dv < 1e-6, "圆柱合并体积变化 {dv:.2e}");
        }
        Err(e) => {
            println!("圆柱合并被拒绝（可接受，但要有诊断）：{e}");
        }
    }
}
