//! Archbloom 工作台壳：文档模型（新建 / 打开 / 保存 / 另存为 / 导出）+ 高保真视口 + 出图面板
//!
//! 文档模型：后端持有「当前文档」状态（路径 + 场景），前端只做展示与命令派发。
//! 文件对话框一律由 Rust 侧通过 tauri-plugin-dialog 弹出（前端是纯静态页，不引 npm 包）。
use archcore_ir::{Scene, Units, UpAxis};
use base64::Engine as _;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{command, Manager, State};
use tauri_plugin_dialog::DialogExt;

/// 当前打开的文档
struct DocState {
    path: Option<PathBuf>,
    scene: Scene,
    /// 自上次保存/载入后是否被改动（GUI 暂无编辑命令，为后续编辑面预留）
    dirty: bool,
}

impl Default for DocState {
    fn default() -> Self {
        Self {
            path: None,
            scene: empty_scene(),
            dirty: false,
        }
    }
}

impl DocState {
    /// 丢弃它会痛吗？——场景非空且（改动过 或 从未存过盘）
    fn has_unsaved_work(&self) -> bool {
        !self.scene.objects.is_empty() && (self.dirty || self.path.is_none())
    }
}

/// 丢弃前拦一道：返回 true 表示用户确认丢弃
async fn confirm_discard(app: &tauri::AppHandle, what: &str) -> bool {
    app.dialog()
        .message(format!(
            "当前场景尚未保存，继续{what}会丢弃它。\n\n先保存再继续比较稳当。"
        ))
        .title("有未保存的场景")
        .kind(tauri_plugin_dialog::MessageDialogKind::Warning)
        .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancel)
        .blocking_show()
}

fn empty_scene() -> Scene {
    Scene {
        schema_version: "0".into(),
        units: Units::M,
        up_axis: UpAxis::ZUp,
        name: "未命名".into(),
        objects: Vec::new(),
        materials: Vec::new(),
        assets: Vec::new(),
        meta: Default::default(),
    }
}

/// 载入场景，并把「不是场景文件」的情况翻译成人话；`.al` 自包含存档自动解包
fn load_scene(path: &str) -> Result<Scene, String> {
    let data = std::fs::read(path).map_err(|e| format!("读文件失败（{path}）：{e}"))?;
    if path.to_ascii_lowercase().ends_with(".al") {
        let dir = archcore_io::archive::default_extract_dir(&data);
        return archcore_io::archive::open(&data, &dir).map_err(|e| format!("存档解包失败：{e}"));
    }
    let err = match serde_json::from_slice::<Scene>(&data) {
        Ok(scene) => return Ok(scene),
        Err(e) => e,
    };
    // 解析失败时做一次结构诊断，别把 serde 原文抛给用户
    let hint = match serde_json::from_slice::<serde_json::Value>(&data) {
        Ok(v) if v.get("ops").is_some() => {
            "这是 op 脚本（顶层是 ops 数组），不是场景文件。脚本请用 archbloom-run 执行；GUI 里要载入的是场景 JSON 或 .al 存档。"
        }
        Ok(v) if v.get("compose").is_some() => {
            "这是 compose 简报，不是场景文件。请先用 archbloom-run 直出，再载入产出的场景。"
        }
        Ok(_) => "该 JSON 不是 Archbloom 场景：缺少 schema_version / objects 等必需字段。",
        Err(_) => "文件不是合法 JSON（可能已损坏或不是 JSON 文件）。",
    };
    Err(format!("{hint}\n底层错误：{err}"))
}

