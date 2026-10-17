//! 网格后端：[`GeometryKernel`] 的三角网格实现。
//!
//! # 能力边界（如实申报，不"尽力而为"）
//!
//! - 布尔：**有**，借 manifold（流形布尔，非近似）
//! - 曲面布尔：**有**（manifold 只看三角面，不区分曲直）
//! - 倒角 / 圆角：**没有**（网格上做真倒角需要曲面，那是 B-rep 的活）
//! - STEP：**没有**（网格出不了加工格式；这条路就是为什么要有 B-rep 后端）
//! - 精确算术：**没有**（浮点 + 流形保证，不是有理数精确）
//!
//! # 关于取消
//!
//! manifold 的布尔是一次黑盒调用，中途插不进取消检查。所以这里的语义是
//! **开始前可取消**——开始后一旦进入 manifold 就得等它跑完。契约要求的
//! "取消不留半成品"仍然成立：取消发生在进入之前，此时尚未分配新 ID。

use std::collections::HashMap;
use std::sync::Arc;

use archcore_kernel::{
    BoolOp, CancelToken, Capabilities, GeometryKernel, KernelError, Point3, Progress, SolidFacts,
    SolidId, Tess, Vector3,
};
use archcore_mesh::Mesh;

use crate::measure;

/// 实体存储：ID → 三角网格。ID 单调递增、永不复用。
pub struct MeshKernel {
    solids: HashMap<SolidId, (Vec<Point3>, Vec<u32>)>,
    next: u64,
}

impl MeshKernel {
    pub fn new() -> Self {
        Self {
            solids: HashMap::new(),
            next: 1,
        }
    }

    /// 放进一个实体，返回其 ID。用于把场景几何喂进内核。
    pub fn insert(&mut self, positions: Vec<Point3>, indices: Vec<u32>) -> SolidId {
        let id = SolidId(self.next);
        self.next += 1;
        self.solids.insert(id, (positions, indices));
        id
    }

    pub fn geometry(&self, id: SolidId) -> Option<&(Vec<Point3>, Vec<u32>)> {
        self.solids.get(&id)
    }

    pub fn len(&self) -> usize {
        self.solids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.solids.is_empty()
    }
}

impl Default for MeshKernel {
    fn default() -> Self {
        Self::new()
    }
}

fn to_mesh(positions: &[Point3], indices: &[u32]) -> Result<Mesh, KernelError> {
    Mesh::from_indexed(positions.to_vec(), indices).map_err(|e| KernelError::TopologyBroken {
        at: "输入网格".into(),
        reason: format!("{e}"),
        suggestion: "先检查网格是否退化为非流形（同一对顶点被多条边共享）".into(),
    })
}

impl GeometryKernel for MeshKernel {
    fn capabilities(&self) -> Capabilities {
        let mut c = Capabilities::none("mesh");
        c.solid_bool = true;
        c.curved_bool = true;
        c.exact = false;
        c.max_precision = 1e-6;
        c
    }

    fn create_from_mesh(
        &mut self,
        positions: Vec<Point3>,
        indices: Vec<u32>,
    ) -> Result<SolidId, KernelError> {
        // 网格后端：几何已是最终表示，存下即可。
        // 但空网格拒绝——让调用方尽早知道它给错了东西。
        if indices.len() < 3 {
            return Err(KernelError::Degenerate {
                at: [0.0, 0.0, 0.0],
                reason: "三角网格没有面片".into(),
                suggestion: "检查 positions/indices 是否为空或索引越界".into(),
            });
        }
        Ok(self.insert(positions, indices))
    }

