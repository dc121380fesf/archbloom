//! IFC 子集回归：解析、空间结构、往返数值、敌对输入、单位语义
use archcore_io::{export_ifc, import_ifc};
use archcore_ir::{Geometry, Material, Object, ObjectKind, Scene, ShadingModel, Transform, Units, UpAxis};

const WALL_IFC: &str = r#"ISO-10303-21;
HEADER;
FILE_DESCRIPTION((''),'2;1');
FILE_NAME('t','',(''),(''),'','','');
FILE_SCHEMA(('IFC4'));
ENDSEC;
DATA;
#1=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);
#2=IFCUNITASSIGNMENT((#1));
#3=IFCCARTESIANPOINT((0.,0.,0.));
#4=IFCAXIS2PLACEMENT3D(#3,$,$);
#5=IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.0E-5,#4,$);
#6=IFCPROJECT('g1',$,'P',$,$,$,$,(#5),#2);
#7=IFCSITE('g2',$,'S',$,$,$,$,$,.ELEMENT.,$,$,$,$,$);
#8=IFCBUILDING('g3',$,'B',$,$,$,$,$,.ELEMENT.,$,$,$,$);
#9=IFCBUILDINGSTOREY('g4',$,'L1',$,$,$,$,$,.ELEMENT.,0.);
#10=IFCRELAGGREGATES('g5',$,$,$,#6,(#7));
#11=IFCRELAGGREGATES('g6',$,$,$,#7,(#8));
#12=IFCRELAGGREGATES('g7',$,$,$,#8,(#9));
#13=IFCLOCALPLACEMENT($,#4);
#14=IFCCARTESIANPOINT((0.,0.,0.));
#15=IFCAXIS2PLACEMENT2D(#14,$);
#16=IFCRECTANGLEPROFILEDEF(.AREA.,$,#15,6.,0.2);
#17=IFCDIRECTION((0.,0.,1.));
#18=IFCEXTRUDEDAREASOLID(#16,#4,#17,2.8);
#19=IFCSHAPEREPRESENTATION(#5,'Body','SweptSolid',(#18));
#20=IFCPRODUCTDEFINITIONSHAPE($,$,(#19));
#21=IFCWALL('g8',$,'Wall',$,$,#13,#20,$);
#22=IFCRELCONTAINEDINSPATIALSTRUCTURE('g9',$,$,$,#9,(#21));
ENDSEC;
END-ISO-10303-21;"#;

/// 拉伸体墙：解析出楼层组 + 网格，尺寸贴合 profile
#[test]
fn imports_extruded_wall_with_storey() {
    let scene = import_ifc(WALL_IFC).unwrap();
    assert_eq!(scene.units, Units::M);
    let storey = scene.objects.iter().find(|o| o.name == "L1").unwrap();
    let wall = scene.objects.iter().find(|o| o.name == "Wall").unwrap();
    assert_eq!(wall.parent.as_deref(), Some(storey.id.as_str()));
    let ObjectKind::Mesh { geometry, .. } = &wall.kind else {
        panic!("应为网格");
    };
    let lo = |axis: usize| {
        geometry
            .positions
            .iter()
            .map(|p| p[axis])
            .fold(f64::MAX, f64::min)
    };
    let hi = |axis: usize| {
        geometry
            .positions
            .iter()
            .map(|p| p[axis])
            .fold(f64::MIN, f64::max)
    };
    assert!((hi(0) - lo(0) - 6.0).abs() < 1e-9, "X 尺寸漂移");
    assert!((hi(1) - lo(1) - 0.2).abs() < 1e-9, "Y 尺寸漂移");
    assert!((hi(2) - lo(2) - 2.8).abs() < 1e-9, "Z 尺寸漂移");
    assert!(!geometry.indices.is_empty());
}

