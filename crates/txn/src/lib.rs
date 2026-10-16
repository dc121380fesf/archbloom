//! 事务层 v0 —— undo/redo/checkpoint/批量 all-or-nothing
//!
//! 硬保证：handler 即使部分写入后报错，场景也回滚到操作前（事务层兜底，不信任 handler 自觉）。
//! 检查点语义参照 SolidCraft `edit.checkpoint`：恢复检查点 = 场景 + 撤销历史一起回到当时。
//! v0 用整场景快照实现 undo（正确性优先）；优化路线（命令反操作/差分快照）留到负载实测后。
use archcore_ir::{Id, Scene};
use archcore_ops::{OpError, OpRegistry, OpReport};
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub label: String,
    pub report: OpReport,
    pub before: Scene,
    pub after: Scene,
}

#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub id: Id,
    pub label: String,
    pub scene: Scene,
    /// 检查点时刻的撤销深度（恢复时历史一并回退到此）
    pub history_len: usize,
}

#[derive(Default)]
pub struct History {
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    checkpoints: Vec<Checkpoint>,
    /// 检查点 id 单调计数器（审计 #8：len 派生在 forget 后会撞名复用）
    ckpt_seq: u64,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    /// 事务化执行：成功入史（清空 redo 分支），失败回滚且不入史
    pub fn execute(
        &mut self,
        reg: &OpRegistry,
        scene: &mut Scene,
        op_id: &str,
        args: &Value,
        label: &str,
    ) -> Result<OpReport, OpError> {
        let before = scene.clone();
        match reg.execute(scene, op_id, args) {
            Ok(report) => {
                let after = scene.clone();
                self.undo.push(HistoryEntry {
                    label: label.to_string(),
                    report: report.clone(),
                    before,
                    after,
                });
                self.redo.clear();
                Ok(report)
            }
            Err(e) => {
                // 事务兜底：即便 handler 已部分写入，也整场景回滚
                *scene = before;
                Err(e)
            }
        }
    }

    /// 批量执行：all-or-nothing，失败回滚全批并指名失败步序号（对齐 SolidCraft batch 语义）
    pub fn execute_batch(
        &mut self,
        reg: &OpRegistry,
        scene: &mut Scene,
        steps: &[(String, Value)],
    ) -> Result<Vec<OpReport>, (usize, OpError)> {
        let before = scene.clone();
        let depth = self.undo.len();
        let mut reports = Vec::new();
        for (i, (op_id, args)) in steps.iter().enumerate() {
            match self.execute(reg, scene, op_id, args, op_id) {
                Ok(r) => reports.push(r),
                Err(e) => {
                    *scene = before;
                    self.undo.truncate(depth);
                    self.redo.clear();
                    return Err((i, e));
                }
            }
        }
        Ok(reports)
    }

    /// 整批入史（preview/commit 的批量提交口）
    pub fn record_snapshot(&mut self, label: &str, report: OpReport, before: Scene, after: Scene) {
        self.undo.push(HistoryEntry {
            label: label.to_string(),
            report,
            before,
            after,
        });
        self.redo.clear();
    }

    /// 撤销：空历史显式报错（静默 no-op 是缺陷）
    pub fn undo(&mut self, scene: &mut Scene) -> Result<OpReport, OpError> {
        let entry = self.undo.pop().ok_or_else(|| OpError::Precondition {
            reason: "无可撤销操作".to_string(),
        })?;
        *scene = entry.before.clone();
        let report = entry.report.clone();
        self.redo.push(entry);
        Ok(report)
    }

    pub fn redo(&mut self, scene: &mut Scene) -> Result<OpReport, OpError> {
        let entry = self.redo.pop().ok_or_else(|| OpError::Precondition {
            reason: "无可重做操作".to_string(),
        })?;
        *scene = entry.after.clone();
        let report = entry.report.clone();
        self.undo.push(entry);
        Ok(report)
    }

    /// 记检查点：场景 + 当时的撤销深度
    pub fn checkpoint(&mut self, scene: &Scene, label: &str) -> Id {
        self.ckpt_seq += 1;
        let id = format!("ckpt-{}", self.ckpt_seq);
        self.checkpoints.push(Checkpoint {
            id: id.clone(),
            label: label.to_string(),
            scene: scene.clone(),
            history_len: self.undo.len(),
        });
        id
    }

