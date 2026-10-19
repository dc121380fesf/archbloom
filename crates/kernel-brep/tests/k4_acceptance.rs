//! K4 闸门验收：平面布尔。
//!
//! # 闸门判据（分工与步骤 §五-E）
//!
//! - 两个实体做并 / 差 / 交；
//! - **结果水密**；
//! - 可导出，能被外部工具读回。
//!
//! # 为什么体积能当精确判据
//!
//! 布尔结果由平面多边形组成，散度定理对平面多面体是**精确**的（无离散误差）。
//! 所以这里可以用解析体积直接对账到 1e-9，而不是"D 阶段那种收敛逼近"。

use archcore_kernel::{BoolOp, CancelToken, GeometryKernel, Progress};
use archcore_kernel_brep::{to_stl_ascii, verify_against_solid, BrepKernel};

fn noop_progress(_: Progress) {}

/// 用内核做一次布尔，返回 (结果 id, 体积, 是否水密)
fn do_bool(k: &mut BrepKernel, a: u32, b: u32, op: BoolOp) -> (u32, f64, bool) {
    let id = k
        .boolean(
            archcore_kernel::SolidId(a as u64),
            archcore_kernel::SolidId(b as u64),
            op,
            &CancelToken::new(),
            &mut noop_progress,
        )
        .expect("布尔失败");
    let f = k.facts(id).expect("取事实失败");
    (id.0 as u32, f.volume, f.watertight)
}

#[test]
fn k4_并集_相邻两盒成一体() {
    let mut k = BrepKernel::new();
    let a = k.add_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let b = k.add_box([1.0, 0.0, 0.0], [2.0, 1.0, 1.0]);
    let (_, vol, tight) = do_bool(&mut k, a.0 as u32, b.0 as u32, BoolOp::Union);
    assert!(tight, "并集必须水密");
    assert!((vol - 2.0).abs() < 1e-9, "并集体积应为 2.0，实际 {vol}");
}

