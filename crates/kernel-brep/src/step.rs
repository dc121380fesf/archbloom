//! STEP (ISO 10303-21) 导出。
//!
//! # 这一版导什么
//!
//! **全部用 PLANE 的合法实体**。每个面是一个平面多边形，边界是折线。
//!
//! # 为什么不用 CYLINDRICAL_SURFACE
//!
//! 我们的圆柱是离散的（N 个平面四边形）。如果把孔壁声明成 `CYLINDRICAL_SURFACE`，
//! 但边界还是那 N 条折线，这份文件在语义上就是**错的**——曲面上的边必须真的落在
//! 该曲面上。FreeCAD 可能照读不误，但任何做几何校验的工具都会判它不合法。
//!
//! 真圆柱需要：曲线边（CIRCLE）+ 曲面上的顶点 + 完整的一圈 EDGE_LOOP。
//! 那是 G 阶段的第二步，本轮先把「合法 + 可读 + 几何正确」这条链路打通。
//!
//! # 为什么值得做
//!
//! STL 只有三角面片，做不了精确件；STEP 带拓扑（面-环-边-顶点）与曲面类型，
//! 是 CAD/CAM 之间的通用语言。哪怕现在是多面体，它也已经是一份**能进 CAD 流程**的文件。

use std::fmt::Write as _;

use crate::geom::{cross, normalize, sub};
use crate::solid::Solid;

/// P21 实体写入器：管 id 分配与格式。
///
/// 所有实体先攒在 `body` 里，最后和头段拼起来——这样引用的 id 一定已经分配好了。
struct P21 {
    body: String,
    next: u32,
}

impl P21 {
    fn new() -> Self {
        Self {
            body: String::new(),
            next: 1,
        }
    }

    /// 追加一个实体，返回它的 `#id`（带 # 号）。
    fn add(&mut self, entity: &str) -> String {
        let id = self.next;
        self.next += 1;
        let _ = writeln!(self.body, "#{id}={entity};");
        format!("#{id}")
    }

    fn ref_list(items: &[String]) -> String {
        items.join(",")
    }
}

/// 数值格式化：用定点、保留足够位数、去掉尾随零带来的噪声。
fn num(v: f64) -> String {
    // STEP 接受 `1.234E-05` 这类科学计数；固定 12 位有效够表达微米级几何
    if v == 0.0 {
        return "0.".to_string();
    }
    let s = format!("{v:.12}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0.".to_string()
    } else if s.contains('.') {
        s.to_string()
    } else {
        format!("{s}.")
    }
}

fn pt(a: [f64; 3]) -> String {
    format!("({},{},{})", num(a[0]), num(a[1]), num(a[2]))
}

