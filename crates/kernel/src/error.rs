//! 端口层错误：**为 agent 设计，不是为编译器设计**。
//!
//! # 为什么长这样
//!
//! 老几何内核报 `"boolean failed"`，agent 收到这句话什么也做不了——不知道哪一步、
//! 为什么、能不能换个参数重试。
//!
//! 所以这里的每个变体都必须能回答三问：
//! - **在哪**：`at`（点 / 拓扑位置 / 实体 ID）
//! - **为什么**：`reason`
//! - **怎么办**：`suggestion`（agent 靠它决定重试方向）
//!
//! 缺 `suggestion` 的错误视为**契约未完成**——不是风格问题，是功能缺失。
//!
//! # 为什么禁用 tuple variant
//!
//! `Degenerate(Point3, String)` 三个月后没人记得第二个参数是什么。结构化字段自解释。

use std::fmt;

use crate::types::{Point3, SolidId};

#[derive(Debug, Clone)]
pub enum KernelError {
    /// 几何退化：算出来的东西没有体积或面积为 0。
    Degenerate {
        at: Point3,
        reason: String,
        suggestion: String,
    },

    /// 精度不足：当前容差不足以判定内外、切干净或稳住拓扑。
    ToleranceExceeded {
        required: f64,
        achieved: f64,
        suggestion: String,
    },

    /// 后端不支持该能力。
    ///
    /// 正常流程里这种事应该被 [`Capabilities`](crate::Capabilities) 提前拦下；
    /// 走到这里说明调用方跳过了能力查询，或者能力声明与实际不符（后者是 bug）。
    Unsupported {
        feature: &'static str,
        alternatives: Vec<&'static str>,
    },

    /// 拓扑损坏：面/环/壳不自洽（非流形边、悬挂面、壳未闭合）。
    TopologyBroken {
        at: String,
        reason: String,
        suggestion: String,
    },

    /// 实体 ID 不存在或已随删除失效。
    ///
    /// 这个变体刻意不带 suggestion：agent 需要做的是**重新查询当前场景**，
    /// 而不是改参数重试。给一句"检查 ID 是否已被删除"足够。
    UnknownSolid { id: SolidId },

    /// 操作被主动取消。
    ///
    /// 这不是错误——用户或 agent 决定不做了。调用方应当把它与真失败区分开，
    /// 不要打印成红字，也不要重试。
    Cancelled,
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Degenerate {
                at,
                reason,
                suggestion,
            } => write!(
                f,
                "几何退化（位置 {:.6},{:.6},{:.6}）：{reason}。建议：{suggestion}",
                at[0], at[1], at[2]
            ),
            Self::ToleranceExceeded {
                required,
                achieved,
                suggestion,
            } => write!(
                f,
                "精度不足：需要 {required:.3e}，实际 {achieved:.3e}。建议：{suggestion}"
            ),
            Self::Unsupported {
                feature,
                alternatives,
            } => {
                write!(f, "后端不支持 {feature}")?;
                if !alternatives.is_empty() {
                    write!(f, "；可选做法：{}", alternatives.join(" 或 "))?;
                }
                Ok(())
            }
            Self::TopologyBroken {
                at,
                reason,
                suggestion,
            } => write!(f, "拓扑损坏（{at}）：{reason}。建议：{suggestion}"),
            Self::UnknownSolid { id } => write!(
                f,
                "实体 #{} 不存在（可能已被删除）。建议：重新查询当前场景取有效 ID",
                id.0
            ),
            Self::Cancelled => write!(f, "操作已取消"),
        }
    }
}

impl std::error::Error for KernelError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 每个变体的文案都能指导下一步() {
        let cases = [
            KernelError::Degenerate {
                at: [0.0, 0.0, 0.0],
                reason: "两个面完全重合".into(),
                suggestion: "先做 0.1mm 平移再布尔".into(),
            },
            KernelError::ToleranceExceeded {
                required: 1e-6,
                achieved: 3.2e-7,
                suggestion: "放宽容差到 1e-4".into(),
            },
            KernelError::Unsupported {
                feature: "fillet",
                alternatives: vec!["改用 chamfer"],
            },
            KernelError::TopologyBroken {
                at: "face#3".into(),
                reason: "壳未闭合".into(),
                suggestion: "检查输入是否水密".into(),
            },
            KernelError::UnknownSolid { id: SolidId(42) },
            KernelError::Cancelled,
        ];
        for c in cases {
            let s = c.to_string();
            assert!(!s.is_empty());
            // 除 UnknownSolid / Cancelled / Unsupported 外，文案里必须出现"建议"
            let needs_suggestion = matches!(
                c,
                KernelError::Degenerate { .. }
                    | KernelError::ToleranceExceeded { .. }
                    | KernelError::TopologyBroken { .. }
            );
            if needs_suggestion {
                assert!(s.contains("建议"), "缺建议即契约未完成：{s}");
            }
        }
    }
}
