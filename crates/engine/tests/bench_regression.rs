//! 评测基准 v1 回归化：纯引擎子集入 cargo（外链任务 5/8/9 需 Blender/Max，留在 demo/bench.py）
//! 任务编号对齐《评测基准v0.md》；每次合入跑全量。
use archcore_engine::Engine;
use serde_json::Value;

fn box_op(name: &str, x0: f64, y0: f64, z0: f64, x1: f64, y1: f64, z1: f64) -> (String, Value) {
    (
        "mesh.create".into(),
        serde_json::json!({
            "name": name,
            "positions": [[x0,y0,z0],[x1,y0,z0],[x1,y1,z0],[x0,y1,z0],[x0,y0,z1],[x1,y0,z1],[x1,y1,z1],[x0,y1,z1]],
            "indices": [0,2,1,0,3,2,4,5,6,4,6,7,0,1,5,0,5,4,2,3,7,2,7,6,1,2,6,1,6,5,0,4,7,0,7,3]
        }),
    )
}

/// T1：墙 3×2.8m 开 0.9×2.1m 门洞
#[test]
fn bench_t1_door_opening() {
    let mut e = Engine::new();
    e.execute_script(&[
        box_op("Wall", 0.0, 0.0, 0.0, 3.0, 0.2, 2.8),
        box_op("DoorCutter", 1.05, -0.1, 0.0, 1.95, 0.3, 2.1),
    ])
    .unwrap();
    let (a, b) = (e.scene.objects[0].id.clone(), e.scene.objects[1].id.clone());
    let r = e
        .execute(
            "mesh.boolean",
            &serde_json::json!({"id_a": a, "id_b": b, "op": "difference", "name": "Wall_门洞"}),
        )
        .unwrap();
    assert_ne!(r.facts_before.geometry_hash, r.facts_after.geometry_hash);
    assert!(r.facts_after.triangle_count > 12);
}

/// T3：窗洞阵列——5 次链式差集每次都必须产生几何变化
#[test]
fn bench_t3_window_array() {
    let mut e = Engine::new();
    let mut steps = vec![box_op("Wall", 0.0, 0.0, 0.0, 8.0, 0.2, 2.4)];
    for (i, c) in [0.75f64, 2.25, 3.75, 5.25, 6.75].into_iter().enumerate() {
        steps.push(box_op(&format!("Cut_{i}"), c - 0.6, -0.1, 0.5, c + 0.6, 0.3, 2.0));
    }
    e.execute_script(&steps).unwrap();
    let wall = e.scene.objects[0].id.clone();
    let mut prev = wall;
    for i in 0..5 {
        let cutter = e.scene.objects[i + 1].id.clone();
        let r = e
            .execute(
                "mesh.boolean",
                &serde_json::json!({"id_a": prev, "id_b": cutter, "op": "difference"}),
            )
            .unwrap();
        assert_ne!(
            r.facts_before.geometry_hash, r.facts_after.geometry_hash,
            "第{i}次切割未产生几何变化"
        );
        prev = r.changed[0].clone();
    }
}

/// T4：材质 + 贴图 + UV 绑定（资产计数入事实面）
#[test]
fn bench_t4_materials() {
    let mut e = Engine::new();
    // 材质 id 由单调计数器分配，测试用返回 id，不硬编码（审计 #6 后语义）
    let r = e
        .execute(
            "material.create",
            &serde_json::json!({"name": "MAT_Bricks"}),
        )
        .unwrap();
    let mid = r.changed[0].clone();
    e.execute(
        "material.set_texture",
        &serde_json::json!({"material": mid, "slot": "base_color", "uri": "D:/x/bricks.jpg"}),
    )
    .unwrap();
    e.execute(
        "mesh.create",
        &serde_json::json!({
            "name": "Wall",
            "positions": [[0,0,0],[3,0,0],[3,0,1.8],[0,0,1.8]],
            "indices": [0,2,1,0,3,2],
            "uv": [[0,0],[3,0],[3,1.8],[0,1.8]],
            "material": mid
        }),
    )
    .unwrap();
    let f = archcore_ops::geo_facts(&e.scene);
    assert_eq!(f.material_count, 1);
    assert_eq!(f.asset_count, 1);
    match &e.scene.objects[0].kind {
        archcore_ir::ObjectKind::Mesh { geometry, material_slots } => {
            assert_eq!(geometry.uv_sets[0].coords.len(), 4);
            assert_eq!(material_slots[0], mid);
        }
        _ => panic!("应为网格"),
    }
}

