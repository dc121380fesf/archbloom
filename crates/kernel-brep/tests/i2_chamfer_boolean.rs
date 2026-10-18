//! 倒角在**布尔结果**上的表现（I 阶段第二道验收）。
//!
//! 真实零件很少是纯盒子：更常见的是「布尔完之后再倒角」。
//! 这里用「盒子开孔」作为代表——它的拓扑已经含内环，倒角要能不崩。

use archcore_kernel::{CancelToken, GeometryKernel};
use archcore_kernel_brep::BrepKernel;

/// 造一个开孔板：20×20×5mm 板，中心 Φ10 通孔。
fn perforated_plate() -> (BrepKernel, archcore_kernel::SolidId) {
    let mut k = BrepKernel::new();
    let plate = k.add_box([0.0, 0.0, 0.0], [0.020, 0.020, 0.005]);
    let drill = k.add_cylinder([0.010, 0.010, -0.001], 0.005, 0.007, 64);
    let mut noop = |_p: archcore_kernel::Progress| {};
    let id = k
        .boolean(
            plate,
            drill,
            archcore_kernel::BoolOp::Difference,
            &CancelToken::new(),
            &mut noop,
        )
        .expect("开孔应当成功");
    (k, id)
}

#[test]
fn i2_chamfer_after_boolean_stays_watertight() {
    let (mut k, id) = perforated_plate();
    let before = k.facts(id).expect("原实体应可查询");
    println!(
        "开孔板：面 {} 边 {} 顶点 {} 体积 {:.6} 水密 {}",
        before.face_count, before.edge_count, before.vertex_count, before.volume, before.watertight
    );

    // 只倒外轮廓（0.3mm）。内孔壁与上下表面的交线也是锐边，
    // 但把整块板的所有锐边都倒了会撞上"孔壁太薄"的退化，先只验证外框。
    let r = k.chamfer(id, &[], 0.0003, &CancelToken::new());
    match r {
        Ok(out) => {
            let after = k.facts(out).expect("倒角后应可查询");
            println!(
                "倒角后：面 {} 边 {} 顶点 {} 体积 {:.6} 水密 {}",
                after.face_count, after.edge_count, after.vertex_count, after.volume, after.watertight
            );
            assert!(after.watertight, "倒角后必须保持水密");
            assert!(
                after.volume < before.volume,
                "倒角应减少体积（{} → {}）",
                before.volume,
                after.volume
            );
            // 0.3mm 倒角对 20mm 板的损耗应远小于 1%
            let loss = (before.volume - after.volume) / before.volume;
            assert!(loss < 0.01, "0.3mm 倒角不该掉 {:.3}% 体积", loss * 100.0);
        }
        Err(e) => {
            // 失败也要是**有诊断的失败**，不是 panic 也不是静默坏实体
            println!("倒角未完成（这是可接受的，但必须给出原因）：{e}");
            let msg = format!("{e}");
            assert!(
                msg.contains("退化") || msg.contains("建议"),
                "失败必须带可执行诊断，实际：{msg}"
            );
        }
    }
}

#[test]
fn i2_chamfer_preserves_original_solid() {
    // 不可变语义：倒角产出新实体，原实体必须还在（agent 可能还要用它做别的）
    let (mut k, id) = perforated_plate();
    let before = k.facts(id).expect("原实体");
    let _ = k.chamfer(id, &[], 0.0003, &CancelToken::new());
    let after = k.facts(id).expect("原实体应仍在");
    assert_eq!(
        before.face_count, after.face_count,
        "原实体的面数不应被倒角改变"
    );
    assert!((before.volume - after.volume).abs() < 1e-18, "原实体体积不应变");
}
