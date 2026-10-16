//! op 注册表 v0 —— 声明式注册 · 能力发现 · fail-closed 执行
//!
//! 宪法条款（评测基准任务 10 的根基）：任何操作未产生预期效果必须显式报错，
//! 静默 no-op 是最高级缺陷。因此每次执行的 [`OpReport`] 恒带前后 [`GeoFacts`] 快照。
use archcore_ir::{Id, Object, ObjectKind, Scene, Transform};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

// ---------- 定义面（自描述，供 agent 能力发现） ----------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpCategory {
    Object,
    Material,
    Geometry,
    Scene,
    Io,
    Render,
}

/// 行为注解：给 agent 的操作性质提示（对齐 MCP annotations 语义）
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpAnnotations {
    pub destructive: bool,
    pub idempotent: bool,
    pub read_only: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ParamSpec {
    pub name: String,
    pub type_name: String,
    pub required: bool,
    pub description: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OpDef {
    pub id: String,
    pub label: String,
    pub description: String,
    pub category: OpCategory,
    pub annotations: OpAnnotations,
    pub params: Vec<ParamSpec>,
}

/// 能力清单：agent 连上来先拉这份，再决定怎么规划
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CapabilityManifest {
    pub registry_version: String,
    pub ops: Vec<OpDef>,
}

// ---------- 执行面 ----------

/// 几何事实快照：验证回路的断言锚点（可从 IR 确定性导出）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GeoFacts {
    pub object_count: usize,
    pub mesh_count: usize,
    pub triangle_count: usize,
    pub material_count: usize,
    pub asset_count: usize,
    pub hierarchy_hash: u64,
    pub geometry_hash: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OpReport {
    pub op_id: String,
    pub changed: Vec<Id>,
    pub facts_before: GeoFacts,
    pub facts_after: GeoFacts,
    pub read_only: bool,
    pub idempotent: bool,
    /// 结构化返回值通道（审计 #10）：op 专有结果（如渲染 renderer/adapter）从此回传
    #[serde(default)]
    pub extra: Value,
}

/// 结构化错误：错误码可机读，agent 据此自我修正
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum OpError {
    BadRequest { param: String, reason: String },
    NotFound { what: String, id: String },
    Precondition { reason: String },
    Internal { reason: String },
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpError::BadRequest { param, reason } => write!(f, "参数错误 {param}: {reason}"),
            OpError::NotFound { what, id } => write!(f, "{what} 不存在: {id}"),
            OpError::Precondition { reason } => write!(f, "前置条件不满足: {reason}"),
            OpError::Internal { reason } => write!(f, "内部错误: {reason}"),
        }
    }
}
impl std::error::Error for OpError {}

pub type OpHandler = Box<dyn Fn(&mut Scene, &Value) -> Result<(Vec<Id>, Value), OpError> + Send + Sync>;

pub struct OpSpec {
    pub def: OpDef,
    pub handler: OpHandler,
}

pub struct OpRegistry {
    version: String,
    ops: BTreeMap<String, (OpDef, OpHandler)>,
}

impl OpRegistry {
    pub fn new(version: &str) -> Self {
        Self {
            version: version.to_string(),
            ops: BTreeMap::new(),
        }
    }

    /// 声明式注册；重复 id 直接拒绝（注册表自身 fail-closed）
    pub fn register(&mut self, spec: OpSpec) -> Result<(), OpError> {
        if self.ops.contains_key(&spec.def.id) {
            return Err(OpError::Precondition {
                reason: format!("op 重复注册: {}", spec.def.id),
            });
        }
        for p in &spec.def.params {
            if !matches!(p.type_name.as_str(), "string" | "number" | "array" | "boolean") {
                return Err(OpError::Precondition {
                    reason: format!(
                        "op {} 参数 {} 类型名非法：{}（string|number|array|boolean）",
                        spec.def.id, p.name, p.type_name
                    ),
                });
            }
        }
        self.ops.insert(spec.def.id.clone(), (spec.def, spec.handler));
        Ok(())
    }

    pub fn capabilities(&self) -> CapabilityManifest {
        CapabilityManifest {
            registry_version: self.version.clone(),
            ops: self.ops.values().map(|(def, _)| def.clone()).collect(),
        }
    }

