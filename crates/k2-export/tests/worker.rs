//! The real `k2-export` binary through the host runner: hello, describe, an
//! as-is export byte for byte, and the protocol's refusals.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use k2_export::host::{self, Bounds, RunError, Spawn};
use k2_export::proto::{Header, Kind, Limits};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-export-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("tmp dir");
    d
}

fn spawn(dir: &PathBuf) -> Spawn {
    Spawn { bin: PathBuf::from(env!("CARGO_BIN_EXE_k2-export")), cwd: dir.clone() }
}

fn bounds() -> Bounds {
    Bounds { wall: Duration::from_secs(30), memory_bytes: 512 << 20, max_output_bytes: 64 << 20 }
}

fn limits(size: u64) -> Limits {
    Limits {
        max_input_bytes: size.max(1),
        max_output_bytes: 64 << 20,
        max_pixels: 1 << 20,
        max_pages: 10,
        max_wall_ms: 30_000,
        memory_bytes: 512 << 20,
    }
}

#[test]
fn version_flag_exits_zero_without_stdin() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_k2-export")).arg("--version").output().expect("run");
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("k2-export "), "{out:?}");
}

#[test]
fn describe_offers_as_is_for_any_kind() {
    let d = tmp("describe");
    for kind in [Kind::Pack("markdown".into()), Kind::Format("png".into()), Kind::Format(String::new())] {
        let (hello, targets) = host::describe(&spawn(&d), &kind, bounds()).expect("describe");
        assert_eq!(hello.version, k2_export::proto::VERSION);
        assert!(hello.exporters.iter().any(|e| e.starts_with("k2-mpc-base-as-is@")), "{hello:?}");
        assert!(targets.iter().any(|t| t["id"] == "as-is" && t["fidelity"] == "lossless"), "{targets:?}");
    }
}

#[test]
fn as_is_round_trips_a_binary_file_byte_for_byte() {
    let d = tmp("asis");
    let input = d.join("in");
    let body: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
    std::fs::write(&input, &body).expect("write input");
    let header = Header::Export {
        job: "j1".into(),
        kind: Kind::Format("png".into()),
        target: "as-is".into(),
        options: vec![],
        limits: limits(body.len() as u64),
        name: "photo.png".into(),
        size: body.len() as u64,
    };
    let mut out = Vec::new();
    let never = AtomicBool::new(false);
    let done = host::run(&spawn(&d), &header, Some((&input, body.len() as u64)), bounds(), &never, &|_, _| {}, &mut out)
        .expect("export");
    assert_eq!(out, body);
    assert_eq!(done.bytes, body.len() as u64);
    assert_eq!(done.report["bytes"], serde_json::json!(body.len()));
}

#[test]
fn a_bad_option_and_an_unknown_target_are_the_kits_refusals() {
    let d = tmp("refuse");
    let input = d.join("in");
    std::fs::write(&input, b"x").expect("write");
    for (target, options, id, exit) in [
        ("as-is", vec![("dpi".to_string(), "300".to_string())], "MPC-E002", 2),
        ("pdf", vec![], "MPC-E001", 2),
    ] {
        let header = Header::Export {
            job: "j".into(),
            kind: Kind::Pack("markdown".into()),
            target: target.into(),
            options,
            limits: limits(1),
            name: "notes.md".into(),
            size: 1,
        };
        let never = AtomicBool::new(false);
        let mut out = Vec::new();
        let err = host::run(&spawn(&d), &header, Some((&input, 1)), bounds(), &never, &|_, _| {}, &mut out)
            .expect_err("refused");
        match err.error {
            RunError::Kit(v) => {
                assert_eq!(v["id"], serde_json::json!(id), "{v}");
                assert_eq!(v["exit"], serde_json::json!(exit), "{v}");
            }
            other => panic!("{target}: want the kit's {id}, got {other:?}"),
        }
    }
}

#[test]
fn a_missing_worker_binary_is_a_spawn_error() {
    let d = tmp("nobin");
    let s = Spawn { bin: d.join("no-such-worker"), cwd: d.clone() };
    let err = host::describe(&s, &Kind::Format("png".into()), bounds()).expect_err("no binary");
    assert!(matches!(err.error, RunError::Spawn(_)), "{err:?}");
}
