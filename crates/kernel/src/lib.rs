//! # 几何内核端口（`archcore-kernel`）
//!
//! 引擎只依赖本 crate 的 trait，不依赖任何具体后端。后端实现放在
//! `kernel-mesh`（网格，包 manifold）与 `kernel-brep`（自研精确内核）。
//!
//! ```text
//! engine（只依赖 trait）
//!    │
//! archcore-kernel（本 crate：端口 + 类型 + 诊断）
//!    ▲                    ▲
//! kernel-mesh        kernel-brep
//! ```
//!
//! **不要在本 crate 里放几何算法。** 它只定义契约、类型和错误语义。
//!
//! ## 给 agent 的四个承诺
//!
//! 1. **能先问再做**——[`Capabilities`] 是运行时查询，不必撞墙才知道后端能不能倒角；
//! 2. **失败带建议**——[`KernelError`] 每个变体都给位置、原因、下一步；
//! 3. **能中止**——[`CancelToken`] 让长操作可以被放弃，且放弃后状态干净；
//! 4. **能看进度**——[`Progress`] 报阶段而不是假精度百分比。
//!
//! 设计取舍与代码风格约定见 `docs/kernel-port.md` 与项目书 §5.1。

mod capability;
mod error;
mod port;
mod progress;
mod types;

pub use capability::Capabilities;
pub use error::KernelError;
pub use port::GeometryKernel;
pub use progress::{CancelToken, Progress};
pub use types::{BoolOp, EdgeId, Point3, SolidFacts, SolidId, Tess, Vector3};

/// 端口版本号。后端实现应据此声明兼容性。
pub const PORT_VERSION: (u32, u32) = (0, 3);
