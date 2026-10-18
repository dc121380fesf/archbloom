//! 带洞多边形的三角化：桥接 + 耳切法。
//!
//! # 为什么不能用扇形剖分
//!
//! `Face::triangulate` 原本把环从 0 号顶点扇形展开——只对**凸的简单多边形**成立。
//! 面片合并之后出现「外方形 + 内圆孔」这种带洞面，扇形剖分会**把洞填上**：
//! 实测开孔板体积因此偏大 8.1%。
//!
//! # 算法
//!
//! 1. **桥接**：给每个洞找外环上最近的一个顶点，用一条「来回」的缝把洞挂到外环上，
//!    多环变单环。缝的两条边方向相反，耳切时因面积为零被跳过。
//! 2. **耳切**：经典 ear clipping，对凹多边形也正确。
//!
//! 全程在面的二维局部坐标里做，最后映射回三维索引。

use archcore_kernel::{Point3, Vector3};

use crate::geom::{dot, orthonormal_basis, sub};

/// 二维点。
type P2 = [f64; 2];

/// 有符号面积（逆时针为正）。
fn area2(poly: &[P2]) -> f64 {
    let n = poly.len();
    let mut s = 0.0;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        s += a[0] * b[1] - b[0] * a[1];
    }
    s * 0.5
}

fn cross2(a: P2, b: P2, c: P2) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn point_in_tri(p: P2, a: P2, b: P2, c: P2) -> bool {
    let d1 = cross2(a, b, p);
    let d2 = cross2(b, c, p);
    let d3 = cross2(c, a, p);
    let neg = d1 < -1e-14 || d2 < -1e-14 || d3 < -1e-14;
    let pos = d1 > 1e-14 || d2 > 1e-14 || d3 > 1e-14;
    !(neg && pos)
}

/// 耳切三角化。输入须为逆时针简单多边形，返回其**局部下标**三元组。
fn ear_clip(poly: &[P2]) -> Vec<[usize; 3]> {
    let n = poly.len();
    let mut out = Vec::new();
    if n < 3 {
        return out;
    }
    if n == 3 {
        out.push([0, 1, 2]);
        return out;
    }
    // 阈值必须跟多边形尺度走。写死 1e-16 对 0.02 量级的投影坐标来说太松，
    // 桥接缝附近的退化耳剪不掉，剩下的块被兜底扇形展开时产生翻转三角形。
    let scale = poly
        .iter()
        .map(|p| p[0].abs().max(p[1].abs()))
        .fold(0.0_f64, f64::max)
        .max(1e-9);
    let eps = scale * scale * 1e-10;

    let mut idx: Vec<usize> = (0..n).collect();
    let mut guard = 0usize;
    while idx.len() > 3 && guard < n * n + 4 {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let ia = idx[(i + m - 1) % m];
            let ib = idx[i];
            let ic = idx[(i + 1) % m];
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            if cross2(a, b, c) <= eps {
                continue; // 凹点或退化
            }
            let mut clean = true;
            for &j in &idx {
                if j == ia || j == ib || j == ic {
                    continue;
                }
                // 坐标与三角形任一顶点重合的点不算遮挡。
                //
                // 桥接缝里 anchor 会出现两次（两个不同索引、同一坐标），
                // 洞上相邻点在极瘦三角形里也几乎共线——`point_in_tri`
                // 对「全 0」情形返回 true（视为边界上），于是洞点互相
                // “占位”，所有耳都被拒（实测 guard=1 就卡住）。
                let pj = poly[j];
                let dup = [ia, ib, ic].iter().any(|&k| {
                    let q = poly[k];
                    (pj[0] - q[0]).abs() <= eps && (pj[1] - q[1]).abs() <= eps
                });
                if dup {
                    continue;
                }
                if point_in_tri(pj, a, b, c) {
                    clean = false;
                    break;
                }
            }
            if !clean {
                continue;
            }
            out.push([ia, ib, ic]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break; // 退化/自交：交给下面的兜底
        }
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    } else if idx.len() > 3 {
        // 兜底：**不能盲目扇形展开**。剩余块多半是凹的，扇出的三角形会翻转，
        // 有向面积被抵消（实测 195 个三角形里 43 个翻转，体积偏 1.4 mm³）。
        // 逐三角形校正方向：负面积的交换两个顶点。
        for i in 1..idx.len() - 1 {
            let mut tri = [idx[0], idx[i], idx[i + 1]];
            if cross2(poly[tri[0]], poly[tri[1]], poly[tri[2]]) < 0.0 {
                tri.swap(1, 2);
            }
            out.push(tri);
        }
    }
    out
}

