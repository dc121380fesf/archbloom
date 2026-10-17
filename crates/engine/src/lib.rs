//! 引擎门面 v0 —— 五砖合一：IR + op 注册表 + 事务 + 验证回路 + 拓扑内核
//!
//! 核心语义：[`Engine::execute`] = 执行 + 验证闸 + **拦截自动回滚**。
//! 验证不过的操作不但拒绝，还从撤销历史抹掉，场景回到操作前——宪法条款的执行级落地。
use archcore_ir::{
    AssetRef, Geometry, Id, Material, Object, ObjectKind, Scene, ShadingModel, Transform, Units, UpAxis,
};
use archcore_mesh::{BoolOp, Mesh, MeshError};
use archcore_ops::{
    register_builtins, CapabilityManifest, GeoFacts, OpAnnotations, OpCategory, OpDef, OpError,
    OpRegistry, OpReport, OpSpec, ParamSpec,
};
use archcore_txn::History;
use archcore_verify::{default_registry, Finding, VerifyRegistry};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug)]
pub enum EngineError {
    Op(OpError),
    Verify(Vec<Finding>),
    /// 修订冲突：审阅期间文档已被修改，提交必须拒绝（丢失更新防线）
    Conflict { expected: u64, actual: u64 },
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Op(e) => write!(f, "op 失败: {e}"),
            EngineError::Verify(fs) => {
                write!(f, "验证不过（{} 条发现）", fs.len())?;
                for x in fs.iter().take(3) {
                    write!(f, "; [{}] {}", x.checker, x.message)?;
                }
                Ok(())
            }
            EngineError::Conflict { expected, actual } => {
                write!(f, "修订冲突: 期望 {expected}，实际 {actual}")
            }
        }
    }
}
impl std::error::Error for EngineError {}

/// 预览摘要：交给人审的三件套——操作报告 + 事实变化 + 验证结果
#[derive(Clone, Debug, serde::Serialize)]
pub struct PreviewSummary {
    pub id: String,
    pub label: String,
    pub base_revision: u64,
    pub reports: Vec<OpReport>,
    pub facts_before: GeoFacts,
    pub facts_after: GeoFacts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewStatus {
    Pending,
    Committed,
    Rejected,
}

struct PreviewRecord {
    summary: PreviewSummary,
    scene_after: Scene,
    status: PreviewStatus,
}

pub struct Engine {
    pub scene: Scene,
    /// 文档修订号：每次成功提交 +1（preview 的基线校验锚点）
    pub revision: u64,
    registry: OpRegistry,
    history: History,
    verify: VerifyRegistry,
    previews: BTreeMap<String, PreviewRecord>,
    preview_seq: u64,
}

/// 稳定 ID 分配（委托 ops 的单调计数器，永不复用）
fn next_id(scene: &mut Scene) -> String {
    archcore_ops::next_id(scene)
}

/// 输出父目录自建（直出工具不能让渲染白跑——目录缺失当场补，路径非法显式报错）
fn ensure_parent(path: &str) -> Result<(), OpError> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| OpError::Internal {
                reason: format!("创建输出目录失败 {parent:?}: {e}"),
            })?;
        }
    }
    Ok(())
}

/// 渲染尺寸/采样钳制：越界显式拒绝（审计 #3：0 维 panic、巨维溢出的封口）
fn dims_clamp(args: &Value, name: &str, default: u32, lo: u32, hi: u32) -> Result<u32, OpError> {
    let v = match args.get(name) {
        None | Some(Value::Null) => default,
        Some(v) => {
            let n = v.as_u64().ok_or_else(|| OpError::BadRequest {
                param: name.into(),
                reason: "应为非负整数".into(),
            })?;
            u32::try_from(n).map_err(|_| OpError::BadRequest {
                param: name.into(),
                reason: format!("超出范围：{n}"),
            })?
        }
    };
    if !(lo..=hi).contains(&v) {
        return Err(OpError::BadRequest {
            param: name.into(),
            reason: format!("应在 {lo}..={hi}，实为 {v}"),
        });
    }
    Ok(v)
}

fn get_mesh(scene: &Scene, id: &str) -> Result<Geometry, OpError> {
    scene
        .objects
        .iter()
        .find(|o| o.id == id)
        .and_then(|o| match &o.kind {
            ObjectKind::Mesh { geometry, .. } => Some(geometry.clone()),
            _ => None,
        })
        .ok_or_else(|| OpError::NotFound {
            what: "mesh".to_string(),
            id: id.to_string(),
        })
}

fn put_mesh(scene: &mut Scene, id: &str, positions: Vec<[f64; 3]>, indices: Vec<u32>) -> Result<(), OpError> {
    // 写回前几何健全性回检（审计 #5）：坏几何不得静默入 IR
    archcore_mesh::Mesh::from_indexed(positions.clone(), &indices).map_err(|e| OpError::BadRequest {
        param: "geometry".into(),
        reason: format!("写回几何不健全: {e:?}"),
    })?;
    if let Some(o) = scene.objects.iter_mut().find(|o| o.id == id) {
        if let ObjectKind::Mesh { geometry, .. } = &mut o.kind {
            geometry.positions = positions;
            geometry.indices = indices;
            // 顶点数变了旧 uv 必错位（glTF 导出曾因此被 Blender 拒收）：拓扑编辑一律清 uv，
            // 需要贴图的调用方（如 opening.cut）自行重投影
            geometry.uv_sets.clear();
        }
    }
    Ok(())
}

fn mesh_err(e: MeshError) -> OpError {
    match e {
        MeshError::NonManifoldEdge { x, y } => OpError::Precondition {
            reason: format!("非流形边 ({x},{y})"),
        },
        MeshError::EmptyResult => OpError::Precondition {
            reason: "布尔结果为空（无交或一方全吞噬）".into(),
        },
        MeshError::UnchangedResult => OpError::Precondition {
            reason: "布尔未产生几何变化（打空/无交）——静默打空禁止".into(),
        },
        other => OpError::BadRequest {
            param: "geometry".to_string(),
            reason: format!("{other:?}"),
        },
    }
}

/// 导入场景合流：材质合流（旧 id -> 新 id）+ 对象 id 重排 + 父子/材质引用同步重映射。
/// import_fbx / import_max 共用（消除重复块）。
fn merge_imported(scene: &mut Scene, imported: Scene) -> Vec<Id> {
    // 材质合流（旧 id -> 新 id）
    let mut mat_map = BTreeMap::new();
    for m in imported.materials {
        let nid = archcore_ops::next_mat_id(scene);
        mat_map.insert(m.id.clone(), nid.clone());
        scene.materials.push(Material { id: nid, ..m });
    }
    // 对象 id 重排（单调计数器，不复用），父子引用同步重映射
    let mut id_map = BTreeMap::new();
    for o in &imported.objects {
        id_map.insert(o.id.clone(), next_id(scene));
    }
    let mut changed = Vec::new();
    for mut o in imported.objects {
        let nid = id_map[&o.id].clone();
        o.id = nid.clone();
        o.parent = o.parent.and_then(|p| id_map.get(&p).cloned());
        if let ObjectKind::Mesh { material_slots, .. } = &mut o.kind {
            for s in material_slots.iter_mut() {
                *s = mat_map.get(s).cloned().unwrap_or_default();
            }
        }
        changed.push(nid);
        scene.objects.push(o);
    }
    changed
}

/// mesh 类 op 注册（拓扑内核经 IR Geometry 进出）
fn register_mesh_ops(reg: &mut OpRegistry) -> Result<(), OpError> {
    reg.register(OpSpec {
        def: OpDef {
            id: "mesh.create".into(),
            label: "创建网格对象".into(),
            description: "由索引几何建网格对象；拓扑非法（非流形/退化）显式拒绝".into(),
            category: OpCategory::Geometry,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![
                param("name", "string", true, "对象名"),
                param("positions", "array", true, "顶点坐标数组 [[x,y,z],...]"),
                param("indices", "array", false, "三角索引数组（3 的倍数）"),
                param("color", "array", false, "可选 [r,g,b]，自动建材质并绑定"),
                param("material", "string", false, "可选，绑定既有材质 id（优先于 color）"),
                param("uv", "array", false, "可选逐顶点 UV [[u,v],...]，数量须与顶点一致"),
            ],
        },
        handler: Box::new(|scene: &mut Scene, args: &Value| {
            let name = args["name"].as_str().unwrap().to_string();
            let positions: Vec<[f64; 3]> =
                serde_json::from_value(args["positions"].clone()).map_err(|e| OpError::BadRequest {
                    param: "positions".into(),
                    reason: e.to_string(),
                })?;
            let indices: Vec<u32> = args
                .get("indices")
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()
                .map_err(|e| OpError::BadRequest {
                    param: "indices".into(),
                    reason: e.to_string(),
                })?
                .unwrap_or_default();
            let mesh = Mesh::from_indexed(positions, &indices).map_err(mesh_err)?;
            let (pos, idx) = mesh.to_indexed();
            let uv_sets = match args
                .get("uv")
                .map(|v| serde_json::from_value::<Vec<[f64; 2]>>(v.clone()))
                .transpose()
                .map_err(|e| OpError::BadRequest {
                    param: "uv".into(),
                    reason: e.to_string(),
                })? {
                Some(coords) if !coords.is_empty() => {
                    if coords.len() != pos.len() {
                        return Err(OpError::BadRequest {
                            param: "uv".into(),
                            reason: "UV 数量必须与顶点数一致".into(),
                        });
                    }
                    vec![archcore_ir::UvSet {
                        name: "UVMap".into(),
                        coords,
                    }]
                }
                _ => vec![],
            };
            let material_slots = if let Some(mid) = args.get("material").and_then(|v| v.as_str()) {
                if !scene.materials.iter().any(|m| m.id == mid) {
                    return Err(OpError::NotFound {
                        what: "material".into(),
                        id: mid.to_string(),
                    });
                }
                vec![mid.to_string()]
            } else {
                match args.get("color").and_then(|v| serde_json::from_value::<[f64; 3]>(v.clone()).ok()) {
                Some(c) => {
                    let mid = archcore_ops::next_mat_id(scene);
                    scene.materials.push(Material {
                        id: mid.clone(),
                        name: format!("MAT_{name}"),
                        shading_model: ShadingModel::PbrMetallicRoughness,
                        base_color: [c[0], c[1], c[2], 1.0],
                        metallic: 0.0,
                        roughness: 0.8,
                        opacity: 1.0,
                        textures: Default::default(),
                    });
                    vec![mid]
                }
                None => vec![],
                }
            };
            let id = next_id(scene);
            scene.objects.push(Object {
                id: id.clone(),
                name,
                kind: ObjectKind::Mesh {
                    geometry: Geometry {
                        positions: pos,
                        indices: idx,
                        normals: None,
                        uv_sets,
                    },
                    material_slots,
                },
                parent: None,
                transform: Transform::default(),
                visible: true,
                meta: BTreeMap::new(),
            });
            Ok((vec![id], Value::Null))
        }),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "mesh.add_face".into(),
            label: "加三角面".into(),
            description: "向网格加面（自动配对 twin）；非流形/退化显式拒绝".into(),
            category: OpCategory::Geometry,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![
                param("id", "string", true, "网格对象 id"),
                param("a", "number", true, "顶点索引"),
                param("b", "number", true, "顶点索引"),
                param("c", "number", true, "顶点索引"),
            ],
        },
        handler: Box::new(|scene: &mut Scene, args: &Value| {
            let id = args["id"].as_str().unwrap().to_string();
            let geo = get_mesh(scene, &id)?;
            let mut mesh = Mesh::from_indexed(geo.positions, &geo.indices).map_err(mesh_err)?;
            let a = archcore_ops::arg_u32(args, "a")?;
            let b = archcore_ops::arg_u32(args, "b")?;
            let c = archcore_ops::arg_u32(args, "c")?;
            mesh.add_face(a, b, c).map_err(mesh_err)?;
            let (pos, idx) = mesh.to_indexed();
            put_mesh(scene, &id, pos, idx)?;
            Ok((vec![id], Value::Null))
        }),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "mesh.flip_edge".into(),
            label: "翻转对角线".into(),
            description: "翻转 (v0,v1) 对角线；边界边/退化显式拒绝".into(),
            category: OpCategory::Geometry,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![
                param("id", "string", true, "网格对象 id"),
                param("v0", "number", true, "边端点"),
                param("v1", "number", true, "边端点"),
            ],
        },
        handler: Box::new(|scene: &mut Scene, args: &Value| {
            let id = args["id"].as_str().unwrap().to_string();
            let geo = get_mesh(scene, &id)?;
            let mut mesh = Mesh::from_indexed(geo.positions, &geo.indices).map_err(mesh_err)?;
            let v0 = archcore_ops::arg_u32(args, "v0")?;
            let v1 = archcore_ops::arg_u32(args, "v1")?;
            let he = mesh.find_halfedge(v0, v1).ok_or_else(|| OpError::NotFound {
                what: "edge".into(),
                id: format!("({v0},{v1})"),
            })?;
            mesh.edge_flip(he).map_err(mesh_err)?;
            let (pos, idx) = mesh.to_indexed();
            put_mesh(scene, &id, pos, idx)?;
            Ok((vec![id], Value::Null))
        }),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "mesh.boolean".into(),
            label: "布尔运算".into(),
            description: "并/差/交（manifold 外借）；结果为空或与输入全同必须显式报错（禁止静默打空）".into(),
            category: OpCategory::Geometry,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![
                param("id_a", "string", true, "A 网格 id"),
                param("id_b", "string", true, "B 网格 id"),
                param("op", "string", true, "union | difference | intersection"),
                param("name", "string", false, "结果对象名"),
            ],
        },
        handler: Box::new(|scene, args| {
            let id_a = args["id_a"].as_str().unwrap().to_string();
            let id_b = args["id_b"].as_str().unwrap().to_string();
            let ga = get_mesh(scene, &id_a)?;
            let gb = get_mesh(scene, &id_b)?;
            let op_str = args["op"].as_str().unwrap();
            let op = match op_str {
                "union" => BoolOp::Union,
                "difference" => BoolOp::Difference,
                "intersection" => BoolOp::Intersection,
                other => {
                    return Err(OpError::BadRequest {
                        param: "op".into(),
                        reason: format!("未知布尔类型: {other}"),
                    })
                }
            };
            let ma = Mesh::from_indexed(ga.positions.clone(), &ga.indices).map_err(mesh_err)?;
            let mb = Mesh::from_indexed(gb.positions.clone(), &gb.indices).map_err(mesh_err)?;
            let out = ma.boolean(&mb, op).map_err(mesh_err)?;
            let (pos, idx) = out.to_indexed();
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| format!("{id_a}_{op_str}_{id_b}"));
            let id = next_id(scene);
            scene.objects.push(Object {
                id: id.clone(),
                name,
                kind: ObjectKind::Mesh {
                    geometry: Geometry {
                        positions: pos,
                        indices: idx,
                        normals: None,
                        uv_sets: vec![],
                    },
                    material_slots: vec![],
                },
                parent: None,
                transform: Transform::default(),
                visible: true,
                meta: BTreeMap::new(),
            });
            Ok((vec![id], Value::Null))
        }),
    })?;

    Ok(())
}

