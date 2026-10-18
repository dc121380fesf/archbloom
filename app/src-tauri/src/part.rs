//! 零件工作台：把 B-rep 内核的能力暴露给界面。
//!
//! 与建筑场景平行——建筑场景走 `Scene`（语义模型：墙/板/柱），
//! 零件工作台走 `BrepKernel`（精确 B-rep：面/边/曲面）。
//!
//! # 分工
//!
//! 带 `tauri::State` 的 `part_*` 命令只做「取锁 + 转发」，
//! 真正的逻辑在 [`Workbench`] 上——它能脱离 Tauri 单独测试，
//! 这也是 `tests` 模块能端到端跑通内核链路的底气。
//!
//! ```text
//! 新建（盒子 / 圆柱）→ 打孔 / 布尔 → 合并共面 → 导出 STEP
//! ```

use std::sync::Mutex;

use archcore_kernel::{BoolOp, CancelToken, GeometryKernel, SolidId};
use archcore_kernel_brep::{merge_coplanar, to_step_curved, BrepKernel, Solid};
use serde_json::{json, Value};
use tauri::{command, State};

/// 工作台：内核 + 当前零件。与 Tauri 无关，可以单独测。
pub struct Workbench {
    kernel: BrepKernel,
    active: Option<SolidId>,
    last_step: String,
}

impl Default for Workbench {
    fn default() -> Self {
        Self::new()
    }
}

impl Workbench {
    pub fn new() -> Self {
        Self {
            kernel: BrepKernel::new(),
            active: None,
            last_step: String::new(),
        }
    }

    /// 新建零件：盒子或圆柱。尺寸用**毫米**，内核内部是米。
    pub fn new_part(&mut self, kind: &str, params: &[f64]) -> Result<Value, String> {
        let label = match kind {
            "box" => {
                let [dx, dy, dz] = take3(params)?;
                if dx <= 0.0 || dy <= 0.0 || dz <= 0.0 {
                    return Err("尺寸必须大于 0".to_string());
                }
                let id = self
                    .kernel
                    .add_box([0.0, 0.0, 0.0], [dx / 1000.0, dy / 1000.0, dz / 1000.0]);
                self.active = Some(id);
                format!("盒子 {dx}×{dy}×{dz} mm")
            }
            "cylinder" => {
                let [d, h] = take2(params)?;
                if d <= 0.0 || h <= 0.0 {
                    return Err("尺寸必须大于 0".to_string());
                }
                let id = self
                    .kernel
                    .add_cylinder([0.0, 0.0, 0.0], d / 2000.0, h / 1000.0, 64);
                self.active = Some(id);
                format!("圆柱 Φ{d} × {h} mm")
            }
            other => return Err(format!("未知零件类型：{other}")),
        };
        self.last_step = "新建".to_string();
        let id = self.active.ok_or("未创建零件")?;
        self.info(id, &label)
    }

    /// 沿 Z 轴居中打一个通孔。
    pub fn drill(&mut self, diameter: f64) -> Result<Value, String> {
        if diameter <= 0.0 {
            return Err("孔径必须大于 0".to_string());
        }
        let base = self.active.ok_or("先新建一个零件")?;
        let (min, max) = {
            let s = self.kernel.get(base).ok_or("零件不存在")?;
            bbox(s)
        };
        let cx = (min[0] + max[0]) / 2.0;
        let cy = (min[1] + max[1]) / 2.0;
        let thickness = max[2] - min[2];
        // 刀具从底面下方进场、长度覆盖整个厚度，保证贯通
        let r = diameter / 2000.0;
        let drill = self.kernel.add_cylinder(
            [cx, cy, min[2] - thickness * 0.25],
            r,
            thickness * 1.5,
            64,
        );
        let mut noop = |_p: archcore_kernel::Progress| {};
        let id = self
            .kernel
            .boolean(base, drill, BoolOp::Difference, &CancelToken::new(), &mut noop)
            .map_err(|e| format!("打孔失败：{e}"))?;
        self.active = Some(id);
        self.last_step = "打孔".to_string();
        self.info(id, &format!("Φ{diameter} 通孔"))
    }