    /// fail-closed 执行：未知 op / handler 失败一律显式返回，绝不静默
    pub fn execute(
        &self,
        scene: &mut Scene,
        op_id: &str,
        args: &Value,
    ) -> Result<OpReport, OpError> {
        let (def, handler) = self
            .ops
            .get(op_id)
            .ok_or_else(|| OpError::NotFound {
                what: "op".to_string(),
                id: op_id.to_string(),
            })?;
        // 前置校验（缺参/类型错/未知参拒绝，场景保持不变）：
        // 三层保证后 handler 取值不再面对敌对输入，负例测试锚定此契约。
        let obj = args.as_object().ok_or_else(|| OpError::BadRequest {
            param: "$args".to_string(),
            reason: "args 必须是 JSON 对象".to_string(),
        })?;
        for p in &def.params {
            if let Some(v) = obj.get(&p.name) {
                let ok = match p.type_name.as_str() {
                    "string" => v.is_string(),
                    "number" => v.is_number(),
                    "array" => v.is_array(),
                    "boolean" => v.is_boolean(),
                    _ => true,
                };
                if !ok {
                    return Err(OpError::BadRequest {
                        param: p.name.clone(),
                        reason: format!(
                            "参数类型应为 {}，实为 {}",
                            p.type_name,
                            if v.is_null() { "null" } else { "其他类型" }
                        ),
                    });
                }
            } else if p.required {
                return Err(OpError::BadRequest {
                    param: p.name.clone(),
                    reason: "缺少必填参数".to_string(),
                });
            }
        }
        for k in obj.keys() {
            if !def.params.iter().any(|p| p.name == *k) {
                return Err(OpError::BadRequest {
                    param: k.clone(),
                    reason: "未知参数".to_string(),
                });
            }
        }
        let facts_before = geo_facts(scene);
        let (changed, extra) = handler(scene, args)?;
        let facts_after = geo_facts(scene);
        Ok(OpReport {
            op_id: op_id.to_string(),
            changed,
            facts_before,
            facts_after,
            read_only: def.annotations.read_only,
            idempotent: def.annotations.idempotent,
            extra,
        })
    }
}

/// 类型已过 execute 前置校验；取值助手再验一道（双保险，绝不 panic）
pub fn arg_str<'a>(args: &'a Value, name: &str) -> Result<&'a str, OpError> {
    args.get(name)
        .and_then(|v| v.as_str())
        .ok_or_else(|| OpError::BadRequest {
            param: name.to_string(),
            reason: "参数缺失或非字符串".to_string(),
        })
}

pub fn arg_u64(args: &Value, name: &str) -> Result<u64, OpError> {
    args.get(name)
        .and_then(|v| v.as_u64())
        .ok_or_else(|| OpError::BadRequest {
            param: name.to_string(),
            reason: "参数缺失或非非负整数".to_string(),
        })
}

/// u32 取值（溢出显式拒绝，封死截断 bug 类）
pub fn arg_u32(args: &Value, name: &str) -> Result<u32, OpError> {
    let v = arg_u64(args, name)?;
    u32::try_from(v).map_err(|_| OpError::BadRequest {
        param: name.to_string(),
        reason: format!("超出 u32 范围：{v}"),
    })
}

pub fn arg_f64(args: &Value, name: &str) -> Result<f64, OpError> {
    args.get(name)
        .and_then(|v| v.as_f64())
        .ok_or_else(|| OpError::BadRequest {
            param: name.to_string(),
            reason: "参数缺失或非数值".to_string(),
        })
}

/// 稳定哈希 FNV-1a 64（审计 #7）：DefaultHasher 不保证跨版本稳定，
/// 事实面锚点必须钉死算法与字节序。
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 由 IR 确定性导出的几何事实
pub fn geo_facts(scene: &Scene) -> GeoFacts {
    let mut hbytes: Vec<u8> = Vec::new();
    for t in scene.hierarchy_triples() {
        hbytes.extend_from_slice(serde_json::to_string(&t).unwrap_or_default().as_bytes());
        hbytes.push(b'\n');
    }
    let mut mesh_count = 0;
    let mut triangle_count = 0;
    let mut gbytes: Vec<u8> = Vec::new();
    for o in &scene.objects {
        if let ObjectKind::Mesh { geometry, .. } = &o.kind {
            mesh_count += 1;
            triangle_count += geometry.indices.len() / 3;
            for p in &geometry.positions {
                for c in p {
                    gbytes.extend_from_slice(&c.to_bits().to_le_bytes());
                }
            }
            for i in &geometry.indices {
                gbytes.extend_from_slice(&i.to_le_bytes());
            }
        }
    }
    GeoFacts {
        object_count: scene.objects.len(),
        mesh_count,
        triangle_count,
        material_count: scene.materials.len(),
        asset_count: scene.assets.len(),
        hierarchy_hash: fnv1a64(&hbytes),
        geometry_hash: fnv1a64(&gbytes),
    }
}

