//! 导出一个 K4 验收用的 STL，交给 FreeCAD 做外部验证。
//!
//! 用法：
//! ```text
//! cargo run -p archcore-kernel-brep --example export_k4_stl -- <输出路径>
//! ```

use archcore_kernel::{BoolOp, CancelToken, GeometryKernel};
use archcore_kernel_brep::{to_stl_ascii, verify_against_solid, BrepKernel};

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "k4_union.stl".to_string());

    let mut k = BrepKernel::new();
    // 两个重叠盒子做并集：结果应是阶梯体，体积 1.875
    let a = k.add_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let b = k.add_box([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]);
    let id = k
        .boolean(a, b, BoolOp::Union, &CancelToken::new(), &mut |_| {})
        .expect("并集失败");

    let solid = k.get(id).expect("实体不存在");
    let f = k.facts(id).expect("取事实失败");
    let stl = to_stl_ascii(solid, "k4_union");
    verify_against_solid(&stl, solid).expect("STL 自检失败");

    std::fs::write(&out, &stl).expect("写出 STL 失败");
    println!(
        "STL → {out}\n体积 {:.9} m³ / 面 {} / 水密 {}",
        f.volume, f.face_count, f.watertight
    );
}
