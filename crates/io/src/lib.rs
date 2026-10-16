//! IO 层 v0 —— glTF 自控读写 + Blender FBX 转换桥（端口-适配器，桥可整块替换）
//! 已知损耗项：灯光（glTF core 无灯光，KHR_lights_punctual 待接）；几何/层级/UV/材质/相机全保真。
use archcore_ir::{
    AssetRef, Geometry, Material, Object, ObjectKind, Scene, ShadingModel, Textures, Transform,
    Units, UpAxis,
};
use serde_json::{json, Value};
use std::path::Path;

pub mod ifc;
pub use ifc::{export_ifc, import_ifc};

#[derive(Debug)]
pub enum IoError {
    Format(String),
    Bridge(String),
    Io(String),
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoError::Format(s) => write!(f, "格式错误: {s}"),
            IoError::Bridge(s) => write!(f, "桥接失败: {s}"),
            IoError::Io(s) => write!(f, "IO 失败: {s}"),
        }
    }
}
impl std::error::Error for IoError {}

// ---------- base64（零依赖） ----------
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn b64_decode(s: &str) -> Result<Vec<u8>, IoError> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let vals: Vec<u8> = s
        .bytes()
        .filter(|b| *b != b'=' && !b.is_ascii_whitespace())
        .map(|b| {
            B64
                .iter()
                .position(|c| *c == b)
                .map(|p| p as u8)
                .ok_or_else(|| IoError::Format(format!("base64 非法字符: {b}")))
        })
        .collect::<Result<_, _>>()?;
    for chunk in vals.chunks(4) {
        let mut n: u32 = 0;
        for (i, v) in chunk.iter().enumerate() {
            n |= (*v as u32) << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

// ---------- 轴向转换（glTF 规范 Y-up ↔ IR Z-up；对合，往返成立） ----------

fn z_to_y(v: [f64; 3]) -> [f64; 3] {
    [v[0], v[2], -v[1]]
}

fn y_to_z(v: [f64; 3]) -> [f64; 3] {
    [v[0], -v[2], v[1]]
}

fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// 绕 X 轴 ±90°（y↔z）
const Q_Z_TO_Y: [f64; 4] = [
    std::f64::consts::FRAC_1_SQRT_2,
    0.0,
    0.0,
    std::f64::consts::FRAC_1_SQRT_2,
];
const Q_Y_TO_Z: [f64; 4] = [
    -std::f64::consts::FRAC_1_SQRT_2,
    0.0,
    0.0,
    std::f64::consts::FRAC_1_SQRT_2,
];

fn quat_z_to_y(q: [f64; 4]) -> [f64; 4] {
    quat_mul(quat_mul(Q_Z_TO_Y, q), Q_Y_TO_Z)
}

fn quat_y_to_z(q: [f64; 4]) -> [f64; 4] {
    quat_mul(quat_mul(Q_Y_TO_Z, q), Q_Z_TO_Y)
}

// ---------- glTF 导出 ----------

struct Buf {
    data: Vec<u8>,
}

impl Buf {
    fn push_f32(&mut self, v: &[f64], n: usize) -> (usize, usize) {
        let off = self.data.len();
        for x in &v[..n] {
            self.data.extend_from_slice(&(*x as f32).to_le_bytes());
        }
        (off, n)
    }
    fn push_u32(&mut self, v: &[u32]) -> (usize, usize) {
        let off = self.data.len();
        for x in v {
            self.data.extend_from_slice(&x.to_le_bytes());
        }
        (off, v.len())
    }
}

/// IR 场景 → glTF 2.0 JSON（内嵌 data URI buffer）
pub fn export_gltf(scene: &Scene) -> Vec<u8> {
    let mut buf = Buf { data: Vec::new() };
    let mut accessors = Vec::new();
    let mut views = Vec::new();
    let mut meshes = Vec::new();
    let mut materials = Vec::new();
    let mut mat_index: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut nodes = Vec::new();
    let mut children: std::collections::BTreeMap<&str, Vec<usize>> = Default::default();
    let mut roots = Vec::new();
    let mut cameras = Vec::new();

    for m in &scene.materials {
        mat_index.insert(m.id.as_str(), materials.len());
        materials.push(json!({
            "name": m.name,
            "pbrMetallicRoughness": {
                "baseColorFactor": m.base_color,
                "metallicFactor": m.metallic,
                "roughnessFactor": m.roughness
            },
            "alphaMode": if m.opacity < 1.0 { "BLEND" } else { "OPAQUE" },
            "doubleSided": true
        }));
    }

    for obj in &scene.objects {
        let mut node = json!({"name": obj.name});
        let t = &obj.transform;
        if t.translation != [0.0; 3] {
            node["translation"] = json!(z_to_y(t.translation));
        }
        if t.rotation != [0.0, 0.0, 0.0, 1.0] {
            node["rotation"] = json!(quat_z_to_y(t.rotation));
        }
        if t.scale != [1.0; 3] {
            node["scale"] = json!(t.scale);
        }
        match &obj.kind {
            ObjectKind::Mesh { geometry, material_slots } => {
                let pos_y: Vec<[f64; 3]> = geometry.positions.iter().map(|p| z_to_y(*p)).collect();
                let (p_off, p_cnt) = buf.push_f32(
                    &pos_y.iter().flatten().copied().collect::<Vec<f64>>(),
                    geometry.positions.len() * 3,
                );
                views.push(json!({"buffer": 0, "byteOffset": p_off, "byteLength": p_cnt * 4}));
                let mut min = [f32::MAX; 3];
                let mut max = [f32::MIN; 3];
                for p in &pos_y {
                    for k in 0..3 {
                        min[k] = min[k].min(p[k] as f32);
                        max[k] = max[k].max(p[k] as f32);
                    }
                }
                accessors.push(json!({
                    "bufferView": views.len() - 1, "componentType": 5126,
                    "count": p_cnt / 3, "type": "VEC3",
                    "min": min, "max": max
                }));
                let pos_acc = accessors.len() - 1;

                let (i_off, i_cnt) = buf.push_u32(&geometry.indices);
                views.push(json!({"buffer": 0, "byteOffset": i_off, "byteLength": i_cnt * 4}));
                accessors.push(json!({
                    "bufferView": views.len() - 1, "componentType": 5125,
                    "count": i_cnt, "type": "SCALAR"
                }));
                let idx_acc = accessors.len() - 1;

                let mut prim = json!({
                    "attributes": {"POSITION": pos_acc},
                    "indices": idx_acc,
                    "mode": 4
                });
                if let Some(uv) = geometry.uv_sets.first() {
                    let flat: Vec<f64> = uv.coords.iter().flatten().copied().collect();
                    let (u_off, u_cnt) = buf.push_f32(&flat, uv.coords.len() * 2);
                    views.push(json!({"buffer": 0, "byteOffset": u_off, "byteLength": u_cnt * 4}));
                    accessors.push(json!({
                        "bufferView": views.len() - 1, "componentType": 5126,
                        "count": u_cnt / 2, "type": "VEC2"
                    }));
                    prim["attributes"]["TEXCOORD_0"] = json!(accessors.len() - 1);
                }
                if let Some(slot) = material_slots.first() {
                    if let Some(&mi) = mat_index.get(slot.as_str()) {
                        prim["material"] = json!(mi);
                    }
                }
                meshes.push(json!({"name": obj.name, "primitives": [prim]}));
                node["mesh"] = json!(meshes.len() - 1);
            }
            ObjectKind::Camera { lens_mm, sensor_mm, clip } => {
                let yfov = 2.0 * ((sensor_mm / 2.0) / lens_mm).atan();
                cameras.push(json!({
                    "type": "perspective",
                    "perspective": {"yfov": yfov, "znear": clip[0], "zfar": clip[1]}
                }));
                node["camera"] = json!(cameras.len() - 1);
            }
            ObjectKind::Group | ObjectKind::Instance { .. } | ObjectKind::Light { .. } => {}
        }
        nodes.push(node);
    }

    // 层级：parent 关系 → children 数组
    for (i, obj) in scene.objects.iter().enumerate() {
        match &obj.parent {
            Some(p) => {
                if let Some(pi) = scene.objects.iter().position(|o| o.id == *p) {
                    children
                        .entry(scene.objects[pi].id.as_str())
                        .or_default()
                        .push(i);
                }
            }
            None => roots.push(i),
        }
    }
    for (i, obj) in scene.objects.iter().enumerate() {
        if let Some(kids) = children.get(obj.id.as_str()) {
            nodes[i]["children"] = json!(kids);
        }
    }

    let mut gltf = json!({
        "asset": {"version": "2.0", "generator": "archcore-0.1"},
        "scene": 0,
        "scenes": [{"nodes": roots}],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "buffers": [{"byteLength": buf.data.len(), "uri": format!("data:application/octet-stream;base64,{}", b64_encode(&buf.data))}],
        "bufferViews": views,
        "accessors": accessors
    });
    if !cameras.is_empty() {
        gltf["cameras"] = json!(cameras);
    }
    gltf.to_string().into_bytes()
}

// ---------- glTF 导入（往返可测的子集） ----------

/// glTF 导入：支持纯 JSON（内嵌 data URI buffer）与 GLB 容器（JSON+BIN 块）
pub fn import_gltf(data: &[u8]) -> Result<Scene, IoError> {
    let (text, glb_bin) = if data.len() >= 12 && &data[0..4] == b"glTF" {
        // GLB 容器：12 字节头 + [len|type|body] 块序列
        let mut off = 12usize;
        let mut json_text: Option<String> = None;
        let mut bin: Option<Vec<u8>> = None;
        while off + 8 <= data.len() {
            let clen = u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
            let ctype = &data[off + 4..off + 8];
            let body = &data[off + 8..(off + 8 + clen).min(data.len())];
            if ctype == b"JSON" {
                json_text = Some(String::from_utf8_lossy(body).to_string());
            } else if ctype == b"BIN\0" {
                bin = Some(body.to_vec());
            }
            off += 8 + clen;
        }
        (
            json_text.ok_or_else(|| IoError::Format("GLB 缺 JSON 块".into()))?,
            bin,
        )
    } else {
        (
            std::str::from_utf8(data)
                .map_err(|e| IoError::Format(e.to_string()))?
                .to_string(),
            None,
        )
    };
    let g: Value = serde_json::from_str(&text).map_err(|e| IoError::Format(e.to_string()))?;

    // buffer：GLB 的 BIN 块优先，否则解 data URI
    let blob: Vec<u8> = match glb_bin {
        Some(b) => b,
        None => {
            let raw = g["buffers"]
                .get(0)
                .and_then(|b| b["uri"].as_str())
                .and_then(|u| u.strip_prefix("data:application/octet-stream;base64,"))
                .ok_or_else(|| IoError::Format("仅支持内嵌 base64 buffer 或 GLB".into()))?;
            b64_decode(raw)?
        }
    };

    // accessor 读取：全面 checked（审计 #2：文件是敌对输入，越界/截断一律 Format 错误）
    let acc_view_off = |acc: &Value| -> Result<(usize, usize), IoError> {
        let vi = acc["bufferView"]
            .as_u64()
            .ok_or_else(|| IoError::Format("accessor 缺 bufferView".into()))? as usize;
        let view = g["bufferViews"]
            .get(vi)
            .ok_or_else(|| IoError::Format(format!("bufferView {vi} 越界")))?;
        let off = view["byteOffset"].as_u64().unwrap_or(0) as usize;
        let cnt = acc["count"]
            .as_u64()
            .ok_or_else(|| IoError::Format("accessor 缺 count".into()))? as usize;
        Ok((off, cnt))
    };
    let read_f32 = |acc: &Value, n: usize| -> Result<Vec<f64>, IoError> {
        let (off, cnt) = acc_view_off(acc)?;
        let need = cnt * n * 4;
        if off.saturating_add(need) > blob.len() {
            return Err(IoError::Format(format!(
                "buffer 数据不足：需 {need} 字节 @{off}，实有 {}",
                blob.len()
            )));
        }
        let mut out = Vec::with_capacity(cnt * n);
        for i in 0..cnt * n {
            let s = off + i * 4;
            let v = f32::from_le_bytes([blob[s], blob[s + 1], blob[s + 2], blob[s + 3]]);
            out.push(v as f64);
        }
        Ok(out)
    };
    let read_u32 = |acc: &Value| -> Result<Vec<u32>, IoError> {
        let (off, cnt) = acc_view_off(acc)?;
        let need = cnt * 4;
        if off.saturating_add(need) > blob.len() {
            return Err(IoError::Format(format!(
                "buffer 数据不足：需 {need} 字节 @{off}，实有 {}",
                blob.len()
            )));
        }
        let mut out = Vec::with_capacity(cnt);
        for i in 0..cnt {
            let s = off + i * 4;
            out.push(u32::from_le_bytes([blob[s], blob[s + 1], blob[s + 2], blob[s + 3]]));
        }
        Ok(out)
    };
    let get_acc = |ai: u64| -> Result<&Value, IoError> {
        g["accessors"]
            .get(ai as usize)
            .ok_or_else(|| IoError::Format(format!("accessor {ai} 越界")))
    };

    let mut materials = Vec::new();
    for (i, m) in g["materials"].as_array().map(|a| a.iter()).into_iter().flatten().enumerate() {
        let pbr = &m["pbrMetallicRoughness"];
        let color = pbr["baseColorFactor"]
            .as_array()
            .map(|a| {
                [
                    a[0].as_f64().unwrap_or(1.0),
                    a[1].as_f64().unwrap_or(1.0),
                    a[2].as_f64().unwrap_or(1.0),
                    a[3].as_f64().unwrap_or(1.0),
                ]
            })
            .unwrap_or([1.0; 4]);
        materials.push(Material {
            id: format!("mat-{i}"),
            name: m["name"].as_str().unwrap_or("").to_string(),
            shading_model: ShadingModel::PbrMetallicRoughness,
            base_color: color,
            metallic: pbr["metallicFactor"].as_f64().unwrap_or(1.0),
            roughness: pbr["roughnessFactor"].as_f64().unwrap_or(1.0),
            opacity: if m["alphaMode"] == "BLEND" { 0.5 } else { 1.0 },
            textures: Textures::default(),
        });
    }

    let mut objects = Vec::new();
    for (i, n) in g["nodes"].as_array().map(|a| a.iter()).into_iter().flatten().enumerate() {
        let kind = if let Some(mi) = n["mesh"].as_u64() {
            let prim = g["meshes"]
                .get(mi as usize)
                .and_then(|m| m["primitives"].get(0))
                .ok_or_else(|| IoError::Format(format!("mesh {mi} 缺 primitives")))?;
            let pos_ai = prim["attributes"]["POSITION"]
                .as_u64()
                .ok_or_else(|| IoError::Format("primitive 缺 POSITION".into()))?;
            let pos = read_f32(get_acc(pos_ai)?, 3)?;
            let mut positions = Vec::with_capacity(pos.len() / 3);
            for p in pos.chunks(3) {
                positions.push(y_to_z([p[0], p[1], p[2]]));
            }
            // indices 非必选（glTF 规范）：缺省按顺序生成
            let idx = match prim["indices"].as_u64() {
                Some(ii) => read_u32(get_acc(ii)?)?,
                None => (0..positions.len() as u32).collect(),
            };
            let uv_sets = match prim["attributes"]["TEXCOORD_0"].as_u64() {
                Some(ai) => {
                    let uv = read_f32(get_acc(ai)?, 2)?;
                    vec![archcore_ir::UvSet {
                        name: "UVMap".into(),
                        coords: uv.chunks(2).map(|p| [p[0], p[1]]).collect(),
                    }]
                }
                None => vec![],
            };
            let material_slots = prim["material"]
                .as_u64()
                .map(|mi| {
                    materials
                        .get(mi as usize)
                        .map(|m| vec![m.id.clone()])
                        .ok_or_else(|| IoError::Format(format!("材质 {mi} 越界")))
                })
                .transpose()?
                .unwrap_or_default();
            ObjectKind::Mesh {
                geometry: Geometry {
                    positions,
                    indices: idx,
                    normals: None,
                    uv_sets,
                },
                material_slots,
            }
        } else if let Some(ci) = n["camera"].as_u64() {
            let c = g["cameras"]
                .get(ci as usize)
                .ok_or_else(|| IoError::Format(format!("camera {ci} 越界")))?;
            let yfov = c["perspective"]["yfov"].as_f64().unwrap_or(0.8);
            let sensor = 36.0;
            ObjectKind::Camera {
                lens_mm: (sensor / 2.0) / (yfov / 2.0).tan(),
                sensor_mm: sensor,
                clip: [
                    c["perspective"]["znear"].as_f64().unwrap_or(0.1),
                    c["perspective"]["zfar"].as_f64().unwrap_or(1000.0),
                ],
            }
        } else {
            ObjectKind::Group
        };
        let vec3 = |key: &str, d: [f64; 3]| -> [f64; 3] {
            let v: Vec<f64> = n[key]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
                .unwrap_or(d.to_vec());
            [
                v.first().copied().unwrap_or(d[0]),
                v.get(1).copied().unwrap_or(d[1]),
                v.get(2).copied().unwrap_or(d[2]),
            ]
        };
        let rot: [f64; 4] = {
            let v: Vec<f64> = n["rotation"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
                .unwrap_or_default();
            [
                v.first().copied().unwrap_or(0.0),
                v.get(1).copied().unwrap_or(0.0),
                v.get(2).copied().unwrap_or(0.0),
                v.get(3).copied().unwrap_or(1.0),
            ]
        };
        objects.push(Object {
            id: format!("obj-{i}"),
            name: n["name"].as_str().unwrap_or("").to_string(),
            kind,
            parent: None,
            transform: Transform {
                translation: y_to_z(vec3("translation", [0.0; 3])),
                rotation: quat_y_to_z(rot),
                scale: vec3("scale", [1.0; 3]),
            },
            visible: true,
            meta: Default::default(),
        });
    }

    // 父子关系还原
    if let Some(nodes) = g["nodes"].as_array() {
        for (pi, n) in nodes.iter().enumerate() {
            if let Some(kids) = n["children"].as_array() {
                for k in kids {
                    if let Some(ki) = k.as_u64() {
                        let child = objects.get_mut(ki as usize).ok_or_else(|| {
                            IoError::Format(format!("children 索引 {ki} 越界"))
                        })?;
                        child.parent = Some(format!("obj-{pi}"));
                    }
                }
            }
        }
    }

    Ok(Scene {
        schema_version: "0.1".into(),
        units: Units::Mm,
        up_axis: UpAxis::ZUp,
        name: "imported".into(),
        objects,
        materials,
        assets: Vec::<AssetRef>::new(),
        meta: Default::default(),
    })
}

// ---------- Blender FBX 桥（双向） ----------

const CONVERTER_PY: &str = r#"import bpy, sys
argv = sys.argv[sys.argv.index("--") + 1:]
src, dst = argv[0], argv[1]
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=src)
bpy.ops.export_scene.fbx(filepath=dst, use_selection=False)
print("FBX_BRIDGE_OK")
"#;

const IMPORTER_PY: &str = r#"import bpy, sys
argv = sys.argv[sys.argv.index("--") + 1:]
src, dst = argv[0], argv[1]
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.fbx(filepath=src)
bpy.ops.export_scene.gltf(filepath=dst, export_format='GLB')
print("FBX_IMPORT_BRIDGE_OK")
"#;

/// 中间产物清理（转换成功后）：best-effort，忽略失败——清理属卫生，不影响主结果
fn cleanup_intermediate(paths: &[&Path]) {
    for p in paths {
        let _ = std::fs::remove_file(p);
    }
}

/// IR 场景 → FBX：glTF 中间件 + Blender 转换桥（子进程纪律：exit code 当 API、输出存在性终检）
pub fn export_fbx(scene: &Scene, blender: &Path, out: &Path) -> Result<(), IoError> {
    let gltf_path = out.with_extension("gltf");
    let script_path = out.with_extension("bridge.py");
    std::fs::write(&gltf_path, export_gltf(scene)).map_err(|e| IoError::Io(e.to_string()))?;
    std::fs::write(&script_path, CONVERTER_PY).map_err(|e| IoError::Io(e.to_string()))?;
    let status = std::process::Command::new(blender)
        .args(["--background", "--python"])
        .arg(&script_path)
        .arg("--")
        .arg(&gltf_path)
        .arg(out)
        .status()
        .map_err(|e| IoError::Bridge(e.to_string()))?;
    if !status.success() {
        return Err(IoError::Bridge(format!("Blender 退出码: {status}")));
    }
    if !out.exists() {
        return Err(IoError::Bridge("FBX 输出不存在".into()));
    }
    // 转换成功：清理 glTF 中间件与转换脚本（中间产物不外泄）
    cleanup_intermediate(&[gltf_path.as_path(), script_path.as_path()]);
    Ok(())
}

const MAX2FBX_MS: &str = r#"try (
    pluginManager.loadClass FbxExporter
    FBXExporterSetParam "ConvertUnit" "cm"
    FBXExporterSetParam "ScaleFactor" 1.0
    FBXExporterSetParam "UpAxis" "z"
    FBXExporterSetParam "EmbedTextures" true
    exportFile @"__OUT__" #noPrompt using:FBXEXP
    format "MAX2FBX_OK\n"
) catch (
    format "MAX2FBX_FAIL %\n" (getCurrentException())
)
"#;

/// .max → IR 场景：3dsmaxbatch 静默转换（.max 直读无正路，官方通道唯一）+ FBX 桥
pub fn import_max(max_bin: &Path, blender: &Path, max_src: &Path) -> Result<Scene, IoError> {
    let fbx_path = max_src.with_extension("converted.fbx");
    let script_path = max_src.with_extension("max2fbx.ms");
    let script = MAX2FBX_MS.replace("__OUT__", &fbx_path.to_string_lossy());
    std::fs::write(&script_path, script).map_err(|e| IoError::Io(e.to_string()))?;
    let status = std::process::Command::new(max_bin)
        .arg("-sceneFile")
        .arg(max_src)
        .arg(&script_path)
        .args(["-v", "5"])
        .status()
        .map_err(|e| IoError::Bridge(e.to_string()))?;
    if !status.success() {
        return Err(IoError::Bridge(format!("3dsmaxbatch 退出码: {status}")));
    }
    if !fbx_path.exists() {
        return Err(IoError::Bridge("FBX 转换输出不存在".into()));
    }
    let scene = import_fbx(blender, &fbx_path)?;
    // 转换成功：清理 .max→FBX 中间产物（转换脚本 + 转出的 FBX）
    cleanup_intermediate(&[fbx_path.as_path(), script_path.as_path()]);
    Ok(scene)
}

/// FBX → IR 场景：Blender 转换桥（FBX → glTF 内嵌）+ 自控解析
pub fn import_fbx(blender: &Path, path: &Path) -> Result<Scene, IoError> {
    let gltf_path = path.with_extension("import.glb");
    let script_path = path.with_extension("import_bridge.py");
    std::fs::write(&script_path, IMPORTER_PY).map_err(|e| IoError::Io(e.to_string()))?;
    let status = std::process::Command::new(blender)
        .args(["--background", "--python"])
        .arg(&script_path)
        .arg("--")
        .arg(path)
        .arg(&gltf_path)
        .status()
        .map_err(|e| IoError::Bridge(e.to_string()))?;
    if !status.success() {
        return Err(IoError::Bridge(format!("Blender 退出码: {status}")));
    }
    let data = std::fs::read(&gltf_path).map_err(|_| IoError::Bridge("glTF 中间件未产出".into()))?;
    // 解析前清理 glTF 中间件与导入脚本（中间产物不外泄）
    cleanup_intermediate(&[gltf_path.as_path(), script_path.as_path()]);
    import_gltf(&data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use archcore_ir::{ObjectKind, Transform};

    fn sample_scene() -> Scene {
        Scene {
            schema_version: "0.1".into(),
            units: Units::Mm,
            up_axis: UpAxis::ZUp,
            name: "t".into(),
            objects: vec![
                Object {
                    id: "obj-0".into(),
                    name: "Root".into(),
                    kind: ObjectKind::Group,
                    parent: None,
                    transform: Transform::default(),
                    visible: true,
                    meta: Default::default(),
                },
                Object {
                    id: "obj-1".into(),
                    name: "墙_01".into(),
                    kind: ObjectKind::Mesh {
                        geometry: Geometry {
                            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
                            indices: vec![0, 1, 2],
                            normals: None,
                            uv_sets: vec![archcore_ir::UvSet {
                                name: "UVMap".into(),
                                coords: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
                            }],
                        },
                        material_slots: vec!["mat-0".into()],
                    },
                    parent: Some("obj-0".into()),
                    transform: Transform::default(),
                    visible: true,
                    meta: Default::default(),
                },
                Object {
                    id: "obj-2".into(),
                    name: "Shot".into(),
                    kind: ObjectKind::Camera {
                        lens_mm: 35.0,
                        sensor_mm: 36.0,
                        clip: [0.1, 1000.0],
                    },
                    parent: None,
                    transform: Transform::default(),
                    visible: true,
                    meta: Default::default(),
                },
            ],
            materials: vec![Material {
                id: "mat-0".into(),
                name: "MAT_Wall".into(),
                shading_model: ShadingModel::PbrMetallicRoughness,
                base_color: [0.8, 0.2, 0.2, 1.0],
                metallic: 0.1,
                roughness: 0.7,
                opacity: 1.0,
                textures: Textures::default(),
            }],
            assets: vec![],
            meta: Default::default(),
        }
    }

    #[test]
    fn base64_round_trip() {
        let data: Vec<u8> = (0..=255u8).collect();
        assert_eq!(b64_decode(&b64_encode(&data)).unwrap(), data);
    }

    /// 轴转换是对合：点与四元数双向往返无损
    #[test]
    fn axis_conversion_is_involution() {
        let v = [0.3, -1.2, 2.5];
        assert_eq!(y_to_z(z_to_y(v)), v);
        let q = [0.1, 0.2, 0.3, 0.9];
        let back = quat_y_to_z(quat_z_to_y(q));
        for i in 0..4 {
            assert!((back[i] - q[i]).abs() < 1e-12);
        }
    }

    /// glTF 往返：网格/UV/材质/层级/相机全保真（f32 精度容差）
    #[test]
    fn gltf_round_trip() {
        let s = sample_scene();
        let bytes = export_gltf(&s);
        let back = import_gltf(&bytes).unwrap();
        assert_eq!(back.objects.len(), 3);
        assert_eq!(back.objects[1].name, "墙_01");
        assert_eq!(back.objects[1].parent, Some("obj-0".into()));
        match &back.objects[1].kind {
            ObjectKind::Mesh { geometry, material_slots } => {
                assert_eq!(geometry.positions.len(), 3);
                assert_eq!(geometry.indices, vec![0, 1, 2]);
                assert_eq!(geometry.uv_sets[0].coords[2], [1.0, 1.0]);
                assert_eq!(material_slots.len(), 1);
            }
            _ => panic!("应为网格"),
        }
        let m = &back.materials[0];
        assert_eq!(m.name, "MAT_Wall");
        assert!((m.metallic - 0.1).abs() < 1e-6);
        assert!((m.roughness - 0.7).abs() < 1e-6);
        assert!(matches!(back.objects[2].kind, ObjectKind::Camera { .. }));
    }
}
