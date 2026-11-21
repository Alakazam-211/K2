//! The rules of the Codex per-tab passport check, over a fake record of
//! the live tabs, plus the Codex version pin.

use std::collections::HashMap;
use std::sync::Mutex;

use super::*;

const A: &str = "sid-a";
const B: &str = "sid-b";
const C: &str = "sid-c";
const CHECKED: &str = "codex-cli 0.163.0";

#[derive(Default)]
struct Fake {
    tabs: Mutex<HashMap<String, TabFacts>>,
    versions: HashMap<String, Result<String, String>>,
    /// (pid, ancestor) pairs that descend.
    tree: Vec<(u32, i32)>,
    /// What a re-check of a session would find: the id of its next log.
    next_log: HashMap<String, String>,
    rechecks: Mutex<Vec<(String, String)>>,
    confirmed: Mutex<Vec<(String, String)>>,
    version_asks: Mutex<usize>,
    /// (session, conversation) pairs whose log K2 parsed.
    logs: Vec<(String, String)>,
    warned: WarnOnce,
    warn_lines: Mutex<Vec<String>>,
}

impl Fake {
    fn with(mut self, sid: &str, t: TabFacts) -> Self {
        if t.codex {
            self.versions.insert(sid.to_string(), Ok(CHECKED.to_string()));
        }
        self.tabs.get_mut().expect("tabs").insert(sid.to_string(), t);
        self
    }
    fn rechecks(&self) -> Vec<(String, String)> {
        self.rechecks.lock().expect("rechecks").clone()
    }
    fn confirmed(&self) -> Vec<(String, String)> {
        self.confirmed.lock().expect("confirmed").clone()
    }
}

impl Records for Fake {
    fn tab(&self, session_id: &str) -> Option<TabFacts> {
        self.tabs.lock().expect("tabs").get(session_id).cloned()
    }
    fn others(&self, session_id: &str) -> Vec<TabFacts> {
        self.tabs.lock().expect("tabs").iter().filter(|(k, _)| *k != session_id).map(|(_, v)| v.clone()).collect()
    }
    fn version(&self, session_id: &str) -> Result<String, String> {
        *self.version_asks.lock().expect("asks") += 1;
        self.versions.get(session_id).cloned().unwrap_or_else(|| Err("not recorded".to_string()))
    }
    fn descends(&self, pid: u32, ancestor: i32) -> bool {
        self.tree.contains(&(pid, ancestor))
    }
    fn recheck(&self, session_id: &str, wanted: &str) -> bool {
        self.rechecks.lock().expect("rechecks").push((session_id.to_string(), wanted.to_string()));
        if self.next_log.get(session_id).map(String::as_str) != Some(wanted) {
            return false;
        }
        let mut tabs = self.tabs.lock().expect("tabs");
        let t = tabs.get_mut(session_id).expect("rechecked tab exists");
        t.conversation = Some(wanted.to_string());
        t.settled = false;
        true
    }
    fn confirm(&self, session_id: &str, conversation: &str) {
        self.confirmed.lock().expect("confirmed").push((session_id.to_string(), conversation.to_string()));
    }
    fn log_confirms(&self, session_id: &str, conversation: &str) -> bool {
        self.logs.contains(&(session_id.to_string(), conversation.to_string()))
    }
    fn warn_newer_once(&self, version: &str) {
        if self.warned.first(version) {
            self.warn_lines.lock().expect("warn lines").push(newer_line(version));
        }
    }
}

fn codex(conv: &str, settled: bool, cwd: &str, child: i32) -> TabFacts {
    TabFacts {
        codex: true,
        cwd: Some(cwd.to_string()),
        conversation: Some(conv.to_string()),
        settled,
        child_pid: Some(child),
    }
}

fn hdr(conv: Option<&str>, thread: Option<&str>) -> CodexHeaders {
    CodexHeaders { conversation: conv.map(str::to_string), thread: thread.map(str::to_string) }
}

fn sock(pid: u32) -> Via {
    Via::Socket { peer_pid: Some(pid), vmm_peer: false }
}

