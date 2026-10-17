//! 后端能力自述：**运行时契约**，不是文档。
//!
//! # 为什么要它
//!
//! agent 的工作方式是先规划、再执行。规划需要知道「这个后端能不能做」——
//! 如果只能靠撞墙才知道，那每次失败都是一次昂贵的试错（几何布尔可能跑三十秒）。
//!
//! 有了 `Capabilities`，agent 可以先问：
//!
//! ```ignore
//! let caps = kernel.capabilities();
//! if !caps.fillet {
//!     // 提前改方案，而不是等它报 Unsupported
//! }
//! if caps.max_precision > required_tol {
//!     // 提前知道精不够，换后端或放宽要求
//! }
//! ```
//!
//! # 契约
//!
//! **申报必须与实际一致。** 声明 `fillet: true` 却返回 `Unsupported` 是 bug，
//! 不是"优雅降级"——agent 已经按你的申报规划过了。

/// 后端能力自述。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capabilities {
    /// 后端标识（如 "mesh" / "brep"），用于错误信息与 `.al` 溯源。
    pub backend: &'static str,

    /// 支持实体布尔（并/差/交）。
    pub solid_bool: bool,

    /// 布尔支持曲面（圆柱/圆锥等），而非仅平面。
    ///
    /// 单独列出来是因为它的算法复杂度差一个量级：平面-平面是线段求交，
    /// 平面-圆柱是二次曲线，圆柱-圆柱是四次空间曲线。agent 需要知道边界在哪。
    pub curved_bool: bool,

    /// 支持倒角/圆角。
    pub fillet: bool,

    /// 支持 STEP 读写。
    ///
    /// 这条决定「可出加工件」这个产品承诺能否兑现。
    pub step_io: bool,

    /// 使用精确算术（非纯浮点近似）做几何谓词。
    pub exact: bool,

    /// 能保证的最小几何偏差（米）。
    ///
    /// 网格后端通常 1e-6；精确后端可达 1e-9 或更好。调用方拿它和自己的需求比。
    pub max_precision: f64,
}

impl Capabilities {
    /// 全不支持的空壳：便于后端只覆写自己有的那几项。
    pub fn none(backend: &'static str) -> Self {
        Self {
            backend,
            solid_bool: false,
            curved_bool: false,
            fillet: false,
            step_io: false,
            exact: false,
            max_precision: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 空壳后端不冒领任何能力() {
        let c = Capabilities::none("test");
        assert_eq!(c.backend, "test");
        assert!(!c.solid_bool && !c.curved_bool && !c.fillet && !c.step_io && !c.exact);
        assert_eq!(c.max_precision, 0.0);
    }
}
