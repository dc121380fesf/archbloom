//! Archcore 评审视口（Tauri 壳）：读 IR 场景 JSON → 喂三角形给前端 WebGL 轨道视口
use archcore_ir::Scene;
use tauri::command;

#[command]
fn read_scene(path: String) -> Result<serde_json::Value, String> {
    let data = std::fs::read(&path).map_err(|e| e.to_string())?;
    let scene: Scene = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    let (tris, _tex) = archcore_render::review_triangles(&scene);
    let out: Vec<serde_json::Value> = tris
        .iter()
        .map(|t| serde_json::json!({ "p": t.p, "color": t.color }))
        .collect();
    Ok(serde_json::json!({ "name": scene.name, "triangles": out }))
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![read_scene])
        .run(tauri::generate_context!())
        .expect("archcore-app 启动失败");
}
