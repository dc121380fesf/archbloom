//! 直出场景编排回归：一条龙产物、失败步骤定位、vr 快捷位
use archcore_engine::Engine;
use archcore_ir::ObjectKind;
use serde_json::json;

#[test]
fn compose_one_shot_scene() {
    let dir = std::env::temp_dir().join("archcore_compose_test");
    let _ = std::fs::remove_dir_all(&dir);
    let mut e = Engine::new();
    let summary = e
        .compose(&json!({
            "name": "社区中心",
            "materials": [{"name": "MAT_Wood", "id": "mat-0"}],
            "site": {"size": [24, 18]},
            "buildings": [{
                "name": "B", "floors": 2, "floor_height": 3.0,
                "bays_x": 2, "bay_width": 4.0, "depth": 8.0,
                "position": [2, 2, 0], "columns": true, "material": "mat-0"
            }],
            "furniture": [
                {"kind": "table", "position": [12, 8, 0], "material": "mat-0"}
            ],
            "deliverables": {
                "perspective": {"path": dir.join("view.png").to_str().unwrap(), "width": 48, "height": 32, "spp": 2},
                "walkthrough": {"dir": dir.join("walk").to_str().unwrap(), "waypoints": [[1,14,1.6],[10,8,1.6]], "target": [6,2,1], "frames": 2, "width": 32, "height": 24, "spp": 2}
            },
            "renderer": "cpu"
        }))
        .unwrap();
    // 材质 + 场地 + 方案（根+包络+2 层组+2 楼板+2 柱网 = 8）+ 桌子 = 11 对象
    assert_eq!(summary["scene"], "社区中心");
    assert_eq!(summary["objects_created"], json!(11));
    assert_eq!(summary["renderer"], "cpu");
    assert!(dir.join("view.png").exists());
    assert!(dir.join("walk/walk_0000.png").exists());
    assert!(summary["deliverables"]["walkthrough"]["frames"] == 2
        || summary["deliverables"]["walkthrough"].is_object());
}

/// 失败带步骤号与 op 名（fail-closed 口径）
#[test]
fn compose_failure_names_step() {
    let mut e = Engine::new();
    let err = e
        .compose(&json!({
            "site": {"ground": true},
            "buildings": [{"name": "X", "floors": 3}]
        }))
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("compose 第"), "{msg}");
    assert!(msg.contains("massing.build"), "{msg}");
}

/// M2：内装预设 + 门窗开洞进管线 + 质量缺省
#[test]
fn compose_m2_interiors_openings_quality() {
    let dir = std::env::temp_dir().join("archcore_compose_m2");
    let _ = std::fs::remove_dir_all(&dir);
    let mut e = Engine::new();
    let summary = e
        .compose(&json!({
            "materials": [{"name": "M", "id": "mat-0"}],
            "site": {"ground": true, "size": [20, 16]},
            "buildings": [{
                "name": "H", "floors": 1, "floor_height": 3.0,
                "bays_x": 1, "bay_width": 6.0, "depth": 6.0,
                "position": [2, 2, 0], "material": "mat-0"
            }],
            "openings": [{"wall": "H_Envelope", "kind": "door", "position": [5, 2, 0], "width": 1.0, "height": 2.1}],
            "interiors": [{"preset": "dining", "position": [3, 3, 0], "size": [4, 4], "material": "mat-0"}],
            "deliverables": {
                "perspective": {"path": dir.join("v.png").to_str().unwrap(), "width": 48, "height": 32, "spp": 2}
            },
            "renderer": "cpu"
        }))
        .unwrap();
    // 内装：餐桌 +4 椅落位
    assert!(e.scene.objects.iter().any(|o| o.name == "餐桌"));
    assert_eq!(
        e.scene.objects.iter().filter(|o| o.name.starts_with("餐椅")).count(),
        4
    );
    // 开洞：Envelope 三角数 > 盒体基准 12
    let env = e.scene.objects.iter().find(|o| o.name == "H_Envelope").unwrap();
    let ObjectKind::Mesh { geometry, .. } = &env.kind else { panic!() };
    assert!(geometry.indices.len() / 3 > 12, "门洞必须切进墙");
    // 质量缺省
    assert_eq!(summary["quality_defaults"]["perspective_spp"], 64);
}

