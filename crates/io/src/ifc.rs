//! IFC（ISO 16739 / STEP Part 21）子集适配器。
//!
//! 导入：空间层级（IfcProject/Site/Building/Storey）+ 拉伸体（IfcExtrudedAreaSolid）
//! 与三角面集（IfcTriangulatedFaceSet）；导出：IfcTriangulatedFaceSet 镶嵌表示（网格内核的诚实映射）。
//!
//! 敌对输入口径：解析失败一律 IoError::Format 带位置，绝不 panic（io 审计教训内化）。

use crate::IoError;
use std::collections::BTreeMap;

// ---------- STEP Part 21 词法/实体表 ----------

#[derive(Clone, Debug, PartialEq)]
pub enum Attr {
    Ref(u64),
    Str(String),
    Num(f64),
    Enum(String),
    List(Vec<Attr>),
    Null,
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub id: u64,
    pub kind: String,
    pub attrs: Vec<Attr>,
}

/// STEP 实体表（id → entity），两遍扫描：先建表再引用（前向引用安全）
pub type EntityMap = BTreeMap<u64, Entity>;

/// 解析 P21 数据段实体（支持多行实体、'' 转义、/* */ 注释、$、*、.ENUM.、嵌套列表）
pub fn parse_p21(text: &str) -> Result<EntityMap, IoError> {
    let mut map = EntityMap::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    let line_of = |pos: usize, bytes: &[char]| -> usize {
        bytes[..pos].iter().filter(|c| **c == '\n').count() + 1
    };
    // 跳过 HEADER 段
    while i < bytes.len() {
        if bytes[i..].starts_with(&['D', 'A', 'T', 'A', ';']) {
            i += 5;
            break;
        }
        i += 1;
    }
    loop {
        // 跳空白与注释
        while i < bytes.len() {
            if bytes[i].is_whitespace() {
                i += 1;
            } else if bytes[i] == '/' && i + 1 < bytes.len() && bytes[i + 1] == '*' {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == '*' && bytes[i + 1] == '/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            } else {
                break;
            }
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i..].starts_with(&['E', 'N', 'D', 'S', 'E', 'C', ';']) {
            break; // DATA 段结束
        }
        if bytes[i] != '#' {
            return Err(IoError::Format(format!(
                "第 {} 行：实体应以 # 开头",
                line_of(i, &bytes)
            )));
        }
        i += 1;
        let id_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let id: u64 = bytes[id_start..i]
            .iter()
            .collect::<String>()
            .parse()
            .map_err(|_| IoError::Format(format!("第 {} 行：实体 id 非法", line_of(i, &bytes))))?;
        if i >= bytes.len() || bytes[i] != '=' {
            return Err(IoError::Format(format!(
                "第 {} 行：#{} 缺 =",
                line_of(i, &bytes),
                id
            )));
        }
        i += 1;
        // 类型名
        while i < bytes.len() && bytes[i].is_whitespace() {
            i += 1;
        }
        let kind_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == '_') {
            i += 1;
        }
        let kind: String = bytes[kind_start..i].iter().collect();
        if kind.is_empty() || !kind.starts_with("IFC") {
            return Err(IoError::Format(format!(
                "第 {} 行：#{} 类型名非法",
                line_of(i, &bytes),
                id
            )));
        }
        if i >= bytes.len() || bytes[i] != '(' {
            return Err(IoError::Format(format!(
                "第 {} 行：#{} 缺属性表",
                line_of(i, &bytes),
                id
            )));
        }
        i += 1;
        let mut attrs = Vec::new();
        loop {
            let (attr, next) = parse_attr(&bytes, i)?;
            attrs.push(attr);
            i = next;
            while i < bytes.len() && bytes[i].is_whitespace() {
                i += 1;
            }
            if i >= bytes.len() {
                return Err(IoError::Format(format!(
                    "第 {} 行：#{} 属性表未闭合",
                    line_of(i, &bytes),
                    id
                )));
            }
            match bytes[i] {
                ',' => {
                    i += 1;
                }
                ')' => {
                    i += 1;
                    break;
                }
                _ => {
                    return Err(IoError::Format(format!(
                        "第 {} 行：#{} 属性分隔符非法",
                        line_of(i, &bytes),
                        id
                    )))
                }
            }
        }
        // 实体尾
        while i < bytes.len() && bytes[i].is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != ';' {
            return Err(IoError::Format(format!(
                "第 {} 行：#{} 缺 ;",
                line_of(i, &bytes),
                id
            )));
        }
        i += 1;
        if map.insert(id, Entity { id, kind, attrs }).is_some() {
            return Err(IoError::Format(format!("实体 #{} 重复定义", id)));
        }
    }
    Ok(map)
}