/// 参数表构造助手（各注册器共用）
fn param(name: &str, ty: &str, required: bool, desc: &str) -> ParamSpec {
    ParamSpec {
        name: name.into(),
        type_name: ty.into(),
        required,
        description: desc.into(),
    }
}

/// IO 类 op（read_only 语义：写文件不动文档，正是 no-op 检测豁免口径的正例）
fn register_io_ops(reg: &mut OpRegistry) -> Result<(), OpError> {
    reg.register(OpSpec {
        def: OpDef {
            id: "io.export_gltf".into(),
            label: "导出 glTF".into(),
            description: "场景导出 glTF 2.0（内嵌 buffer，几何/层级/UV/材质/相机保真）".into(),
            category: OpCategory::Io,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: true,
            },
            params: vec![ParamSpec {
                name: "path".into(),
                type_name: "string".into(),
                required: true,
                description: "输出文件路径".into(),
            }],
        },
        handler: Box::new(|scene, args| {
            let path = args["path"].as_str().unwrap();
            std::fs::write(path, archcore_io::export_gltf(scene))
                .map_err(|e| OpError::Internal { reason: e.to_string() })?;
            Ok((vec![], Value::Null))
        }),
    })?;
    reg.register(OpSpec {
        def: OpDef {
            id: "io.export_fbx".into(),
            label: "导出 FBX".into(),
            description: "经 Blender 转换桥导出 FBX（glTF 中间件；桥可替换为 FBX SDK 适配器）".into(),
            category: OpCategory::Io,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: true,
            },
            params: vec![ParamSpec {
                name: "path".into(),
                type_name: "string".into(),
                required: true,
                description: "输出 .fbx 路径".into(),
            }],
        },
        handler: Box::new(|scene, args| {
            let path = args["path"].as_str().unwrap();
            let blender = std::env::var("ARCHCORE_BLENDER")
                .unwrap_or_else(|_| "D:/New Folder/blender.exe".into());
            archcore_io::export_fbx(
                scene,
                std::path::Path::new(&blender),
                std::path::Path::new(path),
            )
            .map_err(|e| OpError::Internal { reason: format!("{e:?}") })?;
            Ok((vec![], Value::Null))
        }),
    })?;
    reg.register(OpSpec {
        def: OpDef {
            id: "io.save_scene".into(),
            label: "保存场景".into(),
            description: "场景 IR 序列化落盘（人审视口的入料口）".into(),
            category: OpCategory::Io,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: true,
            },
            params: vec![ParamSpec {
                name: "path".into(),
                type_name: "string".into(),
                required: true,
                description: "输出 JSON 路径".into(),
            }],
        },
        handler: Box::new(|scene, args| {
            let path = args["path"].as_str().unwrap();
            let text = serde_json::to_string_pretty(scene)
                .map_err(|e| OpError::Internal { reason: e.to_string() })?;
            ensure_parent(path)?;
            std::fs::write(path, text).map_err(|e| OpError::Internal { reason: e.to_string() })?;
            Ok((vec![], Value::Null))
        }),
    })?;
    reg.register(OpSpec {
        def: OpDef {
            id: "io.save_archive".into(),
            label: "保存存档".into(),
            description: "打包为 .al 自包含存档（场景 + 贴图字节，容器内相对路径）；换机器可直接打开".into(),
            category: OpCategory::Io,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: true,
            },
            params: vec![param("path", "string", true, ".al 输出路径")],
        },
        handler: Box::new(|scene, args| {
            let path = args["path"].as_str().unwrap();
            ensure_parent(path)?;
            let report = archcore_io::archive::pack(scene, std::path::Path::new(path))
                .map_err(|e| OpError::Internal { reason: format!("{e}") })?;
            Ok((
                vec![],
                serde_json::json!({
                    "path": path,
                    "assets_packed": report.assets_packed,
                    "warnings": report.warnings,
                }),
            ))
        }),
    })?;
    reg.register(OpSpec {
        def: OpDef {
            id: "io.import_fbx".into(),
            label: "导入 FBX".into(),
            description: "经 Blender 桥导入 FBX 并入场景（id 重排、父子/材质重映射；灯光为已知损耗项）".into(),
            category: OpCategory::Io,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![param("path", "string", true, "FBX 文件路径")],
        },
        handler: Box::new(|scene, args| {
            let path = args["path"].as_str().unwrap();
            let blender = std::env::var("ARCHCORE_BLENDER")
                .unwrap_or_else(|_| "D:/New Folder/blender.exe".into());
            let imported = archcore_io::import_fbx(
                std::path::Path::new(&blender),
                std::path::Path::new(path),
            )
            .map_err(|e| OpError::Internal { reason: format!("{e:?}") })?;
            let changed = merge_imported(scene, imported);
            Ok((changed, Value::Null))
        }),
    })?;
    reg.register(OpSpec {
        def: OpDef {
            id: "io.import_max".into(),
            label: "导入 .max".into(),
            description: "3dsmaxbatch 静默转换（.max→FBX）+ FBX 桥并入场景（官方通道唯一）".into(),
            category: OpCategory::Io,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![param("path", "string", true, ".max 文件路径")],
        },
        handler: Box::new(|scene, args| {
            let path = args["path"].as_str().unwrap();
            let max_bin = std::env::var("ARCHCORE_MAX")
                .unwrap_or_else(|_| "D:/3D MAX/3ds Max 2024/3dsmaxbatch.exe".into());
            let blender = std::env::var("ARCHCORE_BLENDER")
                .unwrap_or_else(|_| "D:/New Folder/blender.exe".into());
            let imported = archcore_io::import_max(
                std::path::Path::new(&max_bin),
                std::path::Path::new(&blender),
                std::path::Path::new(path),
            )
            .map_err(|e| OpError::Internal { reason: format!("{e:?}") })?;
            let changed = merge_imported(scene, imported);
            Ok((changed, Value::Null))
        }),
    })?;
    reg.register(OpSpec {
        def: OpDef {
            id: "io.import_ifc".into(),
            label: "导入 IFC".into(),
            description: "IFC（STEP P21）子集导入：空间层级 + 拉伸体（矩形/折线轮廓）+ 三角面集；单位从 IfcSIUnit 读取（缺失则假定米并标注）；构件并入当前场景（id 重排不复用）".into(),
            category: OpCategory::Io,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![param("path", "string", true, "IFC 文件路径")],
        },
        handler: Box::new(|scene, args| {
            let path = args["path"].as_str().unwrap();
            let data =
                std::fs::read(path).map_err(|e| OpError::Internal { reason: e.to_string() })?;
            let text = std::str::from_utf8(&data)
                .map_err(|e| OpError::Internal { reason: e.to_string() })?;
            let imported =
                archcore_io::import_ifc(text).map_err(|e| OpError::Internal { reason: e.to_string() })?;
            let changed = merge_imported(scene, imported);
            Ok((changed, Value::Null))
        }),
    })?;
    reg.register(OpSpec {
        def: OpDef {
            id: "io.export_ifc".into(),
            label: "导出 IFC".into(),
            description: "IR 场景导出 IFC4（IfcTriangulatedFaceSet 镶嵌表示 + 单楼层归并 + 材质关联）".into(),
            category: OpCategory::Io,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: true,
            },
            params: vec![param("path", "string", true, "输出 .ifc 路径")],
        },
        handler: Box::new(|scene, args| {
            let path = args["path"].as_str().unwrap();
            let text = archcore_io::export_ifc(scene);
            ensure_parent(path)?;
            std::fs::write(path, text)
                .map_err(|e| OpError::Internal { reason: e.to_string() })?;
            Ok((vec![], Value::Null))
        }),
    })?;
    reg.register(OpSpec {
        def: OpDef {
            id: "io.load_scene".into(),
            label: "载入场景".into(),
            description: "从场景文件载入 IR（整体替换当前文档）；`.al` 自包含存档自动解包并落地贴图".into(),
            category: OpCategory::Io,
            annotations: OpAnnotations {
                destructive: true,
                idempotent: true,
                read_only: false,
            },
            params: vec![param("path", "string", true, "场景 JSON 路径")],
        },
        handler: Box::new(|scene, args| {
            let path = args["path"].as_str().unwrap();
            let data =
                std::fs::read(path).map_err(|e| OpError::Internal { reason: e.to_string() })?;
            let loaded: Scene = if path.to_ascii_lowercase().ends_with(".al") {
                // .al：解包到内容指纹目录，贴图 uri 指回落盘位置
                let dir = archcore_io::archive::default_extract_dir(&data);
                archcore_io::archive::open(&data, &dir)
                    .map_err(|e| OpError::Internal { reason: format!("{e}") })?
            } else {
                serde_json::from_slice(&data)
                    .map_err(|e| OpError::Internal { reason: e.to_string() })?
            };
            let changed: Vec<String> = loaded.objects.iter().map(|o| o.id.clone()).collect();
            *scene = loaded;
            // 载入外部场景后同步 id 计数器（审计 #6：否则删除最高 id 后新建即复用）
            archcore_ops::sync_id_seqs(scene);
            Ok((changed, Value::Null))
        }),
    })?;
    Ok(())
}

/// 渲染类 op（评审图 + 路径追踪出图；read_only：出图不动文档）
fn register_render_ops(reg: &mut OpRegistry) -> Result<(), OpError> {
    reg.register(OpSpec {
        def: OpDef {
            id: "render.preview".into(),
            label: "渲染评审图".into(),
            description: "软光栅评审图（自动取景+平涂），人审三件套的视觉件".into(),
            category: OpCategory::Render,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: true,
            },
            params: vec![
                ParamSpec {
                    name: "path".into(),
                    type_name: "string".into(),
                    required: true,
                    description: "输出 PNG 路径".into(),
                },
                ParamSpec {
                    name: "width".into(),
                    type_name: "number".into(),
                    required: false,
                    description: "宽（默认 800）".into(),
                },
                ParamSpec {
                    name: "height".into(),
                    type_name: "number".into(),
                    required: false,
                    description: "高（默认 600）".into(),
                },
            ],
        },
        handler: Box::new(|scene, args| {
            let path = args["path"].as_str().unwrap();
            let w = dims_clamp(args, "width", 800, 1, 16384)?;
            let h = dims_clamp(args, "height", 600, 1, 16384)?;
            archcore_render::render_to_file(scene, w, h, path)
                .map_err(|e| OpError::Internal { reason: e })?;
            Ok((vec![], Value::Null))
        }),
    })?;
    reg.register(OpSpec {
        def: OpDef {
            id: "render.high_quality".into(),
            label: "路径追踪出图".into(),
            description: "离线路径追踪静帧（BVH+NEE+余弦采样；renderer=cpu 确定性默认 / gpu 速度轨 / auto 自动）".into(),
            category: OpCategory::Render,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: true,
            },
            params: vec![
                param("path", "string", true, "输出 PNG 路径"),
                param("width", "number", false, "宽（默认 640）"),
                param("height", "number", false, "高（默认 480）"),
                param("spp", "number", false, "每像素采样数（默认 32）"),
                param("exposure", "number", false, "曝光倍数（默认 1.0，ACES 色调映射）"),
                param("renderer", "string", false, "渲染轨：cpu（默认，确定性）| gpu（无 GPU 显式报错）| auto（GPU 优先，回退注明）"),
            ],
        },
        handler: Box::new(|scene, args| {
            let path = args["path"].as_str().unwrap();
            let w = dims_clamp(args, "width", 640, 1, 16384)?;
            let h = dims_clamp(args, "height", 480, 1, 16384)?;
            let spp = dims_clamp(args, "spp", 32, 1, 4096)? as usize;
            let exposure = args.get("exposure").and_then(|v| v.as_f64()).unwrap_or(1.0);
            let renderer = args.get("renderer").and_then(|v| v.as_str()).unwrap_or("cpu");
            let (png, used, adapter) = match renderer {
                "gpu" => {
                    let (png, ad) =
                        archcore_render::render_pathtraced_gpu(scene, w, h, spp, exposure).map_err(
                            |e| OpError::Precondition {
                                reason: format!("GPU 渲染请求失败（fail-closed，不回退）：{e}"),
                            },
                        )?;
                    (png, "gpu", Some(ad))
                }
                "auto" => {
                    match archcore_render::render_pathtraced_gpu(scene, w, h, spp, exposure) {
                        Ok((png, ad)) => (png, "gpu", Some(ad)),
                        Err(e) => {
                            let png = archcore_render::render_pathtraced_exposed(
                                scene, w, h, spp, exposure,
                            )
                            .map_err(|e2| OpError::Internal { reason: e2 })?;
                            (png, "cpu", Some(format!("GPU 不可用已回退 CPU：{e}")))
                        }
                    }
                }
                "cpu" => {
                    let png =
                        archcore_render::render_pathtraced_exposed(scene, w, h, spp, exposure)
                            .map_err(|e| OpError::Internal { reason: e })?;
                    (png, "cpu", None)
                }
                other => {
                    return Err(OpError::BadRequest {
                        param: "renderer".into(),
                        reason: format!("未知 renderer：{other}（cpu|gpu|auto）"),
                    })
                }
            };
            ensure_parent(path)?;
            std::fs::write(path, png).map_err(|e| OpError::Internal { reason: e.to_string() })?;
            Ok((
                vec![],
                serde_json::json!({"path": path, "renderer": used, "adapter": adapter}),
            ))
        }),
    })?;
    Ok(())
}

