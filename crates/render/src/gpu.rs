//! GPU 路径追踪（wgpu compute）：与 CPU 内核同语义、非确定性（浮点/调度差异）。
//! 双轨制：CPU 轨保持确定性参考（回归锁定），GPU 轨为速度轨；`gpu()` 显式要求 GPU，
//! 不可用时显式报错（fail-closed，绝不静默回退）。
use crate::pathtracer::{
    encode_png, panorama_check, prepare_scene, PanoramaOpts, Prepared,
};
use crate::Tri;
use archcore_ir::Scene;
use bytemuck::Zeroable;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GTri {
    p0: [f32; 4],
    p1: [f32; 4],
    p2: [f32; 4],
    col: [f32; 4],
    nor: [f32; 4],
    uv01: [f32; 4],
    uv2m: [f32; 4], // u2, v2, tex(+1)/0, mirror
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GNode {
    bmin: [f32; 4],
    bmax: [f32; 4],
    data: [i32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GLight {
    p0: [f32; 4],
    u: [f32; 4],
    v: [f32; 4],
    n: [f32; 4],
    le: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uni {
    eye: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    fwd: [f32; 4],
    cam: [f32; 4],    // fov_scale, aspect, exposure, _
    dims: [u32; 4],   // w, h, spp, bounces
    counts: [u32; 4], // nlights, ntex, _, _
}

fn f3(v: [f64; 3]) -> [f32; 3] {
    [v[0] as f32, v[1] as f32, v[2] as f32]
}

fn pack_tri(t: &Tri) -> GTri {
    let p0 = f3(t.p[0]);
    let p1 = f3(t.p[1]);
    let p2 = f3(t.p[2]);
    let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
    let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
    let mut n = [
        e1[1] * e2[2] - e1[2] * e2[1],
        e1[2] * e2[0] - e1[0] * e2[2],
        e1[0] * e2[1] - e1[1] * e2[0],
    ];
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
    n = [n[0] / l, n[1] / l, n[2] / l];
    let (u0, v0, u1, v1, u2, v2) = match t.uv {
        Some([[a, b], [c, d], [e, f]]) => (a, b, c, d, e, f),
        None => (0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
    };
    GTri {
        p0: [p0[0], p0[1], p0[2], 0.0],
        p1: [p1[0], p1[1], p1[2], 0.0],
        p2: [p2[0], p2[1], p2[2], 0.0],
        col: [t.color[0] as f32, t.color[1] as f32, t.color[2] as f32, 0.0],
        nor: [n[0], n[1], n[2], 0.0],
        uv01: [u0 as f32, v0 as f32, u1 as f32, v1 as f32],
        uv2m: [
            u2 as f32,
            v2 as f32,
            t.tex.map(|i| i + 1).unwrap_or(0) as f32,
            if t.mirror { 1.0 } else { 0.0 },
        ],
    }
}

/// 探测 GPU 适配器（不可用返回 None——fail-closed 上报用）
pub fn gpu_adapter() -> Option<String> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;
    let info = adapter.get_info();
    Some(format!("{} ({:?})", info.name, info.backend))
}

/// 持久 GPU 上下文（设备缓存小债治本）：设备/管线建一次，缓冲/绑定组每帧上传。
/// 并发安全：全局 Mutex 串行化 GPU 提交；设备丢失 v1 显式报错不自动重建。
struct GpuCtx {
    _instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    adapter_name: String,
}

static GPU_CTX: std::sync::OnceLock<std::sync::Mutex<Option<GpuCtx>>> = std::sync::OnceLock::new();
static CTX_CREATIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 测试可见：上下文创建次数（缓存复用的回归锚）
pub fn gpu_ctx_creations() -> usize {
    CTX_CREATIONS.load(std::sync::atomic::Ordering::SeqCst)
}

fn create_ctx() -> Result<GpuCtx, String> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok_or_else(|| "GPU 渲染不可用：无可用适配器（fail-closed）".to_string())?;
    let info = adapter.get_info();
    let adapter_name = format!("{} ({:?})", info.name, info.backend);
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("archcore-gpu-pt"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
        },
        None,
    ))
    .map_err(|e| format!("GPU 设备创建失败：{e}"))?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("kernel"),
        source: wgpu::ShaderSource::Wgsl(include_str!("kernel_gpu.wgsl").into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("gpu-pt"),
        layout: None,
        module: &shader,
        entry_point: "main",
        compilation_options: Default::default(),
        cache: None,
    });
    Ok(GpuCtx {
        _instance: instance,
        device,
        queue,
        pipeline,
        adapter_name,
    })
}