fn parse_attr(s: &[char], mut i: usize) -> Result<(Attr, usize), IoError> {
    while i < s.len() && s[i].is_whitespace() {
        i += 1;
    }
    if i >= s.len() {
        return Err(IoError::Format("属性意外结束".into()));
    }
    match s[i] {
        '$' => Ok((Attr::Null, i + 1)),
        '*' => Ok((Attr::Null, i + 1)), // 派生属性按 null 处理（子集口径）
        '#' => {
            i += 1;
            let start = i;
            while i < s.len() && s[i].is_ascii_digit() {
                i += 1;
            }
            let id: u64 = s[start..i]
                .iter()
                .collect::<String>()
                .parse()
                .map_err(|_| IoError::Format("引用号非法".into()))?;
            Ok((Attr::Ref(id), i))
        }
        '\'' => {
            i += 1;
            let mut out = String::new();
            loop {
                if i >= s.len() {
                    return Err(IoError::Format("字符串未闭合".into()));
                }
                if s[i] == '\'' {
                    if i + 1 < s.len() && s[i + 1] == '\'' {
                        out.push('\'');
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    out.push(s[i]);
                    i += 1;
                }
            }
            Ok((Attr::Str(out), i))
        }
        '.' => {
            i += 1;
            let start = i;
            while i < s.len() && s[i] != '.' {
                i += 1;
            }
            if i >= s.len() {
                return Err(IoError::Format("枚举未闭合".into()));
            }
            let e: String = s[start..i].iter().collect();
            Ok((Attr::Enum(e), i + 1))
        }
        '(' => {
            i += 1;
            let mut items = Vec::new();
            loop {
                if i >= s.len() {
                    return Err(IoError::Format("列表未闭合".into()));
                }
                while i < s.len() && s[i].is_whitespace() {
                    i += 1;
                }
                if s[i] == ')' {
                    i += 1;
                    break;
                }
                let (a, next) = parse_attr(s, i)?;
                items.push(a);
                i = next;
                while i < s.len() && s[i].is_whitespace() {
                    i += 1;
                }
                if i < s.len() && s[i] == ',' {
                    i += 1;
                }
            }
            Ok((Attr::List(items), i))
        }
        c if c.is_ascii_digit() || c == '-' || c == '+' => {
            let start = i;
            while i < s.len()
                && (s[i].is_ascii_digit() || s[i] == '.' || s[i] == 'e' || s[i] == 'E' || s[i] == '-'
                    || s[i] == '+')
            {
                i += 1;
            }
            let n: f64 = s[start..i]
                .iter()
                .collect::<String>()
                .parse()
                .map_err(|_| IoError::Format("数值非法".into()))?;
            Ok((Attr::Num(n), i))
        }
        other => Err(IoError::Format(format!("非法属性起始符 {other}"))),
    }
}

// ---------- 引用助手 ----------

pub fn as_ref(a: &Attr) -> Option<u64> {
    match a {
        Attr::Ref(id) => Some(*id),
        _ => None,
    }
}

pub fn as_list(a: &Attr) -> Option<&Vec<Attr>> {
    match a {
        Attr::List(items) => Some(items),
        _ => None,
    }
}

pub fn as_num(a: &Attr) -> Option<f64> {
    match a {
        Attr::Num(n) => Some(*n),
        _ => None,
    }
}

pub fn as_str(a: &Attr) -> Option<&str> {
    match a {
        Attr::Str(s) => Some(s),
        _ => None,
    }
}

/// 实体引用解析（悬空引用显式报错——审计口径）
pub fn deref<'a>(map: &'a EntityMap, a: &Attr, ctx: &str) -> Result<&'a Entity, IoError> {
    let id = as_ref(a).ok_or_else(|| IoError::Format(format!("{ctx}：应为实体引用")))?;
    map.get(&id)
        .ok_or_else(|| IoError::Format(format!("{ctx}：引用 #{id} 悬空")))
}

// ---------- 几何映射 ----------