/// 把洞挂到外环上，多环变单环。
///
/// # 为什么不能只挑最近点
///
/// 老实现只找「距离最近的洞点 / 外环点」就下桥。但桥接线可能**穿过别的边**——
/// 自交之后耳切必然卡住，剩下的块被兜底扇形展开，三角形覆盖到洞里，
/// 有向面积算出来比外框还大（实测 0.000513 m² vs 理论 0.000322 m²）。
///
/// # 现在的做法
///
/// 射线法（和 earcut 的 eliminateHole 同思路）：
/// 1. 取洞的**最右点** h（x 最大）
/// 2. 从 h 向右发射线，找外环上**最近**的跨越边
/// 3. 桥接到那条边的**右端点**
///
/// 因为射线只在 h 的右侧扫，落点必然在洞外，桥接线也不可能穿过洞本身。
///
/// 返回的环里，桥接处有相邻重复点（缝），耳切时按退化跳过。
fn bridge(outer: &[P2], holes: &[Vec<P2>]) -> Vec<usize> {
    let mut ring: Vec<usize> = (0..outer.len()).collect();
    let mut all: Vec<P2> = outer.to_vec();

    for h in holes {
        if h.len() < 3 {
            continue;
        }
        let base = all.len();
        all.extend_from_slice(h);
        let hidx: Vec<usize> = (base..base + h.len()).collect();

        // 1. 洞的最右点
        let mut best_hi = 0usize;
        for (k, &i) in hidx.iter().enumerate() {
            let p = all[i];
            let q = all[hidx[best_hi]];
            if p[0] > q[0] + 1e-18 || (p[0] - q[0]).abs() <= 1e-18 && p[1] < q[1] {
                best_hi = k;
            }
        }
        let hp = all[hidx[best_hi]];

        // 2. 向右射线，找最近跨越边
        let mut hit: Option<(f64, usize)> = None; // (交点 x, ring 中边的起点下标)
        for ri in 0..ring.len() {
            let oi = ring[ri];
            let oj = ring[(ri + 1) % ring.len()];
            let a = all[oi];
            let b = all[oj];
            // 边的两个端点必须分居射线两侧（严格）
            let above_a = a[1] > hp[1];
            let above_b = b[1] > hp[1];
            if above_a == above_b {
                continue;
            }
            let dy = b[1] - a[1];
            if dy.abs() < 1e-18 {
                continue;
            }
            let t = (hp[1] - a[1]) / dy;
            if !(0.0..=1.0).contains(&t) {
                continue;
            }
            let x = a[0] + t * (b[0] - a[0]);
            if x < hp[0] {
                continue; // 交点在洞的左边，不要
            }
            if hit.is_none_or(|(bx, _)| x < bx) {
                hit = Some((x, ri));
            }
        }

        // 3. 桥接到那条边的右端点；找不到就退回最近点对（保底）
        let anchor = match hit {
            Some((_, ri)) => {
                let oi = ring[ri];
                let oj = ring[(ri + 1) % ring.len()];
                if all[oi][0] >= all[oj][0] {
                    ri
                } else {
                    (ri + 1) % ring.len()
                }
            }
            None => {
                let mut best = (f64::MAX, 0usize);
                for (ri, &oi) in ring.iter().enumerate() {
                    for &hii in &hidx {
                        let a = all[oi];
                        let b = all[hii];
                        let d = (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2);
                        if d < best.0 {
                            best = (d, ri);
                        }
                    }
                }
                best.1
            }
        };

        // 桥接：ring[anchor] → 洞全程 → ring[anchor]
        //
        // inset 里**不能再包含 anchor 本身**。anchor 已经在环上，
        // 写成 [anchor, 洞…, anchor] 会让环里出现 3 个 anchor，
        // 耳切第一轮就全部被 point_in_tri 拒掉（guard=1 就卡住）。
        let mut inset: Vec<usize> = Vec::with_capacity(hidx.len() + 1);
        for k in 0..hidx.len() {
            inset.push(hidx[(best_hi + k) % hidx.len()]);
        }
        inset.push(ring[anchor]);
        ring.splice(anchor + 1..anchor + 1, inset);
    }
    ring
}

/// 三角化一个面（可能带洞），返回三维顶点索引三元组。
pub fn triangulate_face(
    verts: &[Point3],
    ring: &[u32],
    holes: &[Vec<u32>],
    normal: Vector3,
) -> Vec<[u32; 3]> {
    if ring.len() < 3 {
        return Vec::new();
    }
    let (u, w) = match orthonormal_basis(normal) {
        Some(x) => x,
        None => return Vec::new(),
    };
    let origin = verts[ring[0] as usize];
    let proj = |i: u32| -> P2 {
        let d = sub(verts[i as usize], origin);
        [dot(d, u), dot(d, w)]
    };

    // 外环：投影并统一成逆时针
    let mut outer_idx: Vec<u32> = ring.to_vec();
    let mut outer2: Vec<P2> = ring.iter().map(|&i| proj(i)).collect();
    if area2(&outer2) < 0.0 {
        outer2.reverse();
        outer_idx.reverse();
    }

    if holes.is_empty() {
        return ear_clip(&outer2)
            .into_iter()
            .map(|t| [outer_idx[t[0]], outer_idx[t[1]], outer_idx[t[2]]])
            .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
            .collect();
    }

    // 洞：投影并转成顺时针（与外环反向）
    let mut hole2: Vec<Vec<P2>> = Vec::new();
    let mut hole_idx: Vec<Vec<u32>> = Vec::new();
    for h in holes {
        if h.len() < 3 {
            continue;
        }
        let mut ids = h.clone();
        let mut pts: Vec<P2> = h.iter().map(|&i| proj(i)).collect();
        if area2(&pts) > 0.0 {
            pts.reverse();
            ids.reverse();
        }
        hole2.push(pts);
        hole_idx.push(ids);
    }
    if hole2.is_empty() {
        return ear_clip(&outer2)
            .into_iter()
            .map(|t| [outer_idx[t[0]], outer_idx[t[1]], outer_idx[t[2]]])
            .collect();
    }

    let mut flat: Vec<u32> = outer_idx.clone();
    let mut flat2: Vec<P2> = outer2.clone();
    for (pts, ids) in hole2.iter().zip(hole_idx.iter()) {
        flat.extend_from_slice(ids);
        flat2.extend_from_slice(pts);
    }

    let ring_local = bridge(&outer2, &hole2);
    let poly: Vec<P2> = ring_local.iter().map(|&i| flat2[i]).collect();
    ear_clip(&poly)
        .into_iter()
        .map(|t| [
            flat[ring_local[t[0]]],
            flat[ring_local[t[1]]],
            flat[ring_local[t[2]]],
        ])
        // 桥接缝会产生重复点的退化三角形，丢掉
        .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
        .collect()
}

