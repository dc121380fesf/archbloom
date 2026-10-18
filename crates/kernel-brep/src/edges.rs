//! 边查询：把实体的拓扑边暴露给调用方。
//!
//! # 为什么需要它
//!
//! 倒角接口是 `chamfer(id, edges: &[EdgeId], width, ...)`——`edges` 非空时只倒指定的边。
//! 但如果没人能查到「这个实体有哪些边、多长、两面夹角多少」，那个参数就是空的：
//! agent 只能传空数组（全倒），而布尔件的碎片面根本经不起全倒。
//!
//! 这个模块把边表变成**可读、可筛**的列表，并且边号与 [`crate::chamfer`] 内部用的编号
//! 严格一致——否则调用方选了 5 号边，实现却倒的是另一条。
//!
//! # 边号不稳定（重要）
//!
//! 边号是按 `(端点A, 端点B)` 的顶点号排序后的序号。**任何一次几何运算之后，
//! 顶点号都可能重排，边号随之失效**。所以：
//!
//! - 查边 → 倒角，中间不要插入别的运算；
//! - 需要跨操作引用某条边时，用它的几何特征（长度、中点、方向）重新查找，不要缓存号。

use archcore_kernel::{EdgeId, KernelError, Point3};
use serde::{Deserialize, Serialize};

use crate::chamfer::{edge_table, ChamferOptions};

/// 一条边的可读描述。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EdgeInfo {
    /// 边号，可直接传给 `chamfer` 的 `edges` 参数。
    pub id: EdgeId,
    /// 两个端点的坐标（米）。
    pub a: Point3,
    pub b: Point3,
    /// 边长（米）。
    pub length: f64,
    /// 相邻两个面的索引。
    pub faces: (usize, usize),
    /// 两面法线夹角（度）。90° 表示垂直，接近 180° 表示平滑过渡。
    pub angle_deg: f64,
    /// 是否达到默认倒角阈值（30°）。false 表示这条边两侧几乎是同一个曲面。
    pub sharp: bool,
}

impl EdgeInfo {
    /// 边的中点。按中点筛边通常比按端点稳定。
    pub fn midpoint(&self) -> Point3 {
        [
            (self.a[0] + self.b[0]) * 0.5,
            (self.a[1] + self.b[1]) * 0.5,
            (self.a[2] + self.b[2]) * 0.5,
        ]
    }

    /// 边的方向单位向量。零长度边返回 None。
    pub fn direction(&self) -> Option<[f64; 3]> {
        let d = [
            self.b[0] - self.a[0],
            self.b[1] - self.a[1],
            self.b[2] - self.a[2],
        ];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if l <= 1e-15 {
            return None;
        }
        Some([d[0] / l, d[1] / l, d[2] / l])
    }
}

/// 列出实体所有边，按边号升序（与内部编号一致）。
pub fn list_edges(solid: &crate::solid::Solid) -> Result<Vec<EdgeInfo>, KernelError> {
    let table = edge_table(solid)?;
    let threshold = ChamferOptions::default().angle_threshold_deg;
    let mut out = Vec::with_capacity(table.len());
    for (i, e) in table.iter().enumerate() {
        let pa = solid.verts[e.a as usize];
        let pb = solid.verts[e.b as usize];
        let dx = pb[0] - pa[0];
        let dy = pb[1] - pa[1];
        let dz = pb[2] - pa[2];
        out.push(EdgeInfo {
            id: EdgeId(i as u64),
            a: pa,
            b: pb,
            length: (dx * dx + dy * dy + dz * dz).sqrt(),
            faces: (e.f1, e.f2),
            angle_deg: e.angle_deg,
            sharp: e.angle_deg > threshold,
        });
    }
    Ok(out)
}