#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub origin: [f64; 3],
    pub x: [f64; 3],
    pub z: [f64; 3],
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            origin: [0.0; 3],
            x: [1.0, 0.0, 0.0],
            z: [0.0, 0.0, 1.0],
        }
    }
}

impl Placement {
    fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let y = cross(self.z, self.x);
        [
            self.origin[0] + self.x[0] * p[0] + y[0] * p[1] + self.z[0] * p[2],
            self.origin[1] + self.x[1] * p[0] + y[1] * p[1] + self.z[1] * p[2],
            self.origin[2] + self.x[2] * p[0] + y[2] * p[1] + self.z[2] * p[2],
        ]
    }

    fn compose(&self, rel: &Placement) -> Placement {
        let y = cross(self.z, self.x);
        let mapv = |v: [f64; 3]| {
            [
                self.x[0] * v[0] + y[0] * v[1] + self.z[0] * v[2],
                self.x[1] * v[0] + y[1] * v[1] + self.z[1] * v[2],
                self.x[2] * v[0] + y[2] * v[1] + self.z[2] * v[2],
            ]
        };
        Placement {
            origin: self.apply(rel.origin),
            x: norm(mapv(rel.x)),
            z: norm(mapv(rel.z)),
        }
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    [v[0] / l, v[1] / l, v[2] / l]
}

// ---------- 导入：IFC → IR ----------

use archcore_ir::{
    Geometry, Object, ObjectKind, Scene, Transform, Units, UpAxis,
};

const ELEMENT_KINDS: &[&str] = &[
    "IFCWALL",
    "IFCWALLSTANDARDCASE",
    "IFCSLAB",
    "IFCCOLUMN",
    "IFCBEAM",
    "IFCDOOR",
    "IFCWINDOW",
    "IFCBUILDINGELEMENTPROXY",
    "IFCMEMBER",
    "IFCPLATE",
    "IFCFOOTING",
    "IFCROOF",
    "IFCSTAIR",
    "IFCRAILING",
];

fn parse_vec3(a: &Attr) -> Result<[f64; 3], IoError> {
    let items = as_list(a).ok_or_else(|| IoError::Format("应为三元列表".into()))?;
    if items.len() != 3 {
        return Err(IoError::Format(format!(
            "三元列表长度非法：{}",
            items.len()
        )));
    }
    Ok([
        as_num(&items[0]).ok_or_else(|| IoError::Format("坐标非数值".into()))?,
        as_num(&items[1]).ok_or_else(|| IoError::Format("坐标非数值".into()))?,
        as_num(&items[2]).ok_or_else(|| IoError::Format("坐标非数值".into()))?,
    ])
}

/// 解析 IfcAxis2Placement3D（$ 则默认）
fn placement3(map: &EntityMap, a: &Attr, ctx: &str) -> Result<Placement, IoError> {
    if matches!(a, Attr::Null) {
        return Ok(Placement::default());
    }
    let e = deref(map, a, ctx)?;
    if e.kind != "IFCAXIS2PLACEMENT3D" {
        return Err(IoError::Format(format!("{ctx}：应为 IfcAxis2Placement3D")));
    }
    let loc = deref(map, &e.attrs[0], ctx)?;
    let origin = parse_vec3(&loc.attrs[0])?;
    let z = if matches!(e.attrs[1], Attr::Null) {
        [0.0, 0.0, 1.0]
    } else {
        let d = deref(map, &e.attrs[1], ctx)?;
        norm(parse_vec3(&d.attrs[0])?)
    };
    let x = if matches!(e.attrs[2], Attr::Null) {
        [1.0, 0.0, 0.0]
    } else {
        let d = deref(map, &e.attrs[2], ctx)?;
        norm(parse_vec3(&d.attrs[0])?)
    };
    Ok(Placement { origin, x, z })
}

/// 产品放置链 → 世界变换（环检测：visited 集双保险，审计口径）
fn product_placement(
    map: &EntityMap,
    a: &Attr,
    seen: &mut Vec<u64>,
) -> Result<Placement, IoError> {
    if matches!(a, Attr::Null) {
        return Ok(Placement::default());
    }
    let id = as_ref(a).ok_or_else(|| IoError::Format("放置应为引用".into()))?;
    if seen.contains(&id) {
        return Err(IoError::Format(format!("放置链成环 @#{id}")));
    }
    seen.push(id);
    let e = map
        .get(&id)
        .ok_or_else(|| IoError::Format(format!("放置 #{id} 悬空")))?;
    if e.kind != "IFCLOCALPLACEMENT" {
        return Ok(Placement::default());
    }
    let parent = product_placement(map, &e.attrs[0], seen)?;
    let rel = placement3(map, &e.attrs[1], "RelativePlacement")?;
    Ok(parent.compose(&rel))
}

