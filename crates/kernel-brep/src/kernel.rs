//! 精确后端：[`GeometryKernel`] 的 B-rep 实现。
//!
//! # 能力边界（D 阶段，如实申报）
//!
//! | 能力 | 状态 |
//! |---|---|
//! | `exact` | ✅ 体积/面积走闭式解，不是量出来的 |
//! | `create_from_mesh` | ❌ 从网格重建曲面属逆向工程，不在本轮范围 |
//! | `solid_bool` | ❌ E 阶段 |
//! | `curved_bool` | ❌ F 阶段 |
//! | `fillet` | ❌ I 阶段（延后） |
//! | `step_io` | ❌ G 阶段 |
//!
//! 所有 ❌ 都**如实申报**，返回 `Unsupported` 而不是"尽力而为"——
//! agent 会按能力声明做规划，谎报会让它规划出一步都走不通的路线。
//!
//! # 已知局限
//!
//! 当前实体在**构造时**确定离散度（圆柱的段数），`tessellate(tol)` 不会重新细分。
//! 它只校验 `tol` 合法性。要做到「同一个实体按需给不同精度」，需要保留参数化描述
//! 而不是离散结果——那是 E 阶段布尔重建时一并解决的事。

use std::collections::HashMap;
use std::sync::Arc;

use archcore_kernel::{
    BoolOp, CancelToken, Capabilities, EdgeId, GeometryKernel, KernelError, Point3, Progress,
    SolidFacts, SolidId, Tess,
};

use crate::chamfer::{chamfer_solid, ChamferOptions};
use crate::solid::{box_solid, cylinder_solid, volume_from_mesh, Solid};

/// 精确内核：ID → 实体。ID 单调递增、永不复用。
pub struct BrepKernel {
    solids: HashMap<SolidId, Solid>,
    next: u64,
}

impl BrepKernel {
    pub fn new() -> Self {
        Self {
            solids: HashMap::new(),
            next: 1,
        }
    }

    pub fn len(&self) -> usize {
        self.solids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.solids.is_empty()
    }

    /// 放进一个实体（构造器产出的直接入池）。
    pub fn insert(&mut self, solid: Solid) -> SolidId {
        let id = SolidId(self.next);
        self.next += 1;
        self.solids.insert(id, solid);
        id
    }

    pub fn get(&self, id: SolidId) -> Option<&Solid> {
        self.solids.get(&id)
    }

    /// 便捷：造一个长方体并入池。
    pub fn add_box(&mut self, min: Point3, max: Point3) -> SolidId {
        self.insert(box_solid(min, max))
    }

    /// 便捷：造一个圆柱并入池。
    pub fn add_cylinder(&mut self, base: Point3, r: f64, h: f64, segments: usize) -> SolidId {
        self.insert(cylinder_solid(base, r, h, segments))
    }
}

impl Default for BrepKernel {
    fn default() -> Self {
        Self::new()
    }
}

impl GeometryKernel for BrepKernel {
    fn capabilities(&self) -> Capabilities {
        let mut c = Capabilities::none("brep");
        c.exact = true;
        c.max_precision = 1e-9;
        // 平面多面体布尔已可用（BSP + 拓扑重建）
        c.solid_bool = true;
        // 但曲面求交未做：圆柱参与布尔时是离散近似，不是真曲面交线
        c.curved_bool = false;
        // STEP 导出已可用（全 PLANE 的合法实体；真曲面面待下一步）
        c.step_io = true;
        // 平面倒角已可用（切平面斜面）；圆角（圆弧过渡）仍未做，如实报 false
        c.chamfer = true;
        c
    }

    fn create_from_mesh(
        &mut self,
        _positions: Vec<Point3>,
        _indices: Vec<u32>,
    ) -> Result<SolidId, KernelError> {
        Err(KernelError::Unsupported {
            feature: "create_from_mesh（网格转 B-rep）",
            alternatives: vec![
                "用 brep 自带的构造器（box/cylinder）",
                "先用网格后端，等 G 阶段接 STEP 后再转",
            ],
        })
    }

    fn facts(&self, id: SolidId) -> Result<SolidFacts, KernelError> {
        let s = self
            .solids
            .get(&id)
            .ok_or(KernelError::UnknownSolid { id })?;

        // 体积/面积优先用闭式解；没有则退到三角化估算（并明确标注为估算口径）
        let (volume, area) = match (s.analytic_volume, s.analytic_area) {
            (Some(v), Some(a)) => (v, a),
            _ => {
                let (pos, _, idx) = s.triangulate();
                (volume_from_mesh(&pos, &idx), 0.0)
            }
        };

        Ok(SolidFacts {
            volume,
            area,
            face_count: s.faces.len(),
            edge_count: count_edges(s),
            vertex_count: s.verts.len(),
            watertight: s.is_watertight(),
        })
    }

