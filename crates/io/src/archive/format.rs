//! `.al` v2 段表格式：布局、CRC 校验、原子写。
//!
//! # 布局
//!
//! ```text
//! [0..8)    魔数 b"ALBLOOM2"
//! [8..12)   version u32
//! [12..16)  段数 u32
//! [16..)    段表：N × 32 字节
//!             { id u32, kind u16, flags u16, offset u64, size u64, crc32 u32, reserved u32 }
//! [..]      段数据区（各段首尾相接）
//! ```
//!
//! # 为什么是段表而不是整块
//!
//! 三个诉求只能靠段表满足：
//! - **随机读**：agent 只要一个实体的几何，不该读整个文件；
//! - **可检错**：每段独立 CRC，坏了一段能指名道姓；
//! - **可裁剪**：`FLAG_DISCARDABLE` 标出派生数据（网格缓存），脱机瘦身时能丢掉。
//!
//! # 为什么 v2 不做压缩
//!
//! 压缩破坏随机访问（要解压整段）。v2 的首要目标是「能定位」，不是「文件最小」。
//! 段布局已经为 per-section 压缩留好位置，留到 v3。

use std::fs::File;
use std::io::Write;
use std::path::Path;

use crate::IoError;

pub const MAGIC_V1: [u8; 8] = *b"ALBLOOM1";
pub const MAGIC_V2: [u8; 8] = *b"ALBLOOM2";
/// 魔数(8) + version(4) + 段数(4)
pub const HEADER_LEN: usize = 16;
/// 段表项长度：id(4)+kind(2)+flags(2)+offset(8)+size(8)+crc32(4)+reserved(4)
pub const ENTRY_LEN: usize = 32;

/// 段类型。数值一经发布不得改动（存档要能长期读）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u16)]
pub enum SectionKind {
    /// 场景 IR（JSON）
    Scene = 1,
    /// 二进制资产（贴图等）
    Asset = 2,
    /// 精确几何（B-rep）—— D 阶段填充，当前只读不写
    GeoBrep = 3,
    /// 三角网格缓存（可从 GeoBrep 重算，因此可丢）
    GeoMesh = 4,
    /// 几何事实面（双哈希/体积等）
    Facts = 5,
    /// 生成溯源（内核版本、创建者）
    Provenance = 6,
}

impl SectionKind {
    pub fn as_u16(self) -> u16 {
        self as u16
    }

    /// 未知类型返回 `None`——读取方应当跳过而不是报错（向前兼容）。
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            1 => Some(Self::Scene),
            2 => Some(Self::Asset),
            3 => Some(Self::GeoBrep),
            4 => Some(Self::GeoMesh),
            5 => Some(Self::Facts),
            6 => Some(Self::Provenance),
            _ => None,
        }
    }
}

/// 派生数据标记：可从其它段重算，允许被裁剪。
pub const FLAG_DISCARDABLE: u16 = 1 << 0;

#[derive(Clone, Copy, Debug)]
pub struct SectionEntry {
    pub id: u32,
    pub kind: u16,
    pub flags: u16,
    pub offset: u64,
    pub size: u64,
    pub crc32: u32,
}

impl SectionEntry {
    pub fn is_discardable(&self) -> bool {
        self.flags & FLAG_DISCARDABLE != 0
    }

    pub fn kind_name(&self) -> &'static str {
        match SectionKind::from_u16(self.kind) {
            Some(SectionKind::Scene) => "scene",
            Some(SectionKind::Asset) => "asset",
            Some(SectionKind::GeoBrep) => "geo_brep",
            Some(SectionKind::GeoMesh) => "geo_mesh",
            Some(SectionKind::Facts) => "facts",
            Some(SectionKind::Provenance) => "provenance",
            None => "unknown",
        }
    }
}

/// 写入端：先攒段，再一次性构建与落盘。
pub struct Builder {
    sections: Vec<(u16, u16, Vec<u8>)>,
}

impl Builder {
    pub fn new() -> Self {
        Self { sections: vec![] }
    }

    pub fn push(&mut self, kind: SectionKind, flags: u16, bytes: Vec<u8>) {
        self.sections.push((kind.as_u16(), flags, bytes));
    }

