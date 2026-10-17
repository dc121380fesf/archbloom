//! 长操作的进度回报与取消。
//!
//! # 为什么内核需要这个
//!
//! 几何布尔可能跑三十秒。期间 agent 想知道两件事：
//! 1. **还要多久 / 现在在哪个阶段** —— 决定「再等一会儿」还是「换策略」；
//! 2. **能不能中止** —— 发现方向错了要能及时收手，且收手后状态必须干净。
//!
//! 商业内核没有这两样，因为它们假设调用者是人：人看得见进度条，人按 Ctrl-C。
//! agent 两样都没有，只能靠接口给。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 阶段化进度。
///
/// 刻意**不报告百分比**：几何算法里各阶段耗时极不均匀，百分比是假精度。
/// 报「现在在求交」比报「37%」对 agent 有用得多。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// 开始。`total_hint` 是粗估的总量（如待求交的面对数），可能不准，仅供排序。
    Start { total_hint: Option<usize> },
    /// 某个阶段推进了一步。
    Stage {
        name: &'static str,
        done: usize,
        total: usize,
    },
    /// 结束（无论成败，调用方会在返回值里看到结果）。
    Done,
}

/// 取消令牌。
///
/// 内核在长循环里周期性调用 [`is_cancelled`](Self::is_cancelled)，
/// 发现取消就**回滚到操作前状态**并返回 [`KernelError::Cancelled`](crate::KernelError::Cancelled)。
///
/// 用 `Arc<AtomicBool>` 而不是 channel：检查点非常密（可能每几微秒一次），
/// 原子读远比收发消息便宜。
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// 请求取消。可重复调用。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// 便于测试与调试：给一个已取消的令牌。
    pub fn cancelled() -> Self {
        let t = Self::new();
        t.cancel();
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 取消状态可跨克隆传递() {
        let t = CancelToken::new();
        let t2 = t.clone();
        assert!(!t2.is_cancelled());
        t.cancel();
        assert!(t2.is_cancelled(), "克隆应共享同一取消位");
    }

    #[test]
    fn 新建令牌默认未取消() {
        assert!(!CancelToken::new().is_cancelled());
        assert!(CancelToken::cancelled().is_cancelled());
    }
}
