//! `.al` 自包含存档。
//!
//! # 两个版本
//!
//! - **v1**：单块布局（`magic + header JSON + blob 区`）。能读、不再写——保留只为打开老文件。
//! - **v2**：段表布局（[`format`]）。写入一律用 v2，支持随机读、逐段 CRC、原子写、
//!   以及为双表示（B-rep + 网格缓存）预留的段类型。
//!
//! # 高层 API 与低层 API
//!
//! - 高层：[`pack`] / [`read`] / [`open`] / [`extract_to`]——面向「场景 + 贴图」这个业务语义；
//! - 低层：[`format::Builder`] / [`format::Reader`]——面向需要随机读、按段取、裁剪派生数据的场合。
//!
//! 场景仍是几何的真相源（IR 未拆分），所以 v2 当前只写 `SCENE` / `ASSET` / `PROVENANCE` 三类段；
//! `GEO_BREP` / `GEO_MESH` / `FACTS` 已定义并在读取侧支持，等 B-rep 落地后填内容。

mod format;

pub use format::{
    entry_bytes, parse_entries, Builder, Reader, SectionEntry, SectionKind, FLAG_DISCARDABLE,
    MAGIC_V1, MAGIC_V2,
};

use archcore_ir::Scene;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::IoError;

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
    /// 容器内相对路径，形如 `assets/asset-1.png`
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

/// 打包成 v2 存档：场景与贴图字节收进一个文件，原子落盘。
pub fn pack(scene: &Scene, out: &Path) -> Result<PackReport, IoError> {
    let mut scene = scene.clone();
    let mut report = PackReport::default();
    let mut b = Builder::new();

    // slot 0 固定给场景；资产从 slot 1 起，按 scene.assets 下标对齐
    b.push_with_slot(0, SectionKind::Scene, 0, Vec::new());

    for (i, asset) in scene.assets.iter_mut().enumerate() {
        let uri = asset.uri.clone();
        let ext = ext_of(&uri);
        let rel = format!("assets/{}.{ext}", asset.id);
        match std::fs::read(&uri) {
            Ok(bytes) => {
                asset.uri = rel;
                report.assets_packed += 1;
                b.push_with_slot(1 + i, SectionKind::Asset, 0, bytes);
            }
            Err(e) => {
                report
                    .warnings
                    .push(format!("贴图缺失，未打包：{uri}（{e}）"));
            }
        }
    }

    let scene_bytes = serde_json::to_vec(&scene)
        .map_err(|e| IoError::Format(format!("场景序列化失败：{e}")))?;
    b.push_with_slot(0, SectionKind::Scene, 0, scene_bytes);

    let prov = json!({
        "created_by": format!("archbloom {}", env!("CARGO_PKG_VERSION")),
        "archive_version": 2,
        "assets_packed": report.assets_packed,
    });
    b.push(
        SectionKind::Provenance,
        0,
        serde_json::to_vec(&prov).map_err(|e| IoError::Format(format!("溯源段序列化失败：{e}")))?,
    );

    b.write_atomic(out)?;
    Ok(report)
}

/// 读取存档（自动识别 v1 / v2）。
pub fn read(data: &[u8]) -> Result<Archive, IoError> {
    if data.len() >= 8 && data[0..8] == MAGIC_V2 {
        read_v2(data)
    } else if data.len() >= 8 && data[0..8] == MAGIC_V1 {
        read_v1(data)
    } else if data.len() < 8 {
        Err(IoError::Format(format!(
            ".al 文件过短（{} 字节，至少 8 字节才能识别魔数）",
            data.len()
        )))
    } else {
        Err(IoError::Format(
            "不是 Archbloom 存档：魔数不符（文件头应为 ALBLOOM1/ALBLOOM2）".into(),
        ))
    }
}

fn read_v2(data: &[u8]) -> Result<Archive, IoError> {
    let (_, entries) = parse_entries(data)?;

    let scene_entry = entries
        .iter()
        .find(|e| e.kind == SectionKind::Scene.as_u16())
        .ok_or_else(|| IoError::Format("存档缺少 scene 段".into()))?;
    let scene_bytes = entry_bytes(data, scene_entry)?;
    let scene: Scene = serde_json::from_slice(scene_bytes)
        .map_err(|e| IoError::Format(format!("内嵌场景解析失败：{e}")))?;

    // 资产段按 slot 对齐：slot = 1 + asset 下标
    let mut files = Vec::new();
    for e in &entries {
        if e.kind != SectionKind::Asset.as_u16() || e.size == 0 {
            continue;
        }
        let Some(idx) = (e.id as usize).checked_sub(1) else {
            continue;
        };
        let Some(asset) = scene.assets.get(idx) else {
            continue;
        };
        let ext = ext_of(&asset.uri);
        files.push(ArchiveFile {
            path: asset.uri.clone(),
            mime: mime_of_ext(&ext).to_string(),
            bytes: entry_bytes(data, e)?.to_vec(),
        });
    }

    Ok(Archive { scene, files })
}

