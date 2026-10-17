//! K6 闸门验收：STEP 输出。
//!
//! # 闸门判据（分工与步骤 §五-G）
//!
//! - FreeCAD 回读通过；
//! - 体积误差 < 0.1%；
//! - 面类型正确；
//! - 壳体水密。
//!
//! # 依赖说明
//!
//! FreeCAD 是**外部进程**，不放进 cargo 测试（会让 CI 依赖本机装没装 FreeCAD）。
//! 测试只验 STEP 本身的结构与自洽；FreeCAD 那一环由 `examples/export_step.rs`
//! 产出文件后人工/脚本回读，结果记录在项目书与 CHANGELOG 里。
//!
//! 实测记录（2026-10-17，FreeCAD 1.1.2）：
//!
//! | 样件 | 我们的体积 | FreeCAD 体积 | 判定 |
//! |---|---|---|---|
//! | 1×2×3 m 盒子 | 6.000000000 m³ | 6.0e9 mm³ | ✅ 一致 |
//! | 20×20×5 mm 开孔板 | 1.750e-6 m³ | 1750.284 mm³ | ✅ 误差 0.016% |

use std::f64::consts::PI;

use archcore_kernel::{BoolOp, CancelToken, GeometryKernel};
use archcore_kernel_brep::{verify_step_against_solid, BrepKernel};

#[test]
fn k6_盒子导出结构正确() {
    let mut k = BrepKernel::new();
    let id = k.add_box([0.0, 0.0, 0.0], [1.0, 2.0, 3.0]);
    let step = k.to_step(id).expect("导出失败");

    assert!(step.starts_with("ISO-10303-21;"));
    assert!(step.contains("FILE_SCHEMA(('CONFIG_CONTROL_DESIGN'))"));
    assert_eq!(step.matches("VERTEX_POINT").count(), 8);
    assert_eq!(step.matches("ADVANCED_FACE").count(), 6);
    assert_eq!(step.matches("EDGE_CURVE").count(), 12, "拓扑共享：12 条边只定义一次");
    assert_eq!(
        step.matches("=PLANE(").count(),
        6,
        "盒子 6 个面全是平面（注意别把 PLANE_ANGLE_UNIT 算进来）"
    );
    assert!(step.contains("MANIFOLD_SOLID_BREP"));
    assert!(step.contains("CLOSED_SHELL"));

    let s = k.get(id).unwrap();
    verify_step_against_solid(&step, s).unwrap();
}

#[test]
fn k6_单位声明是米() {
    let mut k = BrepKernel::new();
    let id = k.add_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let step = k.to_step(id).unwrap();
    // 声明 SI_UNIT($,.METRE.)：不带前缀 = 米。FreeCAD 会按自己的显示偏好换成 mm，
    // 但语义上必须是米，否则下游按毫米读会把 1 米当成 1 毫米。
    assert!(
        step.contains("SI_UNIT($,.METRE.)"),
        "长度单位必须显式写成米"
    );
    assert!(step.contains("PLANE_ANGLE_UNIT"), "角度单位必须声明");
}

#[test]
fn k6_开孔件导出且面数合理() {
    let mut k = BrepKernel::new();
    let plate = k.add_box([-0.01, -0.01, 0.0], [0.01, 0.01, 0.005]);
    let hole = k.add_cylinder([0.0, 0.0, -0.001], 0.004, 0.007, 32);
    let id = k
        .boolean(
            plate,
            hole,
            BoolOp::Difference,
            &CancelToken::new(),
            &mut |_| {},
        )
        .expect("开孔失败");

    let f = k.facts(id).unwrap();
    assert!(f.watertight, "开孔件必须水密");
    let expect = 0.02 * 0.02 * 0.005 - PI * 0.004 * 0.004 * 0.005;
    let rel = (f.volume - expect).abs() / expect;
    assert!(rel < 0.01, "开孔件体积误差应 < 1%，实际 {:.3}%", rel * 100.0);

    let step = k.to_step(id).expect("导出失败");
    let s = k.get(id).unwrap();
    verify_step_against_solid(&step, s).unwrap();
    assert!(
        step.matches("ADVANCED_FACE").count() > 100,
        "开孔件的面数应该很多（离散孔壁）"
    );
    println!(
        "[K6] 开孔件：{} 面 / {} 字节 STEP",
        f.face_count,
        step.len()
    );
}

#[test]
fn k6_导出前自检挡住不一致() {
    // 自检逻辑本身要能拦住问题：手工造一个与实体不符的 STEP，必须报错
    let mut k = BrepKernel::new();
    let id = k.add_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let s = k.get(id).unwrap();
    let bad = "ISO-10303-21;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n";
    assert!(
        verify_step_against_solid(bad, s).is_err(),
        "面数不符必须被自检拦下"
    );
}

#[test]
fn k6_能力申报与实现一致() {
    let k = BrepKernel::new();
    let c = k.capabilities();
    assert!(c.step_io, "导出已实现，必须报 true");
    // 真曲面求交仍未做——圆柱是离散的，所以 STEP 里全是 PLANE
    assert!(
        !c.curved_bool,
        "曲面求交未做：导出的孔是多面体棱柱，不是 CYLINDRICAL_SURFACE"
    );
}