/// 材质类 op（创建 + 贴图绑定）
fn register_material_ops(reg: &mut OpRegistry) -> Result<(), OpError> {
    reg.register(OpSpec {
        def: OpDef {
            id: "material.create".into(),
            label: "创建材质".into(),
            description: "PBR 材质（metallic-roughness）；返回材质 id".into(),
            category: OpCategory::Material,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![
                param("name", "string", true, "材质名"),
                param("id", "string", false, "显式材质 id（脚本链用；缺省单调分配）"),
                param("base_color", "array", false, "[r,g,b,a] 0–1（默认中灰）"),
                param("metallic", "number", false, "金属度（默认 0）"),
                param("roughness", "number", false, "粗糙度（默认 0.8）"),
                param("opacity", "number", false, "不透明度（默认 1）"),
            ],
        },
        handler: Box::new(|scene, args| {
            let name = args["name"].as_str().unwrap().to_string();
            let base_color: [f64; 4] = args
                .get("base_color")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or([0.8, 0.8, 0.8, 1.0]);
            let mid = match args.get("id").and_then(|v| v.as_str()) {
                Some(explicit) => {
                    if scene.materials.iter().any(|m| m.id == explicit) {
                        return Err(OpError::BadRequest {
                            param: "id".into(),
                            reason: format!("材质 id 已存在：{explicit}"),
                        });
                    }
                    explicit.to_string()
                }
                None => archcore_ops::next_mat_id(scene),
            };
            scene.materials.push(Material {
                id: mid.clone(),
                name,
                shading_model: ShadingModel::PbrMetallicRoughness,
                base_color,
                metallic: args.get("metallic").and_then(|v| v.as_f64()).unwrap_or(0.0),
                roughness: args.get("roughness").and_then(|v| v.as_f64()).unwrap_or(0.8),
                opacity: args.get("opacity").and_then(|v| v.as_f64()).unwrap_or(1.0),
                textures: Default::default(),
            });
            Ok((vec![mid], Value::Null))
        }),
    })?;
    reg.register(OpSpec {
        def: OpDef {
            id: "material.set_texture".into(),
            label: "绑定贴图".into(),
            description: "给材质的贴图槽位挂资产（资产授权信息随模型走）；悬空引用由验证器查".into(),
            category: OpCategory::Material,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: false,
            },
            params: vec![
                param("material", "string", true, "材质 id"),
                param("slot", "string", true, "base_color|normal|roughness|metallic|ao"),
                param("uri", "string", true, "贴图文件路径/URI"),
            ],
        },
        handler: Box::new(|scene, args| {
            let mid = args["material"].as_str().unwrap().to_string();
            let slot = args["slot"].as_str().unwrap();
            let uri = args["uri"].as_str().unwrap().to_string();
            // slot 先校验：非法槽位不得污染资产表（审计 #14）
            match slot {
                "base_color" | "normal" | "roughness" | "metallic" | "ao" => {}
                other => {
                    return Err(OpError::BadRequest {
                        param: "slot".into(),
                        reason: format!("未知贴图槽位: {other}"),
                    })
                }
            }
            if !scene.materials.iter().any(|m| m.id == mid) {
                return Err(OpError::NotFound {
                    what: "material".into(),
                    id: mid,
                });
            }
            let aid = archcore_ops::next_asset_id(scene);
            scene.assets.push(AssetRef {
                id: aid.clone(),
                uri,
                kind: "texture".into(),
                license: None,
                source: None,
            });
            let Some(mat) = scene.materials.iter_mut().find(|m| m.id == mid) else {
                return Err(OpError::Internal {
                    reason: "材质在校验后消失（并发写入？）".into(),
                });
            };
            let target = match slot {
                "base_color" => &mut mat.textures.base_color,
                "normal" => &mut mat.textures.normal,
                "roughness" => &mut mat.textures.roughness,
                "metallic" => &mut mat.textures.metallic,
                _ => &mut mat.textures.ao,
            };
            *target = Some(aid);
            Ok((vec![mid], Value::Null))
        }),
    })?;
    Ok(())
}

/// 路径线性插值（v1：段内线性；样条缓动留 v2）
fn lerp_path(pts: &[[f64; 3]], t: f64) -> [f64; 3] {
    let n = pts.len();
    if n == 1 {
        return pts[0];
    }
    let t = t.clamp(0.0, 1.0);
    let seg = t * (n - 1) as f64;
    let i0 = (seg.floor() as usize).min(n - 2);
    let f = seg - i0 as f64;
    let (a, b) = (pts[i0], pts[i0 + 1]);
    [
        a[0] + (b[0] - a[0]) * f,
        a[1] + (b[1] - a[1]) * f,
        a[2] + (b[2] - a[2]) * f,
    ]
}

/// 数值参数范围校验（f64 版 dims_clamp）
fn dimf(args: &Value, name: &str, default: f64, lo: f64, hi: f64) -> Result<f64, OpError> {
    let v = match args.get(name) {
        None | Some(Value::Null) => default,
        Some(v) => v.as_f64().ok_or_else(|| OpError::BadRequest {
            param: name.into(),
            reason: "应为数值".into(),
        })?,
    };
    if !(v.is_finite() && (lo..=hi).contains(&v)) {
        return Err(OpError::BadRequest {
            param: name.into(),
            reason: format!("应在 {lo}..={hi}，实为 {v}"),
        });
    }
    Ok(v)
}

/// 盒体几何（8 顶点 12 三角，bench 同款三角化）
fn box_geometry(min: [f64; 3], max: [f64; 3]) -> Geometry {
    let (x0, y0, z0) = (min[0], min[1], min[2]);
    let (x1, y1, z1) = (max[0], max[1], max[2]);
    Geometry {
        positions: vec![
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y1, z0],
            [x0, y1, z0],
            [x0, y0, z1],
            [x1, y0, z1],
            [x1, y1, z1],
            [x0, y1, z1],
        ],
        indices: vec![
            0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 2, 3, 7, 2, 7, 6, 1, 2, 6, 1,
            6, 5, 0, 4, 7, 0, 7, 3,
        ],
        normals: None,
        uv_sets: vec![],
    }
}

/// 多盒体合并几何（柱网按层合并，控对象数）
fn boxes_geometry(boxes: &[([f64; 3], [f64; 3])]) -> Geometry {
    let mut g = Geometry::default();
    for (i, &(min, max)) in boxes.iter().enumerate() {
        let b = box_geometry(min, max);
        let base = (i * 8) as u32;
        g.positions.extend(b.positions);
        g.indices.extend(b.indices.into_iter().map(|x| x + base));
    }
    g
}

/// 体块方案 op：楼层/开间/进深参数化生成（方案核心价值）
fn register_massing_ops(reg: &mut OpRegistry) -> Result<(), OpError> {
    reg.register(OpSpec {
        def: OpDef {
            id: "massing.build".into(),
            label: "体块方案生成".into(),
            description: "参数化生成建筑体块：外包络 + 每层楼板 + 可选周圈柱网，分层分组（Name_Fn）便于人审；产出即 IR（可布尔/渲染/出 FBX）。W=开间数×开间宽，H=楼层数×层高，position 为 footprint 角点".into(),
            category: OpCategory::Geometry,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![
                param("name", "string", true, "方案名（分组命名前缀）"),
                param("floors", "number", true, "楼层数（1–200）"),
                param("floor_height", "number", true, "层高（0.5–20）"),
                param("bays_x", "number", true, "开间数（1–50）"),
                param("bay_width", "number", true, "开间宽（0.5–100）"),
                param("depth", "number", true, "进深（0.5–10000）"),
                param("position", "array", false, "footprint 角点 [x,y,z]（默认原点）"),
                param("envelope", "boolean", false, "封闭外包络体块（默认 true，布尔就绪）"),
                param("slabs", "boolean", false, "每层楼板（默认 true）"),
                param("columns", "boolean", false, "周圈柱网（默认 false）"),
                param("slab_thickness", "number", false, "楼板厚（默认 0.15）"),
                param("column_size", "number", false, "柱截面（默认 0.5）"),
                param("material", "string", false, "材质 id（须存在）"),
            ],
        },
        handler: Box::new(|scene, args| {
            let name = args["name"].as_str().unwrap().to_string();
            let position: [f64; 3] = args
                .get("position")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or([0.0; 3]);
            let floors = dims_clamp(args, "floors", 1, 1, 200)?;
            let floor_height = dimf(args, "floor_height", 3.0, 0.5, 20.0)?;
            let bays_x = dims_clamp(args, "bays_x", 1, 1, 50)?;
            let bay_width = dimf(args, "bay_width", 4.0, 0.5, 100.0)?;
            let depth = dimf(args, "depth", 10.0, 0.5, 10000.0)?;
            let envelope = args.get("envelope").and_then(|v| v.as_bool()).unwrap_or(true);
            let slabs = args.get("slabs").and_then(|v| v.as_bool()).unwrap_or(true);
            let columns = args.get("columns").and_then(|v| v.as_bool()).unwrap_or(false);
            let slab_t = dimf(args, "slab_thickness", 0.15, 0.01, 2.0)?;
            let col_s = dimf(args, "column_size", 0.5, 0.05, 5.0)?;
            let material: Option<String> = args
                .get("material")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if let Some(m) = &material {
                if !scene.materials.iter().any(|x| x.id == *m) {
                    return Err(OpError::NotFound {
                        what: "material".into(),
                        id: m.clone(),
                    });
                }
            }
            let slots: Vec<String> = material.iter().cloned().collect();
            let w = bays_x as f64 * bay_width;
            let h = floors as f64 * floor_height;
            let (px, py, pz) = (position[0], position[1], position[2]);
            let mut changed: Vec<String> = Vec::new();
            let root_id = archcore_ops::next_id(scene);
            changed.push(root_id.clone());
            scene.objects.push(Object {
                id: root_id.clone(),
                name: name.clone(),
                kind: ObjectKind::Group,
                parent: None,
                transform: Transform::default(),
                visible: true,
                meta: BTreeMap::new(),
            });
            if envelope {
                let id = archcore_ops::next_id(scene);
                changed.push(id.clone());
                scene.objects.push(Object {
                    id: id.clone(),
                    name: format!("{name}_Envelope"),
                    kind: ObjectKind::Mesh {
                        geometry: box_geometry([px, py, pz], [px + w, py + depth, pz + h]),
                        material_slots: slots.clone(),
                    },
                    parent: Some(root_id.clone()),
                    transform: Transform::default(),
                    visible: true,
                    meta: BTreeMap::new(),
                });
            }
            for f in 0..floors {
                let gid = archcore_ops::next_id(scene);
                changed.push(gid.clone());
                scene.objects.push(Object {
                    id: gid.clone(),
                    name: format!("{name}_F{}", f + 1),
                    kind: ObjectKind::Group,
                    parent: Some(root_id.clone()),
                    transform: Transform::default(),
                    visible: true,
                    meta: BTreeMap::new(),
                });
                if slabs {
                    let z0 = pz + f as f64 * floor_height;
                    let id = archcore_ops::next_id(scene);
                    changed.push(id.clone());
                    scene.objects.push(Object {
                        id: id.clone(),
                        name: format!("{name}_F{}_Slab", f + 1),
                        kind: ObjectKind::Mesh {
                            geometry: box_geometry(
                                [px, py, z0],
                                [px + w, py + depth, z0 + slab_t],
                            ),
                            material_slots: slots.clone(),
                        },
                        parent: Some(gid.clone()),
                        transform: Transform::default(),
                        visible: true,
                        meta: BTreeMap::new(),
                    });
                }
                if columns {
                    let mut boxes = Vec::new();
                    for i in 0..=bays_x {
                        let cx = px + i as f64 * bay_width;
                        for cy in [py, py + depth] {
                            boxes.push((
                                [
                                    cx - col_s / 2.0,
                                    cy - col_s / 2.0,
                                    pz + f as f64 * floor_height,
                                ],
                                [
                                    cx + col_s / 2.0,
                                    cy + col_s / 2.0,
                                    pz + (f + 1) as f64 * floor_height,
                                ],
                            ));
                        }
                    }
                    let id = archcore_ops::next_id(scene);
                    changed.push(id.clone());
                    scene.objects.push(Object {
                        id: id.clone(),
                        name: format!("{name}_F{}_Columns", f + 1),
                        kind: ObjectKind::Mesh {
                            geometry: boxes_geometry(&boxes),
                            material_slots: slots.clone(),
                        },
                        parent: Some(gid.clone()),
                        transform: Transform::default(),
                        visible: true,
                        meta: BTreeMap::new(),
                    });
                }
            }
            let summary = serde_json::json!({
                "name": name,
                "floors": floors,
                "floor_height": floor_height,
                "footprint": [w, depth],
                "height": h,
                "gross_floor_area": w * depth * floors as f64,
                "envelope": envelope,
                "slabs": slabs,
                "columns": columns,
                "objects_created": changed.len(),
            });
            Ok((changed, summary))
        }),
    })?;
    Ok(())
}