fn code(r: Result<(), Refusal>) -> &'static str {
    match r {
        Ok(()) => "ok",
        Err(r) => r.code,
    }
}

/// Two settled Codex tabs in different folders: A on conv-A (terminal
/// process 100, a tool call is pid 150), B on conv-B (200, 250).
fn two_tabs() -> Fake {
    Fake { tree: vec![(150, 100), (250, 200)], ..Default::default() }
        .with(A, codex("conv-A", true, "/w/a", 100))
        .with(B, codex("conv-B", true, "/w/b", 200))
}

#[test]
fn a_call_that_is_not_codex_is_unchanged() {
    let f = Fake::default().with(
        C,
        TabFacts { codex: false, conversation: Some("claude-conv".into()), settled: true, ..Default::default() },
    );
    assert_eq!(code(decide(&f, C, &hdr(None, None), Via::Tcp)), "ok");
    assert_eq!(code(decide(&f, C, &hdr(None, None), Via::Socket { peer_pid: None, vmm_peer: false })), "ok");
    assert_eq!(*f.version_asks.lock().expect("asks"), 0, "a non-Codex call asks nothing");
}

#[test]
fn a_codex_call_without_its_conversation_is_refused() {
    let f = two_tabs();
    // A Codex tab, no headers at all.
    assert_eq!(code(decide(&f, A, &hdr(None, None), Via::Tcp)), "conversation_missing");
    // The thread alone.
    assert_eq!(code(decide(&f, A, &hdr(None, Some("conv-A")), sock(150))), "conversation_missing");
    // A header that isn't an id the CLI would send.
    assert_eq!(code(decide(&f, A, &hdr(Some("conv A;x"), None), Via::Tcp)), "conversation_missing");
    // Codex headers on a passport of a tab that isn't Codex.
    let f = Fake::default().with(C, TabFacts { codex: false, ..Default::default() });
    assert_eq!(code(decide(&f, C, &hdr(None, Some("t-1")), Via::Tcp)), "conversation_missing");
    assert_eq!(code(decide(&f, C, &hdr(Some("conv-X"), None), Via::Tcp)), "unknown_conversation");
}

#[test]
fn each_tab_with_its_own_root_works_and_a_subagent_too() {
    let f = two_tabs();
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A"), Some("conv-A")), Via::Tcp)), "ok");
    assert_eq!(code(decide(&f, B, &hdr(Some("conv-B"), Some("conv-B")), sock(250))), "ok");
    // A subagent: the same root, its own thread.
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A"), Some("child-thread-1")), sock(150))), "ok");
    assert!(f.rechecks().is_empty(), "a match needs no re-check");
}

#[test]
fn another_tabs_conversation_is_refused_without_a_recheck() {
    let f = two_tabs();
    let r = decide(&f, A, &hdr(Some("conv-B"), Some("conv-B")), Via::Tcp).expect_err("refused");
    assert_eq!(r.code, "passport_not_this_conversation");
    assert!(r.message.contains(&format!("close this tab and reopen it from {}", k2_core::brand::PRODUCT_NAME)), "{}", r.message);
    assert_eq!(r.codex_version.as_deref(), Some(CHECKED), "the log line sees the version");
    let body: serde_json::Value = serde_json::from_str(&r.body()).expect("json body");
    assert_eq!(body["error"], "passport_not_this_conversation");
    assert_eq!(body.as_object().expect("object").len(), 2, "the reply is the code and the fix only");
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-B"), None), sock(150))), "passport_not_this_conversation");
    assert!(f.rechecks().is_empty(), "an id another tab holds is never re-checked");
}

#[test]
fn an_unknown_conversation_is_refused_after_one_recheck() {
    let f = two_tabs();
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-Z"), None), Via::Tcp)), "unknown_conversation");
    assert_eq!(f.rechecks(), vec![(A.to_string(), "conv-Z".to_string())], "exactly one re-check");
}

