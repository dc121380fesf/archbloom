//! 用半边重做面片合并。
//!
//! # 为什么重做
//!
//! 现有 `merge.rs` 是「几何拼接」：两个面的顶点环数组合并、算重复顶点、按面积猜哪段是洞。
//! 十轮调试证明这条路不可靠——环的边界取决于拼接顺序，洞会被划错（实测 955 点里
//! 只有 17 个在孔圆上）。
//!
//! # 半边拼接的优势
//!
//! 沿共享边拼接两个面，本质是**改两条 next 指针**：
//!
//! ```text
//! A 的环：… → ha → next_a → …
//! B 的环：… → hb → next_b → …
//!
//! 拼接后：… → next_b → … → next_a → …
//!          （ha 与 hb 被跳过）
//! ```
//!
//! **完全不做几何计算**，不存在「算出来坐标相同但索引不同」的问题。
//!
//! # 环的重建
//!
//! 拼接后沿 next 走一圈就是新环。如果回到起点之前遇到已访问顶点，
//! 说明这段闭合成了**洞**——按栈切开，与 `merge.rs` 里那段判环逻辑同源，
//! 但输入是「半边链」而不是「几何数组」，不会因为浮点误差串错。
//!
//! # 现状
//!
//! 这是探索性实现，还没接进 `merge_coplanar`。先验证：
//! 1. 盒子/圆柱这类干净实体，拼接后面积与顶点数都对
//! 2. 开孔板合并后，洞的顶点数接近圆柱段数

use std::collections::{HashMap, HashSet};

use archcore_kernel::{KernelError, Point3, Vector3};

use crate::geom::{dot, sub};
use crate::solid::{Face, Solid};

/// 一次拼接的候选：两个面 + 它们的共享边。
#[allow(dead_code)]
struct Pair {
    f1: usize,
    f2: usize,
    ha: usize,
    hb: usize,
}

/// 用半边拼接把「同曲面且相邻」的面合并。返回新实体。
pub fn merge_by_half_edge(
    solid: &Solid,
    angle_tol_deg: f64,
) -> Result<(Solid, usize), KernelError> {
    let mut work = solid.clone();
    work.build_topology();

    let mut merged_count = 0usize;
    while let Some(pair) = find_pair(&work, angle_tol_deg) {
        splice(&mut work, &pair)?;
        merged_count += 1;
        if merged_count > solid.faces.len() * 2 {
            break;
        }
    }

    // 重建面表
    let faces = rebuild_faces(&work);
    let mut out = Solid {
        verts: work.verts.clone(),
        faces,
        half_edges: Vec::new(),
        edges: Vec::new(),
        analytic_volume: solid.analytic_volume,
        analytic_area: solid.analytic_area,
    };
    out.build_topology();
    Ok((out, merged_count))
}

/// 找一对「同曲面、相邻」的面。
fn find_pair(s: &Solid, angle_tol_deg: f64) -> Option<Pair> {
    for (ei, e) in s.edges.iter().enumerate() {
        let _ = ei;
        let (ha, hb) = e.he;
        let f1 = s.half_edges[ha].face;
        let f2 = s.half_edges[hb].face;
        if f1 == f2 {
            continue;
        }
        if !same_surface(&s.faces[f1], &s.faces[f2], s, angle_tol_deg) {
            continue;
        }
        // 已经是同一条边了（不会发生），跳过
        return Some(Pair { f1, f2, ha, hb });
    }
    None
}

/// 两个面是否在同一个「平面」上（只处理平面，曲面暂不合并）。
fn same_surface(a: &Face, b: &Face, s: &Solid, angle_tol_deg: f64) -> bool {
    let na = a.surface.normal_at(s.verts[a.verts[0] as usize]);
    let nb = b.surface.normal_at(s.verts[b.verts[0] as usize]);
    match (na, nb) {
        (Some(na), Some(nb)) => {
            let ang = dot(na, nb).clamp(-1.0, 1.0).acos().to_degrees();
            if ang > angle_tol_deg {
                return false;
            }
            // 同平面：a 的任一点到 b 的平面距离 ≈ 0
            // 用 a 的质心到 b 的平面距离判断
            let ca = centroid(s, &a.verts);
            let d = sub(ca, s.verts[b.verts[0] as usize]);
            dot(d, nb).abs() < 1e-9
        }
        _ => false,
    }
}

fn centroid(s: &Solid, ring: &[u32]) -> Point3 {
    let mut c = [0.0; 3];
    for &i in ring {
        let p = s.verts[i as usize];
        for (ck, pk) in c.iter_mut().zip(p.iter()) {
            *ck += *pk;
        }
    }
    let n = ring.len() as f64;
    [c[0] / n, c[1] / n, c[2] / n]
}