    /// 恢复检查点：场景与撤销历史一起回到当时；forget=true 则顺手丢弃该检查点
    pub fn restore_checkpoint(
        &mut self,
        scene: &mut Scene,
        id: &str,
        forget: bool,
    ) -> Result<(), OpError> {
        let pos = self
            .checkpoints
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(|| OpError::NotFound {
                what: "checkpoint".to_string(),
                id: id.to_string(),
            })?;
        let ckpt = if forget {
            self.checkpoints.remove(pos)
        } else {
            self.checkpoints[pos].clone()
        };
        *scene = ckpt.scene;
        self.undo.truncate(ckpt.history_len);
        self.redo.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archcore_ir::{UpAxis, Units};
    use archcore_ops::register_builtins;
    use std::collections::BTreeMap;

    fn empty_scene() -> Scene {
        Scene {
            schema_version: "0.1".into(),
            units: Units::Mm,
            up_axis: UpAxis::ZUp,
            name: "t".into(),
            objects: vec![],
            materials: vec![],
            assets: vec![],
            meta: BTreeMap::new(),
        }
    }

    fn setup() -> (OpRegistry, History, Scene) {
        let mut reg = OpRegistry::new("0.1");
        register_builtins(&mut reg).unwrap();
        (reg, History::new(), empty_scene())
    }

    #[test]
    fn undo_redo_round_trip() {
        let (reg, mut hist, mut scene) = setup();
        hist.execute(&reg, &mut scene, "object.create_group", &serde_json::json!({"name":"A"}), "建组A")
            .unwrap();
        assert_eq!(scene.objects.len(), 1);
        hist.undo(&mut scene).unwrap();
        assert_eq!(scene.objects.len(), 0);
        hist.redo(&mut scene).unwrap();
        assert_eq!(scene.objects.len(), 1);
    }

    /// 批量 all-or-nothing：第 2 步失败，全批回滚且指名失败步
    #[test]
    fn batch_rolls_back_all_or_nothing() {
        let (reg, mut hist, mut scene) = setup();
        let steps = vec![
            ("object.create_group".to_string(), serde_json::json!({"name":"A"})),
            ("object.delete".to_string(), serde_json::json!({"id":"obj-404"})),
        ];
        let err = hist.execute_batch(&reg, &mut scene, &steps).unwrap_err();
        assert_eq!(err.0, 1); // 失败在第 2 步（索引 1）
        assert!(matches!(err.1, OpError::NotFound { .. }));
        assert_eq!(scene.objects.len(), 0); // 第 1 步的成果也被回滚
        assert_eq!(hist.undo_depth(), 0); // 历史干净
    }

    /// 检查点恢复 = 场景 + 撤销深度一起回去
    #[test]
    fn checkpoint_rewinds_history_too() {
        let (reg, mut hist, mut scene) = setup();
        hist.execute(&reg, &mut scene, "object.create_group", &serde_json::json!({"name":"A"}), "A").unwrap();
        let ckpt = hist.checkpoint(&scene, "存档");
        hist.execute(&reg, &mut scene, "object.create_group", &serde_json::json!({"name":"B"}), "B").unwrap();
        assert_eq!(scene.objects.len(), 2);
        assert_eq!(hist.undo_depth(), 2);
        hist.restore_checkpoint(&mut scene, &ckpt, false).unwrap();
        assert_eq!(scene.objects.len(), 1); // 场景回到检查点
        assert_eq!(hist.undo_depth(), 1); // 历史深度也回去
    }

    /// 空历史 undo 显式报错，不静默
    #[test]
    fn undo_on_empty_history_errors() {
        let (_reg, mut hist, mut scene) = setup();
        let err = hist.undo(&mut scene).unwrap_err();
        assert!(matches!(err, OpError::Precondition { .. }));
    }

    /// 失败操作不入史，场景不变
    #[test]
    fn failed_op_not_recorded() {
        let (reg, mut hist, mut scene) = setup();
        let before = scene.clone();
        hist.execute(&reg, &mut scene, "object.delete", &serde_json::json!({"id":"obj-404"}), "删")
            .unwrap_err();
        assert_eq!(scene, before);
        assert_eq!(hist.undo_depth(), 0);
    }
}