    /// 按指定 id 补一个段。
    ///
    /// 用于需要「段 ↔ 场景记录」一一对应的场合：资产段用 `scene.assets` 的下标作 id，
    /// 读取时就不靠顺序配对（顺序一旦错位会把贴图贴到别的材质上，而且不报错）。
    pub fn push_with_slot(&mut self, slot: usize, kind: SectionKind, flags: u16, bytes: Vec<u8>) {
        while self.sections.len() < slot {
            // 占位段：类型写 0（未知），读取方会跳过
            self.sections.push((0, 0, Vec::new()));
        }
        if self.sections.len() == slot {
            self.sections.push((kind.as_u16(), flags, bytes));
        } else {
            self.sections[slot] = (kind.as_u16(), flags, bytes);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// 拼出完整字节（含段表）。
    pub fn build(&self) -> Vec<u8> {
        let table_len = self.sections.len() * ENTRY_LEN;
        let mut offset = (HEADER_LEN + table_len) as u64;

        let mut entries: Vec<SectionEntry> = Vec::with_capacity(self.sections.len());
        for (i, (kind, flags, bytes)) in self.sections.iter().enumerate() {
            entries.push(SectionEntry {
                id: i as u32,
                kind: *kind,
                flags: *flags,
                offset,
                size: bytes.len() as u64,
                crc32: crc32fast::hash(bytes),
            });
            offset += bytes.len() as u64;
        }

        let mut out = Vec::with_capacity(offset as usize);
        out.extend_from_slice(&MAGIC_V2);
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(self.sections.len() as u32).to_le_bytes());
        for e in &entries {
            out.extend_from_slice(&e.id.to_le_bytes());
            out.extend_from_slice(&e.kind.to_le_bytes());
            out.extend_from_slice(&e.flags.to_le_bytes());
            out.extend_from_slice(&e.offset.to_le_bytes());
            out.extend_from_slice(&e.size.to_le_bytes());
            out.extend_from_slice(&e.crc32.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes()); // reserved
        }
        for (_, _, bytes) in &self.sections {
            out.extend_from_slice(bytes);
        }
        out
    }

    /// 原子落盘：临时文件 → fsync → rename。
    ///
    /// 为什么必须这样：直接写目标文件时，中途断电/磁盘满会把旧存档截断成半截，
    /// 而旧版本已经没了。rename 在同一分区上是原子的，要么全新要么全旧。
    pub fn write_atomic(&self, path: &Path) -> Result<(), IoError> {
        let bytes = self.build();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| IoError::Io(format!("创建目录失败 {}：{e}", parent.display())))?;
            }
        }
        let tmp = path.with_extension("al.tmp");
        {
            let mut f = File::create(&tmp)
                .map_err(|e| IoError::Io(format!("创建临时文件失败 {}：{e}", tmp.display())))?;
            f.write_all(&bytes)
                .map_err(|e| IoError::Io(format!("写入临时文件失败：{e}")))?;
            // fsync：确保内容真的落盘，不只是进了页缓存
            f.sync_all()
                .map_err(|e| IoError::Io(format!("fsync 失败：{e}")))?;
        }
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            IoError::Io(format!("替换目标文件失败 {}：{e}", path.display()))
        })
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

/// 从字节解析段表（不持有数据）。供 mmap 读取器与内存读取路径共用。
pub fn parse_entries(data: &[u8]) -> Result<(u32, Vec<SectionEntry>), IoError> {
    let len = data.len();
    if len >= 8 && data[0..8] == MAGIC_V1 {
        return Err(IoError::Format(
            "这是 v1 存档（单块布局），请走 v1 读取路径".into(),
        ));
    }
    if len < HEADER_LEN {
        return Err(IoError::Format(format!(
            "v2 头部不完整（{len} 字节，需要 {HEADER_LEN}）"
        )));
    }
    if data[0..8] != MAGIC_V2 {
        return Err(IoError::Format(
            "不是 Archbloom 存档：魔数不符（文件头应为 ALBLOOM1/ALBLOOM2）".into(),
        ));
    }
    let version = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
    if version != 2 {
        return Err(IoError::Format(format!(
            "存档版本不支持：{version}（本程序支持 2）"
        )));
    }
    let count = u32::from_le_bytes([data[12], data[13], data[14], data[15]]) as usize;
    let table_end = HEADER_LEN
        .checked_add(count.saturating_mul(ENTRY_LEN))
        .ok_or_else(|| IoError::Format("段表长度溢出".into()))?;
    if table_end > len {
        return Err(IoError::Format(format!(
            "段表越界：声明 {count} 段（需 {table_end} 字节），文件仅 {len} 字节"
        )));
    }

    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let base = HEADER_LEN + i * ENTRY_LEN;
        let e = SectionEntry {
            id: u32::from_le_bytes([data[base], data[base + 1], data[base + 2], data[base + 3]]),
            kind: u16::from_le_bytes([data[base + 4], data[base + 5]]),
            flags: u16::from_le_bytes([data[base + 6], data[base + 7]]),
            offset: u64::from_le_bytes(data[base + 8..base + 16].try_into().unwrap_or([0; 8])),
            size: u64::from_le_bytes(data[base + 16..base + 24].try_into().unwrap_or([0; 8])),
            crc32: u32::from_le_bytes([
                data[base + 24],
                data[base + 25],
                data[base + 26],
                data[base + 27],
            ]),
        };
        let end = e
            .offset
            .checked_add(e.size)
            .ok_or_else(|| IoError::Format(format!("第 {i} 段长度溢出")))?;
        if end > len as u64 {
            return Err(IoError::Format(format!(
                "第 {i} 段越界：offset {} + size {} > 文件 {len}",
                e.offset, e.size
            )));
        }
        entries.push(e);
    }
    Ok((version, entries))
}