fn with_ctx<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce(&GpuCtx) -> Result<T, String>,
{
    let cell = GPU_CTX.get_or_init(|| std::sync::Mutex::new(None));
    let mut guard = cell.lock().map_err(|_| "GPU 上下文锁中毒".to_string())?;
    if guard.is_none() {
        *guard = Some(create_ctx()?);
        CTX_CREATIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    f(guard.as_ref().expect("上下文已建"))
}

fn do_render(
    prep: &Prepared,
    width: u32,
    height: u32,
    spp: usize,
    exposure: f64,
    eye: [f64; 3],
    equirect: bool,
) -> Result<(image::RgbImage, String), String> {
    let flat = prep.bvh.flatten();
    let mut tris: Vec<GTri> = flat.tris.iter().map(pack_tri).collect();
    let mut nodes: Vec<GNode> = flat
        .nodes
        .iter()
        .map(|n| GNode {
            bmin: [n.bmin[0] as f32, n.bmin[1] as f32, n.bmin[2] as f32, 0.0],
            bmax: [n.bmax[0] as f32, n.bmax[1] as f32, n.bmax[2] as f32, 0.0],
            data: [n.left, n.right, n.start as i32, n.count as i32],
        })
        .collect();
    let mut lights: Vec<GLight> = prep
        .lights
        .iter()
        .map(|l| GLight {
            p0: [l.p0[0] as f32, l.p0[1] as f32, l.p0[2] as f32, 0.0],
            u: [l.u[0] as f32, l.u[1] as f32, l.u[2] as f32, 0.0],
            v: [l.v[0] as f32, l.v[1] as f32, l.v[2] as f32, 0.0],
            n: [l.n[0] as f32, l.n[1] as f32, l.n[2] as f32, 0.0],
            le: [l.le[0] as f32, l.le[1] as f32, l.le[2] as f32, 0.0],
        })
        .collect();

    // 哨兵：存储缓冲不得为 0 字节（绑定最小 16B）；节点哨兵用倒置盒保证永不命中
    if tris.is_empty() {
        tris.push(GTri::zeroed());
    }
    if nodes.is_empty() {
        nodes.push(GNode {
            bmin: [1.0, 1.0, 1.0, 0.0],
            bmax: [0.0, 0.0, 0.0, 0.0],
            data: [0, 0, 0, 0],
        });
    }
    if lights.is_empty() {
        lights.push(GLight::zeroed());
    }

    // 贴图集：hdr(offset,w,h) + 连续 rgba 数据
    let mut tex_hdr: Vec<[u32; 4]> = Vec::new();
    let mut tex_data: Vec<[f32; 4]> = Vec::new();
    for t in prep.textures.iter().flatten() {
        tex_hdr.push([tex_data.len() as u32, t.w, t.h, 0]);
        for c in &t.data {
            tex_data.push([c[0] as f32, c[1] as f32, c[2] as f32, 0.0]);
        }
    }
    if tex_hdr.is_empty() {
        tex_hdr.push([0, 0, 0, 0]);
    }
    if tex_data.is_empty() {
        tex_data.push([0.0; 4]);
    }

    let (_eye0, fwd, right, up, fov) = prep.cam;
    let uni = Uni {
        eye: [eye[0] as f32, eye[1] as f32, eye[2] as f32, 0.0],
        right: [right[0] as f32, right[1] as f32, right[2] as f32, 0.0],
        up: [up[0] as f32, up[1] as f32, up[2] as f32, 0.0],
        fwd: [fwd[0] as f32, fwd[1] as f32, fwd[2] as f32, 0.0],
        cam: [
            (fov / 2.0).tan() as f32,
            width as f32 / height as f32,
            exposure as f32,
            if equirect { 1.0 } else { 0.0 },
        ],
        dims: [width, height, spp as u32, 5],
        counts: [lights.len() as u32, tex_hdr.len() as u32, 0, 0],
    };

    with_ctx(|ctx| {
    let device = &ctx.device;
    let queue = &ctx.queue;

    use wgpu::util::DeviceExt;
    let mk = |label: &str, bytes: &[u8], usage| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytes,
            usage,
        })
    };
    let tri_buf = mk("tris", bytemuck::cast_slice(&tris), wgpu::BufferUsages::STORAGE);
    let node_buf = mk("nodes", bytemuck::cast_slice(&nodes), wgpu::BufferUsages::STORAGE);
    let light_buf = mk("lights", bytemuck::cast_slice(&lights), wgpu::BufferUsages::STORAGE);
    let th_buf = mk("tex_hdr", bytemuck::cast_slice(&tex_hdr), wgpu::BufferUsages::STORAGE);
    let td_buf = mk("tex_data", bytemuck::cast_slice(&tex_data), wgpu::BufferUsages::STORAGE);
    let uni_buf = mk("uni", bytemuck::bytes_of(&uni), wgpu::BufferUsages::UNIFORM);
    let out_size = width as u64 * height as u64 * 4; // u64 先行乘，封 u32 溢出（审计 #3）
    let out_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("out"),
        size: out_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging"),
        size: out_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bg"),
        layout: &ctx.pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: tri_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: node_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: light_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: th_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: td_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: uni_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 6, resource: out_buf.as_entire_binding() },
        ],
    });

    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&ctx.pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
    }
    encoder.copy_buffer_to_buffer(&out_buf, 0, &staging, 0, out_size);
    queue.submit(Some(encoder.finish()));

    let out_slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    out_slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("GPU 结果回读失败：{e}"))?;
    let data = out_slice.get_mapped_range().to_vec();
    staging.unmap();

    // PNG 编码（RGB8，与 CPU 轨同格式）交给调用方（全景需拼接后再编码）
    let mut img = image::RgbImage::new(width, height);
    for (i, px) in data.chunks_exact(4).enumerate() {
        let (x, y) = ((i as u32) % width, (i as u32) / width);
        img.put_pixel(x, y, image::Rgb([px[0], px[1], px[2]]));
    }
    Ok((img, ctx.adapter_name.clone()))
    })
}

