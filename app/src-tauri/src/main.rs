//! Archbloom 评审视口（Tauri 壳）：IR 场景 → 高保真视口 + 出图面板
//!
//! 视口取材与路径追踪同源（review_triangles）：三角形带 UV/贴图索引/材质语义；
//! 出图面板走 render_scene：预设 view/pano × 渲染轨 cpu/gpu/auto，产物落场景旁。
use archcore_ir::Scene;
use base64::Engine as _;
use tauri::command;

fn load_scene(path: &str) -> Result<Scene, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice(&data).map_err(|e| e.to_string())
}

/// 视口取材：三角形（含 UV/贴图索引/镜面语义）+ 贴图路径表
#[command]
fn read_scene(path: String) -> Result<serde_json::Value, String> {
    let scene = load_scene(&path)?;
    let (tris, tex_paths) = archcore_render::review_triangles(&scene);
    let out: Vec<serde_json::Value> = tris
        .iter()
        .map(|t| {
            serde_json::json!({ "p": t.p, "color": t.color, "mirror": t.mirror, "uv": t.uv, "tex": t.tex })
        })
        .collect();
    Ok(serde_json::json!({ "name": scene.name, "triangles": out, "textures": tex_paths }))
}

/// 读贴图文件 → data URL（视口纹理上传）
#[command]
fn read_texture(path: String) -> Result<String, String> {
    let data = std::fs::read(&path).map_err(|e| e.to_string())?;
    let mime = match path.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "png" => "image/png",
        _ => "image/jpeg",
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
    Ok(format!("data:{mime};base64,{b64}"))
}

/// 出图面板后端：预设 view|pano × 轨 cpu|gpu|auto（auto=有 GPU 适配器即用，轨迹显式回报）
#[command]
fn render_scene(
    path: String,
    preset: String,
    width: u32,
    height: u32,
    spp: usize,
    renderer: String,
) -> Result<serde_json::Value, String> {
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(format!("渲染尺寸非法: {width}x{height}"));
    }
    if spp == 0 || spp > 4096 {
        return Err(format!("spp 非法: {spp}"));
    }
    let scene = load_scene(&path)?;
    let started = std::time::Instant::now();
    let use_gpu = match renderer.as_str() {
        "cpu" => false,
        "gpu" => true,
        _ => archcore_render::gpu_adapter().is_some(),
    };
    let (png, track) = match preset.as_str() {
        "view" => {
            if use_gpu {
                let (png, adapter) =
                    archcore_render::render_pathtraced_gpu(&scene, width, height, spp, 1.0)?;
                (png, format!("gpu · {adapter}"))
            } else {
                (archcore_render::render_pathtraced(&scene, width, height, spp)?, "cpu".into())
            }
        }
        "pano" => {
            // 全景恒为等距柱状 2:1，高度按半宽收敛（panorama_check 同口径）
            let opts = archcore_render::PanoramaOpts {
                width,
                height: width / 2,
                spp,
                exposure: 1.0,
                stereo: false,
                ipd: 0.065,
            };
            if use_gpu {
                let (png, adapter) = archcore_render::render_panorama_gpu(&scene, &opts)?;
                (png, format!("gpu · {adapter}"))
            } else {
                (archcore_render::render_panorama(&scene, &opts)?, "cpu".into())
            }
        }
        other => return Err(format!("未知预设: {other}")),
    };
    let scene_path = std::path::Path::new(&path);
    let dir = scene_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let stem = scene_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("scene");
    let out = dir.join(format!("{stem}_{preset}_{width}x{height}_{spp}spp.png"));
    std::fs::write(&out, &png).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "path": out.display().to_string(),
        "elapsed_ms": started.elapsed().as_millis() as u64,
        "track": track,
    }))
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![read_scene, read_texture, render_scene])
        .run(tauri::generate_context!())
        .expect("archbloom-app 启动失败");
}
