use super::*;

#[test]
fn daemon_pty_config_default_is_80x24() {
    let cfg = DaemonPtyConfig::default();
    assert_eq!(cfg.cols, 80);
    assert_eq!(cfg.rows, 24);
    assert!(cfg.program.is_none());
    assert!(cfg.cwd.is_none());
    assert!(cfg.drain_on_exit);
}

/// Feed `chunks` to a real alacritty Term through the same parser
/// type alacritty's `EventLoop` drives (one `advance` per PTY read)
/// and return the first grid row's text (wide-char spacer cells
/// skipped, trailing blanks trimmed).
fn first_row_from_reads(chunks: &[&[u8]]) -> String {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::index::{Column, Line, Point};
    use alacritty_terminal::term::cell::Flags;
    use alacritty_terminal::vte::ansi::Processor;
    let size = TermSize { cols: 80, rows: 4 };
    let mut term = Term::new(TermConfig::default(), &size, VoidListener);
    let mut parser: Processor = Processor::new();
    for chunk in chunks {
        parser.advance(&mut term, chunk);
    }
    let grid = term.grid();
    let mut row = String::new();
    for c in 0..term.columns() {
        let cell = &grid[Point::new(Line(0), Column(c))];
        if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            row.push(cell.c);
        }
    }
    row.trim_end().to_string()
}

/// vte 0.15.0 dropped the ASCII byte when a PTY read ended right
/// after the lead byte of a 2-byte char and the next read went on
/// with one ASCII byte plus another non-ASCII char: "ïcø" rendered
/// as "ïø". Fixed by the vendored `third_party/vte` (patch
/// `third_party/upstream/vte-0001-*`). This fails if the workspace
/// ever resolves alacritty's vte to an unpatched copy.
#[test]
fn split_utf8_read_keeps_the_ascii_byte() {
    let text = "ïcø"; // C3 AF 63 C3 B8
    let bytes = text.as_bytes();
    assert_eq!(first_row_from_reads(&[&bytes[..1], &bytes[1..]]), text);
}

/// Every 2-way read split of mixed-width UTF-8 text renders the
/// same as one read (the class of bug above, at any position).
#[test]
fn utf8_text_renders_the_same_at_every_read_split() {
    for text in ["ïcø", "é-ä", "héllo wörld", "naïve café", "漢字かな", "a→b→c", "🙂x🙂"] {
        let bytes = text.as_bytes();
        assert_eq!(first_row_from_reads(&[bytes]), text, "single read of {text:?}");
        for at in 1..bytes.len() {
            assert_eq!(
                first_row_from_reads(&[&bytes[..at], &bytes[at..]]),
                text,
                "{text:?} split at byte {at}"
            );
        }
    }
}

/// Scan rows carry each row's OSC 8 target whole (even when the
/// visible text wraps), and include scrollback above the screen.
#[test]
fn scan_rows_keep_osc8_targets_and_scrollback() {
    let target = format!("https://example.test/oauth?state={}", "s".repeat(120));
    let mut bytes = format!("see: \x1b]8;;{target}\x07{target}\x1b]8;;\x07\r\n").into_bytes();
    for i in 0..20 {
        bytes.extend_from_slice(format!("line {i}\r\n").as_bytes());
    }
    let rows = scan_rows_from_bytes(40, 6, 100, &bytes);
    assert!(rows.len() > 6, "history rows must be included: {rows:?}");
    assert!(!rows[0].visible, "first row is scrollback: {rows:?}");
    assert!(rows.last().expect("rows").visible);
    assert!(rows[0].text.starts_with("see: https://example.test/oauth"), "{rows:?}");
    assert_eq!(rows[0].links, vec![target.clone()]);
    // Every wrapped row of the link holds the same, whole target.
    assert!(rows[0].wrapped && rows[1].links == vec![target.clone()], "{rows:?}");
    // No history asked: only the screen.
    let screen_only = scan_rows_from_bytes(40, 6, 0, &bytes);
    assert_eq!(screen_only.len(), 6);
    assert!(screen_only.iter().all(|r| r.visible));
}

/// A line longer than the terminal soft-wraps: every full row but
/// the last carries WRAPLINE, and the rows rejoin to the original.
#[test]
fn visible_rows_flag_soft_wrapped_rows() {
    let long: String = (0..25).map(|i| format!("{i:04}")).collect(); // 100 chars
    let rows = visible_rows_from_bytes(40, 6, format!("{long}\r\nnext").as_bytes());
    let flags: Vec<bool> = rows.iter().map(|r| r.wrapped).collect();
    assert_eq!(flags, vec![true, true, false, false, false, false]);
    assert_eq!(rows[0].text.chars().count(), 40);
    let joined = format!("{}{}{}", rows[0].text, rows[1].text, rows[2].text);
    assert_eq!(joined, long);
    assert_eq!(rows[3].text, "next");
}

/// Text that ends exactly on the last column followed by a newline
/// is NOT a soft wrap (alacritty's wrap is deferred until the next
/// printable char), and a wrapped row keeps a typed space that
/// landed on the boundary.
#[test]
fn visible_rows_exact_edge_then_newline_is_not_wrapped() {
    let exact = "x".repeat(40);
    let rows = visible_rows_from_bytes(40, 4, format!("{exact}\r\nafter").as_bytes());
    assert!(!rows[0].wrapped, "exact-edge row + newline must not be wrapped: {rows:?}");
    assert_eq!(rows[0].text, exact);
    assert_eq!(rows[1].text, "after");

    let spaced = format!("{} tail", "y".repeat(39));
    let rows = visible_rows_from_bytes(40, 4, spaced.as_bytes());
    assert!(rows[0].wrapped);
    assert_eq!(rows[0].text, format!("{} ", "y".repeat(39)));
    assert_eq!(rows[1].text, "tail");
}

#[test]
fn spawn_uses_passthrough_by_default() {
    // Sandbox P1 default-OFF proof: a default config selects the
    // host-direct Passthrough backend, so every existing spawn path is
    // byte-identical to pre-seam behavior.
    let cfg = DaemonPtyConfig::default();
    assert_eq!(cfg.sandbox, SandboxSpec::Passthrough);
    assert_eq!(cfg.sandbox.backend().name(), "passthrough");
}

#[test]
fn daemon_pty_config_default_label_state() {
    // Phase B: defaults are empty label + Pty source. PTY title
    // events fill the label.
    let cfg = DaemonPtyConfig::default();
    assert_eq!(cfg.label, "");
    assert_eq!(cfg.label_source, LabelSource::Pty);
}

// ── 0.45.0 smoke: unusable PATH entries never reach a child ─────

#[cfg(unix)]
#[test]
fn drop_unsafe_path_entries_cleans_a_caller_path() {
    let mut env = HashMap::new();
    env.insert(
        "PATH".to_string(),
        "/u/.k2/bin:compinit () {\n\ttypeset -g x\n}:/usr/bin:/bin".to_string(),
    );
    assert_eq!(drop_unsafe_path_entries(&mut env), 1);
    assert_eq!(env.get("PATH").map(String::as_str), Some("/u/.k2/bin:/usr/bin:/bin"));
}

#[test]
fn drop_unsafe_path_entries_without_path_is_a_no_op() {
    let mut env = HashMap::new();
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    assert_eq!(drop_unsafe_path_entries(&mut env), 0);
    assert_eq!(env.len(), 1);
}

/// The 0.45.0 wedge, end to end: a bare program name plus a PATH whose
/// first entry is ~4 KB of multi-line text. On macOS, execvp wrote
/// "execvP: <entry>: path too long" into the PTY before exec and the
/// fork blocked there, so `spawn` never returned. It must return.
#[cfg(unix)]
#[test]
fn spawn_with_an_overlong_multiline_path_entry_returns() {
    use std::path::PathBuf;
    let junk = format!("compdump () {{\n\t# undefined\n}}\n{}", "x".repeat(4096));
    let mut env = HashMap::new();
    env.insert("PATH".to_string(), format!("{junk}:/usr/bin:/bin"));
    let cfg = DaemonPtyConfig {
        survive: false,
        ptyd_meta: None,
        isolation: None,
        session_id: SessionId::new(),
        cols: 80,
        rows: 24,
        cwd: Some(PathBuf::from("/tmp")),
        sandbox: SandboxSpec::Passthrough,
        cell_uid: None,
        overlay: None,
        program: Some("cat".to_string()),
        args: vec![],
        durable_args: None,
        env,
        drain_on_exit: true,
        label: "path-junk".to_string(),
        label_source: LabelSource::Locked,
    };
    // Held here, not around the spawn: the 15 s below times the spawn alone.
    let _env = crate::test_env::lock();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = DaemonPtySession::spawn(cfg).map(|s| s.kill()).map_err(|e| e.to_string());
        let _ = tx.send(r);
    });
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => panic!("spawn failed: {e}"),
        Err(_) => panic!("spawn did not return in 15 s: the child is stuck before exec"),
    }
}

