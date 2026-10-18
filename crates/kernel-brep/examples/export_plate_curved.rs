use archcore_kernel::{BoolOp, CancelToken, GeometryKernel};
use archcore_kernel_brep::{merge_coplanar, to_step, to_step_curved, BrepKernel};

fn main() {
    let mut k = BrepKernel::new();
    let plate = k.add_box([0.0, 0.0, 0.0], [0.020, 0.020, 0.005]);
    let drill = k.add_cylinder([0.010, 0.010, -0.001], 0.005, 0.007, 64);
    let mut noop = |_p: archcore_kernel::Progress| {};
    let id = k
        .boolean(plate, drill, BoolOp::Difference, &CancelToken::new(), &mut noop)
        .expect("bool");
    let s = k.get(id).expect("solid");
    let (m, stats) = merge_coplanar(s).expect("merge");
    println!(
        "合并后：面 {} → {}（带洞 {}），水密 {}",
        stats.faces_before, stats.faces_after, stats.holed_faces, m.is_watertight()
    );

    let stable = to_step(&m, "plate");
    let curved = to_step_curved(&m, "plate");
    println!(
        "稳定路径：{} 字节，面 {}",
        stable.len(),
        stable.matches("ADVANCED_FACE").count()
    );
    println!(
        "曲面路径：{} 字节，面 {}，CYLINDRICAL {}，CIRCLE {}",
        curved.len(),
        curved.matches("ADVANCED_FACE").count(),
        curved.matches("CYLINDRICAL_SURFACE").count(),
        curved.matches("CIRCLE(''").count()
    );
    std::fs::write("D:\\Word\\_plate_curved.step", &curved).expect("write");
}