// ---------- 内置 op（范例实现，兼作注册范式） ----------

/// 矩阵转四元数（列主序：col0=right, col1=up, col2=backward）
fn mat3_to_quat(m: [[f64; 3]; 3]) -> [f64; 4] {
    let tr = m[0][0] + m[1][1] + m[2][2];
    let (x, y, z, w);
    if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        w = 0.25 * s;
        x = (m[2][1] - m[1][2]) / s;
        y = (m[0][2] - m[2][0]) / s;
        z = (m[1][0] - m[0][1]) / s;
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        w = (m[2][1] - m[1][2]) / s;
        x = 0.25 * s;
        y = (m[0][1] + m[1][0]) / s;
        z = (m[0][2] + m[2][0]) / s;
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        w = (m[0][2] - m[2][0]) / s;
        x = (m[0][1] + m[1][0]) / s;
        y = 0.25 * s;
        z = (m[1][2] + m[2][1]) / s;
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        w = (m[1][0] - m[0][1]) / s;
        x = (m[0][2] + m[2][0]) / s;
        y = (m[1][2] + m[2][1]) / s;
        z = 0.25 * s;
    }
    let n = (x * x + y * y + z * z + w * w).sqrt().max(1e-12);
    [x / n, y / n, z / n, w / n]
}

/// 四元数乘法 q1⊗q2（对向量先作用 q2 再 q1）
fn quat_mul(q1: [f64; 4], q2: [f64; 4]) -> [f64; 4] {
    let (x1, y1, z1, w1) = (q1[0], q1[1], q1[2], q1[3]);
    let (x2, y2, z2, w2) = (q2[0], q2[1], q2[2], q2[3]);
    [
        w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
        w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2,
        w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2,
        w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2,
    ]
}

