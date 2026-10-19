use archcore_kernel_brep::{box_solid, fillet_solid, FilletOptions};

fn main() {
    let s = box_solid([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    let theory: f64 = 7.804696111;
    println!("FreeCAD 真值 {theory:.9}");
    for seg in [2usize, 4, 8, 16, 24, 32] {
        let out = fillet_solid(
            &s,
            &[],
            &FilletOptions {
                radius: 0.2,
                segments: seg,
            },
        )
        .expect("f");
        let (p, _, i) = out.triangulate();
        let v = archcore_kernel_brep::volume_from_mesh(&p, &i);
        println!(
            "seg={:<3} 面 {:<4} 边界 {:<3} 欧拉 {:<3} 体积 {:.9} 误差 {:.4}%",
            seg,
            out.faces.len(),
            out.boundary_half_edges(),
            out.euler_characteristic(),
            v,
            (v - theory).abs() / theory * 100.0
        );
    }
}