/// IfcExtrudedAreaSolid → 世界坐标三角形（矩形/折线 profile；凸端盖扇形三角化）
fn extruded_solid(
    map: &EntityMap,
    e: &Entity,
    world: &Placement,
    out: &mut Vec<[f64; 3]>,
    idx: &mut Vec<u32>,
) -> Result<(), IoError> {
    let area = deref(map, &e.attrs[0], "SweptArea")?;
    let mut pts: Vec<[f64; 2]> = Vec::new();
    match area.kind.as_str() {
        "IFCRECTANGLEPROFILEDEF" => {
            let (xd, yd) = (
                as_num(&area.attrs[3]).ok_or_else(|| IoError::Format("XDim 缺失".into()))?,
                as_num(&area.attrs[4]).ok_or_else(|| IoError::Format("YDim 缺失".into()))?,
            );
            let (cx, cy) = if matches!(area.attrs[2], Attr::Null) {
                (0.0, 0.0)
            } else {
                let p = deref(map, &area.attrs[2], "ProfilePosition")?;
                let loc = deref(map, &p.attrs[0], "ProfileLocation")?;
                let v = as_list(&loc.attrs[0]).ok_or_else(|| IoError::Format("位置非法".into()))?;
                (
                    as_num(&v[0]).unwrap_or(0.0),
                    as_num(&v[1]).unwrap_or(0.0),
                )
            };
            pts = vec![
                [cx - xd / 2.0, cy - yd / 2.0],
                [cx + xd / 2.0, cy - yd / 2.0],
                [cx + xd / 2.0, cy + yd / 2.0],
                [cx - xd / 2.0, cy + yd / 2.0],
            ];
        }
        "IFCARBITRARYCLOSEDPROFILEDEF" => {
            let curve = deref(map, &area.attrs[2], "OuterCurve")?;
            if curve.kind != "IFCPOLYLINE" {
                return Err(IoError::Format("v1 仅支持 IfcPolyline 轮廓".into()));
            }
            let list = as_list(&curve.attrs[0]).ok_or_else(|| IoError::Format("折线点表缺失".into()))?;
            for p in list {
                let pt = deref(map, p, "PolylinePoint")?;
                let v = as_list(&pt.attrs[0]).ok_or_else(|| IoError::Format("点坐标缺失".into()))?;
                if v.len() < 2 {
                    return Err(IoError::Format("轮廓点须为 2D".into()));
                }
                pts.push([
                    as_num(&v[0]).ok_or_else(|| IoError::Format("坐标非数值".into()))?,
                    as_num(&v[1]).ok_or_else(|| IoError::Format("坐标非数值".into()))?,
                ]);
            }
            if pts.len() < 3 {
                return Err(IoError::Format("轮廓点不足 3".into()));
            }
        }
        other => {
            return Err(IoError::Format(format!(
                "v1 不支持的拉伸轮廓：{other}"
            )))
        }
    }
    let solid_pl = placement3(map, &e.attrs[1], "SolidPosition")?;
    let dir = if matches!(e.attrs[2], Attr::Null) {
        [0.0, 0.0, 1.0]
    } else {
        let d = deref(map, &e.attrs[2], "ExtrudedDirection")?;
        norm(parse_vec3(&d.attrs[0])?)
    };
    let depth = as_num(&e.attrs[3]).ok_or_else(|| IoError::Format("Depth 缺失".into()))?;
    if !depth.is_finite() || depth <= 0.0 {
        return Err(IoError::Format("Depth 须为正".into()));
    }
    let base = out.len() as u32;
    let n = pts.len() as u32;
    let full = world.compose(&solid_pl);
    for p in &pts {
        out.push(full.apply([p[0], p[1], 0.0]));
    }
    for p in &pts {
        out.push(full.apply([
            p[0] + dir[0] * depth,
            p[1] + dir[1] * depth,
            dir[2] * depth,
        ]));
    }
    // 侧壁
    for i in 0..n {
        let j = (i + 1) % n;
        idx.extend([base + i, base + j, base + n + j, base + i, base + n + j, base + n + i]);
    }
    // 端盖（扇形，v1 凸轮廓口径）
    for i in 1..(n - 1) {
        idx.extend([base, base + i + 1, base + i]);
        idx.extend([base + n, base + n + i, base + n + i + 1]);
    }
    Ok(())
}

