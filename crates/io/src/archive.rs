//! `.al` 自包含存档（Archbloom Archive）。
//!
//! 容器布局（全部小端）：
//!
//! ```text
//! [0..8)               魔数 b"ALBLOOM1"
//! [8..12)              header 长度 u32
//! [12..12+hdr_len)     header（UTF-8 JSON）
//! [12+hdr_len..)       blob 区（scene JSON + 各贴图原始字节首尾相接）
//! ```
//!
//! header 形如：
//!
//! ```json
//! {"format":"archbloom-archive","version":1,
//!  "scene":{"offset":0,"size":123},
//!  "assets":[{"path":"assets/asset-1.jpg","mime":"image/jpeg","offset":123,"size":456}]}
//! ```
//!
//! 设计取向：
//! - **零外部依赖**：贴图本身是 jpg/png 已压缩格式，再套 zip 收益极小，不值得为它引依赖；
//! - **容器内一律相对路径**：场景里的 `assets[].uri` 存 `assets/<id>.<ext>`，
//!   打包时读原路径取字节，解包时落到目标目录再重写为绝对路径——这样 `villa.al`
//!   拷到任何机器都能开；
//! - **敌对输入口径**：魔数不符、头部截断、offset/size 越界一律 `IoError::Format`
//!   带原因返回，绝不 panic、绝不静默截断。

use archcore_ir::Scene;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::IoError;

const MAGIC: &[u8; 8] = b"ALBLOOM1";
const FORMAT: &str = "archbloom-archive";
const VERSION: u32 = 1;
/// 魔数(8) + header 长度(4)
const PREFIX_LEN: usize = 12;

/// 打包结果。缺贴图不算失败，但要一条不落地报出来（不静默降级）。
#[derive(Debug, Clone, Default)]
pub struct PackReport {
    pub assets_packed: usize,
    pub warnings: Vec<String>,
}

/// 解包出来的存档：场景 + 容器内各文件原始字节
#[derive(Debug, Clone)]
pub struct Archive {
    pub scene: Scene,
    pub files: Vec<ArchiveFile>,
}

#[derive(Debug, Clone)]
pub struct ArchiveFile {
    /// 容器内相对路径，形如 `assets/asset-1.jpg`
    pub path: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

fn ext_of(uri: &str) -> String {
    Path::new(uri)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "bin".to_string())
}

fn mime_of_ext(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tga" => "image/x-tga",
        _ => "application/octet-stream",
    }
}

/// 打包：把场景与它引用的贴图字节收进一个 `.al`。
///
/// 贴图读取失败不中断打包——该 asset 保留原 uri 并记入 `warnings`，
/// 调用方据此决定是否提示用户。
pub fn pack(scene: &Scene, out: &Path) -> Result<PackReport, IoError> {
    let mut scene = scene.clone();
    let mut report = PackReport::default();

    // 1) 容器内相对路径 + 读取字节
    let mut collected: Vec<(Value, Vec<u8>)> = Vec::new();
    for i in 0..scene.assets.len() {
        let id = scene.assets[i].id.clone();
        let uri = scene.assets[i].uri.clone();
        let ext = ext_of(&uri);
        let rel = format!("assets/{id}.{ext}");
        match std::fs::read(&uri) {
            Ok(bytes) => {
                scene.assets[i].uri = rel.clone();
                report.assets_packed += 1;
                collected.push((
                    json!({
                        "path": rel,
                        "mime": mime_of_ext(&ext),
                        "size": bytes.len(),
                    }),
                    bytes,
                ));
            }
            Err(e) => {
                report
                    .warnings
                    .push(format!("贴图缺失，未打包：{uri}（{e}）"));
            }
        }
    }

    // 2) 场景 JSON 作为第一个 blob
    let scene_bytes = serde_json::to_vec(&scene)
        .map_err(|e| IoError::Format(format!("场景序列化失败：{e}")))?;

    // 3) 拼接 blob 区，回填 offset
    let mut blobs: Vec<u8> = Vec::new();
    blobs.extend_from_slice(&scene_bytes);
    let mut asset_entries: Vec<Value> = Vec::new();
    for (mut entry, bytes) in collected {
        let offset = blobs.len();
        blobs.extend_from_slice(&bytes);
        entry["offset"] = json!(offset);
        asset_entries.push(entry);
    }

    // 4) header + 落盘
    let header = json!({
        "format": FORMAT,
        "version": VERSION,
        "scene": { "offset": 0, "size": scene_bytes.len() },
        "assets": asset_entries,
    });
    let header_bytes = serde_json::to_vec(&header)
        .map_err(|e| IoError::Format(format!("header 序列化失败：{e}")))?;
    let hdr_len = u32::try_from(header_bytes.len())
        .map_err(|_| IoError::Format("header 过大（超出 u32）".into()))?;

    let mut buf: Vec<u8> = Vec::with_capacity(PREFIX_LEN + header_bytes.len() + blobs.len());
    buf.extend_from_slice(MAGIC);
    buf.extend_from_slice(&hdr_len.to_le_bytes());
    buf.extend_from_slice(&header_bytes);
    buf.extend_from_slice(&blobs);
    std::fs::write(out, &buf).map_err(|e| IoError::Io(format!("写入失败 {}：{e}", out.display())))?;

    Ok(report)
}

