use archcore_kernel::{BoolOp, CancelToken, GeometryKernel};
use archcore_kernel_brep::{merge_coplanar, solid_to_polygons, BrepKernel};

fn main() {
    // 做一个孔，合并，然后看：
    //   ① 带洞面进 solid_to_polygons 时洞还在不在
    //   ② 切分（split_all）之后洞还在不在
    let mut k = BrepKernel::new();
    let p = k.add_box([0.0, 0.0, 0.0], [0.020, 0.020, 0.005]);
    let d = k.add_cylinder([0.010, 0.010, -0.001], 0.005, 0.007, 64);
    let mut noop = |_p: archcore_kernel::Progress| {};
    let id = k
        .boolean(p, d, BoolOp::Difference, &CancelToken::new(), &mut noop)
        .expect("b");
    let s = k.get(id).expect("s");
    let (m, _) = merge_coplanar(s).expect("merge");

    let polys = solid_to_polygons(&m);
    let in_holes = polys.iter().filter(|p| !p.holes.is_empty()).count();
    let in_hole_pts: usize = polys.iter().map(|p| p.holes.iter().map(|h| h.len()).sum::<usize>()).sum();
    println!("① 转换后：{} 个 Polygon，带洞 {} 个，洞的顶点总数 {}", polys.len(), in_holes, in_hole_pts);

    // ② 切分：拿一个刀体切它（模拟第二次布尔的第一步）
    let cutter = k.add_box([0.035, 0.030, 0.005], [0.065, 0.050, 0.012]);
    let _ = cutter;
    // csg::split_all 需要两个 Polygon 集；这里用另一个盒子
    let cutter_solid = archcore_kernel_brep::box_solid([0.005, 0.005, -0.001], [0.015, 0.015, 0.006]);
    let mut planes = Vec::new();
    for f in &cutter_solid.faces {
        if let Some(pl) = archcore_kernel_brep::Plane::from_polygon(&f.verts.iter().map(|i| cutter_solid.verts[*i as usize]).collect::<Vec<_>>()) {
            planes.push(pl);
        }
    }
    let split = archcore_kernel_brep::split_all(polys.clone(), &planes);
    let out_holes = split.iter().filter(|p| !p.holes.is_empty()).count();
    let out_hole_pts: usize = split.iter().map(|p| p.holes.iter().map(|h| h.len()).sum::<usize>()).sum();
    println!("② 切分后：{} 个 Polygon，带洞 {} 个，洞的顶点总数 {}", split.len(), out_holes, out_hole_pts);
    println!();
    if in_holes > 0 && out_holes == 0 {
        println!("结论：洞在 split_all 里全丢了（输入 {} 个带洞面，输出 0 个）", in_holes);
    } else {
        println!("结论：洞在切分里保留了 {} 个（输入 {} 个）", out_holes, in_holes);
    }
}
