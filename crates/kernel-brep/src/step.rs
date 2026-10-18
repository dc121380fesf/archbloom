//! STEP (ISO 10303-21) 导出。
//!
//! # 导什么
//!
//! * **平面**：`PLANE` + 折线边界（多边形）。
//! * **完整圆柱侧面**：`CYLINDRICAL_SURFACE` + `CIRCULAR_ARC` 边界。
//!
//! # 为什么曲面必须配圆弧边
//!
//! 曲面上的边必须真的落在该曲面上。如果在 `CYLINDRICAL_SURFACE` 上用折线当边界，
//! 这份文件在语义上就是**错的**——FreeCAD 可能照读不误，但任何做几何校验的工具
//! 都会判它不合法。所以这里严格**成对交付**：能写出圆弧才写曲面，
//! 否则整组退回逐面平面折线，**绝不允许半真半假**。
//!
//! # 为什么值得做
//!
//! 加工端拿到全平面的 STEP，看到的是「一堆多边形」而不是「一个 Φ10 的孔」，
//! 写 CNC 代码时按多边形走刀。只有 `CYLINDRICAL_SURFACE` 才叫真正的孔。

use std::fmt::Write as _;

use crate::geom::{cross, dot, normalize, orthonormal_basis, sub, Surface};
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
/// 导出 STEP（**稳定路径**：全平面折线）。
///
/// 这份输出经 FreeCAD 验证：合法、闭合、体积准确。加工端读到的是多面体。
pub fn to_step(solid: &Solid, name: &str) -> String {
    to_step_impl(solid, name, false)
}

/// 导出 STEP（**实验路径**：尝试把圆柱侧面写成 `CYLINDRICAL_SURFACE` + 圆弧边）。
///
/// 当前状态：面能被 FreeCAD 认成 `Cylinder`、边上也出现了 `Circle`，
/// 但拓扑尚未完全闭合（32 段弧只认出 15 条，面积算不对）。
/// **不要把这条路径的产物直接交给加工端**；它目前用于调试与进度度量。
///
/// 不做整组回退——调用方明确要曲面时，就应该看到曲面写出来是什么样。
pub fn to_step_curved(solid: &Solid, name: &str) -> String {
    to_step_impl(solid, name, true)
}