#[test]
fn k4_差集_挖出一个槽() {
    let mut k = BrepKernel::new();
    let big = k.add_box([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    // 从 x 方向切掉一半
    let cut = k.add_box([1.0, -1.0, -1.0], [3.0, 3.0, 3.0]);
    let (_, vol, tight) = do_bool(&mut k, big.0 as u32, cut.0 as u32, BoolOp::Difference);
    assert!(tight, "差集必须水密");
    assert!((vol - 4.0).abs() < 1e-9, "差集体积应为 4.0（8−4），实际 {vol}");
}

#[test]
fn k4_交集_重叠部分() {
    let mut k = BrepKernel::new();
    let a = k.add_box([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    let b = k.add_box([1.0, 1.0, 1.0], [3.0, 3.0, 3.0]);
    let (_, vol, tight) = do_bool(&mut k, a.0 as u32, b.0 as u32, BoolOp::Intersection);
    assert!(tight, "交集必须水密");
    assert!((vol - 1.0).abs() < 1e-9, "交集体积应为 1.0，实际 {vol}");
}

#[test]
#[ignore = "F 阶段（平面-圆柱布尔）：当前圆柱参与时走离散近似，拓扑重建不稳定，先把平面布尔做扎实"]
fn k4_差集挖圆孔_离散圆柱参与() {
    // 圆柱参与布尔时走离散近似（curved_bool=false 已申报）
    let mut k = BrepKernel::new();
    let plate = k.add_box([-0.01, -0.01, 0.0], [0.01, 0.01, 0.005]); // 20×20×5mm 板
    let hole = k.add_cylinder([0.0, 0.0, -0.001], 0.004, 0.007, 64); // Φ8 通孔
    let (id, vol, tight) = do_bool(&mut k, plate.0 as u32, hole.0 as u32, BoolOp::Difference);
    assert!(tight, "开孔后必须水密");

    let plate_vol = 0.02 * 0.02 * 0.005;
    let hole_vol = std::f64::consts::PI * 0.004 * 0.004 * 0.005;
    let expect = plate_vol - hole_vol;
    let rel = (vol - expect).abs() / expect;
    println!(
        "[K4] 板开孔：体积 {:.4e}（期望 {:.4e}），误差 {:.3}%",
        vol,
        expect,
        rel * 100.0
    );
    assert!(
        rel < 0.01,
        "离散孔体积误差应 < 1%（64 段），实际 {:.4}%",
        rel * 100.0
    );

    let f = k.facts(archcore_kernel::SolidId(id as u64)).unwrap();
    assert!(f.face_count > 4, "开孔后应有侧面");
}

#[test]
fn k4_布尔结果可导出stl且自检通过() {
    let mut k = BrepKernel::new();
    let a = k.add_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let b = k.add_box([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]);
    let id = k
        .boolean(a, b, BoolOp::Union, &CancelToken::new(), &mut noop_progress)
        .unwrap();
    let solid = k.get(id).expect("实体应存在");

    let stl = to_stl_ascii(solid, "union_result");
    verify_against_solid(&stl, solid).expect("STL 与实体必须一致");
    assert!(stl.starts_with("solid union_result"));
    assert!(stl.trim_end().ends_with("endsolid union_result"));

    let facets = stl.matches("facet normal").count();
    println!("[K4] 并集 STL：{facets} 个三角面");
    assert!(facets >= 12, "并集至少 12 个三角面，实际 {facets}");
}

#[test]
fn k4_拓扑重建后欧拉数正确() {
    let mut k = BrepKernel::new();
    let a = k.add_box([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    let b = k.add_box([1.0, 1.0, 1.0], [3.0, 3.0, 3.0]);
    let id = k
        .boolean(a, b, BoolOp::Union, &CancelToken::new(), &mut noop_progress)
        .unwrap();
    let s = k.get(id).unwrap();
    // 实心单连通实体：V − E + F = 2
    assert_eq!(
        s.euler_characteristic(),
        2,
        "并集欧拉数应为 2（V{} E封闭 F{}）",
        s.verts.len(),
        s.faces.len()
    );
    println!(
        "[K4] 并集拓扑：{} 顶点 / {} 面，欧拉数 {}",
        s.verts.len(),
        s.faces.len(),
        s.euler_characteristic()
    );
}

#[test]
fn k4_不相交时交集显式报错() {
    let mut k = BrepKernel::new();
    let a = k.add_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let b = k.add_box([5.0, 5.0, 5.0], [6.0, 6.0, 6.0]);
    let err = k
        .boolean(
            a,
            b,
            BoolOp::Intersection,
            &CancelToken::new(),
            &mut noop_progress,
        )
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("退化") || msg.contains("没有") || msg.contains("为空"),
        "空交集应明确报错，实际：{msg}"
    );
}

#[test]
fn k4_取消优先于计算() {
    let mut k = BrepKernel::new();
    let a = k.add_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let b = k.add_box([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]);
    let before = k.len();
    let err = k
        .boolean(
            a,
            b,
            BoolOp::Union,
            &CancelToken::cancelled(),
            &mut noop_progress,
        )
        .unwrap_err();
    assert!(matches!(err, archcore_kernel::KernelError::Cancelled));
    assert_eq!(k.len(), before, "取消不得留下半成品实体");
}
// ─── 以下两个场景原本由 bsp.rs 里的 BSP 算法测试覆盖。 ───
//
// BSP 的 Node/union/difference/intersection 已是死代码（正式路径走 csg.rs 的
// 切分+分类+缝合），连同它们的测试一并删除。但这两个**边界场景**确实有价值，
// 所以改写成走内核正式路径的版本，保住同样的可验证性。

#[test]
fn k4_交集_两立方体只重叠一角() {
    // [0,1]³ 与 [0.5,1.5]³ 的重叠只有 [0.5,1]³ = 0.125
    let mut k = BrepKernel::new();
    let a = k.add_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]).0 as u32;
    let b = k.add_box([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]).0 as u32;
    let (_, v, wt) = do_bool(&mut k, a, b, BoolOp::Intersection);
    assert!(wt, "交集必须水密");
    assert!((v - 0.125).abs() < 1e-9, "重叠体积应为 0.125，实际 {v:.12}");
}

#[test]
fn k4_差集_完全包含时为空() {
    // 小盒完全在大盒内部：A - B 得到带空腔的实体，体积 = 大 - 小
    let mut k = BrepKernel::new();
    let big = k.add_box([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]).0 as u32;
    let small = k.add_box([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]).0 as u32;
    let (_, v, wt) = do_bool(&mut k, big, small, BoolOp::Difference);
    assert!(wt, "带内腔的实体必须水密");
    assert!(
        (v - 7.0).abs() < 1e-9,
        "体积应为 8-1=7，实际 {v:.12}"
    );
}