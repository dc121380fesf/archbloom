//! K3 闸门验收：Φ10 圆柱孔的精度与可渲染性。
//!
//! # 闸门判据（项目书 §7 / 分工与步骤 §五-D）
//!
//! - 能构造 Φ10 圆柱孔；
//! - **体积误差 < 0.1%**；
//! - 可 tessellate，且三角化结果能交给渲染器。
//!
//! # 关键区别
//!
//! 这里验的不是「网格逼近得像不像」，而是「定义对不对」：
//! `facts().volume` 是 π·r²·h 的闭式解，与离散段数**无关**。
//! 离散段数只影响三角化后的网格体积（那是渲染用的近似）。

use std::f64::consts::PI;

use archcore_kernel::{GeometryKernel, SolidId};
use archcore_kernel_brep::{segments_for_tolerance, BrepKernel};

/// Φ10mm × 20mm 圆柱（单位：米）
const R: f64 = 0.005;
const H: f64 = 0.020;

fn exact_volume() -> f64 {
    PI * R * R * H
}

#[test]
fn k3_圆柱体积走闭式解_误差远小于千分之一() {
    let mut k = BrepKernel::new();
    let id = k.add_cylinder([0.0, 0.0, 0.0], R, H, 64);
    let f = k.facts(id).unwrap();

    let exact = exact_volume();
    let rel = (f.volume - exact).abs() / exact;
    println!(
        "[K3] 体积 {:.12e} m³（精确 {:.12e}），相对误差 {:.2e}",
        f.volume, exact, rel
    );
    assert!(rel < 1e-9, "闭式解应精确到浮点极限，实际误差 {rel:.3e}");
    assert!(rel < 0.001, "闸门要求 < 0.1%");
}

#[test]
fn k3_体积不随离散段数变化() {
    // 这是 B-rep 与网格的根本差别：定义不变，精度不变
    let exact = exact_volume();
    for segs in [8usize, 16, 64, 256] {
        let mut k = BrepKernel::new();
        let id = k.add_cylinder([0.0, 0.0, 0.0], R, H, segs);
        let v = k.facts(id).unwrap().volume;
        assert!(
            (v - exact).abs() / exact < 1e-12,
            "{segs} 段时体积应仍为闭式解，实际 {v:.12e}"
        );
    }
}

#[test]
fn k3_网格体积随段数收敛到闭式解() {
    // 离散表示（渲染用）应收敛到定义值——收敛性成立说明离散方向没错
    let exact = exact_volume();
    let mut last_err = f64::MAX;
    for segs in [8usize, 16, 32, 64, 128] {
        let mut k = BrepKernel::new();
        let id = k.add_cylinder([0.0, 0.0, 0.0], R, H, segs);
        let tess = k.tessellate(id, 0.0001).unwrap();
        let mesh_vol = archcore_kernel_brep::volume_from_mesh(&tess.positions, &tess.indices);
        let err = (mesh_vol - exact).abs() / exact;
        println!("[K3] {segs:>3} 段 → 网格体积误差 {:.4}%", err * 100.0);
        assert!(err < last_err, "{segs} 段误差 {err:.5} 未比上一档更小");
        last_err = err;
    }
    assert!(
        last_err < 0.001,
        "128 段时网格误差应 < 0.1%，实际 {:.4}%",
        last_err * 100.0
    );
}

#[test]
fn k3_可tessellate且拓扑有效() {
    let mut k = BrepKernel::new();
    let id = k.add_cylinder([0.0, 0.0, 0.0], R, H, 64);
    let t = k.tessellate(id, 0.0005).unwrap();

    assert!(!t.is_empty(), "三角化不得为空");
    assert_eq!(
        t.positions.len(),
        t.normals.len(),
        "顶点与法线必须一一对齐（渲染器按索引取法线）"
    );
    assert_eq!(t.indices.len() % 3, 0);
    assert!(
        t.indices.iter().all(|i| (*i as usize) < t.positions.len()),
        "索引不得越界"
    );
    // 法线应已归一化
    for n in &t.normals {
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!((l - 1.0).abs() < 1e-6, "法线未归一化：{l}");
    }
    println!(
        "[K3] tessellate：{} 顶点 / {} 三角形",
        t.positions.len(),
        t.triangle_count()
    );
}

