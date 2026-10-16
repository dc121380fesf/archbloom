//! 漫游交付物回归：帧序列 + manifest + 相机轨迹
use archcore_engine::Engine;
use serde_json::json;

#[test]
fn walkthrough_writes_frames_and_manifest() {
    let mut e = Engine::new();
    e.execute_script(&[(
        "mesh.create".into(),
        json!({
            "name": "Room",
            "positions": [[0,0,0],[4,0,0],[4,3,0],[0,3,0],[0,0,2.6],[4,0,2.6],[4,3,2.6],[0,3,2.6]],
            "indices": [0,2,1,0,3,2,4,5,6,4,6,7,0,1,5,0,5,4,2,3,7,2,7,6,1,2,6,1,6,5,0,4,7,0,7,3]
        }),
    )])
    .unwrap();
    let dir = std::env::temp_dir().join("archcore_walk_test");
    let _ = std::fs::remove_dir_all(&dir);
    let r = e
        .execute(
            "render.walkthrough",
            &json!({
                "dir": dir.to_str().unwrap(),
                "waypoints": [[1,1,1.6],[3,2,1.6]],
                "target": [2,1.5,1.0],
                "frames": 3,
                "width": 48,
                "height": 32,
                "spp": 2,
                "renderer": "cpu"
            }),
        )
        .unwrap();
    for i in 0..3 {
        assert!(
            dir.join(format!("walk_{i:04}.png")).exists(),
            "缺第 {i} 帧"
        );
    }
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["frames"], 3);
    assert_eq!(manifest["pattern"], "walk_%04d.png");
    // 相机停在末帧位姿
    let cam_id = &r.changed[0];
    let cam = e.scene.objects.iter().find(|o| o.id == *cam_id).unwrap();
    assert!((cam.transform.translation[0] - 3.0).abs() < 1e-9);
    assert!((cam.transform.translation[2] - 1.6).abs() < 1e-9);
}

/// 路径点不足两个：显式拒绝
#[test]
fn walkthrough_rejects_single_waypoint() {
    let mut e = Engine::new();
    let err = e
        .execute(
            "render.walkthrough",
            &json!({
                "dir": std::env::temp_dir().join("archcore_walk_reject").to_str().unwrap(),
                "waypoints": [[1,1,1]],
                "frames": 2
            }),
        )
        .unwrap_err();
    let _ = err;
}
