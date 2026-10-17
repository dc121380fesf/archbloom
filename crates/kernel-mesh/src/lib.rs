//! # 网格后端（`archcore-kernel-mesh`）
//!
//! [`GeometryKernel`] 的三角网格实现：布尔借 `manifold-rust`（流形布尔，非近似）。
//!
//! 能力边界见 [`kernel`] 模块文档——倒角与 STEP **明确不支持**，如实申报而非"尽力而为"。
//! 这两样正是 B-rep 后端存在的理由。
//!
//! ```ignore
//! use archcore_kernel_mesh::{MeshKernel, shared};
//! use archcore_kernel::{GeometryKernel, BoolOp, CancelToken};
//!
//! let mut k = MeshKernel::new();
//! let a = k.insert(box_positions(), box_indices());
//! let out = k.boolean(a, a, BoolOp::Union, &CancelToken::new(), &mut |_| {})?;
//! println!("体积 {}", k.facts(out)?.volume);
//! ```

mod kernel;
mod measure;

pub use kernel::{shared, MeshKernel};
pub use measure::Measure;
