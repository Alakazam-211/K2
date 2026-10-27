//! A file landing on THIS computer from a download (`src-tauri`
//! `local_download_chunk`): its name and its quarantine mark. Plain Rust so
//! it is tested without building the app shell.
//!
//! prd-garden-board-needs-v1 security review #4: a Garden widget's
//! `invoice<RLO>gnp.svg` would read as `invoicegvs.png`, so names lose
//! control and Unicode format characters; and a file a widget made is
//! marked `com.apple.quarantine` on macOS, so Gatekeeper asks before it
//! opens.

use std::path::Path;

/// Unicode format characters that change how a name reads: bidi
/// embeddings, overrides and isolates, zero-width characters, the BOM, the
/// soft hyphen.
pub fn is_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

/// A safe basename: separators, control and format characters stripped;
/// `download` when nothing is left.
pub fn sanitize_filename(filename: &str) -> String {
    let last = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    let cleaned: String = last.chars().filter(|c| !c.is_control() && !is_format_char(*c)).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "download".to_string()
    } else {
        cleaned.to_string()
    }
}

/// The value macOS writes for a downloaded file: flags 0081 (downloaded,
/// user approval needed), the time in hex, the app that wrote it.
pub fn quarantine_value(now_secs: u64) -> String {
    format!("0081;{now_secs:08x};K2;")
}

/// Mark a downloaded file with `com.apple.quarantine` (macOS).
#[cfg(target_os = "macos")]
pub fn set_quarantine(path: &Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let value = quarantine_value(now);
    let p = CString::new(path.as_os_str().as_bytes()).map_err(|e| format!("Cannot mark the file: {e}"))?;
    let name = CString::new("com.apple.quarantine").map_err(|e| format!("Cannot mark the file: {e}"))?;
    // SAFETY: both strings are NUL-terminated and live across the call; the
    // value pointer and length describe `value`'s bytes.
    let rc = unsafe { libc::setxattr(p.as_ptr(), name.as_ptr(), value.as_ptr() as *const libc::c_void, value.len(), 0, 0) };
    if rc != 0 {
        return Err(format!("Cannot mark the downloaded file: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

/// Elsewhere there is no such mark, and nothing to do.
#[cfg(not(target_os = "macos"))]
pub fn set_quarantine(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_lose_separators_controls_and_bidi_tricks() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("C:\\Users\\x\\a.png"), "a.png");
        assert_eq!(sanitize_filename("invoice\u{202E}gnp.svg"), "invoicegnp.svg");
        assert_eq!(sanitize_filename("a\u{2066}b\u{200B}c\u{FEFF}.png"), "abc.png");
        assert_eq!(sanitize_filename("tab\tnew\nline\u{0}.txt"), "tabnewline.txt");
        assert_eq!(sanitize_filename("\u{202E}"), "download");
        assert_eq!(sanitize_filename(".."), "download");
    }

    #[test]
    fn the_quarantine_value_is_the_downloaded_shape() {
        assert_eq!(quarantine_value(0x6720_aa00), "0081;6720aa00;K2;");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_marked_file_carries_the_quarantine_xattr() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let dir = std::env::temp_dir().join(format!("k2-quarantine-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let f = dir.join("board.png");
        std::fs::write(&f, b"x").expect("write");
        set_quarantine(&f).expect("mark");
        let p = CString::new(f.as_os_str().as_bytes()).expect("path");
        let name = CString::new("com.apple.quarantine").expect("name");
        let mut buf = [0u8; 128];
        // SAFETY: the buffer outlives the call and its length is passed.
        let n = unsafe { libc::getxattr(p.as_ptr(), name.as_ptr(), buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0, 0) };
        assert!(n > 0, "the xattr is set: {}", std::io::Error::last_os_error());
        let v = String::from_utf8_lossy(&buf[..n as usize]).to_string();
        assert!(v.starts_with("0081;") && v.ends_with(";K2;"), "{v}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn marking_is_a_no_op_off_macos() {
        assert_eq!(set_quarantine(Path::new("/nonexistent/k2-file")), Ok(()));
    }
}