// ── 0.40.34 browser-open shim env injection ─────────────────────

/// A PATH value with the host separator (`:` Unix, `;` Windows).
fn j(parts: &[&str]) -> String {
    parts.join(if cfg!(windows) { ";" } else { ":" })
}

#[test]
fn browser_shim_env_prepends_bin_dir_and_sets_browser() {
    let shim = std::path::Path::new("/home/u/.k2/bin/k2-open");
    let mut env = HashMap::new();
    env.insert("PATH".to_string(), j(&["/usr/bin", "/bin"]));
    apply_browser_shim_env(&mut env, shim);
    // PREPEND = the shim dir is the FIRST segment, so execvp finds
    // our staged xdg-open before any system one.
    assert_eq!(env.get("PATH"), Some(&j(&["/home/u/.k2/bin", "/usr/bin", "/bin"])));
    assert_eq!(
        env.get("BROWSER").map(String::as_str),
        Some("/home/u/.k2/bin/k2-open")
    );
}

#[test]
fn browser_shim_env_respects_caller_browser_and_is_path_idempotent() {
    let shim = std::path::Path::new("/home/u/.k2/bin/k2-open");
    let mut env = HashMap::new();
    env.insert("PATH".to_string(), j(&["/home/u/.k2/bin", "/usr/bin"]));
    env.insert("BROWSER".to_string(), "firefox".to_string());
    apply_browser_shim_env(&mut env, shim);
    // Caller-supplied BROWSER wins (or_insert semantics)…
    assert_eq!(env.get("BROWSER").map(String::as_str), Some("firefox"));
    // …and an already-present bin dir is not duplicated.
    assert_eq!(env.get("PATH"), Some(&j(&["/home/u/.k2/bin", "/usr/bin"])));
}

#[test]
fn term_size_dimensions_include_scrollback() {
    let size = TermSize {
        cols: 120,
        rows: 40,
    };
    assert_eq!(size.columns(), 120);
    assert_eq!(size.screen_lines(), 40);
    assert_eq!(size.total_lines(), 40 + SCROLLBACK_CAP);
}

// Phase B label-state-machine tests. These exercise the
// `try_set_label_from_pty` / `set_label` / accessor surface
// without spawning a real PTY — we hand-construct a session
// and verify the state-machine logic in isolation. Requires a
// tokio runtime because `broadcast::channel` lives inside a
// tokio module, and we need a Term + EventLoop to satisfy the
// struct layout. We use the simplest valid spawn (a `cat`
// subprocess on Unix) under #[cfg(unix)] — the test exits
// when the Arc drops at scope end.

// 2026-07-02 PTY-leak incident (Bug 2) — the viewer-count
// invariant, on ONE spawned PTY (budget-frugal): internal
// `subscribe_events()` receivers (the child-exit observer /
// grid-emitter shape) never count as viewers; only
// `attach_viewer()` registrations do, the RAII guard decrements
// on drop, and the `ever_attached` latch flips with the first
// registration and stays latched.
#[cfg(unix)]
#[test]
fn viewer_count_ignores_internal_receivers_and_tracks_registrations() {
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    let cfg = DaemonPtyConfig {
        program: Some("cat".to_string()),
        cwd: Some(PathBuf::from("/tmp")),
        ..Default::default()
    };
    let s = crate::test_env::spawn_plain_pty(cfg).expect("spawn cat");

    // Internal observers: events receivers only — must be invisible
    // to the viewer signal (pre-fix these inflated it forever).
    let _internal_rx = s.subscribe_events();
    let _internal_rx2 = s.subscribe_events();
    assert_eq!(
        s.subscriber_count(),
        0,
        "internal subscribe_events() receivers must not count as viewers"
    );
    assert!(
        !s.ever_attached.load(Ordering::Relaxed),
        "internal receivers must not latch ever_attached either"
    );

    // Real viewers: attach_viewer registrations count and latch.
    let reg1 = s.attach_viewer();
    let reg2 = s.attach_viewer();
    assert_eq!(s.subscriber_count(), 2, "each registration counts once");
    assert!(
        s.ever_attached.load(Ordering::Relaxed),
        "first registration must latch ever_attached"
    );

    // RAII: every drop decrements; the latch survives.
    drop(reg1);
    assert_eq!(s.subscriber_count(), 1, "drop must decrement");
    drop(reg2);
    assert_eq!(s.subscriber_count(), 0, "count must return to true zero");
    assert!(
        s.ever_attached.load(Ordering::Relaxed),
        "ever_attached must stay latched after all viewers detach"
    );

    s.kill();
}

#[cfg(unix)]
#[test]
fn label_starts_with_seed_and_source() {
    use std::path::PathBuf;
    let cfg = DaemonPtyConfig {
        survive: false,
        ptyd_meta: None,
        isolation: None,
        session_id: SessionId::new(),
        cols: 80,
        rows: 24,
        cwd: Some(PathBuf::from("/tmp")),
        sandbox: SandboxSpec::Passthrough,
        cell_uid: None,
        overlay: None,
        program: Some("cat".to_string()),
        args: vec![],
        durable_args: None,
        env: Default::default(),
        drain_on_exit: true,
        label: "scout".to_string(),
        label_source: LabelSource::Locked,
    };
    let s = crate::test_env::spawn_plain_pty(cfg).expect("spawn cat");
    assert_eq!(s.label(), "scout");
    assert_eq!(s.label_source(), LabelSource::Locked);
}

#[cfg(unix)]
#[test]
fn try_set_label_from_pty_drops_locked() {
    use std::path::PathBuf;
    let cfg = DaemonPtyConfig {
        survive: false,
        ptyd_meta: None,
        isolation: None,
        session_id: SessionId::new(),
        cols: 80,
        rows: 24,
        cwd: Some(PathBuf::from("/tmp")),
        sandbox: SandboxSpec::Passthrough,
        cell_uid: None,
        overlay: None,
        program: Some("cat".to_string()),
        args: vec![],
        durable_args: None,
        env: Default::default(),
        drain_on_exit: true,
        label: "scout".to_string(),
        label_source: LabelSource::Locked,
    };
    let s = crate::test_env::spawn_plain_pty(cfg).expect("spawn cat");
    // PTY would emit "Claude Code" — must be silently dropped.
    let result = s.try_set_label_from_pty("Claude Code".to_string());
    assert!(result.is_none(), "Locked label must reject PTY update");
    assert_eq!(s.label(), "scout", "label must not have changed");
}

#[cfg(unix)]
#[test]
fn try_set_label_from_pty_accepts_pty_source() {
    use std::path::PathBuf;
    let cfg = DaemonPtyConfig {
        survive: false,
        ptyd_meta: None,
        isolation: None,
        session_id: SessionId::new(),
        cols: 80,
        rows: 24,
        cwd: Some(PathBuf::from("/tmp")),
        sandbox: SandboxSpec::Passthrough,
        cell_uid: None,
        overlay: None,
        program: Some("cat".to_string()),
        args: vec![],
        durable_args: None,
        env: Default::default(),
        drain_on_exit: true,
        label: String::new(),
        label_source: LabelSource::Pty,
    };
    let s = crate::test_env::spawn_plain_pty(cfg).expect("spawn cat");
    let r1 = s.try_set_label_from_pty("vim README.md".to_string());
    assert_eq!(r1.as_deref(), Some("vim README.md"));
    assert_eq!(s.label(), "vim README.md");
    // Same value again is a no-op (returns None).
    let r2 = s.try_set_label_from_pty("vim README.md".to_string());
    assert!(r2.is_none(), "no-op rewrite must return None");
}

#[cfg(unix)]
#[test]
fn set_label_with_lock_locks_source() {
    use std::path::PathBuf;
    let cfg = DaemonPtyConfig {
        session_id: SessionId::new(),
        cols: 80,
        rows: 24,
        cwd: Some(PathBuf::from("/tmp")),
        sandbox: SandboxSpec::Passthrough,
        cell_uid: None,
        overlay: None,
        program: Some("cat".to_string()),
        args: vec![],
        durable_args: None,
        env: Default::default(),
        drain_on_exit: true,
        ..Default::default()
    };
    let s = crate::test_env::spawn_plain_pty(cfg).expect("spawn cat");
    assert_eq!(s.label_source(), LabelSource::Pty);
    s.set_label("user-named".to_string(), true);
    assert_eq!(s.label(), "user-named");
    assert_eq!(s.label_source(), LabelSource::Locked);
    // Subsequent PTY update must be ignored.
    let r = s.try_set_label_from_pty("Claude Code".to_string());
    assert!(r.is_none());
    assert_eq!(s.label(), "user-named");
}

