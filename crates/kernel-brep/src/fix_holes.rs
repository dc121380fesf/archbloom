//! 用圆柱面的端圆修复带洞面的洞边界。
//!
//! # 为什么这么做
//!
//! 合并后的洞边界是「44 个圆点 + 41 个伸向板内的偏点」——多环切分把一小块
//! 三角区域误判成了洞的一部分。
//!
//! 但**孔壁本身是 64 个圆柱面片**，它们的端圆就是精确的 64 点圆。
//! 与其修洞的边界，不如直接用孔壁的端圆替换它——这是唯一能保证孔口
//! 真正是圆的办法。
//!
//! # 前提
//!
//! 洞的圆心 / 半径必须与某个圆柱面组吻合，否则不动（宁可留错，不乱改）。

use std::collections::HashMap;

use archcore_kernel::{Point3, Vector3};

use crate::geom::{dot, normalize, sub, Surface};
use crate::solid::{Face, Solid};

/// 一个圆柱面组在某个端面上的圆。
struct EndCircle {
    center: Point3,
    #[allow(dead_code)]
    axis: Vector3,
    radius: f64,
    /// 圆上的顶点（按绕轴角度排序）。
    ring: Vec<u32>,
}

/// 用圆柱面的端圆修复洞边界。返回被替换的洞数量。
pub fn fix_holes_with_cylinder_rings(solid: &mut Solid) -> usize {
    let circles = collect_end_circles(solid);
    if circles.is_empty() {
        return 0;
    }

    let mut fixed = 0usize;    for fi in 0..solid.faces.len() {
        if solid.faces[fi].holes.is_empty() {
            continue;
        }
        let surface = solid.faces[fi].surface;
        let normal = surface
            .normal_at(solid.verts[solid.faces[fi].verts[0] as usize])
            .unwrap_or([0.0, 0.0, 1.0]);
        let mut new_holes: Vec<Vec<u32>> = Vec::new();
        for hole in &solid.faces[fi].holes {
            // 洞的质心（只用来校验端圆是否同平面）
            let n = hole.len() as f64;
            let mut c = [0.0; 3];
            for &v in hole {
                let p = solid.verts[v as usize];
                for (ck, pk) in c.iter_mut().zip(p.iter()) {
                    *ck += *pk;
                }
            }
            for ck in c.iter_mut() {
                *ck /= n;
            }

            // 洞与端圆的匹配：不看质心（偏点会把质心拉偏），
            // 而是看**有多少洞的点落在端圆上**。
            // 开孔板的洞是 44 个圆点 + 41 个偏点，占比 52%，足够判定。
            let mut best: Option<(&EndCircle, f64)> = None;
            for ec in &circles {
                // 端圆必须在同一个平面上
                let d = dot(sub(ec.center, c), normal).abs();
                if d > 1e-6 {
                    continue;
                }
                let mut on = 0usize;
                for &v in hole {
                    let p = solid.verts[v as usize];
                    let dv = sub(p, ec.center);
                    // 到圆周的距离（允许 1% 误差）
                    let rr = dot(dv, dv).sqrt();
                    if (rr - ec.radius).abs() < ec.radius * 0.01 {
                        on += 1;
                    }
                }
                let ratio = on as f64 / hole.len() as f64;
                if ratio < 0.3 {
                    continue;
                }
                match best {
                    Some((_, br)) if br >= ratio => {}
                    _ => best = Some((ec, ratio)),
                }
            }
            match best {
                Some((ec, _)) => {
                    new_holes.push(ec.ring.clone());
                    fixed += 1;
                }
                None => new_holes.push(hole.clone()),
            }
        }
        solid.faces[fi].holes = new_holes;
    }

    // 删除「完全落在洞内部」的面
    //
    // 换掉洞边界后，原洞边界上那些碎片面还在实体里，它们的边全变成孤边
    // （实测 86 条），水密直接变 false。这些面是位置错乱的垃圾——真实几何里
    // 洞内部不该有面。
    if fixed > 0 {
        drop_faces_inside_holes(solid);
    }

    fixed
}

