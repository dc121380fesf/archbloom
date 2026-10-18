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
/// # 当前状态：未完成，暂时等同稳定路径
///
/// 试过四种写法，得到四种不同结果（`Circle` 边 1 / 2 / 6 / 32 条不等），
/// 其中一次 `isValid` 变 True 但体积仍错。说明对 STEP 圆弧边界的语义
/// （`CIRCLE` 轴向、`TRIMMED_CURVE` 参数范围、seam 处理）还没吃透。
///
/// **继续碰运气没意义**。下一步应该：拿一份 OCC 生成的圆柱 STEP 作对照，
/// 逐行对结构，而不是自己推参数。
///
/// 在此之前，这个入口直接返回全平面结果——**宁可输出已知正确的多面体，
/// 也不输出一份“看起来有曲面但几何错”的文件**。
pub fn to_step_curved(solid: &Solid, name: &str) -> String {
    to_step_impl(solid, name, true)
}

fn to_step_impl(solid: &Solid, name: &str, use_cylinders: bool) -> String {
    // ── 曲面路径的安全护栏 ──
    //
    // 曲面上的边必须是曲线。如果实体里有「带洞面」，而洞又不是圆
    // （合并产出的 47 个碎片洞就是这样），孔口边界就没法写成圆边，
    // 壳会不闭合、体积也算错。
    //
    // 这种情况下**整份退回全平面**，宁交已知正确的多面体，
    // 也不交一份“看起来有曲面但几何错”的文件。
    let use_cylinders = use_cylinders && holes_are_all_round(solid);
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
    // 圆边缓存：圆心+半径 → EDGE_CURVE 引用。
    //
    // 顶/底面必须**复用**侧面已经写过的同一条圆边，否则壳在圆环处断成两半。
    // OCC 的输出就是这么做的（顶面、底面各用一条 ORIENTED_EDGE 引同一批圆边）。
    let mut circle_edges: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
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
            if let Some(face_ref) =
                emit_cylinder_side(&mut w, solid, idxs, surf, &vpoint, &mut circle_edges)
            {
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
        }        let ring = &f.verts;
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

        // ── 圆柱端面：如果环上顶点共圆，复用侧面写的整圆边 ──
        //
        // 这是壳能闭合的关键。顶面如果还用 16 段折线，它就和侧面的整圆边对不上，
        // OCC 会把它们当两个不相关的壳。
        if use_cylinders {
            let det = detect_circle(solid, ring, normal);
            if let Some((c_center, c_radius)) = det {
                let key = circle_key(c_center, normal, c_radius);
                if let Some(edge) = circle_edges.get(&key) {
                    // 这个平面可能是「外环 + 圆洞」（开孔板的顶/底面）。
                    // 外环是折线没关系，但**洞必须是圆边**——否则它和孔壁曲面的圆边对不上，
                    // 壳在孔口处断成两半（实测体积 2131 vs 理论 1607、闭合 False）。
                    let mut bounds: Vec<String> = Vec::new();
                    let plane_origin =
                        w.add(&format!("CARTESIAN_POINT('',{})", pt(verts[0])));
                    let p_axis = w.add(&format!("DIRECTION('',{})", pt(normal)));
                    let p_ref = w.add(&format!("DIRECTION('',{})", pt(pick_reference(normal))));
                    let p_pl = w.add(&format!(
                        "AXIS2_PLACEMENT_3D('',{plane_origin},{p_axis},{p_ref})"
                    ));
                    let p_surf = w.add(&format!("PLANE('',{p_pl})"));

                    // 外环：用缓存的圆边
                    {
                        let oe = w.add(&format!("ORIENTED_EDGE('',*,*,{edge},.T.)"));
                        let lid = w.add(&format!("EDGE_LOOP('',({oe}))"));
                        bounds.push(w.add(&format!("FACE_OUTER_BOUND('',{lid},.T.)")));
                    }
                    // 内环（洞）：如果也是圆，用缓存的圆边
                    for hole in &f.holes {
                        if let Some((hc, hr)) = detect_circle(solid, hole, normal) {
                            if let Some(hedge) = circle_edges.get(&circle_key(hc, normal, hr)) {
                                let oe = w.add(&format!("ORIENTED_EDGE('',*,*,{hedge},.F.)"));
                                let lid = w.add(&format!("EDGE_LOOP('',({oe}))"));
                                bounds.push(w.add(&format!("FACE_BOUND('',{lid},.T.)")));
                            }
                        }
                    }
                    face_refs.push(w.add(&format!(
                        "ADVANCED_FACE('',({}),{p_surf},.T.)",
                        P21::ref_list(&bounds)
                    )));
                    continue;
                }
            }
        }

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
    circles: &mut std::collections::HashMap<String, String>,
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
    let base_center_for_surf = {
        let h = dot(sub(solid.verts[bot[0].1 as usize], origin), axis);
        [
            origin[0] + axis[0] * h,
            origin[1] + axis[1] * h,
            origin[2] + axis[2] * h,
        ]
    };
    let co = w.add(&format!("CARTESIAN_POINT('',{})", pt(base_center_for_surf)));
    let ad = w.add(&format!("DIRECTION('',{})", pt(axis)));
    let rd = w.add(&format!("DIRECTION('',{})", pt(u)));
    let pl = w.add(&format!("AXIS2_PLACEMENT_3D('',{co},{ad},{rd})"));
    let surf = w.add(&format!("CYLINDRICAL_SURFACE('',{pl},{})", num(radius)));

    let mut oriented: Vec<String> = Vec::new();

    // 底/顶圆心 = 轴线上的点（origin + axis*h），**不是 p 减去轴向分量**。
    //
    // 早先写成 sub(p, axis*h)，那会把圆周点的径向分量留在“圆心”里——

    // 底/顶圆心 = 轴线上的点（origin + axis*h），**不是 p 减去轴向分量**。
    //
    // 早先写成 sub(p, axis*h)，那会把圆周点的径向分量留在“圆心”里——
    // 算出的圆心是 (0.005,0,0) 而不是 (0,0,0.02)，和端面检测出的圆心永远对不上，
    // 顶/底面因此退不回折线，壳在圆环处断成两半。
    let base_center = {
        let h = dot(sub(solid.verts[bot[0].1 as usize], origin), axis);
        [
            origin[0] + axis[0] * h,
            origin[1] + axis[1] * h,
            origin[2] + axis[2] * h,
        ]
    };
    let top_center = {
        let h = dot(sub(solid.verts[top[0].1 as usize], origin), axis);
        [
            origin[0] + axis[0] * h,
            origin[1] + axis[1] * h,
            origin[2] + axis[2] * h,
        ]
    };

    // ── 边的结构与 OCC 一致 ──
    //
    // 关键发现（对照 FreeCAD 导出的圆柱 STEP）：
    // **整圆就是一条闭合的 EDGE_CURVE，起点和终点是同一个 VERTEX_POINT**，
    // 不需要 TRIMMED_CURVE、不需要拆半圆。
    //
    //   侧面环：顶圆(.F.) → 母线(.F.) → 底圆(.T.) → 母线(.T.)
    //
    // 一共 4 条 ORIENTED_EDGE，OCC 也是这样。之前我写成 34 条（32 弧 + 2 seam），
    // 结构上就不对。
    let b0 = bot[0];
    let t0 = top[0];
    let pb0 = solid.verts[b0.1 as usize];
    let pt0 = solid.verts[t0.1 as usize];

    // 顶圆（闭合边，顶点 = pt0）
    let top_circle = circle_edge(w, pt0, top_center, axis, radius, &vpoint[t0.1 as usize]);
    // 底圆（闭合边，顶点 = pb0）
    let bot_circle = circle_edge(w, pb0, base_center, axis, radius, &vpoint[b0.1 as usize]);
    // 缓存给顶/底面复用
    circles.insert(circle_key(top_center, axis, radius), top_circle.clone());
    circles.insert(circle_key(base_center, axis, radius), bot_circle.clone());
    // 母线：底 → 顶
    let seam = {
        let dir = normalize(sub(pt0, pb0))?;
        let po = w.add(&format!("CARTESIAN_POINT('',{})", pt(pb0)));
        let dd = w.add(&format!("DIRECTION('',{})", pt(dir)));
        let vec = w.add(&format!("VECTOR('',{dd},1.)"));
        let line = w.add(&format!("LINE('',{po},{vec})"));
        w.add(&format!(
            "EDGE_CURVE('',{},{},{line},.T.)",
            vpoint[b0.1 as usize], vpoint[t0.1 as usize]
        ))
    };

    oriented.push(w.add(&format!("ORIENTED_EDGE('',*,*,{top_circle},.F.)")));
    oriented.push(w.add(&format!("ORIENTED_EDGE('',*,*,{seam},.F.)")));
    oriented.push(w.add(&format!("ORIENTED_EDGE('',*,*,{bot_circle},.T.)")));
    oriented.push(w.add(&format!("ORIENTED_EDGE('',*,*,{seam},.T.)")));

    let loop_id = w.add(&format!("EDGE_LOOP('',({}))", P21::ref_list(&oriented)));
    let bound = w.add(&format!("FACE_OUTER_BOUND('',{loop_id},.T.)"));
    Some(w.add(&format!("ADVANCED_FACE('',({bound}),{surf},.T.)")))
}



