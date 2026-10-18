# 半边结构重构（A 方案）

> 起因：倒角、面片合并、圆角三个功能都栽在「边靠推导、不靠存证」上。
> 十轮圆角尝试每轮修一层，就是因为这一层缺数据结构。

## 现状的问题

```rust
struct Solid {
    verts: Vec<Point3>,
    faces: Vec<Face>,   // Face = { surface, verts: Vec<u32> }
}
```

**没有边**。三个模块各建一套临时边：

```rust
chamfer.rs: fn edge_table(solid) -> Vec<EdgeRec>
merge.rs:   fn ring_edges(ring) -> Vec<(u32,u32)>
fillet.rs:  let mut rails: HashMap<(u32,u32), Rails>
```

边 = 「两个顶点号组成的有序对」，身份是**推导**出来的。两个面如果不是同一次算出
来的，索引就不保证相同——只能靠 weld 合并坐标，浮点一抖就合不上。

## 目标结构

```rust
pub struct Solid {
    pub verts: Vec<Point3>,
    pub faces: Vec<Face>,
    pub half_edges: Vec<HalfEdge>,
    pub edges: Vec<Edge>,
}

pub struct HalfEdge {
    pub origin: u32,     // 起点顶点
    pub twin: usize,     // 反向半边（同一个 Edge 的另一侧）
    pub next: usize,     // 环上的下一条半边
    pub edge: usize,     // 所属 Edge
}

pub struct Edge {
    pub he: (usize, usize),   // 两个半边，互为 twin
}

pub struct Face {
    pub surface: Surface,
    pub first_half_edge: usize,   // 环的入口
    pub holes: Vec<usize>,        // 内环入口
}
```

**关键**：边的身份 = `edges` 数组里的下标。两个面共享边就是共享同一个 `Edge` 对象，
不可能"对不上"。

## 怎么迁移（分四步，每步可编译可测试）

### 第一步：加结构，不动现有代码

- 新增 `half_edges: Vec<HalfEdge>`、`edges: Vec<Edge>`
- `Solid::build_topology()` 从现有 `Face.verts` 构建半边表
- `Face` 保留 `verts`（先不删），新增 `first_half_edge`
- **这一步不改任何调用方**，254 测试必须继续绿

### 第二步：拓扑自检

- `check_topology()`：每条边的两个半边必须 twin 对称、next 闭合、每个顶点至少一条出边
- 作为契据测试：任何几何操作后都跑一遍

### 第三步：圆角改用半边

- 轨线端点不再"算坐标 + put!"，而是**在边上分裂顶点、插入圆角面**
- 圆角面的边界直接引用现有的 HalfEdge
- 这是真正解决问题的部分

### 第四步：其余模块逐步迁移

- chamfer / merge 用半边替代临时边表
- STEP 输出用半边（天然有方向，`ORIENTED_EDGE` 不用再推）

## 风险与对策

| 风险 | 对策 |
|---|---|
| 改 `Face.verts` 会波及 90+ 处 | 第一步**不删** `verts`，只是并行维护半边 |
| 254 测试可能挂 | 每步跑全量，设闸门 |
| 半边表与 `verts` 不同步 | `build_topology()` 唯一入口 + `check_topology()` 自检 |

## 验收口径

1. 第一步后：**254 测试全绿**，`check_topology()` 对新构造的盒子/圆柱为 true
2. 第三步后：盒子全边圆角**水密 true、欧拉 2、体积误差 < 0.1%**
3. 第四步后：`Capabilities.fillet = true`，chamfer 的布尔件问题一并验证

## 为什么值得做

不只是为了圆角：

- 倒角的布尔件失败（孔壁碎片对不上）
- 面片合并的洞边界错（环划分靠面积猜）
- STEP 的 `ORIENTED_EDGE` 靠环顺序推方向

**四个地方都在补同一块洞。** 补一次，四处受益。
