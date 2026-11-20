//! Codex per-tab passport binding: a Codex tool call must come from the
//! conversation of the tab whose passport it carries.
//!
//! Codex 0.155+ can run one shared app-server per `CODEX_HOME`, and tool
//! calls under it inherit the env of the tab that started it: that tab's
//! `K2_HOOK_TOKEN` and `K2_HOOK_SOCK`. `validate_hook` proves a passport is
//! valid, not which conversation sent it. The `k2` CLI sends the call's
//! conversation in `X-K2-Conversation` (Codex's `CODEX_SESSION_ID`, the
//! ROOT thread, so a subagent names its parent conversation) and its own
//! thread in `X-K2-Thread` (`CODEX_THREAD_ID`, kept for the log only).
//! [`check_conversation`] compares the root with the conversation K2 knows
//! the tab is on, and fails closed:
//!
//! - A Codex call (a conversation or thread header, or a passport of a
//!   Codex tab) with no `X-K2-Conversation` is refused.
//! - On the tab socket the caller must descend from the tab's terminal
//!   process (the OS peer pid, walked up the process table). A process
//!   under a shared app-server descends from no tab.
//! - The tab's Codex version must be one K2 checked
//!   (`k2_core::terminal::codex_version::SUPPORTED`).
//! - A different conversation gets ONE re-check from K2's own record of
//!   the tab (`/new`, `/clear` and fork start a new log K2 hasn't seen):
//!   [`crate::activity_transcript::recheck_for`]. Then: another tab's
//!   conversation is `passport_not_this_conversation`; one no tab holds is
//!   `unknown_conversation`.
//! - Over TCP, where no peer process can be seen, a tab whose conversation
//!   K2 only adopted (a guess from the folder's newest log) is refused
//!   while another Codex tab in the same folder is also unsettled, and the
//!   re-check may move a tab only when no other Codex tab shares its
//!   folder. A call proven on the socket settles the tab's adopted
//!   conversation.
//!
//! A call without a passport (the owner token, a login) never reaches this
//! check, and a non-Codex call is unchanged. `K2_CODEX_PASSPORT_BIND=0` in
//! the daemon's env turns the check off (the daemon logs one line at boot).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use k2_core::log_debug;
use k2_core::session::SessionId;
use k2_core::terminal::codex_version;

use super::ValidatedHook;

/// The daemon env switch; `0`/`off`/`false`/`no` turns the check off.
pub const BIND_ENV: &str = "K2_CODEX_PASSPORT_BIND";

/// The longest parent walk from the socket's peer to the tab's process.
const MAX_ANCESTRY: usize = 32;

/// True unless the owner turned the check off in the daemon's env.
pub fn bind_on() -> bool {
    match std::env::var(BIND_ENV) {
        Ok(v) => !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "off" | "false" | "no"),
        Err(_) => true,
    }
}

/// The one boot line when the switch is off.
pub fn log_switch_at_boot() {
    if !bind_on() {
        log_debug!("[daemon/boot] {BIND_ENV}=0: Codex tool calls are not checked against their tab's conversation");
    }
}

/// The hook ingest routes keep their own pane pin and process owner
/// check; every other route a passport reaches is checked.
pub fn route_is_checked(path: &str) -> bool {
    !matches!(path, "/hook/complete" | "/hook/event")
}

/// The conversation headers of one request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexHeaders {
    /// `X-K2-Conversation`: the conversation's root thread.
    pub conversation: Option<String>,
    /// `X-K2-Thread`: the thread that made the call (log only).
    pub thread: Option<String>,
}

impl CodexHeaders {
    /// Read both headers from a request head. An empty value is absent.
    pub fn from_head(head: &str) -> Self {
        let get = |name: &str| {
            crate::routes::http::extract_header(head, name)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Self { conversation: get("x-k2-conversation"), thread: get("x-k2-thread") }
    }
}

/// How the request reached the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The tab's own socket. `peer_pid` is the OS's peer pid (`None` when
    /// it couldn't say); `vmm_peer` is a microVM cell's forwarder, whose
    /// Codex runs in the cell's own guest.
    Socket { peer_pid: Option<u32>, vmm_peer: bool },
    /// Loopback TCP: no peer process.
    Tcp,
}

/// A refused call: the reply's `error` code and its one-line fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
    /// The tab's `codex --version` answer, for the daemon log.
    pub codex_version: Option<String>,
}