/// 删除所有顶点都落在某个洞内部的面。
fn drop_faces_inside_holes(solid: &mut Solid) {
    // 先收集所有洞（平面 + 圆），把它们投影成 2D 多边形
    struct HolePoly {
        normal: Vector3,
        origin: Point3,
        pts: Vec<[f64; 2]>,
    }
    let mut polys: Vec<HolePoly> = Vec::new();
    for f in &solid.faces {
        if f.holes.is_empty() {
            continue;
        }
        let normal = f
            .surface
            .normal_at(solid.verts[f.verts[0] as usize])
            .unwrap_or([0.0, 0.0, 1.0]);
        let Some((u, w)) = crate::geom::orthonormal_basis(normal) else {
            continue;
        };
        for h in &f.holes {
            let origin = solid.verts[h[0] as usize];
            let pts: Vec<[f64; 2]> = h
                .iter()
                .map(|&i| {
                    let d = sub(solid.verts[i as usize], origin);
                    [dot(d, u), dot(d, w)]
                })
                .collect();
            polys.push(HolePoly {
                normal,
                origin,
                pts,
            });
        }
    }
    if polys.is_empty() {
        return;
    }

    let mut keep: Vec<Face> = Vec::with_capacity(solid.faces.len());
    for f in solid.faces.drain(..) {
        // 只考虑小面（面积小于洞的 1/4）——大面可能是真正的面
        let mut drop = false;
        for p in &polys {
            // 顶点必须与该洞同平面
            let mut all_in = true;
            let Some((u, w)) = crate::geom::orthonormal_basis(p.normal) else {
                continue;
            };
            for &v in f.all_rings().flatten() {
                let d = sub(solid.verts[v as usize], p.origin);
                let off = dot(d, p.normal);
                if off.abs() > 1e-6 {
                    all_in = false;
                    break;
                }
                if !crate::fix_holes::point_in_poly(
                    [dot(d, u), dot(d, w)],
                    &p.pts,
                ) {
                    all_in = false;
                    break;
                }
            }
            if all_in {
                drop = true;
                break;
            }
        }
        if !drop {
            keep.push(f);
        }
    }
    solid.faces = keep;
}

/// 射线法判点在多边形内。
pub fn point_in_poly(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let n = poly.len();
    let mut inside = false;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let t = (p[1] - a[1]) / (b[1] - a[1]);
            let x = a[0] + t * (b[0] - a[0]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// 从所有圆柱面片收集端圆。
fn collect_end_circles(solid: &Solid) -> Vec<EndCircle> {
    // 按 (origin, axis, radius) 分组
    let mut groups: HashMap<String, (Point3, Vector3, f64, Vec<usize>)> = HashMap::new();
    for (i, f) in solid.faces.iter().enumerate() {
        if let Surface::Cylinder {
            origin,
            axis,
            radius,
        } = f.surface
        {
            let key = format!(
                "{:.6},{:.6},{:.6}|{:.6},{:.6},{:.6}|{:.6}",
                origin[0], origin[1], origin[2], axis[0], axis[1], axis[2], radius
            );
            groups
                .entry(key)
                .or_insert_with(|| (origin, axis, radius, Vec::new()))
                .3
                .push(i);
        }
    }

    let mut out = Vec::new();
    for (_, (origin, axis, radius, faces)) in groups {
        let Some(ax) = normalize(axis) else { continue };
        // 收集组内所有顶点，按沿轴高度分两层
        let mut hs: Vec<f64> = Vec::new();
        let mut verts: Vec<u32> = Vec::new();
        for &fi in &faces {
            for &v in &solid.faces[fi].verts {
                if !verts.contains(&v) {
                    verts.push(v);
                    let h = dot(sub(solid.verts[v as usize], origin), ax);
                    hs.push(h);
                }
            }
        }
        if hs.len() < 8 {
            continue;
        }
        hs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let hmin = hs[0];
        let hmax = hs[hs.len() - 1];
        let _mid = (hmin + hmax) * 0.5;
        let (u, w) = match crate::geom::orthonormal_basis(ax) {
            Some(x) => x,
            None => continue,
        };
        // 两层分别成环
        for &h_target in &[hmin, hmax] {
            let mut ring: Vec<(f64, u32)> = Vec::new();
            for &v in &verts {
                let p = solid.verts[v as usize];
                let h = dot(sub(p, origin), ax);
                if (h - h_target).abs() > 1e-9 {
                    continue;
                }
                let d = sub(p, origin);
                let radial = sub(d, [ax[0] * h, ax[1] * h, ax[2] * h]);
                let ang = dot(radial, w).atan2(dot(radial, u));
                ring.push((ang, v));
            }
            if ring.len() < 8 {
                continue;
            }
            ring.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            out.push(EndCircle {
                center: [
                    origin[0] + ax[0] * h_target,
                    origin[1] + ax[1] * h_target,
                    origin[2] + ax[2] * h_target,
                ],
                axis: ax,
                radius,
                ring: ring.into_iter().map(|(_, v)| v).collect(),
            });
        }
    }
    out
}