/// 单位盒列表：（min, max）对，各轴落在 [0,1]³；size 缩放出实际尺寸
type BoxList = Vec<([f64; 3], [f64; 3])>;

/// 家具库（单位盒 [0,1]³ 组合，size 缩放出实际尺寸；size=包围盒目标）
fn furniture_boxes(kind: &str) -> Result<BoxList, OpError> {
    let b: BoxList = match kind {
        // 门：左右门框 + 门楣 + 门扇（各轴铺满 [0,1]，size=包围盒目标）
        "door" => vec![
            ([0.0, 0.0, 0.0], [0.07, 1.0, 1.0]),
            ([0.93, 0.0, 0.0], [1.0, 1.0, 1.0]),
            ([0.0, 0.0, 0.93], [1.0, 1.0, 1.0]),
            ([0.1, 0.05, 0.02], [0.9, 0.95, 0.9]),
        ],
        // 窗：窗框四边 + 中挺
        "window" => vec![
            ([0.0, 0.0, 0.0], [1.0, 1.0, 0.05]),
            ([0.0, 0.0, 0.95], [1.0, 1.0, 1.0]),
            ([0.0, 0.0, 0.0], [0.05, 1.0, 1.0]),
            ([0.95, 0.0, 0.0], [1.0, 1.0, 1.0]),
            ([0.475, 0.05, 0.05], [0.525, 0.95, 0.95]),
        ],
        // 桌：桌面 + 四腿
        "table" => vec![
            ([0.0, 0.0, 0.92], [1.0, 1.0, 1.0]),
            ([0.0, 0.0, 0.0], [0.09, 0.09, 0.92]),
            ([0.91, 0.0, 0.0], [1.0, 0.09, 0.92]),
            ([0.0, 0.91, 0.0], [0.09, 1.0, 0.92]),
            ([0.91, 0.91, 0.0], [1.0, 1.0, 0.92]),
        ],
        // 椅：座 + 背 + 四腿（角柱贴角）
        "chair" => vec![
            ([0.12, 0.12, 0.45], [0.88, 0.88, 0.52]),
            ([0.12, 0.82, 0.52], [0.88, 0.88, 1.0]),
            ([0.0, 0.0, 0.0], [0.1, 0.1, 0.45]),
            ([0.9, 0.0, 0.0], [1.0, 0.1, 0.45]),
            ([0.0, 0.9, 0.0], [0.1, 1.0, 0.45]),
            ([0.9, 0.9, 0.0], [1.0, 1.0, 0.45]),
        ],
        // 床：床架 + 床垫 + 床头板 + 枕（床头板顶天立地）
        "bed" => vec![
            ([0.0, 0.0, 0.0], [1.0, 1.0, 0.35]),
            ([0.03, 0.03, 0.35], [0.97, 0.97, 0.55]),
            ([0.0, 0.9, 0.0], [1.0, 1.0, 1.0]),
            ([0.15, 0.68, 0.55], [0.85, 0.92, 0.63]),
        ],
        // 沙发：底座 + 靠背 + 双扶手 + 坐垫
        "sofa" => vec![
            ([0.0, 0.0, 0.0], [1.0, 0.85, 0.42]),
            ([0.0, 0.72, 0.42], [1.0, 1.0, 1.0]),
            ([0.0, 0.0, 0.42], [0.13, 1.0, 0.62]),
            ([0.87, 0.0, 0.42], [1.0, 1.0, 0.62]),
            ([0.16, 0.08, 0.42], [0.84, 0.68, 0.52]),
        ],
        // 柜：柜体 + 双门扇
        "cabinet" => vec![
            ([0.0, 0.0, 0.0], [1.0, 0.92, 1.0]),
            ([0.03, 0.92, 0.03], [0.49, 1.0, 0.97]),
            ([0.51, 0.92, 0.03], [0.97, 1.0, 0.97]),
        ],
        // 书架：双侧板 + 背板 + 层板
        "bookshelf" => vec![
            ([0.0, 0.0, 0.0], [0.04, 1.0, 1.0]),
            ([0.96, 0.0, 0.0], [1.0, 1.0, 1.0]),
            ([0.04, 0.92, 0.0], [0.96, 1.0, 1.0]),
            ([0.04, 0.0, 0.0], [0.96, 1.0, 0.03]),
            ([0.04, 0.0, 0.24], [0.96, 1.0, 0.27]),
            ([0.04, 0.0, 0.48], [0.96, 1.0, 0.51]),
            ([0.04, 0.0, 0.72], [0.96, 1.0, 0.75]),
            ([0.04, 0.0, 0.97], [0.96, 1.0, 1.0]),
        ],
        other => {
            return Err(OpError::BadRequest {
                param: "kind".into(),
                reason: format!(
                    "未知家具类型：{other}（door|window|table|chair|bed|sofa|cabinet|bookshelf）"
                ),
            })
        }
    };
    Ok(b)
}

fn default_furniture_size(kind: &str) -> [f64; 3] {
    match kind {
        "door" => [0.9, 0.12, 2.1],
        "window" => [1.2, 0.12, 1.5],
        "table" => [1.4, 0.8, 0.75],
        "chair" => [0.45, 0.45, 0.95],
        "bed" => [1.8, 2.0, 0.8],
        "sofa" => [2.0, 0.9, 0.85],
        "cabinet" => [0.6, 0.4, 1.8],
        "bookshelf" => [0.9, 0.3, 1.8],
        _ => [1.0, 1.0, 1.0],
    }
}

/// 室内软装 op：程序化家具 + 门窗开洞（接布尔链）
fn register_furnishing_ops(reg: &mut OpRegistry) -> Result<(), OpError> {
    reg.register(OpSpec {
        def: OpDef {
            id: "furniture.create".into(),
            label: "放置家具".into(),
            description: "程序化家具（door|window|table|chair|bed|sofa|cabinet|bookshelf）；size=包围盒目标 [w,d,h]，position=包围盒角点，rotate_z 为绕 Z 转角（度）。产出即 IR，可变换/渲染/出 FBX".into(),
            category: OpCategory::Object,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![
                param("kind", "string", true, "家具类型（八类见描述）"),
                param("name", "string", false, "对象名（默认同类型）"),
                param("position", "array", false, "包围盒角点 [x,y,z]（默认原点）"),
                param("rotate_z", "number", false, "绕 Z 转角（度，默认 0）"),
                param("size", "array", false, "包围盒目标 [w,d,h]（缺省用类型默认尺寸）"),
                param("material", "string", false, "材质 id（须存在）"),
            ],
        },
        handler: Box::new(|scene, args| {
            let kind = args["kind"].as_str().unwrap();
            let boxes = furniture_boxes(kind)?;
            let size: [f64; 3] = args
                .get("size")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_else(|| default_furniture_size(kind));
            for (i, s) in size.iter().enumerate() {
                if !(*s > 0.0 && s.is_finite()) {
                    return Err(OpError::BadRequest {
                        param: "size".into(),
                        reason: format!("第 {i} 维须为正数"),
                    });
                }
            }
            let scaled: Vec<([f64; 3], [f64; 3])> = boxes
                .iter()
                .map(|(a, b)| {
                    (
                        [a[0] * size[0], a[1] * size[1], a[2] * size[2]],
                        [b[0] * size[0], b[1] * size[1], b[2] * size[2]],
                    )
                })
                .collect();
            let position: [f64; 3] = args
                .get("position")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or([0.0; 3]);
            let rotate_z = args.get("rotate_z").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let material: Option<String> = args
                .get("material")
                .and_then(|v| v.as_str())
                .map(String::from);
            if let Some(m) = &material {
                if !scene.materials.iter().any(|x| x.id == *m) {
                    return Err(OpError::NotFound {
                        what: "material".into(),
                        id: m.clone(),
                    });
                }
            }
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(kind)
                .to_string();
            let id = archcore_ops::next_id(scene);
            let h = rotate_z.to_radians() / 2.0;
            scene.objects.push(Object {
                id: id.clone(),
                name,
                kind: ObjectKind::Mesh {
                    geometry: boxes_geometry(&scaled),
                    material_slots: material.into_iter().collect(),
                },
                parent: None,
                transform: Transform {
                    translation: position,
                    rotation: [0.0, 0.0, h.sin(), h.cos()],
                    scale: [1.0, 1.0, 1.0],
                },
                visible: true,
                meta: BTreeMap::new(),
            });
            Ok((
                vec![id],
                serde_json::json!({"kind": kind, "size": size, "position": position}),
            ))
        }),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "opening.cut".into(),
            label: "门窗开洞".into(),
            description: "在竖直墙体上就地开门窗洞（墙 id 不变，洞进墙里）；墙体须为封闭体块（布尔口径）；position=洞口底边中点，墙厚轴自动识别、双向穿透。打空显式报错".into(),
            category: OpCategory::Geometry,
            annotations: OpAnnotations {
                destructive: true,
                idempotent: false,
                read_only: false,
            },
            params: vec![
                param("wall", "string", true, "墙体 mesh id"),
                param("kind", "string", true, "door|window"),
                param("position", "array", true, "洞口底边中点 [x,y,z]"),
                param("width", "number", false, "洞宽（默认 0.9）"),
                param("height", "number", false, "洞高（默认 2.1）"),
            ],
        },
        handler: Box::new(|scene, args| {
            let wall_id = args["wall"].as_str().unwrap().to_string();
            let kind = args["kind"].as_str().unwrap();
            if !matches!(kind, "door" | "window") {
                return Err(OpError::BadRequest {
                    param: "kind".into(),
                    reason: format!("仅支持 door|window，实为 {kind}"),
                });
            }
            let position: [f64; 3] = args
                .get("position")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .ok_or_else(|| OpError::BadRequest {
                    param: "position".into(),
                    reason: "需 [x,y,z]".into(),
                })?;
            let width = dimf(args, "width", 0.9, 0.1, 50.0)?;
            let height = dimf(args, "height", 2.1, 0.1, 50.0)?;
            let geo = get_mesh(scene, &wall_id)?;
            let mut lo = [f64::MAX; 3];
            let mut hi = [f64::MIN; 3];
            for p in &geo.positions {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            let ext = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
            // v1 口径：Z 为竖向；墙厚轴 = 水平面内最薄轴
            if hi[2] - lo[2] < 0.01 {
                return Err(OpError::Precondition {
                    reason: "v1 仅支持竖直墙体（Z 向有高度）".into(),
                });
            }
            let t_axis = if ext[0] <= ext[1] { 0 } else { 1 };
            let u_axis = 1 - t_axis;
            let t_pad = ext[t_axis] * 0.2 + 0.02;
            let mut cmin = [0.0; 3];
            let mut cmax = [0.0; 3];
            cmin[t_axis] = lo[t_axis] - t_pad;
            cmax[t_axis] = hi[t_axis] + t_pad;
            cmin[u_axis] = position[u_axis] - width / 2.0;
            cmax[u_axis] = position[u_axis] + width / 2.0;
            cmin[2] = position[2];
            cmax[2] = position[2] + height;
            let cutter = box_geometry(cmin, cmax);
            let wall_mesh = archcore_mesh::Mesh::from_indexed(geo.positions, &geo.indices)
                .map_err(mesh_err)?;
            let cutter_mesh =
                archcore_mesh::Mesh::from_indexed(cutter.positions, &cutter.indices)
                    .map_err(mesh_err)?;
            let result = wall_mesh
                .boolean(&cutter_mesh, archcore_mesh::BoolOp::Difference)
                .map_err(mesh_err)?;
            let (pos, idx) = result.to_indexed();
            put_mesh(scene, &wall_id, pos, idx)?;
            // 开洞后平面重投影 UV（贴图连续性：与墙身同标尺，切洞处不断纹）
            if let Some(o) = scene.objects.iter_mut().find(|o| o.id == wall_id) {
                if let ObjectKind::Mesh { geometry, .. } = &mut o.kind {
                    let coords = geometry
                        .positions
                        .iter()
                        .map(|p| {
                            let h = if u_axis == 0 { p[0] } else { p[1] };
                            [h / 2.5, p[2] / 2.5]
                        })
                        .collect();
                    geometry.uv_sets = vec![archcore_ir::UvSet {
                        name: "UVMap".into(),
                        coords,
                    }];
                }
            }
            Ok((
                vec![wall_id],
                serde_json::json!({"kind": kind, "width": width, "height": height}),
            ))
        }),
    })?;
    Ok(())
}