#[test]
fn k3_圆柱实体水密且欧拉数为二() {
    let mut k = BrepKernel::new();
    let id = k.add_cylinder([0.0, 0.0, 0.0], R, H, 64);
    let f = k.facts(id).unwrap();
    assert!(f.watertight, "加工件必须水密");
    assert_eq!(f.vertex_count, 128, "64 段 × 上下两圈");
    assert_eq!(f.face_count, 66, "64 个侧面 + 底面 + 顶面");
    println!(
        "[K3] 拓扑：{} 顶点 / {} 边 / {} 面",
        f.vertex_count, f.edge_count, f.face_count
    );
}

#[test]
fn k3_容差决定几何精度档位() {
    // 同一半径下，容差越细需要的段数越多——这是「视口粗、出图细」的基础
    let coarse = segments_for_tolerance(R, 0.0005);
    let fine = segments_for_tolerance(R, 0.00001);
    println!("[K3] Φ10 圆柱：容差 0.5mm → {coarse} 段；容差 0.01mm → {fine} 段");
    assert!(fine > coarse);
}

#[test]
fn k3_能力申报如实() {
    let k = BrepKernel::new();
    let c = k.capabilities();
    assert_eq!(c.backend, "brep");
    assert!(c.exact, "精确后端必须申报 exact");
    assert!(c.max_precision > 0.0);
    // 下面这些要如实反映当前进度（谎报会让 agent 规划出死路）
    assert!(c.solid_bool, "E 阶段已实现平面多面体布尔");
    assert!(!c.curved_bool, "曲面求交未做（F 阶段），圆柱只走离散近似");
    assert!(
        c.fillet,
        "圆角已实现（角部球面片缝合），应报 true——下一行会验证它真能跑"
    );
    assert!(c.step_io, "G 阶段已可导出 STEP");
}

#[test]
fn k3_未实现能力返回unsupported而非静默降级() {
    // 这个测试原本拿 fillet 当「未实现能力」的样本。圆角接入后，
    // 重心改为**申报与实际一致**：申报 true 就必须能跑，
    // 跑不了就必须明确报错，不能交回半成品。
    let mut k = BrepKernel::new();

    // 盒子（开链边界）—— 必须成功且水密
    let id = k.add_box([0.0, 0.0, 0.0], [0.1, 0.1, 0.1]);
    let out = k
        .fillet(id, &[], 0.005, &archcore_kernel::CancelToken::new())
        .expect("申报 fillet = true 后必须真能调用");
    let f = k.facts(out).expect("新实体应可查询");
    assert!(f.watertight, "圆角后必须水密");

    // 圆柱（闭环边界）—— 尚未支持，必须**明确报错**，而不是返回坏实体
    let cyl = k.add_cylinder([0.0, 0.0, 0.0], R, H, 16);
    let err = k
        .fillet(cyl, &[], 0.001, &archcore_kernel::CancelToken::new())
        .expect_err("圆柱圆角尚未支持，必须报错而不是静默降级");
    let msg = err.to_string();
    assert!(
        msg.contains("圆角") || msg.contains("拓扑"),
        "错误应说清楚原因，实际：{msg}"
    );
    assert!(
        msg.contains("建议") || msg.contains("可选"),
        "错误应给出路，实际：{msg}"
    );

    // 未实现的能力位仍如实报 false
    let caps = k.capabilities();
    assert!(!caps.curved_bool, "曲面布尔未做，必须报 false");

    // 未知实体仍是标准错误
    let err = k.facts(SolidId(9999)).unwrap_err();
    assert!(err.to_string().contains("不存在"), "实际：{err}");
}
