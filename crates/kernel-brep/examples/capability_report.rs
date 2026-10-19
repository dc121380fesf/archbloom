use archcore_kernel::{BoolOp, CancelToken, GeometryKernel};
use archcore_kernel_brep::{
    box_solid, chamfer_solid, cylinder_solid, fillet_solid, merge_coplanar, ChamferOptions,
    FilletOptions, BrepKernel,
};

fn vol(s: &archcore_kernel_brep::Solid) -> f64 {
    let (p, _, i) = s.triangulate();
    archcore_kernel_brep::volume_from_mesh(&p, &i)
}

fn main() {
    let pi = std::f64::consts::PI;

    // 1. 盒子
    let b = box_solid([0.0, 0.0, 0.0], [0.020, 0.020, 0.005]);
    println!("盒子 20×20×5:     体积 {:.12} m³  理论 {:.12}", vol(&b), 0.020 * 0.020 * 0.005);

    // 2. 圆柱（不同段数）
    for seg in [16usize, 64, 256] {
        let c = cylinder_solid([0.0, 0.0, 0.0], 0.005, 0.020, seg);
        let v = vol(&c);
        let exact = pi * 0.005 * 0.005 * 0.020;
        println!(
            "圆柱 Φ10×20 {seg:>3} 段: 体积 {:.12}  误差 {:.4}%",
            v,
            (v - exact).abs() / exact * 100.0
        );
    }

    // 3. 开孔板（布尔 + 合并）
    let mut k = BrepKernel::new();
    let p = k.add_box([0.0, 0.0, 0.0], [0.020, 0.020, 0.005]);
    let d = k.add_cylinder([0.010, 0.010, -0.001], 0.005, 0.007, 64);
    let mut noop = |_p: archcore_kernel::Progress| {};
    let id = k
        .boolean(p, d, BoolOp::Difference, &CancelToken::new(), &mut noop)
        .expect("bool");
    let s = k.get(id).expect("s");
    let (m, _) = merge_coplanar(s).expect("m");
    let vm = vol(&m);
    let exact = 0.020 * 0.020 * 0.005 - pi * 0.005 * 0.005 * 0.005;
    println!(
        "开孔板 Φ10:        体积 {:.12}  误差 {:.4}%（{} 面）",
        vm,
        (vm - exact).abs() / exact * 100.0,
        m.faces.len()
    );

    // 4. 倒角盒子
    let cb = chamfer_solid(
        &box_solid([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]),
        &[],
        &ChamferOptions { width: 0.2, angle_threshold_deg: 30.0 },
    )
    .expect("chamfer");
    println!("倒角盒子 R0.2:     体积 {:.9}  FreeCAD 7.562666561  误差 {:.2e}", vol(&cb), ((vol(&cb) - 7.562666561) / 7.562666561).abs());

    // 5. 圆角盒子
    let fb = fillet_solid(
        &box_solid([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]),
        &[],
        &FilletOptions { radius: 0.2, segments: 8 },
    )
    .expect("fillet");
    println!("圆角盒子 R0.2:     体积 {:.9}  FreeCAD 7.804696111  误差 {:.4}%", vol(&fb), ((vol(&fb) - 7.804696111) / 7.804696111).abs() * 100.0);
}