#[test]
fn after_new_the_recheck_moves_the_tab_once_its_log_exists() {
    let mut f = two_tabs();
    f.next_log.insert(A.to_string(), "conv-A2".to_string());
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A2"), None), Via::Tcp)), "ok");
    assert_eq!(f.tab(A).expect("A").conversation.as_deref(), Some("conv-A2"));
    // The old conversation is now another log: refused.
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A"), None), Via::Tcp)), "unknown_conversation");
    // No log for it yet: refused.
    let f = two_tabs();
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A3"), None), sock(150))), "unknown_conversation");
}

#[test]
fn on_the_socket_the_caller_must_descend_from_the_tab() {
    let f = two_tabs();
    // B's tool call on A's socket with A's conversation: not A's process.
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A"), None), sock(250))), "not_this_tabs_process");
    // A process the OS can't name.
    let unknown = Via::Socket { peer_pid: None, vmm_peer: false };
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A"), None), unknown)), "not_this_tabs_process");
    // A tab with no terminal process recorded.
    let f = Fake { tree: vec![(150, 100)], ..Default::default() }
        .with(A, TabFacts { child_pid: None, ..codex("conv-A", true, "/w/a", 0) });
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A"), None), sock(150))), "not_this_tabs_process");
    // A microVM cell's forwarder: its Codex runs in the cell's own guest.
    let f = two_tabs();
    let vmm = Via::Socket { peer_pid: Some(9), vmm_peer: true };
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A"), None), vmm)), "ok");
}

#[test]
fn an_unchecked_codex_version_fails_closed() {
    let reopen = format!("then close this tab and reopen it from {P}");
    let resend = "send Codex one message and run the command again".to_string();
    for (answer, shown, fix) in [
        // Newer, with no log that confirms the conversation.
        (Ok("codex-cli 0.172.0".to_string()), "0.172.0", &resend),
        // Older than every checked release.
        (Ok("codex-cli 0.154.0".to_string()), "0.154.0", &reopen),
        // A version K2 can't read.
        (Ok("codex-cli 0.163.0-alpha.2".to_string()), "codex-cli 0.163.0-alpha.2", &reopen),
        (Err("probe timed out".to_string()), "unknown", &reopen),
    ] {
        let mut f = two_tabs();
        f.versions.insert(A.to_string(), answer.clone());
        let r = decide(&f, A, &hdr(Some("conv-A"), None), sock(150)).expect_err("unchecked version refused");
        assert_eq!(r.code, "codex_version_unverified", "{answer:?}");
        assert!(r.message.contains(&format!("({shown})")), "{}", r.message);
        // The agent's refusal ends at its fix. The off switch is the
        // owner's, in the docs, never offered to the agent.
        assert!(r.message.ends_with(fix.as_str()), "{}", r.message);
        assert!(!r.message.contains(BIND_ENV), "{}", r.message);
    }
}

#[test]
fn a_newer_codex_passes_only_when_its_log_confirms_and_warns_once() {
    let mut f = two_tabs();
    f.versions.insert(A.to_string(), Ok("codex-cli 0.180.2".to_string()));
    // No parsed log for the conversation: it can't be verified.
    let r = decide(&f, A, &hdr(Some("conv-A"), None), sock(150)).expect_err("unverified newer Codex");
    assert_eq!(r.code, "codex_version_unverified");
    assert!(r.message.contains("(0.180.2)"), "{}", r.message);
    assert!(f.warn_lines.lock().expect("warn lines").is_empty(), "no warning for a refused call");
    // Its log confirms the conversation: allowed, with one warning line.
    f.logs.push((A.to_string(), "conv-A".to_string()));
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A"), None), sock(150))), "ok");
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A"), None), Via::Tcp)), "ok");
    assert_eq!(*f.warn_lines.lock().expect("warn lines"), vec![newer_line("0.180.2")], "one warning per version");
    assert!(newer_line("0.180.2").starts_with("Codex 0.180.2 is newer than "));
    assert!(newer_line("0.180.2").ends_with(" has tested; conversation check passed"));
    // The conversation check still runs first: another tab's is refused.
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-B"), None), sock(150))), "passport_not_this_conversation");
}