/// T7：整层复制三层——子树克隆、偏移、层级保持
#[test]
fn bench_t7_subtree_duplicate() {
    let mut e = Engine::new();
    e.execute_script(&[
        (
            "object.create_group".into(),
            serde_json::json!({"name": "Floor_A"}),
        ),
        box_op("Slab", 0.0, 0.0, 0.0, 4.0, 4.0, 0.15),
        box_op("Wall", 0.0, 3.85, 0.15, 4.0, 4.0, 2.8),
        (
            "object.set_parent".into(),
            serde_json::json!({"id": "obj-2", "parent": "obj-1"}),
        ),
        (
            "object.set_parent".into(),
            serde_json::json!({"id": "obj-3", "parent": "obj-1"}),
        ),
    ])
    .unwrap();
    let r1 = e
        .execute(
            "object.duplicate",
            &serde_json::json!({"id": "obj-1", "name": "Floor_B", "translate": [0,0,3]}),
        )
        .unwrap();
    let r2 = e
        .execute(
            "object.duplicate",
            &serde_json::json!({"id": "obj-1", "name": "Floor_C", "translate": [0,0,6]}),
        )
        .unwrap();
    assert_eq!(r1.changed.len(), 3);
    assert_eq!(r2.changed.len(), 3);
    assert_eq!(e.scene.objects.len(), 9);
    let groups: Vec<_> = e
        .scene
        .objects
        .iter()
        .filter(|o| o.name.starts_with("Floor_"))
        .collect();
    assert_eq!(groups.len(), 3);
    for g in &groups {
        assert_eq!(
            e.scene
                .objects
                .iter()
                .filter(|o| o.parent.as_deref() == Some(g.id.as_str()))
                .count(),
            2
        );
    }
}

/// T10（一票否决）：切除打空必须显式报错，静默 no-op = 全基准不合格
#[test]
fn bench_t10_silence_veto() {
    let mut e = Engine::new();
    e.execute_script(&[
        box_op("Wall", 0.0, 0.0, 0.0, 3.0, 0.2, 2.4),
        box_op("Cutter", 9.0, 9.0, 9.0, 10.0, 10.0, 10.0),
    ])
    .unwrap();
    let (a, b) = (e.scene.objects[0].id.clone(), e.scene.objects[1].id.clone());
    let err = e
        .execute(
            "mesh.boolean",
            &serde_json::json!({"id_a": a, "id_b": b, "op": "difference"}),
        )
        .unwrap_err();
    assert!(matches!(
        err,
        archcore_engine::EngineError::Op(archcore_ops::OpError::Precondition { .. })
    ));
    assert_eq!(e.scene.objects.len(), 2);
}

/// 相机取景（债务回归锁）：aim 后 -Z 朝向目标
#[test]
fn camera_aim_points_at_target() {
    let mut e = Engine::new();
    e.execute(
        "camera.create",
        &serde_json::json!({"name": "Shot", "translation": [5,-5,4], "target": [0,0,1]}),
    )
    .unwrap();
    match &e.scene.objects[0].kind {
        archcore_ir::ObjectKind::Camera { .. } => {
            let q = e.scene.objects[0].transform.rotation;
            let qv = [q[0], q[1], q[2]];
            let cross = |a: [f64; 3], b: [f64; 3]| {
                [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
            };
            let v = [0.0, 0.0, -1.0];
            let t = cross(qv, v);
            let t = [t[0] * 2.0, t[1] * 2.0, t[2] * 2.0];
            let c2 = cross(qv, t);
            let fwd = [
                v[0] + t[0] * q[3] + c2[0],
                v[1] + t[1] * q[3] + c2[1],
                v[2] + t[2] * q[3] + c2[2],
            ];
            let d = [-5.0f64, 5.0, -3.0];
            let dl = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            for i in 0..3 {
                assert!((fwd[i] - d[i] / dl).abs() < 1e-9);
            }
        }
        _ => panic!("应为相机"),
    }
}