impl Refusal {
    /// The 403 body: the code and the fix, nothing else.
    pub fn body(&self) -> String {
        serde_json::json!({ "error": self.code, "message": self.message }).to_string()
    }
}

/// The fix for a call that isn't this tab's: reopen it.
fn reopen() -> String {
    format!("this Codex call isn't from this tab's conversation: reopen the tab from {}", k2_core::brand::PRODUCT_NAME)
}

fn refuse(code: &'static str, message: String, version: &Option<String>) -> Refusal {
    Refusal { code, message, codex_version: version.clone() }
}

/// What K2 knows about one live tab.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TabFacts {
    /// The tab runs Codex.
    pub codex: bool,
    pub cwd: Option<String>,
    /// The conversation K2 knows the tab is on.
    pub conversation: Option<String>,
    /// From the launch argv or a hook, or adopted and since proven on
    /// the tab's socket. An adopted guess is not settled.
    pub settled: bool,
    /// The tab's terminal process.
    pub child_pid: Option<i32>,
}

/// K2's record of the live tabs (the process table and the follower).
pub(crate) trait Records {
    fn tab(&self, session_id: &str) -> Option<TabFacts>;
    /// Every OTHER live tab.
    fn others(&self, session_id: &str) -> Vec<TabFacts>;
    /// The tab's `codex --version` answer.
    fn version(&self, session_id: &str) -> Result<String, String>;
    /// `pid` is `ancestor` or descends from it.
    fn descends(&self, pid: u32, ancestor: i32) -> bool;
    /// The one re-check ([`crate::activity_transcript::recheck_for`]).
    fn recheck(&self, session_id: &str, wanted: &str) -> bool;
    /// A proven socket call settles the tab's adopted conversation.
    fn confirm(&self, session_id: &str, conversation: &str);
}

/// An id the CLI would send: letters, digits, `-` and `_`, at most 128.
fn id_shape_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn same_folder(a: &TabFacts, b: &TabFacts) -> bool {
    match (a.cwd.as_deref(), b.cwd.as_deref()) {
        (Some(x), Some(y)) => k2_core::chat_history::same_cwd(x, y.trim_end_matches('/')),
        _ => false,
    }
}

/// The rules (see the module notes), over any [`Records`].
pub(crate) fn decide(rec: &dyn Records, session_id: &str, h: &CodexHeaders, via: Via) -> Result<(), Refusal> {
    let tab = rec.tab(session_id);
    let codex_tab = tab.as_ref().is_some_and(|t| t.codex);
    if h.conversation.is_none() && h.thread.is_none() && !codex_tab {
        return Ok(());
    }
    let version = codex_tab.then(|| rec.version(session_id));
    let seen = version.as_ref().map(|v| match v {
        Ok(s) => s.clone(),
        Err(e) => format!("unknown ({e})"),
    });
    let Some(conv) = h.conversation.as_deref().filter(|c| id_shape_ok(c)) else {
        return Err(refuse("conversation_missing", reopen(), &seen));
    };
    let mut tab = tab.unwrap_or_default();
    let proven = match via {
        Via::Socket { vmm_peer: true, .. } => false,
        Via::Socket { peer_pid, .. } => {
            let ok = matches!((peer_pid, tab.child_pid), (Some(p), Some(c)) if rec.descends(p, c));
            if !ok {
                return Err(refuse("not_this_tabs_process", reopen(), &seen));
            }
            true
        }
        Via::Tcp => false,
    };
    if let Some(v) = &version {
        let checked = v
            .as_ref()
            .ok()
            .and_then(|s| codex_version::parse_version(s))
            .and_then(codex_version::facts_for)
            .is_some();
        if !checked {
            let shown = match v {
                Ok(s) => codex_version::parse_version(s)
                    .map(|(a, b, c)| format!("{a}.{b}.{c}"))
                    .unwrap_or_else(|| s.chars().take(64).collect()),
                Err(_) => "unknown".to_string(),
            };
            return Err(refuse(
                "codex_version_unverified",
                format!(
                    "this Codex version ({shown}) isn't one {p} has checked yet: update {p}, or set {BIND_ENV}=0 (owner only)",
                    p = k2_core::brand::PRODUCT_NAME
                ),
                &seen,
            ));
        }
    }
    if tab.conversation.as_deref() != Some(conv) {
        let others = rec.others(session_id);
        if others.iter().any(|o| o.conversation.as_deref() == Some(conv)) {
            return Err(refuse("passport_not_this_conversation", reopen(), &seen));
        }
        let shares_folder = others.iter().any(|o| o.codex && same_folder(o, &tab));
        let may_move = matches!(via, Via::Socket { .. }) || !shares_folder;
        if may_move && rec.recheck(session_id, conv) {
            tab = rec.tab(session_id).unwrap_or_default();
        }
        if tab.conversation.as_deref() != Some(conv) {
            return Err(refuse("unknown_conversation", reopen(), &seen));
        }
    }
    if via == Via::Tcp && !tab.settled {
        let others = rec.others(session_id);
        if others.iter().any(|o| o.codex && !o.settled && same_folder(o, &tab)) {
            return Err(refuse(
                "conversation_unsettled",
                format!(
                    "another new Codex tab in this folder hasn't settled its conversation yet: try again in a moment, or reopen the tab from {}",
                    k2_core::brand::PRODUCT_NAME
                ),
                &seen,
            ));
        }
    }
    if proven && !tab.settled {
        rec.confirm(session_id, conv);
    }
    Ok(())
}

