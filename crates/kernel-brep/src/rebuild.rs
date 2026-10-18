//! 面片集合 → 实体拓扑。
//!
//! BSP 的输出是一堆**互不相连的多边形**（共享的顶点各存一份，边也不配对）。
//! 要变成可用的 B-rep，必须把顶点焊起来、重新构成面，再检查水密。
//!
//! # 容差
//!
//! 顶点合并用 [`WELD_TOL`]（0.1 微米）。比 [`bsp::EPS`](crate::bsp::EPS) 宽一档，
//! 因为交点是「平面求交 + 插值」算出来的，误差比原始顶点大。
//!
//! 太松会把真的不同顶点焊在一起（几何被悄悄改形），太紧会让本该共享的顶点分开
//! （壳体不闭合）。0.1μm 对建筑（米级）和机械（毫米级）都在安全区。

use archcore_kernel::{KernelError, Point3};

use crate::bsp::Polygon;
use crate::geom::{dot, len, sub};
use crate::solid::{Face, Solid};

/// 顶点合并容差（米）。
///
/// # 为什么是 1e-6（1 微米）而不是更小
///
/// 交点是「平面求交 + 插值」算出来的，坐标误差在 1e-7～1e-6 量级。
/// 容差取 1e-7 时，本该是同一个点的两个顶点焊不到一起——
/// 表象是壳体不闭合，实际只是两个顶点差了几十纳米。
///
/// 128 段圆柱开孔就是死在这里：25996 条边全部配对，但顶点分成了 12936 份。
///
/// 1 微米对建筑（米级）和机械（公分级）都在安全区，不会误合并。
pub const WELD_TOL: f64 = 1e-7;

/// 共面消隐：丢掉「位置重合、法线相反」的面片对。
///
/// # 为什么必须有这一步
///
/// 两个盒子恰好相邻（共享一个面）做并集时，BSP 会把那个共享面**两侧各留一份**：
/// 一份属于 A、一份属于 B。它们位置重合、朝向相反，几何上互相抵消。
/// 不处理的话，重建出的壳体边数对不上，直接报不水密。
///
/// 这正是「相邻两盒拼成一体」这种最常见需求会撞上的坑。
fn drop_coincident(polys: &[Polygon]) -> Vec<Polygon> {
    let mut keep = vec![true; polys.len()];
    for i in 0..polys.len() {
        if !keep[i] {
            continue;
        }
        for j in (i + 1)..polys.len() {
            if !keep[j] {
                continue;
            }
            match overlap_kind(&polys[i], &polys[j]) {
                // 反向重合：两块面背对背抵消，两个都删
                Overlap::Opposite => {
                    keep[i] = false;
                    keep[j] = false;
                    break;
                }
                // 同向重合：冗余，留一个
                Overlap::Same => {
                    keep[j] = false;
                }
                Overlap::None => {}
            }
        }
    }
    polys
        .iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| p.clone())
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Overlap {
    None,
    Opposite,
    Same,
}

/// 两个面片是否「贴在同一个位置」。
///
/// # 阈值为什么要这么紧
///
/// 早期版本用「质心距离 ≤ 100×WELD_TOL」+「面积差 ≤ 1e-7 绝对值」。
/// 对小块面片（开孔时每片面积只有 1e-8 量级）这两个阈值都太宽：
/// 两个相邻但完全不同的孔壁小片，质心隔了半个片宽都可能落在 1e-5 内，
/// 面积差也小于 1e-7——直接被当成重合删掉，壳体就破了。
///
/// 现在：质心必须几乎完全重合（≤ WELD_TOL），面积用**相对**误差（1e-9）。
/// 相邻两盒子的共享面质心完全相同，照样能识别；小碎片不会再被误杀。
fn overlap_kind(a: &Polygon, b: &Polygon) -> Overlap {
    let d = dot(a.plane.normal, b.plane.normal);
    let kind = if d < -0.999 {
        Overlap::Opposite
    } else if d > 0.999 {
        Overlap::Same
    } else {
        return Overlap::None;
    };
    // 同一平面：a 的顶点到 b 所在平面的距离该为 0
    let Some(v0) = a.verts.first() else {
        return Overlap::None;
    };
    if b.plane.distance(*v0).abs() > WELD_TOL {
        return Overlap::None;
    }
    // 质心必须几乎完全重合
    if len(sub(a.centroid(), b.centroid())) > WELD_TOL {
        return Overlap::None;
    }
    // 面积用相对误差：小块面片不能用绝对值卡
    let (aa, ab) = (a.area(), b.area());
    let scale = aa.max(ab);
    if scale <= 0.0 || (aa - ab).abs() > scale * 1e-9 {
        return Overlap::None;
    }
    kind
}

