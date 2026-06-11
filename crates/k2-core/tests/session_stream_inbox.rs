//! E2 tests for `awareness::inbox` — filesystem durable delivery.
//!
//! Each test uses a fresh `tempfile::TempDir` as inbox root so parallel
//! runs don't race on the filesystem. No singleton state to worry
//! about — the inbox module takes all roots as explicit parameters,
//! unlike the bus singleton.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use k2_core::awareness::{
    inbox, AgentAddress, AgentSignal, SignalKind, WorkspaceId,
};

fn workspace() -> WorkspaceId {
    WorkspaceId("k2so".into())
}

fn mk_signal(text: &str) -> AgentSignal {
    AgentSignal::new(
        AgentAddress::Agent {
            workspace: workspace(),
            name: "foo".into(),
        },
        AgentAddress::Agent {
            workspace: workspace(),
            name: "bar".into(),
        },
        SignalKind::Msg {
            text: text.to_string(),
        },
    )
}

/// Build a temp inbox root inside /tmp that this test owns. Each
/// call uses a unique path so two tests that share `bar` as an
/// agent name don't collide.
fn tmp_root(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "k2so-inbox-test-{}-{}-{}",
        tag,
        std::process::id(),
        uuid_ns()
    ));
    let _ = std::fs::remove_dir_all(&path);
    path
}

fn uuid_ns() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn signal_text(signal: &AgentSignal) -> Option<String> {
    match &signal.kind {
        SignalKind::Msg { text } => Some(text.clone()),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────
// Happy-path lifecycle
// ─────────────────────────────────────────────────────────────────────

#[test]
fn write_then_drain_returns_the_signal() {
    let root = tmp_root("basic");
    let signal = mk_signal("hello");
    let path = inbox::write(&root, "bar", &signal).expect("write");
    assert!(path.exists());

    let drained = inbox::drain(&root, "bar");
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].id, signal.id);
    assert_eq!(signal_text(&drained[0]), Some("hello".into()));

    // File is gone after drain.
    assert!(!path.exists());

    // Second drain is a no-op.
    assert!(inbox::drain(&root, "bar").is_empty());
}

#[test]
fn drain_returns_signals_in_temporal_order() {
    let root = tmp_root("ordering");
    let sig_a = mk_signal("first");
    inbox::write(&root, "bar", &sig_a).unwrap();
    // Sleep enough to guarantee distinct ns-timestamp prefixes.
    // A few hundred microseconds is plenty; we use 1ms for safety.
    thread::sleep(Duration::from_millis(1));
    let sig_b = mk_signal("second");
    inbox::write(&root, "bar", &sig_b).unwrap();
    thread::sleep(Duration::from_millis(1));
    let sig_c = mk_signal("third");
    inbox::write(&root, "bar", &sig_c).unwrap();

    let drained = inbox::drain(&root, "bar");
    assert_eq!(drained.len(), 3);
    assert_eq!(signal_text(&drained[0]), Some("first".into()));
    assert_eq!(signal_text(&drained[1]), Some("second".into()));
    assert_eq!(signal_text(&drained[2]), Some("third".into()));
}

#[test]
fn pending_count_matches_drain_len() {
    let root = tmp_root("count");
    for i in 0..5 {
        let sig = mk_signal(&format!("msg-{i}"));
        inbox::write(&root, "bar", &sig).unwrap();
        thread::sleep(Duration::from_micros(100));
    }
    assert_eq!(inbox::pending_count(&root, "bar"), 5);
    let drained = inbox::drain(&root, "bar");
    assert_eq!(drained.len(), 5);
    assert_eq!(inbox::pending_count(&root, "bar"), 0);
}

#[test]
fn drain_on_nonexistent_agent_is_empty() {
    let root = tmp_root("empty");
    // Nothing ever written for "nobody".
    assert!(inbox::drain(&root, "nobody").is_empty());
    assert_eq!(inbox::pending_count(&root, "nobody"), 0);
}

// ─────────────────────────────────────────────────────────────────────
// Concurrency
// ─────────────────────────────────────────────────────────────────────