/// The per-tab passport check for a request that carries a VALID tab
/// passport `v`. `Ok` lets the request through; a [`Refusal`] is a 403
/// with [`Refusal::body`]. Reads the process table and the folder's logs:
/// call it off the async workers (`spawn_blocking`).
pub fn check_conversation(v: &ValidatedHook, h: &CodexHeaders, via: Via) -> Result<(), Refusal> {
    if !bind_on() {
        return Ok(());
    }
    let out = decide(&LiveRecords, &v.session_id, h, via);
    if let Err(r) = &out {
        log_debug!(
            "[codex-passport] refused {} for session {} via {:?}: conversation={:?} thread={:?} codex={}",
            r.code,
            v.session_id,
            via,
            h.conversation,
            h.thread,
            r.codex_version.as_deref().unwrap_or("not a Codex tab")
        );
    }
    out
}

/// The 403 body when the check couldn't run at all.
fn check_failed() -> String {
    serde_json::json!({
        "error": "conversation_check_failed",
        "message": format!("{} couldn't check this call's conversation: try again", k2_core::brand::PRODUCT_NAME),
    })
    .to_string()
}

/// Run [`check_conversation`] off the async workers: `Some(403 body)` when
/// the call is refused.
async fn refusal_off_thread(v: ValidatedHook, h: CodexHeaders, via: Via) -> Option<String> {
    match tokio::task::spawn_blocking(move || check_conversation(&v, &h, via)).await {
        Ok(Ok(())) => None,
        Ok(Err(r)) => Some(r.body()),
        Err(_) => Some(check_failed()),
    }
}

/// The tab socket's door (cell_server): `validated` is the request's
/// passport, already bound to this cell. `Some(403 body)` refuses it.
pub async fn socket_refusal(validated: Option<&ValidatedHook>, path: &str, head: &str, via: Via) -> Option<String> {
    let v = validated.filter(|_| route_is_checked(path))?.clone();
    refusal_off_thread(v, CodexHeaders::from_head(head), via).await
}

/// The TCP prologue's door (dispatcher): only a request whose credential
/// (the bearer, else `?token=`) is a valid tab passport is checked; the
/// owner token and logins never are. `Some(403 body)` refuses it.
pub async fn tcp_refusal(path_and_query: &str, bearer: Option<&str>, head: &str) -> Option<String> {
    let (path, query) = path_and_query.split_once('?').unwrap_or((path_and_query, ""));
    let presented = crate::routes::http::presented_bearer(query, bearer);
    if !presented.contains('.') || !route_is_checked(path) {
        return None;
    }
    let v = super::validate_hook(presented)?;
    refusal_off_thread(v, CodexHeaders::from_head(head), Via::Tcp).await
}

// ── The live record ─────────────────────────────────────────────────────

/// Adopted conversations a proven socket call confirmed (session id →
/// conversation). Entries of tabs that are gone are dropped as it grows.
fn confirmed() -> &'static Mutex<HashMap<String, String>> {
    static C: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

const CONFIRMED_PRUNE_AT: usize = 1024;

fn is_confirmed(session_id: &str, conversation: &str) -> bool {
    confirmed()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_id)
        .is_some_and(|c| c == conversation)
}

struct LiveRecords;

