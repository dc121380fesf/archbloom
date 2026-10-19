use std::collections::HashMap;

use archcore_kernel::Point3;

use crate::geom::{cross, dot, sub, Surface};
use crate::solid::{Face, Solid};

use super::fillet::{centroid_of, dedup};
// ─── 角部缝合（从 fillet_solid 抽出，逻辑逐字未改） ───
//
// 抽出来的目的：倒角和圆角都在「顶点处若干条棱相交，要补一块面」，
// 把这段独立出来之后，才能按「选中棱数」分类，让 2 棱角部（圆柱）
// 和 3 棱角部（盒子）各走各的分支。
//
// 目前这里只处理 3 段弧的情形（盒子）。2 段及以下直接跳过，
// 这也是圆柱圆角报 TopologyBroken 的直接原因。
pub(super) fn corner_blend(
    out: &mut Solid,
    table: &[crate::edges::EdgeRec],
    selected: &[bool],
    spheres: &HashMap<u32, (Point3, Vec<usize>)>,
    arcs: &HashMap<(u32, u32, bool), Vec<u32>>,
    r: f64,
) {
    for (&v, (c, _faces_at_v)) in spheres {
        struct ArcSeg {
            pts: Vec<u32>,
            from: u32,
            to: u32,
        }
        let mut segs: Vec<ArcSeg> = Vec::new();
        for (i, e) in table.iter().enumerate() {
            if !selected[i] || (e.a != v && e.b != v) {
                continue;
            }
            let at_b = e.b == v;
            let Some(chain) = arcs.get(&(e.a, e.b, at_b)) else {
                continue;
            };
            if chain.len() < 2 {
                continue;
            }
            let from = *chain.first().unwrap();
            let to = *chain.last().unwrap();
            segs.push(ArcSeg {
                pts: chain.clone(),
                from,
                to,
            });
        }
        if std::env::var("ARCHCORE_DEBUG_CORNER").is_ok() {
            eprintln!(
                "[corner] v={v} segs={} faces_at_v={:?}",
                segs.len(),
                _faces_at_v
            );
            for (k, s) in segs.iter().enumerate() {
                eprintln!("  seg{k} pts={} from={} to={}", s.pts.len(), s.from, s.to);
            }
        }
        if segs.len() < 3 {
            continue;
        }
        // 串环：从第一条弧开始，每次找「一端等于当前终点」的下一条。
        //
        // **必须允许反向使用**：每条端圆的走向由叉积符号决定，
        // 三条弧不一定首尾同向（实测 seg0: 1→3、seg1: 1→13、seg2: 3→13，
        // 全同向串不起来）。反向时把点列倒过来走就行。
        let mut ring: Vec<u32> = Vec::new();
        let mut used = vec![false; segs.len()];
        let mut cur_end = segs[0].from;
        let mut ok = true;
        let mut first = true;
        for _ in 0..segs.len() {
            let Some(k) = segs
                .iter()
                .enumerate()
                .position(|(j, s)| !used[j] && (s.from == cur_end || s.to == cur_end))
            else {
                ok = false;
                break;
            };
            used[k] = true;
            let rev = segs[k].to == cur_end && segs[k].from != cur_end;
            // **第一条弧不能 skip**：它的首点就是环的起点。
            let skip = if first { 0 } else { 1 };
            first = false;
            if rev {
                for &p in segs[k].pts.iter().rev().skip(skip) {
                    if ring.last() != Some(&p) {
                        ring.push(p);
                    }
                }
                cur_end = segs[k].from;
            } else {
                for &p in segs[k].pts.iter().skip(skip) {
                    if ring.last() != Some(&p) {
                        ring.push(p);
                    }
                }
                cur_end = segs[k].to;
            }
        }
        if !ok || cur_end != segs[0].from {
            continue;
        }
        if ring.len() > 1 && ring.first() == ring.last() {
            ring.pop();
        }
        let mut ring = dedup(ring);
        if ring.len() < 3 {
            continue;
        }
        // 朝向：球面片必须**背离球心**。
        //
        // 不能用「投影累积 ccw」判：球面三角不共面，各项互相抵消，符号会抖。
        // 改用「头三个不共线点的平面法线 vs 径向」。
        let centroid = centroid_of(out, &ring);
        let radial = sub(centroid, *c);
        let mut nrm_ref = [0.0, 0.0, 0.0];
        for i in 1..ring.len().saturating_sub(1) {
            let a = out.verts[ring[0] as usize];
            let b = out.verts[ring[i] as usize];
            let d = out.verts[ring[i + 1] as usize];
            let cr = cross(sub(b, a), sub(d, a));
            if (cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]).sqrt() > 1e-12 {
                nrm_ref = cr;
                break;
            }
        }
        if dot(nrm_ref, radial) < 0.0 {
            ring.reverse();
        }
        out.faces.push(Face {
            surface: Surface::Sphere {
                center: *c,
                radius: r,
            },
            verts: ring,
            holes: Vec::new(),
        });
    }
}