    /// 合并共面/共曲面面片。
    pub fn merge(&mut self) -> Result<Value, String> {
        let base = self.active.ok_or("先新建一个零件")?;
        let s = self.kernel.get(base).ok_or("零件不存在")?;
        let (merged, stats) = merge_coplanar(s).map_err(|e| format!("合并失败：{e}"))?;
        let id = self.kernel.insert(merged);
        self.active = Some(id);
        self.last_step = format!("合并 {} → {} 面", stats.faces_before, stats.faces_after);
        let step = self.last_step.clone();
        self.info(id, &step)
    }

    /// 导出 STEP 文本（真曲面）。
    pub fn step_text(&self, name: &str) -> Result<(String, String), String> {
        let base = self.active.ok_or("先新建一个零件")?;
        let s = self.kernel.get(base).ok_or("零件不存在")?;
        let stem = if name.trim().is_empty() {
            "part".to_string()
        } else {
            name.trim().to_string()
        };
        Ok((to_step_curved(s, &stem), stem))
    }

    /// 把实体指标打包成 JSON。
    fn info(&self, id: SolidId, label: &str) -> Result<Value, String> {
        let s = self.kernel.get(id).ok_or_else(|| format!("零件 {label} 不存在"))?;
        let (pos, _, idx) = s.triangulate();
        let vol = archcore_kernel_brep::volume_from_mesh(&pos, &idx);
        let (min, max) = bbox(s);
        Ok(json!({
            "label": label,
            "step": self.last_step,
            "faces": s.faces.len(),
            "edges": s.edges.len(),
            "halfEdges": s.half_edges.len(),
            "verts": s.verts.len(),
            "volumeMm3": vol * 1e9,
            "euler": s.euler_characteristic(),
            "watertight": s.is_watertight(),
            "boundaryHalfEdges": s.boundary_half_edges(),
            "holedFaces": s.faces.iter().filter(|f| !f.holes.is_empty()).count(),
            "curvedFaces": s
                .faces
                .iter()
                .filter(|f| !matches!(f.surface, archcore_kernel_brep::Surface::Plane { .. }))
                .count(),
            "sizeMm": [
                (max[0] - min[0]) * 1000.0,
                (max[1] - min[1]) * 1000.0,
                (max[2] - min[2]) * 1000.0,
            ],
        }))
    }
}

fn bbox(s: &Solid) -> ([f64; 3], [f64; 3]) {
    if s.verts.is_empty() {
        return ([0.0; 3], [0.0; 3]);
    }
    let mut min = [f64::MAX; 3];
    let mut max = [f64::MIN; 3];
    for p in &s.verts {
        for k in 0..3 {
            if p[k] < min[k] {
                min[k] = p[k];
            }
            if p[k] > max[k] {
                max[k] = p[k];
            }
        }
    }
    (min, max)
}

fn take2(p: &[f64]) -> Result<[f64; 2], String> {
    if p.len() < 2 {
        return Err("需要 2 个参数".to_string());
    }
    Ok([p[0], p[1]])
}

fn take3(p: &[f64]) -> Result<[f64; 3], String> {
    if p.len() < 3 {
        return Err("需要 3 个参数".to_string());
    }
    Ok([p[0], p[1], p[2]])
}

// ─────────────────────────── Tauri 命令（只做转发） ───────────────────────────

pub struct PartState(pub Mutex<Workbench>);

impl Default for PartState {
    fn default() -> Self {
        Self(Mutex::new(Workbench::new()))
    }
}

fn with_wb<T>(
    state: &State<'_, PartState>,
    f: impl FnOnce(&mut Workbench) -> Result<T, String>,
) -> Result<T, String> {
    let mut wb = state.0.lock().map_err(|e| e.to_string())?;
    f(&mut wb)
}

#[command]
pub fn part_new(
    state: State<'_, PartState>,
    kind: String,
    params: Vec<f64>,
) -> Result<Value, String> {
    with_wb(&state, |wb| wb.new_part(&kind, &params))
}

#[command]
pub fn part_drill(state: State<'_, PartState>, diameter: f64) -> Result<Value, String> {
    with_wb(&state, |wb| wb.drill(diameter))
}

#[command]
pub fn part_merge(state: State<'_, PartState>) -> Result<Value, String> {
    with_wb(&state, |wb| wb.merge())
}