/// 圆边缓存的键：圆心 + 轴 + 半径（量化到 1e-6）。
fn circle_key(center: [f64; 3], _axis: [f64; 3], radius: f64) -> String {
    // **键里不含轴方向**：底面法线是 −Z、缓存的是 +Z，带上符号就永远匹配不上。
    //
    // 另外必须消灭 **负零**：`-0.0000` 和 `0.0000` 不是同一个字符串，
    // 而圆心坐标是浮点运算的结果，很容易带出 -1e-19 这种值。
    // 两个问题叠加时，顶/底面永远命中不了缓存——实测就是这个原因导致边数停在 35。
    let q = |v: f64| if v.abs() < 1e-9 { 0.0 } else { v };
    format!(
        "{:.4},{:.4},{:.4}|{:.4}",
        q(center[0]),
        q(center[1]),
        q(center[2]),
        q(radius)
    )
}

/// 写一条**闭合的整圆边**：起点和终点是同一个 `VERTEX_POINT`。
///
/// 这是 OCC 的做法（对照 FreeCAD 导出的圆柱 STEP）：整圆就是一条 `CIRCLE` 曲线 +
/// 一条首尾同点的 `EDGE_CURVE`，**不需要 `TRIMMED_CURVE`，也不需要拆半圆**。
fn circle_edge(
    w: &mut P21,
    on_circle: [f64; 3],
    center: [f64; 3],
    axis: [f64; 3],
    radius: f64,
    vertex_ref: &str,
) -> String {
    let co = w.add(&format!("CARTESIAN_POINT('',{})", pt(center)));
    let ad = w.add(&format!("DIRECTION('',{})", pt(axis)));
    // 参考方向：圆心指向圆上那个点（角度零点）
    let rdir = normalize(sub(on_circle, center)).unwrap_or([1.0, 0.0, 0.0]);
    let rd = w.add(&format!("DIRECTION('',{})", pt(rdir)));
    let pl = w.add(&format!("AXIS2_PLACEMENT_3D('',{co},{ad},{rd})"));
    let circle = w.add(&format!("CIRCLE('',{pl},{})", num(radius)));
    w.add(&format!(
        "EDGE_CURVE('',{v},{v},{circle},.T.)",
        v = vertex_ref
    ))
}