/// 写文档：按扩展名分派（`.al` 走自包含存档，其余按场景 JSON 写）。
/// 返回打包警告（有贴图缺失时非空，但**保存本身已成功**——不谎报失败）。
fn write_doc(scene: &Scene, path: &Path) -> Result<Vec<String>, String> {
    if let Some(p) = path.parent() {
        if !p.as_os_str().is_empty() {
            std::fs::create_dir_all(p)
                .map_err(|e| format!("创建目录失败（{}）：{e}", p.display()))?;
        }
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "al" {
        let report = archcore_io::archive::pack(scene, path).map_err(|e| format!("{e}"))?;
        Ok(report.warnings)
    } else {
        let text = serde_json::to_string_pretty(scene)
            .map_err(|e| format!("序列化失败：{e}"))?;
        std::fs::write(path, text).map_err(|e| format!("写入失败（{}）：{e}", path.display()))?;
        Ok(Vec::new())
    }
}

fn doc_payload(d: &DocState) -> Value {
    json!({
        "path": d.path.as_ref().map(|p| p.display().to_string()),
        "name": d.scene.name,
        "objects": d.scene.objects.len(),
        "materials": d.scene.materials.len(),
        "assets": d.scene.assets.len(),
        "dirty": d.dirty,
    })
}

/// 窗口标题跟随文档：`名称 — Archbloom`，未保存时带标记
fn sync_title(window: &tauri::WebviewWindow, d: &DocState) {
    // 未保存用星号标记（桌面软件惯例）
    let mark = if d.dirty || d.path.is_none() { "*" } else { "" };
    let _ = window.set_title(&format!("{}{} — Archbloom", d.scene.name, mark));
}

// ---------------------------------------------------------------- 文件命令

/// 新建：清空为空白场景（有未保存内容时先确认）
#[command]
async fn new_doc(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, Mutex<DocState>>,
) -> Result<Value, String> {
    let need_confirm = {
        let d = state.lock().map_err(|e| e.to_string())?;
        d.has_unsaved_work()
    };
    if need_confirm && !confirm_discard(&app, "新建").await {
        return Ok(json!({"cancelled": true}));
    }
    let mut d = state.lock().map_err(|e| e.to_string())?;
    d.path = None;
    d.scene = empty_scene();
    d.dirty = false;
    sync_title(&window, &d);
    Ok(doc_payload(&d))
}

/// 打开：原生文件对话框 → 载入
#[command]
async fn open_doc(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, Mutex<DocState>>,
) -> Result<Value, String> {
    let need_confirm = {
        let d = state.lock().map_err(|e| e.to_string())?;
        d.has_unsaved_work()
    };
    if need_confirm && !confirm_discard(&app, "打开其他文件").await {
        return Ok(json!({"cancelled": true}));
    }
    let picked = app
        .dialog()
        .file()
        .set_title("打开 Archbloom 场景")
        .add_filter("Archbloom 存档 / 场景", &["al", "json"])
        .blocking_pick_file();
    let Some(sel) = picked else {
        return Ok(json!({"cancelled": true}));
    };
    let path = sel
        .as_path()
        .ok_or_else(|| "对话框返回了非本地路径".to_string())?
        .to_path_buf();
    let scene = load_scene(&path.display().to_string())?;
    let mut d = state.lock().map_err(|e| e.to_string())?;
    d.scene = scene;
    d.path = Some(path.clone());
    d.dirty = false;
    push_recent(&app, &path);
    sync_title(&window, &d);
    Ok(doc_payload(&d))
}

/// 保存：写回当前路径；没有路径（新建后未存过）则走另存为
#[command]
async fn save_doc(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, Mutex<DocState>>,
) -> Result<Value, String> {
    let existing = {
        let d = state.lock().map_err(|e| e.to_string())?;
        d.path.clone()
    };
    if let Some(path) = existing {
        let warnings = {
            let d = state.lock().map_err(|e| e.to_string())?;
            write_doc(&d.scene, &path)?
        };
        {
            let mut d = state.lock().map_err(|e| e.to_string())?;
            d.dirty = false;
            sync_title(&window, &d);
        }
        return Ok(json!({"path": path.display().to_string(), "warnings": warnings}));
    }
    save_doc_as(app, window, state).await
}

/// 另存为：原生对话框选路径，按扩展名决定存成 `.al` 还是场景 JSON
#[command]
async fn save_doc_as(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, Mutex<DocState>>,
) -> Result<Value, String> {
    let default_name = {
        let d = state.lock().map_err(|e| e.to_string())?;
        format!("{}.al", d.scene.name)
    };
    let picked = app
        .dialog()
        .file()
        .set_title("另存为")
        .set_file_name(&default_name)
        .add_filter("Archbloom 存档 (.al)", &["al"])
        .add_filter("场景 JSON (.json)", &["json"])
        .blocking_save_file();
    let Some(sel) = picked else {
        return Ok(json!({"cancelled": true}));
    };
    let mut path = sel
        .as_path()
        .ok_or_else(|| "对话框返回了非本地路径".to_string())?
        .to_path_buf();
    // 用户没写扩展名时补 .al（私有存档是默认格式）
    if path.extension().is_none() {
        path.set_extension("al");
    }
    let warnings = {
        let d = state.lock().map_err(|e| e.to_string())?;
        write_doc(&d.scene, &path)?
    };
    let mut d = state.lock().map_err(|e| e.to_string())?;
    d.path = Some(path.clone());
    d.dirty = false;
    push_recent(&app, &path);
    sync_title(&window, &d);
    Ok(json!({"path": path.display().to_string(), "warnings": warnings}))
}

/// 导出：模型格式（gltf / glb / fbx / ifc / html）。渲染图片走 render_scene。
#[command]
async fn export_scene(
    app: tauri::AppHandle,
    state: State<'_, Mutex<DocState>>,
    kind: String,
) -> Result<Value, String> {
    let (filter_label, exts, default_name, stem) = {
        let d = state.lock().map_err(|e| e.to_string())?;
        let stem = d.scene.name.clone();
        let stem = if stem.trim().is_empty() { "scene".to_string() } else { stem };
        match kind.as_str() {
            "gltf" => ("glTF 2.0 (.gltf)", vec!["gltf"], format!("{stem}.gltf"), stem),
            "glb" => ("binary glTF (.glb)", vec!["glb"], format!("{stem}.glb"), stem),
            "fbx" => ("FBX (.fbx)", vec!["fbx"], format!("{stem}.fbx"), stem),
            "ifc" => ("IFC4 (.ifc)", vec!["ifc"], format!("{stem}.ifc"), stem),
            "html" => ("审阅页 (.html)", vec!["html"], format!("{stem}_review.html"), stem),
            other => return Err(format!("未知导出格式：{other}")),
        }
    };

    let picked = app
        .dialog()
        .file()
        .set_title(format!("导出为 {filter_label}"))
        .set_file_name(&default_name)
        .add_filter(filter_label, &exts)
        .blocking_save_file();
    let Some(sel) = picked else {
        return Ok(json!({"cancelled": true}));
    };
    let mut path = sel
        .as_path()
        .ok_or_else(|| "对话框返回了非本地路径".to_string())?
        .to_path_buf();
    if path.extension().is_none() {
        path.set_extension(exts[0]);
    }

    let (scene, doc_path) = {
        let d = state.lock().map_err(|e| e.to_string())?;
        (d.scene.clone(), d.path.clone())
    };

    match kind.as_str() {
        "gltf" => {
            let bytes = archcore_io::export_gltf(&scene);
            std::fs::write(&path, bytes)
                .map_err(|e| format!("写入失败（{}）：{e}", path.display()))?;
        }
        "glb" => {
            let bytes =
                archcore_io::export_glb(&scene).map_err(|e| format!("GLB 打包失败：{e}"))?;
            std::fs::write(&path, bytes)
                .map_err(|e| format!("写入失败（{}）：{e}", path.display()))?;
        }
        "ifc" => {
            let text = archcore_io::export_ifc(&scene);
            std::fs::write(&path, text)
                .map_err(|e| format!("写入失败（{}）：{e}", path.display()))?;
        }
        "html" => {
            let html = archcore_render::export_web(&scene, &stem)
                .map_err(|e| format!("审阅页生成失败：{e}"))?;
            std::fs::write(&path, html)
                .map_err(|e| format!("写入失败（{}）：{e}", path.display()))?;
        }
        "fbx" => {
            let blender = resolve_blender(&app).await?;
            archcore_io::export_fbx(&scene, &blender, &path).map_err(|e| {
                format!(
                    "FBX 导出失败（Blender: {}）：{e}",
                    blender.display()
                )
            })?;
        }
        _ => unreachable!(),
    }

    Ok(json!({
        "path": path.display().to_string(),
        "kind": kind,
        "source": doc_path.map(|p| p.display().to_string()),
    }))
}

// ---------------------------------------------------------------- 视口命令

/// 视口取材：当前文档的三角形（含 UV/贴图索引/镜面语义）+ 贴图路径表
#[command]
fn read_scene(state: State<'_, Mutex<DocState>>) -> Result<Value, String> {
    let d = state.lock().map_err(|e| e.to_string())?;
    let (tris, tex_paths) = archcore_render::review_triangles(&d.scene);
    let out: Vec<Value> = tris
        .iter()
        .map(|t| json!({ "p": t.p, "color": t.color, "mirror": t.mirror, "uv": t.uv, "tex": t.tex }))
        .collect();
    Ok(json!({
        "name": d.scene.name,
        "triangles": out,
        "textures": tex_paths,
        "path": d.path.as_ref().map(|p| p.display().to_string()),
    }))
}

/// 读贴图文件 → data URL（视口纹理上传）
#[command]
fn read_texture(path: String) -> Result<String, String> {
    let data = std::fs::read(&path).map_err(|e| format!("读贴图失败（{path}）：{e}"))?;
    let mime = match path.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "png" => "image/png",
        _ => "image/jpeg",
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
    Ok(format!("data:{mime};base64,{b64}"))
}

/// 出图：预设 view|pano × 轨 cpu|gpu|auto。
/// `out_path` 为空时落到「文档同目录」，文档未存过则落桌面。
///
/// 参数虽多，但都是前端面板的独立旋钮（预设/宽/高/采样/渲染轨/输出路径），
/// 拆结构体只会让 IPC 契约多一层嵌套，得不偿失。
#[command]
#[allow(clippy::too_many_arguments)]
async fn render_scene(
    app: tauri::AppHandle,
    state: State<'_, Mutex<DocState>>,
    preset: String,
    width: u32,
    height: u32,
    spp: usize,
    renderer: String,
    out_path: Option<String>,
) -> Result<Value, String> {
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(format!("渲染尺寸非法：{width}x{height}"));
    }
    if spp == 0 || spp > 4096 {
        return Err(format!("spp 非法：{spp}"));
    }
    let (scene, doc_path) = {
        let d = state.lock().map_err(|e| e.to_string())?;
        (d.scene.clone(), d.path.clone())
    };

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
                (
                    archcore_render::render_pathtraced(&scene, width, height, spp)?,
                    "cpu".into(),
                )
            }
        }
        "pano" => {
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
        other => return Err(format!("未知预设：{other}")),
    };

    let out = match out_path {
        Some(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => {
            let stem = doc_path
                .as_ref()
                .and_then(|p| p.file_stem())
                .and_then(|s| s.to_str())
                .unwrap_or("scene")
                .to_string();
            let dir = match doc_path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
                Some(d) => d,
                None => app
                    .path()
                    .desktop_dir()
                    .map_err(|e| format!("找不到桌面目录：{e}"))?,
            };
            dir.join(format!("{stem}_{preset}_{width}x{height}_{spp}spp.png"))
        }
    };
    if let Some(p) = out.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    std::fs::write(&out, &png).map_err(|e| format!("写出图失败（{}）：{e}", out.display()))?;
    Ok(json!({
        "path": out.display().to_string(),
        "elapsed_ms": started.elapsed().as_millis() as u64,
        "track": track,
    }))
}

