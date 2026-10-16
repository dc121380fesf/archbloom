//! GLB 容器分支 + 畸形输入负例 + 往返数值断言（审计 #2/#15①②③）
use archcore_io::{export_gltf, import_gltf};
use archcore_ir::{Geometry, Object, ObjectKind, Scene, Transform, Units, UpAxis};

fn build_glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut j = json.as_bytes().to_vec();
    while !j.len().is_multiple_of(4) {
        j.push(b' ');
    }
    let mut b = bin.to_vec();
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    let total = 12 + 8 + j.len() + if b.is_empty() { 0 } else { 8 + b.len() };
    let mut out = Vec::new();
    out.extend_from_slice(&0x4654_6C67u32.to_le_bytes()); // 'glTF'
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(j.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // 'JSON'
    out.extend_from_slice(&j);
    if !b.is_empty() {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // 'BIN\0'
        out.extend_from_slice(&b);
    }
    out
}

const TRI_JSON: &str = r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}],"bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36}],"buffers":[{"byteLength":36}]}"#;

fn tri_bin() -> Vec<u8> {
    let verts: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let mut out = Vec::new();
    for v in verts {
        for c in v {
            out.extend_from_slice(&c.to_le_bytes());
        }
    }
    out
}

/// GLB 容器分支（审计 #15①：此前零覆盖）
#[test]
fn glb_container_branch_parses() {
    let glb = build_glb(TRI_JSON, &tri_bin());
    let scene = import_gltf(&glb).expect("GLB 应解析成功");
    assert_eq!(scene.objects.len(), 1);
    let ObjectKind::Mesh { geometry, .. } = &scene.objects[0].kind else {
        panic!("应为网格");
    };
    assert!(geometry.positions.len() >= 3);
    assert!(!geometry.indices.is_empty());
}

/// 截断 GLB：显式 Format 错误，不 panic（审计 #2）
#[test]
fn truncated_glb_errors_not_panics() {
    let err = import_gltf(b"glTF\x02\x00\x00\x00\xff\xff\xff\xff\x00").unwrap_err();
    let _ = err; // 任意 IoError 即可，关键：没 panic
}

/// 越界 bufferView 索引：显式错误，不 panic（审计 #2）
#[test]
fn oob_accessor_errors_not_panics() {
    let bad = TRI_JSON.replace(r#""bufferView":0"#, r#""bufferView":5"#);
    let glb = build_glb(&bad, &tri_bin());
    let err = import_gltf(&glb).unwrap_err();
    let _ = err;
}

/// 往返数值断言（审计 #15②：「全保真」必须有回归锁；导出 f32，容差 1e-5）
#[test]
fn round_trip_preserves_numbers() {
    let mut scene = Scene {
        schema_version: "0.1".into(),
        units: Units::Mm,
        up_axis: UpAxis::ZUp,
        name: "rt".into(),
        objects: vec![Object {
            id: "obj-1".into(),
            name: "板".into(),
            kind: ObjectKind::Mesh {
                geometry: Geometry {
                    positions: vec![[0.25, 0.5, 0.75], [1.25, 0.5, 0.75], [0.25, 1.5, 0.75]],
                    indices: vec![0, 2, 1],
                    normals: None,
                    uv_sets: vec![],
                },
                material_slots: vec![],
            },
            parent: None,
            transform: Transform {
                translation: [1.5, -2.25, 3.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                ..Default::default()
            },
            visible: true,
            meta: Default::default(),
        }],
        materials: vec![],
        assets: vec![],
        meta: Default::default(),
    };
    scene.meta.insert("id_seq".into(), "1".into());
    let data = export_gltf(&scene);
    let back = import_gltf(&data).expect("导出物必须可回读");
    assert_eq!(back.objects.len(), 1);
    let ObjectKind::Mesh { geometry, .. } = &back.objects[0].kind else {
        panic!("应为网格");
    };
    let src = &scene.objects[0];
    let ObjectKind::Mesh { geometry: g0, .. } = &src.kind else {
        panic!();
    };
    for (a, b) in g0.positions.iter().zip(geometry.positions.iter()) {
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 1e-5, "顶点漂移 {a:?} vs {b:?}");
        }
    }
    let t = back.objects[0].transform.translation;
    for k in 0..3 {
        assert!((t[k] - src.transform.translation[k]).abs() < 1e-5, "变换漂移 {t:?}");
    }
}
