//! 几何内核端口：引擎只认这个 trait，不认任何具体后端。
//!
//! # 设计约束
//!
//! - **object-safe**：要能装进 `Box<dyn GeometryKernel>`，所以不能用泛型方法、不能用关联类型；
//! - **无状态调用**：每个方法拿参数、返回结果或错误，中间不留需要 `Check()` 的内部状态机；
//! - **`boolean` 取 `&mut self`**：它要分配新 ID 并写入内部 arena。其余查询方法取 `&self`。
//!
//! # 实现者必须守住的契约
//!
//! 1. **ID 永不复用**：删除后其 ID 作废，新实体分配新 ID；
//! 2. **确定性**：同样的输入必须产出同样的几何（固定归约顺序），agent 会重试与比对；
//! 3. **失败即回滚**：出错或取消时，内部状态必须与调用前一致；
//! 4. **申报即事实**：[`Capabilities`] 声明为 true 的能力必须真能用，否则是 bug；
//! 5. **错误带建议**：不得返回没有 `suggestion` 的诊断（见 [`KernelError`]）。

use crate::capability::Capabilities;
use crate::error::KernelError;
use crate::progress::{CancelToken, Progress};
use crate::types::{BoolOp, EdgeId, Point3, SolidFacts, SolidId, Tess};

/// 几何内核端口。
///
/// 核心五方法必须实现；`fillet` / `to_step` 是可选能力，不支持时保留默认实现即可
/// （会返回 [`KernelError::Unsupported`]）。
pub trait GeometryKernel: Send + Sync {
    /// 能力自述。agent 靠它做规划，不靠撞墙。
    fn capabilities(&self) -> Capabilities;

    /// 从三角网格构造实体——这是把场景几何喂进内核的唯一入口。
    ///
    /// # 各后端的语义差异（调用方必须容忍）
    ///
    /// - **网格后端**：直接存下，几乎不会失败；
    /// - **精确后端**：需要从三角面重建解析曲面，**可能失败**（非水密、
    ///   无法识别的曲面类型）。这正是它必须返回 `Result` 的原因。
    ///
    /// 实现方不得静默降级（比如「重建失败了就当网格存」）——那样会给
    /// 调用方一个错误的精确性预期。做不到就如实报错。
    fn create_from_mesh(
        &mut self,
        positions: Vec<Point3>,
        indices: Vec<u32>,
    ) -> Result<SolidId, KernelError>;

    /// 只校验不保留：这个网格本后端能不能接受。
    ///
    /// 默认实现就是「建一个再删掉」——语义正确（能建就能用），且不增加后端负担。
    /// 瓶颈敏感的后端可以覆写成零拷贝校验。
    ///
    /// 为什么它属于内核而不是 IR：**可接受标准因后端而异**。网格后端只查流形，
    /// B-rep 后端还要求水密与曲面可识别。放在 IR 层就等于用一套标准卡两种表示。
    fn validate_mesh(
        &mut self,
        positions: &[Point3],
        indices: &[u32],
    ) -> Result<(), KernelError> {
        let id = self.create_from_mesh(positions.to_vec(), indices.to_vec())?;
        self.drop_solid(id);
        Ok(())
    }

    /// 单实体的几何事实（体积/面积/拓扑计数/水密性）。
    ///
    /// 这个方法是 agent 的**感知器官**：每一步操作后读一次，就知道自己改了什么。
    fn facts(&self, id: SolidId) -> Result<SolidFacts, KernelError>;

    /// 三角化：把实体转成渲染器吃得下的网格。
    ///
    /// `tol` 是允许的弦高偏差（米）。视口可以给 0.01（快），出图给 0.0005（准）。
    /// 实现方按容差决定细分密度，不得静默忽略 `tol`。
    fn tessellate(&self, id: SolidId, tol: f64) -> Result<Tess, KernelError>;