fn facts_of(session: &k2_core::terminal::DaemonPtySession, agent: Option<&str>) -> TabFacts {
    let sid = session.session_id.to_string();
    let program = session.program.clone().unwrap_or_default();
    let cwd = session.cwd.as_ref().map(|p| p.to_string_lossy().into_owned());
    let (conversation, by_source) = match crate::activity_transcript::conversation_of(&sid) {
        Some((c, src)) => (Some(c), src != crate::activity_transcript::ConversationSource::Adopted),
        None => match crate::v2_spawn::conversation_id_from_args(session.program.as_deref(), &session.args) {
            Some(c) => (Some(c), true),
            None => (
                agent.and_then(|a| {
                    crate::v2_spawn::conversation_id_for_agent(
                        a,
                        cwd.as_deref().unwrap_or(""),
                        session.program.as_deref(),
                        &session.args,
                    )
                }),
                false,
            ),
        },
    };
    let settled = by_source || conversation.as_deref().is_some_and(|c| is_confirmed(&sid, c));
    TabFacts {
        codex: k2_core::terminal::codex_no_daemon::is_codex_program(&program),
        cwd,
        conversation,
        settled,
        child_pid: session.child_pid(),
    }
}

impl Records for LiveRecords {
    fn tab(&self, session_id: &str) -> Option<TabFacts> {
        let id = SessionId::parse(session_id)?;
        let session = crate::v2_session_map::lookup_by_session_id(&id)?;
        let agent = crate::v2_session_map::agent_name_for_session_id(&id);
        Some(facts_of(&session, agent.as_deref()))
    }

    fn others(&self, session_id: &str) -> Vec<TabFacts> {
        crate::v2_session_map::snapshot()
            .into_iter()
            .filter(|(_, s)| s.session_id.to_string() != session_id)
            .map(|(agent, s)| facts_of(&s, Some(agent.as_str())))
            .collect()
    }

    fn version(&self, session_id: &str) -> Result<String, String> {
        let program = SessionId::parse(session_id)
            .and_then(|id| crate::v2_session_map::lookup_by_session_id(&id))
            .and_then(|s| s.program.clone())
            .unwrap_or_default();
        codex_version::session_version(session_id, &program)
    }

    fn descends(&self, pid: u32, ancestor: i32) -> bool {
        use k2_core::agent_hooks::owner::{ancestry, ProcessTable};
        let table = k2_core::proc_ancestry::LiveProcessTable;
        let Ok(pid) = i32::try_from(pid) else { return false };
        let Some(info) = table.info(pid) else { return false };
        ancestry(&table, info, MAX_ANCESTRY).iter().any(|p| p.pid == ancestor)
    }

    fn recheck(&self, session_id: &str, wanted: &str) -> bool {
        crate::activity_transcript::recheck_for(session_id, wanted)
    }

    fn confirm(&self, session_id: &str, conversation: &str) {
        let mut g = confirmed().lock().unwrap_or_else(|e| e.into_inner());
        if g.len() >= CONFIRMED_PRUNE_AT {
            let live: std::collections::HashSet<String> =
                crate::v2_session_map::snapshot().into_iter().map(|(_, s)| s.session_id.to_string()).collect();
            g.retain(|sid, _| live.contains(sid));
        }
        g.insert(session_id.to_string(), conversation.to_string());
    }
}

/// The OS peer pid of a connected Unix socket: `SO_PEERCRED` on Linux,
/// `LOCAL_PEERPID` on macOS (as `vault::human` reads it). `None` where the
/// OS can't say, which the check refuses.
#[cfg(unix)]
pub fn socket_peer_pid(fd: std::os::unix::io::RawFd) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        let mut c = libc::ucred { pid: 0, uid: 0, gid: 0 };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: valid fd, sized out-param.
        let rc = unsafe {
            libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, &mut c as *mut _ as *mut libc::c_void, &mut len)
        };
        (rc == 0 && c.pid > 0).then_some(c.pid as u32)
    }
    #[cfg(target_os = "macos")]
    {
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        // SAFETY: valid fd, sized out-param. SOL_LOCAL = 0, LOCAL_PEERPID = 2.
        let rc = unsafe { libc::getsockopt(fd, 0, 0x002, &mut pid as *mut _ as *mut libc::c_void, &mut len) };
        (rc == 0 && pid > 0).then_some(pid as u32)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = fd;
        None
    }
}

#[cfg(test)]
#[path = "session_token_conversation_tests.rs"]
mod tests;
