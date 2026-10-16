//! 体块方案工具回归：参数化几何、分层层级、方案摘要、敌对参数
use archcore_engine::Engine;
use archcore_ir::ObjectKind;
use serde_json::json;

fn build(e: &mut Engine, args: serde_json::Value) -> serde_json::Value {
    let r = e.execute("massing.build", &args).unwrap();
    r.extra
}

/// 3 层 2 开间：对象数、层级、体块尺寸、方案摘要
#[test]
fn massing_builds_parametric_scheme() {
    let mut e = Engine::new();
    let extra = build(
        &mut e,
        json!({
            "name": "T", "floors": 3, "floor_height": 3.0,
            "bays_x": 2, "bay_width": 4.0, "depth": 10.0, "columns": true
        }),
    );
    // 2 开间×4 = 8 宽；3 层×3 = 9 高；建筑面积 8×10×3 = 240
    assert_eq!(extra["footprint"], json!([8.0, 10.0]));
    assert_eq!(extra["height"], 9.0);
    assert_eq!(extra["gross_floor_area"], 240.0);
    // 根组 + 外包络 + 3×(层组+楼板+柱网) = 11
    assert_eq!(extra["objects_created"], json!(11));
    assert_eq!(e.scene.objects.len(), 11);
    // 外包络尺寸
    let env = e
        .scene
        .objects
        .iter()
        .find(|o| o.name == "T_Envelope")
        .unwrap();
    let ObjectKind::Mesh { geometry, .. } = &env.kind else {
        panic!("应为网格");
    };
    let maxx = geometry.positions.iter().map(|p| p[0]).fold(f64::MIN, f64::max);
    let maxz = geometry.positions.iter().map(|p| p[2]).fold(f64::MIN, f64::max);
    assert!((maxx - 8.0).abs() < 1e-9);
    assert!((maxz - 9.0).abs() < 1e-9);
    // 层组挂在根下
    let root = e.scene.objects.iter().find(|o| o.name == "T").unwrap();
    let floor_children = e
        .scene
        .objects
        .iter()
        .filter(|o| o.parent.as_deref() == Some(root.id.as_str()))
        .count();
    assert_eq!(floor_children, 4); // 外包络 + 3 层组
}

/// 柱网数量：每层 (bays_x+1)×2 根柱合并为一个 mesh
#[test]
fn massing_columns_per_floor() {
    let mut e = Engine::new();
    build(
        &mut e,
        json!({
            "name": "C", "floors": 1, "floor_height": 3.0,
            "bays_x": 3, "bay_width": 4.0, "depth": 8.0, "columns": true
        }),
    );
    let cols = e
        .scene
        .objects
        .iter()
        .find(|o| o.name == "C_F1_Columns")
        .unwrap();
    let ObjectKind::Mesh { geometry, .. } = &cols.kind else {
        panic!();
    };
    assert_eq!(geometry.positions.len(), 8 * 8); // (3+1)×2 柱 × 8 顶点
}

/// 关件选项：envelope/slabs 全关时只剩分层组
#[test]
fn massing_bare_groups_when_disabled() {
    let mut e = Engine::new();
    let extra = build(
        &mut e,
        json!({
            "name": "B", "floors": 2, "floor_height": 3.0,
            "bays_x": 1, "bay_width": 4.0, "depth": 6.0,
            "envelope": false, "slabs": false
        }),
    );
    assert_eq!(extra["objects_created"], json!(3)); // 根 + 2 层组
}

/// 敌对参数：楼层 0 / 层高负 / 悬空材质，全部结构化拒绝
#[test]
fn massing_rejects_bad_params() {
    let mut e = Engine::new();
    for args in [
        json!({"name": "X", "floors": 0, "floor_height": 3.0, "bays_x": 1, "bay_width": 4.0, "depth": 6.0}),
        json!({"name": "X", "floors": 2, "floor_height": -1.0, "bays_x": 1, "bay_width": 4.0, "depth": 6.0}),
        json!({"name": "X", "floors": 2, "floor_height": 3.0, "bays_x": 1, "bay_width": 4.0, "depth": 6.0, "material": "mat-999"}),
    ] {
        assert!(e.execute("massing.build", &args).is_err(), "{args} 应被拒");
    }
    assert!(e.scene.objects.is_empty());
}