/// 校验并取某段字节（内存路径用）。
pub fn entry_bytes<'a>(data: &'a [u8], e: &SectionEntry) -> Result<&'a [u8], IoError> {
    let start = e.offset as usize;
    let end = start.checked_add(e.size as usize).ok_or_else(|| IoError::Format("段长度溢出".into()))?;
    if end > data.len() {
        return Err(IoError::Format(format!(
            "段 {} 越界（{start}..{end} > {}）",
            e.kind_name(),
            data.len()
        )));
    }
    let bytes = &data[start..end];
    let actual = crc32fast::hash(bytes);
    if actual != e.crc32 {
        return Err(IoError::Format(format!(
            "段 {} 校验失败（CRC 期望 {} 实为 {}）——文件已损坏",
            e.kind_name(),
            e.crc32,
            actual
        )));
    }
    Ok(bytes)
}

/// 读取端：mmap 映射，按段取字节。
#[derive(Debug)]
pub struct Reader {
    map: memmap2::Mmap,
    /// 文件长度快照：mmap 后文件若被外部截短会导致 SIGBUS，所以一切边界检查都用快照
    len: usize,
    pub version: u32,
    pub entries: Vec<SectionEntry>,
}

impl Reader {
    /// 打开并解析段表。**不解析场景**——这就是「快速打开」的来源。
    pub fn open(path: &Path) -> Result<Self, IoError> {
        let file = File::open(path)
            .map_err(|e| IoError::Io(format!("打开失败 {}：{e}", path.display())))?;
        let len = file
            .metadata()
            .map_err(|e| IoError::Io(format!("读取元信息失败：{e}")))?
            .len() as usize;
        if len < 8 {
            return Err(IoError::Format(format!(
                "文件过短（{len} 字节，至少 8 字节才能识别魔数）"
            )));
        }
        // SAFETY: 只读映射；文件句柄在 map 建立后即可释放，映射本身持有底层句柄。
        // 边界检查一律用打开时的 len 快照，不信任文件后续变化。
        let map = unsafe { memmap2::Mmap::map(&file) }
            .map_err(|e| IoError::Io(format!("内存映射失败：{e}")))?;

        if map[0..8] == MAGIC_V2 {
            Self::parse_v2(map, len)
        } else if map[0..8] == MAGIC_V1 {
            Err(IoError::Format(
                "这是 v1 存档（单块布局）。请用 `open_any` 读取，或先用旧版本另存。".into(),
            ))
        } else {
            Err(IoError::Format(
                "不是 Archbloom 存档：魔数不符（文件头应为 ALBLOOM1/ALBLOOM2）".into(),
            ))
        }
    }

