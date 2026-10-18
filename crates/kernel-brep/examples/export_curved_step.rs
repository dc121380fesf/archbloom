use archcore_kernel_brep::{cylinder_solid, to_step_curved};

fn main() {
    // Φ10 × 20mm 圆柱（16 段，便于肉眼核对）
    let s = cylinder_solid([0.0, 0.0, 0.0], 0.005, 0.02, 16);
    let step = to_step_curved(&s, "cyl");
    std::fs::write("D:\\Word\\_cyl_curved.step", &step).expect("write");
    let cyl = step.matches("CYLINDRICAL_SURFACE").count();
    let circ = step.matches("CIRCLE(''").count();
    let trim = step.matches("TRIMMED_CURVE").count();
    let plane = step.matches("PLANE(''").count();
    let faces = step.matches("ADVANCED_FACE").count();
    println!("面 {faces} | CYLINDRICAL {cyl} | CIRCLE {circ} | TRIMMED {trim} | PLANE {plane}");
    println!("大小 {} 字节", step.len());
}
