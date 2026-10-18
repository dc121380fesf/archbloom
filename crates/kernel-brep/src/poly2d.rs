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
            if cross2(a, b, c) <= 1e-16 {
                continue; // 凹点或退化
            }
            let mut clean = true;
            for &j in &idx {
                if j == ia || j == ib || j == ic {
                    continue;
                }
                if point_in_tri(poly[j], a, b, c) {
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
        // 兜底：剩余部分扇形展开，保证不丢面积
        for i in 1..idx.len() - 1 {
            out.push([idx[0], idx[i], idx[i + 1]]);
        }
    }
    out
}

/// 把洞挂到外环上，多环变单环。
///
/// 返回的环里，桥接处有相邻重复点（缝），耳切时按退化跳过。
fn bridge(outer: &[P2], holes: &[Vec<P2>]) -> Vec<usize> {
    let mut ring: Vec<usize> = (0..outer.len()).collect();
    let mut all: Vec<P2> = outer.to_vec();
    for h in holes {
        let base = all.len();
        all.extend_from_slice(h);
        let hidx: Vec<usize> = (base..base + h.len()).collect();

        let mut best = (f64::MAX, 0usize, 0usize);
        for (ri, &oi) in ring.iter().enumerate() {
            for (hi, &hii) in hidx.iter().enumerate() {
                let a = all[oi];
                let b = all[hii];
                let d = (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2);
                if d < best.0 {
                    best = (d, ri, hi);
                }
            }
        }
        let (_, ri, hi) = best;
        let mut inset: Vec<usize> = Vec::with_capacity(hidx.len() + 2);
        inset.push(ring[ri]);
        for k in 0..hidx.len() {
            inset.push(hidx[(hi + k) % hidx.len()]);
        }
        inset.push(ring[ri]);
        ring.splice(ri + 1..ri + 1, inset);
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