/// v1 单块布局解析。保留它只为能打开老文件，不再产生 v1。
fn read_v1(data: &[u8]) -> Result<Archive, IoError> {
    const PREFIX_LEN: usize = 12;
    if data.len() < PREFIX_LEN {
        return Err(IoError::Format(format!(
            ".al 文件过短（{} 字节，至少需要 {PREFIX_LEN}）",
            data.len()
        )));
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
    if header.get("format").and_then(|v| v.as_str()) != Some("archbloom-archive") {
        return Err(IoError::Format("header 缺少 format 标记".into()));
    }
    let version = header.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
    if version != 1 {
        return Err(IoError::Format(format!(
            "v1 存档版本不符：{version}（期望 1）"
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
    let scene: Scene = serde_json::from_slice(&slice(scene_obj, "scene")?)
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
            "al_hi_{}_{}_{tag}",
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
        assert_eq!(&data[0..8], &MAGIC_V2, "新存档必须是 v2");

        let arch = read(&data).unwrap();
        assert_eq!(arch.scene.assets.len(), 1);
        assert!(arch.scene.assets[0].uri.starts_with("assets/"));
        assert_eq!(arch.files.len(), 1);
        assert_eq!(arch.files[0].bytes, b"\x89PNG\r\n\x1a\nfakebytes");
        assert_eq!(arch.files[0].mime, "image/png");

        let out = d.join("extract");
        let restored = extract_to(&arch, &out).unwrap();
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

        // 缺的那个资产不该被读成一个空文件
        let arch = read(&std::fs::read(&al).unwrap()).unwrap();
        assert_eq!(arch.files.len(), 1);
    }

    #[test]
    fn 坏魔数报格式错误不panic() {
        let err = read(b"NOTALOOMxxxxxxxx").unwrap_err();
        assert!(matches!(err, IoError::Format(_)));
        assert!(err.to_string().contains("魔数"));
    }

    #[test]
    fn 过短文件报错不panic() {
        let err = read(b"ALB").unwrap_err();
        assert!(matches!(err, IoError::Format(_)));
    }

    #[test]
    fn v1头部截断报错不panic() {
        let mut data = Vec::new();
        data.extend_from_slice(&MAGIC_V1);
        data.extend_from_slice(&999u32.to_le_bytes());
        data.extend_from_slice(b"{}");
        let err = read(&data).unwrap_err();
        assert!(matches!(err, IoError::Format(_)));
        assert!(err.to_string().contains("越界"), "实际：{err}");
    }

    #[test]
    fn v1_offset越界报错不panic() {
        let header = json!({
            "format": "archbloom-archive", "version": 1,
            "scene": {"offset": 0, "size": 10},
            "assets": []
        });
        let hb = serde_json::to_vec(&header).unwrap();
        let mut data = Vec::new();
        data.extend_from_slice(&MAGIC_V1);
        data.extend_from_slice(&(hb.len() as u32).to_le_bytes());
        data.extend_from_slice(&hb);
        data.extend_from_slice(b"short");
        let err = read(&data).unwrap_err();
        assert!(matches!(err, IoError::Format(_)));
        assert!(err.to_string().contains("越界"), "实际：{err}");
    }

    #[test]
    fn v1目录穿越路径被拒() {
        let scene = json!({
            "schema_version": "0", "units": "m", "up_axis": "z_up", "name": "s",
            "objects": [], "materials": [], "assets": [], "meta": {}
        });
        let sb = serde_json::to_vec(&scene).unwrap();
        let header = json!({
            "format": "archbloom-archive", "version": 1,
            "scene": {"offset": 0, "size": sb.len()},
            "assets": [{"path": "../../evil.png", "mime": "image/png", "offset": sb.len(), "size": 1}]
        });
        let hb = serde_json::to_vec(&header).unwrap();
        let mut data = Vec::new();
        data.extend_from_slice(&MAGIC_V1);
        data.extend_from_slice(&(hb.len() as u32).to_le_bytes());
        data.extend_from_slice(&hb);
        data.extend_from_slice(&sb);
        data.extend_from_slice(b"x");
        let err = read(&data).unwrap_err();
        assert!(matches!(err, IoError::Format(_)));
        assert!(err.to_string().contains("非法 asset 路径"), "实际：{err}");
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

    #[test]
    fn v2带溯源段() {
        let d = tmpdir("prov");
        let scene = scene_with_texture(&d, false);
        let al = d.join("p.al");
        pack(&scene, &al).unwrap();
        let r = Reader::open(&al).unwrap();
        let prov = r.section(SectionKind::Provenance).unwrap().unwrap();
        let v: Value = serde_json::from_slice(prov).unwrap();
        assert!(v["created_by"].as_str().unwrap().starts_with("archbloom"));
    }
}