/// 沿共享边拼接两个面：只改 next，不做几何计算。
fn splice(s: &mut Solid, p: &Pair) -> Result<(), KernelError> {
    let (ha, hb) = (p.ha, p.hb);
    if s.half_edges[ha].twin != hb {
        return Err(KernelError::TopologyBroken {
            at: "splice".to_string(),
            reason: "两个半边不是 twin".to_string(),
            suggestion: "先 build_topology".to_string(),
        });
    }
    // A 的路径：从 ha.next 走到 ha
    // B 的路径：从 hb.next 走到 hb
    // 拼接：把 ha 的「前一条」的 next 指向 hb.next，
    //       把 hb 的「前一条」的 next 指向 ha.next
    let prev_a = prev_of(s, ha);
    let prev_b = prev_of(s, hb);
    let next_a = s.half_edges[ha].next;
    let next_b = s.half_edges[hb].next;

    // 把 B 的环接到 A 上（B 的顶点归到 A 的面）
    let f1 = s.half_edges[ha].face;
    let f2 = s.half_edges[hb].face;
    let mut cur = next_b;
    loop {
        s.half_edges[cur].face = f1;
        cur = s.half_edges[cur].next;
        if cur == next_b {
            break;
        }
    }
    s.half_edges[prev_a].next = next_b;
    s.half_edges[prev_b].next = next_a;
    // **摘除 ha/hb**：它们不再属于任何环。
    //
    // 不摘的话它们仍留着旧的 next，遍历时会形成「短路」——
    // 从 ha 走进旧链，遇到已访问顶点就断开，环被切成三角形
    // （实测 4 个洞全是 3 个点）。
    // 指向自己做成自环，遍历时一步就终止（<3 个点会被丢弃）。
    s.half_edges[ha].next = ha;
    s.half_edges[hb].next = hb;
    s.half_edges[ha].twin = usize::MAX;
    s.half_edges[hb].twin = usize::MAX;
    // 同步更新面表里的环（后续 rebuild_faces 会重建）
    let _ = f2;
    Ok(())
}

/// 找某条半边的「前一条」（环上）。
fn prev_of(s: &Solid, he: usize) -> usize {
    let target = he;
    let mut cur = s.half_edges[he].next;
    loop {
        if s.half_edges[cur].next == target {
            return cur;
        }
        cur = s.half_edges[cur].next;
        if cur == he {
            return he;
        }
    }
}

/// 从半边链重建面表（含洞的切分）。
fn rebuild_faces(s: &Solid) -> Vec<Face> {
    let mut visited: HashSet<usize> = HashSet::new();
    let mut out: Vec<Face> = Vec::new();
    // 按面分组：同一 face 的半边应当形成若干环（外环 + 洞）
    let mut by_face: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, he) in s.half_edges.iter().enumerate() {
        if he.twin == usize::MAX {
            continue;
        }
        by_face.entry(he.face).or_default().push(i);
    }
    for (fi, start_hes) in by_face {
        let mut rings: Vec<Vec<u32>> = Vec::new();
        for &start in &start_hes {
            if visited.contains(&start) {
                continue;
            }
            let mut ring = Vec::new();
            let mut cur = start;
            loop {
                visited.insert(cur);
                ring.push(s.half_edges[cur].origin);
                cur = s.half_edges[cur].next;
                if cur == start {
                    break;
                }
                if visited.contains(&cur) {
                    break;
                }
            }
            if ring.len() >= 3 {
                rings.push(ring);
            }
        }
        if rings.is_empty() {
            continue;
        }
        // 面积最大的当外环
        let normal = s.faces[fi]
            .surface
            .normal_at(s.verts[rings[0][0] as usize])
            .unwrap_or([0.0, 0.0, 1.0]);
        rings.sort_by(|a, b| {
            ring_area(s, b, normal)
                .partial_cmp(&ring_area(s, a, normal))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let outer = rings.remove(0);
        out.push(Face {
            surface: s.faces[fi].surface,
            verts: outer,
            holes: rings,
        });
    }
    out
}

fn ring_area(s: &Solid, ring: &[u32], n: Vector3) -> f64 {
    let m = ring.len();
    if m < 3 {
        return 0.0;
    }
    let mut acc = [0.0; 3];
    for i in 0..m {
        let a = s.verts[ring[i] as usize];
        let b = s.verts[ring[(i + 1) % m] as usize];
        acc[0] += (a[1] - b[1]) * (a[2] + b[2]);
        acc[1] += (a[2] - b[2]) * (a[0] + b[0]);
        acc[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    dot(acc, n) * 0.5
}
