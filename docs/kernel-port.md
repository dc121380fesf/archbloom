# 几何内核端口规范（`archcore-kernel`）

> 版本：0.3 ｜ 定稿：2026-10-17 ｜ 对应项目书 §3「0.3.0 目标」与 §5.1「代码风格约定」

本文是**后端接入的唯一依据**。任何人（或 agent）要实现一个几何内核后端，只需要读这一份。

---

## 一、这一层解决什么问题

引擎原来直接依赖 `archcore-mesh`（半边拓扑 + manifold 布尔），渲染和建模共用三角面。
要做机械（可出 STEP 加工）就必须有精确几何，但**不能因此把引擎绑死在某个内核上**。

所以拆出一层端口：

```
engine（只依赖 trait，不得 use 任何具体后端）
   │
archcore-kernel（本层：端口 + 类型 + 诊断）
   ▲                    ▲
kernel-mesh        kernel-brep（自研）
```

**铁律**：`engine` 里出现 `use archcore_kernel_mesh` 或 `archcore_kernel_brep` 即架构破坏。
CI 用 grep 断言守着这条。

---

## 二、六个类型

| 类型 | 作用 | 关键约束 |
|---|---|---|
| `SolidId(u64)` | 实体句柄 | **永不复用**；可序列化（进 `.al`） |
| `EdgeId(u64)` | 边句柄（倒角用） | 生命周期比 `SolidId` 短，布尔后可能失效 |
| `BoolOp` | 并/差/交 | — |
| `Tess` | 三角化产物 | 顶点与法线一一对齐 |
| `SolidFacts` | 单实体事实 | 与 `archcore_ops::GeoFacts`（场景级）不是一回事 |
| `Capabilities` | 能力自述 | **申报必须与实际一致** |

### 为什么 `SolidId` 不是 `type Handle` 关联类型

三个理由：`.al` 要序列化句柄；agent 要按 ID 寻址；engine 不能被泛型传染。
代价是后端内部要维护 `SolidId → 几何` 的 arena 映射。

---

## 三、端口契约

### 必须实现

| 方法 | 契约 |
|---|---|
| `capabilities()` | 申报必须真实；`exact=true` 必须给 `max_precision`；`curved_bool` 蕴含 `solid_bool` |
| `create_from_mesh(pos, idx)` | 把场景几何喂进内核。网格后端几乎不失败；**精确后端可能失败**（不水密/曲面无法识别），不得静默降级 |
| `facts(id)` | 未知 ID 必须报 `UnknownSolid`；不得返回空几何冒充成功 |
| `tessellate(id, tol)` | **不得静默忽略 `tol`**；非法容差（≤0 或非有限）必须报 `ToleranceExceeded` |

### 可选（有默认实现）

| 方法 | 默认行为 |
|---|---|
| `validate_mesh(&pos, &idx)` | 建一个再删掉（语义正确、不增负担）；瓶颈敏感的后端可覆写成零拷贝校验 |

### 两个核心操作

**`boolean(a, b, op, cancel, progress)`**

- `a`/`b` 不存在 → `UnknownSolid`
- 结果必须水密（`facts(...).watertight == true`）
- 被取消 → 回滚 + `Cancelled`，**不得留下半成品**
- `progress` 会被高频调用，里面不要做重活
- 相同输入必须产出相同几何（确定性）

**`drop_solid(id)`**

- 幂等：重复释放不报错、不 panic
- 释放后该 ID 永久作废

### 两个可选能力（有默认实现）

| 方法 | 默认行为 | 何时覆写 |
|---|---|---|
| `fillet(id, edges, r, cancel)` | 返回 `Unsupported` | 精确后端具备曲面能力时 |
| `to_step(id)` | 返回 `Unsupported` | 能出加工件时 |

**为什么不放进核心五方法**：默认实现不增加实现者负担，又省掉一次 trait 转型。
（项目书 §5.1-S8 说「扩展另开 trait」，实践中发现默认方法更省事且不损契约，此处以本文为准。）

---

## 四、错误设计

每个 `KernelError` 必须能回答三问：**在哪 / 为什么 / 怎么办**。

```rust
// ❌ 禁止：tuple variant
Degenerate(Point3, String)

// ✅ 强制：struct variant，带可执行建议
Degenerate { at: Point3, reason: String, suggestion: String }
```

**缺 `suggestion` 的错误视为契约未完成**——不是风格问题，是功能缺失。
agent 收到「boolean failed」什么也做不了；收到「圆柱面与平面近似相切（偏差 3.2e-7），
建议放宽容差到 1e-4」就能自己重试。

例外：`UnknownSolid` 与 `Cancelled` 不需要建议——前者要的是重新查询场景，
后者不是失败。

---

## 五、新后端接入清单

按顺序做完这七步：

1. **建 crate**：`crates/kernel-<name>`，依赖 `archcore-kernel`（以及后端自己的依赖）；
2. **实现六方法**：`capabilities` / `facts` / `tessellate` / `boolean` / `drop_solid` /
   可选 `fillet`、`to_step`；
3. **如实填 `Capabilities`**：做不到的一律 `false`，不要"尽力而为"；
4. **加一行契约测试**：在 `crates/kernel/tests/contract.rs` 末尾加
   ```rust
   #[test]
   fn <name>_backend_satisfies_contract() {
       contract_suite(&mut <Name>Kernel::new(), "<name>");
   }
   ```
5. **跑通全套契约测试**：一条不过就不要合入；
6. **补后端专属测试**：精度、性能、已知退化情形；
7. **更新本文**：在第六节的能力矩阵里加一列。

---

## 六、能力矩阵（随实现增长）

| 能力 | mesh（manifold） | brep（自研，规划中） |
|---|---|---|
| `solid_bool` | ✅ 已实现 | 🎯 计划 E 阶段 |
| `curved_bool` | ✅ 已实现 | 🎯 计划 F 阶段（圆柱-平面） |
| `fillet` | ❌ | ⏳ 延后（阶段 I） |
| `step_io` | ❌ | 🎯 计划 G 阶段 |
| `exact` | ❌（浮点） | ✅ 目标 |
| `max_precision` | 1e-6 | 1e-9 目标 |

### 端口方法清单（实际交付）

| 类别 | 方法 | 数量 |
|---|---|---|
| 必须实现 | `capabilities` / `create_from_mesh` / `facts` / `tessellate` / `boolean` / `drop_solid` | 6 |
| 默认实现 | `validate_mesh` / `fillet` / `to_step` | 3 |

> 项目书 §5.1-S8 原写「核心 5 方法」，落地时发现 `create_from_mesh` 不可省（否则几何喂不进内核），
> 实际为 **6 必须 + 3 默认**。以本表为准。

---

## 七、给 agent 的四个承诺（验收时逐条查）

1. **能先问再做**——`Capabilities` 是运行时查询，不必撞墙；
2. **失败带建议**——每个错误都有下一步；
3. **能中止**——`CancelToken` 可放弃，且放弃后状态干净；
4. **能看进度**——`Progress` 报阶段，不报假精度百分比。

---

## 八、不与渲染层纠缠

B-rep 只多一层 `tessellate`，渲染器与视口**本轮完全不改**：

```
B-rep ──tessellate(tol)──► Tess ──► 现有路径追踪 / WebGL 视口
```

视口给 `tol=0.01`（快），出图给 `tol=0.0005`（准）。同一份几何，两种精度。