#[test]
fn concurrent_writes_do_not_collide_on_filename() {
    // Ten threads each write 20 signals to the same agent. Expect
    // 200 distinct files — if ns+uuid weren't both in the name,
    // same-ns writes from two threads would clobber.
    let root = tmp_root("concurrent");
    let mut handles = Vec::new();
    for _ in 0..10 {
        let root = root.clone();
        handles.push(thread::spawn(move || {
            for i in 0..20 {
                let sig = mk_signal(&format!("m{i}"));
                inbox::write(&root, "bar", &sig).unwrap();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let drained = inbox::drain(&root, "bar");
    assert_eq!(drained.len(), 200);
}

// ─────────────────────────────────────────────────────────────────────
// Corrupt file resilience
// ─────────────────────────────────────────────────────────────────────

#[test]
fn corrupt_file_is_skipped_not_fatal() {
    let root = tmp_root("corrupt");
    // Write one valid signal first so the agent directory exists.
    let sig = mk_signal("valid");
    inbox::write(&root, "bar", &sig).unwrap();

    // Inject a corrupt .json file directly into the agent's dir.
    let agent_dir = root.join("bar");
    let corrupt_path = agent_dir.join("00000000000000000001-bogus.json");
    std::fs::write(&corrupt_path, b"{not json}").unwrap();

    let drained = inbox::drain(&root, "bar");
    // Valid signal still returned.
    assert_eq!(drained.len(), 1);
    assert_eq!(signal_text(&drained[0]), Some("valid".into()));
    // Corrupt file left in place for human triage.
    assert!(corrupt_path.exists());
}

// ─────────────────────────────────────────────────────────────────────
// Path-traversal defense
// ─────────────────────────────────────────────────────────────────────

#[test]
fn slash_in_agent_name_is_sanitized() {
    let root = tmp_root("traversal");
    let sig = mk_signal("sneak");
    // Caller passed a hostile agent name. The writer must not let
    // this escape the inbox root.
    let path =
        inbox::write(&root, "../../evil", &sig).expect("sanitize not fail");
    assert!(
        path.starts_with(&root),
        "sanitized path escaped root: path={path:?} root={root:?}"
    );
    // Something named `_invalid` or similar under the root; the
    // exact sanitized name is `.._.._evil` → starts_with `_` after
    // prefixing. Not asserting a specific string — just containment.
    assert!(!has_parent_dir_component(&path));
}

fn has_parent_dir_component(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "..")
}

#[test]
fn empty_agent_name_is_rejected_semantically() {
    let root = tmp_root("empty-name");
    let sig = mk_signal("oops");
    // Empty name sanitizes to "_invalid". Writer doesn't error —
    // the file lands, the caller gets a usable path — but it lands
    // under a clearly-bogus directory so it's easy to eyeball.
    let path = inbox::write(&root, "", &sig).unwrap();
    assert!(path.starts_with(&root));
    assert!(path.to_string_lossy().contains("_invalid"));
}

// ─────────────────────────────────────────────────────────────────────
// Signal-payload round-trip
// ─────────────────────────────────────────────────────────────────────

#[test]
fn write_drain_preserves_every_signal_field() {
    let root = tmp_root("fields");
    let mut sig = mk_signal("full");
    sig.priority = k2_core::awareness::Priority::High;
    sig.session = Some(k2_core::session::SessionId::new());
    inbox::write(&root, "bar", &sig).unwrap();

    let drained = inbox::drain(&root, "bar");
    assert_eq!(drained.len(), 1);
    let got = &drained[0];
    assert_eq!(got.id, sig.id);
    assert_eq!(got.from, sig.from);
    assert_eq!(got.to, sig.to);
    assert_eq!(got.priority, sig.priority);
    assert_eq!(got.session, sig.session);
    // `at` survives microseconds but may lose sub-microsecond
    // precision through chrono's RFC3339 serialization. Verify
    // within 1ms.
    let delta = (got.at - sig.at).num_microseconds().unwrap_or(0).abs();
    assert!(delta < 1000, "timestamp drifted by {delta}μs");
}
