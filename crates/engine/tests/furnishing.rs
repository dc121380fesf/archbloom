//! 室内软装回归：家具库几何、布置变换、门窗开洞（含打空拒绝）
use archcore_engine::Engine;
use archcore_ir::ObjectKind;
use serde_json::json;

fn wall(e: &mut Engine) -> String {
    e.execute_script(&[(
        "mesh.create".into(),
        json!({
            "name": "Wall",
            "positions": [[0,0,0],[6,0,0],[6,0.2,0],[0,0.2,0],[0,0,2.8],[6,0,2.8],[6,0.2,2.8],[0,0.2,2.8]],
            "indices": [0,2,1,0,3,2,4,5,6,4,6,7,0,1,5,0,5,4,2,3,7,2,7,6,1,2,6,1,6,5,0,4,7,0,7,3]
        }),
    )])
    .unwrap();
    e.scene.objects[0].id.clone()
}

/// 八类家具：包围盒贴合 size
#[test]
fn furniture_kinds_fit_size() {
    let mut e = Engine::new();
    for kind in [
        "door", "window", "table", "chair", "bed", "sofa", "cabinet", "bookshelf",
    ] {
        let r = e
            .execute(
                "furniture.create",
                &json!({"kind": kind, "size": [1.0, 2.0, 3.0], "position": [0,0,0]}),
            )
            .unwrap();
        let obj = e
            .scene
            .objects
            .iter()
            .find(|o| o.id == r.changed[0])
            .unwrap();
        let ObjectKind::Mesh { geometry, .. } = &obj.kind else {
            panic!("{kind} 应为网格");
        };
        assert!(!geometry.positions.is_empty(), "{kind} 几何为空");
        for (axis, want) in [(0, 1.0), (1, 2.0), (2, 3.0)] {
            let lo = geometry
                .positions
                .iter()
                .map(|p| p[axis])
                .fold(f64::MAX, f64::min);
            let hi = geometry
                .positions
                .iter()
                .map(|p| p[axis])
                .fold(f64::MIN, f64::max);
            assert!((hi - lo - want).abs() < 1e-6, "{kind} 轴 {axis} 尺寸漂移");
        }
    }
}

/// 布置变换：平移/旋转/缩放落 transform；scale 非正拒绝
#[test]
fn object_transform_moves_rotates_scales() {
    let mut e = Engine::new();
    e.execute_script(&[(
        "object.create_group".into(),
        json!({"name": "G"}),
    )])
    .unwrap();
    e.execute(
        "object.transform",
        &json!({"id": "obj-1", "translate": [1, 2, 3], "rotate_z": 90, "scale": 2}),
    )
    .unwrap();
    let g = &e.scene.objects[0];
    assert_eq!(g.transform.translation, [1.0, 2.0, 3.0]);
    // 绕 Z 90°：四元数 [0,0,sin45,cos45]
    let s = std::f64::consts::FRAC_1_SQRT_2;
    for (a, b) in g.transform.rotation.iter().zip([0.0, 0.0, s, s]) {
        assert!((a - b).abs() < 1e-9);
    }
    assert_eq!(g.transform.scale, [2.0, 2.0, 2.0]);
    let err = e
        .execute("object.transform", &json!({"id": "obj-1", "scale": -1}))
        .unwrap_err();
    let _ = err;
}

/// 门窗开洞：墙 id 不变、几何变复杂；打空显式报错
#[test]
fn opening_cut_modifies_wall_in_place() {
    let mut e = Engine::new();
    let wid = wall(&mut e);
    let before = e.scene.objects[0].clone();
    let ObjectKind::Mesh { geometry: g0, .. } = &before.kind else {
        panic!();
    };
    let tris0 = g0.indices.len() / 3;
    let r = e
        .execute(
            "opening.cut",
            &json!({"wall": wid, "kind": "door", "position": [1.0, 0, 0], "width": 0.9, "height": 2.1}),
        )
        .unwrap();
    assert_eq!(r.changed[0], wid, "开洞必须就地改墙");
    let ObjectKind::Mesh { geometry: g1, .. } = &e.scene.objects[0].kind else {
        panic!();
    };
    assert!(g1.indices.len() / 3 > tris0, "洞口必须增加几何");
    assert_ne!(r.facts_before.geometry_hash, r.facts_after.geometry_hash);
    // 打空：洞在墙外 → 显式报错（宪法：禁止静默）
    let err = e
        .execute(
            "opening.cut",
            &json!({"wall": wid, "kind": "window", "position": [100, 0, 1], "width": 1.2, "height": 1.5}),
        )
        .unwrap_err();
    let _ = err;
}

/// uv 健全性回归（错位 uv 曾让 Blender 拒收整个 glTF）：开洞后 uv 必须与顶点对齐
#[test]
fn opening_cut_reprojects_uv() {
    let mut e = Engine::new();
    e.execute_script(&[(
        "mesh.create".into(),
        json!({
            "name": "Wall",
            "positions": [[0,0,0],[6,0,0],[6,0.2,0],[0,0.2,0],[0,0,2.8],[6,0,2.8],[6,0.2,2.8],[0,0.2,2.8]],
            "indices": [0,2,1,0,3,2,4,5,6,4,6,7,0,1,5,0,5,4,2,3,7,2,7,6,1,2,6,1,6,5,0,4,7,0,7,3],
            "uv": [[0,0],[2.4,0],[2.4,0.08],[0,0.08],[0,1.12],[2.4,1.12],[2.4,1.2],[0,1.2]]
        }),
    )])
    .unwrap();
    let wid = e.scene.objects[0].id.clone();
    e.execute(
        "opening.cut",
        &json!({"wall": wid, "kind": "door", "position": [1.2,0,0], "width": 1.0, "height": 2.1}),
    )
    .unwrap();
    let ObjectKind::Mesh { geometry, .. } = &e.scene.objects[0].kind else {
        panic!()
    };
    assert!(geometry.positions.len() > 8, "切洞应增加顶点");
    assert_eq!(
        geometry.uv_sets[0].coords.len(),
        geometry.positions.len(),
        "uv 必须与顶点对齐（错位即非法 glTF）"
    );
}

/// 拓扑编辑清 uv（顶点数变了旧 uv 必错位，宁缺勿错）
#[test]
fn topology_edit_clears_stale_uv() {
    let mut e = Engine::new();
    e.execute_script(&[(
        "mesh.create".into(),
        json!({
            "name": "Plane",
            "positions": [[0,0,0],[1,0,0],[1,1,0],[0,1,0]],
            "indices": [0,2,1,0,3,2],
            "uv": [[0,0],[1,0],[1,1],[0,1]]
        }),
    )])
    .unwrap();
    e.execute("mesh.add_face", &json!({"id": "obj-1", "a": 0, "b": 1, "c": 3}))
        .unwrap();
    let ObjectKind::Mesh { geometry, .. } = &e.scene.objects[0].kind else {
        panic!()
    };
    assert!(geometry.uv_sets.is_empty(), "拓扑编辑后旧 uv 必须清除");
}

/// 未知家具类型：结构化拒绝
#[test]
fn furniture_bad_kind_rejected() {
    let mut e = Engine::new();
    assert!(e
        .execute("furniture.create", &json!({"kind": "unicorn"}))
        .is_err());
    assert!(e.scene.objects.is_empty());
}
