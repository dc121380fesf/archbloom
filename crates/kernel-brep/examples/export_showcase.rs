//! 导出一套 STL 样件，供人工检视（FreeCAD / Blender 直接打开）。
//!
//! 用法：
//! ```text
//! cargo run -p archcore-kernel-brep --example export_showcase -- <输出目录>
//! ```

use archcore_kernel::{BoolOp, CancelToken, GeometryKernel, SolidId};
use archcore_kernel_brep::{to_stl_ascii, verify_against_solid, BrepKernel};

fn write(k: &BrepKernel, id: SolidId, dir: &std::path::Path, name: &str) {
    let solid = k.get(id).expect("实体不存在");
    let stl = to_stl_ascii(solid, name);
    verify_against_solid(&stl, solid).expect("STL 自检失败");
    let path = dir.join(format!("{name}.stl"));
    std::fs::write(&path, &stl).expect("写 STL 失败");
    let f = k.facts(id).expect("取事实失败");
    println!(
        "{:<22} 体积 {:>12.9} m³  面 {:>3}  水密 {}",
        name, f.volume, f.face_count, f.watertight
    );
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    std::fs::create_dir_all(&dir).expect("建目录失败");

    let mut k = BrepKernel::new();

    // 1. 盒子 2×1×1
    let box_id = k.add_box([0.0, 0.0, 0.0], [2.0, 1.0, 1.0]);
    write(&k, box_id, &dir, "01_box");

    // 2. Φ10 × 20mm 圆柱（毫米级尺寸，换算成米）
    let cyl_id = k.add_cylinder([0.0, 0.0, 0.0], 0.005, 0.020, 64);
    write(&k, cyl_id, &dir, "02_cylinder_phi10");

    // 3. 重叠并集：两个 1×1×1 盒，结果应是阶梯体（体积 1.875）
    let a = k.add_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let b = k.add_box([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]);
    if let Ok(id) = k.boolean(a, b, BoolOp::Union, &CancelToken::new(), &mut |_| {}) {
        write(&k, id, &dir, "03_union_step");
    }

    // 4. 差集：2×2×2 被切掉 x>1 的一半（体积 4.0）
    let c = k.add_box([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    let d = k.add_box([1.0, -1.0, -1.0], [3.0, 3.0, 3.0]);
    if let Ok(id) = k.boolean(c, d, BoolOp::Difference, &CancelToken::new(), &mut |_| {}) {
        write(&k, id, &dir, "04_difference_cut");
    }

    // 5. 交集：重叠区（体积 1.0）
    let e = k.add_box([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    let g = k.add_box([1.0, 1.0, 1.0], [3.0, 3.0, 3.0]);
    if let Ok(id) = k.boolean(e, g, BoolOp::Intersection, &CancelToken::new(), &mut |_| {}) {
        write(&k, id, &dir, "05_intersection");
    }

    // 6. 开孔板：20×20×5mm 板 + 4 个 Φ6 孔（K5 样件）
    let plate = k.add_box([-0.01, -0.01, 0.0], [0.01, 0.01, 0.005]);
    let mut cur = plate;
    let mut ok = true;
    for (x, y) in [(-0.006, -0.006), (0.006, -0.006), (-0.006, 0.006), (0.006, 0.006)] {
        let hole = k.add_cylinder([x, y, -0.001], 0.003, 0.007, 64);
        match k.boolean(cur, hole, BoolOp::Difference, &CancelToken::new(), &mut |_| {}) {
            Ok(id) => cur = id,
            Err(_) => {
                ok = false;
                break;
            }
        }
    }
    if ok {
        write(&k, cur, &dir, "06_plate_4holes");
    }

    println!("\n输出目录：{}", dir.display());
}
