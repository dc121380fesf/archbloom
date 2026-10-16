//! 全景/立体全景回归（交付物线）：投影正确性、确定性、SBS 差异
use archcore_ir::{Geometry, Material, Object, ObjectKind, Scene, ShadingModel, Transform, Units, UpAxis};
use archcore_render::{render_panorama, PanoramaOpts};

fn scene() -> Scene {
    Scene {
        schema_version: "0.1".into(),
        units: Units::Mm,
        up_axis: UpAxis::ZUp,
        name: "pano".into(),
        objects: vec![
            Object {
                id: "obj-1".into(),
                name: "板".into(),
                kind: ObjectKind::Mesh {
                    geometry: Geometry {
                        positions: vec![
                            [0.0, 0.0, 0.0],
                            [2.0, 0.0, 0.0],
                            [2.0, 2.0, 0.0],
                            [0.0, 2.0, 0.0],
                        ],
                        indices: vec![0, 2, 1, 0, 3, 2],
                        normals: None,
                        uv_sets: vec![],
                    },
                    material_slots: vec!["mat-0".into()],
                },
                parent: None,
                transform: Transform::default(),
                visible: true,
                meta: Default::default(),
            },
            Object {
                id: "obj-2".into(),
                name: "柱".into(),
                kind: ObjectKind::Mesh {
                    geometry: Geometry {
                        positions: vec![
                            [0.5, 0.5, 0.0],
                            [0.9, 0.5, 0.0],
                            [0.9, 0.9, 0.0],
                            [0.5, 0.9, 0.0],
                            [0.5, 0.5, 1.5],
                            [0.9, 0.5, 1.5],
                            [0.9, 0.9, 1.5],
                            [0.5, 0.9, 1.5],
                        ],
                        indices: [
                            0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 2, 3, 7, 2, 7,
                            6, 1, 2, 6, 1, 6, 5, 0, 4, 7, 0, 7, 3,
                        ]
                        .into(),
                        normals: None,
                        uv_sets: vec![],
                    },
                    material_slots: vec!["mat-0".into()],
                },
                parent: None,
                transform: Transform::default(),
                visible: true,
                meta: Default::default(),
            },
        ],
        materials: vec![Material {
            id: "mat-0".into(),
            name: "M".into(),
            shading_model: ShadingModel::PbrMetallicRoughness,
            base_color: [0.7, 0.5, 0.4, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            opacity: 1.0,
            textures: Default::default(),
        }],
        assets: vec![],
        meta: Default::default(),
    }
}

fn opts(width: u32, stereo: bool) -> PanoramaOpts {
    PanoramaOpts {
        width,
        height: width / 2,
        spp: 3,
        exposure: 1.0,
        stereo,
        ipd: 0.065,
    }
}

/// 单目全景：非平凡 + 确定性
#[test]
fn panorama_renders_and_is_deterministic() {
    let s = scene();
    let a = render_panorama(&s, &opts(64, false)).unwrap();
    let b = render_panorama(&s, &opts(64, false)).unwrap();
    assert_eq!(a, b, "CPU 全景必须确定性");
    let img = image::load_from_memory(&a).unwrap().to_rgb8();
    let mut colors = std::collections::BTreeSet::new();
    for p in img.pixels() {
        colors.insert((p[0] / 32, p[1] / 32, p[2] / 32));
    }
    assert!(colors.len() > 1, "全景不得为纯色");
}

/// 立体 SBS：左右半图必须有差异（双眼视差）
#[test]
fn stereo_halves_differ() {
    let s = scene();
    let png = render_panorama(&s, &opts(64, true)).unwrap();
    let img = image::load_from_memory(&png).unwrap().to_rgb8();
    assert_eq!(img.width(), 64);
    let mut diff = 0u32;
    for y in 0..img.height() {
        for x in 0..32 {
            let l = img.get_pixel(x, y).0;
            let r = img.get_pixel(x + 32, y).0;
            if l != r {
                diff += 1;
            }
        }
    }
    assert!(diff > 0, "SBS 左右眼完全相同 = 视差丢失");
}

/// 立体全景宽度必须偶数（审计同款 fail-closed 风格）
#[test]
fn stereo_odd_width_rejected() {
    let err = render_panorama(&scene(), &opts(63, true)).unwrap_err();
    assert!(err.contains("偶数"), "{err}");
}
