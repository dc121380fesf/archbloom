use archcore_kernel_brep::{box_solid, chamfer_solid, to_stl_ascii, ChamferOptions};

fn main() {
    let s = box_solid([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    let out = chamfer_solid(
        &s,
        &[],
        &ChamferOptions {
            width: 0.2,
            angle_threshold_deg: 30.0,
        },
    )
    .expect("chamfer");
    std::fs::write("D:\\Word\\_chamfer_box.stl", to_stl_ascii(&out, "chamfer_box")).expect("write");
    let (pos, _, idx) = out.triangulate();
    println!(
        "三角化体积 {:.9}  面数 {}  水密 {}  欧拉 {}",
        archcore_kernel_brep::volume_from_mesh(&pos, &idx),
        out.faces.len(),
        out.is_watertight(),
        out.euler_characteristic()
    );
}
