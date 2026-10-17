//! # 精确几何内核（`archcore-kernel-brep`）
//!
//! 自研窄 B-rep：**平面 / 圆柱 / 圆锥 / 球 / 圆环** + **直线 / 圆弧 / 椭圆弧**。
//! 不做自由曲面（NURBS）、放样、扫掠——范围写死在项目书 §2「重启记录」里。
//!
//! # 与网格后端的根本区别
//!
//! | | 网格（manifold） | 精确（本 crate） |
//! |---|---|---|
//! | 几何本质 | 三角面逼近 | 解析公式 |
//! | 体积 | 量出来的 | **算出来的**（闭式解） |
//! | 倒角 / STEP | 做不到 | 路线上的下一步 |
//!
//! 这就是「可出 STEP 加工」的技术前提：加工端要的是**定义**（Φ10 的孔），不是 32 个三角面。
//!
//! # 当前进度（D 阶段）
//!
//! ✅ 解析几何（五种曲面 + 三种曲线）、实体拓扑、闭式体积/面积、三角化
//! ⏳ 布尔（E）、平面-圆柱布尔（F）、STEP 输出（G）、倒角（I）
//!
//! 未实现的能力一律在 [`Capabilities`](archcore_kernel::Capabilities) 里报 `false`，
//! 调用时返回 `Unsupported`，**不静默降级**。
//!
//! # 用法
//!
//! ```ignore
//! use archcore_kernel_brep::BrepKernel;
//! use archcore_kernel::{GeometryKernel, CancelToken};
//!
//! let mut k = BrepKernel::new();
//! let id = k.add_cylinder([0.0, 0.0, 0.0], 0.005, 0.02, 64); // Φ10 × 20mm
//! let f = k.facts(id)?;
//! // f.volume 是 π·r²·h 的闭式解，不是从三角面量出来的
//! ```

mod bsp;
mod csg;
mod geom;
mod kernel;
mod rebuild;
mod solid;
mod step;
mod stl;

pub use bsp::{difference, intersection, union, Node, Plane, Polygon};
pub use csg::{point_inside, select_faces, split_all};
pub use geom::{Curve, Surface};
pub use kernel::{shared, BrepKernel};
pub use rebuild::{polygons_to_solid, solid_to_polygons, WELD_TOL};
pub use solid::{
    box_solid, cylinder_solid, segments_for_tolerance, volume_from_mesh, Face, Solid,
};
pub use stl::{to_stl_ascii, verify_against_solid};
pub use step::{to_step, verify_against_solid as verify_step_against_solid};
