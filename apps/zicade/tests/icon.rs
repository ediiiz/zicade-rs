//! The embedded app icon (`assets/zicade.ico`) must be a well-formed, multi-size
//! Windows ICO: `build.rs` compiles it into the exe and the tray loads it at the
//! system small-icon size, so it needs both a small frame (tray, 16px) and a
//! large one (Explorer, 256px). This guards the asset deterministically, with no
//! desktop session or FFI — it just parses the ICONDIR header.

use std::path::Path;

fn icon_bytes() -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/zicade.ico");
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn u16le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

#[test]
fn app_icon_is_a_valid_multi_size_ico() {
    let b = icon_bytes();
    assert!(b.len() > 6, "icon file is too small to hold an ICONDIR");

    // ICONDIR: reserved=0, type=1 (icon), count>=1.
    assert_eq!(u16le(&b, 0), 0, "ICONDIR reserved field must be 0");
    assert_eq!(u16le(&b, 2), 1, "ICONDIR type must be 1 (icon)");
    let count = u16le(&b, 4) as usize;
    assert!(count >= 2, "expected several sizes, got {count}");

    // ICONDIRENTRY[count]: 16 bytes each; a width/height byte of 0 means 256.
    let mut sizes = Vec::with_capacity(count);
    for i in 0..count {
        let off = 6 + i * 16;
        assert!(b.len() >= off + 16, "icon directory entry {i} is truncated");
        let w = if b[off] == 0 { 256 } else { b[off] as u32 };
        let h = if b[off + 1] == 0 {
            256
        } else {
            b[off + 1] as u32
        };
        let bytes = u32le(&b, off + 8) as usize;
        let data_off = u32le(&b, off + 12) as usize;
        assert!(
            bytes > 0 && data_off >= 6 + count * 16 && data_off + bytes <= b.len(),
            "entry {i} ({w}x{h}) points outside the file"
        );
        sizes.push((w, h));
    }

    assert!(
        sizes.contains(&(16, 16)),
        "need a 16x16 frame for the tray; got {sizes:?}"
    );
    assert!(
        sizes.contains(&(256, 256)),
        "need a 256x256 frame for Explorer; got {sizes:?}"
    );
}