/// 只解析容器，不落地。
pub fn read(data: &[u8]) -> Result<Archive, IoError> {
    if data.len() < PREFIX_LEN {
        return Err(IoError::Format(format!(
            ".al 文件过短（{} 字节，至少需要 {PREFIX_LEN}）",
            data.len()
        )));
    }
    if &data[0..8] != MAGIC {
        return Err(IoError::Format(
            "不是 Archbloom 存档：魔数不符（文件头应为 ALBLOOM1）".into(),
        ));
    }
    let hdr_len = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
    let hdr_end = PREFIX_LEN
        .checked_add(hdr_len)
        .ok_or_else(|| IoError::Format("header 长度溢出".into()))?;
    if hdr_end > data.len() {
        return Err(IoError::Format(format!(
            "header 越界：声明 {hdr_len} 字节，实际可用 {}",
            data.len().saturating_sub(PREFIX_LEN)
        )));
    }
    let header: Value = serde_json::from_slice(&data[PREFIX_LEN..hdr_end])
        .map_err(|e| IoError::Format(format!("header 不是合法 JSON：{e}")))?;
    if header.get("format").and_then(|v| v.as_str()) != Some(FORMAT) {
        return Err(IoError::Format("header 缺少 format 标记".into()));
    }
    let version = header.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
    if version != VERSION as u64 {
        return Err(IoError::Format(format!(
            "存档版本不支持：{version}（本程序支持 {VERSION}）"
        )));
    }

    let blobs = &data[hdr_end..];
    let slice = |obj: &Value, what: &str| -> Result<Vec<u8>, IoError> {
        let off = obj
            .get("offset")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| IoError::Format(format!("{what} 缺少 offset")))?
            as usize;
        let size = obj
            .get("size")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| IoError::Format(format!("{what} 缺少 size")))?
            as usize;
        let end = off
            .checked_add(size)
            .ok_or_else(|| IoError::Format(format!("{what} 长度溢出")))?;
        if end > blobs.len() {
            return Err(IoError::Format(format!(
                "{what} 越界：需要 {off}+{size}，blob 区只有 {}",
                blobs.len()
            )));
        }
        Ok(blobs[off..end].to_vec())
    };

    let scene_obj = header
        .get("scene")
        .ok_or_else(|| IoError::Format("header 缺少 scene 段".into()))?;
    let scene_bytes = slice(scene_obj, "scene")?;
    let scene: Scene = serde_json::from_slice(&scene_bytes)
        .map_err(|e| IoError::Format(format!("内嵌场景解析失败：{e}")))?;

    let mut files = Vec::new();
    if let Some(list) = header.get("assets").and_then(|v| v.as_array()) {
        for (i, a) in list.iter().enumerate() {
            let path = a
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| IoError::Format(format!("第 {i} 个 asset 缺少 path")))?
                .to_string();
            // 容器内路径只允许相对路径，防目录穿越
            if path.is_empty()
                || path.contains("..")
                || Path::new(&path).is_absolute()
                || path.contains(':')
            {
                return Err(IoError::Format(format!("非法 asset 路径：{path}")));
            }
            let bytes = slice(a, &format!("asset[{i}]"))?;
            let mime = a
                .get("mime")
                .and_then(|v| v.as_str())
                .unwrap_or("application/octet-stream")
                .to_string();
            files.push(ArchiveFile { path, mime, bytes });
        }
    }

    Ok(Archive { scene, files })
}

