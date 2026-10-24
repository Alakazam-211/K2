//! T1 (prd-compute-update-continuity-v1 §6): the executor stages a build
//! under the units' `UMask=0077` (plist `Umask 63`). Its own test binary,
//! because the umask is process-wide: nothing else runs in this process.
//!
//! 0.46.3 and older created `.new` with `.mode(0o755)` and got 0700 under
//! that umask, so the node user couldn't exec it (203/EXEC) and every
//! update rolled back. `stage_build` must give 0755 whatever the umask.

use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

#[test]
fn a_build_staged_under_umask_077_is_0755_and_ours() {
    // SAFETY: umask has no failure mode; this test binary is single-purpose.
    unsafe { libc::umask(0o077) };
    let dir = k2_node::util::temp_dir("stage-umask");
    let bin = dir.join("k2-node");
    std::fs::write(&bin, b"old build").unwrap();

    // The old way, under the same umask: proves this test reproduces Bug 1.
    let old_way = dir.join("old-way.new");
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o755).open(&old_way).unwrap();
    assert_eq!(std::fs::metadata(&old_way).unwrap().permissions().mode() & 0o7777, 0o700, "umask 077 turns a 0755 create into 0700");

    let new = k2_node::admin::update::stage_build(&bin, b"new build bytes").unwrap();
    assert_eq!(new, dir.join("k2-node.new"));
    let m = std::fs::symlink_metadata(&new).unwrap();
    assert!(m.file_type().is_file());
    assert_eq!(m.permissions().mode() & 0o7777, 0o755, "staged 0755 under umask 077");
    assert_eq!(m.uid(), unsafe { libc::geteuid() }, "owned by the executor's uid (root in production)");
    assert_eq!(std::fs::read(&new).unwrap(), b"new build bytes");

    // A stale .new is replaced; a .new that is a symlink is refused.
    let again = k2_node::admin::update::stage_build(&bin, b"newer").unwrap();
    assert_eq!(std::fs::read(&again).unwrap(), b"newer");
    assert_eq!(std::fs::metadata(&again).unwrap().permissions().mode() & 0o7777, 0o755);
    std::fs::remove_file(&again).unwrap();
    std::os::unix::fs::symlink(&old_way, &again).unwrap();
    let err = k2_node::admin::update::stage_build(&bin, b"x").unwrap_err();
    assert!(err.contains("not a regular file"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}
