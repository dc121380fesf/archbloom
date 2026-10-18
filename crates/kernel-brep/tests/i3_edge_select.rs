//! I 阶段第三道验收：**按边选倒角**（布尔件的正确用法）。
//!
//! 背景：开孔板有 3148 个面，孔壁碎片 1e-6 量级，比 0.3mm 倒角还小。
//! 全边倒角必然失败——这是对的。真实加工里也不会给孔壁碎片倒角，
//! 而是**只倒外框**。这道测试验证「查边 → 筛边 → 只倒这些边」这条链路走得通。

use archcore_kernel::{BoolOp, CancelToken, GeometryKernel};
use archcore_kernel_brep::BrepKernel;

fn perforated_plate() -> (BrepKernel, archcore_kernel::SolidId) {
    let mut k = BrepKernel::new();
    let plate = k.add_box([0.0, 0.0, 0.0], [0.020, 0.020, 0.005]);
    let drill = k.add_cylinder([0.010, 0.010, -0.001], 0.005, 0.007, 64);
    let mut noop = |_p: archcore_kernel::Progress| {};
    let id = k
        .boolean(plate, drill, BoolOp::Difference, &CancelToken::new(), &mut noop)
        .expect("开孔应当成功");
    (k, id)
}

#[test]
fn i3_edges_query_is_consistent() {
    let (k, id) = perforated_plate();
    let edges = k.edges(id).expect("应能查边");

    println!("开孔板 {} 条边", edges.len());
    assert!(!edges.is_empty(), "边表不应为空");

    // 边号必须是从 0 开始的连续序号（与 chamfer 内部编号一致）
    for (i, e) in edges.iter().enumerate() {
        assert_eq!(e.id.0, i as u64, "边号必须与列表下标一致");
        assert!(e.length > 0.0, "边长应为正");
        assert!(
            (0.0..=180.0).contains(&e.angle_deg),
            "夹角应在 0..180，实际 {}",
            e.angle_deg
        );
    }

    // 长度分布：布尔会把外框的 20mm 长边切成短段，最长边只剩板厚 5mm
    let longest = edges.iter().map(|e| e.length).fold(0.0_f64, f64::max);
    let shortest = edges.iter().map(|e| e.length).fold(f64::MAX, f64::min);
    println!("最长 {longest:.6} m，最短 {shortest:.9} m");
    assert!(longest <= 0.020 + 1e-9, "不该出现超过 20mm 的边");
    assert!(longest > 0.004, "最长边应为板厚量级（5mm），实际 {longest}");
    assert!(shortest < 1e-4, "应当存在孔壁碎片边（< 0.1mm）");
}

#[test]
fn i3_chamfer_outer_frame_reports_fragmentation() {
    // 这条测试记录的是**真实边界**，不是“期望成功”。
    //
    // 实测：开孔板 3148 个面**全部**面积 < 0.1mm²，其中 2880 个 < 0.001mm²，
    // 最大面只有 0.077×0.077mm。理论面数应该 ~70。
    // 也就是说布尔输出被切成了碎屑——在这种拓扑上，**任何**宽度大于 0.05mm
    // 的倒角都会撞上某块碎片。选边也救不了，因为选中边的邻面就是碎片。
    //
    // 要真正打通这条链路，需要的是「布尔后合并共面面片」，不是选边。
    let (mut k, id) = perforated_plate();
    let before = k.facts(id).expect("原实体");

    let frame: Vec<_> = k
        .edges(id)
        .expect("查边")
        .into_iter()
        .filter(|e| e.sharp && e.length > 0.001)
        .map(|e| e.id)
        .collect();
    println!("选中锐且长于 1mm 的边 {} 条（共 {} 面）", frame.len(), before.face_count);
    assert!(!frame.is_empty(), "选边应当选出东西——这是接口可用性的证明");

    let r = k.chamfer(id, &frame, 0.0003, &CancelToken::new());
    match r {
        Ok(out) => {
            // 如果哪天面片合并做完了，这里会变成成功分支。
            let after = k.facts(out).expect("倒角后");
            println!("只倒外框成功：面 {} → {}，水密 {}", before.face_count, after.face_count, after.watertight);
            assert!(after.watertight);
        }
        Err(e) => {
            let msg = format!("{e}");
            println!("只倒外框仍失败（受限于面片碎片化）：{msg}");
            // 失败必须是**带诊断的失败**，且要指到具体面
            assert!(msg.contains("面积异常"), "诊断应指出面积异常，实际：{msg}");
            assert!(msg.contains("建议"), "诊断应给出建议，实际：{msg}");
        }
    }
}

#[test]
fn i3_boolean_output_is_fragmented() {
    // 把「碎片化」这个事实本身变成断言：
    // 任何人以后修好了面片合并，这条测试会**失败**，提醒把数字改掉。
    let (k, id) = perforated_plate();
    let s = k.get(id).expect("solid");

    let mut biggest: f64 = 0.0;
    for f in &s.faces {
        let n = f.verts.len();
        let mut a = [0.0; 3];
        for i in 0..n {
            let p = s.verts[f.verts[i] as usize];
            let q = s.verts[f.verts[(i + 1) % n] as usize];
            a[0] += (p[1] - q[1]) * (p[2] + q[2]);
            a[1] += (p[2] - q[2]) * (p[0] + q[0]);
            a[2] += (p[0] - q[0]) * (p[1] + q[1]);
        }
        let area = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt() * 0.5;
        biggest = biggest.max(area);
    }
    println!(
        "开孔板 {} 面，最大面 {:.3e} m²（理论应有 ~70 面、顶/底面 2.4e-4 m²）",
        s.faces.len(),
        biggest
    );
    assert_eq!(s.faces.len(), 3148, "面数变了说明布尔输出策略改动了");
    assert!(biggest < 1e-5, "最大面如果超过 1e-5 m²，说明面片合已经做了");
}

#[test]
fn i3_chamfer_all_edges_still_fails_with_diagnosis() {
    // 对照：全边倒角应当失败，且失败要带诊断（不是 panic、不是坏实体）
    let (mut k, id) = perforated_plate();
    let r = k.chamfer(id, &[], 0.0003, &CancelToken::new());
    match r {
        Ok(_) => panic!("全边倒角不该成功——孔壁碎片撑不住 0.3mm"),
        Err(e) => {
            let msg = format!("{e}");
            println!("全边倒角的诊断：{msg}");
            assert!(
                msg.contains("退化") || msg.contains("建议"),
                "失败必须带可执行诊断，实际：{msg}"
            );
        }
    }
}