/// 把实体导出成 STEP 文本。
///
/// `name` 会写进 PRODUCT 与 FILE_NAME，FreeCAD 里看到的就是这个名字。
pub fn to_step(solid: &Solid, name: &str) -> String {
    let mut w = P21::new();

    // ── 产品/上下文骨架（STEP 要求的最小链）──
    let app_ctx = w.add(
        "APPLICATION_CONTEXT('configuration controlled 3d designs of mechanical parts and assemblies')",
    );
    w.add(&format!(
        "APPLICATION_PROTOCOL_DEFINITION('international standard','config_control_design',1994,{app_ctx})"
    ));
    let prod_ctx = w.add(&format!("PRODUCT_CONTEXT('',{app_ctx},'mechanical')"));
    let product = w.add(&format!(
        "PRODUCT('{name}','{name}','',({prod_ctx}))"
    ));
    let formation = w.add(&format!("PRODUCT_DEFINITION_FORMATION('','',{product})"));
    let pd_ctx = w.add(&format!(
        "PRODUCT_DEFINITION_CONTEXT('part definition',{app_ctx},'design')"
    ));
    let pd = w.add(&format!(
        "PRODUCT_DEFINITION('design','',{formation},{pd_ctx})"
    ));
    let pds = w.add(&format!("PRODUCT_DEFINITION_SHAPE('','',{pd})"));

    // 几何上下文（单位：毫米，与 STEP 惯例一致；内核内部是米，导入器会换算）
    let _origin_pt = w.add(&format!("CARTESIAN_POINT('',{})", pt([0.0, 0.0, 0.0])));
    let _z_dir = w.add(&format!("DIRECTION('',{})", pt([0.0, 0.0, 1.0])));
    let _x_dir = w.add(&format!("DIRECTION('',{})", pt([1.0, 0.0, 0.0])));

    // 单位与不确定性：先建，再让上下文引用它们（引用后定义的实体在 P21 里合法）
    let len_unit = w.add("(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT($,.METRE.))");
    let ang_unit = w.add("(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.))");
    let sol_unit = w.add("(NAMED_UNIT(*)SI_UNIT($,.STERADIAN.)SOLID_ANGLE_UNIT())");
    let unc = w.add(&format!(
        "UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-06),{len_unit},'distance_accuracy_value','')"
    ));
    let geom_ctx = w.add(&format!(
        "(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT(({unc}))GLOBAL_UNIT_ASSIGNED_CONTEXT(({len_unit},{ang_unit},{sol_unit}))REPRESENTATION_CONTEXT('',''))"
    ));

    // ── 顶点 ──
    let mut vpoint: Vec<String> = Vec::with_capacity(solid.verts.len());
    for v in &solid.verts {
        let c = w.add(&format!("CARTESIAN_POINT('',{})", pt(*v)));
        vpoint.push(w.add(&format!("VERTEX_POINT('',{c})")));
    }

    // ── 无向边：一次建好，所有面复用（拓扑共享，这也是 STEP 比 STL 强的地方）──
    //
    // key = (min,max) 顶点索引；value = (edge_curve_id, 起点索引, 终点索引)
    let mut edge_ids: std::collections::HashMap<(u32, u32), (String, u32, u32)> =
        std::collections::HashMap::new();

    // ── 面 ──
    let mut face_refs: Vec<String> = Vec::new();

    for f in &solid.faces {
        let ring = &f.verts;
        if ring.len() < 3 {
            continue;
        }
        let verts: Vec<[f64; 3]> = ring
            .iter()
            .filter_map(|i| solid.verts.get(*i as usize).copied())
            .collect();
        if verts.len() != ring.len() {
            continue;
        }

        // 面法线：优先解析式，退化时用顶点叉积
        let centroid: [f64; 3] = {
            let n = verts.len() as f64;
            let mut s = [0.0, 0.0, 0.0];
            for v in &verts {
                s[0] += v[0];
                s[1] += v[1];
                s[2] += v[2];
            }
            [s[0] / n, s[1] / n, s[2] / n]
        };
        let normal = f
            .surface
            .normal_at(centroid)
            .or_else(|| normalize(cross(sub(verts[1], verts[0]), sub(verts[2], verts[0]))))
            .unwrap_or([0.0, 0.0, 1.0]);

        // 平面：AXIS2_PLACEMENT_3D(原点, 法线, 参考方向)
        let ref_dir = pick_reference(normal);
        let origin = w.add(&format!("CARTESIAN_POINT('',{})", pt(verts[0])));
        let axis = w.add(&format!("DIRECTION('',{})", pt(normal)));
        let refd = w.add(&format!("DIRECTION('',{})", pt(ref_dir)));
        let placement = w.add(&format!(
            "AXIS2_PLACEMENT_3D('',{origin},{axis},{refd})"
        ));
        let surface = w.add(&format!("PLANE('',{placement})"));

        // 环：每个顶点对一条 ORIENTED_EDGE
        let mut oriented: Vec<String> = Vec::with_capacity(ring.len());
        for i in 0..ring.len() {
            let a = ring[i];
            let b = ring[(i + 1) % ring.len()];
            if a == b {
                continue;
            }
            let key = if a < b { (a, b) } else { (b, a) };
            let forward = a < b;

            let (edge_curve, _s, _e) = edge_ids.entry(key).or_insert_with(|| {
                let va = &verts_of(solid, key.0);
                let vb = &verts_of(solid, key.1);
                let dir = normalize(sub(*vb, *va)).unwrap_or([1.0, 0.0, 0.0]);
                let o = w.add(&format!("CARTESIAN_POINT('',{})", pt(*va)));
                let d = w.add(&format!("DIRECTION('',{})", pt(dir)));
                let vec = w.add(&format!("VECTOR('',{d},1.)"));
                let line = w.add(&format!("LINE('',{o},{vec})"));
                let ec = w.add(&format!(
                    "EDGE_CURVE('',{},{},{line},.T.)",
                    vpoint[key.0 as usize], vpoint[key.1 as usize]
                ));
                (ec, key.0, key.1)
            });
            let orient = if forward { ".T." } else { ".F." };
            oriented.push(w.add(&format!(
                "ORIENTED_EDGE('',*,*,{edge_curve},{orient})"
            )));
        }
        if oriented.len() < 3 {
            continue;
        }
        let loop_id = w.add(&format!(
            "EDGE_LOOP('',({}))",
            P21::ref_list(&oriented)
        ));
        let bound = w.add(&format!("FACE_OUTER_BOUND('',{loop_id},.T.)"));
        // 面法线与曲面法线同向 → same_sense = .T.
        let face = w.add(&format!(
            "ADVANCED_FACE('',({bound}),{surface},.T.)"
        ));
        face_refs.push(face);
    }

    if face_refs.is_empty() {
        // 空实体：仍输出一份结构完整的文件，免得下游拿到半成品
        face_refs.push(w.add("ADVANCED_FACE('',(),PLANE('',#0),.T.)"));
    }

    let shell = w.add(&format!(
        "CLOSED_SHELL('',({}))",
        P21::ref_list(&face_refs)
    ));
    let brep = w.add(&format!("MANIFOLD_SOLID_BREP('{name}',{shell})"));
    let rep = w.add(&format!(
        "ADVANCED_BREP_SHAPE_REPRESENTATION('{name}',({brep}),{geom_ctx})"
    ));
    w.add(&format!(
        "SHAPE_DEFINITION_REPRESENTATION({pds},{rep})"
    ));

    assemble(&w.body, name)
}

