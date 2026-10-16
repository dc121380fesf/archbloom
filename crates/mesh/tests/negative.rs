//! 网格编辑负例（审计 #5/#15④）：edge_flip 不得造非流形
use archcore_mesh::{Mesh, MeshError};

/// 翻转后新对角线 (1,3) 已存在（第三面带边 (1,3)）→ 必须拒绝
#[test]
fn edge_flip_rejects_non_manifold_diagonal() {
    let mut m = Mesh::new();
    let v: Vec<u32> = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [2.0, 0.5, 0.5]]
        .into_iter()
        .map(|p| m.add_vertex(p))
        .collect();
    m.add_face(v[0], v[1], v[2]).unwrap();
    m.add_face(v[0], v[2], v[3]).unwrap();
    m.add_face(v[1], v[3], v[4]).unwrap(); // 引入边 (1,3)
    let he = m.find_halfedge(v[0], v[2]).unwrap(); // 对角线 (0,2)
    let err = m.edge_flip(he).unwrap_err();
    assert!(matches!(err, MeshError::NonManifoldEdge { .. }), "{err:?}");
    m.validate().unwrap(); // 拒绝后拓扑仍须健全
}

/// 边界边（无 twin）不可翻
#[test]
fn edge_flip_rejects_boundary() {
    let mut m = Mesh::new();
    let v: Vec<u32> = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
        .into_iter()
        .map(|p| m.add_vertex(p))
        .collect();
    m.add_face(v[0], v[1], v[2]).unwrap();
    let he = m.find_halfedge(v[0], v[1]).unwrap();
    let err = m.edge_flip(he).unwrap_err();
    assert!(matches!(err, MeshError::BoundaryEdge { .. }));
}

/// 合法四边形翻转仍放行（正例不回归）
#[test]
fn edge_flip_still_works_for_quad() {
    let mut m = Mesh::new();
    let v: Vec<u32> = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]]
        .into_iter()
        .map(|p| m.add_vertex(p))
        .collect();
    m.add_face(v[0], v[1], v[2]).unwrap();
    m.add_face(v[0], v[2], v[3]).unwrap();
    let he = m.find_halfedge(v[0], v[2]).unwrap();
    m.edge_flip(he).unwrap();
    m.validate().unwrap();
    assert!(m.find_halfedge(v[1], v[3]).is_some(), "新对角线 (1,3) 应已上线");
}