/// 交付物 op：全景（等距柱状/立体 SBS）与漫游（关键帧轨迹批量渲染）
fn register_deliverable_ops(reg: &mut OpRegistry) -> Result<(), OpError> {
    reg.register(OpSpec {
        def: OpDef {
            id: "render.panorama".into(),
            label: "全景出图".into(),
            description: "等距柱状全景（360°×180°，图心=前方）；stereo=true 时左右眼 SBS 并排（VR 交付物）".into(),
            category: OpCategory::Render,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: true,
            },
            params: vec![
                param("path", "string", true, "输出 PNG 路径"),
                param("width", "number", false, "宽（默认 4096；stereo 时为总宽，须偶数）"),
                param("height", "number", false, "高（默认 2048）"),
                param("spp", "number", false, "每像素采样数（默认 32）"),
                param("exposure", "number", false, "曝光倍数（默认 1.0）"),
                param("renderer", "string", false, "渲染轨：cpu|gpu|auto（默认 cpu）"),
                param("stereo", "boolean", false, "立体全景 SBS（默认 false）"),
                param("ipd", "number", false, "瞳距（默认 0.065，场景单位）"),
            ],
        },
        handler: Box::new(|scene, args| {
            let path = args["path"].as_str().unwrap();
            let w = dims_clamp(args, "width", 4096, 1, 16384)?;
            let h = dims_clamp(args, "height", 2048, 1, 16384)?;
            let spp = dims_clamp(args, "spp", 32, 1, 4096)? as usize;
            let exposure = args.get("exposure").and_then(|v| v.as_f64()).unwrap_or(1.0);
            let stereo = args.get("stereo").and_then(|v| v.as_bool()).unwrap_or(false);
            let ipd = args.get("ipd").and_then(|v| v.as_f64()).unwrap_or(0.065);
            let renderer = args.get("renderer").and_then(|v| v.as_str()).unwrap_or("cpu");
            let opts = archcore_render::PanoramaOpts {
                width: w,
                height: h,
                spp,
                exposure,
                stereo,
                ipd,
            };
            let (png, used, adapter) = match renderer {
                "gpu" => {
                    let (png, ad) = archcore_render::render_panorama_gpu(scene, &opts).map_err(
                        |e| OpError::Precondition {
                            reason: format!("GPU 全景请求失败（fail-closed，不回退）：{e}"),
                        },
                    )?;
                    (png, "gpu", Some(ad))
                }
                "auto" => match archcore_render::render_panorama_gpu(scene, &opts) {
                    Ok((png, ad)) => (png, "gpu", Some(ad)),
                    Err(e) => {
                        let png = archcore_render::render_panorama(scene, &opts)
                            .map_err(|e2| OpError::Internal { reason: e2 })?;
                        (png, "cpu", Some(format!("GPU 不可用已回退 CPU：{e}")))
                    }
                },
                "cpu" => {
                    let png = archcore_render::render_panorama(scene, &opts)
                        .map_err(|e| OpError::Internal { reason: e })?;
                    (png, "cpu", None)
                }
                other => {
                    return Err(OpError::BadRequest {
                        param: "renderer".into(),
                        reason: format!("未知 renderer：{other}（cpu|gpu|auto）"),
                    })
                }
            };
            ensure_parent(path)?;
            std::fs::write(path, png).map_err(|e| OpError::Internal { reason: e.to_string() })?;
            Ok((
                vec![],
                serde_json::json!({"path": path, "renderer": used, "adapter": adapter, "stereo": stereo}),
            ))
        }),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "render.walkthrough".into(),
            label: "漫游出图".into(),
            description: "关键帧轨迹批量渲染：waypoints 线性插值（v1），相机 look-at；产出 walk_%04d.png + manifest.json，可 ffmpeg 装配视频".into(),
            category: OpCategory::Render,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: true,
            },
            params: vec![
                param("dir", "string", true, "输出目录"),
                param("waypoints", "array", true, "相机路径点 [[x,y,z],...]（至少两个）"),
                param("target", "array", false, "固定注视点 [x,y,z]"),
                param("targets", "array", false, "逐点注视点（与 waypoints 等长，优先于 target）"),
                param("frames", "number", false, "帧数（默认 24，2–600）"),
                param("width", "number", false, "宽（默认 960）"),
                param("height", "number", false, "高（默认 540）"),
                param("spp", "number", false, "每像素采样数（默认 16）"),
                param("exposure", "number", false, "曝光倍数（默认 1.0）"),
                param("fps", "number", false, "manifest 帧率提示（默认 30）"),
                param("renderer", "string", false, "渲染轨：cpu|gpu|auto（默认 cpu）"),
            ],
        },
        handler: Box::new(|scene, args| {
            let dir = args["dir"].as_str().unwrap();
            let waypoints: Vec<[f64; 3]> = args["waypoints"]
                .as_array()
                .and_then(|a| {
                    a.iter()
                        .map(|v| serde_json::from_value(v.clone()).ok())
                        .collect()
                })
                .ok_or_else(|| OpError::BadRequest {
                    param: "waypoints".into(),
                    reason: "需 [[x,y,z],...]".into(),
                })?;
            if waypoints.len() < 2 {
                return Err(OpError::BadRequest {
                    param: "waypoints".into(),
                    reason: "至少两个路径点".into(),
                });
            }
            let targets: Option<Vec<[f64; 3]>> = args
                .get("targets")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| serde_json::from_value(v.clone()).ok())
                        .collect()
                })
                .filter(|t: &Vec<[f64; 3]>| t.len() == waypoints.len());
            let fixed_target: Option<[f64; 3]> =
                args.get("target").and_then(|v| serde_json::from_value(v.clone()).ok());
            let frames = dims_clamp(args, "frames", 24, 2, 600)?;
            let w = dims_clamp(args, "width", 960, 1, 16384)?;
            let h = dims_clamp(args, "height", 540, 1, 16384)?;
            let spp = dims_clamp(args, "spp", 16, 1, 4096)? as usize;
            let exposure = args.get("exposure").and_then(|v| v.as_f64()).unwrap_or(1.0);
            let fps = dims_clamp(args, "fps", 30, 1, 240)?;
            let renderer = args.get("renderer").and_then(|v| v.as_str()).unwrap_or("cpu");
            std::fs::create_dir_all(dir)
                .map_err(|e| OpError::Internal { reason: e.to_string() })?;
            // 相机：场景首个 Camera，无则建（id 单调分配）
            let cam_id = match scene
                .objects
                .iter()
                .find(|o| matches!(o.kind, ObjectKind::Camera { .. }))
            {
                Some(o) => o.id.clone(),
                None => {
                    let id = archcore_ops::next_id(scene);
                    scene.objects.push(Object {
                        id: id.clone(),
                        name: "WalkCam".into(),
                        kind: ObjectKind::Camera {
                            lens_mm: 24.0,
                            sensor_mm: 36.0,
                            clip: [0.1, 1000.0],
                        },
                        parent: None,
                        transform: Transform::default(),
                        visible: true,
                        meta: BTreeMap::new(),
                    });
                    id
                }
            };
            for i in 0..frames {
                let t = i as f64 / (frames - 1) as f64;
                let pos = lerp_path(&waypoints, t);
                let tgt = if let Some(ts) = &targets {
                    lerp_path(ts, t)
                } else if let Some(fx) = fixed_target {
                    fx
                } else {
                    // 无注视点：沿路径切线看向前方
                    lerp_path(&waypoints, (t + 1.0 / (frames - 1) as f64).min(1.0))
                };
                let cam = scene
                    .objects
                    .iter_mut()
                    .find(|o| o.id == cam_id)
                    .expect("相机已建");
                cam.transform.translation = pos;
                cam.transform.rotation = archcore_ops::look_at_quat(pos, tgt);
                let png = match renderer {
                    "cpu" => archcore_render::render_pathtraced_exposed(
                        scene, w, h, spp, exposure,
                    )
                    .map_err(|e| OpError::Internal { reason: e })?,
                    "gpu" | "auto" => {
                        match archcore_render::render_pathtraced_gpu(scene, w, h, spp, exposure) {
                            Ok((p, _)) => p,
                            Err(_e) if renderer == "auto" => {
                                archcore_render::render_pathtraced_exposed(
                                    scene, w, h, spp, exposure,
                                )
                                .map_err(|e2| OpError::Internal { reason: e2 })?
                            }
                            Err(e) => {
                                return Err(OpError::Precondition {
                                    reason: format!("GPU 渲染失败（fail-closed）：{e}"),
                                })
                            }
                        }
                    }
                    other => {
                        return Err(OpError::BadRequest {
                            param: "renderer".into(),
                            reason: format!("未知 renderer：{other}（cpu|gpu|auto）"),
                        })
                    }
                };
                let fp = format!("{dir}/walk_{i:04}.png");
                std::fs::write(&fp, png)
                    .map_err(|e| OpError::Internal { reason: e.to_string() })?;
            }
            let manifest = serde_json::json!({
                "frames": frames,
                "fps": fps,
                "width": w,
                "height": h,
                "pattern": "walk_%04d.png",
                "renderer": renderer,
            });
            std::fs::write(
                format!("{dir}/manifest.json"),
                serde_json::to_string_pretty(&manifest).unwrap(),
            )
            .map_err(|e| OpError::Internal { reason: e.to_string() })?;
            Ok((
                vec![cam_id],
                serde_json::json!({"dir": dir, "frames": frames, "renderer": renderer}),
            ))
        }),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "web.export".into(),
            label: "Web 查看器导出".into(),
            description: "场景 → 自包含审阅 HTML（内嵌几何 + 手写 WebGL2 查看器，零外部请求）：轨道相机/物体树/线框/事实面，浏览器直接打开".into(),
            category: OpCategory::Io,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: true,
            },
            params: vec![
                param("path", "string", true, "输出 .html 路径"),
                param("title", "string", false, "审阅标题（默认场景名）"),
            ],
        },
        handler: Box::new(|scene, args| {
            let path = args["path"].as_str().unwrap();
            let title = args
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or(&scene.name)
                .to_string();
            let html = archcore_render::export_web(scene, &title)
                .map_err(|e| OpError::Internal { reason: e.to_string() })?;
            ensure_parent(path)?;
            std::fs::write(path, html)
                .map_err(|e| OpError::Internal { reason: e.to_string() })?;
            Ok((
                vec![],
                serde_json::json!({"path": path, "title": title}),
            ))
        }),
    })?;

    Ok(())
}

/// 房间内装预设（确定性布局：房间中心 + 固定偏移；size 为房间 [w,d]，unit 为包围盒参数）
fn interior_bundle(
    preset: &str,
    origin: [f64; 3],
    size: [f64; 2],
    material: Option<&str>,
) -> Result<Vec<Value>, OpError> {
    let (cx, cy, z) = (origin[0] + size[0] / 2.0, origin[1] + size[1] / 2.0, origin[2]);
    let item = |kind: &str, name: &str, pos: [f64; 2], rot: f64, sz: Option<[f64; 3]>| -> Value {
        let mut m = serde_json::Map::new();
        m.insert("kind".into(), Value::String(kind.into()));
        m.insert("name".into(), Value::String(name.into()));
        m.insert("position".into(), serde_json::json!([pos[0], pos[1], z]));
        if rot != 0.0 {
            m.insert("rotate_z".into(), Value::from(rot));
        }
        if let Some(s) = sz {
            m.insert("size".into(), serde_json::json!(s));
        }
        if let Some(ma) = material {
            m.insert("material".into(), Value::String(ma.to_string()));
        }
        Value::Object(m)
    };
    let items = match preset {
        // 餐叙：长桌 + 四椅（两侧各二）
        "dining" => vec![
            item("table", "餐桌", [cx - 0.7, cy - 0.4], 0.0, Some([1.4, 0.8, 0.75])),
            item("chair", "餐椅1", [cx - 0.35, cy - 1.05], 0.0, None),
            item("chair", "餐椅2", [cx + 0.35, cy - 1.05], 0.0, None),
            item("chair", "餐椅3", [cx - 0.35, cy + 0.65], 180.0, None),
            item("chair", "餐椅4", [cx + 0.35, cy + 0.65], 180.0, None),
        ],
        // 办公：桌 + 椅 + 书架 + 柜
        "office" => vec![
            item("table", "办公桌", [cx - 0.6, cy - 0.3], 0.0, Some([1.2, 0.6, 0.75])),
            item("chair", "办公椅", [cx - 0.25, cy + 0.35], 180.0, None),
            item("bookshelf", "书架", [cx + 1.2, cy - 0.15], 90.0, None),
            item("cabinet", "文件柜", [cx - 1.6, cy + 0.9], 0.0, Some([0.8, 0.4, 1.6])),
        ],
        // 起居：沙发 + 柜 + 书架
        "living" => vec![
            item("sofa", "沙发", [cx - 1.0, cy + 0.7], 180.0, Some([2.0, 0.9, 0.85])),
            item("cabinet", "电视柜", [cx - 0.8, cy - 1.3], 0.0, Some([1.6, 0.4, 0.6])),
            item("bookshelf", "书架", [cx + 1.1, cy + 0.7], 90.0, None),
        ],
        // 卧室：床 + 柜
        "bedroom" => vec![
            item("bed", "床", [cx - 0.9, cy + 0.2], 180.0, Some([1.8, 2.0, 0.8])),
            item("cabinet", "衣柜", [cx - 0.7, cy - 1.5], 0.0, Some([1.4, 0.6, 2.0])),
        ],
        other => {
            return Err(OpError::BadRequest {
                param: "preset".into(),
                reason: format!("未知内装预设：{other}（dining|office|living|bedroom）"),
            })
        }
    };
    Ok(items)
}

