//! Windows file identity for change detection: what Unix code reads as
//! (inode, size, mode, ctime). The change time (`FILE_BASIC_INFO
//! .ChangeTime`) moves on every write or metadata change and, unlike the
//! last-write time, ordinary file APIs (`SetFileTime`, std's
//! `set_modified`) can't put it back.

use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{
    FileBasicInfo, GetFileInformationByHandle, GetFileInformationByHandleEx, BY_HANDLE_FILE_INFORMATION,
    FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

/// One path's identity: file index (the inode's role), size, attributes
/// and change time (100 ns since 1601).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeStamp {
    pub file_index: u64,
    pub size: u64,
    pub attrs: u32,
    pub change_time: i64,
}

impl ChangeStamp {
    /// The change time as Unix (seconds, nanoseconds), as `ctime` reads.
    pub fn change_unix(&self) -> (i64, i64) {
        /// 100 ns ticks from 1601-01-01 to 1970-01-01.
        const EPOCH_DIFF: i64 = 116_444_736_000_000_000;
        let t = self.change_time - EPOCH_DIFF;
        (t.div_euclid(10_000_000), t.rem_euclid(10_000_000) * 100)
    }
}

/// The stamp of `p` itself (a link is not followed; a folder works too).
/// `None` when it can't be opened for its attributes.
pub fn change_stamp(p: &Path) -> Option<ChangeStamp> {
    let f = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(p)
        .ok()?;
    let h = f.as_raw_handle();
    // SAFETY: a live handle opened above and zeroed out-structs of the
    // size each call is told.
    unsafe {
        let mut by: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(h, &mut by) == 0 {
            return None;
        }
        let mut basic: FILE_BASIC_INFO = std::mem::zeroed();
        if GetFileInformationByHandleEx(
            h,
            FileBasicInfo,
            &mut basic as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        ) == 0
        {
            return None;
        }
        Some(ChangeStamp {
            file_index: (u64::from(by.nFileIndexHigh) << 32) | u64::from(by.nFileIndexLow),
            size: (u64::from(by.nFileSizeHigh) << 32) | u64::from(by.nFileSizeLow),
            attrs: by.dwFileAttributes,
            change_time: basic.ChangeTime,
        })
    }
}

/// What a disk walk needs to count a file once (Unix: dev, ino, nlink).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    pub volume: u32,
    pub file_index: u64,
    pub links: u32,
}

/// The identity of `p` itself (a link is not followed). `None` when it
/// can't be opened for its attributes.
pub fn file_identity(p: &Path) -> Option<FileIdentity> {
    let f = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(p)
        .ok()?;
    // SAFETY: a live handle opened above and a zeroed out-struct.
    unsafe {
        let mut by: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(f.as_raw_handle(), &mut by) == 0 {
            return None;
        }
        Some(FileIdentity {
            volume: by.dwVolumeSerialNumber,
            file_index: (u64::from(by.nFileIndexHigh) << 32) | u64::from(by.nFileIndexLow),
            links: by.nNumberOfLinks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rewrite_with_the_old_mtime_put_back_still_changes_the_stamp() {
        let d = crate::test_env::unique_temp_path("win-fs");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("f.json");
        std::fs::write(&p, b"aaaa").unwrap();
        let before = change_stamp(&p).expect("stamp");
        let mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&p, b"bbbb").unwrap();
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(mtime).unwrap();
        let after = change_stamp(&p).expect("stamp");
        assert_eq!((before.file_index, before.size), (after.file_index, after.size));
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), mtime, "the mtime was put back");
        assert_ne!(before.change_time, after.change_time, "the change time can't be");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        assert!((after.change_unix().0 - now).abs() < 60, "{:?} vs {now}", after.change_unix());
        assert!(change_stamp(&d).is_some(), "a folder has a stamp");
        assert!(change_stamp(&d.join("missing")).is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
