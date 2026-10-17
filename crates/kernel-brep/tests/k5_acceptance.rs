//! K5 闸门验收：平面-圆柱布尔（开孔）。
//!
//! # 闸门判据（项目书 §7 / 分工与步骤 §五-F）
//!
//! - 平面上开圆孔；
//! - 能导出、被 FreeCAD 读回（STEP 待 G 阶段，本轮用 STL 验几何）。
//!
//! # 一个重要区别：这是「离散近似」不是「真曲面求交」
//!
//! 圆柱在 B-rep 里被离散成 N 个平面四边形（`cylinder_solid(.., segments)`），
//! 布尔运算在**这些平面片**上做，不是在圆柱方程上做。
//!
//! 所以：
//! - ✅ 能开孔、水密、体积误差随段数收敛；
//! - ❌ 拿不到**解析交线**（真曲面求交应该给出椭圆弧，这里给出折线）。
//!
//! 这直接决定了 `Capabilities.curved_bool` 该报什么——见文件末尾的说明。

use std::f64::consts::PI;

use archcore_kernel::{BoolOp, CancelToken, GeometryKernel, Progress, SolidId};
use archcore_kernel_brep::{to_stl_ascii, verify_against_solid, BrepKernel};

fn noop_progress(_: Progress) {}

fn cut_hole(segments: usize) -> (f64, bool, BrepKernel, SolidId) {
    let mut k = BrepKernel::new();
    // 20×20×5 mm 板
    let plate = k.add_box([-0.01, -0.01, 0.0], [0.01, 0.01, 0.005]);
    // Φ8 通孔（上下各留 1mm 余量确保穿透）
    let hole = k.add_cylinder([0.0, 0.0, -0.001], 0.004, 0.007, segments);
    let id = k
        .boolean(
            plate,
            hole,
            BoolOp::Difference,
            &CancelToken::new(),
            &mut noop_progress,
        )
        .expect("开孔失败");
    let f = k.facts(id).expect("取事实失败");
    (f.volume, f.watertight, k, id)
}

fn expected_volume() -> f64 {
    let plate = 0.02 * 0.02 * 0.005;
    let hole = PI * 0.004 * 0.004 * 0.005;
    plate - hole
}

#[test]
fn k5_单孔体积误差小于千分之一() {
    let (vol, tight, _, _) = cut_hole(64);
    let expect = expected_volume();
    let rel = (vol - expect).abs() / expect;
    println!(
        "[K5] 单孔：体积 {:.6e}（期望 {:.6e}），误差 {:.4}%",
        vol,
        expect,
        rel * 100.0
    );
    assert!(tight, "开孔后必须水密");
    assert!(rel < 0.001, "K5 要求 < 0.1%，实际 {:.4}%", rel * 100.0);
}

/// 已知限制：段数拉高后拓扑重建失败。
///
/// 现象：128 段时欧拉数 -256（恰为 128×2，即上下两圈圆环的边数各 128）。
/// 已排除的解释：
/// - **缺内环**：切分把环形区域切成独立扇区，并不需要带洞的面；
/// - **单一容差**：把 WELD_TOL 从 1e-7 抬到 1e-6 会让单孔也失败（误合并，欧拉 0）。
///
/// 待查方向：切分产生 13000+ 碎片时，孔壁顶边与板面扇区边之间的顶点坐标
/// 差了多少——需要在真实容差下逐点比对，而不是量化成键（量化会掩盖问题）。
#[test]
fn k5_误差随段数收敛() {
    let expect = expected_volume();
    let mut last = f64::MAX;
    for segs in [16usize, 32, 64, 128] {
        let (vol, tight, _, _) = cut_hole(segs);
        let rel = (vol - expect).abs() / expect;
        println!("[K5] {segs:>3} 段 → 误差 {:.4}%", rel * 100.0);
        assert!(tight, "{segs} 段时也应水密");
        assert!(rel < last, "{segs} 段误差应比上一档小");
        last = rel;
    }
    assert!(last < 0.0005, "128 段应优于 0.05%，实际 {:.4}%", last * 100.0);
}

/// 已知限制：与上面同源——多孔时碎片规模更大，问题更早暴露。
#[test]
fn k5_四个孔阵列() {
    let mut k = BrepKernel::new();
    let plate = k.add_box([-0.02, -0.02, 0.0], [0.02, 0.02, 0.005]);
    let mut cur = plate;
    for (x, y) in [
        (-0.012, -0.012),
        (0.012, -0.012),
        (-0.012, 0.012),
        (0.012, 0.012),
    ] {
        let hole = k.add_cylinder([x, y, -0.001], 0.003, 0.007, 48);
        cur = k
            .boolean(
                cur,
                hole,
                BoolOp::Difference,
                &CancelToken::new(),
                &mut noop_progress,
            )
            .expect("开孔失败");
    }
    let f = k.facts(cur).expect("取事实");
    let expect = 0.04 * 0.04 * 0.005 - 4.0 * PI * 0.003 * 0.003 * 0.005;
    let rel = (f.volume - expect).abs() / expect;
    println!(
        "[K5] 四孔阵列：体积 {:.6e}（期望 {:.6e}），误差 {:.4}%，面 {}",
        f.volume,
        expect,
        rel * 100.0,
        f.face_count
    );
    assert!(f.watertight, "四孔阵列也必须水密");
    assert!(rel < 0.002, "四孔误差应 < 0.2%，实际 {:.4}%", rel * 100.0);
}

#[test]
fn k5_盲孔_孔深小于板厚() {
    let mut k = BrepKernel::new();
    let plate = k.add_box([-0.01, -0.01, 0.0], [0.01, 0.01, 0.005]);
    // 只挖进去 2mm（板厚 5mm）——从顶面向下
    let hole = k.add_cylinder([0.0, 0.0, 0.003], 0.004, 0.004, 64);
    let id = k
        .boolean(
            plate,
            hole,
            BoolOp::Difference,
            &CancelToken::new(),
            &mut noop_progress,
        )
        .expect("盲孔失败");
    let f = k.facts(id).expect("取事实");
    let expect = 0.02 * 0.02 * 0.005 - PI * 0.004 * 0.004 * 0.002;
    let rel = (f.volume - expect).abs() / expect;
    println!(
        "[K5] 盲孔：体积 {:.6e}（期望 {:.6e}），误差 {:.4}%",
        f.volume,
        expect,
        rel * 100.0
    );
    assert!(f.watertight);
    assert!(rel < 0.002, "盲孔误差应 < 0.2%");
}

#[test]
fn k5_开孔件可导出且自检通过() {
    let (_, _, k, id) = cut_hole(64);
    let solid = k.get(id).expect("实体不存在");
    let stl = to_stl_ascii(solid, "plate_with_hole");
    verify_against_solid(&stl, solid).expect("STL 与实体一致");
    let facets = stl.matches("facet normal").count();
    println!("[K5] STL：{facets} 个三角面");
    assert!(facets > 50, "开孔件的三角面应远多于 12");
}

#[test]
fn k5_能力申报保持诚实() {
    let k = BrepKernel::new();
    let c = k.capabilities();
    // 圆柱参与布尔能工作，但那是「离散近似」，不是解析曲面求交。
    // 所以 curved_bool 仍报 false——若报 true，agent 会期待拿到圆/椭圆交线，
    // 而实际只能拿到折线，那是谎报。
    assert!(c.solid_bool, "平面布尔已实现");
    assert!(
        !c.curved_bool,
        "真曲面求交未实现（当前是离散近似），必须报 false"
    );
}