/// 内置场景模板（描述→场景的一句话入口）；用户简报 deep-merge 覆盖：对象深合并、数组整体替换
fn template_base(name: &str) -> Result<Value, OpError> {
    let v = match name {
        "community_center" => serde_json::json!({
            "materials": [
                {"name": "MAT_Concrete", "id": "mat-0", "base_color": [0.72,0.72,0.7,1]},
                {"name": "MAT_Wood", "id": "mat-2", "base_color": [0.45,0.3,0.18,1]},
                {"name": "MAT_Fabric", "id": "mat-3", "base_color": [0.35,0.42,0.55,1]}
            ],
            "site": {"size": [40,30]},
            "walls": [{"name": "景墙", "position": [12,24,0], "size": [16,0.24,2.2], "material": "mat-0"}],
            "buildings": [
                {"name": "Tower_A", "floors": 5, "floor_height": 3.0, "bays_x": 3, "bay_width": 4.0, "depth": 12.0, "position": [3,4,0], "columns": true, "material": "mat-0"},
                {"name": "Hall_B", "floors": 2, "floor_height": 3.6, "bays_x": 2, "bay_width": 5.0, "depth": 10.0, "position": [22,12,0], "columns": true, "material": "mat-0"}
            ],
            "openings": [{"wall": "景墙", "kind": "door", "position": [20,24,0], "width": 1.8, "height": 2.2}],
            "interiors": [
                {"preset": "dining", "position": [12,14,0], "size": [6,6], "material": "mat-2"},
                {"preset": "office", "position": [23,13,0], "size": [6,5], "material": "mat-2"}
            ],
            "camera": {"translation": [2,27,2.6], "target": [14,11,4]}
        }),
        "school" => serde_json::json!({
            "materials": [
                {"name": "MAT_Brick", "id": "mat-0", "base_color": [0.68,0.52,0.42,1]},
                {"name": "MAT_Wood", "id": "mat-2", "base_color": [0.45,0.3,0.18,1]}
            ],
            "site": {"size": [52,36]},
            "buildings": [
                {"name": "教学楼", "floors": 4, "floor_height": 3.4, "bays_x": 8, "bay_width": 3.6, "depth": 12.0, "position": [4,8,0], "columns": true, "material": "mat-0"}
            ],
            "interiors": [
                {"preset": "office", "position": [6,10,0], "size": [6,5], "material": "mat-2"},
                {"preset": "office", "position": [14,10,0], "size": [6,5], "material": "mat-2"}
            ],
            "camera": {"translation": [2,32,3.0], "target": [18,12,6]}
        }),
        "residence" => serde_json::json!({
            "materials": [
                {"name": "MAT_Wall", "id": "mat-0", "base_color": [0.78,0.76,0.72,1]},
                {"name": "MAT_Wood", "id": "mat-2", "base_color": [0.45,0.3,0.18,1]},
                {"name": "MAT_Fabric", "id": "mat-3", "base_color": [0.35,0.42,0.55,1]}
            ],
            "site": {"size": [44,32]},
            "buildings": [
                {"name": "住宅A", "floors": 6, "floor_height": 2.9, "bays_x": 3, "bay_width": 3.3, "depth": 11.0, "position": [3,5,0], "material": "mat-0"},
                {"name": "住宅B", "floors": 6, "floor_height": 2.9, "bays_x": 3, "bay_width": 3.3, "depth": 11.0, "position": [24,5,0], "material": "mat-0"}
            ],
            "interiors": [
                {"preset": "living", "position": [4,18,0], "size": [7,6], "material": "mat-3"},
                {"preset": "bedroom", "position": [14,18,0], "size": [5,5], "material": "mat-2"}
            ],
            "camera": {"translation": [2,29,2.6], "target": [14,10,5]}
        }),
        "park" => serde_json::json!({
            "materials": [
                {"name": "MAT_Stone", "id": "mat-0", "base_color": [0.6,0.6,0.58,1]},
                {"name": "MAT_Wood", "id": "mat-2", "base_color": [0.45,0.3,0.18,1]}
            ],
            "site": {"size": [60,45]},
            "walls": [
                {"name": "景墙A", "position": [18,34,0], "size": [18,0.24,2.0], "material": "mat-0"},
                {"name": "景墙B", "position": [40,10,0], "size": [0.24,18,2.0], "material": "mat-0"}
            ],
            "furniture": [
                {"kind": "chair", "name": "园椅1", "position": [14,20,0], "rotate_z": 45, "material": "mat-2"},
                {"kind": "chair", "name": "园椅2", "position": [16,18,0], "rotate_z": 225, "material": "mat-2"},
                {"kind": "table", "name": "石桌", "position": [14,18,0], "size": [0.9,0.9,0.72], "material": "mat-0"}
            ],
            "camera": {"translation": [2,40,2.2], "target": [24,16,3]}
        }),
        other => {
            return Err(OpError::BadRequest {
                param: "template".into(),
                reason: format!("未知模板：{other}（community_center|school|residence|park）"),
            })
        }
    };
    Ok(v)
}

/// deep-merge：对象递归合并，数组与标量整体替换（规则写进契约，变更需同步文档）
fn deep_merge(base: &Value, over: &Value) -> Value {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            let mut m = b.clone();
            for (k, v) in o {
                let cur = m.get(k).cloned().unwrap_or(Value::Null);
                m.insert(k.clone(), deep_merge(&cur, v));
            }
            Value::Object(m)
        }
        (_, v) => v.clone(),
    }
}

/// 简报展开：template 打底 + overrides + 显式键（template/overrides/variants/comparison_path 除外）
fn expand_spec(spec: &Value) -> Result<Value, EngineError> {
    let mut base = match spec.get("template").and_then(|v| v.as_str()) {
        Some(t) => template_base(t).map_err(EngineError::Op)?,
        None => serde_json::json!({}),
    };
    if let Some(over) = spec.get("overrides") {
        base = deep_merge(&base, over);
    }
    if let (Some(b), Some(o)) = (base.as_object_mut(), spec.as_object()) {
        for (k, v) in o {
            if matches!(k.as_str(), "template" | "overrides" | "variants" | "comparison_path") {
                continue;
            }
            let cur = b.get(k).cloned().unwrap_or(Value::Null);
            b.insert(k.clone(), deep_merge(&cur, v));
        }
    }
    Ok(base)
}

/// 变体产物路径加后缀（防变体互相覆盖）
fn suffix_deliverable_paths(d: &mut serde_json::Map<String, Value>, sfx: &str) {
    for key in ["perspective", "panorama", "vr"] {
        if let Some(Value::Object(m)) = d.get_mut(key) {
            if let Some(Value::String(p)) = m.get_mut("path") {
                *p = match p.strip_suffix(".png") {
                    Some(stripped) => format!("{stripped}{sfx}.png"),
                    None => format!("{p}{sfx}"),
                };
            }
        }
    }
    if let Some(Value::Object(m)) = d.get_mut("walkthrough") {
        if let Some(Value::String(p)) = m.get_mut("dir") {
            *p = format!("{p}{sfx}");
        }
    }
}

impl Engine {
    /// 直出场景（编排层）：结构化简报 → 场地/方案/软装/交付物一条龙。
    /// 逐步事务（每步=一个原子 op），失败即停带步骤号——与 execute_script 同口径。
    /// 简报 v2：{name, materials, site:{size,ground}, walls:[{name,position,size,material}...],
    ///          buildings:[massing 参数...],
    ///          openings:[{wall(按名引用),kind,position,width,height}...],
    ///          interiors:[{preset,position,size,material}...], furniture:[furniture 参数...],
    ///          camera:{translation,target,...},
    ///          deliverables:{perspective/panorama/vr/walkthrough/assemble_video},
    ///          renderer:"cpu|gpu|auto"}。
    /// 质量缺省（缺省即应用并在 summary 注明）：透视 spp64 / 全景 spp48 / 漫游 spp24。
    /// 确定性口径：renderer 默认 auto（直出工具要体验）；要字节级确定传 cpu。
    pub fn compose(&mut self, spec: &Value) -> Result<Value, EngineError> {
        let spec_owned = expand_spec(spec)?;
        let spec = &spec_owned;
        let name = spec
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("composed_scene");
        self.scene.name = name.to_string();
        let renderer = spec
            .get("renderer")
            .and_then(|v| v.as_str())
            .unwrap_or("auto")
            .to_string();
        let wrap = |step: usize, op: &str, e: EngineError| -> EngineError {
            EngineError::Op(OpError::Internal {
                reason: format!("compose 第 {step} 步 {op} 失败：{e}"),
            })
        };
        let mut step = 0usize;
        let mut created = 0usize;
        let mut outs = serde_json::Map::new();
        // 0) 材质先行：后续方案/家具引用（显式 id 可钉，缺省单调）
        if let Some(ms) = spec.get("materials").and_then(|v| v.as_array()) {
            for m in ms {
                step += 1;
                let r = self
                    .execute("material.create", m)
                    .map_err(|e| wrap(step, "material.create", e))?;
                created += r.changed.len();
            }
        }
        // 1) 场地：地面板（景观组件挂起期间的最小场地步进；L1 解挂后此处接 terrain/path/water）
        let site = spec.get("site");
        let ground = site
            .and_then(|s| s.get("ground"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if ground {
            let size: [f64; 2] = site
                .and_then(|s| s.get("size"))
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or([40.0, 30.0]);
            step += 1;
            let r = self
                .execute(
                    "mesh.create",
                    &serde_json::json!({
                        "name": "Ground",
                        "positions": [[0,0,-0.02],[size[0],0,-0.02],[size[0],size[1],-0.02],[0,size[1],-0.02]],
                        "indices": [0,2,1,0,3,2],
                        "uv": [[0,0],[size[0],0],[size[0],size[1]],[0,size[1]]]
                    }),
                )
                .map_err(|e| wrap(step, "mesh.create", e))?;
            created += r.changed.len();
        }
        // 1.5) 薄墙/隔断（门窗开洞的正确对象；封闭盒体，布尔就绪）
        if let Some(ws) = spec.get("walls").and_then(|v| v.as_array()) {
            for w in ws {
                step += 1;
                let wname = w
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Wall")
                    .to_string();
                let position: [f64; 3] = w
                    .get("position")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or([0.0; 3]);
                let size: [f64; 3] = w
                    .get("size")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or([4.0, 0.2, 2.8]);
                let material = w.get("material").and_then(|v| v.as_str());
                let (x0, y0, z0) = (position[0], position[1], position[2]);
                let (x1, y1, z1) = (x0 + size[0], y0 + size[1], z0 + size[2]);
                let mut m = serde_json::Map::new();
                m.insert("name".into(), Value::String(wname));
                m.insert(
                    "positions".into(),
                    serde_json::json!([[x0,y0,z0],[x1,y0,z0],[x1,y1,z0],[x0,y1,z0],[x0,y0,z1],[x1,y0,z1],[x1,y1,z1],[x0,y1,z1]]),
                );
                m.insert(
                    "indices".into(),
                    serde_json::json!([0,2,1,0,3,2,4,5,6,4,6,7,0,1,5,0,5,4,2,3,7,2,7,6,1,2,6,1,6,5,0,4,7,0,7,3]),
                );
                if let Some(ma) = material {
                    m.insert("material".into(), Value::String(ma.to_string()));
                }
                let r = self
                    .execute("mesh.create", &Value::Object(m))
                    .map_err(|e| wrap(step, "mesh.create(wall)", e))?;
                created += r.changed.len();
            }
        }
        // 2) 方案：逐栋 massing.build（参数原样透传，由 op 自校验——fail-closed）
        if let Some(bs) = spec.get("buildings").and_then(|v| v.as_array()) {
            for b in bs {
                step += 1;
                let r = self
                    .execute("massing.build", b)
                    .map_err(|e| wrap(step, "massing.build", e))?;
                created += r.changed.len();
            }
        }
        // 2.5) 门窗开洞：按名引用墙体（massing 的 Envelope 即封闭墙，直吃 opening.cut）
        if let Some(ops_list) = spec.get("openings").and_then(|v| v.as_array()) {
            for o in ops_list {
                step += 1;
                let wall_name = o
                    .get("wall")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| wrap(step, "opening.cut", EngineError::Op(OpError::BadRequest {
                        param: "wall".into(),
                        reason: "openings 条目须含 wall（按名引用）".into(),
                    })))?;
                let wid = self
                    .scene
                    .objects
                    .iter()
                    .find(|x| x.name == wall_name)
                    .map(|x| x.id.clone())
                    .ok_or_else(|| wrap(step, "opening.cut", EngineError::Op(OpError::NotFound {
                        what: "wall".into(),
                        id: wall_name.to_string(),
                    })))?;
                let mut args = o.as_object().cloned().unwrap_or_default();
                args.insert("wall".into(), Value::String(wid));
                self.execute("opening.cut", &Value::Object(args))
                    .map_err(|e| wrap(step, "opening.cut", e))?;
            }
        }
        // 2.6) 房间自动内装：预设包确定性布置
        if let Some(rooms) = spec.get("interiors").and_then(|v| v.as_array()) {
            for room in rooms {
                let preset = room
                    .get("preset")
                    .and_then(|v| v.as_str())
                    .unwrap_or("office");
                let position: [f64; 3] = room
                    .get("position")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or([0.0; 3]);
                let size: [f64; 2] = room
                    .get("size")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or([4.0, 4.0]);
                let material = room.get("material").and_then(|v| v.as_str());
                let bundle = interior_bundle(preset, position, size, material)
                    .map_err(|e| wrap(step, "interior_bundle", EngineError::Op(e)))?;
                for item in &bundle {
                    step += 1;
                    let r = self
                        .execute("furniture.create", item)
                        .map_err(|e| wrap(step, "furniture.create", e))?;
                    created += r.changed.len();
                }
            }
        }
        // 3) 软装：逐件 furniture.create
        if let Some(items) = spec.get("furniture").and_then(|v| v.as_array()) {
            for it in items {
                step += 1;
                let r = self
                    .execute("furniture.create", it)
                    .map_err(|e| wrap(step, "furniture.create", e))?;
                created += r.changed.len();
            }
        }
        // 3.5) 相机构图（可选）
        if let Some(c) = spec.get("camera") {
            step += 1;
            let mut args = c.as_object().cloned().unwrap_or_default();
            args.insert("name".into(), Value::String(format!("{name}_Cam")));
            let r = self
                .execute("camera.create", &Value::Object(args))
                .map_err(|e| wrap(step, "camera.create", e))?;
            created += r.changed.len();
        }
        // 4) 交付物自动全出（质量缺省：透视 64 / 全景 48 / 漫游 24，缺省即应用）
        if let Some(d) = spec.get("deliverables").and_then(|v| v.as_object()) {
            let with_defaults = |args: &Value, spp_default: u64| -> Value {
                let mut m = args.as_object().cloned().unwrap_or_default();
                m.insert("renderer".into(), Value::String(renderer.clone()));
                m.entry("spp").or_insert(Value::from(spp_default));
                Value::Object(m)
            };
            if let Some(p) = d.get("perspective") {
                step += 1;
                let r = self
                    .execute("render.high_quality", &with_defaults(p, 64))
                    .map_err(|e| wrap(step, "render.high_quality", e))?;
                outs.insert("perspective".into(), r.extra);
            }
            if let Some(p) = d.get("panorama") {
                step += 1;
                let r = self
                    .execute("render.panorama", &with_defaults(p, 48))
                    .map_err(|e| wrap(step, "render.panorama", e))?;
                outs.insert("panorama".into(), r.extra);
            }
            if let Some(p) = d.get("vr") {
                step += 1;
                let mut a = with_defaults(p, 48);
                a["stereo"] = Value::Bool(true);
                let r = self
                    .execute("render.panorama", &a)
                    .map_err(|e| wrap(step, "render.panorama(vr)", e))?;
                outs.insert("vr".into(), r.extra);
            }
            if let Some(p) = d.get("walkthrough") {
                step += 1;
                let args = with_defaults(p, 24);
                let dir = args["dir"].as_str().unwrap_or("").to_string();
                let r = self
                    .execute("render.walkthrough", &args)
                    .map_err(|e| wrap(step, "render.walkthrough", e))?;
                outs.insert("walkthrough".into(), r.extra);
                if d.get("assemble_video").and_then(|v| v.as_bool()).unwrap_or(false) {
                    let mp4 = format!("{dir}/walk.mp4");
                    let ok = std::process::Command::new("ffmpeg")
                        .args([
                            "-y",
                            "-framerate",
                            "30",
                            "-i",
                            &format!("{dir}/walk_%04d.png"),
                            "-c:v",
                            "libx264",
                            "-pix_fmt",
                            "yuv420p",
                            &mp4,
                        ])
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false);
                    if ok {
                        outs.insert("video".into(), Value::String(mp4));
                    } else {
                        outs.insert(
                            "video_note".into(),
                            Value::String("ffmpeg 不可用：帧已产出，可手动装配".into()),
                        );
                    }
                }
            }
        }
        Ok(serde_json::json!({
            "scene": name,
            "objects_created": created,
            "renderer": renderer,
            "quality_defaults": {"perspective_spp": 64, "panorama_spp": 48, "walkthrough_spp": 24},
            "deliverables": outs,
        }))
    }