    fn parse_v2(map: memmap2::Mmap, len: usize) -> Result<Self, IoError> {
        let (version, entries) = parse_entries(&map)?;
        Ok(Self { map, len, version, entries })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 按类型找第一段。
    pub fn find(&self, kind: SectionKind) -> Option<usize> {
        self.entries.iter().position(|e| e.kind == kind.as_u16())
    }

    /// 取段字节并校验 CRC。
    ///
    /// 校验发生在**读取时**而不是打开时：打开只读段表（快），
    /// 真正用到哪段才验哪段——坏段不会拖慢没用到它的场景。
    pub fn bytes(&self, idx: usize) -> Result<&[u8], IoError> {
        let e = self
            .entries
            .get(idx)
            .ok_or_else(|| IoError::Format(format!("段 {idx} 不存在")))?;
        let start = e.offset as usize;
        let end = start + e.size as usize;
        // 双保险：offset/size 已在 parse 时校验过，这里再挡一次越界
        if end > self.len {
            return Err(IoError::Format(format!(
                "段 {} 越界（{start}..{end} > {}）",
                e.kind_name(),
                self.len
            )));
        }
        let data = &self.map[start..end];
        let actual = crc32fast::hash(data);
        if actual != e.crc32 {
            return Err(IoError::Format(format!(
                "段 {} 校验失败（CRC 期望 {} 实为 {}）——文件已损坏",
                e.kind_name(),
                e.crc32,
                actual
            )));
        }
        Ok(data)
    }

    /// 按类型取段字节。
    pub fn section(&self, kind: SectionKind) -> Result<Option<&[u8]>, IoError> {
        match self.find(kind) {
            Some(i) => Ok(Some(self.bytes(i)?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "al_v2_{}_{}_{}.al",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn 往返_段内容与crc都对() {
        let p = tmp("roundtrip");
        let mut b = Builder::new();
        b.push(SectionKind::Scene, 0, b"{\"a\":1}".to_vec());
        b.push(SectionKind::Asset, 0, vec![1, 2, 3, 4]);
        b.write_atomic(&p).unwrap();

        let r = Reader::open(&p).unwrap();
        assert_eq!(r.entries.len(), 2);
        assert_eq!(r.section(SectionKind::Scene).unwrap().unwrap(), b"{\"a\":1}");
        assert_eq!(r.section(SectionKind::Asset).unwrap().unwrap(), &[1, 2, 3, 4]);
        assert!(r.section(SectionKind::GeoBrep).unwrap().is_none());
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 原子写覆盖旧文件不残留临时件() {
        let p = tmp("atomic");
        let mut b = Builder::new();
        b.push(SectionKind::Scene, 0, b"first".to_vec());
        b.write_atomic(&p).unwrap();
        let mut b2 = Builder::new();
        b2.push(SectionKind::Scene, 0, b"second".to_vec());
        b2.write_atomic(&p).unwrap();

        let r = Reader::open(&p).unwrap();
        assert_eq!(r.section(SectionKind::Scene).unwrap().unwrap(), b"second");
        assert!(!p.with_extension("al.tmp").exists(), "临时文件必须被清掉");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn crc不符时报错并指名段() {
        let p = tmp("crc");
        let mut b = Builder::new();
        b.push(SectionKind::Scene, 0, b"abcdef".to_vec());
        b.write_atomic(&p).unwrap();
        // 篡改段数据区最后一个字节
        let mut bytes = std::fs::read(&p).unwrap();
        let n = bytes.len();
        bytes[n - 1] ^= 0xFF;
        std::fs::write(&p, &bytes).unwrap();

        let r = Reader::open(&p).unwrap(); // 打开只看段表，不校验
        let err = r.bytes(0).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("校验失败"), "应报 CRC 失败，实际：{msg}");
        assert!(msg.contains("scene"), "应指名段类型，实际：{msg}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 段表越界在打开时就拦住() {
        let p = tmp("badtable");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC_V2);
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&99u32.to_le_bytes()); // 谎报 99 段
        std::fs::write(&p, &bytes).unwrap();
        let err = Reader::open(&p).unwrap_err();
        assert!(err.to_string().contains("越界"), "实际：{err}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 魔数不符报错不panic() {
        let p = tmp("badmagic");
        std::fs::write(&p, b"NOTALOOMxxxxxxxx").unwrap();
        let err = Reader::open(&p).unwrap_err();
        assert!(err.to_string().contains("魔数"), "实际：{err}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 未知段类型可跳过() {
        let p = tmp("unknown");
        let mut b = Builder::new();
        b.push(SectionKind::Scene, 0, b"scene".to_vec());
        // 手搓一个未知 kind=999 的段
        b.sections.push((999, 0, b"future".to_vec()));
        b.write_atomic(&p).unwrap();

        let r = Reader::open(&p).unwrap();
        assert_eq!(r.entries.len(), 2);
        assert!(SectionKind::from_u16(999).is_none());
        // 未知段不应阻止读取已知段
        assert_eq!(r.section(SectionKind::Scene).unwrap().unwrap(), b"scene");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn 可丢弃标记可查询() {
        let mut b = Builder::new();
        b.push(SectionKind::GeoMesh, FLAG_DISCARDABLE, vec![0; 4]);
        b.push(SectionKind::GeoBrep, 0, vec![0; 4]);
        let bytes = b.build();
        std::fs::write(tmp("flags"), &bytes).ok();
        let p = tmp("flags");
        std::fs::write(&p, &bytes).unwrap();
        let r = Reader::open(&p).unwrap();
        assert!(r.entries[0].is_discardable());
        assert!(!r.entries[1].is_discardable());
        std::fs::remove_file(&p).ok();
    }
}