/// 坏预设/悬空墙名：结构化拒绝带步骤号
#[test]
fn compose_m2_rejects_bad_refs() {
    let mut e = Engine::new();
    let err = e
        .compose(&json!({
            "interiors": [{"preset": "garage", "position": [0,0,0], "size": [4,4]}]
        }))
        .unwrap_err();
    assert!(format!("{err}").contains("garage"));
    let err2 = e
        .compose(&json!({
            "openings": [{"wall": "Ghost", "kind": "door", "position": [0,0,0]}]
        }))
        .unwrap_err();
    assert!(format!("{err2}").contains("Ghost"));
}
/// M3：模板套用 + 覆盖语义（对象深合并、数组替换）
#[test]
fn compose_template_expansion() {
    let mut e = Engine::new();
    let summary = e
        .compose(&json!({
            "template": "community_center",
            "name": "模板实例"
        }))
        .unwrap();
    assert_eq!(summary["scene"], "模板实例");
    assert!(summary["objects_created"].as_u64().unwrap() > 20, "模板应铺满场景");
    // 未知模板拒绝
    let err = e.compose(&json!({"template": "spaceship"})).unwrap_err();
    assert!(format!("{err}").contains("spaceship"));
}

/// M3：多方案对比——变体隔离、数组替换语义、对比长图产物
#[test]
fn compose_variants_comparison() {
    let dir = std::env::temp_dir().join("archcore_variants_test");
    let _ = std::fs::remove_dir_all(&dir);
    let result = Engine::compose_variants(&json!({
        "template": "school",
        "site": {"ground": true, "size": [20, 16]},
        "interiors": [],
        "variants": [
            {"name": "矮方案", "buildings": [{"name": "楼", "floors": 2, "floor_height": 3.0, "bays_x": 1, "bay_width": 6.0, "depth": 6.0, "position": [2,2,0]}]},
            {"name": "高方案", "buildings": [{"name": "楼", "floors": 4, "floor_height": 3.0, "bays_x": 1, "bay_width": 6.0, "depth": 6.0, "position": [2,2,0]}]}
        ],
        "deliverables": {
            "perspective": {"path": dir.join("view.png").to_str().unwrap(), "width": 48, "height": 32, "spp": 2}
        },
        "comparison_path": dir.join("comparison.png").to_str().unwrap(),
        "renderer": "cpu"
    }))
    .unwrap();
    assert_eq!(result["variants"].as_array().unwrap().len(), 2);
    // 变体产物不互覆（后缀隔离）
    assert!(dir.join("view_v1.png").exists());
    assert!(dir.join("view_v2.png").exists());
    // 对比长图（列序=变体序）
    assert!(dir.join("comparison.png").exists());
    assert_eq!(result["columns"].as_array().unwrap().len(), 2);
    // 数组替换语义：高方案层数多→对象数更多
    let c1 = result["variants"][0]["objects_created"].as_u64().unwrap();
    let c2 = result["variants"][1]["objects_created"].as_u64().unwrap();
    assert!(c2 > c1, "数组整体替换应生效");
    // 名称带变体后缀
    assert_eq!(result["variants"][0]["scene"], "矮方案_v1");
}

#[test]
fn compose_vr_shorthand() {
    let dir = std::env::temp_dir().join("archcore_compose_vr");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = Engine::new();
    let summary = e
        .compose(&json!({
            "site": {"ground": false},
            "deliverables": {
                "vr": {"path": dir.join("vr.png").to_str().unwrap(), "width": 64, "height": 16, "spp": 2}
            },
            "renderer": "cpu"
        }))
        .unwrap();
    assert!(dir.join("vr.png").exists());
    assert!(summary["deliverables"]["vr"]["stereo"] == true);
}
