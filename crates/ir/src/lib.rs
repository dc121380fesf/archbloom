//! 场景 IR v0 —— schema 单一来源的 Rust 侧实现。
//! 规范见 docs/ir-schema-v0.md；JSON Schema 见 schema/scene_ir.v0.schema.json。
//! 精度口径：内部 f64；FBX/glTF 导出落盘为 f32（容差 1e-5），非全字长保真。
//! 变更纪律：三处同步 + schema_version 递增 + 迁移说明。
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type Id = String;

fn default_true() -> bool {
    true
}

/// 场景物理单位（必填，禁止隐式默认——FBX 0.0254 单位鬼影的防线）
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Units {
    Mm,
    Cm,
    M,
    In,
    Ft,
}

/// 上轴约定（必填）
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpAxis {
    YUp,
    ZUp,
}

/// 变换：旋转用四元数 [x, y, z, w]，规避万向锁
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Transform {
    pub translation: [f64; 3],
    pub rotation: [f64; 4],
    pub scale: [f64; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LightKind {
    Area,
    Point,
    Spot,
    Sun,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UvSet {
    pub name: String,
    pub coords: Vec<[f64; 2]>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Geometry {
    pub positions: Vec<[f64; 3]>,
    pub indices: Vec<u32>,
    pub normals: Option<Vec<[f64; 3]>>,
    pub uv_sets: Vec<UvSet>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObjectKind {
    Mesh {
        #[serde(default)]
        geometry: Geometry,
        #[serde(default)]
        material_slots: Vec<Id>,
    },
    Group,
    Instance {
        source: Id,
    },
    Light {
        light_kind: LightKind,
        color: [f64; 3],
        intensity: f64,
        #[serde(default)]
        size: Option<f64>,
        #[serde(default)]
        cone: Option<[f64; 2]>,
    },
    Camera {
        lens_mm: f64,
        sensor_mm: f64,
        clip: [f64; 2],
    },
}

/// 场景图节点。id 不可变、跨编辑存续、不复用（agent 按 id 寻址）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Object {
    pub id: Id,
    pub name: String,
    #[serde(flatten)]
    pub kind: ObjectKind,
    #[serde(default)]
    pub parent: Option<Id>,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default)]
    pub meta: BTreeMap<String, String>,
}

/// 材质模型；经第三方工具往返退化时必须置 Unknown 并由验证层报出
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ShadingModel {
    PbrMetallicRoughness,
    Unknown,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Textures {
    pub base_color: Option<Id>,
    pub normal: Option<Id>,
    pub roughness: Option<Id>,
    pub metallic: Option<Id>,
    pub ao: Option<Id>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Material {
    pub id: Id,
    pub name: String,
    pub shading_model: ShadingModel,
    pub base_color: [f64; 4],
    pub metallic: f64,
    pub roughness: f64,
    pub opacity: f64,
    #[serde(default)]
    pub textures: Textures,
}

/// 外部资产引用；授权信息随模型走（版权链）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AssetRef {
    pub id: Id,
    pub uri: String,
    pub kind: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

/// 场景根
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Scene {
    pub schema_version: String,
    pub units: Units,
    pub up_axis: UpAxis,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub objects: Vec<Object>,
    #[serde(default)]
    pub materials: Vec<Material>,
    #[serde(default)]
    pub assets: Vec<AssetRef>,
    #[serde(default)]
    pub meta: BTreeMap<String, String>,
}

impl Scene {
    /// 几何事实断言的锚点之一：层级树哈希输入（id + name + parent 三元组排序）
    pub fn hierarchy_triples(&self) -> Vec<(Id, String, Option<Id>)> {
        let mut v: Vec<_> = self
            .objects
            .iter()
            .map(|o| (o.id.clone(), o.name.clone(), o.parent.clone()))
            .collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_scene() -> Scene {
        Scene {
            schema_version: "0.1".into(),
            units: Units::Mm,
            up_axis: UpAxis::ZUp,
            name: "test".into(),
            objects: vec![Object {
                id: "obj-1".into(),
                name: "墙_01".into(),
                kind: ObjectKind::Group,
                parent: None,
                transform: Transform::default(),
                visible: true,
                meta: BTreeMap::new(),
            }],
            materials: Vec::new(),
            assets: Vec::new(),
            meta: BTreeMap::new(),
        }
    }

    /// IR 必须可无损 JSON 往返（schema 契约的地基测试）
    #[test]
    fn json_round_trip() {
        let s = minimal_scene();
        let text = serde_json::to_string(&s).unwrap();
        let back: Scene = serde_json::from_str(&text).unwrap();
        assert_eq!(s, back);
    }

    /// 非 ASCII 名字必须原样保留（FBX/Max 往返实测教训）
    #[test]
    fn non_ascii_name_survives() {
        let s = minimal_scene();
        let text = serde_json::to_string(&s).unwrap();
        let back: Scene = serde_json::from_str(&text).unwrap();
        assert_eq!(back.objects[0].name, "墙_01");
    }

    /// 稳定 ID + 层级三元组（验证回路断言锚点）
    #[test]
    fn hierarchy_triples_sorted() {
        let s = minimal_scene();
        assert_eq!(s.hierarchy_triples().len(), 1);
        assert_eq!(s.hierarchy_triples()[0].0, "obj-1");
    }

    /// 单位/轴向必填：缺字段必须解析失败，禁止静默默认
    #[test]
    fn units_required() {
        let bad = r#"{"schema_version":"0.1","up_axis":"z_up"}"#;
        assert!(serde_json::from_str::<Scene>(bad).is_err());
    }
}
