use archcore_kernel_brep::{box_solid, cylinder_solid, edge_table};
use std::collections::HashMap;

/// 对给定实体，测量「同一顶点在同一个面上，两条棱给出的轨线端点」的间距。
/// 求知的理论：这个间距应 ≈ sqrt(2)*R*sin(pi/N)，随 N 增大按 ~N 反比缩小。
fn measure(label: &str, s: &archcore_kernel_brep::Solid, r: f64) {
    let t = edge_table(s).expect("table");
    // 面 → 顶点 → 各条棱在这个面上的端点（用"沿棱内缩 R"近似轨线端点）
    let mut groups: HashMap<(usize, u32), Vec<[f64; 3]>> = HashMap::new();
    for e in &t {
        let pa = s.verts[e.a as usize];
        let pb = s.verts[e.b as usize];
        for (v, other) in [(e.a, pb), (e.b, pa)] {
            let pv = s.verts[v as usize];
            let d = [other[0] - pv[0], other[1] - pv[1], other[2] - pv[2]];
            let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if l < 1e-15 {
                continue;
            }
            // 沿棱内缩 R
            let q = [
                pv[0] + d[0] / l * r,
                pv[1] + d[1] / l * r,
                pv[2] + d[2] / l * r,
            ];
            for fi in [e.f1, e.f2] {
                groups.entry((fi, v)).or_default().push(q);
            }
        }
    }
    let mut gaps: Vec<f64> = Vec::new();
    for pts in groups.values() {
        if pts.len() < 2 {
            continue;
        }
        for i in 0..pts.len() {
            for j in (i + 1)..pts.len() {
                let d = [
                    pts[i][0] - pts[j][0],
                    pts[i][1] - pts[j][1],
                    pts[i][2] - pts[j][2],
                ];
                gaps.push((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt());
            }
        }
    }
    let max_gap = gaps.iter().cloned().fold(0.0_f64, f64::max);
    println!(
        "{label:<24} R={r:.4}  样本 {:>3} 个  最大间隙 {max_gap:.6}  相对 R 的倍数 {:.4}",
        gaps.len(),
        max_gap / r
    );
}

fn main() {
    let r = 0.2;
    measure("盒子 2×2×2", &box_solid([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]), r);

    let rc = 0.005;
    let rr = 0.001;
    for n in [16usize, 32, 64, 128] {
        let c = cylinder_solid([0.0, 0.0, 0.0], rc, 0.02, n);
        measure(&format!("圆柱 {n} 段"), &c, rr);
        // 理论：sqrt(2) * R * sin(pi/N)
        let theory = 2.0_f64.sqrt() * rr * (std::f64::consts::PI / n as f64).sin();
        println!("  理论 √2·R·sin(π/{n}) = {theory:.6}（{:.4}·R）", theory / rr);
    }
}
