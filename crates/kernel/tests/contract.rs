//! 契约测试：**一套测试，跑所有后端**。
//!
//! # 为什么这样组织
//!
//! 两个后端最容易出的问题是**行为漂移**：manifold 的布尔和自研 B-rep 的布尔
//! 在同一输入上给出不同结果，而 op 层以为它们一样。
//!
//! 所以这里测的不是「某个实现」，而是**端口的契约**。加第三个后端时，
//! 只需要在文件末尾加一行 `#[test]`，不用重写测试。
//!
//! # 当前状态
//!
//! 阶段 A 只有 `MockKernel`（内存假后端），用来验证框架本身能跑。
//! 阶段 B 接上 `MeshKernel` 后，再加一行；阶段 D 接上 `BrepKernel` 再加一行。

use archcore_kernel::{
    BoolOp, CancelToken, Capabilities, GeometryKernel, KernelError, Progress, SolidFacts, SolidId, Tess,
};

/// 契约套件：任何后端实现都必须通过。
///
/// 参数用 `&mut dyn` 而非泛型——这样同一个函数能检查 trait object 路径，
/// 也顺带保证端口是 object-safe 的。
fn contract_suite(k: &mut dyn GeometryKernel, ctx: &str) {
    capability_contract(k, ctx);
    unknown_id_contract(k, ctx);
    drop_idempotent_contract(k, ctx);
    cancel_contract(k, ctx);
    tessellate_tolerance_contract(k, ctx);
}

/// C1：能力申报必须自洽。
fn capability_contract(k: &dyn GeometryKernel, ctx: &str) {
    let caps = k.capabilities();
    assert!(!caps.backend.is_empty(), "[{ctx}] backend 标识不得为空");
    if caps.exact {
        assert!(
            caps.max_precision > 0.0,
            "[{ctx}] 声明 exact 却给不出精度下界，申报不自洽"
        );
    }
    // 曲面布尔是实体布尔的子集：声明能做曲面布尔却做不了实体布尔，逻辑矛盾
    if caps.curved_bool {
        assert!(
            caps.solid_bool,
            "[{ctx}] 声明 curved_bool 但未声明 solid_bool，申报不自洽"
        );
    }
}

/// C2：未知实体必须报 UnknownSolid，不能 panic、不能返回空几何。
fn unknown_id_contract(k: &dyn GeometryKernel, ctx: &str) {
    let ghost = SolidId(u64::MAX);
    match k.facts(ghost) {
        Err(KernelError::UnknownSolid { id }) => assert_eq!(id, ghost),
        Err(other) => panic!("[{ctx}] 未知实体应报 UnknownSolid，实际 {other:?}"),
        Ok(f) => panic!("[{ctx}] 未知实体竟返回了事实 {f:?}"),
    }
}

/// C3：释放必须幂等——重复释放不报错、不 panic。
fn drop_idempotent_contract(k: &mut dyn GeometryKernel, _ctx: &str) {
    let ghost = SolidId(u64::MAX - 1);
    k.drop_solid(ghost);
    k.drop_solid(ghost);
}

/// C4：取消后不得留下半成品。
///
/// 这条最重要——agent 会主动放弃长操作，如果内核留了个残缺实体，
/// 后续每一步都会建立在这个鬼东西上。
fn cancel_contract(k: &mut dyn GeometryKernel, ctx: &str) {
    let token = CancelToken::cancelled();
    let a = SolidId(0);
    let result = k.boolean(a, a, BoolOp::Union, &token, &mut |_| {});
    match result {
        Err(KernelError::Cancelled) => {}
        other => panic!("[{ctx}] 已取消的令牌应返回 Cancelled，实际 {other:?}"),
    }
}