/// 选一个保存路径（供导出渲染图片这类「先选位置再执行」的流程用）
#[command]
async fn pick_save_path(
    app: tauri::AppHandle,
    default_name: String,
    filter_label: String,
    exts: Vec<String>,
) -> Result<Value, String> {
    let ext_refs: Vec<&str> = exts.iter().map(|s| s.as_str()).collect();
    let picked = app
        .dialog()
        .file()
        .set_title(format!("保存为 {filter_label}"))
        .set_file_name(&default_name)
        .add_filter(filter_label, &ext_refs)
        .blocking_save_file();
    match picked {
        None => Ok(json!({"cancelled": true})),
        Some(sel) => {
            let mut path = sel
                .as_path()
                .ok_or_else(|| "对话框返回了非本地路径".to_string())?
                .to_path_buf();
            if path.extension().is_none() {
                if let Some(e) = exts.first() {
                    path.set_extension(e);
                }
            }
            Ok(json!({"path": path.display().to_string()}))
        }
    }
}

// ---------------------------------------------------------------- 设置与最近打开

const RECENT_MAX: usize = 8;

fn read_settings(app: &tauri::AppHandle) -> Value {
    settings_file(app)
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or_else(|| json!({}))
}

fn write_settings(app: &tauri::AppHandle, v: &Value) {
    if let Some(f) = settings_file(app) {
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(text) = serde_json::to_string_pretty(v) {
            let _ = std::fs::write(f, text);
        }
    }
}