#[test]
fn every_refusal_says_what_to_do() {
    let all = [
        missing_msg(),
        outside_msg(),
        other_tab_msg(),
        unknown_msg(),
        unsettled_msg(),
        older_msg("0.150.0"),
        unreadable_msg("unknown"),
        newer_unconfirmed_msg("0.180.0"),
    ];
    for m in all {
        let (_, fix) = m.split_once(": ").unwrap_or_else(|| panic!("no fix after the reason: {m}"));
        assert!(
            ["close ", "run ", "send ", "update ", "check "].iter().any(|v| fix.starts_with(v)),
            "the fix starts with what to do: {m}"
        );
    }
}

#[test]
fn over_tcp_two_unsettled_tabs_in_one_folder_are_refused() {
    let f = Fake::default()
        .with(A, codex("conv-A", false, "/w/same", 100))
        .with(B, codex("conv-B", false, "/w/same/", 200));
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A"), None), Via::Tcp)), "conversation_unsettled");
    // A settled neighbour doesn't count.
    let f = Fake::default()
        .with(A, codex("conv-A", false, "/w/same", 100))
        .with(B, codex("conv-B", true, "/w/same", 200));
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A"), None), Via::Tcp)), "ok");
    // Another folder doesn't count.
    let f = Fake::default()
        .with(A, codex("conv-A", false, "/w/one", 100))
        .with(B, codex("conv-B", false, "/w/two", 200));
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A"), None), Via::Tcp)), "ok");
}

#[test]
fn a_proven_socket_call_settles_an_adopted_conversation() {
    let f = Fake { tree: vec![(150, 100)], ..Default::default() }
        .with(A, codex("conv-A", false, "/w/same", 100))
        .with(B, codex("conv-B", false, "/w/same", 200));
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A"), None), sock(150))), "ok");
    assert_eq!(f.confirmed(), vec![(A.to_string(), "conv-A".to_string())]);
    // TCP never confirms.
    let f = Fake::default().with(A, codex("conv-A", false, "/w/alone", 100));
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-A"), None), Via::Tcp)), "ok");
    assert!(f.confirmed().is_empty());
}

#[test]
fn over_tcp_the_recheck_never_moves_a_tab_that_shares_its_folder() {
    let mut f = Fake::default()
        .with(A, codex("conv-A", true, "/w/same", 100))
        .with(B, codex("conv-B", true, "/w/same", 200));
    f.next_log.insert(A.to_string(), "conv-new".to_string());
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-new"), None), Via::Tcp)), "unknown_conversation");
    assert!(f.rechecks().is_empty(), "no TCP re-check beside another Codex tab");
    // On the socket the caller is proven to be A's.
    f.tree.push((150, 100));
    assert_eq!(code(decide(&f, A, &hdr(Some("conv-new"), None), sock(150))), "ok");
}

#[test]
fn the_switch_reads_the_daemon_env() {
    for (v, on) in [("0", false), ("off", false), (" FALSE ", false), ("no", false), ("1", true), ("", true), ("yes", true)] {
        let _g = k2_core::test_env::EnvVar::set(BIND_ENV, v);
        assert_eq!(bind_on(), on, "{BIND_ENV}={v:?}");
    }
    let _g = k2_core::test_env::EnvVar::remove(BIND_ENV);
    assert!(bind_on(), "on by default");
}

#[test]
fn hook_ingest_routes_keep_their_own_check() {
    assert!(!route_is_checked("/hook/complete"));
    assert!(!route_is_checked("/hook/event"));
    assert!(route_is_checked("/cli/whoami"));
    assert!(route_is_checked("/cli/msg"));
}

// ── The version pin ────────────────────────────────────────────────────

/// The releases the pin covers: both ends of each checked range and one
/// inside it.
fn pinned() -> Vec<(u32, u32, u32)> {
    let mut out = Vec::new();
    for f in codex_version::SUPPORTED {
        out.push(f.first);
        out.push(f.last);
        out.push((f.first.0, (f.first.1 + f.last.1) / 2, 0));
    }
    out
}