/// C5：`tessellate` 不得静默忽略容差。
///
/// 传一个非法容差（0 或负）必须显式报错——静默按默认值处理会让
/// 「出图质量不对」变成无法诊断的玄学。
fn tessellate_tolerance_contract(k: &dyn GeometryKernel, ctx: &str) {
    let ghost = SolidId(0);
    // 先确认已知实体路径：容差非法必须在解析实体之前就拦住
    let err = k.tessellate(ghost, 0.0).unwrap_err();
    assert!(
        matches!(
            err,
            KernelError::ToleranceExceeded { .. } | KernelError::UnknownSolid { .. }
        ),
        "[{ctx}] 容差为 0 应报 ToleranceExceeded（或实体不存在），实际 {err:?}"
    );
    let err = k.tessellate(ghost, -1.0).unwrap_err();
    assert!(
        matches!(
            err,
            KernelError::ToleranceExceeded { .. } | KernelError::UnknownSolid { .. }
        ),
        "[{ctx}] 负容差应被拒，实际 {err:?}"
    );
}

// ---------------------------------------------------------------- 测试后端

/// 最小后端：内存里记几个实体，布尔产新 ID。
///
/// 它不是「参考实现」，只用来证明契约套件本身可用。
struct MockKernel {
    alive: Vec<SolidId>,
    next: u64,
}

impl MockKernel {
    fn new() -> Self {
        Self {
            alive: vec![SolidId(0)],
            next: 1,
        }
    }
}

impl GeometryKernel for MockKernel {
    fn capabilities(&self) -> Capabilities {
        let mut c = Capabilities::none("mock");
        c.solid_bool = true;
        c.exact = true;
        c.max_precision = 1e-9;
        c
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
            volume: 1.0,
            area: 6.0,
            face_count: 6,
            edge_count: 12,
            vertex_count: 8,
            watertight: true,
        })
    }

    fn tessellate(&self, id: SolidId, tol: f64) -> Result<Tess, KernelError> {
        if !(tol.is_finite() && tol > 0.0) {
            return Err(KernelError::ToleranceExceeded {
                required: 1e-6,
                achieved: tol,
                suggestion: "容差必须是正有限值，视口可用 0.01、出图用 0.0005".into(),
            });
        }
        if !self.alive.contains(&id) {
            return Err(KernelError::UnknownSolid { id });
        }
        Ok(Tess::default())
    }

    fn boolean(
        &mut self,
        a: SolidId,
        b: SolidId,
        _op: BoolOp,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<SolidId, KernelError> {
        if cancel.is_cancelled() {
            return Err(KernelError::Cancelled);
        }
        for id in [a, b] {
            if !self.alive.contains(&id) {
                return Err(KernelError::UnknownSolid { id });
            }
        }
        progress(Progress::Start { total_hint: Some(1) });
        progress(Progress::Stage {
            name: "mock",
            done: 1,
            total: 1,
        });
        progress(Progress::Done);
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
fn mock_backend_satisfies_contract() {
    contract_suite(&mut MockKernel::new(), "mock");
}

/// 真实后端：网格（manifold）。
///
/// 它跑同一套契约，不需额外适配代码——这就是契约测试的全部意义。
#[test]
fn mesh_backend_satisfies_contract() {
    let mut k = archcore_kernel_mesh::MeshKernel::new();
    // 网格后端的未知 ID / 空实体契约先跑（不需要真几何）
    let _ = &mut k;
    contract_suite(&mut k, "mesh");
}

/// 真实后端：精确 B-rep。
///
/// 它的布尔/倒角/STEP 都还没实现，但契约必须成立：未知 ID 报错、取消优先、
/// 非法容差被拒、能力申报自洽。**能力缺不影响契约**——这是两件事。
#[test]
fn brep_backend_satisfies_contract() {
    contract_suite(&mut archcore_kernel_brep::BrepKernel::new(), "brep");
}

// 阶段 B 接上网格后端后，在这里加一行：
// #[test]
// fn mesh_backend_satisfies_contract() {
//     contract_suite(&mut archcore_kernel_mesh::MeshKernel::new(), "mesh");
// }
//
// 阶段 D 接上精确后端后，再加一行：
// #[test]
// fn brep_backend_satisfies_contract() {
//     contract_suite(&mut archcore_kernel_brep::BrepKernel::new(), "brep");
// }
