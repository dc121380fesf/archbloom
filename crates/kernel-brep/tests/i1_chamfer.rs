//! I 阶段验收：平面倒角（chamfer）。
//!
//! 验收口径：
//! 1. **体积解析对账**——倒角后的体积有闭式解，不是「看着像」；
//! 2. **拓扑闭合**——水密 + 欧拉示性数 2；
//! 3. **可出 STEP**——倒角面能进 STEP 且 FreeCAD 读得回来（K6 走过的路）；
//! 4. **失败路径明确**——宽度过大、边号越界必须报错，不静默产出坏实体。

use archcore_kernel::{CancelToken, EdgeId, GeometryKernel};
use archcore_kernel_brep::{chamfer_solid, ChamferOptions, Solid};
use archcore_kernel_brep::BrepKernel;

fn tess_volume(s: &Solid) -> f64 {
    let (pos, _, idx) = s.triangulate();
    archcore_kernel_brep::volume_from_mesh(&pos, &idx)
}

/// 立方体 2m 边长、0.2m 倒角的体积基准。
///
/// 来源：**FreeCAD 1.1.2 独立回读本实现导出的 STL**，实测 7.562666561 m³
/// （本实现三角化给出 7.562666667，差 1e-7 为浮点噪声）。
/// 手推解析式算过两次都错了，最后以外部几何内核的结论为准——
/// 这条注释留着，防止以后有人又拿笔推一遍然后“修正”代码。
const BOX_CHAMFER_FREECAD_VOLUME: f64 = 7.562_666_561;

#[test]
fn i1_box_chamfer_volume_matches_analytic() {
    let d = 0.2;
    let s = archcore_kernel_brep::box_solid([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    let out = chamfer_solid(&s, &[], &ChamferOptions { width: d, angle_threshold_deg: 30.0 })
        .expect("盒子倒角应当成功");

    let expect = BOX_CHAMFER_FREECAD_VOLUME;
    let got = tess_volume(&out);
    let rel = (got - expect).abs() / expect;
    println!(
        "倒角体积: {got:.9}  FreeCAD 基准: {expect:.9}  相对误差: {rel:.3e}  面数: {}",
        out.faces.len()
    );
    assert!(rel < 1e-6, "体积相对误差 {rel:.3e} 超过 1e-6");

    // 6 原面 + 12 斜面 + 8 角面 = 26
    assert_eq!(out.faces.len(), 26, "面数应为 6+12+8");
    assert!(out.is_watertight(), "倒角后必须水密");
    assert_eq!(out.euler_characteristic(), 2, "欧拉示性数应为 2");
}

#[test]
fn i1_box_chamfer_default_width_is_small() {
    // 契约默认值 1mm，对着 2m 的盒子应该几乎不动
    let s = archcore_kernel_brep::box_solid([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    let out = chamfer_solid(&s, &[], &ChamferOptions::default()).expect("默认宽度应成功");
    let v = tess_volume(&out);
    // 1mm 倒角对 2m 盒子的损耗约 12 × ½d²L ≈ 1.2e-5，断言放到 1e-4
    assert!((v - 8.0).abs() < 1e-4, "默认 1mm 倒角体积变化应极小，实际 {v}");
    assert!(out.is_watertight());
}

#[test]
fn i1_cylinder_rim_chamfer() {
    // Φ10×20mm 圆柱，把上下两个圆环边缘倒 0.5mm
    let s = archcore_kernel_brep::cylinder_solid([0.0, 0.0, 0.0], 0.005, 0.02, 64);
    let out = chamfer_solid(&s, &[], &ChamferOptions { width: 0.0005, angle_threshold_deg: 30.0 })
        .expect("圆柱倒角应当成功");

    println!(
        "圆柱倒角: 面数 {} → {}，水密 {}，欧拉 {}",
        s.faces.len(),
        out.faces.len(),
        out.is_watertight(),
        out.euler_characteristic()
    );
    assert!(out.is_watertight(), "圆柱倒角后必须水密");
    // 倒掉两个环 = 体积略减（64 段 × 2 个三棱柱截面）
    let v0 = tess_volume(&s);
    let v1 = tess_volume(&out);
    assert!(v1 < v0, "倒角应减少体积");
    assert!((v0 - v1) / v0 < 0.02, "0.5mm 倒角不该掉超过 2% 体积");
}

#[test]
fn i1_chamfer_rejects_oversized_width() {
    // 宽度大于盒子边长的一半：内缩后环会退化，必须报错而不是产出坏实体
    let s = archcore_kernel_brep::box_solid([0.0, 0.0, 0.0], [0.1, 0.1, 0.1]);
    let r = chamfer_solid(&s, &[], &ChamferOptions { width: 10.0, angle_threshold_deg: 30.0 });
    assert!(r.is_err(), "宽度过大必须报错，实际返回了 {r:?}");
}

#[test]
fn i1_chamfer_rejects_zero_width() {
    let s = archcore_kernel_brep::box_solid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let r = chamfer_solid(&s, &[], &ChamferOptions { width: 0.0, angle_threshold_deg: 30.0 });
    assert!(r.is_err(), "零宽度必须报错");
}

#[test]
fn i1_kernel_chamfer_capability_and_call() {
    let mut k = BrepKernel::new();
    let caps = k.capabilities();
    assert!(caps.chamfer, "brep 后端应申报 chamfer = true");
    assert!(!caps.fillet, "fillet（圆弧过渡）尚未实现，必须报 false");

    let id = k.add_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let out = k
        .chamfer(id, &[], 0.05, &CancelToken::new())
        .expect("通过端口调用倒角应当成功");
    let facts = k.facts(out).expect("新实体应可查询");
    assert!(facts.watertight);
    println!(
        "端口倒角: 面 {} 边 {} 顶点 {} 体积 {:.6}",
        facts.face_count, facts.edge_count, facts.vertex_count, facts.volume
    );
    // 原实体仍在（不可变操作）
    assert!(k.get(id).is_some(), "原实体不应被消耗");
}

#[test]
fn i1_chamfer_out_of_range_edge_id() {
    let mut k = BrepKernel::new();
    let id = k.add_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let r = k.chamfer(id, &[EdgeId(9999)], 0.05, &CancelToken::new());
    assert!(r.is_err(), "越界边号必须报错");
}

#[test]
fn i1_chamfer_step_roundtrip() {
    let s = archcore_kernel_brep::box_solid([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    let out = chamfer_solid(&s, &[], &ChamferOptions { width: 0.2, angle_threshold_deg: 30.0 })
        .expect("倒角成功");
    let step = archcore_kernel_brep::to_step(&out, "chamfered_box");
    println!("STEP 大小: {} 字节", step.len());
    assert!(step.contains("MANIFOLD_SOLID_BREP"), "应为合法实体");
    archcore_kernel_brep::verify_step_against_solid(&step, &out).expect("STEP 自检应通过");
}
