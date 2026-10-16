//! 验证回路 v0 —— checker 注册表 + 静默 no-op 检测 + fail-closed 门禁
//!
//! 宪法条款的产品级落地：任何操作未产生预期几何事实必须显式报错。
//! checker 可插拔：`VerifyRegistry::register` 接受任意 `Fn(&Scene, Option<&OpReport>) -> Vec<Finding>`。
use archcore_ir::{ObjectKind, Scene};
use archcore_ops::OpReport;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub checker: String,
    pub severity: Severity,
    /// 问题主体（对象 id / op id / 范围名）
    pub subject: String,
    pub message: String,
    #[serde(default)]
    pub expected: Option<String>,
    #[serde(default)]
    pub actual: Option<String>,
}

pub type CheckFn = Box<dyn Fn(&Scene, Option<&OpReport>) -> Vec<Finding> + Send + Sync>;

pub struct CheckDef {
    pub id: String,
    pub label: String,
    pub description: String,
}

pub struct VerifyRegistry {
    checks: Vec<(CheckDef, CheckFn)>,
}

impl VerifyRegistry {
    pub fn new() -> Self {
        Self { checks: Vec::new() }
    }

    pub fn register(&mut self, def: CheckDef, f: CheckFn) {
        self.checks.push((def, f));
    }

    pub fn check_ids(&self) -> Vec<String> {
        self.checks.iter().map(|(d, _)| d.id.clone()).collect()
    }

    /// 跑全部 checker，findings 按严重度降序
    pub fn run(&self, scene: &Scene, report: Option<&OpReport>) -> Vec<Finding> {
        let mut out = Vec::new();
        for (_, f) in &self.checks {
            out.extend(f(scene, report));
        }
        out.sort_by_key(|f| std::cmp::Reverse(f.severity));
        out
    }

    /// fail-closed 门禁：存在 Error 级 finding 即整体不过
    pub fn gate(&self, scene: &Scene, report: Option<&OpReport>) -> Result<(), Vec<Finding>> {
        let findings = self.run(scene, report);
        if findings.iter().any(|f| f.severity == Severity::Error) {
            Err(findings)
        } else {
            Ok(())
        }
    }
}

impl Default for VerifyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------- 内置 checker ----------

/// 静默 no-op 检测（宪法条款）：
/// 非只读 op 声称成功但几何事实零变化 —— 非幂等 = Error，幂等 = Warning（保住幂等语义）
pub fn noop_detection(_scene: &Scene, report: Option<&OpReport>) -> Vec<Finding> {
    let Some(r) = report else { return vec![] };
    if r.read_only || !r.changed.is_empty() || r.facts_before != r.facts_after {
        return vec![];
    }
    let severity = if r.idempotent {
        Severity::Warning
    } else {
        Severity::Error
    };
    vec![Finding {
        checker: "op.noop_detection".into(),
        severity,
        subject: r.op_id.clone(),
        message: "操作未产生任何几何事实变化（静默 no-op）".into(),
        expected: Some("几何事实发生变化".into()),
        actual: Some("facts 前后完全一致".into()),
    }]
}

/// 悬空父引用：parent 必须指向存在的对象
pub fn dangling_parent(scene: &Scene, _report: Option<&OpReport>) -> Vec<Finding> {
    let ids: std::collections::BTreeSet<_> = scene.objects.iter().map(|o| o.id.as_str()).collect();
    scene
        .objects
        .iter()
        .filter_map(|o| match &o.parent {
            Some(p) if !ids.contains(p.as_str()) => Some(Finding {
                checker: "structure.dangling_parent".into(),
                severity: Severity::Error,
                subject: o.id.clone(),
                message: format!("父引用悬空: {p}"),
                expected: Some("parent 指向存在的对象".into()),
                actual: Some(p.clone()),
            }),
            _ => None,
        })
        .collect()
}