/// For every checked Codex release: a tool call's root, thread and resume
/// ids line up the way the check relies on (the facts read from Codex's
/// source, `codex-per-tab-passport-plan.md`), and the call passes as its
/// own tab's, a subagent's call passes, and a `/new` thread does not.
#[test]
fn version_pin_root_thread_and_resume_ids_line_up() {
    for v in pinned() {
        let facts = codex_version::facts_for(v).unwrap_or_else(|| panic!("{v:?} is a checked release"));
        assert_eq!(facts.root_env, "CODEX_SESSION_ID", "{v:?}");
        assert_eq!(facts.thread_env, "CODEX_THREAD_ID", "{v:?}");
        assert!(facts.top_level_root_is_thread, "{v:?}: a top-level root is its thread");
        assert!(facts.thread_is_rollout_id, "{v:?}: the thread is the rollout id K2 adopts");
        assert!(facts.resume_takes_thread_id, "{v:?}: `codex resume <id>` takes the thread id");

        // `codex resume R`: K2's record of the tab is R (argv). The call's
        // env, by the facts: root = thread = R.
        let resume = "0199aaaa-1111-7000-8000-00000000000a";
        let mut f = Fake { tree: vec![(150, 100)], ..Default::default() }
            .with(A, codex(resume, true, "/w/pin", 100));
        f.versions.insert(A.to_string(), Ok(format!("codex-cli {}.{}.{}", v.0, v.1, v.2)));
        let env: HashMap<&str, &str> = [(facts.root_env, resume), (facts.thread_env, resume)].into();
        let call = hdr(Some(env[facts.root_env]), Some(env[facts.thread_env]));
        assert_eq!(code(decide(&f, A, &call, sock(150))), "ok", "{v:?} top-level");
        // A subagent: root R, a child thread.
        let call = hdr(Some(resume), Some("0199aaaa-1111-7000-8000-0000000000c1"));
        assert_eq!(code(decide(&f, A, &call, sock(150))), "ok", "{v:?} subagent");
        // `/new`: a new thread is a new root, unknown until its log exists.
        let call = hdr(Some("0199aaaa-1111-7000-8000-0000000000b2"), None);
        assert_eq!(code(decide(&f, A, &call, sock(150))), "unknown_conversation", "{v:?} /new");
    }
    // Just outside every checked range: refused, never passed.
    for f in codex_version::SUPPORTED {
        let after = (f.last.0, f.last.1, f.last.2 + 1);
        let before = (f.first.0, f.first.1 - 1, 0);
        for v in [after, before] {
            if codex_version::facts_for(v).is_some() {
                continue; // inside the next range
            }
            let mut fake = Fake { tree: vec![(150, 100)], ..Default::default() }.with(A, codex("r", true, "/w", 100));
            fake.versions.insert(A.to_string(), Ok(format!("codex-cli {}.{}.{}", v.0, v.1, v.2)));
            assert_eq!(code(decide(&fake, A, &hdr(Some("r"), None), sock(150))), "codex_version_unverified", "{v:?}");
        }
    }
}

/// The CLI sends the root as X-K2-Conversation and the thread as
/// X-K2-Thread, from the env vars the facts name.
#[test]
fn version_pin_the_cli_sends_the_ids_the_facts_name() {
    let src = include_str!("../../../cli/src/cli-tools-policy.sh");
    for f in codex_version::SUPPORTED {
        let line = format!(r#"local root="${{{}:-}}" thread="${{{}:-}}""#, f.root_env, f.thread_env);
        assert!(src.contains(&line), "cli-tools-policy.sh no longer reads {line}");
    }
    assert!(src.contains(r#"_K2_CODEX_HDRS+=("X-K2-Conversation: $root")"#), "the root is X-K2-Conversation");
    assert!(src.contains(r#"_K2_CODEX_HDRS+=("X-K2-Thread: $thread")"#), "the thread is X-K2-Thread");
}