    /// 布尔运算。
    ///
    /// # 契约
    ///
    /// - `a` / `b` 必须存在，否则返回 [`KernelError::UnknownSolid`]；
    /// - 结果实体必须水密（`facts(...).watertight == true`）；
    /// - 被取消时回滚并返回 [`KernelError::Cancelled`]；
    /// - `progress` 允许被高频调用，实现方不要在里面做重活。
    fn boolean(
        &mut self,
        a: SolidId,
        b: SolidId,
        op: BoolOp,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<SolidId, KernelError>;

    /// 释放实体。不存在时静默返回（幂等）——重复释放不该报错。
    fn drop_solid(&mut self, id: SolidId);

    /// 可选：平面倒角（chamfer）。
    ///
    /// `edges` 为空表示「所有锐边」；非空时只倒指定的边。
    /// `width` 是从原棱起算、向两侧面各切进的尺寸（米）。
    ///
    /// 默认返回不支持。几何上倒角比布尔更难（平移面 + 重建环 + 补角面），
    /// 网格后端通常做不到。
    fn chamfer(
        &mut self,
        _id: SolidId,
        _edges: &[EdgeId],
        _width: f64,
        _cancel: &CancelToken,
    ) -> Result<SolidId, KernelError> {
        Err(KernelError::Unsupported {
            feature: "chamfer",
            alternatives: vec!["用网格后端并手动构造斜面", "切换到精确后端"],
        })
    }

    /// 可选：圆角（fillet）。
    ///
    /// 默认返回不支持。圆角比倒角更难：需要圆柱面 + 圆弧边界 + 曲面求交，
    /// 只有精确后端才有能力，而且实现成本高一个量级。
    fn fillet(
        &mut self,
        _id: SolidId,
        _edges: &[EdgeId],
        _radius: f64,
        _cancel: &CancelToken,
    ) -> Result<SolidId, KernelError> {
        Err(KernelError::Unsupported {
            feature: "fillet",
            alternatives: vec!["改用 chamfer（平面切口）", "切换到精确后端"],
        })
    }

    /// 可选：导出 STEP。
    ///
    /// 默认返回不支持。这条能力是否具备，直接决定「可出加工件」这个产品承诺。
    fn to_step(&self, _id: SolidId) -> Result<String, KernelError> {
        Err(KernelError::Unsupported {
            feature: "step_io",
            alternatives: vec!["导出 STL 做 3D 打印", "切换到精确后端"],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小后端：只用来验证端口本身可被实现、trait object 可装。
    struct Stub {
        alive: Vec<SolidId>,
        next: u64,
    }

    impl GeometryKernel for Stub {
        fn capabilities(&self) -> Capabilities {
            Capabilities::none("stub")
        }

        fn create_from_mesh(
            &mut self,
            _positions: Vec<[f64; 3]>,
            _indices: Vec<u32>,
        ) -> Result<SolidId, KernelError> {
            let id = SolidId(self.next);
            self.next += 1;
            self.alive.push(id);
            Ok(id)
        }

        fn facts(&self, id: SolidId) -> Result<SolidFacts, KernelError> {
            if !self.alive.contains(&id) {
                return Err(KernelError::UnknownSolid { id });
            }
            Ok(SolidFacts {
                volume: 0.0,
                area: 0.0,
                face_count: 0,
                edge_count: 0,
                vertex_count: 0,
                watertight: true,
            })
        }

        fn tessellate(&self, _id: SolidId, _tol: f64) -> Result<Tess, KernelError> {
            Ok(Tess::default())
        }

        fn boolean(
            &mut self,
            a: SolidId,
            _b: SolidId,
            _op: BoolOp,
            cancel: &CancelToken,
            _progress: &mut dyn FnMut(Progress),
        ) -> Result<SolidId, KernelError> {
            if cancel.is_cancelled() {
                return Err(KernelError::Cancelled);
            }
            if !self.alive.contains(&a) {
                return Err(KernelError::UnknownSolid { id: a });
            }
            let id = SolidId(self.next);
            self.next += 1;
            self.alive.push(id);
            Ok(id)
        }

        fn drop_solid(&mut self, id: SolidId) {
            self.alive.retain(|x| *x != id);
        }
    }

    #[test]
    fn 端口可被实现且可装箱为trait对象() {
        let mut boxed: Box<dyn GeometryKernel> = Box::new(Stub {
            alive: vec![],
            next: 1,
        });
        assert_eq!(boxed.capabilities().backend, "stub");

        let a = SolidId(0);
        boxed.drop_solid(a); // 幂等，不存在也不报错
        let err = boxed.facts(a).unwrap_err();
        assert!(matches!(err, KernelError::UnknownSolid { .. }));
    }

    #[test]
    fn 取消时布尔回滚不产实体() {
        let mut k = Stub {
            alive: vec![SolidId(0)],
            next: 1,
        };
        let before = k.alive.len();
        let err = k
            .boolean(
                SolidId(0),
                SolidId(0),
                BoolOp::Union,
                &CancelToken::cancelled(),
                &mut |_| {},
            )
            .unwrap_err();
        assert!(matches!(err, KernelError::Cancelled));
        assert_eq!(k.alive.len(), before, "取消不得留下半成品");
    }

    #[test]
    fn 默认能力如实报不支持() {
        let mut k = Stub {
            alive: vec![],
            next: 0,
        };
        let err = k.to_step(SolidId(0)).unwrap_err();
        assert!(matches!(
            err,
            KernelError::Unsupported {
                feature: "step_io",
                ..
            }
        ));
        let err = k.fillet(SolidId(0), &[], 0.1, &CancelToken::new()).unwrap_err();
        assert!(matches!(
            err,
            KernelError::Unsupported {
                feature: "fillet",
                ..
            }
        ));
    }
}
