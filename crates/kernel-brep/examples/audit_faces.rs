use archcore_kernel::{BoolOp, CancelToken, GeometryKernel};
use archcore_kernel_brep::{box_solid, cylinder_solid, merge_by_half_edge, BrepKernel};

fn main() {
    println!("=== 体检：同一个曲面被切成多少片 ===\n");

    let b = box_solid([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    println!("盒子（6 面）: {:?}", b.audit());

    for seg in [16usize, 64] {
        let c = cylinder_solid([0.0, 0.0, 0.0], 0.005, 0.02, seg);
        println!("圆柱 {seg} 段（{} 面）: {:?}", c.faces.len(), c.audit());
    }

    let mut k = BrepKernel::new();
    let p = k.add_box([0.0, 0.0, 0.0], [0.020, 0.020, 0.005]);
    let d = k.add_cylinder([0.010, 0.010, -0.001], 0.005, 0.007, 64);
    let mut noop = |_p: archcore_kernel::Progress| {};
    let id = k
        .boolean(p, d, BoolOp::Difference, &CancelToken::new(), &mut noop)
        .expect("bool");
    let s = k.get(id).expect("s");
    println!("\n开孔板（布尔后 {} 面）:", s.faces.len());
    for (desc, n) in s.audit() {
        println!("  {desc} → {n} 片");
    }
    let (m, _) = merge_by_half_edge(s, 1.0).expect("m");
    println!("\n开孔板（合并后 {} 面）:", m.faces.len());
    for (desc, n) in m.audit() {
        println!("  {desc} → {n} 片");
    }
    // 面级 check
    let mut bad = 0;
    for f in &m.faces {
        if f.check(&m, 1e-6).is_err() {
            bad += 1;
        }
    }
    println!("\nFace::check 不通过的面: {bad}（目前 boundary 全为 None，应为 0）");
}