/// GPU 渲染 → (PNG 字节, 适配器名)。无 GPU 显式报错。
pub fn render_pathtraced_gpu(
    scene: &Scene,
    width: u32,
    height: u32,
    spp: usize,
    exposure: f64,
) -> Result<(Vec<u8>, String), String> {
    if width == 0 || height == 0 || spp == 0 {
        return Err(format!("渲染参数非法：{width}x{height} @{spp}spp，各值必须 > 0"));
    }
    let prep = prepare_scene(scene)?;
    let (img, adapter) = do_render(&prep, width, height, spp, exposure, prep.cam.0, false)?;
    Ok((encode_png(&img), adapter))
}

/// GPU 全景渲染 → (PNG 字节, 适配器名)。stereo 时左右眼 SBS 并排。
pub fn render_panorama_gpu(
    scene: &Scene,
    opts: &PanoramaOpts,
) -> Result<(Vec<u8>, String), String> {
    panorama_check(opts)?;
    let prep = prepare_scene(scene)?;
    let eye0 = prep.cam.0;
    if !opts.stereo {
        let (img, adapter) =
            do_render(&prep, opts.width, opts.height, opts.spp, opts.exposure, eye0, true)?;
        return Ok((encode_png(&img), adapter));
    }
    let half = opts.width / 2;
    let off = [
        prep.cam.2[0] * opts.ipd / 2.0,
        prep.cam.2[1] * opts.ipd / 2.0,
        prep.cam.2[2] * opts.ipd / 2.0,
    ];
    let l_eye = [eye0[0] - off[0], eye0[1] - off[1], eye0[2] - off[2]];
    let r_eye = [eye0[0] + off[0], eye0[1] + off[1], eye0[2] + off[2]];
    let (l, adapter) = do_render(&prep, half, opts.height, opts.spp, opts.exposure, l_eye, true)?;
    let (r, _) = do_render(&prep, half, opts.height, opts.spp, opts.exposure, r_eye, true)?;
    let mut img = image::RgbImage::new(opts.width, opts.height);
    for y in 0..opts.height {
        for x in 0..half {
            img.put_pixel(x, y, *l.get_pixel(x, y));
            img.put_pixel(x + half, y, *r.get_pixel(x, y));
        }
    }
    Ok((encode_png(&img), adapter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathtracer::render_pathtraced_exposed;
    use archcore_ir::{Geometry, Material, Object, ObjectKind, ShadingModel, Transform, Units, UpAxis};
    use std::collections::BTreeMap;

    fn quad_scene() -> Scene {
        Scene {
            schema_version: "0.1".into(),
            units: Units::Mm,
            up_axis: UpAxis::ZUp,
            name: "gpu-t".into(),
            objects: vec![Object {
                id: "obj-1".into(),
                name: "板".into(),
                kind: ObjectKind::Mesh {
                    geometry: Geometry {
                        positions: vec![
                            [0.0, 0.0, 0.0],
                            [1.0, 0.0, 0.0],
                            [1.0, 1.0, 0.0],
                            [0.0, 1.0, 0.0],
                        ],
                        indices: vec![0, 2, 1, 0, 3, 2],
                        normals: None,
                        uv_sets: vec![],
                    },
                    material_slots: vec!["mat-0".into()],
                },
                parent: None,
                transform: Transform::default(),
                visible: true,
                meta: BTreeMap::new(),
            }],
            materials: vec![Material {
                id: "mat-0".into(),
                name: "M".into(),
                shading_model: ShadingModel::PbrMetallicRoughness,
                base_color: [0.8, 0.3, 0.3, 1.0],
                metallic: 0.0,
                roughness: 0.5,
                opacity: 1.0,
                textures: Default::default(),
            }],
            assets: vec![],
            meta: BTreeMap::new(),
        }
    }

    /// 设备缓存回归锚（小债治本）：两次渲染只建一次 GPU 上下文
    #[test]
    fn gpu_context_is_cached_across_renders() {
        if gpu_adapter().is_none() {
            eprintln!("skip: 无 GPU 适配器");
            return;
        }
        let scene = quad_scene();
        let _ = render_pathtraced_gpu(&scene, 32, 24, 2, 1.0).unwrap();
        let before = gpu_ctx_creations();
        let _ = render_pathtraced_gpu(&scene, 32, 24, 2, 1.0).unwrap();
        assert_eq!(gpu_ctx_creations(), before, "第二次渲染不得重建 GPU 上下文");
    }

    /// 双轨一致性：GPU 无适配器则跳过；有则与 CPU 轨做容差对比（非确定性 → 块均值容差）
    #[test]
    fn gpu_matches_cpu_within_tolerance() {
        if gpu_adapter().is_none() {
            eprintln!("skip: 无 GPU 适配器");
            return;
        }
        let scene = quad_scene();
        let cpu = render_pathtraced_exposed(&scene, 48, 36, 8, 1.0).unwrap();
        let (gpu, adapter) = render_pathtraced_gpu(&scene, 48, 36, 8, 1.0).unwrap();
        eprintln!("GPU: {adapter}");
        let ci = image::load_from_memory(&cpu).unwrap().to_rgb8();
        let gi = image::load_from_memory(&gpu).unwrap().to_rgb8();
        // 4×4 块均值容差（抹平采样噪声），逐通道平均差 < 12/255
        let mut diff = 0.0f64;
        let mut n = 0.0f64;
        for by in 0..9 {
            for bx in 0..12 {
                let (mut cs, mut gs) = ([0.0f64; 3], [0.0f64; 3]);
                for y in 0..4 {
                    for x in 0..4 {
                        let c = ci.get_pixel(bx * 4 + x, by * 4 + y).0;
                        let g = gi.get_pixel(bx * 4 + x, by * 4 + y).0;
                        for k in 0..3 {
                            cs[k] += c[k] as f64;
                            gs[k] += g[k] as f64;
                        }
                    }
                }
                for k in 0..3 {
                    diff += (cs[k] - gs[k]).abs() / 16.0;
                    n += 1.0;
                }
            }
        }
        let mean = diff / n;
        assert!(mean < 12.0, "GPU/CPU 块均值偏差过大：{mean}");
    }
}