    fn facts(&self, id: SolidId) -> Result<SolidFacts, KernelError> {
        let (pos, idx) = self
            .solids
            .get(&id)
            .ok_or(KernelError::UnknownSolid { id })?;
        let m = measure::measure(pos, idx);
        Ok(SolidFacts {
            volume: m.volume,
            area: m.area,
            face_count: m.triangle_count,
            edge_count: edge_count(idx),
            vertex_count: m.vertex_count,
            watertight: m.watertight,
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
        let (pos, idx) = self
            .solids
            .get(&id)
            .ok_or(KernelError::UnknownSolid { id })?;
        // 网格后端没有细分自由度：几何已是三角面，容差只影响将来的 B-rep 后端。
        // 这里不做伪细分（静默忽略容差比明说不支持更坏，但我们确实"已经是最终表示"）。
        let mut normals: Vec<Vector3> = vec![[0.0, 0.0, 0.0]; pos.len()];
        for tri in idx.as_chunks::<3>().0 {
            let (Some(a), Some(b), Some(c)) = (
                pos.get(tri[0] as usize),
                pos.get(tri[1] as usize),
                pos.get(tri[2] as usize),
            ) else {
                continue;
            };
            let n = tri_normal(*a, *b, *c);
            for v in [tri[0], tri[1], tri[2]] {
                if let Some(slot) = normals.get_mut(v as usize) {
                    *slot = n;
                }
            }
        }
        Ok(Tess {
            positions: pos.clone(),
            normals,
            indices: idx.clone(),
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
        // 取消只在进入 manifold 之前有效（见模块文档）
        if cancel.is_cancelled() {
            return Err(KernelError::Cancelled);
        }
        let (pa, ia) = self
            .solids
            .get(&a)
            .ok_or(KernelError::UnknownSolid { id: a })?
            .clone();
        let (pb, ib) = self
            .solids
            .get(&b)
            .ok_or(KernelError::UnknownSolid { id: b })?
            .clone();

        progress(Progress::Start { total_hint: Some(1) });
        progress(Progress::Stage {
            name: "streaming_manifold",
            done: 0,
            total: 1,
        });

        let ma = to_mesh(&pa, &ia)?;
        let mb = to_mesh(&pb, &ib)?;
        let mesh_op = match op {
            BoolOp::Union => archcore_mesh::BoolOp::Union,
            BoolOp::Difference => archcore_mesh::BoolOp::Difference,
            BoolOp::Intersection => archcore_mesh::BoolOp::Intersection,
        };
        let out = ma.boolean(&mb, mesh_op).map_err(|e| KernelError::Degenerate {
            at: centroid(&pa),
            reason: format!("布尔结果无效：{e}"),
            suggestion: "检查两实体是否真正相交；完全分离时差集等于原实体、交集为空".into(),
        })?;
        let (pos, idx) = out.to_indexed();
        if idx.is_empty() {
            return Err(KernelError::Degenerate {
                at: centroid(&pa),
                reason: "布尔结果为空（无三角形）".into(),
                suggestion: "交集场景下两实体可能不相交；改用 difference 或调整位置".into(),
            });
        }

        progress(Progress::Stage {
            name: "done",
            done: 1,
            total: 1,
        });
        progress(Progress::Done);
        Ok(self.insert(pos, idx))
    }

    fn drop_solid(&mut self, id: SolidId) {
        self.solids.remove(&id);
    }
}

fn centroid(pos: &[Point3]) -> Point3 {
    if pos.is_empty() {
        return [0.0, 0.0, 0.0];
    }
    let n = pos.len() as f64;
    let mut s = [0.0, 0.0, 0.0];
    for p in pos {
        s[0] += p[0];
        s[1] += p[1];
        s[2] += p[2];
    }
    [s[0] / n, s[1] / n, s[2] / n]
}

fn tri_normal(a: Point3, b: Point3, c: Point3) -> Vector3 {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len <= 1e-12 {
        [0.0, 0.0, 0.0]
    } else {
        [n[0] / len, n[1] / len, n[2] / len]
    }
}

fn edge_count(indices: &[u32]) -> usize {
    use std::collections::HashSet;
    let mut set: HashSet<(u32, u32)> = HashSet::new();
    for tri in indices.as_chunks::<3>().0 {
        for (x, y) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            set.insert(if x < y { (x, y) } else { (y, x) });
        }
    }
    set.len()
}

/// 便于上层把「内核实例」塞进 `Arc<Mutex<dyn GeometryKernel>>`。
pub fn shared() -> Arc<std::sync::Mutex<dyn GeometryKernel>> {
    Arc::new(std::sync::Mutex::new(MeshKernel::new()))
}