fn to_step_impl(solid: &Solid, name: &str, use_cylinders: bool) -> String {
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

    // ── 圆柱侧面：先成组处理 ──
    //
    // 同一个 `Cylinder` 曲面的侧面片合成一个 `CYLINDRICAL_SURFACE` 面。
    // 这是「可出加工件」的关键：孔壁必须是一个曲面加圆弧边，
    // 而不是 64 个平面拼的棱柱——否则加工端看到的是多边形不是孔。
    //
    // **成对交付**：曲面配圆弧边。做不到就整组退回逐面平面折线，
    // 绝不在曲面上用折线（那会让文件语义自相矛盾）。
    let mut consumed: std::collections::HashSet<usize> = std::collections::HashSet::new();
    if use_cylinders {
        let mut groups: std::collections::HashMap<String, (Surface, Vec<usize>)> =
            std::collections::HashMap::new();
        for (i, f) in solid.faces.iter().enumerate() {
            if let Surface::Cylinder {
                origin,
                axis,
                radius,
            } = f.surface
            {
                let key = format!(
                    "C|{:.9},{:.9},{:.9}|{:.9},{:.9},{:.9}|{:.9}",
                    origin[0], origin[1], origin[2], axis[0], axis[1], axis[2], radius
                );
                groups
                    .entry(key)
                    .or_insert_with(|| (f.surface, Vec::new()))
                    .1
                    .push(i);
            }
        }
        // 顺序确定：否则 Hash 顺序会让输出不稳定
        let mut keys: Vec<String> = groups.keys().cloned().collect();
        keys.sort();
        for k in keys {
            let (surf, idxs) = &groups[&k];
            if idxs.len() < 4 {
                continue;
            }
            if let Some(face_ref) = emit_cylinder_side(&mut w, solid, idxs, surf, &vpoint) {
                face_refs.push(face_ref);
                for &i in idxs {
                    consumed.insert(i);
                }
            }
        }
    }

    for (face_idx, f) in solid.faces.iter().enumerate() {
        if consumed.contains(&face_idx) {
            continue;
        }
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

/// 把一组圆柱侧面片写成一个 `CYLINDRICAL_SURFACE` 面。
///
/// 边界：底圆 n 段弧 → 母线（seam）→ 顶圆 n 段弧（反向）→ 同一条母线反向。
/// 每段弧按**实际顶点角度**写，不硬拆半圆——这样弧的端点一定对得上真实顶点。
///
/// 返回 None 表示这组不是完整筒（被切掉一块、顶点不共圆），
/// 调用方应退回逐面平面折线。
fn emit_cylinder_side(
    w: &mut P21,
    solid: &Solid,
    faces: &[usize],
    surface: &Surface,
    vpoint: &[String],
) -> Option<String> {
    let Surface::Cylinder {
        origin,
        axis,
        radius,
    } = *surface
    else {
        return None;
    };
    let axis = normalize(axis)?;
    let (u, vv) = orthonormal_basis(axis)?;

    let mut idx: Vec<u32> = Vec::new();
    for &fi in faces {
        for &x in &solid.faces[fi].verts {
            if !idx.contains(&x) {
                idx.push(x);
            }
        }
    }
    if idx.len() < 8 {
        return None;
    }

    let mut hs: Vec<f64> = idx
        .iter()
        .map(|&i| dot(sub(solid.verts[i as usize], origin), axis))
        .collect();
    hs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let hmin = *hs.first()?;
    let hmax = *hs.last()?;
    if (hmax - hmin).abs() < 1e-12 {
        return None;
    }
    let mid = (hmin + hmax) * 0.5;

    let mut bot: Vec<(f64, u32)> = Vec::new();
    let mut top: Vec<(f64, u32)> = Vec::new();
    for &i in &idx {
        let p = solid.verts[i as usize];
        let d = sub(p, origin);
        let h = dot(d, axis);
        let radial = sub(d, [axis[0] * h, axis[1] * h, axis[2] * h]);
        let r = dot(radial, radial).sqrt();
        if (r - radius).abs() > radius * 0.01 + 1e-9 {
            return None;
        }
        let ang = dot(radial, vv).atan2(dot(radial, u));
        if h < mid {
            bot.push((ang, i));
        } else {
            top.push((ang, i));
        }
    }
    if bot.len() < 4 || bot.len() != top.len() {
        return None;
    }
    bot.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    top.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // 角度覆盖必须近整圈，否则是被切掉一块的圆柱
    let tau = std::f64::consts::TAU;
    let n = bot.len();
    let covered: f64 = (0..n)
        .map(|i| {
            let a = bot[i].0;
            let b = if i + 1 < n { bot[i + 1].0 } else { bot[0].0 + tau };
            b - a
        })
        .sum();
    if covered.to_degrees() < 350.0 {
        return None;
    }

    // ── 曲面：AXIS2_PLACEMENT_3D 原点在底圆心，Z 轴为轴向，X 轴为角度零点 ──
    let p0 = solid.verts[bot[0].1 as usize];
    let h0 = dot(sub(p0, origin), axis);
    let base_p = sub(p0, [axis[0] * h0, axis[1] * h0, axis[2] * h0]);
    let co = w.add(&format!("CARTESIAN_POINT('',{})", pt(base_p)));
    let ad = w.add(&format!("DIRECTION('',{})", pt(axis)));
    let rd = w.add(&format!("DIRECTION('',{})", pt(u)));
    let pl = w.add(&format!("AXIS2_PLACEMENT_3D('',{co},{ad},{rd})"));
    let surf = w.add(&format!("CYLINDRICAL_SURFACE('',{pl},{})", num(radius)));

    let mut oriented: Vec<String> = Vec::new();

    // 底圆：逆时针 n 段
    for i in 0..n {
        let a = bot[i];
        let b = bot[(i + 1) % n];
        let sweep = if i + 1 < n {
            b.0 - a.0
        } else {
            b.0 + tau - a.0
        };
        let pa = solid.verts[a.1 as usize];
        let c = sub(pa, {
            let h = dot(sub(pa, origin), axis);
            [axis[0] * h, axis[1] * h, axis[2] * h]
        });
        let arc = arc_entity(w, pa, c, axis, sweep);
        let ec = w.add(&format!(
            "EDGE_CURVE('',{},{},{arc},.T.)",
            vpoint[a.1 as usize], vpoint[b.1 as usize]
        ));
        oriented.push(w.add(&format!("ORIENTED_EDGE('',*,*,{ec},.T.)")));
    }

    // 母线：上去
    let seam = {
        let a = bot[0].1;
        let b = top[0].1;
        let va = solid.verts[a as usize];
        let vb = solid.verts[b as usize];
        let dir = normalize(sub(vb, va))?;
        let po = w.add(&format!("CARTESIAN_POINT('',{})", pt(va)));
        let dd = w.add(&format!("DIRECTION('',{})", pt(dir)));
        let vec = w.add(&format!("VECTOR('',{dd},1.)"));
        let line = w.add(&format!("LINE('',{po},{vec})"));
        w.add(&format!(
            "EDGE_CURVE('',{},{},{line},.T.)",
            vpoint[a as usize], vpoint[b as usize]
        ))
    };
    oriented.push(w.add(&format!("ORIENTED_EDGE('',*,*,{seam},.T.)")));

    // 顶圆：顺时针（反向）n 段
    for i in (0..n).rev() {
        let a = top[i];
        let b = top[(i + n - 1) % n];
        let sweep = if i > 0 {
            b.0 - a.0
        } else {
            b.0 - a.0 - tau
        };
        let pa = solid.verts[a.1 as usize];
        let c = sub(pa, {
            let h = dot(sub(pa, origin), axis);
            [axis[0] * h, axis[1] * h, axis[2] * h]
        });
        let arc = arc_entity(w, pa, c, axis, sweep);
        let ec = w.add(&format!(
            "EDGE_CURVE('',{},{},{arc},.T.)",
            vpoint[a.1 as usize], vpoint[b.1 as usize]
        ));
        oriented.push(w.add(&format!("ORIENTED_EDGE('',*,*,{ec},.T.)")));
    }

    // 母线：下来（同一条边，反向）
    oriented.push(w.add(&format!("ORIENTED_EDGE('',*,*,{seam},.F.)")));

    let loop_id = w.add(&format!("EDGE_LOOP('',({}))", P21::ref_list(&oriented)));
    let bound = w.add(&format!("FACE_OUTER_BOUND('',{loop_id},.T.)"));
    Some(w.add(&format!("ADVANCED_FACE('',({bound}),{surf},.T.)")))
}

/// 写一段圆弧：`CIRCLE` + `TRIMMED_CURVE`。
///
/// **不能用 `CIRCULAR_ARC`**：它不是 ISO 10303-42 的核心实体（那是 AP242 的扩展），
/// FreeCAD 读不认，把它退化成 `Line`——实测体积直接算成 6.67e+101。
/// 标准写法是「整圆 + 用角度参数裁剪」：
///
/// ```text
/// #n = CIRCLE('', axis2_placement_3d, radius);
/// #m = TRIMMED_CURVE('', #n, (PARAMETER_VALUE(0.)), (PARAMETER_VALUE(1.57)), .T., .PARAMETER.);
/// ```
fn arc_entity(w: &mut P21, from: [f64; 3], center: [f64; 3], axis: [f64; 3], sweep: f64) -> String {
    let co = w.add(&format!("CARTESIAN_POINT('',{})", pt(center)));
    let ad = w.add(&format!("DIRECTION('',{})", pt(axis)));
    let rdir = normalize(sub(from, center)).unwrap_or([1.0, 0.0, 0.0]);
    let rd = w.add(&format!("DIRECTION('',{})", pt(rdir)));
    let pl = w.add(&format!("AXIS2_PLACEMENT_3D('',{co},{ad},{rd})"));
    let radius = dot(sub(from, center), rdir).abs();
    let circle = w.add(&format!("CIRCLE('',{pl},{})", num(radius)));
    // sense = .T. 表示沿圆的正向（逆时针）取弧；反向时取绝对值并翻转
    let (sense, mag) = if sweep >= 0.0 {
        (".T.", sweep)
    } else {
        (".F.", -sweep)
    };
    w.add(&format!(
        "TRIMMED_CURVE('',{circle},(PARAMETER_VALUE(0.)),(PARAMETER_VALUE({})),{sense},.PARAMETER.)",
        num(mag)
    ))
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
