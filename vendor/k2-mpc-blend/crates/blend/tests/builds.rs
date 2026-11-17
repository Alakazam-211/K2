//! The program's builds of the pack's test scenes, kept in this repo (`tests/builds/`, see
//! `NOTICE.md` and `PROVENANCE.md`): `MANIFEST.tsv` checks every file's size and sha256, and
//! each release's saves are read against that release's own report the way the round trip's
//! builds are (`builds_support`). It runs in every gate and needs no environment.

mod builds_support;

#[test]
fn kept_builds_read_as_the_program_reports_them() {
    if let Err(e) = builds_support::row_kept_builds() {
        panic!("row failed at its step: {}", e.replace('\n', " | "));
    }
}
