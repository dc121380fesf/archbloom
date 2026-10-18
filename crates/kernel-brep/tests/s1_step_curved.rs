//! STEP 真曲面的验收（I 阶段第四档）。
//!
//! 核心断言：圆柱侧面必须写成 `CYLINDRICAL_SURFACE` + `CIRCLE` 边界，
//! 而不是 16 个平面拼的棱柱。加工端只有看到 `Cylinder` 才知道那是一个孔。
//!
//! 结构对齐的参考物：FreeCAD（OCC 内核）导出的同尺寸圆柱 STEP。
//! 它的结构是 **3 面 / 3 边（2 Circle + 1 Line）**，我们与之保持一致。

use archcore_kernel_brep::{cylinder_solid, to_step, to_step_curved};

#[test]
fn s1_cylinder_written_as_true_surface() {
    let s = cylinder_solid([0.0, 0.0, 0.0], 0.005, 0.02, 16);
    let step = to_step_curved(&s, "cyl");

    let cyl_surfaces = step.matches("CYLINDRICAL_SURFACE").count();
    let circles = step.matches("CIRCLE(''").count();
    let faces = step.matches("ADVANCED_FACE").count();

    println!("面 {faces} | CYLINDRICAL {cyl_surfaces} | CIRCLE {circles}");
    assert_eq!(cyl_surfaces, 1, "圆柱侧面应写成一个 CYLINDRICAL_SURFACE");
    assert_eq!(circles, 2, "应有顶/底两个整圆");
    assert_eq!(faces, 3, "结构应与 OCC 一致：1 圆柱 + 2 平面");

    // 整圆是**闭合边**：起点终点是同一个 VERTEX_POINT
    assert!(
        step.contains("EDGE_CURVE('',#") && step.matches(".T.);").count() > 0,
        "应有闭合边"
    );
    // 不应出现折线近似的痕迹
    assert!(
        !step.contains("TRIMMED_CURVE"),
        "整圆不需要 TRIMMED_CURVE"
    );
}

#[test]
fn s1_stable_path_unchanged() {
    // 稳定路径必须保持全平面（加工端已验证过的那份）
    let s = cylinder_solid([0.0, 0.0, 0.0], 0.005, 0.02, 16);
    let stable = to_step(&s, "cyl");
    assert_eq!(
        stable.matches("CYLINDRICAL_SURFACE").count(),
        0,
        "稳定路径不应出现曲面"
    );
    assert!(stable.matches("ADVANCED_FACE").count() > 3);
}

#[test]
fn s1_box_has_no_cylinders() {
    // 盒子没有圆柱面，两条路径应给出一致结果
    let s = archcore_kernel_brep::box_solid([0.0, 0.0, 0.0], [1.0, 2.0, 3.0]);
    let a = to_step(&s, "b");
    let b = to_step_curved(&s, "b");
    assert_eq!(a.matches("ADVANCED_FACE").count(), 6);
    assert_eq!(b.matches("ADVANCED_FACE").count(), 6);
    assert!(!b.contains("CYLINDRICAL_SURFACE"));
}