    /// 多方案对比：N 变体独立场景 + 对比长图 + 摘要（人审审稿面）。变体数 1..=8。
    pub fn compose_variants(spec: &Value) -> Result<Value, EngineError> {
        let variants = spec
            .get("variants")
            .and_then(|v| v.as_array())
            .filter(|v| !v.is_empty() && v.len() <= 8)
            .ok_or_else(|| {
                EngineError::Op(OpError::BadRequest {
                    param: "variants".into(),
                    reason: "需要 1..=8 个变体对象".into(),
                })
            })?;
        let base = expand_spec(spec)?;
        let mut summaries = Vec::new();
        let mut views: Vec<(String, String)> = Vec::new(); // (变体名, 透视图路径)
        for (i, v) in variants.iter().enumerate() {
            let sfx = format!("_v{}", i + 1);
            let mut s = deep_merge(&base, v);
            if let Some(obj) = s.as_object_mut() {
                let nm = obj
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("variant")
                    .to_string();
                obj.insert("name".into(), Value::String(format!("{nm}{sfx}")));
                if let Some(Value::Object(d)) = obj.get_mut("deliverables") {
                    suffix_deliverable_paths(d, &sfx);
                }
            }
            let mut eng = Engine::new();
            let summary = eng.compose(&s)?;
            if let Some(p) = summary["deliverables"]["perspective"]["path"].as_str() {
                views.push((summary["scene"].as_str().unwrap_or("variant").to_string(), p.to_string()));
            }
            summaries.push(summary);
        }
        // 对比长图（≥2 张透视图才拼；列序=变体序）
        let mut comparison: Option<String> = None;
        if views.len() >= 2 {
            let mut pngs = Vec::new();
            for (_, p) in &views {
                pngs.push(
                    std::fs::read(p)
                        .map_err(|e| EngineError::Op(OpError::Internal {
                            reason: format!("读取透视图失败 {p}: {e}"),
                        }))?,
                );
            }
            let stitched = archcore_render::stitch_horizontal(&pngs)
                .map_err(|e| EngineError::Op(OpError::Internal { reason: e }))?;
            let out = spec
                .get("comparison_path")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| {
                    let first = &views[0].1;
                    let parent = std::path::Path::new(first)
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                    parent.join("comparison.png").to_string_lossy().to_string()
                });
            ensure_parent(&out).map_err(EngineError::Op)?;
            std::fs::write(&out, stitched).map_err(|e| {
                EngineError::Op(OpError::Internal {
                    reason: format!("对比图写入失败：{e}"),
                })
            })?;
            comparison = Some(out);
        }
        Ok(serde_json::json!({
            "variants": summaries,
            "columns": views.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            "comparison": comparison,
        }))
    }

    pub fn new() -> Self {
        let mut registry = OpRegistry::new("0.1");
        register_builtins(&mut registry).expect("builtin 注册失败");
        register_mesh_ops(&mut registry).expect("mesh op 注册失败");
        register_io_ops(&mut registry).expect("io op 注册失败");
        register_render_ops(&mut registry).expect("render op 注册失败");
        register_material_ops(&mut registry).expect("material op 注册失败");
        register_deliverable_ops(&mut registry).expect("deliverable op 注册失败");
        register_massing_ops(&mut registry).expect("massing op 注册失败");
        register_furnishing_ops(&mut registry).expect("furnishing op 注册失败");
        Self {
            scene: Scene {
                schema_version: "0.1".into(),
                units: Units::Mm,
                up_axis: UpAxis::ZUp,
                name: "untitled".into(),
                objects: vec![],
                materials: vec![],
                assets: vec![],
                meta: BTreeMap::new(),
            },
            revision: 0,
            registry,
            history: History::new(),
            verify: default_registry(),
            previews: BTreeMap::new(),
            preview_seq: 0,
        }
    }

    pub fn capabilities(&self) -> CapabilityManifest {
        self.registry.capabilities()
    }

    /// 执行 + 验证闸 + 拦截自动回滚
    pub fn execute(&mut self, op: &str, args: &Value) -> Result<OpReport, EngineError> {
        let report = self
            .history
            .execute(&self.registry, &mut self.scene, op, args, op)
            .map_err(EngineError::Op)?;
        if let Err(findings) = self.verify.gate(&self.scene, Some(&report)) {
            let _ = self.history.undo(&mut self.scene); // 拦截 = 自动回滚
            return Err(EngineError::Verify(findings));
        }
        self.revision += 1;
        Ok(report)
    }

    /// 脚本执行：全有回滚（对齐 SolidCraft batch 语义）
    pub fn execute_script(
        &mut self,
        steps: &[(String, Value)],
    ) -> Result<Vec<OpReport>, (usize, EngineError)> {
        let snapshot = self.scene.clone();
        let depth = self.history.undo_depth();
        let rev = self.revision;
        let mut reports = Vec::new();
        for (i, (op, args)) in steps.iter().enumerate() {
            match self.execute(op, args) {
                Ok(r) => reports.push(r),
                Err(e) => {
                    self.scene = snapshot;
                    while self.history.undo_depth() > depth {
                        let _ = self.history.undo(&mut self.scene);
                    }
                    self.revision = rev;
                    return Err((i, e));
                }
            }
        }
        Ok(reports)
    }

    pub fn undo(&mut self) -> Result<OpReport, EngineError> {
        self.history.undo(&mut self.scene).map_err(EngineError::Op)
    }

    pub fn checkpoint(&mut self, label: &str) -> String {
        self.history.checkpoint(&self.scene, label)
    }

    pub fn restore_checkpoint(&mut self, id: &str) -> Result<(), EngineError> {
        self.history
            .restore_checkpoint(&mut self.scene, id, false)
            .map_err(EngineError::Op)
    }

    /// detached 预演：在场景克隆上跑步骤，每步过验证闸；活文档纹丝不动
    pub fn preview(
        &mut self,
        label: &str,
        steps: &[(String, Value)],
    ) -> Result<PreviewSummary, (usize, EngineError)> {
        let mut scene = self.scene.clone();
        let mut reports = Vec::new();
        for (i, (op, args)) in steps.iter().enumerate() {
            match self.registry.execute(&mut scene, op, args) {
                Ok(report) => {
                    if let Err(findings) = self.verify.gate(&scene, Some(&report)) {
                        return Err((i, EngineError::Verify(findings)));
                    }
                    reports.push(report);
                }
                Err(e) => return Err((i, EngineError::Op(e))),
            }
        }
        self.preview_seq += 1;
        let id = format!("preview-{}", self.preview_seq);
        let summary = PreviewSummary {
            id: id.clone(),
            label: label.to_string(),
            base_revision: self.revision,
            reports,
            facts_before: archcore_ops::geo_facts(&self.scene),
            facts_after: archcore_ops::geo_facts(&scene),
        };
        self.previews.insert(
            id,
            PreviewRecord {
                summary: summary.clone(),
                scene_after: scene,
                status: PreviewStatus::Pending,
            },
        );
        Ok(summary)
    }

    pub fn list_previews(&self) -> Vec<(String, PreviewStatus)> {
        self.previews
            .iter()
            .map(|(id, r)| (id.clone(), r.status))
            .collect()
    }

    /// 修订号安全提交：审阅期间文档被改则拒绝（丢失更新防线）
    pub fn commit(&mut self, id: &str) -> Result<PreviewSummary, EngineError> {
        let rec = self.previews.get_mut(id).ok_or_else(|| {
            EngineError::Op(OpError::NotFound {
                what: "preview".into(),
                id: id.into(),
            })
        })?;
        if rec.status != PreviewStatus::Pending {
            return Err(EngineError::Op(OpError::Precondition {
                reason: "预览已关闭".into(),
            }));
        }
        if rec.summary.base_revision != self.revision {
            return Err(EngineError::Conflict {
                expected: rec.summary.base_revision,
                actual: self.revision,
            });
        }
        let before = self.scene.clone();
        self.scene = rec.scene_after.clone();
        rec.status = PreviewStatus::Committed;
        let report = OpReport {
            op_id: "preview.commit".into(),
            changed: rec
                .summary
                .reports
                .iter()
                .flat_map(|r| r.changed.clone())
                .collect(),
            facts_before: rec.summary.facts_before.clone(),
            facts_after: rec.summary.facts_after.clone(),
            read_only: false,
            idempotent: false,
            extra: Value::Null,
        };
        let label = rec.summary.label.clone();
        self.history
            .record_snapshot(&label, report, before, self.scene.clone());
        self.revision += 1;
        Ok(rec.summary.clone())
    }

    pub fn reject(&mut self, id: &str) -> Result<(), EngineError> {
        let rec = self.previews.get_mut(id).ok_or_else(|| {
            EngineError::Op(OpError::NotFound {
                what: "preview".into(),
                id: id.into(),
            })
        })?;
        if rec.status != PreviewStatus::Pending {
            return Err(EngineError::Op(OpError::Precondition {
                reason: "预览已关闭".into(),
            }));
        }
        rec.status = PreviewStatus::Rejected;
        Ok(())
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// 导入合流：材质/对象 id 重排、父子与材质绑定同步改写（重复块抽取的回归锁）
    #[test]
    fn merge_imported_remaps_ids_and_parent_refs() {
        let mut scene = Scene {
            schema_version: "0.1".into(),
            units: Units::Mm,
            up_axis: UpAxis::ZUp,
            name: "host".into(),
            objects: vec![],
            materials: vec![Material {
                id: "mat-0".into(),
                name: "已有".into(),
                shading_model: ShadingModel::PbrMetallicRoughness,
                base_color: [0.5; 4],
                metallic: 0.0,
                roughness: 0.8,
                opacity: 1.0,
                textures: Default::default(),
            }],
            assets: vec![],
            meta: BTreeMap::new(),
        };
        let imported = Scene {
            schema_version: "0.1".into(),
            units: Units::Mm,
            up_axis: UpAxis::ZUp,
            name: "imported".into(),
            objects: vec![
                Object {
                    id: "obj-0".into(),
                    name: "组".into(),
                    kind: ObjectKind::Group,
                    parent: None,
                    transform: Transform::default(),
                    visible: true,
                    meta: BTreeMap::new(),
                },
                Object {
                    id: "obj-1".into(),
                    name: "网格".into(),
                    kind: ObjectKind::Mesh {
                        geometry: Geometry::default(),
                        material_slots: vec!["mat-0".into()],
                    },
                    parent: Some("obj-0".into()),
                    transform: Transform::default(),
                    visible: true,
                    meta: BTreeMap::new(),
                },
            ],
            materials: vec![Material {
                id: "mat-0".into(),
                name: "导入材质".into(),
                shading_model: ShadingModel::PbrMetallicRoughness,
                base_color: [1.0; 4],
                metallic: 0.1,
                roughness: 0.7,
                opacity: 1.0,
                textures: Default::default(),
            }],
            assets: vec![],
            meta: BTreeMap::new(),
        };

        let changed = merge_imported(&mut scene, imported);

        // 对象 id 重排为 obj-1/obj-2，返回值即新 id
        assert_eq!(changed, vec!["obj-1".to_string(), "obj-2".to_string()]);
        assert_eq!(scene.objects.len(), 2);
        assert_eq!(scene.objects[0].id, "obj-1");
        assert_eq!(scene.objects[1].id, "obj-2");
        // 父子引用改写到重映射后的父 id
        assert_eq!(scene.objects[1].parent.as_deref(), Some("obj-1"));
        // 材质合流：新 id 唯一（mat-1，不与 host 已有 mat-0 冲突）
        assert_eq!(scene.materials.len(), 2);
        assert_eq!(scene.materials[1].id, "mat-1");
        assert_eq!(scene.materials[1].name, "导入材质");
        // 子网格材质绑定改写到新材质 id
        if let ObjectKind::Mesh { material_slots, .. } = &scene.objects[1].kind {
            assert_eq!(material_slots, &vec!["mat-1".to_string()]);
        } else {
            panic!("应为网格");
        }
    }


    fn quad_steps() -> Vec<(String, Value)> {
        vec![(
            "mesh.create".into(),
            serde_json::json!({
                "name": "墙",
                "positions": [[0,0,0],[1,0,0],[1,1,0],[0,1,0]],
                "indices": [0,1,2, 0,2,3]
            }),
        )]
    }

    /// 端到端：建网格 → 翻对角线 → facts 变化、验证过
    #[test]
    fn end_to_end_mesh_script() {
        let mut e = Engine::new();
        let reports = e.execute_script(&quad_steps()).unwrap();
        assert_eq!(reports[0].facts_after.mesh_count, 1);
        let id = reports[0].changed[0].clone();
        let r = e
            .execute(
                "mesh.flip_edge",
                &serde_json::json!({"id": id, "v0": 0, "v1": 2}),
            )
            .unwrap();
        assert_ne!(r.facts_before.geometry_hash, r.facts_after.geometry_hash);
    }

    /// 验证拦截 + 自动回滚：no-op op 被 gate 逮住，场景复原
    #[test]
    fn verification_interception_rolls_back() {
        let mut e = Engine::new();
        // 注册一个静默 no-op 的 op（非幂等）
        e.registry.register(OpSpec {
            def: OpDef {
                id: "test.vacuum".into(),
                label: "真空".into(),
                description: "".into(),
                category: OpCategory::Scene,
                annotations: OpAnnotations {
                    destructive: false,
                    idempotent: false,
                    read_only: false,
                },
                params: vec![],
            },
            handler: Box::new(|_s: &mut Scene, _a: &Value| Ok((vec![], Value::Null))),
        })
        .unwrap();
        let before = e.scene.clone();
        let err = e.execute("test.vacuum", &serde_json::json!({})).unwrap_err();
        assert!(matches!(err, EngineError::Verify(_)));
        assert_eq!(e.scene, before); // 自动回滚
    }

    /// 脚本失败全量回滚（含验证拦截的情况）
    #[test]
    fn script_failure_rolls_back_all() {
        let mut e = Engine::new();
        let mut steps = quad_steps();
        steps.push(("object.delete".into(), serde_json::json!({"id": "obj-404"})));
        let before = e.scene.clone();
        let err = e.execute_script(&steps).unwrap_err();
        assert_eq!(err.0, 1);
        assert_eq!(e.scene, before);
        assert_eq!(e.history.undo_depth(), 0);
    }

    /// 能力清单包含 mesh op（agent 可发现）
    #[test]
    fn capabilities_include_mesh_ops() {
        let cap = Engine::new().capabilities();
        assert!(cap.ops.iter().any(|o| o.id == "mesh.create"));
        assert!(cap.ops.iter().any(|o| o.id == "mesh.flip_edge"));
    }

    /// 拓扑非法直通 op 错误面：建非流形网格被拒
    #[test]
    fn non_manifold_create_rejected() {
        let mut e = Engine::new();
        let err = e
            .execute(
                "mesh.create",
                &serde_json::json!({
                    "name": "坏",
                    "positions": [[0,0,0],[1,0,0],[1,1,0],[-1,1,0]],
                    "indices": [0,1,2, 1,0,3, 0,1,2]
                }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::Op(OpError::Precondition { .. })));
    }

    /// detached 预演：活文档纹丝不动，摘要带事实前后对照
    #[test]
    fn preview_is_detached() {
        let mut e = Engine::new();
        let before = e.scene.clone();
        let p = e.preview("试", &quad_steps()).unwrap();
        assert_eq!(e.scene, before);
        assert_eq!(p.facts_before.object_count, 0);
        assert_eq!(p.facts_after.object_count, 1);
    }

    /// 提交生效：场景入位、修订号 +1、整批一条历史（一次 undo 全回来）
    #[test]
    fn commit_applies_and_bumps_revision() {
        let mut e = Engine::new();
        let p = e.preview("试", &quad_steps()).unwrap();
        assert_eq!(e.revision, 0);
        e.commit(&p.id).unwrap();
        assert_eq!(e.scene.objects.len(), 1);
        assert_eq!(e.revision, 1);
        e.undo().unwrap();
        assert_eq!(e.scene.objects.len(), 0);
    }

    /// 修订冲突拒绝：审阅期间文档被改，提交必须失败（丢失更新防线）
    #[test]
    fn commit_conflict_rejected() {
        let mut e = Engine::new();
        let p = e.preview("试", &quad_steps()).unwrap();
        e.execute("object.create_group", &serde_json::json!({"name":"别人改的"}))
            .unwrap(); // revision -> 1
        let err = e.commit(&p.id).unwrap_err();
        assert!(matches!(
            err,
            EngineError::Conflict {
                expected: 0,
                actual: 1
            }
        ));
        assert_eq!(e.scene.objects.len(), 1); // 预演未被应用
    }

    /// 弃单：场景不动，预览关闭后不可提交
    #[test]
    fn reject_leaves_scene() {
        let mut e = Engine::new();
        let p = e.preview("试", &quad_steps()).unwrap();
        e.reject(&p.id).unwrap();
        assert_eq!(e.scene.objects.len(), 0);
        assert!(e.commit(&p.id).is_err());
    }

    /// 预演失败不留单（步骤显式报错，指名步序号）
    #[test]
    fn preview_failure_leaves_no_record() {
        let mut e = Engine::new();
        let steps = vec![("object.delete".to_string(), serde_json::json!({"id":"obj-404"}))];
        let err = e.preview("坏", &steps).unwrap_err();
        assert_eq!(err.0, 0);
        assert!(e.list_previews().is_empty());
    }

    /// IO op 的 read_only 语义：写文件、不动文档、no-op 检测豁免
    #[test]
    fn export_gltf_writes_file_read_only_semantics() {
        let mut e = Engine::new();
        e.execute_script(&quad_steps()).unwrap();
        let out = std::env::temp_dir().join("archcore_test_export.gltf");
        let report = e
            .execute(
                "io.export_gltf",
                &serde_json::json!({"path": out.to_string_lossy()}),
            )
            .unwrap();
        assert!(report.read_only);
        assert!(out.exists());
    }

    fn box_step(name: &str, x0: f64, y0: f64, z0: f64, x1: f64, y1: f64, z1: f64) -> (String, Value) {
        (
            "mesh.create".into(),
            serde_json::json!({
                "name": name,
                "positions": [[x0,y0,z0],[x1,y0,z0],[x1,y1,z0],[x0,y1,z0],[x0,y0,z1],[x1,y0,z1],[x1,y1,z1],[x0,y1,z1]],
                "indices": [0,2,1,0,3,2,4,5,6,4,6,7,0,1,5,0,5,4,2,3,7,2,7,6,1,2,6,1,6,5,0,4,7,0,7,3]
            }),
        )
    }

    /// 布尔差集：墙开门洞（评测基准任务 1 的核心动作）
    #[test]
    fn boolean_door_opening() {
        let mut e = Engine::new();
        let steps = vec![
            box_step("Wall", 0.0, 0.0, 0.0, 2.0, 0.2, 1.5),
            box_step("Door", 0.7, -0.1, 0.0, 1.3, 0.3, 0.9),
        ];
        e.execute_script(&steps).unwrap();
        let (a, b) = (e.scene.objects[0].id.clone(), e.scene.objects[1].id.clone());
        let r = e
            .execute(
                "mesh.boolean",
                &serde_json::json!({"id_a": a, "id_b": b, "op": "difference", "name": "Wall_开门"}),
            )
            .unwrap();
        assert_eq!(r.changed.len(), 1);
        assert!(r.facts_after.triangle_count > 12); // 门洞切割后几何变复杂
        assert_ne!(r.facts_before.geometry_hash, r.facts_after.geometry_hash);
    }

    /// 切割打空：切割体没碰上 → 显式报错（M0 教训的布尔特化执法）
    #[test]
    fn cut_miss_errors_explicitly() {
        let mut e = Engine::new();
        let steps = vec![
            box_step("Wall", 0.0, 0.0, 0.0, 2.0, 0.2, 1.5),
            box_step("Cutter", 5.0, 5.0, 5.0, 6.0, 6.0, 6.0),
        ];
        e.execute_script(&steps).unwrap();
        let (a, b) = (e.scene.objects[0].id.clone(), e.scene.objects[1].id.clone());
        let err = e
            .execute(
                "mesh.boolean",
                &serde_json::json!({"id_a": a, "id_b": b, "op": "difference"}),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::Op(OpError::Precondition { .. })));
        assert_eq!(e.scene.objects.len(), 2); // 无新对象，场景干净
    }

    /// 材质管线：创建 → 贴图绑定，资产计数入事实面
    #[test]
    fn material_pipeline_ops() {
        let mut e = Engine::new();
        e.execute(
            "material.create",
            &serde_json::json!({"name": "砖", "base_color": [0.6, 0.3, 0.2, 1.0]}),
        )
        .unwrap();
        let mid = e.scene.materials[0].id.clone();
        let r = e
            .execute(
                "material.set_texture",
                &serde_json::json!({"material": mid, "slot": "base_color", "uri": "D:/x/brick.jpg"}),
            )
            .unwrap();
        assert_eq!(r.facts_after.asset_count, 1);
        assert!(matches!(
            e.execute(
                "material.set_texture",
                &serde_json::json!({"material": "mat-404", "slot": "base_color", "uri": "x"})
            ),
            Err(EngineError::Op(OpError::NotFound { .. }))
        ));
    }

    /// 网格 UV：数量与顶点不一致拒绝，一致则入 UV 集
    #[test]
    fn mesh_create_uv_validation() {
        let mut e = Engine::new();
        let err = e
            .execute(
                "mesh.create",
                &serde_json::json!({"name": "U", "positions": [[0,0,0],[1,0,0],[0,1,0]], "indices": [0,1,2], "uv": [[0,0]]}),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::Op(OpError::BadRequest { .. })));
        e.execute(
            "mesh.create",
            &serde_json::json!({"name": "U", "positions": [[0,0,0],[1,0,0],[0,1,0]], "indices": [0,1,2], "uv": [[0,0],[1,0],[0,1]]}),
        )
        .unwrap();
        match &e.scene.objects[0].kind {
            ObjectKind::Mesh { geometry, .. } => assert_eq!(geometry.uv_sets[0].coords.len(), 3),
            _ => panic!("应为网格"),
        }
    }
}