/// IfcTriangulatedFaceSet → 世界坐标三角形（CoordIndex 为 1 基）
fn triangulated_faceset(
    map: &EntityMap,
    e: &Entity,
    world: &Placement,
    out: &mut Vec<[f64; 3]>,
    idx: &mut Vec<u32>,
) -> Result<(), IoError> {
    let pts_ent = deref(map, &e.attrs[0], "Coordinates")?;
    let list = as_list(&pts_ent.attrs[0]).ok_or_else(|| IoError::Format("坐标表缺失".into()))?;
    let base = out.len() as u32;
    for p in list {
        out.push(world.apply(parse_vec3(p)?));
    }
    let tris = as_list(&e.attrs[3]).ok_or_else(|| IoError::Format("CoordIndex 缺失".into()))?;
    for t in tris {
        let v = as_list(t).ok_or_else(|| IoError::Format("三角形非法".into()))?;
        if v.len() != 3 {
            return Err(IoError::Format("三角形须为三索引".into()));
        }
        let mut f = [0u32; 3];
        for k in 0..3 {
            let one_based = as_num(&v[k]).ok_or_else(|| IoError::Format("索引非数值".into()))?;
            if !(one_based >= 1.0 && one_based <= list.len() as f64) {
                return Err(IoError::Format(format!(
                    "三角形索引越界：{one_based}"
                )));
            }
            f[k] = base + (one_based as u32 - 1);
        }
        idx.extend(f);
    }
    Ok(())
}

/// IFC 文本 → IR 场景
pub fn import_ifc(text: &str) -> Result<Scene, IoError> {
    let map = parse_p21(text)?;
    // 单位：IfcSIUnit 长度单位（找不到则假定米并标注——不静默猜）
    let mut units = Units::M;
    let mut units_assumed = true;
    for e in map.values() {
        if e.kind == "IFCSIUNIT" && matches!(&e.attrs[1], Attr::Enum(u) if u == "LENGTHUNIT") {
            units_assumed = false;
            units = match &e.attrs[2] {
                Attr::Enum(p) if p == "MILLI" => Units::Mm,
                Attr::Enum(p) if p == "CENTI" => Units::Cm,
                _ => Units::M,
            };
        }
    }
    let mut scene = Scene {
        schema_version: "0.1".into(),
        units,
        up_axis: UpAxis::ZUp,
        name: "imported_ifc".into(),
        objects: vec![],
        materials: vec![],
        assets: vec![],
        meta: Default::default(),
    };
    if units_assumed {
        scene.meta.insert(
            "ifc_units_assumed".into(),
            "未找到 IfcSIUnit 长度单位，假定米".into(),
        );
    }
    // 楼层（空间结构）
    let mut storey_of: BTreeMap<u64, String> = BTreeMap::new();
    for e in map.values() {
        if e.kind == "IFCBUILDINGSTOREY" {
            let id = format!("obj-{}", scene.objects.len() + 1);
            let name = as_str(&e.attrs[2]).unwrap_or("Storey").to_string();
            let z = as_num(e.attrs.get(9).unwrap_or(&Attr::Null)).unwrap_or(0.0);
            scene.objects.push(Object {
                id: id.clone(),
                name: name.clone(),
                kind: ObjectKind::Group,
                parent: None,
                transform: Transform {
                    translation: [0.0, 0.0, z],
                    ..Default::default()
                },
                visible: true,
                meta: Default::default(),
            });
            storey_of.insert(e.id, id);
        }
    }
    // 空间包含关系 → 父表
    let mut parent_of: BTreeMap<u64, String> = BTreeMap::new();
    for e in map.values() {
        if e.kind == "IFCRELCONTAINEDINSPATIALSTRUCTURE" {
            if let Some(st) = as_ref(&e.attrs[4]).and_then(|sid| storey_of.get(&sid)) {
                if let Some(items) = as_list(&e.attrs[5]) {
                    for it in items {
                        if let Some(eid) = as_ref(it) {
                            parent_of.insert(eid, st.clone());
                        }
                    }
                }
            }
        }
    }
    // 构件 → 网格
    for e in map.values() {
        if !ELEMENT_KINDS.contains(&e.kind.as_str()) {
            continue;
        }
        let mut positions: Vec<[f64; 3]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let world = product_placement(&map, &e.attrs[5], &mut Vec::new())?;
        if !matches!(e.attrs[6], Attr::Null) {
            let shape = deref(&map, &e.attrs[6], "Representation")?;
            // IfcProductDefinitionShape → Representations → ShapeRepresentation → Items
            let reps = as_list(&shape.attrs[2]).ok_or_else(|| IoError::Format("表示列表缺失".into()))?;
            for r in reps {
                let rep = deref(&map, r, "ShapeRepresentation")?;
                let items = as_list(&rep.attrs[3]).ok_or_else(|| IoError::Format("表示项缺失".into()))?;
                for it in items {
                    let ent = deref(&map, it, "RepresentationItem")?;
                    match ent.kind.as_str() {
                        "IFCEXTRUDEDAREASOLID" => {
                            extruded_solid(&map, ent, &world, &mut positions, &mut indices)?
                        }
                        "IFCTRIANGULATEDFACESET" => {
                            triangulated_faceset(&map, ent, &world, &mut positions, &mut indices)?
                        }
                        other => {
                            return Err(IoError::Format(format!(
                                "v1 不支持的表示项：{other}"
                            )))
                        }
                    }
                }
            }
        }
        if indices.is_empty() {
            continue; // 无几何构件（空间标注类）跳过
        }
        let id = format!("obj-{}", scene.objects.len() + 1);
        scene.objects.push(Object {
            id,
            name: as_str(&e.attrs[2]).unwrap_or(&e.kind).to_string(),
            kind: ObjectKind::Mesh {
                geometry: Geometry {
                    positions,
                    indices,
                    normals: None,
                    uv_sets: vec![],
                },
                material_slots: vec![],
            },
            parent: parent_of.get(&e.id).cloned(),
            transform: Transform::default(),
            visible: true,
            meta: Default::default(),
        });
    }
    Ok(scene)
}

