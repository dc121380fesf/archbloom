//! 边：显式边表 + 可读的边查询。
//!
//! # 为什么需要它
//!
//! 倒角接口是 `chamfer(id, edges: &[EdgeId], width, ...)`——`edges` 非空时只倒指定的边。
//! 但如果没人能查到「这个实体有哪些边、多长、两面夹角多少」，那个参数就是空的：
//! agent 只能传空数组（全倒），而布尔件的碎片面根本经不起全倒。
//!
//! 这个模块把边表变成**可读、可筛**的列表，并且边号与
//! [`crate::blend::chamfer`] / [`crate::blend::fillet`] 内部用的编号**严格一致**——
//! 否则调用方选了 5 号边，实现却倒的是另一条。
//!
//! # 边号不稳定（重要）
//!
//! 边号是按 `(端点A, 端点B)` 的顶点号排序后的序号。**任何一次几何运算之后，
//! 顶点号都可能重排，边号随之失效**。所以：
//!
//! - 查边 → 倒角/圆角，中间不要插入别的运算；
//! - 需要跨操作引用某条边时，用它的几何特征（长度、中点、方向）重新查找，不要缓存号。
//!
//! # 归位说明
//!
//! `edge_table` / `EdgeRec` 原来住在 `chamfer.rs` 里。**边的定义不该住在
//! 某一个特征模块中**——圆角、布尔、导出都要用它。现在归到这里，
//! `chamfer` 和 `fillet` 都从这里取。

use std::collections::HashMap;

use archcore_kernel::{EdgeId, KernelError, Point3, Vector3};
use serde::{Deserialize, Serialize};

use crate::blend::ChamferOptions;
use crate::geom::{dot, sub, Surface};
use crate::solid::Solid;

/// 一条无向边的完整信息。
pub struct EdgeRec {
    pub a: u32,
    pub b: u32,
    /// 两个相邻面。水密实体必有恰好两个。
    pub f1: usize,
    pub f2: usize,
    /// 是否达到倒角/圆角的「真边」条件（两面夹角 > 阈值）。
    pub sharp: bool,
    /// 两面法线夹角（度）。
    pub angle_deg: f64,
}

type Key = (u32, u32);

