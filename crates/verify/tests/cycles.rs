//! 结构校验负例（审计 #4/#6）：成环父链、材质/资产撞 id 必须 Error
use archcore_ir::{Geometry, Material, Object, ObjectKind, Scene, ShadingModel, Transform, Units, UpAxis};
use archcore_verify::{default_registry, duplicate_material_asset_id, parent_cycles, Severity};

fn obj(id: &str, parent: Option<&str>) -> Object {
    Object {
        id: id.into(),
        name: id.into(),
        kind: ObjectKind::Mesh {
            geometry: Geometry::default(),
            material_slots: vec![],
        },
        parent: parent.map(|p| p.to_string()),
        transform: Transform::default(),
        visible: true,
        meta: Default::default(),
    }
}

fn scene_with(objects: Vec<Object>) -> Scene {
    Scene {
        schema_version: "0.1".into(),
        units: Units::Mm,
        up_axis: UpAxis::ZUp,
        name: "t".into(),
        objects,
        materials: vec![],
        assets: vec![],
        meta: Default::default(),
    }
}

/// 二元环：a→b→a
#[test]
fn detects_parent_cycle() {
    let scene = scene_with(vec![obj("obj-1", Some("obj-2")), obj("obj-2", Some("obj-1"))]);
    let fs = parent_cycles(&scene, None);
    assert_eq!(fs.len(), 2);
    assert!(fs.iter().all(|f| f.severity == Severity::Error));
}

/// 自环
#[test]
fn detects_self_parent() {
    let scene = scene_with(vec![obj("obj-1", Some("obj-1"))]);
    let fs = parent_cycles(&scene, None);
    assert_eq!(fs.len(), 1);
}

/// 无环场景零误报
#[test]
fn clean_hierarchy_no_false_positive() {
    let scene = scene_with(vec![
        obj("obj-1", None),
        obj("obj-2", Some("obj-1")),
        obj("obj-3", Some("obj-2")),
    ]);
    assert!(parent_cycles(&scene, None).is_empty());
}

/// 材质 id 撞车（审计 #6 补口）
#[test]
fn detects_duplicate_material_id() {
    let mat = |id: &str| Material {
        id: id.into(),
        name: "m".into(),
        shading_model: ShadingModel::PbrMetallicRoughness,
        base_color: [0.5; 4],
        metallic: 0.0,
        roughness: 0.5,
        opacity: 1.0,
        textures: Default::default(),
    };
    let mut scene = scene_with(vec![]);
    scene.materials = vec![mat("mat-1"), mat("mat-1")];
    let fs = duplicate_material_asset_id(&scene, None);
    assert_eq!(fs.len(), 1);
    assert_eq!(fs[0].severity, Severity::Error);
}

/// 门禁整体行为：环 + 撞 id 的场景必须 gate 不过
#[test]
fn gate_rejects_cycle_scene() {
    let verify = default_registry();
    let scene = scene_with(vec![obj("obj-1", Some("obj-2")), obj("obj-2", Some("obj-1"))]);
    let err = verify.gate(&scene, None).unwrap_err();
    assert!(err.iter().any(|f| f.checker == "structure.parent_cycle"));
}
