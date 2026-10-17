//! 导出 STEP 样件，交给 FreeCAD 做外部验证。
//!
//! 用法：
//! ```text
//! cargo run -p archcore-kernel-brep --example export_step -- <输出目录>
//! ```

use archcore_kernel::{BoolOp, CancelToken, GeometryKernel};
use archcore_kernel_brep::BrepKernel;

fn dump(k: &BrepKernel, id: archcore_kernel::SolidId, dir: &std::path::Path, name: &str) {
    let text = k.to_step(id).expect("STEP 导出失败");
    let path = dir.join(format!("{name}.step"));
    std::fs::write(&path, &text).expect("写文件失败");
    let f = k.facts(id).expect("取事实失败");
    println!(
        "{:<20} 体积 {:>12.9}  面 {:>3}  水密 {}  → {} 字节",
        name,
        f.volume,
        f.face_count,
        f.watertight,
        text.len()
    );
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    std::fs::create_dir_all(&dir).expect("建目录失败");

    let mut k = BrepKernel::new();

    // 1. 盒子 1×2×3（米）
    let b = k.add_box([0.0, 0.0, 0.0], [1.0, 2.0, 3.0]);
    dump(&k, b, &dir, "step_box");

    // 2. 简单差集（切角）
    let a1 = k.add_box([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    let b1 = k.add_box([1.0, -1.0, -1.0], [3.0, 3.0, 3.0]);
    if let Ok(id) = k.boolean(a1, b1, BoolOp::Difference, &CancelToken::new(), &mut |_| {}) {
        dump(&k, id, &dir, "step_difference");
    }

    // 3. 开孔板（20×20×5mm，Φ8 通孔）
    let plate = k.add_box([-0.01, -0.01, 0.0], [0.01, 0.01, 0.005]);
    let hole = k.add_cylinder([0.0, 0.0, -0.001], 0.004, 0.007, 32);
    if let Ok(id) = k.boolean(plate, hole, BoolOp::Difference, &CancelToken::new(), &mut |_| {}) {
        dump(&k, id, &dir, "step_plate_hole");
    }

    // 4. 相贯双孔（K7 样件：两个 Φ6 孔中心距 5mm，真相交）
    let p2 = k.add_box([-0.01, -0.01, 0.0], [0.01, 0.01, 0.005]);
    let h1 = k.add_cylinder([-0.0025, 0.0, -0.001], 0.003, 0.007, 24);
    let h2 = k.add_cylinder([0.0025, 0.0, -0.001], 0.003, 0.007, 24);
    if let Ok(mid) = k.boolean(p2, h1, BoolOp::Difference, &CancelToken::new(), &mut |_| {}) {
        if let Ok(id) = k.boolean(mid, h2, BoolOp::Difference, &CancelToken::new(), &mut |_| {}) {
            dump(&k, id, &dir, "step_cross_holes");
        }
    }

    println!("\n输出目录：{}", dir.display());
}