fn push_recent(app: &tauri::AppHandle, path: &Path) {
    let mut s = read_settings(app);
    let p = path.display().to_string();
    let mut list: Vec<String> = s
        .get("recent")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    list.retain(|x| x != &p);
    list.insert(0, p);
    list.truncate(RECENT_MAX);
    s["recent"] = json!(list);
    write_settings(app, &s);
}

/// 最近打开的文件（已消失的不返回，免得菜单里堆死链）
#[command]
fn recent_files(app: tauri::AppHandle) -> Value {
    let s = read_settings(&app);
    let list: Vec<Value> = s
        .get("recent")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .filter(|p| Path::new(p).is_file())
                .map(|p| {
                    json!({
                        "path": p,
                        "name": Path::new(p)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(p),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!(list)
}

/// 按路径打开（最近打开列表用）：与对话框版共享未保存确认
#[command]
async fn open_recent(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, Mutex<DocState>>,
    path: String,
) -> Result<Value, String> {
    let need_confirm = {
        let d = state.lock().map_err(|e| e.to_string())?;
        d.has_unsaved_work()
    };
    if need_confirm && !confirm_discard(&app, "打开其他文件").await {
        return Ok(json!({"cancelled": true}));
    }
    let p = PathBuf::from(&path);
    if !p.is_file() {
        return Err(format!("文件已不存在：{path}"));
    }
    let scene = load_scene(&path)?;
    let mut d = state.lock().map_err(|e| e.to_string())?;
    d.scene = scene;
    d.path = Some(p.clone());
    d.dirty = false;
    push_recent(&app, &p);
    sync_title(&window, &d);
    Ok(doc_payload(&d))
}

// ---------------------------------------------------------------- 外部依赖解析

fn settings_file(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("settings.json"))
}

fn load_blender_from_settings(app: &tauri::AppHandle) -> Option<PathBuf> {
    let v = read_settings(app);
    let pb = PathBuf::from(v.get("blender")?.as_str()?);
    if pb.is_file() { Some(pb) } else { None }
}

fn save_blender_to_settings(app: &tauri::AppHandle, path: &Path) {
    let mut s = read_settings(app);
    s["blender"] = json!(path.display().to_string());
    write_settings(app, &s);
}

/// 自动探测 Blender：环境变量 → PATH → 标准安装目录（取版本号最大者）
fn probe_blender() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ARCHCORE_BLENDER") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    if let Ok(out) = std::process::Command::new("where").arg("blender").output() {
        if out.status.success() {
            if let Some(line) = String::from_utf8_lossy(&out.stdout).lines().next() {
                let pb = PathBuf::from(line.trim());
                if pb.is_file() {
                    return Some(pb);
                }
            }
        }
    }
    for base in [
        "C:\\Program Files\\Blender Foundation",
        "C:\\Program Files (x86)\\Blender Foundation",
    ] {
        if let Ok(rd) = std::fs::read_dir(base) {
            let mut hits: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path().join("blender.exe"))
                .filter(|p| p.is_file())
                .collect();
            hits.sort();
            if let Some(p) = hits.pop() {
                return Some(p);
            }
        }
    }
    None
}

