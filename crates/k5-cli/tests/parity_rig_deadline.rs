//! The parity rig's deadline: a side that never ends fails its row fast,
//! naming the side, instead of hanging the test binary.
//!
//! The row is `compute wait <job>` with no answer queued: the stub's
//! default answer has no state, so the wait polls forever (the hang the
//! deadline was made for), on whichever side runs it first.

#![cfg(unix)]

mod common;

use common::parity::*;
use std::time::{Duration, Instant};

#[test]
fn a_side_that_never_ends_fails_its_row_fast() {
    let rig = Rig::new("compute", "deadline");
    let c = Case {
        deadline: Some(Duration::from_secs(3)),
        ..case(
            "a wait the stub never ends",
            &["wait", "aaaa1111"],
            &[],
            vec![],
            owner_and_port,
            0,
            1,
        )
    };
    let started = Instant::now();
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.check(&c)))
        .expect_err("a row whose side never ends must fail");
    let text = failed
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| failed.downcast_ref::<&str>().map(|s| s.to_string()))
        .expect("the failure's text");
    assert!(
        text.starts_with("a wait the stub never ends: the ") && text.contains(" side hung"),
        "{text}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the row took {:?} to fail",
        started.elapsed()
    );
}

/// [`run_within`] alone: a process that ends in time gives its output.
#[test]
fn a_side_that_ends_in_time_gives_its_output() {
    let mut cmd = std::process::Command::new("sh");
    cmd.args(["-c", "cat; echo err >&2; exit 3"]);
    let o = run_within(&mut cmd, Some(b"in\n"), Duration::from_secs(30), "sh");
    assert_eq!(o.status.code(), Some(3));
    assert_eq!(o.stdout, b"in\n");
    assert_eq!(o.stderr, b"err\n");
}