fn key(a: u32, b: u32) -> Key {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// 面法线：优先解析曲面在这个面质心处的法线，退化时用顶点叉积。
fn face_normal(solid: &Solid, fi: usize) -> Vector3 {
    let f = &solid.faces[fi];
    if f.verts.is_empty() {
        return [0.0, 0.0, 1.0];
    }
    let n = f.verts.len();
    let mut c = [0.0; 3];
    for &v in &f.verts {
        let p = solid.verts[v as usize];
        for (k, ck) in c.iter_mut().enumerate() {
            *ck += p[k];
        }
    }
    let m = n as f64;
    let centroid = [c[0] / m, c[1] / m, c[2] / m];
    if let Some(nn) = f.surface.normal_at(centroid) {
        return nn;
    }
    // 退化：顶点叉积
    let mut acc = [0.0; 3];
    for i in 0..n {
        let a = solid.verts[f.verts[i] as usize];
        let b = solid.verts[f.verts[(i + 1) % n] as usize];
        acc[0] += (a[1] - b[1]) * (a[2] + b[2]);
        acc[1] += (a[2] - b[2]) * (a[0] + b[0]);
        acc[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let l = dot(acc, acc).sqrt();
    if l < 1e-15 {
        [0.0, 0.0, 1.0]
    } else {
        [acc[0] / l, acc[1] / l, acc[2] / l]
    }
}

/// 把隐式边落成显式表。
///
/// 顺带做水密检查：每条边必须恰好两个面。两个后端（网格/B-rep）都靠这一条
/// 判断「这个实体还能不能做拓扑运算」。
pub fn edge_table(solid: &Solid) -> Result<Vec<EdgeRec>, KernelError> {
    let mut map: HashMap<Key, Vec<usize>> = HashMap::new();
    for (fi, f) in solid.faces.iter().enumerate() {
        let n = f.verts.len();
        for i in 0..n {
            let a = f.verts[i];
            let b = f.verts[(i + 1) % n];
            if a == b {
                return Err(KernelError::TopologyBroken {
                    at: String::new(),
                    reason: format!("面 {fi} 含零长度边（顶点 {a} 重复）"),
                    suggestion: "检查实体构造".to_string(),
                });
            }
            map.entry(key(a, b)).or_default().push(fi);
        }
    }

    let mut out = Vec::with_capacity(map.len());
    // 锐边阈值从 [`crate::blend::ChamferOptions`] 取，不在这里再写死一个数。
    let threshold = crate::blend::ChamferOptions::default().angle_threshold_deg;
    for ((a, b), faces) in map {
        if faces.len() != 2 {
            return Err(KernelError::TopologyBroken {
                at: String::new(),
                reason: format!(
                    "边 ({a},{b}) 被 {} 个面共享（水密实体应为 2）",
                    faces.len()
                ),
                suggestion: "先修复实体水密性（is_watertight 应返回 true）".to_string(),
            });
        }
        let (f1, f2) = (faces[0], faces[1]);
        let n1 = face_normal(solid, f1);
        let n2 = face_normal(solid, f2);
        let ang = dot(n1, n2).clamp(-1.0, 1.0).acos().to_degrees();
        out.push(EdgeRec {
            a,
            b,
            f1,
            f2,
            sharp: ang > threshold,
            angle_deg: ang,
        });
    }
    // 确定性排序：按顶点号，保证同一实体两次得到的边号一致
    out.sort_by_key(|e| (e.a, e.b));
    Ok(out)
}

/// 这条边是否「真边」——两侧不属于同一个曲面。
///
/// 圆柱侧面离散成 N 个四边形时，相邻两片共享 `Cylinder` 标注，
/// 它们之间的接缝**不是真实几何边**（夹角只有 22.5° 这种），
/// 不应参与倒角/圆角。判断依据是曲面标注是否相同，不是角度。
pub fn is_real_edge(solid: &Solid, e: &EdgeRec) -> bool {
    !same_surface(&solid.faces[e.f1].surface, &solid.faces[e.f2].surface)
}

/// 两个曲面标注是否相同。
fn same_surface(a: &Surface, b: &Surface) -> bool {
    match (a, b) {
        (Surface::Plane { normal: n1, .. }, Surface::Plane { normal: n2, .. }) => {
            dot(*n1, *n2).abs() > 1.0 - 1e-9
        }
        (
            Surface::Cylinder {
                origin: o1,
                axis: a1,
                radius: r1,
            },
            Surface::Cylinder {
                origin: o2,
                axis: a2,
                radius: r2,
            },
        ) => {
            (r1 - r2).abs() < 1e-12
                && dot(*a1, *a2).abs() > 1.0 - 1e-9
                && {
                    let d = sub(*o2, *o1);
                    let along = dot(d, *a1);
                    let perp = sub(d, [a1[0] * along, a1[1] * along, a1[2] * along]);
                    dot(perp, perp).sqrt() < 1e-12
                }
        }
        _ => false,
    }
}

/// 一条边的可读描述。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EdgeInfo {
    /// 边号，可直接传给 `chamfer` / `fillet` 的 `edges` 参数。
    pub id: EdgeId,
    /// 两个端点的坐标（米）。
    pub a: Point3,
    pub b: Point3,
    pub length: f64,
    /// 两个相邻面的序号。
    pub faces: (usize, usize),
    pub angle_deg: f64,
    /// 两面夹角是否超过阈值（倒角/圆角的默认筛选条件）。
    pub sharp: bool,
    /// 是否是「真边」——两侧不属于同一曲面。
    ///
    /// 圆柱侧面的离散接缝为 `false`：它们不是真实几何边，
    /// 即使夹角看着不小也不该倒角。
    pub real: bool,
}

impl EdgeInfo {
    pub fn midpoint(&self) -> Point3 {
        [
            (self.a[0] + self.b[0]) * 0.5,
            (self.a[1] + self.b[1]) * 0.5,
            (self.a[2] + self.b[2]) * 0.5,
        ]
    }

    pub fn direction(&self) -> Option<[f64; 3]> {
        let d = sub(self.b, self.a);
        let l = dot(d, d).sqrt();
        if l < 1e-15 {
            None
        } else {
            Some([d[0] / l, d[1] / l, d[2] / l])
        }
    }
}

/// 列出实体所有边：边号、两端点、长度、相邻面、两面夹角、是否真边。
pub fn list_edges(solid: &Solid) -> Result<Vec<EdgeInfo>, KernelError> {
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
            real: is_real_edge(solid, e),
        });
    }
    Ok(out)
}
