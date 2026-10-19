//! 共面面片合并：**只保留半边路径**。
//!
//! # 历史
//!
//! 这里原来有两套实现：
//!
//! - `merge_coplanar`（几何拼接）：把两个面的顶点环按坐标拼起来、数重复顶点、
//!   再按面积猜哪段是洞。三条硬伤——环的边界取决于拼接顺序、靠坐标判重合、
//!   遇自交就拒（于是洞永远是留缝的 C 形）。实测 3148 → 534 面，
//!   洞边界 955 点里只有 17 个落在孔圆上。
//! - `merge_he::merge_by_half_edge`（半边拼接）：沿共享边只改两条 `next` 指针，
//!   全程不做几何计算。实测 3148 → **70 面**（理论极限），洞是 64 点真圆，
//!   FreeCAD 回读体积 1607.300918301 与理论一字不差。
//!
//! 之前旧实现作为「新路径失败时」的 fallback 保留着。**那是掩盖问题**：
//! 一边失败一边悄悄退回旧路，问题永远不会被发现。现在删掉——
//! 新路径失败就如实报错。

use archcore_kernel::KernelError;

use crate::solid::Solid;

/// 合并统计。
pub struct MergeStats {
    pub faces_before: usize,
    pub faces_after: usize,
    /// 合并后带洞的面数。
    pub holed_faces: usize,
    /// 面数减少比例（0.0 ~ 1.0）。
    pub reduction: f64,
}

/// 把同一曲面上相邻的面片合并成整面。返回新实体与统计。
///
/// 走后端：[`crate::merge_he::merge_by_half_edge`]。
/// 失败时**不降级**——直接返回错误，让调用方看见问题。
pub fn merge_coplanar(solid: &Solid) -> Result<(Solid, MergeStats), KernelError> {
    let n = solid.faces.len();
    if n == 0 {
        return Ok((solid.clone(), MergeStats {
            faces_before: 0,
            faces_after: 0,
            holed_faces: 0,
            reduction: 0.0,
        }));
    }
    let (out, _) = crate::merge_he::merge_by_half_edge(solid, 1.0)?;
    let after = out.faces.len();
    let holed = out.faces.iter().filter(|f| !f.holes.is_empty()).count();
    Ok((
        out,
        MergeStats {
            faces_before: n,
            faces_after: after,
            holed_faces: holed,
            reduction: (n - after) as f64 / n as f64,
        },
    ))
}
