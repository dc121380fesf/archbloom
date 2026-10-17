//! `.al` v2 性能基准：守住「可定位」这个承诺。
//!
//! # 两个口径必须分开看
//!
//! - **打开**（`Reader::open`）：只解析段表 + mmap，不碰段内容 → 这才是「秒开」的来源；
//! - **取段**（`Reader::section`）：读字节 + CRC 校验 → 与被读段的大小成正比。
//!
//! 把两者混成一个「打开耗时」是自欺——100MB 的场景不管怎么优化，
//! 完整解析 JSON 都要几百毫秒到几秒。
//!
//! # 阈值为什么这么松
//!
//! 这是 `cargo test` 的 debug 构建，比 release 慢一个数量级；且 CI runner 抖动大。
//! 阈值只用来抓「数量级退化」（比如有人把打开改成读全文件），不追求精确判定。
//! release 实测值在 println 里，看日志即可。

use std::io::{Seek, SeekFrom, Write};
use std::time::Instant;

use archcore_io::archive::{Builder, Reader, SectionKind};

const MB: usize = 1024 * 1024;

fn tmp_path(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "al_bench_{}_{}_{tag}.al",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_file(&p);
    p
}

/// 造一个约 `total_mb` 的存档：1 个场景段 + N 个资产段。
fn make_blob(total_mb: usize, chunk_mb: usize) -> (Builder, usize) {
    let mut b = Builder::new();
    b.push(SectionKind::Scene, 0, br#"{"schema_version":"0"}"#.to_vec());
    let chunks = total_mb / chunk_mb;
    let payload = vec![0xA5u8; chunk_mb * MB];
    for i in 0..chunks {
        // 每个 chunk 打一个不同标记，便于事后确认读到的确实是目标段
        let mut c = payload.clone();
        c[0] = i as u8;
        b.push_with_slot(1 + i, SectionKind::Asset, 0, c);
    }
    (b, chunks)
}

#[test]
fn bench_al_v2_保存_打开_取段() {
    let total_mb = 100;
    let chunk_mb = 1;
    let (builder, chunks) = make_blob(total_mb, chunk_mb);

    // ── 保存 ──
    let path = tmp_path("save");
    let t = Instant::now();
    builder.write_atomic(&path).unwrap();
    let save_ms = t.elapsed().as_millis();
    let size = std::fs::metadata(&path).unwrap().len();
    println!(
        "[.al] 保存 {} MB → {} ms（debug）",
        size / MB as u64,
        save_ms
    );

    // ── 打开（只读段表） ──
    let t = Instant::now();
    let reader = Reader::open(&path).unwrap();
    let open_ms = t.elapsed().as_millis();
    println!("[.al] 打开（段表）→ {} ms，{} 段", open_ms, reader.entries.len());
    assert_eq!(reader.entries.len(), chunks + 1, "场景 + assets（本基准不写溯源段）");

    // ── 随机取一段（中位那个） ──
    let mid = 1 + chunks / 2;
    let t = Instant::now();
    let bytes = reader.bytes(mid).unwrap();
    let read_ms = t.elapsed().as_millis();
    println!("[.al] 随机取 1 MB 段 → {} ms", read_ms);
    assert_eq!(bytes.len(), chunk_mb * MB);
    assert_eq!(bytes[0], (mid - 1) as u8, "读到的必须是目标段");

    // ── 阈值（宽松，只抓数量级退化） ──
    assert!(open_ms < 500, "打开超过 500ms（debug），说明读了段内容而不是只看段表");
    assert!(read_ms < 100, "取单个 1MB 段超过 100ms（debug）");
    assert!(save_ms < 10_000, "保存 100MB 超过 10s（debug）");

    // ── 完整性：段被篡改必须被 CRC 拦下 ──
    drop(reader);
    let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.seek(SeekFrom::End(-1)).unwrap();
    f.write_all(&[0x00]).unwrap();
    f.sync_all().unwrap();
    drop(f);
    let r2 = Reader::open(&path).unwrap();
    let last = r2.entries.len() - 1;
    let err = r2.bytes(last).unwrap_err();
    assert!(
        err.to_string().contains("校验失败"),
        "篡改后必须报 CRC 失败，实际：{err}"
    );

    std::fs::remove_file(&path).ok();
}

#[test]
fn bench_al_v2_空资产段不被当成贴图() {
    // 回归：缺贴图时用占位空段，读取端不能把它读成一个 0 字节的贴图
    let mut b = Builder::new();
    b.push(SectionKind::Scene, 0, b"{}".to_vec());
    b.push_with_slot(1, SectionKind::Asset, 0, Vec::new());
    let path = tmp_path("empty");
    b.write_atomic(&path).unwrap();
    let r = Reader::open(&path).unwrap();
    let e = &r.entries[1];
    assert_eq!(e.kind, SectionKind::Asset.as_u16());
    assert_eq!(e.size, 0);
    // 空段本身是合法的，CRC 是空内容的 hash
    assert!(r.bytes(1).is_ok());
    std::fs::remove_file(&path).ok();
}
