//! 评审图渲染器 v0 —— 软光栅（平涂 + z-buffer + 自动取景）
//! 职责：让"人审"看得见——结构对不对一眼可辨。美感渲染是阶段二渲染器的事。
//! 已知边界：自动取景（IR 相机驱动留给 wgpu 视口）；Instance 节点暂不展开；灯光通道无（平涂假光）。
use archcore_ir::{ObjectKind, Scene};
use image::{Rgb, RgbImage};

pub mod pathtracer;
pub use pathtracer::{
    render_panorama, render_pathtraced, render_pathtraced_exposed, PanoramaOpts, Projection,
};

/// Web 查看器导出（审阅表现面）：场景 → 自包含 HTML
pub mod web;
pub use web::export_web;

/// GPU 渲染轨（feature = "gpu"）：与 CPU 同语义、非确定性；CPU 轨保持确定性参考。
#[cfg(feature = "gpu")]
pub mod gpu;
#[cfg(feature = "gpu")]
pub use gpu::{gpu_adapter, render_panorama_gpu, render_pathtraced_gpu};

type M3 = [[f64; 3]; 3];
type V3 = [f64; 3];

fn quat_to_m3(q: [f64; 4]) -> M3 {
    let [x, y, z, w] = q;
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

fn m3_mul(a: M3, b: M3) -> M3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn m3_apply(m: M3, v: V3) -> V3 {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// 世界变换 =（旋转×缩放矩阵, 平移），父子链合成
#[derive(Clone, Copy)]
struct Xf {
    m: M3,
    t: V3,
}

impl Xf {
    fn apply(&self, v: V3) -> V3 {
        let r = m3_apply(self.m, v);
        [r[0] + self.t[0], r[1] + self.t[1], r[2] + self.t[2]]
    }
    fn compose(parent: &Xf, local: &Xf) -> Xf {
        Xf {
            m: m3_mul(parent.m, local.m),
            t: parent.apply(local.t),
        }
    }
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(v: V3) -> V3 {
    let l = dot(v, v).sqrt().max(1e-12);
    [v[0] / l, v[1] / l, v[2] / l]
}

#[derive(Clone, Copy, Debug)]
pub struct Tri {
    pub p: [V3; 3],
    pub color: [f64; 3],
    /// 材质语义：金属度 ≥ 0.5 视为镜面（路径追踪用；光栅忽略）
    pub mirror: bool,
    /// 逐顶点 UV（几何有 UV 集且与顶点数对齐时）
    pub uv: Option<[[f64; 2]; 3]>,
    /// base_color 贴图在路径表中的索引
    pub tex: Option<u32>,
}

/// 给视口/导出用的取材口：三角形 + 贴图路径表（与评审图/路径追踪同源）
pub fn review_triangles(scene: &Scene) -> (Vec<Tri>, Vec<String>) {
    collect_tris(scene)
}

/// 解析父链世界变换，收集三角形；附贴图路径表（Tri.tex 的索引域）
fn collect_tris(scene: &Scene) -> (Vec<Tri>, Vec<String>) {
    let mut tris = Vec::new();
    // 贴图表：材质 base_color 贴图去重
    let mut tex_paths: Vec<String> = Vec::new();
    let mut tex_of: std::collections::BTreeMap<&str, u32> = Default::default();
    for m in &scene.materials {
        if let Some(aid) = &m.textures.base_color {
            if let Some(a) = scene.assets.iter().find(|a| &a.id == aid) {
                let idx = match tex_paths.iter().position(|p| p == &a.uri) {
                    Some(i) => i as u32,
                    None => {
                        tex_paths.push(a.uri.clone());
                        (tex_paths.len() - 1) as u32
                    }
                };
                tex_of.insert(m.id.as_str(), idx);
            }
        }
    }
    let identity = Xf {
        m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        t: [0.0; 3],
    };
    for obj in &scene.objects {
        // 父链展开（visited 双保险：成环坏数据不得挂死渲染——审计 #4）
        let mut chain = vec![obj.id.as_str()];
        let mut cur = obj.parent.as_deref();
        while let Some(p) = cur {
            if chain.contains(&p) {
                break; // 环：verify 闸负责报错，这里只求不挂死
            }
            chain.push(p);
            cur = scene.objects.iter().find(|o| o.id == p).and_then(|o| o.parent.as_deref());
        }
        let mut xf = identity;
        for id in chain.iter().rev() {
            let Some(o) = scene.objects.iter().find(|o| o.id == *id) else { continue };
            let t = &o.transform;
            let mut m = quat_to_m3(t.rotation);
            for r in m.iter_mut() {
                r[0] *= t.scale[0];
                r[1] *= t.scale[1];
                r[2] *= t.scale[2];
            }
            xf = Xf::compose(&xf, &Xf { m, t: t.translation });
        }
        if let ObjectKind::Mesh { geometry, material_slots } = &obj.kind {
            let (color, mirror, tex) = material_slots
                .first()
                .and_then(|s| {
                    let m = scene.materials.iter().find(|m| m.id == *s)?;
                    Some((
                        [m.base_color[0], m.base_color[1], m.base_color[2]],
                        m.metallic >= 0.5,
                        tex_of.get(m.id.as_str()).copied(),
                    ))
                })
                .unwrap_or(([0.72, 0.72, 0.75], false, None));
            let uvs = geometry.uv_sets.first().map(|u| &u.coords);
            for tri in geometry.indices.as_chunks::<3>().0 {
                let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
                if a < geometry.positions.len() && b < geometry.positions.len() && c < geometry.positions.len() {
                    let uv = uvs.and_then(|coords| {
                        if coords.len() == geometry.positions.len() {
                            Some([coords[a], coords[b], coords[c]])
                        } else {
                            None
                        }
                    });
                    tris.push(Tri {
                        p: [
                            xf.apply(geometry.positions[a]),
                            xf.apply(geometry.positions[b]),
                            xf.apply(geometry.positions[c]),
                        ],
                        color,
                        mirror,
                        uv,
                        tex,
                    });
                }
            }
        }
    }
    (tris, tex_paths)
}

/// 评审图：自动取景 + 平涂着色 + z-buffer
pub fn render_review(scene: &Scene, width: u32, height: u32) -> Vec<u8> {
    let (tris, _) = collect_tris(scene);
    let mut img = RgbImage::from_pixel(width, height, Rgb([245, 246, 248]));
    let mut zbuf = vec![f64::INFINITY; (width * height) as usize];

    // 自动取景：包围球
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for t in &tris {
        for p in &t.p {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    let (center, radius) = if tris.is_empty() {
        ([0.0; 3], 1.0)
    } else {
        let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
        let r = dot(sub(hi, lo), sub(hi, lo)).sqrt() / 2.0;
        (c, r.max(0.5))
    };

    let dir = norm([1.0, 0.65, 0.9]);
    let cam = [center[0] + dir[0] * radius * 2.4, center[1] + dir[1] * radius * 2.4, center[2] + dir[2] * radius * 2.4];
    let fwd = norm(sub(center, cam));
    let up_hint = [0.0, 0.0, 1.0];
    let right = norm(cross(fwd, up_hint));
    let up = cross(right, fwd);
    let fov_scale = (50.0f64.to_radians() / 2.0).tan();
    let aspect = width as f64 / height as f64;

    let project = |p: V3| -> Option<(f64, f64, f64)> {
        let rel = sub(p, cam);
        let z = dot(rel, fwd);
        if z < 1e-3 {
            return None;
        }
        let x = dot(rel, right) / z / (fov_scale * aspect);
        let y = dot(rel, up) / z / fov_scale;
        Some((
            (x + 1.0) * 0.5 * width as f64,
            (1.0 - y) * 0.5 * height as f64,
            z,
        ))
    };

    let light = norm([0.4, 0.35, 1.0]);
    for t in &tris {
        let (Some(a), Some(b), Some(c)) = (project(t.p[0]), project(t.p[1]), project(t.p[2])) else {
            continue;
        };
        let n = norm(cross(sub(t.p[1], t.p[0]), sub(t.p[2], t.p[0])));
        let lambert = 0.28 + 0.72 * dot(n, light).abs();
        let col = Rgb([
            (t.color[0].clamp(0.0, 1.0) * lambert * 255.0) as u8,
            (t.color[1].clamp(0.0, 1.0) * lambert * 255.0) as u8,
            (t.color[2].clamp(0.0, 1.0) * lambert * 255.0) as u8,
        ]);
        // 包围盒 + 重心坐标 + z-buffer
        let min_x = a.0.min(b.0).min(c.0).floor().max(0.0) as u32;
        let max_x = a.0.max(b.0).max(c.0).ceil().min(width as f64 - 1.0) as u32;
        let min_y = a.1.min(b.1).min(c.1).floor().max(0.0) as u32;
        let max_y = a.1.max(b.1).max(c.1).ceil().min(height as f64 - 1.0) as u32;
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area.abs() < 1e-9 {
            continue;
        }
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let px = x as f64 + 0.5;
                let py = y as f64 + 0.5;
                let w0 = ((b.0 - px) * (c.1 - py) - (b.1 - py) * (c.0 - px)) / area;
                let w1 = ((c.0 - px) * (a.1 - py) - (c.1 - py) * (a.0 - px)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * a.2 + w1 * b.2 + w2 * c.2;
                let idx = (y * width + x) as usize;
                if z < zbuf[idx] {
                    zbuf[idx] = z;
                    img.put_pixel(x, y, col);
                }
            }
        }
    }

    // 线框描边（评审图必须读得出棱：门洞/开孔一眼可辨）
    let edge_col = Rgb([42, 46, 56]);
    for t in &tris {
        let (Some(a), Some(b), Some(c)) = (project(t.p[0]), project(t.p[1]), project(t.p[2])) else {
            continue;
        };
        for (p0, p1) in [(a, b), (b, c), (c, a)] {
            let steps = ((p1.0 - p0.0).abs().max((p1.1 - p0.1).abs()) as u32).max(1);
            for s in 0..=steps {
                let f = s as f64 / steps as f64;
                let x = p0.0 + (p1.0 - p0.0) * f;
                let y = p0.1 + (p1.1 - p0.1) * f;
                let z = p0.2 + (p1.2 - p0.2) * f - 1e-3; // 深度偏置：描边浮面
                if x < 0.0 || y < 0.0 || x >= width as f64 || y >= height as f64 {
                    continue;
                }
                let idx = (y as u32 * width + x as u32) as usize;
                if z <= zbuf[idx] {
                    zbuf[idx] = z;
                    img.put_pixel(x as u32, y as u32, edge_col);
                }
            }
        }
    }

    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("PNG 编码失败");
    out
}

/// 横向拼接多张 PNG 为对比长图（人审审稿面；列序=变体序，高度取最大、不足处留白）
pub fn stitch_horizontal(pngs: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    if pngs.is_empty() {
        return Err("无图可拼".into());
    }
    if pngs.len() > 8 {
        return Err("对比图最多 8 列".into());
    }
    let imgs: Vec<RgbImage> = pngs
        .iter()
        .map(|p| {
            image::load_from_memory(p)
                .map_err(|e| format!("解码失败：{e}"))
                .map(|i| i.to_rgb8())
        })
        .collect::<Result<_, _>>()?;
    let h = imgs.iter().map(|i| i.height()).max().unwrap_or(0);
    let w_total: u32 = imgs.iter().map(|i| i.width()).sum();
    let mut out = RgbImage::from_pixel(w_total, h, Rgb([255, 255, 255]));
    let mut x0 = 0u32;
    for im in &imgs {
        for y in 0..im.height() {
            for x in 0..im.width() {
                out.put_pixel(x0 + x, y, *im.get_pixel(x, y));
            }
        }
        x0 += im.width();
    }
    let mut buf = Vec::new();
    out.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| format!("对比图编码失败：{e}"))?;
    Ok(buf)
}

pub fn render_to_file(scene: &Scene, width: u32, height: u32, path: &str) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err(format!("渲染尺寸必须 > 0：{width}x{height}"));
    }
    if width > 16384 || height > 16384 {
        return Err(format!("渲染尺寸超上限 16384：{width}x{height}"));
    }
    std::fs::write(path, render_review(scene, width, height)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use archcore_ir::{Geometry, Object, Transform, Units, UpAxis};
    use std::collections::BTreeMap;

    fn quad_scene() -> Scene {
        Scene {
            schema_version: "0.1".into(),
            units: Units::Mm,
            up_axis: UpAxis::ZUp,
            name: "t".into(),
            objects: vec![Object {
                id: "obj-1".into(),
                name: "墙".into(),
                kind: ObjectKind::Mesh {
                    geometry: Geometry {
                        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
                        indices: vec![0, 1, 2, 0, 2, 3],
                        normals: None,
                        uv_sets: vec![],
                    },
                    material_slots: vec![],
                },
                parent: None,
                transform: Transform::default(),
                visible: true,
                meta: BTreeMap::new(),
            }],
            materials: vec![],
            assets: vec![],
            meta: BTreeMap::new(),
        }
    }

    /// 渲染产出 PNG 且画面非单色（真画了东西）
    #[test]
    fn renders_non_trivial_png() {
        let png = render_review(&quad_scene(), 64, 64);
        let img = image::load_from_memory(&png).unwrap().to_rgb8();
        let mut colors = std::collections::BTreeSet::new();
        for p in img.pixels() {
            colors.insert((p[0] / 16, p[1] / 16, p[2] / 16));
        }
        assert!(colors.len() > 1, "画面应有几何着色变化");
    }

    /// 空场景不炸，出纯背景图
    #[test]
    fn empty_scene_renders_background() {
        let empty = Scene {
            schema_version: "0.1".into(),
            units: Units::Mm,
            up_axis: UpAxis::ZUp,
            name: "空".into(),
            objects: vec![],
            materials: vec![],
            assets: vec![],
            meta: BTreeMap::new(),
        };
        let png = render_review(&empty, 32, 32);
        assert!(image::load_from_memory(&png).is_ok());
    }

    /// 父链变换不炸：子节点挂在旋转父组下仍可渲染
    #[test]
    fn parented_transform_renders() {
        let mut s = quad_scene();
        s.objects[0].parent = Some("obj-0".into());
        s.objects.insert(
            0,
            Object {
                id: "obj-0".into(),
                name: "组".into(),
                kind: ObjectKind::Group,
                parent: None,
                transform: Transform {
                    translation: [0.5, 0.0, 0.5],
                    rotation: [0.0, 0.0, 0.3826834, 0.9238795], // 绕 Z 45°
                    scale: [1.0; 3],
                },
                visible: true,
                meta: BTreeMap::new(),
            },
        );
        let png = render_review(&s, 48, 48);
        assert!(image::load_from_memory(&png).is_ok());
    }
}
