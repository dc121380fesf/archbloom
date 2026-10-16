//! 敌对输入负例（审计 #15③/#1）：坏参数必须结构化报错，绝不 panic
use archcore_ir::{Scene, Units, UpAxis};
use archcore_ops::{register_builtins, OpError, OpRegistry};
use serde_json::json;

fn reg() -> OpRegistry {
    let mut r = OpRegistry::new("test");
    register_builtins(&mut r).unwrap();
    r
}

fn empty_scene() -> Scene {
    Scene {
        schema_version: "0.1".into(),
        units: Units::Mm,
        up_axis: UpAxis::ZUp,
        name: "t".into(),
        objects: vec![],
        materials: vec![],
        assets: vec![],
        meta: Default::default(),
    }
}

/// 类型错参数：{"name": 42} 必须 BadRequest（曾经直接 panic 杀死 MCP）
#[test]
fn wrong_param_type_is_structured_error() {
    let reg = reg();
    let mut scene = empty_scene();
    let err = reg
        .execute(&mut scene, "object.create_group", &json!({"name": 42}))
        .unwrap_err();
    assert!(matches!(err, OpError::BadRequest { .. }), "{err:?}");
    assert!(scene.objects.is_empty(), "坏输入不得污染场景");
}

/// 未知参数拒绝（拼写错误当场暴露）
#[test]
fn unknown_param_rejected() {
    let reg = reg();
    let mut scene = empty_scene();
    let err = reg
        .execute(&mut scene, "object.create_group", &json!({"name": "x", "bogus": 1}))
        .unwrap_err();
    assert!(matches!(err, OpError::BadRequest { .. }));
}

/// 缺必填参数
#[test]
fn missing_required_param() {
    let reg = reg();
    let mut scene = empty_scene();
    let err = reg
        .execute(&mut scene, "object.create_group", &json!({}))
        .unwrap_err();
    assert!(matches!(err, OpError::BadRequest { .. }));
}

/// args 非对象
#[test]
fn args_must_be_object() {
    let reg = reg();
    let mut scene = empty_scene();
    let err = reg
        .execute(&mut scene, "object.create_group", &json!(42))
        .unwrap_err();
    assert!(matches!(err, OpError::BadRequest { .. }));
}

/// ID 不复用（审计 #6 回归锁）：删除最高 id 后新建也不复用
#[test]
fn ids_never_reused_even_after_delete() {
    let reg = reg();
    let mut scene = empty_scene();
    reg.execute(&mut scene, "object.create_group", &json!({"name": "A"}))
        .unwrap();
    reg.execute(&mut scene, "object.create_group", &json!({"name": "B"}))
        .unwrap();
    reg.execute(&mut scene, "object.delete", &json!({"id": "obj-2"}))
        .unwrap();
    let r = reg
        .execute(&mut scene, "object.create_group", &json!({"name": "C"}))
        .unwrap();
    assert_eq!(r.changed[0], "obj-3");
}

/// sync_id_seqs（审计 #6）：外部场景无 seq 元数据时补到 ≥ 现存最大 id
#[test]
fn sync_seqs_prevents_reuse_after_load() {
    let reg = reg();
    let mut scene = empty_scene();
    reg.execute(&mut scene, "object.create_group", &json!({"name": "A"}))
        .unwrap();
    reg.execute(&mut scene, "object.create_group", &json!({"name": "B"}))
        .unwrap();
    // 模拟 load_scene：meta 丢失（无 id_seq）
    scene.meta.clear();
    archcore_ops::sync_id_seqs(&mut scene);
    reg.execute(&mut scene, "object.delete", &json!({"id": "obj-2"}))
        .unwrap();
    let r = reg
        .execute(&mut scene, "object.create_group", &json!({"name": "C"}))
        .unwrap();
    assert_eq!(r.changed[0], "obj-3", "load 后必须继续单调，不得复用 obj-2");
}

/// FNV-1a 稳定哈希锚点值（审计 #7：算法换代时此测试必须显式红，禁止静默漂移）
#[test]
fn fnv1a64_known_vectors() {
    assert_eq!(archcore_ops::fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(archcore_ops::fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
}