/// 往返数值：IR → IFC → IR，顶点不漂移（审计 #15② 同款口径）
#[test]
fn round_trip_preserves_vertices() {
    let scene = Scene {
        schema_version: "0.1".into(),
        units: Units::M,
        up_axis: UpAxis::ZUp,
        name: "rt".into(),
        objects: vec![Object {
            id: "obj-1".into(),
            name: "体".into(),
            kind: ObjectKind::Mesh {
                geometry: Geometry {
                    positions: vec![
                        [0.25, 0.5, 0.75],
                        [1.25, 0.5, 0.75],
                        [1.25, 1.5, 0.75],
                        [0.25, 1.5, 0.75],
                        [0.25, 0.5, 1.75],
                        [1.25, 0.5, 1.75],
                        [1.25, 1.5, 1.75],
                        [0.25, 1.5, 1.75],
                    ],
                    indices: vec![
                        0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 2, 3, 7, 2, 7, 6,
                        1, 2, 6, 1, 6, 5, 0, 4, 7, 0, 7, 3,
                    ],
                    normals: None,
                    uv_sets: vec![],
                },
                material_slots: vec!["mat-0".into()],
            },
            parent: None,
            transform: Transform::default(),
            visible: true,
            meta: Default::default(),
        }],
        materials: vec![Material {
            id: "mat-0".into(),
            name: "M".into(),
            shading_model: ShadingModel::PbrMetallicRoughness,
            base_color: [0.5; 4],
            metallic: 0.0,
            roughness: 0.5,
            opacity: 1.0,
            textures: Default::default(),
        }],
        assets: vec![],
        meta: Default::default(),
    };
    let text = export_ifc(&scene);
    assert!(text.contains("IFCTRIANGULATEDFACESET"));
    let back = import_ifc(&text).unwrap();
    let mesh = back
        .objects
        .iter()
        .find_map(|o| match &o.kind {
            ObjectKind::Mesh { geometry, .. } => Some(geometry),
            _ => None,
        })
        .unwrap();
    let src = match &scene.objects[0].kind {
        ObjectKind::Mesh { geometry, .. } => geometry,
        _ => panic!(),
    };
    assert_eq!(mesh.positions.len(), src.positions.len());
    for (a, b) in src.positions.iter().zip(mesh.positions.iter()) {
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 1e-9, "顶点漂移 {a:?} vs {b:?}");
        }
    }
    assert_eq!(mesh.indices.len(), src.indices.len());
}

/// 敌对输入：截断实体 / 悬空引用 / 索引越界，全部结构化报错不 panic
#[test]
fn malformed_ifc_errors_not_panics() {
    // 截断
    assert!(import_ifc("ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#1=IFCWALL('g',$'bad").is_err());
    // 悬空放置引用
    let bad = WALL_IFC.replace("#13=IFCLOCALPLACEMENT($,#4);", "#13=IFCLOCALPLACEMENT($,#41);");
    assert!(import_ifc(&bad).is_err());
    // 三角形索引越界（真面集 + 99 号索引）
    let oob = WALL_IFC
        .replace(
            "#16=IFCRECTANGLEPROFILEDEF(.AREA.,$,#15,6.,0.2);\n#17=IFCDIRECTION((0.,0.,1.));\n#18=IFCEXTRUDEDAREASOLID(#16,#4,#17,2.8);",
            "#16=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1.,0.,0.),(0.,1.,0.)),$);\n#17=IFCDIRECTION((0.,0.,1.));\n#18=IFCTRIANGULATEDFACESET(#16,$,.T.,((1,2,99)),$);",
        );
    assert!(import_ifc(&oob).is_err(), "越界索引必须显式报错");
}

/// 单位语义：MILLI→Mm；缺单位→假定并标注
#[test]
fn units_semantics() {
    let mm = WALL_IFC.replace(".LENGTHUNIT.,$,.METRE.", ".LENGTHUNIT.,.MILLI.,.METRE.");
    let s = import_ifc(&mm).unwrap();
    assert_eq!(s.units, Units::Mm);
    let none = WALL_IFC.replace("#1=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);", "#1=IFCPROPERTY('x');");
    let s2 = import_ifc(&none).unwrap();
    assert_eq!(s2.units, Units::M);
    assert!(s2.meta.contains_key("ifc_units_assumed"));
}