/// 把容器内文件落到 `dir`，并把场景里的 uri 重写为落盘后的绝对路径。
pub fn extract_to(archive: &Archive, dir: &Path) -> Result<Scene, IoError> {
    std::fs::create_dir_all(dir)
        .map_err(|e| IoError::Io(format!("创建目录失败 {}：{e}", dir.display())))?;
    let mut scene = archive.scene.clone();
    for f in &archive.files {
        let abs = dir.join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| IoError::Io(format!("创建目录失败 {}：{e}", parent.display())))?;
        }
        std::fs::write(&abs, &f.bytes)
            .map_err(|e| IoError::Io(format!("写出贴图失败 {}：{e}", abs.display())))?;
        // 容器内相对路径 → 场景 uri 指回落盘位置
        let rel = f.path.clone();
        for a in scene.assets.iter_mut() {
            if a.uri == rel {
                a.uri = abs.display().to_string();
            }
        }
    }
    Ok(scene)
}

/// 打开 `.al`：解析 + 落地到 `dir`，返回可直接渲染的场景。
pub fn open(data: &[u8], dir: &Path) -> Result<Scene, IoError> {
    let archive = read(data)?;
    extract_to(&archive, dir)
}

/// FNV-1a 64：给解包目录取个稳定的内容指纹名（同一存档复用同一目录）
pub fn content_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 场景是否引用了任何贴图（无贴图时打包更简单，也便于快速判断）
pub fn has_assets(scene: &Scene) -> bool {
    !scene.assets.is_empty()
}

