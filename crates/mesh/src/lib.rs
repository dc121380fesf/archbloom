//! 拓扑内核 v0 —— 三角半边结构：数据结构 + 遍历 + 编辑算子
//!
//! 混合方案的自研半区（红线：不碰算法层——布尔/简化/谓词全部外借）。
//! v0 范围：arena 只增不删（删除引入 ABA，需代数索引，升级路径另记）；
//! 编辑算子只做 edge_flip（rule of three：等 op 层真用到再加 split/collapse）。
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HalfEdgeId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FaceId(pub u32);

#[derive(Clone, Debug)]
pub struct HalfEdge {
    pub origin: u32,
    pub twin: Option<HalfEdgeId>,
    pub next: HalfEdgeId,
    pub face: FaceId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshError {
    DegenerateFace { a: u32, b: u32, c: u32 },
    VertexOutOfRange { index: u32 },
    NonManifoldEdge { x: u32, y: u32 },
    BoundaryEdge { edge: HalfEdgeId },
    EmptyResult,
    UnchangedResult,
    Corrupt { reason: String },
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeshError::DegenerateFace { a, b, c } => write!(f, "退化面 ({a},{b},{c})"),
            MeshError::VertexOutOfRange { index } => write!(f, "顶点越界 {index}"),
            MeshError::NonManifoldEdge { x, y } => write!(f, "非流形边 ({x},{y})"),
            MeshError::BoundaryEdge { edge } => write!(f, "边界边 {:?}", edge),
            MeshError::EmptyResult => write!(f, "结果为空"),
            MeshError::UnchangedResult => write!(f, "结果无变化"),
            MeshError::Corrupt { reason } => write!(f, "拓扑损坏: {reason}"),
        }
    }
}
impl std::error::Error for MeshError {}

/// 布尔运算类型（算法层外借 manifold，红线：不自研）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolOp {
    Union,
    Difference,
    Intersection,
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    positions: Vec<[f64; 3]>,
    halfedges: Vec<HalfEdge>,
    faces: Vec<HalfEdgeId>,
    vertex_he: Vec<Option<HalfEdgeId>>,
    /// 无序点对 -> 该边上的半边列表（≥3 即非流形）
    edge_map: HashMap<(u32, u32), Vec<HalfEdgeId>>,
}

fn ekey(x: u32, y: u32) -> (u32, u32) {
    if x < y {
        (x, y)
    } else {
        (y, x)
    }
}

