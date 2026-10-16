//! 渲染边界负例（审计 #3/#15③）：0 维/0 spp 必须显式报错，不 panic
use archcore_ir::{Scene, Units, UpAxis};
use archcore_render::{render_pathtraced_exposed, render_to_file};

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

#[test]
fn zero_width_review_errors() {
    let dir = std::env::temp_dir().join("archcore_zero_w.png");
    let err = render_to_file(&empty_scene(), 0, 100, dir.to_str().unwrap()).unwrap_err();
    assert!(err.contains("必须 > 0"), "{err}");
}

#[test]
fn zero_height_pathtrace_errors() {
    let err = render_pathtraced_exposed(&empty_scene(), 64, 0, 4, 1.0).unwrap_err();
    assert!(err.contains("必须 > 0"), "{err}");
}

#[test]
fn zero_spp_errors_not_black() {
    // spp=0 曾静默出黑图（1.0/0.0）——必须显式报错
    let err = render_pathtraced_exposed(&empty_scene(), 64, 48, 0, 1.0).unwrap_err();
    assert!(err.contains("spp"), "{err}");
}

#[test]
fn oversized_review_errors() {
    let dir = std::env::temp_dir().join("archcore_big.png");
    let err = render_to_file(&empty_scene(), 20000, 100, dir.to_str().unwrap()).unwrap_err();
    assert!(err.contains("上限"), "{err}");
}