/// look-at 四元数：相机本地 -Z 对准 target（Z-up 世界）
pub fn look_at_quat(pos: [f64; 3], target: [f64; 3]) -> [f64; 4] {
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot3 = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: [f64; 3], b: [f64; 3]| [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let norm = |v: [f64; 3]| {
        let l = dot3(v, v).sqrt().max(1e-12);
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let fwd = norm(sub(target, pos));
    let up_hint = if fwd[2].abs() > 0.99 {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let right = norm(cross(fwd, up_hint));
    let upv = cross(right, fwd);
    let back = [-fwd[0], -fwd[1], -fwd[2]];
    mat3_to_quat([
        [right[0], upv[0], back[0]],
        [right[1], upv[1], back[1]],
        [right[2], upv[2], back[2]],
    ])
}

/// 稳定 ID 分配：单调计数器（永不复用——宪法"稳定 ID 不复用"的执行器）
pub fn next_id(scene: &mut Scene) -> Id {
    let max_existing = scene
        .objects
        .iter()
        .filter_map(|o| o.id.strip_prefix("obj-").and_then(|s| s.parse::<u64>().ok()))
        .max()
        .unwrap_or(0);
    seq_next(scene, "obj-", "id_seq", max_existing)
}

/// 材质 id 单调分配（审计 #6：禁 len 派生）
pub fn next_mat_id(scene: &mut Scene) -> Id {
    let max_existing = scene
        .materials
        .iter()
        .filter_map(|m| m.id.strip_prefix("mat-").and_then(|s| s.parse::<u64>().ok()))
        .max()
        .unwrap_or(0);
    seq_next(scene, "mat-", "mat_seq", max_existing)
}

/// 资产 id 单调分配（审计 #6）
pub fn next_asset_id(scene: &mut Scene) -> Id {
    let max_existing = scene
        .assets
        .iter()
        .filter_map(|a| a.id.strip_prefix("asset-").and_then(|s| s.parse::<u64>().ok()))
        .max()
        .unwrap_or(0);
    seq_next(scene, "asset-", "asset_seq", max_existing)
}

fn seq_next(scene: &mut Scene, prefix: &str, key: &str, max_existing: u64) -> Id {
    let seq = scene
        .meta
        .get(key)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let n = max_existing.max(seq) + 1;
    scene.meta.insert(key.to_string(), n.to_string());
    format!("{prefix}{n}")
}

/// 外部场景载入后同步计数器（审计 #6）：计数器不得低于现存最大 id，
/// 否则 delete 最高 id 后新建即复用——load_scene/import 后必调。
pub fn sync_id_seqs(scene: &mut Scene) {
    let scans: [( &str, u64 ); 3] = [
        (
            "id_seq",
            scene
                .objects
                .iter()
                .filter_map(|o| o.id.strip_prefix("obj-").and_then(|s| s.parse::<u64>().ok()))
                .max()
                .unwrap_or(0),
        ),
        (
            "mat_seq",
            scene
                .materials
                .iter()
                .filter_map(|m| m.id.strip_prefix("mat-").and_then(|s| s.parse::<u64>().ok()))
                .max()
                .unwrap_or(0),
        ),
        (
            "asset_seq",
            scene
                .assets
                .iter()
                .filter_map(|a| a.id.strip_prefix("asset-").and_then(|s| s.parse::<u64>().ok()))
                .max()
                .unwrap_or(0),
        ),
    ];
    for (key, max_existing) in scans {
        let seq = scene
            .meta
            .get(key)
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        if seq < max_existing {
            scene.meta.insert(key.to_string(), max_existing.to_string());
        }
    }
}

pub fn register_builtins(reg: &mut OpRegistry) -> Result<(), OpError> {
    reg.register(OpSpec {
        def: OpDef {
            id: "object.create_group".into(),
            label: "创建组".into(),
            description: "创建层级组节点，返回稳定 id".into(),
            category: OpCategory::Object,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![
                ParamSpec {
                    name: "name".into(),
                    type_name: "string".into(),
                    required: true,
                    description: "组名".into(),
                },
                ParamSpec {
                    name: "parent".into(),
                    type_name: "string".into(),
                    required: false,
                    description: "父节点 id（缺省为根）".into(),
                },
            ],
        },
        handler: Box::new(|scene, args| {
            let name = args["name"].as_str().unwrap().to_string();
            let parent = args.get("parent").and_then(|v| v.as_str()).map(String::from);
            if let Some(p) = &parent {
                if !scene.objects.iter().any(|o| &o.id == p) {
                    return Err(OpError::NotFound {
                        what: "parent".into(),
                        id: p.clone(),
                    });
                }
            }
            let id = next_id(scene);
            scene.objects.push(Object {
                id: id.clone(),
                name,
                kind: ObjectKind::Group,
                parent,
                transform: Transform::default(),
                visible: true,
                meta: BTreeMap::new(),
            });
            Ok((vec![id], Value::Null))
        }),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "object.delete".into(),
            label: "删除对象".into(),
            description: "按 id 删除对象；**目标不存在必须报错，禁止静默成功**".into(),
            category: OpCategory::Object,
            annotations: OpAnnotations {
                destructive: true,
                idempotent: true,
                read_only: false,
            },
            params: vec![ParamSpec {
                name: "id".into(),
                type_name: "string".into(),
                required: true,
                description: "目标对象 id".into(),
            }],
        },
        handler: Box::new(|scene, args| {
            let id = args["id"].as_str().unwrap().to_string();
            let Some(pos) = scene.objects.iter().position(|o| o.id == id) else {
                // 宪法条款：显式报错
                return Err(OpError::NotFound {
                    what: "object".into(),
                    id,
                });
            };
            scene.objects.remove(pos);
            Ok((vec![id], Value::Null))
        }),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "object.rename".into(),
            label: "重命名对象".into(),
            description: "按 id 重命名；目标不存在报错".into(),
            category: OpCategory::Object,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: false,
            },
            params: vec![
                ParamSpec {
                    name: "id".into(),
                    type_name: "string".into(),
                    required: true,
                    description: "目标对象 id".into(),
                },
                ParamSpec {
                    name: "name".into(),
                    type_name: "string".into(),
                    required: true,
                    description: "新名字".into(),
                },
            ],
        },
        handler: Box::new(|scene, args| {
            let id = args["id"].as_str().unwrap().to_string();
            let name = args["name"].as_str().unwrap().to_string();
            let Some(obj) = scene.objects.iter_mut().find(|o| o.id == id) else {
                return Err(OpError::NotFound {
                    what: "object".into(),
                    id,
                });
            };
            obj.name = name;
            Ok((vec![id], Value::Null))
        }),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "scene.facts".into(),
            label: "读取几何事实".into(),
            description: "返回当前场景的几何事实快照（只读）".into(),
            category: OpCategory::Scene,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: true,
            },
            params: vec![],
        },
        handler: Box::new(|_scene, _args| Ok((vec![], Value::Null))),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "object.set_parent".into(),
            label: "设置父级".into(),
            description: "重组层级；parent 省略则移到根；环引用显式拒绝".into(),
            category: OpCategory::Object,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: false,
            },
            params: vec![
                ParamSpec {
                    name: "id".into(),
                    type_name: "string".into(),
                    required: true,
                    description: "目标对象 id".into(),
                },
                ParamSpec {
                    name: "parent".into(),
                    type_name: "string".into(),
                    required: false,
                    description: "新父 id（省略 = 移到根）".into(),
                },
            ],
        },
        handler: Box::new(|scene, args| {
            let id = args["id"].as_str().unwrap().to_string();
            let parent = args.get("parent").and_then(|v| v.as_str()).map(String::from);
            if !scene.objects.iter().any(|o| o.id == id) {
                return Err(OpError::NotFound {
                    what: "object".into(),
                    id,
                });
            }
            if let Some(p) = &parent {
                if *p == id {
                    return Err(OpError::Precondition {
                        reason: "不能成为自己的父级".into(),
                    });
                }
                let mut cur = Some(p.as_str());
                while let Some(c) = cur {
                    if c == id {
                        return Err(OpError::Precondition {
                            reason: "父级关系成环".into(),
                        });
                    }
                    cur = scene
                        .objects
                        .iter()
                        .find(|o| o.id == c)
                        .and_then(|o| o.parent.as_deref());
                }
                if !scene.objects.iter().any(|o| o.id == *p) {
                    return Err(OpError::NotFound {
                        what: "parent".into(),
                        id: p.clone(),
                    });
                }
            }
            let changed = id.clone();
            let obj = scene.objects.iter_mut().find(|o| o.id == id).unwrap();
            obj.parent = parent;
            Ok((vec![changed], Value::Null))
        }),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "object.duplicate".into(),
            label: "复制子树".into(),
            description: "克隆目标及其全部后代（id 重排、父引用重映射）；translate 打在根上".into(),
            category: OpCategory::Object,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![
                ParamSpec {
                    name: "id".into(),
                    type_name: "string".into(),
                    required: true,
                    description: "子树根 id".into(),
                },
                ParamSpec {
                    name: "name".into(),
                    type_name: "string".into(),
                    required: false,
                    description: "副本根名字（默认原名+副本）".into(),
                },
                ParamSpec {
                    name: "translate".into(),
                    type_name: "array".into(),
                    required: false,
                    description: "根平移偏移 [x,y,z]".into(),
                },
            ],
        },
        handler: Box::new(|scene, args| {
            let id = args["id"].as_str().unwrap().to_string();
            if !scene.objects.iter().any(|o| o.id == id) {
                return Err(OpError::NotFound {
                    what: "object".into(),
                    id,
                });
            }
            let translate: [f64; 3] = args
                .get("translate")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or([0.0; 3]);
            let new_name = args
                .get("name")
                .and_then(|v| v.as_str())
                .map(String::from);
            // 收子树（BFS）
            let mut members = vec![id.clone()];
            let mut i = 0;
            while i < members.len() {
                let cur = members[i].clone();
                for o in &scene.objects {
                    if o.parent.as_deref() == Some(cur.as_str()) && !members.contains(&o.id) {
                        members.push(o.id.clone());
                    }
                }
                i += 1;
            }
            // id 重排（单调计数器，不复用）
            let mut map = std::collections::BTreeMap::new();
            for m in &members {
                let nid = next_id(scene);
                map.insert(m.clone(), nid);
            }
            let clones: Vec<Object> = members
                .iter()
                .map(|m| {
                    let src = scene.objects.iter().find(|o| o.id == *m).unwrap();
                    let mut c = src.clone();
                    c.id = map[m].clone();
                    if m == &id {
                        c.name = new_name.clone().unwrap_or_else(|| format!("{} 副本", src.name));
                        c.transform.translation = [
                            c.transform.translation[0] + translate[0],
                            c.transform.translation[1] + translate[1],
                            c.transform.translation[2] + translate[2],
                        ];
                    }
                    if let Some(p) = &c.parent {
                        c.parent = Some(map.get(p).cloned().unwrap_or_else(|| p.clone()));
                    }
                    c
                })
                .collect();
            let changed: Vec<String> = clones.iter().map(|c| c.id.clone()).collect();
            scene.objects.extend(clones);
            Ok((changed, Value::Null))
        }),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "light.create".into(),
            label: "创建灯光".into(),
            description: "area/point/spot/sun；路径追踪直接消费（sun v0 并入自动补光）".into(),
            category: OpCategory::Object,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![
                ParamSpec {
                    name: "name".into(),
                    type_name: "string".into(),
                    required: true,
                    description: "灯光名".into(),
                },
                ParamSpec {
                    name: "light_kind".into(),
                    type_name: "string".into(),
                    required: true,
                    description: "area|point|spot|sun".into(),
                },
                ParamSpec {
                    name: "color".into(),
                    type_name: "array".into(),
                    required: false,
                    description: "[r,g,b]（默认白）".into(),
                },
                ParamSpec {
                    name: "intensity".into(),
                    type_name: "number".into(),
                    required: false,
                    description: "强度（默认 10）".into(),
                },
                ParamSpec {
                    name: "size".into(),
                    type_name: "number".into(),
                    required: false,
                    description: "面积光边长".into(),
                },
                ParamSpec {
                    name: "translation".into(),
                    type_name: "array".into(),
                    required: false,
                    description: "位置 [x,y,z]".into(),
                },
            ],
        },
        handler: Box::new(|scene, args| {
            let name = args["name"].as_str().unwrap().to_string();
            let light_kind = match args["light_kind"].as_str().unwrap() {
                "area" => archcore_ir::LightKind::Area,
                "point" => archcore_ir::LightKind::Point,
                "spot" => archcore_ir::LightKind::Spot,
                "sun" => archcore_ir::LightKind::Sun,
                other => {
                    return Err(OpError::BadRequest {
                        param: "light_kind".into(),
                        reason: format!("未知灯光类型: {other}"),
                    })
                }
            };
            let color: [f64; 3] = args
                .get("color")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or([1.0; 3]);
            let translation: [f64; 3] = args
                .get("translation")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or([0.0; 3]);
            let id = next_id(scene);
            scene.objects.push(Object {
                id: id.clone(),
                name,
                kind: ObjectKind::Light {
                    light_kind,
                    color,
                    intensity: args.get("intensity").and_then(|v| v.as_f64()).unwrap_or(10.0),
                    size: args.get("size").and_then(|v| v.as_f64()),
                    cone: None,
                },
                parent: None,
                transform: Transform {
                    translation,
                    ..Default::default()
                },
                visible: true,
                meta: BTreeMap::new(),
            });
            Ok((vec![id], Value::Null))
        }),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "camera.create".into(),
            label: "创建相机".into(),
            description: "透视相机；路径追踪渲染优先使用".into(),
            category: OpCategory::Object,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![
                ParamSpec {
                    name: "name".into(),
                    type_name: "string".into(),
                    required: true,
                    description: "相机名".into(),
                },
                ParamSpec {
                    name: "lens_mm".into(),
                    type_name: "number".into(),
                    required: false,
                    description: "焦距（默认 35）".into(),
                },
                ParamSpec {
                    name: "translation".into(),
                    type_name: "array".into(),
                    required: false,
                    description: "位置 [x,y,z]".into(),
                },
                ParamSpec {
                    name: "target".into(),
                    type_name: "array".into(),
                    required: false,
                    description: "取景目标 [x,y,z]（自动 look-at）".into(),
                },
            ],
        },
        handler: Box::new(|scene, args| {
            let name = args["name"].as_str().unwrap().to_string();
            let translation: [f64; 3] = args
                .get("translation")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or([0.0; 3]);
            let rotation = match args
                .get("target")
                .and_then(|v| serde_json::from_value::<[f64; 3]>(v.clone()).ok())
            {
                Some(t) => look_at_quat(translation, t),
                None => [0.0, 0.0, 0.0, 1.0],
            };
            let id = next_id(scene);
            scene.objects.push(Object {
                id: id.clone(),
                name,
                kind: ObjectKind::Camera {
                    lens_mm: args.get("lens_mm").and_then(|v| v.as_f64()).unwrap_or(35.0),
                    sensor_mm: 36.0,
                    clip: [0.1, 1000.0],
                },
                parent: None,
                transform: Transform {
                    translation,
                    rotation,
                    ..Default::default()
                },
                visible: true,
                meta: BTreeMap::new(),
            });
            Ok((vec![id], Value::Null))
        }),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "camera.aim".into(),
            label: "相机取景".into(),
            description: "相机 look-at 对准目标（本地 -Z 朝向目标）".into(),
            category: OpCategory::Object,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: true,
                read_only: false,
            },
            params: vec![
                ParamSpec {
                    name: "id".into(),
                    type_name: "string".into(),
                    required: true,
                    description: "相机 id".into(),
                },
                ParamSpec {
                    name: "target".into(),
                    type_name: "array".into(),
                    required: true,
                    description: "目标点 [x,y,z]".into(),
                },
            ],
        },
        handler: Box::new(|scene, args| {
            let id = args["id"].as_str().unwrap().to_string();
            let target: [f64; 3] =
                serde_json::from_value(args["target"].clone()).map_err(|e| OpError::BadRequest {
                    param: "target".into(),
                    reason: e.to_string(),
                })?;
            let Some(cam) = scene.objects.iter_mut().find(|o| o.id == id) else {
                return Err(OpError::NotFound {
                    what: "camera".into(),
                    id,
                });
            };
            if !matches!(cam.kind, ObjectKind::Camera { .. }) {
                return Err(OpError::BadRequest {
                    param: "id".into(),
                    reason: "目标对象不是相机".into(),
                });
            }
            let changed = cam.id.clone();
            cam.transform.rotation = look_at_quat(cam.transform.translation, target);
            Ok((vec![changed], Value::Null))
        }),
    })?;

    reg.register(OpSpec {
        def: OpDef {
            id: "object.transform".into(),
            label: "变换对象".into(),
            description: "增量变换：平移/绕 Z 旋转/缩放；对组生效时整棵子树跟随（搬家具/换层用）".into(),
            category: OpCategory::Object,
            annotations: OpAnnotations {
                destructive: false,
                idempotent: false,
                read_only: false,
            },
            params: vec![
                ParamSpec {
                    name: "id".into(),
                    type_name: "string".into(),
                    required: true,
                    description: "对象 id".into(),
                },
                ParamSpec {
                    name: "translate".into(),
                    type_name: "array".into(),
                    required: false,
                    description: "平移增量 [x,y,z]".into(),
                },
                ParamSpec {
                    name: "rotate_z".into(),
                    type_name: "number".into(),
                    required: false,
                    description: "绕 Z 旋转增量（度）".into(),
                },
                ParamSpec {
                    name: "scale".into(),
                    type_name: "number".into(),
                    required: false,
                    description: "缩放倍数（默认 1，须为正）".into(),
                },
            ],
        },
        handler: Box::new(|scene, args| {
            let id = args["id"].as_str().unwrap().to_string();
            let translate: [f64; 3] = args
                .get("translate")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or([0.0; 3]);
            let rotate_z = args.get("rotate_z").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let s = args.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0);
            if !(s.is_finite() && s > 0.0) {
                return Err(OpError::BadRequest {
                    param: "scale".into(),
                    reason: "须为正数".into(),
                });
            }
            let Some(o) = scene.objects.iter_mut().find(|o| o.id == id) else {
                return Err(OpError::NotFound {
                    what: "object".into(),
                    id,
                });
            };
            o.transform.translation = [
                o.transform.translation[0] + translate[0],
                o.transform.translation[1] + translate[1],
                o.transform.translation[2] + translate[2],
            ];
            if rotate_z != 0.0 {
                let h = rotate_z.to_radians() / 2.0;
                let dq = [0.0, 0.0, h.sin(), h.cos()];
                o.transform.rotation = quat_mul(o.transform.rotation, dq);
            }
            if s != 1.0 {
                o.transform.scale = [
                    o.transform.scale[0] * s,
                    o.transform.scale[1] * s,
                    o.transform.scale[2] * s,
                ];
            }
            let changed = o.id.clone();
            Ok((vec![changed], Value::Null))
        }),
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use archcore_ir::{Units, UpAxis};

    fn empty_scene() -> Scene {
        Scene {
            schema_version: "0.1".into(),
            units: Units::Mm,
            up_axis: UpAxis::ZUp,
            name: "t".into(),
            objects: vec![],
            materials: vec![],
            assets: vec![],
            meta: BTreeMap::new(),
        }
    }

    fn registry() -> OpRegistry {
        let mut reg = OpRegistry::new("0.1");
        register_builtins(&mut reg).unwrap();
        reg
    }

    #[test]
    fn capabilities_lists_builtins() {
        let cap = registry().capabilities();
        assert_eq!(cap.registry_version, "0.1");
        assert_eq!(cap.ops.len(), 10);
        assert!(cap.ops.iter().any(|o| o.id == "object.delete"));
    }

    #[test]
    fn execute_reports_facts_snapshot() {
        let reg = registry();
        let mut scene = empty_scene();
        let report = reg
            .execute(
                &mut scene,
                "object.create_group",
                &serde_json::json!({"name": "首层"}),
            )
            .unwrap();
        assert_eq!(report.facts_before.object_count, 0);
        assert_eq!(report.facts_after.object_count, 1);
        assert_ne!(report.facts_before.hierarchy_hash, report.facts_after.hierarchy_hash);
    }

    /// 宪法条款：删除不存在的对象必须显式报错，静默 no-op = 缺陷
    #[test]
    fn delete_missing_object_errors_explicitly() {
        let reg = registry();
        let mut scene = empty_scene();
        let err = reg
            .execute(&mut scene, "object.delete", &serde_json::json!({"id": "obj-404"}))
            .unwrap_err();
        assert_eq!(
            err,
            OpError::NotFound {
                what: "object".into(),
                id: "obj-404".into()
            }
        );
    }

    /// fail-closed：缺必填参数拒绝执行，场景保持不变
    #[test]
    fn missing_required_param_rejected() {
        let reg = registry();
        let mut scene = empty_scene();
        let before = geo_facts(&scene);
        let err = reg
            .execute(&mut scene, "object.create_group", &serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, OpError::BadRequest { .. }));
        assert_eq!(geo_facts(&scene), before);
    }

    /// 未知 op 显式拒绝（能力发现的反面兜底）
    #[test]
    fn unknown_op_rejected() {
        let reg = registry();
        let mut scene = empty_scene();
        let err = reg
            .execute(&mut scene, "object.teleport", &serde_json::json!({}))
            .unwrap_err();
        assert!(matches!(err, OpError::NotFound { what, .. } if what == "op"));
    }

    /// 层级成环显式拒绝（set_parent 的宪法防线）
    #[test]
    fn set_parent_cycle_rejected() {
        let reg = registry();
        let mut scene = empty_scene();
        reg.execute(&mut scene, "object.create_group", &serde_json::json!({"name":"A"}))
            .unwrap();
        reg.execute(
            &mut scene,
            "object.create_group",
            &serde_json::json!({"name":"B", "parent":"obj-1"}),
        )
        .unwrap();
        let err = reg
            .execute(
                &mut scene,
                "object.set_parent",
                &serde_json::json!({"id":"obj-1", "parent":"obj-2"}),
            )
            .unwrap_err();
        assert!(matches!(err, OpError::Precondition { .. }));
    }

    /// 子树复制：后代一并克隆、id 不复用、父引用重映射
    #[test]
    fn duplicate_clones_subtree() {
        let reg = registry();
        let mut scene = empty_scene();
        reg.execute(&mut scene, "object.create_group", &serde_json::json!({"name":"Floor"}))
            .unwrap();
        reg.execute(
            &mut scene,
            "object.create_group",
            &serde_json::json!({"name":"Wall", "parent":"obj-1"}),
        )
        .unwrap();
        let report = reg
            .execute(
                &mut scene,
                "object.duplicate",
                &serde_json::json!({"id":"obj-1", "translate":[0,0,3]}),
            )
            .unwrap();
        assert_eq!(report.changed.len(), 2); // 根 + 后代
        assert_eq!(scene.objects.len(), 4);
        let root_copy = scene.objects.iter().find(|o| o.id == report.changed[0]).unwrap();
        assert_eq!(root_copy.transform.translation[2], 3.0);
        let child_copy = scene.objects.iter().find(|o| o.id == report.changed[1]).unwrap();
        assert_eq!(child_copy.parent.as_deref(), Some(report.changed[0].as_str()));
    }

    /// look-at 数学：相机 -Z 旋转向量必须指向目标（取景对准回归锁）
    #[test]
    fn look_at_aims_at_target() {
        let pos = [5.0, -5.0, 4.0];
        let target = [0.0, 0.0, 1.0];
        let q = look_at_quat(pos, target);
        let qv = [q[0], q[1], q[2]];
        let cross = |a: [f64; 3], b: [f64; 3]| {
            [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
        };
        let v = [0.0, 0.0, -1.0];
        let t = {
            let c = cross(qv, v);
            [c[0] * 2.0, c[1] * 2.0, c[2] * 2.0]
        };
        let c2 = cross(qv, t);
        let v2 = [
            v[0] + t[0] * q[3] + c2[0],
            v[1] + t[1] * q[3] + c2[1],
            v[2] + t[2] * q[3] + c2[2],
        ];
        let d = [target[0] - pos[0], target[1] - pos[1], target[2] - pos[2]];
        let dl = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        for i in 0..3 {
            assert!((v2[i] - d[i] / dl).abs() < 1e-9, "轴{i} 未对准");
        }
    }

    /// id 不复用回归锁：删除后新建绝不复用（宪法执行器的行为契约）
    #[test]
    fn ids_never_reused() {
        let reg = registry();
        let mut scene = empty_scene();
        reg.execute(&mut scene, "object.create_group", &serde_json::json!({"name":"A"}))
            .unwrap();
        reg.execute(&mut scene, "object.create_group", &serde_json::json!({"name":"B"}))
            .unwrap();
        reg.execute(&mut scene, "object.delete", &serde_json::json!({"id":"obj-2"}))
            .unwrap();
        let r = reg
            .execute(&mut scene, "object.create_group", &serde_json::json!({"name":"C"}))
            .unwrap();
        assert_eq!(r.changed[0], "obj-3"); // 绝不复用 obj-2
    }
}