impl Mesh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    pub fn halfedge_count(&self) -> usize {
        self.halfedges.len()
    }

    pub fn position(&self, v: u32) -> Option<[f64; 3]> {
        self.positions.get(v as usize).copied()
    }

    pub fn add_vertex(&mut self, p: [f64; 3]) -> u32 {
        let idx = self.positions.len() as u32;
        self.positions.push(p);
        self.vertex_he.push(None);
        idx
    }

    /// 目标半边的终点 = next 半边的起点
    fn target(&self, he: HalfEdgeId) -> u32 {
        self.halfedges[self.next(he).0 as usize].origin
    }

    /// 按端点找有向半边（edge_flip 的寻址口）
    pub fn find_halfedge(&self, x: u32, y: u32) -> Option<HalfEdgeId> {
        (0..self.halfedges.len()).find_map(|i| {
            let id = HalfEdgeId(i as u32);
            if self.halfedges[i].origin == x && self.target(id) == y {
                Some(id)
            } else {
                None
            }
        })
    }

    /// 导出索引几何（对接 IR Geometry）
    pub fn to_indexed(&self) -> (Vec<[f64; 3]>, Vec<u32>) {
        let mut indices = Vec::with_capacity(self.faces.len() * 3);
        for &start in &self.faces {
            let mut cur = start;
            for _ in 0..3 {
                indices.push(self.halfedges[cur.0 as usize].origin);
                cur = self.next(cur);
            }
        }
        (self.positions.clone(), indices)
    }

    fn next(&self, he: HalfEdgeId) -> HalfEdgeId {
        self.halfedges[he.0 as usize].next
    }

    /// 增三角面：自动配 twin，非流形/退化显式拒绝（数据结构层的几何事实断言）
    pub fn add_face(&mut self, a: u32, b: u32, c: u32) -> Result<FaceId, MeshError> {
        if a == b || b == c || a == c {
            return Err(MeshError::DegenerateFace { a, b, c });
        }
        for &v in &[a, b, c] {
            if v as usize >= self.positions.len() {
                return Err(MeshError::VertexOutOfRange { index: v });
            }
        }
        let verts = [a, b, c];
        // 先断言后写入：非流形在动数据前就拒绝（错误不留半成品）
        for i in 0..3 {
            let key = ekey(verts[i], verts[(i + 1) % 3]);
            if self.edge_map.get(&key).map_or(0, |v| v.len()) >= 2 {
                return Err(MeshError::NonManifoldEdge {
                    x: verts[i],
                    y: verts[(i + 1) % 3],
                });
            }
        }
        let face = FaceId(self.faces.len() as u32);
        let base = self.halfedges.len() as u32;
        let hes = [
            HalfEdgeId(base),
            HalfEdgeId(base + 1),
            HalfEdgeId(base + 2),
        ];
        for i in 0..3 {
            self.halfedges.push(HalfEdge {
                origin: verts[i],
                twin: None,
                next: hes[(i + 1) % 3],
                face,
            });
            if self.vertex_he[verts[i] as usize].is_none() {
                self.vertex_he[verts[i] as usize] = Some(hes[i]);
            }
        }
        // twin 配对
        for i in 0..3 {
            let x = verts[i];
            let y = verts[(i + 1) % 3];
            let key = ekey(x, y);
            let existing = self.edge_map.get(&key).cloned().unwrap_or_default();
            if let Some(&twin) = existing.iter().find(|&&h| {
                let he = &self.halfedges[h.0 as usize];
                he.twin.is_none() && he.origin == y && self.target(h) == x
            }) {
                self.halfedges[hes[i].0 as usize].twin = Some(twin);
                self.halfedges[twin.0 as usize].twin = Some(hes[i]);
            }
            self.edge_map.entry(key).or_default().push(hes[i]);
        }
        self.faces.push(hes[0]);
        Ok(face)
    }

    /// 开放边（twin 缺失）——墙洞/边界检测的基础查询
    pub fn boundary_edges(&self) -> Vec<HalfEdgeId> {
        self.halfedges
            .iter()
            .enumerate()
            .filter(|(_, h)| h.twin.is_none())
            .map(|(i, _)| HalfEdgeId(i as u32))
            .collect()
    }

    pub fn edge_endpoints(&self, he: HalfEdgeId) -> (u32, u32) {
        (self.halfedges[he.0 as usize].origin, self.target(he))
    }

    /// 编辑算子：对角线换向。四边形 (b,c,a,d) 的对角线 ab -> cd
    pub fn edge_flip(&mut self, he: HalfEdgeId) -> Result<(), MeshError> {
        let h = self.halfedges.get(he.0 as usize).ok_or_else(|| MeshError::Corrupt {
            reason: "半边索引越界".into(),
        })?;
        let t_id = h.twin.ok_or(MeshError::BoundaryEdge { edge: he })?;
        let t = t_id.0 as usize;
        let (p, q) = (self.next(he), self.next(self.next(he)));
        let (r, s) = (self.next(t_id), self.next(self.next(t_id)));
        let (f1, f2) = (self.halfedges[he.0 as usize].face, self.halfedges[t].face);
        let a = self.halfedges[he.0 as usize].origin;
        let b = self.halfedges[t].origin;
        let c = self.halfedges[q.0 as usize].origin;
        let d = self.halfedges[s.0 as usize].origin;
        if c == d {
            return Err(MeshError::DegenerateFace { a, b, c });
        }
        // 非流形防护（审计 #5）：新对角线 (c,d) 已存在则翻转必造非流形——拒绝
        if self.edge_map.contains_key(&ekey(c, d)) {
            return Err(MeshError::NonManifoldEdge { x: c, y: d });
        }
        // 槽位复用：he 变 c→d（新对角线），t 变 d→c
        self.halfedges[he.0 as usize] = HalfEdge {
            origin: c,
            twin: Some(t_id),
            next: s,
            face: f1,
        };
        self.halfedges[t] = HalfEdge {
            origin: d,
            twin: Some(he),
            next: q,
            face: f2,
        };
        // 新三角形 (b,c,d)：bc -> cd(he) -> db(s)
        self.halfedges[p.0 as usize].next = he;
        self.halfedges[p.0 as usize].face = f1;
        self.halfedges[s.0 as usize].next = p;
        self.halfedges[s.0 as usize].face = f1;
        // 新三角形 (c,a,d)：ca -> ad -> dc(t)
        self.halfedges[q.0 as usize].next = r;
        self.halfedges[q.0 as usize].face = f2;
        self.halfedges[r.0 as usize].next = t_id;
        self.halfedges[r.0 as usize].face = f2;
        // 面起点指针显式校正（槽位复用后归属会变）
        self.faces[f1.0 as usize] = p;
        self.faces[f2.0 as usize] = q;
        // 顶点出射指针修复：a/b 的旧出射边换向了
        if self.vertex_he[a as usize] == Some(he) {
            self.vertex_he[a as usize] = Some(r);
        }
        if self.vertex_he[b as usize] == Some(t_id) {
            self.vertex_he[b as usize] = Some(p);
        }
        // 边表维护：旧对角线 (a,b) 下线，新对角线 (c,d) 上线
        self.edge_map.remove(&ekey(a, b));
        self.edge_map.entry(ekey(c, d)).or_default().extend([he, t_id]);
        // 翻后强校验（审计 #5）
        self.validate()?;
        Ok(())
    }

    /// 完整性校验：twin 对称、next 环闭合、面环 ≥3、顶点指针一致
    pub fn validate(&self) -> Result<(), MeshError> {
        for (i, h) in self.halfedges.iter().enumerate() {
            let id = HalfEdgeId(i as u32);
            if h.next.0 as usize >= self.halfedges.len() {
                return Err(MeshError::Corrupt { reason: format!("next 越界 @he{i}") });
            }
            if h.origin as usize >= self.positions.len() {
                return Err(MeshError::Corrupt { reason: format!("origin 越界 @he{i}") });
            }
            if let Some(t) = h.twin {
                let th = self.halfedges.get(t.0 as usize).ok_or_else(|| MeshError::Corrupt {
                    reason: format!("twin 越界 @he{i}") })?;
                if th.twin != Some(id) || th.origin != self.target(id) {
                    return Err(MeshError::Corrupt { reason: format!("twin 不对称 @he{i}") });
                }
            }
        }
        for (v, &vhe) in self.vertex_he.iter().enumerate() {
            if let Some(id) = vhe {
                if self.halfedges[id.0 as usize].origin != v as u32 {
                    return Err(MeshError::Corrupt { reason: format!("顶点指针错位 @v{v}") });
                }
            }
        }
        for (fi, &start) in self.faces.iter().enumerate() {
            let mut cur = start;
            let mut steps = 0;
            loop {
                if self.halfedges[cur.0 as usize].face != FaceId(fi as u32) {
                    return Err(MeshError::Corrupt { reason: format!("面环归属错位 @face{fi}") });
                }
                cur = self.next(cur);
                steps += 1;
                if cur == start {
                    break;
                }
                if steps > self.halfedges.len() {
                    return Err(MeshError::Corrupt { reason: format!("面环不闭合 @face{fi}") });
                }
            }
            if steps < 3 {
                return Err(MeshError::Corrupt { reason: format!("面环退化 @face{fi}") });
            }
        }
        Ok(())
    }

    /// 布尔运算（manifold-rust 外借；结果为空显式报错）
    pub fn boolean(&self, other: &Mesh, op: BoolOp) -> Result<Mesh, MeshError> {
        use manifold_rust::manifold::Manifold;
        use manifold_rust::types::{MeshGL64, OpType};

        fn to_manifold(m: &Mesh) -> Result<Manifold, MeshError> {
            let (pos, idx) = m.to_indexed();
            let gl = MeshGL64 {
                num_prop: 3,
                vert_properties: pos.iter().flatten().copied().collect(),
                tri_verts: idx.iter().map(|&i| i as u64).collect(),
                ..Default::default()
            };
            let man = Manifold::from_mesh_gl64_robust(&gl);
            if man.is_empty() {
                return Err(MeshError::Corrupt {
                    reason: format!("manifold 导入失败: {:?}", man.status()),
                });
            }
            Ok(man)
        }

        let a = to_manifold(self)?;
        let b = to_manifold(other)?;
        let t = match op {
            BoolOp::Union => OpType::Add,
            BoolOp::Difference => OpType::Subtract,
            BoolOp::Intersection => OpType::Intersect,
        };
        let out = a.boolean(&b, t);
        if out.is_empty() {
            return Err(MeshError::EmptyResult);
        }
        // 宪法条款：结果与输入几何全同（体积+面积不变量）= 打空/无交，显式报错
        let vo = out.volume();
        let ao = out.surface_area();
        let same_as = |v: f64, ar: f64| {
            (vo - v).abs() <= 1e-9 * v.abs().max(1.0) && (ao - ar).abs() <= 1e-9 * ar.abs().max(1.0)
        };
        if same_as(a.volume(), a.surface_area()) || same_as(b.volume(), b.surface_area()) {
            return Err(MeshError::UnchangedResult);
        }
        let gl = out.get_mesh_gl64(-1);
        let stride = (gl.num_prop as usize).max(3);
        let mut positions = Vec::with_capacity(gl.vert_properties.len() / stride);
        for chunk in gl.vert_properties.chunks_exact(stride) {
            positions.push([chunk[0], chunk[1], chunk[2]]);
        }
        let indices: Vec<u32> = gl.tri_verts.iter().map(|&i| i as u32).collect();
        Mesh::from_indexed(positions, &indices)
    }

    /// 从索引几何构建（对接 IR Geometry 的 positions + indices）
    pub fn from_indexed(positions: Vec<[f64; 3]>, indices: &[u32]) -> Result<Self, MeshError> {
        if !indices.len().is_multiple_of(3) {
            return Err(MeshError::Corrupt { reason: "索引数不是 3 的倍数".into() });
        }
        let mut m = Mesh::new();
        for p in positions {
            m.add_vertex(p);
        }
        for tri in indices.chunks_exact(3) {
            m.add_face(tri[0], tri[1], tri[2])?;
        }
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 双三角建四边形：twin 配对、边界 4 条、校验过
    #[test]
    fn quad_from_two_triangles() {
        let mut m = Mesh::new();
        let v: Vec<u32> = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]]
            .into_iter()
            .map(|p| m.add_vertex(p))
            .collect();
        m.add_face(v[0], v[1], v[2]).unwrap();
        m.add_face(v[0], v[2], v[3]).unwrap();
        m.validate().unwrap();
        assert_eq!(m.face_count(), 2);
        assert_eq!(m.boundary_edges().len(), 4);
        // 对角线 (0,2) 应已配 twin
        let diag = m.halfedges.iter().find(|h| {
            let (x, y) = (h.origin, m.target(HalfEdgeId(0)));
            let _ = (x, y);
            h.origin == 0 && h.twin.is_some()
        });
        assert!(diag.is_some());
    }

    /// 非流形拒绝：同一无序边第 3 个面
    #[test]
    fn non_manifold_rejected() {
        let mut m = Mesh::new();
        let v: Vec<u32> = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [-1.0, 1.0, 0.0]]
            .into_iter()
            .map(|p| m.add_vertex(p))
            .collect();
        m.add_face(v[0], v[1], v[2]).unwrap();
        m.add_face(v[1], v[0], v[3]).unwrap();
        let err = m.add_face(v[0], v[1], v[2]).unwrap_err(); // 再压一条到边 (0,1)
        assert!(matches!(err, MeshError::NonManifoldEdge { .. }));
    }

    /// 退化面拒绝
    #[test]
    fn degenerate_face_rejected() {
        let mut m = Mesh::new();
        let a = m.add_vertex([0.0; 3]);
        let b = m.add_vertex([1.0, 0.0, 0.0]);
        assert!(matches!(
            m.add_face(a, a, b),
            Err(MeshError::DegenerateFace { .. })
        ));
    }

    /// edge_flip：对角线 (0,2) 变 (1,3)，翻后校验过、边界不变
    #[test]
    fn edge_flip_swaps_diagonal() {
        let mut m = Mesh::new();
        let v: Vec<u32> = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]]
            .into_iter()
            .map(|p| m.add_vertex(p))
            .collect();
        m.add_face(v[0], v[1], v[2]).unwrap();
        m.add_face(v[0], v[2], v[3]).unwrap();
        let diag = m
            .halfedges
            .iter()
            .enumerate()
            .find(|(_, h)| h.origin == 0 && h.twin.is_some())
            .map(|(i, _)| HalfEdgeId(i as u32))
            .unwrap();
        let (x, y) = m.edge_endpoints(diag);
        assert_eq!((x.min(y), x.max(y)), (0, 2));
        m.edge_flip(diag).unwrap();
        m.validate().unwrap();
        let (x, y) = m.edge_endpoints(diag);
        assert_eq!((x.min(y), x.max(y)), (1, 3)); // 对角线换成了 (1,3)
        assert_eq!(m.boundary_edges().len(), 4);
    }

    /// 校验器必须逮住人为破坏
    #[test]
    fn validate_catches_tampering() {
        let mut m = Mesh::new();
        let v: Vec<u32> = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.5, 1.0, 0.0]]
            .into_iter()
            .map(|p| m.add_vertex(p))
            .collect();
        m.add_face(v[0], v[1], v[2]).unwrap();
        m.validate().unwrap();
        m.halfedges[0].next = HalfEdgeId(0); // 破坏面环
        assert!(m.validate().is_err());
    }

    /// 索引几何往返：构建成功且面数一致
    #[test]
    fn from_indexed_builds() {
        let m = Mesh::from_indexed(
            vec![[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
            &[0, 1, 2, 0, 2, 3],
        )
        .unwrap();
        m.validate().unwrap();
        assert_eq!(m.face_count(), 2);
    }

    fn box_mesh(lo: [f64; 3], hi: [f64; 3]) -> Mesh {
        let [x0, y0, z0] = lo;
        let [x1, y1, z1] = hi;
        let p = vec![
            [x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0],
            [x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1],
        ];
        let i = vec![0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4,
            2, 3, 7, 2, 7, 6, 1, 2, 6, 1, 6, 5, 0, 4, 7, 0, 7, 3];
        Mesh::from_indexed(p, &i).unwrap()
    }

    /// 并集：两箱相交，结果流形且面数增加
    #[test]
    fn boolean_union_validates() {
        let a = box_mesh([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let b = box_mesh([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]);
        let u = a.boolean(&b, BoolOp::Union).unwrap();
        u.validate().unwrap();
        assert!(u.face_count() > 12);
    }

    /// 交集打空：不相交两箱 → 显式 EmptyResult（宪法条款）
    #[test]
    fn disjoint_intersection_is_empty_error() {
        let a = box_mesh([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let b = box_mesh([5.0, 5.0, 5.0], [6.0, 6.0, 6.0]);
        assert_eq!(a.boolean(&b, BoolOp::Intersection).unwrap_err(), MeshError::EmptyResult);
    }

    /// 切割打空：差集没碰上 → 显式 UnchangedResult（体积/面积不变量，重三角化免疫）
    #[test]
    fn difference_miss_is_unchanged_error() {
        let a = box_mesh([0.0, 0.0, 0.0], [2.0, 0.2, 1.5]);
        let b = box_mesh([5.0, 5.0, 5.0], [6.0, 6.0, 6.0]);
        assert_eq!(a.boolean(&b, BoolOp::Difference).unwrap_err(), MeshError::UnchangedResult);
    }

    /// 差集开内腔：箱减内箱 → 流形结果带内壁
    #[test]
    fn difference_opens_void() {
        let a = box_mesh([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
        let b = box_mesh([0.5, 0.5, 0.5], [1.5, 1.5, 1.5]);
        let d = a.boolean(&b, BoolOp::Difference).unwrap();
        d.validate().unwrap();
        assert!(d.face_count() > 12);
    }
}