#[command]
pub async fn part_export_step(
    app: tauri::AppHandle,
    state: State<'_, PartState>,
    name: String,
) -> Result<Value, String> {
    use tauri_plugin_dialog::DialogExt;

    let (step, stem) = with_wb(&state, |wb| wb.step_text(&name))?;

    let default_name = format!("{stem}.step");
    let picked = app
        .dialog()
        .file()
        .set_title("导出 STEP")
        .set_file_name(&default_name)
        .add_filter("STEP AP203 (.step)", &["step", "stp"])
        .blocking_save_file();
    let Some(sel) = picked else {
        return Ok(json!({ "cancelled": true }));
    };
    let path = sel.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, step.as_bytes()).map_err(|e| format!("写文件失败：{e}"))?;
    Ok(json!({
        "cancelled": false,
        "path": path.display().to_string(),
        "bytes": step.len(),
    }))
}

#[command]
pub fn part_reset(state: State<'_, PartState>) -> Result<Value, String> {
    let mut wb = state.0.lock().map_err(|e| e.to_string())?;
    *wb = Workbench::new();
    Ok(json!({ "ok": true }))
}

// ─────────────────────────── 端到端测试 ───────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 盒子 → 打孔 → 合并 → 导出 STEP，整条链路走一遍。
    #[test]
    fn workbench_end_to_end() {
        let mut wb = Workbench::new();

        let d = wb.new_part("box", &[20.0, 20.0, 5.0]).expect("新建盒子");
        assert_eq!(d["faces"], 6, "盒子应有 6 个面");
        assert_eq!(d["watertight"], true);
        assert_eq!(d["euler"], 2, "盒子是球面，欧拉 2");
        let v: f64 = d["volumeMm3"].as_f64().unwrap();
        assert!((v - 2000.0).abs() < 1e-6, "盒子体积应为 2000 mm³，实际 {v}");

        let d = wb.drill(10.0).expect("打孔");
        let v: f64 = d["volumeMm3"].as_f64().unwrap();
        // 2000 - π·5²·5 ≈ 1607.3
        assert!((v - 1607.3).abs() < 1.0, "开孔板体积应约 1607.3 mm³，实际 {v}");
        assert_eq!(d["watertight"], true, "打孔后必须水密");
        assert_eq!(d["euler"], 0, "带一通孔应为环面，欧拉 0");

        let d = wb.merge().expect("合并");
        let faces = d["faces"].as_u64().unwrap();
        assert!(faces < 100, "合并后面数应显著下降，实际 {faces}");
        assert_eq!(d["watertight"], true, "合并后必须水密");
        assert_eq!(d["euler"], 0, "合并后仍是环面");

        let (step, stem) = wb.step_text("part").expect("导出");
        assert_eq!(stem, "part");
        assert!(step.starts_with("ISO-10303-21"), "STEP 头部不对");
        assert!(step.contains("END-ISO-10303-21"), "STEP 尾部不对");
        assert!(step.contains("CIRCLE"), "真曲面路径应写出圆");
    }

    /// 圆柱：曲面面片不该被合并成平面。
    #[test]
    fn workbench_cylinder_keeps_curved_faces() {
        let mut wb = Workbench::new();
        let d = wb.new_part("cylinder", &[10.0, 20.0]).expect("新建圆柱");
        // 新建时固定用 64 段（与 `Workbench::new_part` 一致）
        assert_eq!(d["curvedFaces"], 64, "圆柱侧面 64 片都应标为曲面");
        let v: f64 = d["volumeMm3"].as_f64().unwrap();
        // 64 段多边形内接圆柱 ≈ 1568.4 mm³，真圆是 1570.8（π·5²·20）。
        // 差 0.16% 是**多边形近似的固有损失**，不是 bug。
        let ideal = std::f64::consts::PI * 25.0 * 20.0;
        assert!(
            (v - ideal).abs() / ideal < 3e-3,
            "圆柱体积应接近 {ideal:.1} mm³（±0.3%），实际 {v}"
        );
    }

    /// 非法参数要明确报错，不能悄悄产出坏实体。
    #[test]
    fn workbench_rejects_bad_input() {
        let mut wb = Workbench::new();
        assert!(wb.new_part("box", &[0.0, 20.0, 5.0]).is_err(), "零尺寸应被拒");
        assert!(wb.new_part("sphere", &[10.0]).is_err(), "未知类型应被拒");
        assert!(wb.drill(10.0).is_err(), "没有活动零件时应报错");
        assert!(wb.step_text("x").is_err(), "没有活动零件时应报错");
    }
}