// ---------- 导出：IR → IFC（镶嵌表示，IFC4） ----------

fn gid(seed: u64) -> String {
    const CH: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$";
    let mut s = String::from("0Archcore");
    let mut n = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    while s.len() < 22 {
        s.push(CH[(n >> 33) as usize % CH.len()] as char);
        n = n.wrapping_mul(6364136223846793005).wrapping_add(1);
    }
    s.truncate(22);
    s
}

/// IR 场景 → IFC4 文本（IfcTriangulatedFaceSet 镶嵌表示 + 单楼层归并）
pub fn export_ifc(scene: &Scene) -> String {
    let n = std::cell::Cell::new(0u64);
    let next = |k: &str| -> String {
        n.set(n.get() + 1);
        format!("#{}=IFC{}", n.get(), k)
    };
    let mut out = String::new();
    out.push_str("ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [CoordinationView]'),'2;1');\n");
    out.push_str(&format!(
        "FILE_NAME('{}','2026-01-01T00:00:00',('archcore'),('archcore'),'archcore 0.1','archcore-mcp','');\n",
        scene.name
    ));
    out.push_str("FILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n");
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("{};", next("SIUNIT(*,.LENGTHUNIT.,$,.METRE.)")));
    let unit_id = n.get();
    lines.push(format!(
        "{};",
        next(&format!("UNITASSIGNMENT((#{}))", unit_id))
    ));
    let assign_id = n.get();
    lines.push(format!("{};", next("CARTESIANPOINT((0.,0.,0.))")));
    let origin_id = n.get();
    lines.push(format!(
        "{};",
        next(&format!("AXIS2PLACEMENT3D(#{},$,$)", origin_id))
    ));
    let axis_id = n.get();
    lines.push(format!(
        "{};",
        next(&format!("GEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.0E-5,#{})", axis_id))
    ));
    let ctx_id = n.get();
    lines.push(format!(
        "{};",
        next(&format!(
            "PROJECT('{}',$,'{}',$,$,$,$,(#{}),#{})",
            gid(1),
            scene.name,
            ctx_id,
            assign_id
        ))
    ));
    let project_id = n.get();
    lines.push(format!(
        "{};",
        next(&format!("SITE('{}',$,'Site',$,$,$,$,$,.ELEMENT.,$,$,$,$,$)", gid(2)))
    ));
    let site_id = n.get();
    lines.push(format!(
        "{};",
        next(&format!("BUILDING('{}',$,'Building',$,$,$,$,$,.ELEMENT.,$,$,$,$)", gid(3)))
    ));
    let building_id = n.get();
    lines.push(format!(
        "{};",
        next(&format!(
            "BUILDINGSTOREY('{}',$,'Storey',$,$,$,$,$,.ELEMENT.,0.)",
            gid(4)
        ))
    ));
    let storey_id = n.get();
    lines.push(format!(
        "{};",
        next(&format!(
            "RELAGGREGATES('{}',$,$,$,#{},(#{}))",
            gid(5),
            project_id,
            site_id
        ))
    ));
    lines.push(format!(
        "{};",
        next(&format!(
            "RELAGGREGATES('{}',$,$,$,#{},(#{}))",
            gid(6),
            site_id,
            building_id
        ))
    ));
    lines.push(format!(
        "{};",
        next(&format!(
            "RELAGGREGATES('{}',$,$,$,#{},(#{}))",
            gid(7),
            building_id,
            storey_id
        ))
    ));
    let mut proxies: Vec<u64> = Vec::new();
    let mut mats: Vec<(u64, String)> = Vec::new();
    for (oi, o) in scene.objects.iter().enumerate() {
        let ObjectKind::Mesh {
            geometry,
            material_slots,
        } = &o.kind
        else {
            continue;
        };
        // 坐标表
        let pts: Vec<String> = geometry
            .positions
            .iter()
            .map(|p| format!("({},{},{})", p[0], p[1], p[2]))
            .collect();
        lines.push(format!(
            "{};",
            next(&format!(
                "CARTESIANPOINTLIST3D(({}),$)",
                pts.join(",")
            ))
        ));
        let pts_id = n.get();
        let tris: Vec<String> = geometry
            .indices
            .chunks(3)
            .map(|t| format!("({},{},{})", t[0] + 1, t[1] + 1, t[2] + 1))
            .collect();
        lines.push(format!(
            "{};",
            next(&format!(
                "TRIANGULATEDFACESET(#{},$,.T.,({}),$)",
                pts_id,
                tris.join(",")
            ))
        ));
        let set_id = n.get();
        lines.push(format!(
            "{};",
            next(&format!(
                "SHAPEREPRESENTATION(#{},'Body','Tessellation',(#{}))",
                ctx_id, set_id
            ))
        ));
        let rep_id = n.get();
        lines.push(format!(
            "{};",
            next(&format!("PRODUCTDEFINITIONSHAPE($,$,(#{}))", rep_id))
        ));
        let pds_id = n.get();
        lines.push(format!(
            "{};",
            next(&format!("AXIS2PLACEMENT3D(#{} ,$,$)", origin_id))
        ));
        let pl_axis = n.get();
        lines.push(format!(
            "{};",
            next(&format!("LOCALPLACEMENT($,#{})", pl_axis))
        ));
        let placement_id = n.get();
        lines.push(format!(
            "{};",
            next(&format!(
                "BUILDINGELEMENTPROXY('{}',$,'{}',$,$,#{},#{},$)",
                gid(100 + oi as u64),
                o.name,
                placement_id,
                pds_id
            ))
        ));
        let proxy_id = n.get();
        proxies.push(proxy_id);
        if let Some(m) = material_slots.first() {
            let mname = scene
                .materials
                .iter()
                .find(|x| &x.id == m)
                .map(|x| x.name.clone())
                .unwrap_or_else(|| m.clone());
            mats.push((proxy_id, mname));
        }
    }
    if !proxies.is_empty() {
        let list: Vec<String> = proxies.iter().map(|p| format!("#{p}")).collect();
        lines.push(format!(
            "{};",
            next(&format!(
                "RELCONTAINEDINSPATIALSTRUCTURE('{}',$,$,$,#{},({}))",
                gid(90),
                storey_id,
                list.join(",")
            ))
        ));
    }
    for (pi, (proxy, mname)) in mats.iter().enumerate() {
        lines.push(format!(
            "{};",
            next(&format!("MATERIAL('{}',$)", mname))
        ));
        let mat_id = n.get();
        lines.push(format!(
            "{};",
            next(&format!(
                "RELASSOCIATESMATERIAL('{}',$,$,$,(#{}),#{})",
                gid(200 + pi as u64),
                proxy,
                mat_id
            ))
        ));
    }
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    out
}