/// 稳定 ID 唯一性：重复 id 直接 Error
pub fn duplicate_id(scene: &Scene, _report: Option<&OpReport>) -> Vec<Finding> {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for o in &scene.objects {
        *seen.entry(o.id.as_str()).or_insert(0) += 1;
    }
    seen.into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(id, n)| Finding {
            checker: "structure.duplicate_id".into(),
            severity: Severity::Error,
            subject: id.to_string(),
            message: format!("稳定 ID 重复 {n} 次"),
            expected: Some("id 全局唯一".into()),
            actual: Some(format!("{n} 个同名 id")),
        })
        .collect()
}

/// 悬空材质绑定：mesh 的 material_slots 必须指向存在的材质
pub fn dangling_material(scene: &Scene, _report: Option<&OpReport>) -> Vec<Finding> {
    let mats: std::collections::BTreeSet<_> = scene.materials.iter().map(|m| m.id.as_str()).collect();
    scene
        .objects
        .iter()
        .flat_map(|o| match &o.kind {
            ObjectKind::Mesh { material_slots, .. } => material_slots
                .iter()
                .filter(|m| !mats.contains(m.as_str()))
                .map(|m| Finding {
                    checker: "material.dangling_binding".into(),
                    severity: Severity::Error,
                    subject: o.id.clone(),
                    message: format!("材质绑定悬空: {m}"),
                    expected: Some("material_slots 指向存在的材质".into()),
                    actual: Some(m.clone()),
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect()
}

/// 悬空贴图引用：材质 textures 槽位必须指向存在的资产
pub fn dangling_texture(scene: &Scene, _report: Option<&OpReport>) -> Vec<Finding> {
    let ids: std::collections::BTreeSet<_> = scene.assets.iter().map(|a| a.id.as_str()).collect();
    let mut out = Vec::new();
    for m in &scene.materials {
        for a in [
            &m.textures.base_color,
            &m.textures.normal,
            &m.textures.roughness,
            &m.textures.metallic,
            &m.textures.ao,
        ]
        .into_iter()
        .flatten()
        {
            if !ids.contains(a.as_str()) {
                out.push(Finding {
                    checker: "material.dangling_texture".into(),
                    severity: Severity::Error,
                    subject: m.id.clone(),
                    message: format!("贴图引用悬空: {a}"),
                    expected: Some("textures 指向存在的资产".into()),
                    actual: Some(a.clone()),
                });
            }
        }
    }
    out
}

/// 父链成环（审计 #4）：load_scene 旁路可造环 → 渲染挂死；Error 级一票否决
pub fn parent_cycles(scene: &Scene, _report: Option<&OpReport>) -> Vec<Finding> {
    use std::collections::BTreeSet;
    let mut out = Vec::new();
    let mut reported: BTreeSet<String> = BTreeSet::new();
    for o in &scene.objects {
        let mut chain: Vec<&str> = vec![o.id.as_str()];
        let mut cur = o.parent.as_deref();
        while let Some(p) = cur {
            if chain.contains(&p) {
                if reported.insert(o.id.clone()) {
                    out.push(Finding {
                        checker: "structure.parent_cycle".into(),
                        severity: Severity::Error,
                        subject: o.id.clone(),
                        message: format!("父链成环（链上重访 {p}）"),
                        expected: Some("父链无环".into()),
                        actual: Some(format!("环: {} -> {p}", chain.join(" -> "))),
                    });
                }
                break;
            }
            chain.push(p);
            cur = scene
                .objects
                .iter()
                .find(|x| x.id == p)
                .and_then(|x| x.parent.as_deref());
        }
    }
    out
}

/// 材质/资产 id 唯一性（审计 #6）：撞车不得过闸
pub fn duplicate_material_asset_id(scene: &Scene, _report: Option<&OpReport>) -> Vec<Finding> {
    let mut out = Vec::new();
    let groups: [(&str, Vec<&str>); 2] = [
        ("material", scene.materials.iter().map(|m| m.id.as_str()).collect()),
        ("asset", scene.assets.iter().map(|a| a.id.as_str()).collect()),
    ];
    for (kind, ids) in groups {
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for id in ids {
            *seen.entry(id).or_insert(0) += 1;
        }
        for (id, n) in seen.into_iter().filter(|(_, n)| *n > 1) {
            out.push(Finding {
                checker: format!("structure.duplicate_{kind}_id"),
                severity: Severity::Error,
                subject: id.to_string(),
                message: format!("{kind} ID 重复 {n} 次"),
                expected: Some("ID 唯一".into()),
                actual: Some(format!("{n} 次")),
            });
        }
    }
    out
}

/// 组装默认验证器（产品门禁基线）
pub fn default_registry() -> VerifyRegistry {
    let mut reg = VerifyRegistry::new();
    reg.register(
        CheckDef {
            id: "op.noop_detection".into(),
            label: "静默 no-op 检测".into(),
            description: "非只读 op 声称成功但几何事实零变化必须报出".into(),
        },
        Box::new(noop_detection),
    );
    reg.register(
        CheckDef {
            id: "structure.dangling_parent".into(),
            label: "悬空父引用".into(),
            description: "parent 必须指向存在的对象".into(),
        },
        Box::new(dangling_parent),
    );
    reg.register(
        CheckDef {
            id: "structure.duplicate_id".into(),
            label: "稳定 ID 唯一性".into(),
            description: "id 重复即 Error".into(),
        },
        Box::new(duplicate_id),
    );
    reg.register(
        CheckDef {
            id: "structure.parent_cycle".into(),
            label: "父链成环".into(),
            description: "parent 链不得成环（渲染会展开父链，成环即挂死）".into(),
        },
        Box::new(parent_cycles),
    );
    reg.register(
        CheckDef {
            id: "structure.duplicate_material_asset_id".into(),
            label: "材质/资产 ID 唯一性".into(),
            description: "材质与资产 id 重复即 Error".into(),
        },
        Box::new(duplicate_material_asset_id),
    );
    reg.register(
        CheckDef {
            id: "material.dangling_binding".into(),
            label: "悬空材质绑定".into(),
            description: "material_slots 必须指向存在的材质".into(),
        },
        Box::new(dangling_material),
    );
    reg.register(
        CheckDef {
            id: "material.dangling_texture".into(),
            label: "悬空贴图引用".into(),
            description: "材质 textures 槽位必须指向存在的资产".into(),
        },
        Box::new(dangling_texture),
    );
    reg
}

#[cfg(test)]
mod tests {
    use super::*;
    use archcore_ir::{Object, Textures, Transform, Units, UpAxis};
    use archcore_ops::{GeoFacts, OpAnnotations, OpCategory, OpDef, OpRegistry, OpSpec, ParamSpec};
    use serde_json::Value;

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

    fn facts(scene: &Scene) -> GeoFacts {
        archcore_ops::geo_facts(scene)
    }

    fn make_op(id: &str, idempotent: bool, read_only: bool, noop: bool) -> OpRegistry {
        let mut reg = OpRegistry::new("0.1");
        reg.register(OpSpec {
            def: OpDef {
                id: id.into(),
                label: id.into(),
                description: "".into(),
                category: OpCategory::Scene,
                annotations: OpAnnotations {
                    destructive: false,
                    idempotent,
                    read_only,
                },
                params: vec![ParamSpec {
                    name: "x".into(),
                    type_name: "number".into(),
                    required: false,
                    description: "".into(),
                }],
            },
            handler: Box::new(move |scene: &mut Scene, _args: &Value| {
                if !noop {
                    scene.objects.push(Object {
                        id: format!("obj-{}", scene.objects.len() + 1),
                        name: "gen".into(),
                        kind: ObjectKind::Group,
                        parent: None,
                        transform: Transform::default(),
                        visible: true,
                        meta: BTreeMap::new(),
                    });
                }
                Ok((vec![], Value::Null))
            }),
        })
        .unwrap();
        reg
    }

    /// 宪法条款：非幂等 op 静默 no-op = Error，门禁不过
    #[test]
    fn noop_for_non_idempotent_is_error() {
        let reg = make_op("test.vacuum", false, false, true);
        let mut scene = empty_scene();
        let report = reg
            .execute(&mut scene, "test.vacuum", &serde_json::json!({}))
            .unwrap();
        let verify = default_registry();
        let err = verify.gate(&scene, Some(&report)).unwrap_err();
        assert!(err.iter().any(|f| f.checker == "op.noop_detection" && f.severity == Severity::Error));
    }

    /// 幂等 op 的 no-op 降为 Warning，保住幂等语义
    #[test]
    fn noop_for_idempotent_is_warning() {
        let reg = make_op("test.touch", true, false, true);
        let mut scene = empty_scene();
        let report = reg
            .execute(&mut scene, "test.touch", &serde_json::json!({}))
            .unwrap();
        let verify = default_registry();
        verify.gate(&scene, Some(&report)).unwrap(); // Warning 不拦门禁
        let findings = verify.run(&scene, Some(&report));
        assert!(findings.iter().any(|f| f.severity == Severity::Warning));
    }

    /// 只读 op 不参与 no-op 判定
    #[test]
    fn read_only_exempt_from_noop() {
        let reg = make_op("test.facts", true, true, true);
        let mut scene = empty_scene();
        let report = reg
            .execute(&mut scene, "test.facts", &serde_json::json!({}))
            .unwrap();
        let verify = default_registry();
        assert!(verify.run(&scene, Some(&report)).is_empty());
    }

    #[test]
    fn dangling_parent_is_error() {
        let mut scene = empty_scene();
        scene.objects.push(Object {
            id: "obj-1".into(),
            name: "孤".into(),
            kind: archcore_ir::ObjectKind::Group,
            parent: Some("obj-404".into()),
            transform: Transform::default(),
            visible: true,
            meta: BTreeMap::new(),
        });
        let verify = default_registry();
        assert!(verify.gate(&scene, None).is_err());
    }

    #[test]
    fn duplicate_id_is_error() {
        let mut scene = empty_scene();
        for _ in 0..2 {
            scene.objects.push(Object {
                id: "obj-1".into(),
                name: "重".into(),
                kind: archcore_ir::ObjectKind::Group,
                parent: None,
                transform: Transform::default(),
                visible: true,
                meta: BTreeMap::new(),
            });
        }
        let verify = default_registry();
        let err = verify.gate(&scene, None).unwrap_err();
        assert!(err.iter().any(|f| f.checker == "structure.duplicate_id"));
    }

    /// 干净场景（含一次真实变更）过闸
    #[test]
    fn clean_scene_passes_gate() {
        let reg = make_op("test.touch", false, false, false);
        let mut scene = empty_scene();
        let report = reg
            .execute(&mut scene, "test.touch", &serde_json::json!({}))
            .unwrap();
        assert_ne!(facts(&scene).hierarchy_hash, report.facts_before.hierarchy_hash);
        let verify = default_registry();
        verify.gate(&scene, Some(&report)).unwrap();
    }

    /// 悬空贴图引用是 Error（材质通道的结构三查补齐）
    #[test]
    fn dangling_texture_is_error() {
        let mut scene = empty_scene();
        scene.materials.push(archcore_ir::Material {
            id: "mat-0".into(),
            name: "M".into(),
            shading_model: archcore_ir::ShadingModel::PbrMetallicRoughness,
            base_color: [1.0; 4],
            metallic: 0.0,
            roughness: 0.8,
            opacity: 1.0,
            textures: Textures {
                base_color: Some("asset-404".into()),
                ..Default::default()
            },
        });
        let err = default_registry().gate(&scene, None).unwrap_err();
        assert!(err.iter().any(|f| f.checker == "material.dangling_texture"));
    }
}