#[cfg(unix)]
#[test]
fn set_label_without_lock_keeps_source() {
    use std::path::PathBuf;
    let cfg = DaemonPtyConfig {
        survive: false,
        ptyd_meta: None,
        isolation: None,
        session_id: SessionId::new(),
        cols: 80,
        rows: 24,
        cwd: Some(PathBuf::from("/tmp")),
        sandbox: SandboxSpec::Passthrough,
        cell_uid: None,
        overlay: None,
        program: Some("cat".to_string()),
        args: vec![],
        durable_args: None,
        env: Default::default(),
        drain_on_exit: true,
        label: "seed".to_string(),
        label_source: LabelSource::Seed,
    };
    let s = crate::test_env::spawn_plain_pty(cfg).expect("spawn cat");
    s.set_label("new-seed".to_string(), false);
    assert_eq!(s.label(), "new-seed");
    // Source preserved — PTY still allowed to update.
    assert_eq!(s.label_source(), LabelSource::Seed);
    let r = s.try_set_label_from_pty("vim".to_string());
    assert_eq!(r.as_deref(), Some("vim"));
}

/// Regression test for the v2 process/memory leak: `kill()` MUST
/// forcefully terminate AND reap the child so no orphan + no zombie
/// remains. We spawn a long-lived `sleep 600` (a process that will
/// NOT exit on its own within the test), capture its PID, call
/// `kill()`, and then prove via `kill(pid, 0)` that the PID is gone
/// (ESRCH). The child was setsid()'d by alacritty so it's its own
/// group leader — exactly the case `kill()`'s killpg targets.
#[cfg(unix)]
#[test]
fn kill_terminates_and_reaps_child() {
    use std::path::PathBuf;

    let cfg = DaemonPtyConfig {
        survive: false,
        ptyd_meta: None,
        isolation: None,
        session_id: SessionId::new(),
        cols: 80,
        rows: 24,
        cwd: Some(PathBuf::from("/tmp")),
        sandbox: SandboxSpec::Passthrough,
        cell_uid: None,
        overlay: None,
        // A 600s sleep: will not self-exit during the test, so if
        // the PID is gone afterwards it's because kill() killed it.
        program: Some("sleep".to_string()),
        args: vec!["600".to_string()],
        durable_args: None,
        env: Default::default(),
        drain_on_exit: true,
        label: String::new(),
        label_source: LabelSource::Pty,
    };
    let s = crate::test_env::spawn_plain_pty(cfg).expect("spawn sleep");

    let pid = s
        .child_pid()
        .expect("child PID must be captured on unix");
    assert!(pid > 0, "captured PID must be positive, got {pid}");

    // Sanity: the child is alive right now (kill(pid, 0) → 0).
    let alive_before = unsafe { libc::kill(pid, 0) };
    assert_eq!(
        alive_before, 0,
        "freshly-spawned child pid={pid} must be alive before kill()"
    );

    // The fix under test.
    s.kill();

    // Poll for the PID to disappear. SIGKILL + our waitpid reap are
    // synchronous-ish, but allow a short window for the kernel to
    // finish teardown. We assert it IS gone — not "best effort".
    let mut gone = false;
    let mut last_errno = 0;
    for _ in 0..50 {
        let r = unsafe { libc::kill(pid, 0) };
        if r == -1 {
            last_errno = std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(0);
            if last_errno == libc::ESRCH {
                gone = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        gone,
        "child pid={pid} must be gone after kill() (kill(pid,0) → ESRCH); \
             last kill(pid,0) errno was {last_errno} (0 = still alive). \
             A surviving PID means the leak fix did not terminate the child."
    );

    // Reap guarantee: the child must NOT be a zombie. We already
    // waitpid'd inside kill(); a second WNOHANG must therefore
    // return -1/ECHILD (no such child to wait on) rather than the
    // PID again (which would mean a zombie is still reapable here).
    let mut status: i32 = 0;
    let wr = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    assert!(
        wr <= 0,
        "no zombie may remain: waitpid(pid={pid}) returned {wr} \
             (>0 means an unreaped zombie child is still present)"
    );
}

// Resize-arbitration polish (orca study takeaways #1/#3/#8) —
// the detach election is a pure function so these cases pin the
// policy without a PTY: who becomes active when the active viewer
// disconnects, and what dims the PTY restores to.

fn vp(cols: u16, rows: u16, last_seen: u64) -> SubscriberViewport {
    SubscriberViewport {
        cols,
        rows,
        last_seen,
    }
}

#[test]
fn elect_promotes_single_survivor() {
    let mut reg = HashMap::new();
    reg.insert(7, vp(100, 30, 1));
    // Active sub 9 departs; 7 is the only survivor with dims.
    let winner = elect_on_detach(&reg, 9, 9);
    assert_eq!(winner, Some((7, vp(100, 30, 1))));
}

#[test]
fn elect_prefers_most_recent_of_many() {
    let mut reg = HashMap::new();
    reg.insert(3, vp(200, 50, 4));
    reg.insert(7, vp(100, 30, 9));
    reg.insert(8, vp(40, 10, 2));
    // 7 acted most recently (stamp 9) — it wins regardless of id
    // order or dims.
    let winner = elect_on_detach(&reg, 5, 5);
    assert_eq!(winner, Some((7, vp(100, 30, 9))));
}

#[test]
fn elect_no_survivors_means_no_takeover() {
    let reg = HashMap::new();
    assert_eq!(elect_on_detach(&reg, 9, 9), None);
}

#[test]
fn elect_noop_when_departing_was_not_active() {
    let mut reg = HashMap::new();
    reg.insert(7, vp(100, 30, 1));
    // Sub 3 departs while 9 holds the claim — nothing to restore;
    // the active viewer is untouched.
    assert_eq!(elect_on_detach(&reg, 3, 9), None);
}

#[test]
fn elect_skips_departing_and_dimless_records() {
    let mut reg = HashMap::new();
    // Departing entry still present (caller hasn't removed it yet)
    // and a survivor that never declared dims — both ineligible.
    reg.insert(9, vp(40, 10, 8));
    reg.insert(5, vp(0, 0, 7));
    reg.insert(7, vp(100, 30, 2));
    let winner = elect_on_detach(&reg, 9, 9);
    assert_eq!(winner, Some((7, vp(100, 30, 2))));
}

#[cfg(unix)]
fn spawn_cat_session() -> Arc<DaemonPtySession> {
    let cfg = DaemonPtyConfig {
        session_id: SessionId::new(),
        cwd: Some(PathBuf::from("/tmp")),
        program: Some("cat".to_string()),
        ..Default::default()
    };
    crate::test_env::spawn_plain_pty(cfg).expect("spawn cat")
}

#[cfg(unix)]
fn term_dims(s: &DaemonPtySession) -> (u16, u16) {
    let tm = s.term();
    let t = tm.lock();
    (t.columns() as u16, t.screen_lines() as u16)
}

/// Poll the Term until it reaches `want` or the deadline passes.
/// Fails LOUDLY with the last observed dims — never a silent skip.
#[cfg(unix)]
fn assert_dims_settle(s: &DaemonPtySession, want: (u16, u16), what: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut got = term_dims(s);
    while got != want && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        got = term_dims(s);
    }
    assert_eq!(
        got, want,
        "{what}: PTY dims never settled to {want:?} (last saw {got:?})"
    );
}

#[cfg(unix)]
#[test]
fn first_request_resize_applies_instantly() {
    let s = spawn_cat_session();
    // Initial fit of a fresh session must never be debounced.
    DaemonPtySession::request_resize(&s, 100, 30);
    assert_eq!(
        term_dims(&s),
        (100, 30),
        "first resize must apply synchronously"
    );
}

/// Attach-critical path: even inside the debounce window,
/// `apply_resize_now` must land the target before returning (spawn
/// reuse / grid pre-snap cannot wait on the trailing flusher).
#[cfg(unix)]
#[test]
fn apply_resize_now_bypasses_debounce_window() {
    let s = spawn_cat_session();
    // Open the debounce window with a leading apply.
    DaemonPtySession::request_resize(&s, 100, 30);
    assert_eq!(term_dims(&s), (100, 30));
    // A request_resize inside the window is deferred...
    DaemonPtySession::request_resize(&s, 90, 25);
    assert_eq!(
        term_dims(&s),
        (100, 30),
        "in-window request_resize must stay deferred"
    );
    // ...but apply_resize_now must land immediately and cancel the
    // pending 90×25 so the flusher cannot reflow later.
    let live = DaemonPtySession::apply_resize_now(&s, 194, 62);
    assert_eq!(live, (194, 62), "apply_resize_now return value");
    assert_eq!(
        term_dims(&s),
        (194, 62),
        "apply_resize_now must apply synchronously inside debounce window"
    );
    // Outwait the old debounce window: pending was cleared, so the
    // flusher must not move the PTY back to 90×25.
    std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
    assert_eq!(
        term_dims(&s),
        (194, 62),
        "cleared pending must not flush after apply_resize_now"
    );
}

#[cfg(unix)]
#[test]
fn apply_resize_now_same_dims_is_noop() {
    let s = spawn_cat_session();
    DaemonPtySession::request_resize(&s, 100, 30);
    let before = s.resizes_applied();
    let live = DaemonPtySession::apply_resize_now(&s, 100, 30);
    assert_eq!(live, (100, 30));
    assert_eq!(
        s.resizes_applied(),
        before,
        "same-dims apply_resize_now must not count as a resize"
    );
}

#[cfg(unix)]
#[test]
fn resize_burst_coalesces_to_final_target() {
    let s = spawn_cat_session();
    // Leading edge: the first of the burst applies instantly...
    DaemonPtySession::request_resize(&s, 100, 30);
    // ...then two more land inside the debounce window. The
    // intermediate 90x25 must never be applied; only the final
    // 40x10 flushes.
    DaemonPtySession::request_resize(&s, 90, 25);
    DaemonPtySession::request_resize(&s, 40, 10);
    assert_eq!(
        term_dims(&s),
        (100, 30),
        "burst members inside the window must be deferred, not applied"
    );
    assert_dims_settle(&s, (40, 10), "trailing flush");
}

#[cfg(unix)]
#[test]
fn same_dims_request_is_skipped_entirely() {
    let s = spawn_cat_session();
    // Kessel postmortem: target == live dims ⇒ nothing may be
    // applied — no grid clear, no winsize set, no debounce window
    // opened. (TUIs don't repaint on an unchanged winsize, so a
    // no-op apply blanks the screen until the next output.)
    DaemonPtySession::request_resize(&s, 80, 24);
    assert_eq!(s.resizes_applied(), 0, "same-dims request must be skipped");
    assert_eq!(term_dims(&s), (80, 24));
    // Because the skip opened no window, the next REAL resize
    // still applies instantly.
    DaemonPtySession::request_resize(&s, 100, 30);
    assert_eq!(s.resizes_applied(), 1);
    assert_eq!(
        term_dims(&s),
        (100, 30),
        "real resize after a skipped no-op must be instant"
    );
}

#[cfg(unix)]
#[test]
fn detach_promotion_with_same_dims_skips_resize() {
    use std::sync::atomic::Ordering::Relaxed;

    let s = spawn_cat_session();
    // Survivor 7 and active viewer 9 declared the SAME viewport —
    // the common case of two same-sized desktop windows.
    s.note_viewport(7, 40, 10);
    s.note_viewport(9, 40, 10);
    s.active_subscriber.store(9, Relaxed);
    DaemonPtySession::request_resize(&s, 40, 10);
    assert_dims_settle(&s, (40, 10), "active claim");
    assert_eq!(s.resizes_applied(), 1);

    // 9 departs; 7 is promoted — but its dims already match the
    // live PTY size, so NO resize may be applied.
    DaemonPtySession::detach_subscriber(&s, 9);
    assert_eq!(s.active_subscriber.load(Relaxed), 7, "survivor promoted");
    // Outwait the debounce window: not even a deferred flush may
    // fire for the same-dims target.
    std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
    assert_eq!(
        s.resizes_applied(),
        1,
        "same-dims promotion must not apply a resize"
    );
    assert_eq!(term_dims(&s), (40, 10));
}

#[cfg(unix)]
#[test]
fn detach_of_active_viewer_restores_survivor_dims() {
    use std::sync::atomic::Ordering::Relaxed;

    let s = spawn_cat_session();
    // Sub 7 (a desktop at 100x30) declared first; sub 9 (a phone
    // at 40x10) claimed active and shrank the PTY.
    s.note_viewport(7, 100, 30);
    s.note_viewport(9, 40, 10);
    s.active_subscriber.store(9, Relaxed);
    s.active_cols.store(40, Relaxed);
    s.active_rows.store(10, Relaxed);
    DaemonPtySession::request_resize(&s, 40, 10);
    assert_dims_settle(&s, (40, 10), "phone claim");

    // The phone disconnects: sub 7 is promoted and the PTY
    // restores to ITS dims.
    DaemonPtySession::detach_subscriber(&s, 9);
    assert_eq!(s.active_subscriber.load(Relaxed), 7, "survivor promoted");
    assert_eq!(s.active_cols.load(Relaxed), 100);
    assert_eq!(s.active_rows.load(Relaxed), 30);
    assert_dims_settle(&s, (100, 30), "restore on detach");
}

#[cfg(unix)]
#[test]
fn detach_of_non_active_viewer_changes_nothing() {
    use std::sync::atomic::Ordering::Relaxed;

    let s = spawn_cat_session();
    s.note_viewport(7, 100, 30);
    s.note_viewport(9, 40, 10);
    s.active_subscriber.store(9, Relaxed);
    DaemonPtySession::request_resize(&s, 40, 10);
    assert_dims_settle(&s, (40, 10), "active claim");

    // A passive watcher leaving must not perturb the active claim
    // or the PTY size.
    DaemonPtySession::detach_subscriber(&s, 7);
    assert_eq!(s.active_subscriber.load(Relaxed), 9, "claim untouched");
    assert_eq!(term_dims(&s), (40, 10), "size untouched");
}

#[cfg(unix)]
#[test]
fn detach_of_last_viewer_clears_claim_and_keeps_dims() {
    use std::sync::atomic::Ordering::Relaxed;

    let s = spawn_cat_session();
    s.note_viewport(9, 40, 10);
    s.active_subscriber.store(9, Relaxed);
    DaemonPtySession::request_resize(&s, 40, 10);
    assert_dims_settle(&s, (40, 10), "sole viewer claim");

    // Last viewer out: no survivor to restore to — claim clears to
    // "none" and dims stay put (a reattaching viewer will claim +
    // resize anyway).
    DaemonPtySession::detach_subscriber(&s, 9);
    assert_eq!(s.active_subscriber.load(Relaxed), 0, "claim released");
    assert_eq!(term_dims(&s), (40, 10), "dims left as-is");
}

#[cfg(unix)]
#[test]
fn resize_generation_bumps_only_on_real_dims_change() {
    let s = spawn_cat_session();
    assert_eq!(s.resize_generation(), 0);
    // Same-dims: the debounced front door skips outright — no
    // clear, no SIGWINCH, no generation bump (the emitter must
    // never open a settle window for a no-op).
    DaemonPtySession::request_resize(&s, 80, 24);
    assert_eq!(s.resize_generation(), 0, "no-op resize must not bump");
    // Direct same-dims resize() is also gated on dims_changed.
    s.resize(80, 24);
    assert_eq!(s.resize_generation(), 0, "direct no-op must not bump");
    // Real change bumps exactly once.
    DaemonPtySession::request_resize(&s, 100, 30);
    assert_eq!(term_dims(&s), (100, 30));
    assert_eq!(s.resize_generation(), 1);
}

// ── S7a pin-to-size (prd-presence-multiplayer-v1 §5.5) ──────────
//
// The clamp lives at the top of `request_resize` (and, belt-and-
// suspenders, `resize`), so a pinned session's PTY cannot be moved
// by ANY resize path. These tests pin that contract at the session
// level; the daemon crate's grid-WS tests cover the SetActive /
// detach-promotion callers, and the integration test covers the
// route + wire.

#[cfg(unix)]
#[test]
fn set_pinned_applies_immediately_and_clamps_request_resize() {
    let s = spawn_cat_session();
    assert_eq!(s.pinned(), None, "fresh session must be unpinned");

    DaemonPtySession::set_pinned(&s, Some((100, 30)), Some("owner".into()));
    assert_eq!(s.pinned(), Some((100, 30)));
    assert_eq!(s.pinned_set_by().as_deref(), Some("owner"));
    assert_dims_settle(&s, (100, 30), "pin must apply its dims");
    let applied_at_pin = s.resizes_applied();

    // Any resize target now clamps to the pin — which equals the
    // live dims, so the same-dims skip applies NOTHING: no apply
    // count bump, no pending flush.
    DaemonPtySession::request_resize(&s, 140, 45);
    assert_eq!(
        s.resizes_applied(),
        applied_at_pin,
        "resize while pinned must be a clamped no-op"
    );
    assert_eq!(term_dims(&s), (100, 30));
    // Outwait the debounce window: no deferred flush may move a
    // pinned PTY either.
    std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
    assert_eq!(
        term_dims(&s),
        (100, 30),
        "pinned dims must hold across the debounce window"
    );
}

#[cfg(unix)]
#[test]
fn direct_resize_is_clamped_while_pinned() {
    let s = spawn_cat_session();
    DaemonPtySession::set_pinned(&s, Some((100, 30)), None);
    assert_dims_settle(&s, (100, 30), "pin");
    // A direct resize() caller (bypassing the front door) must
    // also be clamped — the pin is inescapable.
    s.resize(150, 50);
    assert_eq!(
        term_dims(&s),
        (100, 30),
        "direct resize() must clamp to the pin"
    );
}

#[cfg(unix)]
#[test]
fn unpin_restores_active_viewer_viewport() {
    use std::sync::atomic::Ordering::Relaxed;

    let s = spawn_cat_session();
    // Viewer 7 is the active claimer at 120x40 (the pre-pin
    // "juggling" state the unpin must clear back to).
    s.note_viewport(7, 120, 40);
    s.active_subscriber.store(7, Relaxed);
    DaemonPtySession::request_resize(&s, 120, 40);
    assert_dims_settle(&s, (120, 40), "active viewer fit");

    DaemonPtySession::set_pinned(&s, Some((100, 30)), Some("owner".into()));
    assert_dims_settle(&s, (100, 30), "pin");

    // Unpin: dims must return to the active viewer's declared
    // viewport without any client having to re-send a Resize.
    DaemonPtySession::set_pinned(&s, None, None);
    assert_eq!(s.pinned(), None);
    assert_eq!(s.pinned_set_by(), None, "attribution clears with the pin");
    assert_dims_settle(&s, (120, 40), "unpin restores active viewport");
}

#[cfg(unix)]
#[test]
fn unpin_falls_back_to_most_recent_viewport_without_active_claim() {
    let s = spawn_cat_session();
    // Two viewers declared dims but nobody holds the claim (the
    // elect_on_detach eligibility + recency policy applies): the
    // most recent (9) wins.
    s.note_viewport(7, 100, 30);
    s.note_viewport(9, 90, 25);
    DaemonPtySession::set_pinned(&s, Some((60, 20)), None);
    assert_dims_settle(&s, (60, 20), "pin");
    DaemonPtySession::set_pinned(&s, None, None);
    assert_dims_settle(&s, (90, 25), "unpin restores most-recent viewport");
}

#[cfg(unix)]
#[test]
fn unpin_with_no_viewer_leaves_dims_and_reopens_resizing() {
    let s = spawn_cat_session();
    DaemonPtySession::set_pinned(&s, Some((100, 30)), None);
    assert_dims_settle(&s, (100, 30), "pin");

    // No viewports registered: unpin leaves dims as-is (a
    // reattaching viewer claims + resizes anyway)...
    DaemonPtySession::set_pinned(&s, None, None);
    assert_eq!(s.pinned(), None);
    std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
    assert_eq!(term_dims(&s), (100, 30), "no viewer — dims left as-is");

    // ...and the next resize applies normally again.
    DaemonPtySession::request_resize(&s, 90, 25);
    assert_dims_settle(&s, (90, 25), "resize after unpin must apply");
}

#[cfg(unix)]
#[test]
fn pin_changes_broadcast_on_the_pin_channel() {
    let s = spawn_cat_session();
    let mut rx = s.subscribe_pins();

    DaemonPtySession::set_pinned(&s, Some((100, 30)), Some("teacher".into()));
    DaemonPtySession::set_pinned(&s, None, None);

    // Both events must be observable, in order — the grid-WS
    // subscriber loop turns these into pin_changed frames.
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut seen: Vec<Option<(u16, u16)>> = Vec::new();
    while seen.len() < 2 {
        match rx.try_recv() {
            Ok(ev) => seen.push(ev),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    panic!("pin broadcast incomplete within 2s; saw {seen:?}");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("pin broadcast died: {e}"),
        }
    }
    assert_eq!(seen, vec![Some((100, 30)), None]);
}

// ── Companion T0 — ephemeral "claim session" pin ────────────────
//
// An ephemeral pin is a normal pin PLUS an owner stamp (the
// setting grid-WS subscriber id): it clamps identically, but
// auto-clears when its setter detaches, after which the normal
// election restores the survivors. Manual unpin and normal pins
// supersede it (last write wins). The daemon crate's grid-WS tests
// cover the `claim_pin` socket action that feeds this.

/// Drain exactly `want` pin-broadcast events (bounded wait, loud
/// failure — mirrors `pin_changes_broadcast_on_the_pin_channel`).
#[cfg(unix)]
fn recv_pin_events(
    rx: &mut broadcast::Receiver<Option<(u16, u16)>>,
    want: usize,
) -> Vec<Option<(u16, u16)>> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut seen: Vec<Option<(u16, u16)>> = Vec::new();
    while seen.len() < want {
        match rx.try_recv() {
            Ok(ev) => seen.push(ev),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    panic!(
                        "pin broadcast incomplete within 2s; \
                             saw {seen:?} (wanted {want})"
                    );
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("pin broadcast died: {e}"),
        }
    }
    seen
}