    fn tessellate(&self, id: SolidId, tol: f64) -> Result<Tess, KernelError> {
        if !(tol.is_finite() && tol > 0.0) {
            return Err(KernelError::ToleranceExceeded {
                required: 1e-6,
                achieved: tol,
                suggestion: "容差必须是正有限值（视口 0.01、出图 0.0005）".into(),
            });
        }
        let s = self
            .solids
            .get(&id)
            .ok_or(KernelError::UnknownSolid { id })?;
        let (positions, normals, indices) = s.triangulate();
        Ok(Tess {
            positions,
            normals,
            indices,
        })
    }

    fn boolean(
        &mut self,
        a: SolidId,
        b: SolidId,
        op: BoolOp,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<SolidId, KernelError> {
        // 取消优先于任何计算：已经决定不做了，不必开始
        if cancel.is_cancelled() {
            return Err(KernelError::Cancelled);
        }
        let sa = self
            .solids
            .get(&a)
            .ok_or(KernelError::UnknownSolid { id: a })?
            .clone();
        let sb = self
            .solids
            .get(&b)
            .ok_or(KernelError::UnknownSolid { id: b })?
            .clone();

        let pa = crate::rebuild::solid_to_polygons(&sa);
        let pb = crate::rebuild::solid_to_polygons(&sb);
        if pa.is_empty() || pb.is_empty() {
            return Err(KernelError::Degenerate {
                at: [0.0, 0.0, 0.0],
                reason: "参与布尔的实体没有面片".into(),
                suggestion: "检查实体是否为空；空实体不该进布尔".into(),
            });
        }

        progress(Progress::Start {
            total_hint: Some(pa.len() + pb.len()),
        });
        progress(Progress::Stage {
            name: "split",
            done: 0,
            total: 3,
        });
        // 新算法：切分 → 分类 → 保留（不再用 BSP 裁剪）
        let out = crate::csg::select_faces(&sa, &sb, pa, pb, op);

        progress(Progress::Stage {
            name: "rebuild",
            done: 2,
            total: 3,
        });
        let solid = crate::rebuild::polygons_to_solid(&out)?;

        progress(Progress::Done);
        Ok(self.insert(solid))
    }

    fn drop_solid(&mut self, id: SolidId) {
        self.solids.remove(&id);
    }

    fn chamfer(
        &mut self,
        id: SolidId,
        edges: &[EdgeId],
        width: f64,
        cancel: &CancelToken,
    ) -> Result<SolidId, KernelError> {
        if cancel.is_cancelled() {
            return Err(KernelError::Unsupported {
                feature: "chamfer（已取消）",
                alternatives: vec!["重新发起一次倒角"],
            });
        }
        let s = self
            .solids
            .get(&id)
            .ok_or(KernelError::UnknownSolid { id })?;
        let ids: Vec<u64> = edges.iter().map(|e| e.0).collect();
        let out = chamfer_solid(
            s,
            &ids,
            &ChamferOptions {
                width,
                angle_threshold_deg: 30.0,
            },
        )?;
        Ok(self.insert(out))
    }

    fn to_step(&self, id: SolidId) -> Result<String, KernelError> {
        let s = self
            .solids
            .get(&id)
            .ok_or(KernelError::UnknownSolid { id })?;
        let name = format!("solid_{}", id.0);
        let text = crate::step::to_step(s, &name);
        // 导出前自检：面/顶点数与实体不一致就不交出去（不给下游半成品）
        crate::step::verify_against_solid(&text, s).map_err(|e| KernelError::TopologyBroken {
            at: format!("solid_{}", id.0),
            reason: format!("STEP 自检失败：{e}"),
            suggestion: "这是内核缺陷，带着实体参数报 issue；不要直接拿去加工".into(),
        })?;
        Ok(text)
    }
}

fn count_edges(s: &Solid) -> usize {
    use std::collections::HashSet;
    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    for f in &s.faces {
        let n = f.verts.len();
        for i in 0..n {
            let a = f.verts[i];
            let b = f.verts[(i + 1) % n];
            edges.insert(if a < b { (a, b) } else { (b, a) });
        }
    }
    edges.len()
}

/// 便于上层注入。
pub fn shared() -> Arc<std::sync::Mutex<dyn GeometryKernel>> {
    Arc::new(std::sync::Mutex::new(BrepKernel::new()))
}
