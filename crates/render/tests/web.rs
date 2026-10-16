//! Web 查看器导出回归：自包含、几何嵌入、转义、空场景容错
use archcore_render::export_web;
use archcore_ir::{Geometry, Material, Object, ObjectKind, Scene, ShadingModel, Transform, Units, UpAxis};

fn scene() -> Scene {
    Scene {
        schema_version: "0.1".into(),
        units: Units::M,
        up_axis: UpAxis::ZUp,
        name: "审阅场景".into(),
        objects: vec![
            Object {
                id: "obj-1".into(),
                name: "组".into(),
                kind: ObjectKind::Group,
                parent: None,
                transform: Transform::default(),
                visible: true,
                meta: Default::default(),
            },
            Object {
                id: "obj-2".into(),
                name: "板<script>".into(),
                kind: ObjectKind::Mesh {
                    geometry: Geometry {
                        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                        indices: vec![0, 2, 1],
                        normals: None,
                        uv_sets: vec![],
                    },
                    material_slots: vec!["mat-0".into()],
                },
                parent: Some("obj-1".into()),
                transform: Transform::default(),
                visible: true,
                meta: Default::default(),
            },
        ],
        materials: vec![Material {
            id: "mat-0".into(),
            name: "M".into(),
            shading_model: ShadingModel::PbrMetallicRoughness,
            base_color: [0.8, 0.2, 0.2, 1.0],
            metallic: 0.0,
            roughness: 0.5,
            opacity: 1.0,
            textures: Default::default(),
        }],
        assets: vec![],
        meta: Default::default(),
    }
}

/// 自包含 + 几何嵌入 + 查看器要素齐备
#[test]
fn web_export_is_self_contained() {
    let html = export_web(&scene(), "测试<title>").unwrap();
    // 零外部请求：不得出现外部脚本/链接
    assert!(!html.contains("http://"), "不得引外链");
    assert!(!html.contains("https://"), "不得引外链");
    assert!(!html.contains("<script src"), "脚本必须内嵌");
    // 查看器要素
    for marker in [
        "scene-data",
        "webgl2",
        "uMVP",
        "btn-wire",
        "btn-fit",
        "物体树",
    ] {
        assert!(html.contains(marker), "缺查看器要素：{marker}");
    }
    // 几何嵌入（1 三角 = 9 个坐标）
    assert!(
        html.contains("\"positions\":[0.0,0.0,0.0,0.0,1.0,0.0,1.0,0.0,0.0]"),
        "几何未按序嵌入"
    );
    // 树结构与分段
    assert!(html.contains("\"segments\""));
    assert!(html.contains("\"parent\":\"obj-1\""));
    // 转义（标题进 <title>，对象名进 JSON——JSON 侧安全，标题侧必须转义）
    assert!(html.contains("测试&lt;title&gt;"), "标题转义缺失");
}

/// 空场景容错（查看器应正常打开空树）
#[test]
fn web_export_empty_scene_ok() {
    let mut s = scene();
    s.objects.clear();
    let html = export_web(&s, "空").unwrap();
    assert!(html.contains("\"positions\":[]"));
}

/// 事实面进 HUD（哈希以十六进制呈现）
#[test]
fn web_export_embeds_facts() {
    let html = export_web(&scene(), "f").unwrap();
    assert!(html.contains("hierarchy_hash"));
    assert!(html.contains("geometry_hash"));
}
