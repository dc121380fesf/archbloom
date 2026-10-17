//! 端口层的公共基础类型。
//!
//! 设计取向：
//! - 坐标一律 `f64`（与 IR 的 double 精度一致），不用 `f32`——精确几何的判定不能从降精度开始；
//! - 不引入三维向量库：端口的数学需求只有点/向量/容差，为几个四则运算引依赖不划算；
//! - 单位约定与 IR 一致（米），换算由 IO 层负责，内核只管数值。

use serde::{Deserialize, Serialize};

/// 三维坐标点（米制）。
pub type Point3 = [f64; 3];

/// 三维方向向量。
///
/// 约定：需要单位向量的接口必须在文档里写明，口默认按传入值使用，不隐式归一化
/// （隐式归一化会吞掉退化情形——零向量归一化后方向无意义）。
pub type Vector3 = [f64; 3];

/// 几何实体句柄。
///
/// # 为什么不用 `type Handle` 关联类型
///
/// 三个理由让关联类型在这里是错的：
/// 1. `.al` 存档要序列化句柄——关联类型没法序列化；
/// 2. agent 要按 ID 寻址（"把 solid#7 和 solid#9 做差集"）；
/// 3. engine 会被泛型传染成 `fn f<K: GeometryKernel>`，整条调用链跟着泛型化。
///
/// 用稳定 `u64` 三者都满足，代价是内核内部要维护 ID → 几何的 arena 映射。
///
/// # 契约
///
/// **ID 永不复用**（宪法第 3 条的几何层延伸）。删除实体后其 ID 作废，新实体必须
/// 分配新 ID——否则 agent 手里的旧引用会静默指向另一个东西，且没有任何报错。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SolidId(pub u64);

/// 边句柄（倒角、圆角按边定位）。
///
/// 与 [`SolidId`] 分开是因为边的生命周期短——一次布尔之后旧边可能不复存在，
/// 而实体 ID 要跨操作稳定。混用两者会让 agent 拿着已失效的边去倒角。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EdgeId(pub u64);

/// 布尔运算类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum BoolOp {
    Union,
    Difference,
    Intersection,
}

/// 三角化产物：渲染器与视口消费这个，不直接看 B-rep。
///
/// 顶点与法线一一对齐（`normals[i]` 属于 `positions[i]`），索引指向 `positions`。
#[derive(Clone, Debug, Default)]
pub struct Tess {
    pub positions: Vec<Point3>,
    pub normals: Vec<Vector3>,
    pub indices: Vec<u32>,
}

impl Tess {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// 单个实体的几何事实。
///
/// 与 `archcore_ops::GeoFacts` 的区别：那个是**场景级**（对象数/三角数/双哈希），
/// 这个是**单实体级**（体积/面积/拓扑计数）。两者服务于不同断言，不可互相替代。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolidFacts {
    /// 体积（立方米）。闭合实体才有意义；开放壳为 0。
    pub volume: f64,
    /// 表面积（平方米）。
    pub area: f64,
    pub face_count: usize,
    pub edge_count: usize,
    pub vertex_count: usize,
    /// 是否水密（闭合、无边界边）。加工件必须为 true。
    pub watertight: bool,
}

impl SolidFacts {
    /// 体积相对误差，用于跨后端比对（`expected` 为解析解或参考后端）。
    ///
    /// 两侧都接近零时退化为绝对误差——否则会出现 0/0。
    pub fn volume_rel_err(&self, expected: f64) -> f64 {
        let denom = expected.abs().max(1e-12);
        (self.volume - expected).abs() / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 体积相对误差_两侧都近零时不炸() {
        let f = SolidFacts {
            volume: 1e-15,
            area: 0.0,
            face_count: 0,
            edge_count: 0,
            vertex_count: 0,
            watertight: true,
        };
        let e = f.volume_rel_err(0.0);
        assert!(e.is_finite(), "0/0 必须退化为绝对误差，实际得到 {e}");
    }

    #[test]
    fn 三角数按索引计() {
        let t = Tess {
            positions: vec![[0.0; 3]; 3],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            indices: vec![0, 1, 2],
        };
        assert_eq!(t.triangle_count(), 1);
        assert!(!t.is_empty());
    }
}