/// Assert NO pin event arrives within a settle window — the
/// idempotence / quiet-path contract (no event spam).
#[cfg(unix)]
fn assert_no_pin_event(
    rx: &mut broadcast::Receiver<Option<(u16, u16)>>,
    what: &str,
) {
    std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
    match rx.try_recv() {
        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
        other => panic!("{what}: expected no pin event, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn ephemeral_pin_clamps_resizes_like_a_normal_pin() {
    let s = spawn_cat_session();
    DaemonPtySession::set_pinned_ephemeral(&s, 100, 30, Some("owner".into()), 9);
    assert_eq!(s.pinned(), Some((100, 30)));
    assert_eq!(s.pinned_set_by().as_deref(), Some("owner"));
    assert_eq!(s.ephemeral_pin_owner(), 9);
    assert_dims_settle(&s, (100, 30), "ephemeral pin must apply its dims");

    // Every other client's resize clamps exactly like a normal pin.
    DaemonPtySession::request_resize(&s, 140, 45);
    std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
    assert_eq!(
        term_dims(&s),
        (100, 30),
        "resize while ephemerally pinned must clamp"
    );
}

#[cfg(unix)]
#[test]
fn setter_detach_auto_clears_ephemeral_pin_and_restores_survivor() {
    use std::sync::atomic::Ordering::Relaxed;

    let s = spawn_cat_session();
    let mut rx = s.subscribe_pins();
    // Desktop 7 at 100x30; phone 9 claims active + ephemeral-pins
    // to its own dims.
    s.note_viewport(7, 100, 30);
    s.note_viewport(9, 40, 10);
    s.active_subscriber.store(9, Relaxed);
    DaemonPtySession::set_pinned_ephemeral(&s, 40, 10, Some("owner".into()), 9);
    assert_dims_settle(&s, (40, 10), "ephemeral pin");

    // The phone's socket drops: the pin auto-clears (broadcast!),
    // the election promotes the desktop, and the PTY restores.
    DaemonPtySession::detach_subscriber(&s, 9);
    assert_eq!(s.pinned(), None, "pin must auto-clear on setter detach");
    assert_eq!(s.pinned_set_by(), None, "attribution clears with the pin");
    assert_eq!(s.ephemeral_pin_owner(), 0, "ownership cleared");
    assert_eq!(s.active_subscriber.load(Relaxed), 7, "survivor promoted");
    assert_dims_settle(&s, (100, 30), "survivor dims restored");
    // Wire truth for the remaining subscribers: pinned, then cleared.
    assert_eq!(
        recv_pin_events(&mut rx, 2),
        vec![Some((40, 10)), None],
        "pin_changed must fire for the pin AND the auto-clear"
    );
}

#[cfg(unix)]
#[test]
fn other_subscriber_detach_leaves_ephemeral_pin_in_place() {
    use std::sync::atomic::Ordering::Relaxed;

    let s = spawn_cat_session();
    s.note_viewport(7, 100, 30);
    s.note_viewport(9, 40, 10);
    s.active_subscriber.store(9, Relaxed);
    DaemonPtySession::set_pinned_ephemeral(&s, 40, 10, None, 9);
    assert_dims_settle(&s, (40, 10), "ephemeral pin");

    // A NON-owning viewer leaving must not perturb the pin.
    DaemonPtySession::detach_subscriber(&s, 7);
    assert_eq!(s.pinned(), Some((40, 10)), "pin survives other detaches");
    assert_eq!(s.ephemeral_pin_owner(), 9, "ownership untouched");
    assert_eq!(term_dims(&s), (40, 10));
}

#[cfg(unix)]
#[test]
fn manual_unpin_clears_ephemeral_pin_and_later_detach_stays_quiet() {
    let s = spawn_cat_session();
    DaemonPtySession::set_pinned_ephemeral(&s, 40, 10, None, 9);
    assert_dims_settle(&s, (40, 10), "ephemeral pin");

    // "The user on the other end unpins" — today's route calls
    // exactly this. Ownership clears with the pin...
    DaemonPtySession::set_pinned(&s, None, None);
    assert_eq!(s.pinned(), None);
    assert_eq!(s.ephemeral_pin_owner(), 0, "manual unpin drops ownership");

    // ...so the setter's eventual detach is pin-quiet: no spurious
    // second `pin_changed` for a pin that's already gone.
    let mut rx = s.subscribe_pins();
    DaemonPtySession::detach_subscriber(&s, 9);
    assert_no_pin_event(&mut rx, "detach after manual unpin");
}

#[cfg(unix)]
#[test]
fn normal_pin_replaces_ephemeral_and_survives_setter_detach() {
    let s = spawn_cat_session();
    DaemonPtySession::set_pinned_ephemeral(&s, 40, 10, None, 9);
    assert_dims_settle(&s, (40, 10), "ephemeral pin");

    // A normal pin (pin-size route) lands afterwards: last write
    // wins, the pin is no longer ephemeral...
    DaemonPtySession::set_pinned(&s, Some((100, 30)), Some("owner".into()));
    assert_eq!(s.ephemeral_pin_owner(), 0, "normal pin demotes ownership");
    assert_dims_settle(&s, (100, 30), "normal pin applies");

    // ...so the former setter's detach must NOT clear it
    // (regression: normal pins keep today's semantics exactly).
    DaemonPtySession::detach_subscriber(&s, 9);
    std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
    assert_eq!(
        s.pinned(),
        Some((100, 30)),
        "normal pin must survive the ex-owner's detach"
    );
    assert_eq!(term_dims(&s), (100, 30));
}

#[cfg(unix)]
#[test]
fn reclaim_updates_dims_in_place_and_same_dims_is_silent() {
    let s = spawn_cat_session();
    let mut rx = s.subscribe_pins();
    // Watcher 7 at 100x30 keeps trying to resize throughout.
    s.note_viewport(7, 100, 30);

    DaemonPtySession::set_pinned_ephemeral(&s, 40, 10, None, 9);
    assert_dims_settle(&s, (40, 10), "initial claim");

    // Keyboard opens: the SAME subscriber re-claims at new dims —
    // update in place (no unpin/repin churn), exactly ONE event.
    DaemonPtySession::set_pinned_ephemeral(&s, 40, 18, None, 9);
    assert_eq!(s.pinned(), Some((40, 18)), "re-claim updates the pin");
    assert_eq!(s.ephemeral_pin_owner(), 9, "ownership unchanged");
    assert_dims_settle(&s, (40, 18), "PTY follows the re-claim");
    assert_eq!(
        recv_pin_events(&mut rx, 2),
        vec![Some((40, 10)), Some((40, 18))],
        "one pin_changed per real dims change"
    );

    // Other clients stay clamped at the NEW dims.
    DaemonPtySession::request_resize(&s, 100, 30);
    std::thread::sleep(Duration::from_millis(RESIZE_DEBOUNCE_MS * 2));
    assert_eq!(term_dims(&s), (40, 18), "others clamp to the re-claim");

    // Keyboard bounce with unchanged dims: cheap + idempotent — no
    // broadcast, no resize window, ownership intact.
    let applied_before = s.resizes_applied();
    DaemonPtySession::set_pinned_ephemeral(&s, 40, 18, None, 9);
    assert_no_pin_event(&mut rx, "same-dims re-claim");
    assert_eq!(s.pinned(), Some((40, 18)));
    assert_eq!(s.ephemeral_pin_owner(), 9);
    assert_eq!(
        s.resizes_applied(),
        applied_before,
        "same-dims re-claim must not apply a resize"
    );
}

/// A DSR-6 (`\x1b[6n`) parsed by the Term must produce a CPR answer
/// that lands in the query-reply sink — the seam `spawn()` wires to
/// the PTY's `Notifier` — and must NOT leak onto the viewer
/// broadcast (a query answer is transport, not session output).
#[test]
fn dsr_query_answer_lands_in_reply_sink_not_broadcast() {
    use alacritty_terminal::vte::ansi::Processor;

    let (tx, mut events_rx) = broadcast::channel::<AlacEvent>(16);
    let (reply_tx, reply_rx) = std::sync::mpsc::channel::<String>();
    let reply_slot = Arc::new(std::sync::OnceLock::new());
    assert!(reply_slot
        .set(Box::new(move |text: String| {
            reply_tx.send(text).expect("capture query reply");
        }) as QueryReplySink)
        .is_ok());
    let listener = DaemonEventListener {
        tx,
        reply_tx: reply_slot,
        muted: Default::default(),
    };

    let size = TermSize { cols: 80, rows: 24 };
    let mut term = Term::new(TermConfig::default(), &size, listener);
    let mut parser: Processor = Processor::new();
    // Print "ab" first so the answer proves the LIVE cursor
    // position (row 1, col 3), not a hardcoded origin.
    parser.advance(&mut term, b"ab\x1b[6n");

    let reply = reply_rx
        .try_recv()
        .expect("DSR-6 must synchronously produce a CPR answer");
    assert_eq!(reply, "\x1b[1;3R", "CPR must report the live cursor");
    match events_rx.try_recv() {
        Err(broadcast::error::TryRecvError::Empty) => {}
        other => panic!(
            "PtyWrite must not reach the viewer broadcast, got {other:?}"
        ),
    }
}

/// k2-ptyd P75 (V1): while muted, replaying old bytes that contain
/// terminal queries produces ZERO input for the child and broadcasts
/// no window-system query; the first live query after unmute is
/// answered normally.
#[test]
fn muted_listener_drops_replayed_query_answers_and_unmutes_cleanly() {
    use alacritty_terminal::vte::ansi::Processor;

    let (tx, mut events_rx) = broadcast::channel::<AlacEvent>(64);
    let (reply_tx, reply_rx) = std::sync::mpsc::channel::<String>();
    let reply_slot = Arc::new(std::sync::OnceLock::new());
    assert!(reply_slot
        .set(Box::new(move |text: String| {
            reply_tx.send(text).expect("capture query reply");
        }) as QueryReplySink)
        .is_ok());
    let listener = DaemonEventListener { tx, reply_tx: reply_slot, muted: Default::default() };
    let size = TermSize { cols: 80, rows: 24 };
    let mut term = Term::new(TermConfig::default(), &size, listener.clone());
    let mut parser: Processor = Processor::new();

    listener.set_replies_muted(true);
    parser.advance(
        &mut term,
        b"old\x1b[6n\x1b[c\x1b[>c\x1b[5n\x1b[?2004$p\x1b[?u\x1b[18t\x1b[14t\x1b]10;?\x07\x1b]11;?\x1b\\",
    );
    assert!(reply_rx.try_recv().is_err(), "no query answer may reach the child during replay");
    while let Ok(ev) = events_rx.try_recv() {
        assert!(
            !matches!(ev, AlacEvent::ColorRequest(..) | AlacEvent::TextAreaSizeRequest(..) | AlacEvent::PtyWrite(_)),
            "window-system queries must not be broadcast during replay: {ev:?}"
        );
    }

    listener.set_replies_muted(false);
    parser.advance(&mut term, b"\x1b[6n");
    assert_eq!(reply_rx.try_recv().expect("live query answered"), "\x1b[1;4R");
    assert!(reply_rx.try_recv().is_err(), "exactly one answer");
}

/// k2-ptyd default-off proof (P67/P76): a `survive` session with the
/// switch off is an ordinary in-process session — alacritty's
/// EventLoop, the agent is OUR child — and the env its child sees is
/// byte-identical to a non-survive spawn's.
#[cfg(unix)]
#[test]
fn survive_with_switch_off_is_the_unchanged_in_process_path() {
    assert!(
        std::env::var_os("K2_PTYD").is_none(),
        "unset K2_PTYD before running k2-core tests (CLAUDE.md test rule)"
    );
    assert!(!DaemonPtyConfig::default().survive, "survive defaults to false");
    // Both children get the SAME explicit env: one snapshot of this
    // process's env, passed in full as `cfg.env` (it overrides what the
    // child inherits, and an explicit PATH skips the login-shell
    // enrichment), taken under the crate-wide env lock. A parallel
    // test's HOME/env guard can't change what either child sees (a
    // mini and a Linux node saw HOME swapped between the two spawns).
    //
    // 0.46.5 flipped the default to on (P67), so "switch off" is now the
    // explicit `terminal.ptyd = "off"` setting, written under a temp
    // HOME (the guard holds the crate-wide env lock).
    let _home = crate::test_env::TempHome::new();
    crate::app_settings::update(serde_json::json!({ "terminal": { "ptyd": "off" } })).expect("terminal.ptyd off");
    assert_eq!(crate::terminal::ptyd::Switch::resolve(), crate::terminal::ptyd::Switch::Off, "the setting turns the switch off");
    let explicit: HashMap<String, String> = std::env::vars().collect();
    let dir = std::env::temp_dir().join(format!("k2-survive-off-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut dumps = Vec::new();
    for survive in [false, true] {
        let out = dir.join(format!("env-{survive}"));
        let cfg = DaemonPtyConfig {
            program: Some("/bin/sh".into()),
            args: vec!["-c".into(), format!("env | sort > '{}'; sleep 30", out.display())],
            cwd: Some(dir.clone()),
            env: explicit.clone(),
            survive,
            ..DaemonPtyConfig::default()
        };
        let s = crate::test_env::spawn_plain_pty(cfg).expect("spawn");
        assert_eq!(s.backend_name(), "in-process");
        let pid = s.child_pid().expect("pid captured");
        // SAFETY: getpid has no preconditions.
        let me = unsafe { libc::getpid() };
        assert_eq!(parent_pid(pid), me, "in-process: the agent is the daemon's own child");
        let t = std::time::Instant::now();
        while !(out.exists() && std::fs::metadata(&out).unwrap().len() > 0) {
            assert!(t.elapsed() < Duration::from_secs(10), "child never wrote its env");
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(100));
        dumps.push(std::fs::read_to_string(&out).unwrap());
        s.kill();
    }
    assert_eq!(dumps[0], dumps[1], "survive=true with the switch off changes nothing the child sees");
    // The selection itself (status_line() reads a process-wide last
    // reason that another test's survive spawn may have set first, and a
    // switch-off spawn never logs one, P27).
    assert_eq!(
        crate::terminal::ptyd::select_backend(true, crate::terminal::ptyd::Switch::resolve(), true, || None, || Ok(())),
        crate::terminal::ptyd::Backend::InProcess(crate::terminal::ptyd::Reason::SwitchOff),
        "switch off: survive sessions stay in-process"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// macOS: libproc, not `ps` (setuid, so it can't run under a sandbox).
#[cfg(target_os = "macos")]
fn parent_pid(pid: i32) -> i32 {
    crate::mac_proc::bsd_info(pid).expect("bsd info").ppid
}

#[cfg(all(unix, not(target_os = "macos")))]
fn parent_pid(pid: i32) -> i32 {
    let out = std::process::Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().expect("ppid")
}

/// Slice 5 — the grok DCS gotcha, pinned. Grok echoes unconsumed
/// DCS replies (`ESC P … ST`, e.g. XTVERSION `ESC P>|… ST`) into
/// its composer as literal text, so K2's query-reply layer must
/// NEVER emit a DCS-shaped answer. We drive the parser with the
/// query families a TUI might send — XTVERSION (`CSI > 0 q`),
/// DECRQSS (`DCS $ q m ST`), DA1/DA2, DECRQM, DSR-6 — and assert
/// every produced reply is CSI-shaped (`ESC [`), none DCS-shaped
/// (`ESC P`). If a vte/alacritty bump ever adds a DECRQSS/XTVERSION
/// responder this fails loudly, and DCS must be gated away from
/// grok panes before shipping.
#[test]
fn query_replies_are_never_dcs_shaped() {
    use alacritty_terminal::vte::ansi::Processor;

    let (tx, _events_rx) = broadcast::channel::<AlacEvent>(16);
    let (reply_tx, reply_rx) = std::sync::mpsc::channel::<String>();
    let reply_slot = Arc::new(std::sync::OnceLock::new());
    assert!(reply_slot
        .set(Box::new(move |text: String| {
            reply_tx.send(text).expect("capture query reply");
        }) as QueryReplySink)
        .is_ok());
    let listener = DaemonEventListener {
        tx,
        reply_tx: reply_slot,
        muted: Default::default(),
    };

    let size = TermSize { cols: 80, rows: 24 };
    let mut term = Term::new(TermConfig::default(), &size, listener);
    let mut parser: Processor = Processor::new();
    // XTVERSION, DECRQSS(SGR), DA1, DA2, DECRQM(?2004), DSR-6.
    parser.advance(
        &mut term,
        b"\x1b[>0q\x1bP$qm\x1b\\\x1b[c\x1b[>c\x1b[?2004$p\x1b[6n",
    );

    let mut replies = Vec::new();
    while let Ok(r) = reply_rx.try_recv() {
        replies.push(r);
    }
    assert!(
        !replies.is_empty(),
        "the query batch must produce at least the DA/DSR answers"
    );
    for r in &replies {
        assert!(
            !r.starts_with("\x1bP"),
            "query reply must never be DCS-shaped (grok echoes DCS as composer text): {r:?}"
        );
        assert!(
            r.starts_with("\x1b["),
            "every reply alacritty produces today is CSI-shaped: {r:?}"
        );
    }
}

/// `^[[<digits>;<digits>R` — the tty-ECHO rendering of a CPR answer
/// (ECHOCTL renders the answer's ESC as caret notation on the grid).
#[cfg(unix)]
fn contains_cpr_echo(row: &str) -> bool {
    let bytes = row.as_bytes();
    for start in 0..bytes.len() {
        let rest = &bytes[start..];
        if !rest.starts_with(b"^[[") {
            continue;
        }
        let mut i = 3;
        let row_digits = i;
        while i < rest.len() && rest[i].is_ascii_digit() {
            i += 1;
        }
        if i == row_digits || rest.get(i) != Some(&b';') {
            continue;
        }
        i += 1;
        let col_digits = i;
        while i < rest.len() && rest[i].is_ascii_digit() {
            i += 1;
        }
        if i > col_digits && rest.get(i) == Some(&b'R') {
            return true;
        }
    }
    false
}

/// End-to-end through the REAL notifier + PTY: a DSR-6 the child
/// emits gets answered, and the answer arrives on the child's input.
/// We type `\x1b[6n\n` at `cat`: the tty ECHO mangles the typed ESC
/// to `^[` (no dispatch), but cat's OUTPUT carries a real ESC — the
/// parser dispatches the query, the emulator's CPR answer routes
/// back into PTY input, and the tty ECHO of that ANSWER renders as
/// `^[[<row>;<col>R` on the grid. Before the fix nothing comes back
/// and this times out.
#[cfg(unix)]
#[test]
fn dsr_query_from_child_is_answered_through_the_pty() {
    let s = spawn_cat_session();
    s.write(b"\x1b[6n\n".to_vec());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let rows = s.visible_text_rows();
        if rows.iter().any(|r| contains_cpr_echo(r)) {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "no CPR answer echoed within 5s — the query reply \
                     never reached the child. grid: {rows:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Count this process's open PTY master fds via lsof. On macOS a
/// master opened through posix_openpt shows up as `/dev/ptmx`.
/// Loud on failure — a missing/broken lsof must fail the test,
/// never report a fake 0.
#[cfg(unix)]
fn count_ptmx_fds() -> usize {
    let pid = std::process::id();
    let out = std::process::Command::new("lsof")
        .args(["-p", &pid.to_string()])
        .output()
        .expect("lsof must be runnable");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains("/dev/ptmx"))
        .count()
}

/// PTY-master leak regression (path 1: child exits ON ITS OWN).
/// The session Arc is still held — as the daemon's session map /
/// WS handlers hold theirs — and the master fd must be released
/// anyway: the fd's lifetime is the IO thread's, not the Arc's.
/// Here the IO thread's own `try_wait` wins the reap, sees
/// `ChildEvent::Exited`, breaks its loop, and drops the Pty.
#[cfg(unix)]
#[test]
fn child_self_exit_releases_pty_master_fd() {
    let baseline = count_ptmx_fds();
    let cfg = DaemonPtyConfig {
        cwd: Some(PathBuf::from("/tmp")),
        program: Some("/bin/sh".to_string()),
        args: vec!["-c".to_string(), "exit 0".to_string()],
        ..Default::default()
    };
    let s = crate::test_env::spawn_plain_pty(cfg).expect("spawn sh");
    let pid = s.child_pid().expect("pid captured");

    // Wait for the child to fully disappear (exit + reap).
    let mut gone = false;
    for _ in 0..250 {
        if unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error()
                == Some(libc::ESRCH)
        {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(gone, "child pid={pid} must exit+reap on its own (sh -c 'exit 0')");

    // The Arc is STILL held here. The master must close anyway.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut now_count = count_ptmx_fds();
    while now_count > baseline && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        now_count = count_ptmx_fds();
    }
    assert!(
        now_count <= baseline,
        "PTY master leaked after child self-exit: baseline={baseline} now={now_count} \
             (Arc still held — the fd's lifetime must not be anchored to the session object)"
    );
    drop(s);
}

/// PTY-master leak regression (path 2: teardown via `kill()` when
/// the IO thread LOST the reap race). Before the fix this leaked
/// the master fd until daemon exit: an external `waitpid` reaps the
/// child first, so alacritty's `try_wait` gets ECHILD and
/// `next_child_event` swallows it (no loop break), and with Arcs
/// still retained (session map / WS handler / emitter) nothing else
/// ever woke the parked thread. `kill()` must now release the fd
/// explicitly via `Msg::Shutdown` — with the Arcs STILL held.
#[cfg(unix)]
#[test]
fn kill_releases_pty_master_fd_with_arc_retained() {
    let baseline = count_ptmx_fds();
    let cfg = DaemonPtyConfig {
        cwd: Some(PathBuf::from("/tmp")),
        program: Some("sleep".to_string()),
        args: vec!["600".to_string()],
        ..Default::default()
    };
    let s = crate::test_env::spawn_plain_pty(cfg).expect("spawn sleep");
    let retained = Arc::clone(&s); // simulated WS-handler/emitter holder
    let pid = s.child_pid().expect("pid captured");

    // Deterministically lose the race for the IO thread: reap the
    // child HERE (the test process is the parent) before kill()
    // runs. The IO thread's try_wait can now only ever see ECHILD.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
        let mut status: i32 = 0;
        let r = libc::waitpid(pid, &mut status, 0);
        // A blocking waitpid parked in the kernel beats the IO
        // thread's kqueue→pipe→try_wait chain in practice; but if
        // the IO thread somehow reaped first (ECHILD here), the
        // post-condition below still pins the contract — the fd
        // must be released with Arcs retained either way.
        assert!(
            r == pid
                || (r == -1
                    && std::io::Error::last_os_error().raw_os_error()
                        == Some(libc::ECHILD)),
            "waitpid(pid={pid}) returned {r} (expected the pid, or \
                 -1/ECHILD if alacritty's IO thread won the reap race)"
        );
    }

    // The teardown chokepoint every path funnels through
    // (unregister / close route / reaper / Drop).
    s.kill();

    // Both Arcs are STILL held. The master must close anyway.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut now_count = count_ptmx_fds();
    while now_count > baseline && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        now_count = count_ptmx_fds();
    }
    assert!(
        now_count <= baseline,
        "PTY master leaked after kill() with retained Arcs: \
             baseline={baseline} now={now_count} — kill() must shut the \
             IO thread down explicitly, not wait for the last Arc drop"
    );
    drop(retained);
    drop(s);
}

/// `kill()` is idempotent: calling it twice must not panic, must
/// not waitpid a (possibly-recycled) PID a second time, and the
/// second call is a cheap no-op. Drop then runs kill() a third
/// time implicitly — also a no-op.
#[cfg(unix)]
#[test]
fn kill_is_idempotent() {
    use std::path::PathBuf;

    let cfg = DaemonPtyConfig {
        survive: false,
        ptyd_meta: None,
        isolation: None,
        session_id: SessionId::new(),
        cols: 80,
        rows: 24,
        cwd: Some(PathBuf::from("/tmp")),
        sandbox: SandboxSpec::Passthrough,
        cell_uid: None,
        overlay: None,
        program: Some("sleep".to_string()),
        args: vec!["600".to_string()],
        durable_args: None,
        env: Default::default(),
        drain_on_exit: true,
        label: String::new(),
        label_source: LabelSource::Pty,
    };
    let s = crate::test_env::spawn_plain_pty(cfg).expect("spawn sleep");
    let pid = s.child_pid().expect("pid captured");

    s.kill();
    // Second call must be a no-op (returns early on the `killed`
    // gate) — and must not blow up.
    s.kill();

    // Confirm the child is gone exactly once (no double-reap havoc).
    let mut gone = false;
    for _ in 0..50 {
        if unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error()
                == Some(libc::ESRCH)
        {
            gone = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(gone, "child pid={pid} gone after idempotent kills");
    // Drop fires kill() a third time — no panic, no double free.
    drop(s);
}

/// A Codex stand-in whose `--help` lists `--no-daemon` (Codex 0.155+) and
/// whose launch writes the marker it saw and its argv to `seen.codex`
/// beside it. Every agent CLI name gets the same body; only `codex` runs.
#[cfg(unix)]
const MARKER_SHIM: &str = r#"if [ "$1" = "--help" ]; then
  printf '      --no-daemon\n          Run without the shared background server\n'
  exit 0
fi
out="$(dirname "$0")/seen.$(basename "$0")"
printf '%s\n%s\n' "${K2_CODEX_TAB_ISOLATED-<unset>}" "$*" > "$out.tmp" && mv "$out.tmp" "$out"
exec sleep 30"#;

/// Launch the stand-in `codex --yolo` through the v2 spawn with `env` and
/// return (the marker it saw, its argv).
#[cfg(unix)]
fn codex_tab_marker_seen(env: HashMap<String, String>) -> (String, String) {
    let shim = crate::test_env::AgentShim::with_body(MARKER_SHIM);
    let cfg = DaemonPtyConfig {
        session_id: SessionId::new(),
        cwd: Some(PathBuf::from("/tmp")),
        program: Some("codex".to_string()),
        args: vec!["--yolo".to_string()],
        env,
        ..Default::default()
    };
    let s = crate::test_env::spawn_plain_pty(cfg).expect("spawn the codex stand-in");
    let seen = shim.dir().join("seen.codex");
    let started = std::time::Instant::now();
    while !seen.exists() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "the codex stand-in never wrote {}",
            seen.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    s.kill();
    let text = std::fs::read_to_string(&seen).expect("read seen.codex");
    let mut lines = text.lines();
    let marker = lines.next().expect("marker line").to_string();
    let argv = lines.next().expect("argv line").to_string();
    (marker, argv)
}

/// The passport env a v2 spawn hands a tab, plus a marker the caller's env
/// claims for another tab (the spawner must overwrite it).
#[cfg(unix)]
fn tab_env_with_claimed_marker() -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("K2_HOOK_TOKEN".to_string(), "tab-a.secretA1".to_string());
    env.insert(crate::terminal::codex_no_daemon::ISOLATED_ENV.to_string(), "tab-b".to_string());
    env
}

#[cfg(unix)]
#[test]
fn codex_tab_is_marked_with_its_own_passport_session() {
    let _policy = crate::test_env::EnvVar::remove(crate::terminal::codex_no_daemon::OVERRIDE_ENV);
    // The daemon's own env carries a marker too: never passed on.
    let _own = crate::test_env::EnvVar::set(crate::terminal::codex_no_daemon::ISOLATED_ENV, "daemon-own");
    let (marker, argv) = codex_tab_marker_seen(tab_env_with_claimed_marker());
    assert_eq!(argv, "--no-daemon --yolo");
    assert_eq!(marker, "tab-a", "the marker is the tab's passport session, set by the spawner");
}

#[cfg(unix)]
#[test]
fn codex_tab_without_no_daemon_gets_a_blank_marker() {
    let _policy = crate::test_env::EnvVar::set(crate::terminal::codex_no_daemon::OVERRIDE_ENV, "0");
    let (marker, argv) = codex_tab_marker_seen(tab_env_with_claimed_marker());
    assert_eq!(argv, "--yolo");
    assert_eq!(marker, "", "K2_CODEX_NO_DAEMON=0: the claimed marker is blanked");
}