/// 默认解包目录：系统临时目录下按内容指纹命名
pub fn default_extract_dir(data: &[u8]) -> PathBuf {
    std::env::temp_dir().join(format!("archbloom-{:016x}", content_hash(data)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use archcore_ir::{AssetRef, Scene, Units, UpAxis};

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "archbloom_test_{}_{}_{tag}",
            std::process::id(),
            content_hash(tag.as_bytes())
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn scene_with_texture(dir: &Path, with_missing: bool) -> Scene {
        let tex = dir.join("wall.png");
        std::fs::write(&tex, b"\x89PNG\r\n\x1a\nfakebytes").unwrap();
        let mut assets = vec![AssetRef {
            id: "asset-1".into(),
            uri: tex.display().to_string(),
            kind: "texture".into(),
            license: None,
            source: None,
        }];
        if with_missing {
            assets.push(AssetRef {
                id: "asset-2".into(),
                uri: dir.join("nope.jpg").display().to_string(),
                kind: "texture".into(),
                license: None,
                source: None,
            });
        }
        Scene {
            schema_version: "0".into(),
            units: Units::M,
            up_axis: UpAxis::ZUp,
            name: "测试场景".into(),
            objects: Vec::new(),
            materials: Vec::new(),
            assets,
            meta: Default::default(),
        }
    }

    #[test]
    fn 往返_场景与贴图字节一致() {
        let d = tmpdir("roundtrip");
        let scene = scene_with_texture(&d, false);
        let al = d.join("t.al");
        let rep = pack(&scene, &al).unwrap();
        assert_eq!(rep.assets_packed, 1);
        assert!(rep.warnings.is_empty());

        let data = std::fs::read(&al).unwrap();
        assert_eq!(&data[0..8], MAGIC);

        let arch = read(&data).unwrap();
        assert_eq!(arch.scene.assets.len(), 1);
        assert_eq!(arch.scene.assets[0].uri, "assets/asset-1.png");
        assert_eq!(arch.files.len(), 1);
        assert_eq!(arch.files[0].bytes, b"\x89PNG\r\n\x1a\nfakebytes");
        assert_eq!(arch.files[0].mime, "image/png");

        let out = d.join("extract");
        let restored = extract_to(&arch, &out).unwrap();
        assert!(restored.assets[0].uri.contains("asset-1.png"));
        let laid = std::fs::read(&restored.assets[0].uri).unwrap();
        assert_eq!(laid, b"\x89PNG\r\n\x1a\nfakebytes");
    }

    #[test]
    fn 缺贴图不中断但要报出来() {
        let d = tmpdir("missing");
        let scene = scene_with_texture(&d, true);
        let al = d.join("t.al");
        let rep = pack(&scene, &al).unwrap();
        assert_eq!(rep.assets_packed, 1);
        assert_eq!(rep.warnings.len(), 1);
        assert!(rep.warnings[0].contains("nope.jpg"));
    }

    #[test]
    fn 坏魔数报格式错误不panic() {
        let d = tmpdir("badmagic");
        let al = d.join("x.al");
        std::fs::write(&al, b"NOTALOOMxxxxxxxx").unwrap();
        let err = read(&std::fs::read(&al).unwrap()).unwrap_err();
        assert!(matches!(err, IoError::Format(_)));
        assert!(err.to_string().contains("魔数"));
    }

    #[test]
    fn 头部截断报错不panic() {
        let mut data = Vec::new();
        data.extend_from_slice(MAGIC);
        data.extend_from_slice(&999u32.to_le_bytes());
        data.extend_from_slice(b"{}");
        let err = read(&data).unwrap_err();
        assert!(matches!(err, IoError::Format(_)));
    }

    #[test]
    fn offset越界报错不panic() {
        let header = json!({
            "format": FORMAT, "version": VERSION,
            "scene": {"offset": 0, "size": 10},
            "assets": []
        });
        let hb = serde_json::to_vec(&header).unwrap();
        let mut data = Vec::new();
        data.extend_from_slice(MAGIC);
        data.extend_from_slice(&(hb.len() as u32).to_le_bytes());
        data.extend_from_slice(&hb);
        data.extend_from_slice(b"short");
        let err = read(&data).unwrap_err();
        assert!(matches!(err, IoError::Format(_)));
        assert!(err.to_string().contains("越界"));
    }

    #[test]
    fn 目录穿越路径被拒() {
        let scene = json!({
            "schema_version": "0", "units": "m", "up_axis": "z_up", "name": "s",
            "objects": [], "materials": [], "assets": [], "meta": {}
        });
        let sb = serde_json::to_vec(&scene).unwrap();
        let header = json!({
            "format": FORMAT, "version": VERSION,
            "scene": {"offset": 0, "size": sb.len()},
            "assets": [{"path": "../../evil.png", "mime": "image/png", "offset": sb.len(), "size": 1}]
        });
        let hb = serde_json::to_vec(&header).unwrap();
        let mut data = Vec::new();
        data.extend_from_slice(MAGIC);
        data.extend_from_slice(&(hb.len() as u32).to_le_bytes());
        data.extend_from_slice(&hb);
        data.extend_from_slice(&sb);
        data.extend_from_slice(b"x");
        let err = read(&data).unwrap_err();
        assert!(matches!(err, IoError::Format(_)));
        assert!(err.to_string().contains("非法 asset 路径"));
    }

    #[test]
    fn 无贴图场景可打包解包() {
        let d = tmpdir("noasset");
        let scene = Scene {
            schema_version: "0".into(),
            units: Units::M,
            up_axis: UpAxis::ZUp,
            name: "空".into(),
            objects: Vec::new(),
            materials: Vec::new(),
            assets: Vec::new(),
            meta: Default::default(),
        };
        let al = d.join("e.al");
        let rep = pack(&scene, &al).unwrap();
        assert_eq!(rep.assets_packed, 0);
        let arch = read(&std::fs::read(&al).unwrap()).unwrap();
        assert!(arch.files.is_empty());
        assert_eq!(arch.scene.name, "空");
    }
}