/// 把 BSP 输出的面片集合重建成实体。
///
/// 失败条件：结果为空、有面片不足三个顶点、重建后不水密。全部显式报错——
/// 布尔算出个破壳却返回成功，是最坏的情况（下游会拿它出图、出 STEP）。
pub fn polygons_to_solid(polys: &[Polygon]) -> Result<Solid, KernelError> {
    let polys = drop_coincident(polys);
    let mut verts: Vec<Point3> = Vec::new();
    let mut faces: Vec<Face> = Vec::new();

    for p in &polys {
        if p.verts.len() < 3 {
            continue;
        }
        let mut ring: Vec<u32> = Vec::with_capacity(p.verts.len());
        for v in &p.verts {
            ring.push(find_or_push(&mut verts, *v));
        }
        // 焊接后可能出现相邻重复（两个交点落到同一位置）
        ring.dedup();
        if ring.len() >= 3 && ring.first() == ring.last() {
            ring.pop();
        }
        if ring.len() < 3 {
            continue;
        }
        faces.push(Face {
            // **用多边形自带的曲面标注**，不再无条件写 Plane。
            // 这一行以前是 `Surface::Plane { origin: p.verts[0], ... }`，
            // 后果是布尔一跑，圆柱面的身份就全丢了（开孔板 169 面全成 plane）。
            surface: p.surface,
            verts: ring,
            holes: Vec::new(),
        });
    }

    if faces.is_empty() {
        return Err(KernelError::Degenerate {
            at: [0.0, 0.0, 0.0],
            reason: "布尔结果没有任何面片".into(),
            suggestion: "两个实体可能完全不相交（交集为空），或一方完全吞掉另一方（差集为空）"
                .into(),
        });
    }

    let solid = Solid {
        verts,
        faces,
        // 布尔结果没有闭式解：体积由三角化估算（散度定理，误差随离散度收敛）
        analytic_volume: None,
        analytic_area: None,
    };

    if !solid.is_watertight() {
        return Err(KernelError::TopologyBroken {
            at: "布尔结果".into(),
            reason: format!(
                "重建后不水密（{} 顶点 / {} 面；欧拉数 {}）",
                solid.verts.len(),
                solid.faces.len(),
                solid.euler_characteristic()
            ),
            suggestion: "多半是共面重叠或退化交点；把两个实体错开 1e-4 再试，或检查是否有面完全重合"
                .into(),
        });
    }

    Ok(solid)
}

/// 找既有顶点（容差内）或追加新的，返回索引。
///
/// 线性搜索：布尔结果的顶点数在几百到几千量级，O(n²) 可接受。
/// 真到万级再上空间哈希（那时候瓶颈也不在这儿）。
fn find_or_push(verts: &mut Vec<Point3>, p: Point3) -> u32 {
    for (i, v) in verts.iter().enumerate() {
        if len(sub(*v, p)) <= WELD_TOL {
            return i as u32;
        }
    }
    verts.push(p);
    (verts.len() - 1) as u32
}

/// 把实体转成 BSP 能吃的平面多边形集合。
///
/// 曲面（圆柱侧面）的离散面本身是平面四边形，用顶点算平面即可——
/// 解析标注在这里不参与，布尔结果统一回到平面表示。
pub fn solid_to_polygons(s: &Solid) -> Vec<Polygon> {
    let mut out = Vec::with_capacity(s.faces.len());
    for f in &s.faces {
        let verts: Vec<Point3> = f
            .verts
            .iter()
            .filter_map(|i| s.verts.get(*i as usize).copied())
            .collect();
        // 带着曲面标注进布尔链，否则圆柱会在重建后退成平面
        if let Some(p) = Polygon::with_surface(verts, f.surface) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solid::{box_solid, cylinder_solid};

    #[test]
    fn 盒子转面片再转回来保持水密() {
        let s = box_solid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let polys = solid_to_polygons(&s);
        assert_eq!(polys.len(), 6);
        let back = polygons_to_solid(&polys).unwrap();
        assert!(back.is_watertight());
        assert_eq!(back.verts.len(), 8, "顶点焊接后应回到 8 个");
    }

    #[test]
    fn 圆柱离散面也能走布尔表示() {
        let s = cylinder_solid([0.0, 0.0, 0.0], 1.0, 2.0, 16);
        let polys = solid_to_polygons(&s);
        assert_eq!(polys.len(), 18, "16 侧面 + 顶 + 底");
        let back = polygons_to_solid(&polys).unwrap();
        assert!(back.is_watertight());
    }

    #[test]
    fn 空面片集合显式报错() {
        let err = polygons_to_solid(&[]).unwrap_err();
        assert!(err.to_string().contains("没有任何面片"), "实际：{err}");
    }

    #[test]
    fn 顶点容差内合并() {
        let mut verts = vec![[0.0, 0.0, 0.0]];
        let i = find_or_push(&mut verts, [1e-9, 0.0, 0.0]);
        assert_eq!(i, 0, "1e-9 差应视为同一点");
        let j = find_or_push(&mut verts, [1e-3, 0.0, 0.0]);
        assert_eq!(j, 1, "1mm 差应视为不同点");
    }
}