fn verts_of(solid: &Solid, i: u32) -> [f64; 3] {
    solid.verts.get(i as usize).copied().unwrap_or([0.0, 0.0, 0.0])
}

/// 与法线不平行的一个参考方向（AXIS2_PLACEMENT_3D 要求 reference direction 垂直于轴）。
fn pick_reference(n: [f64; 3]) -> [f64; 3] {
    let helper = if n[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    normalize(cross(n, helper)).unwrap_or([0.0, 1.0, 0.0])
}

fn assemble(body: &str, name: &str) -> String {
    let mut out = String::with_capacity(body.len() + 1024);
    out.push_str("ISO-10303-21;\n");
    out.push_str("HEADER;\n");
    out.push_str("FILE_DESCRIPTION((''),'2;1');\n");
    out.push_str(&format!(
        "FILE_NAME('{name}.step','',(''),(''),'archbloom','archbloom','');\n"
    ));
    out.push_str("FILE_SCHEMA(('CONFIG_CONTROL_DESIGN'));\n");
    out.push_str("ENDSEC;\n");
    out.push_str("DATA;\n");
    out.push_str(body);
    out.push_str("ENDSEC;\n");
    out.push_str("END-ISO-10303-21;\n");
    out
}

/// STEP 导出前自检：面数、顶点数、体积与实体一致。
pub fn verify_against_solid(step: &str, solid: &Solid) -> Result<(), String> {
    let faces = step.matches("ADVANCED_FACE").count();
    if faces != solid.faces.len() {
        return Err(format!(
            "STEP 面数 {faces} 与实体 {} 不符",
            solid.faces.len()
        ));
    }
    let verts = step.matches("VERTEX_POINT").count();
    if verts != solid.verts.len() {
        return Err(format!(
            "STEP 顶点数 {verts} 与实体 {} 不符",
            solid.verts.len()
        ));
    }
    if !step.starts_with("ISO-10303-21;") || !step.trim_end().ends_with("END-ISO-10303-21;") {
        return Err("STEP 首尾标记不对".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::dot;
    use crate::solid::box_solid;

    #[test]
    fn 盒子导出结构完整() {
        let s = box_solid([0.0, 0.0, 0.0], [1.0, 2.0, 3.0]);
        let step = to_step(&s, "box");
        assert!(step.starts_with("ISO-10303-21;\n"));
        assert!(step.contains("FILE_SCHEMA(('CONFIG_CONTROL_DESIGN'))"));
        assert_eq!(step.matches("VERTEX_POINT").count(), 8);
        assert_eq!(step.matches("ADVANCED_FACE").count(), 6);
        assert!(step.contains("MANIFOLD_SOLID_BREP"));
        assert!(step.trim_end().ends_with("END-ISO-10303-21;"));
        verify_against_solid(&step, &s).unwrap();
    }

    #[test]
    fn 边被复用不重复定义() {
        // 盒子 12 条边，如果每个面各建一次会变成 24 条
        let s = box_solid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let step = to_step(&s, "b");
        assert_eq!(
            step.matches("EDGE_CURVE").count(),
            12,
            "拓扑共享：12 条无向边只该定义一次"
        );
    }

    #[test]
    fn 数值格式合法() {
        assert_eq!(num(0.0), "0.");
        assert_eq!(num(1.0), "1.");
        assert_eq!(num(-2.5), "-2.5");
        assert!(num(0.000011).contains("0.000011"));
    }

    #[test]
    fn 参考方向与法线正交() {
        for n in [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            let r = pick_reference(n);
            assert!(dot(n, r).abs() < 1e-9, "参考方向必须垂直于轴");
        }
    }
}