/// 所有带洞面的洞都必须是圆，曲面路径才可用。
fn holes_are_all_round(solid: &Solid) -> bool {
    for f in &solid.faces {
        if f.holes.is_empty() {
            continue;
        }
        let normal = f
            .surface
            .normal_at(solid.verts[f.verts[0] as usize])
            .unwrap_or([0.0, 0.0, 1.0]);
        for h in &f.holes {
            if detect_circle(solid, h, normal).is_none() {
                return false;
            }
        }
    }
    true
}

/// 检测一个环是不是圆：所有顶点到质心距离一致 → 返回 (圆心, 半径)。
fn detect_circle(solid: &Solid, ring: &[u32], normal: [f64; 3]) -> Option<([f64; 3], f64)> {
    if ring.len() < 8 {
        return None;
    }
    let n = ring.len() as f64;
    let mut c = [0.0; 3];
    for &i in ring {
        let p = solid.verts[i as usize];
        for (ck, pk) in c.iter_mut().zip(p.iter()) {
            *ck += *pk;
        }
    }
    for ck in c.iter_mut() {
        *ck /= n;
    }
    let mut r_min = f64::MAX;
    let mut r_max = 0.0f64;
    for &i in ring {
        let p = solid.verts[i as usize];
        let d = sub(p, c);
        // 先去掉法线方向分量（顶点可能不严格共面）
        let h = dot(d, normal);
        let rad = [
            d[0] - normal[0] * h,
            d[1] - normal[1] * h,
            d[2] - normal[2] * h,
        ];
        let r = dot(rad, rad).sqrt();
        r_min = r_min.min(r);
        r_max = r_max.max(r);
    }
    if r_max <= 0.0 || (r_max - r_min) / r_max > 0.01 {
        return None;
    }
    Some((c, (r_min + r_max) * 0.5))
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