/// 解析 Blender 路径：配置 → 探测 → 首次让用户指一次（并记住）
async fn resolve_blender(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Some(p) = load_blender_from_settings(app) {
        return Ok(p);
    }
    if let Some(p) = probe_blender() {
        save_blender_to_settings(app, &p);
        return Ok(p);
    }
    let picked = app
        .dialog()
        .file()
        .set_title("首次使用 FBX 导出：请定位 blender.exe（只需选一次，之后会记住）")
        .add_filter("Blender 可执行文件", &["exe"])
        .blocking_pick_file();
    match picked {
        Some(sel) => {
            let p = sel
                .as_path()
                .ok_or_else(|| "对话框返回了非本地路径".to_string())?
                .to_path_buf();
            save_blender_to_settings(app, &p);
            Ok(p)
        }
        None => Err("未选择 Blender，FBX 导出已取消".to_string()),
    }
}

/// 当前文档信息（标题栏 / 状态栏用）
#[command]
fn doc_info(
    window: tauri::WebviewWindow,
    state: State<'_, Mutex<DocState>>,
) -> Result<Value, String> {
    let d = state.lock().map_err(|e| e.to_string())?;
    sync_title(&window, &d);
    Ok(doc_payload(&d))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Mutex::new(DocState::default()))
        .on_window_event(|window, event| {
            // 关窗前拦一道：有未保存内容就弹确认（确认后才真关）
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let need = window
                    .state::<Mutex<DocState>>()
                    .lock()
                    .map(|d| d.has_unsaved_work())
                    .unwrap_or(false);
                if need {
                    api.prevent_close();
                    let w = window.clone();
                    window
                        .app_handle()
                        .dialog()
                        .message("当前场景尚未保存，关闭会丢弃它。")
                        .title("有未保存的场景")
                        .kind(tauri_plugin_dialog::MessageDialogKind::Warning)
                        .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancel)
                        .show(move |ok| {
                            if ok {
                                let _ = w.destroy();
                            }
                        });
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            new_doc,
            open_doc,
            open_recent,
            recent_files,
            save_doc,
            save_doc_as,
            export_scene,
            pick_save_path,
            read_scene,
            read_texture,
            render_scene,
            doc_info
        ])
        .run(tauri::generate_context!())
        .expect("archbloom-app 启动失败");
}
