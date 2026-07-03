//! HTTP handlers for Kessel session spawn + close.
//!
//! Endpoints (registered in main.rs):
//!   - `POST /cli/sessions/v2/spawn` — find-or-spawn by agent_name.
//!   - `POST /cli/sessions/v2/close` — explicit session teardown.
//!
//! Parallel to `awareness_ws::handle_sessions_spawn` /
//! `handle_sessions_close` which handle v1 / Kessel-T0's
//! `SessionStreamSession`. Kept separate so the two renderer paths
//! don't step on each other; v1's handlers stay untouched during
//! the v2 transition.
//!
//! Find-or-spawn semantics: the client (Tauri) calls this on every
//! tab mount. If a session already exists for the requested
//! `agent_name` (`tab-<terminalId>`), we return its existing
//! `{sessionId, cols, rows}` with `reused: true`. Otherwise we
//! spawn a fresh `DaemonPtySession` and register it. Tauri always
//! calls the same endpoint whether it's a cold launch or a reattach
//! after workspace swap / app quit. See `.k2so/prds/alacritty-v2.md`
//! phase A4.

use std::collections::HashMap;
use std::path::PathBuf;

use k2_core::log_debug;
use k2_core::session::SessionId;
use k2_core::terminal::sandbox::WorkspaceMountSpec;
use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession};

use crate::awareness_ws::HandlerResult;
use crate::pending_live;
use crate::signal_format;
use crate::v2_session_map;

/// Cheap UUID-shape probe: 36 chars, hyphens at the canonical
/// positions (8-4-4-4-12). Used to distinguish a bare `project_id`
/// canonical key (post-0.37.5) from legacy `<pid>:<agent>` strings
/// or ad-hoc tab keys (`tab-XXX`). Doesn't validate the hex digits
/// — fast path for the spawn helper's stamping logic.
fn is_uuid_shape(s: &str) -> bool {
    s.len() == 36
        && s.as_bytes()[8] == b'-'
        && s.as_bytes()[13] == b'-'
        && s.as_bytes()[18] == b'-'
        && s.as_bytes()[23] == b'-'
}

/// Resolve the `workspace_uuid` (`projects.id`) for a spawned cell's scoped
/// principal (#58 Phase 1). A bare-UUID `agent_name` IS the project id
/// (post-0.37.5 canonical key); otherwise resolve from the cwd. Best-effort:
/// the principal is attribution metadata, never a trust input — an empty
/// string is acceptable when the project can't be resolved. Only called when
/// scoped hooks are ON.
fn scoped_principal_workspace(agent_name: &str, cwd: &str) -> String {
    if is_uuid_shape(agent_name) {
        return agent_name.to_string();
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::workspace::agent_identity::resolve_project_id(&conn, cwd).unwrap_or_default()
}

/// Handler for `POST /cli/sessions/v2/spawn`.
///
/// Request body (JSON):
/// ```json
/// {
///   "agent_name": "tab-<terminalId>",
///   "cwd": "/optional/path",
///   "command": "optional program",
///   "args": ["optional", "args"],
///   "cols": 120,
///   "rows": 40,
///   "env": { "KEY": "val" }
/// }
/// ```
///
/// Response body (JSON):
/// ```json
/// {
///   "sessionId": "<uuid>",
///   "agentName": "tab-<terminalId>",
///   "cols": 120,
///   "rows": 40,
///   "reused": false
/// }
/// ```
///
/// `reused: true` means the caller's `agent_name` already had a
/// live session; we returned its handle instead of spawning.
/// Tauri's attach path treats reused and fresh identically.
/// The host-trusted spawn request consumed by [`spawn_session`]. Public within
/// the crate so the P3b policy-resolver (`v1_sandboxes::policy`) can build one
/// HOST-SIDE for an external `/v1/sandboxes` caller. The wire schema IS the v2
/// spawn body; the non-wire `ephemeral_cwd` is `#[serde(skip)]` so the
/// deserialized shape — and thus every existing v2 caller — is byte-identical.
#[derive(serde::Deserialize)]
pub struct SpawnRequest {
    pub agent_name: String,
    #[serde(default = "default_cwd")]
    pub cwd: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Option<Vec<String>>,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    /// Phase B: caller-supplied initial label. Sent to the
    /// first WS subscriber as `LabelInitial`. Empty/absent ⇒
    /// no seed; `Pty` source means PTY title events fill it.
    #[serde(default)]
    pub label: Option<String>,
    /// Phase B: lock the label so future PTY title events
    /// (e.g. claude --resume emitting "Claude Code") cannot
    /// overwrite it. Common cases: canonical workspace+agent
    /// session, heartbeat fire sessions, restored chat-history
    /// tabs whose label is the session-derived friendly name.
    #[serde(default)]
    pub label_locked: Option<bool>,
    /// Sandbox P1: opt-in request to sandbox this session. In P1 this is
    /// ACCEPT-AND-MARK — `true` resolves to the host-direct Passthrough
    /// backend (no real isolation yet) and is echoed back as the literal
    /// backend NAME, never rejected. Absent ⇒ default path (response
    /// unchanged, byte-identical to pre-seam).
    #[serde(default)]
    pub sandbox: Option<bool>,
    /// P3b: a daemon-provisioned EPHEMERAL workspace dir to remove on
    /// ChildExit (the `/v1/sandboxes` per-session cwd). `#[serde(skip)]` so it
    /// NEVER arrives off the wire — a caller can NEVER ask the daemon to delete
    /// an arbitrary path. `None` for every v2 caller (no teardown); `Some` only
    /// when the P3b policy-resolver provisioned a throwaway dir for an API
    /// session, in which case the child-exit observer removes it.
    #[serde(skip)]
    pub ephemeral_cwd: Option<PathBuf>,
    /// P4-H4: the `/v1/sandboxes` principal whose concurrent-cell quota slot
    /// this session HOLDS. `#[serde(skip)]` so it NEVER arrives off the wire (a
    /// caller can't spoof another principal's quota key). `None` for EVERY
    /// non-API caller (normal v2/spawn + cockpit) → no acquire, no release, exact
    /// default-OFF parity. `Some(<display_id>)` only when the P4-H4 spawn door
    /// already `try_acquire`d a slot; the child-exit observer `release`s it on
    /// teardown (the single authoritative point that fires on clean exit AND on
    /// crash/OOM/kill-9, so the counter can never leak).
    #[serde(skip)]
    pub principal_key: Option<String>,
    /// Sandbox v2 — the WORKSPACE-SCOPED **MIRROR** mount spec produced by the
    /// policy-resolver ([`crate::v1_sandboxes::policy::resolve_workspace_session`]).
    /// `#[serde(skip)]` so it NEVER arrives off the wire (a caller can never
    /// hand the daemon arbitrary host mount paths). `None` for EVERY v2/cockpit
    /// + ephemeral `/v1/sandboxes` caller → default-OFF parity. When `Some`, the
    /// spawn door chowns the RW sandbox-home + `/work` dirs to the per-session
    /// cell uid (P4-H6) and threads the spec into [`DaemonPtyConfig`] → the worker.
    #[serde(skip)]
    pub overlay: Option<WorkspaceMountSpec>,
    /// Sandbox v2 — a HOST-DECIDED session id the spawn MUST use instead of
    /// minting a fresh one. `#[serde(skip)]` (never off the wire). This is what
    /// makes the returned/addressable `sessionId` EQUAL the key of the persistent
    /// overlay layer (`~/.k2/sandbox-overlays/<ws>/<sid>/`), so a later
    /// resume can re-find the same layer. `None` for every other caller → the
    /// historical `SessionId::new()` mint (byte-identical). Only the
    /// workspace-scoped door sets it (to the id it provisioned the layer under).
    #[serde(skip)]
    pub forced_session_id: Option<SessionId>,
}

fn default_cwd() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
}
fn default_cols() -> u16 {
    80
}
fn default_rows() -> u16 {
    24
}

/// P4-H6: `chown(path, uid, uid)` — set BOTH owner and group to the per-session
/// uid, mode untouched. The worker drops to `uid:uid` (per-session uid == gid),
/// so the ephemeral workspace must be owned `uid:uid` for the priv-dropped VMM
/// to mount + write it. Mirrors `cell_uds::set_cell_socket_owner`'s libc call but
/// also sets the group (the socket leaves the group as `-1`; here we want the
/// per-session gid so a future per-cell group bit can't leak access to another
/// cell). Only ever called on a daemon-MINTED ephemeral path, never a caller one.
#[cfg(unix)]
fn chown_path_to_uid(path: &std::path::Path, uid: u32) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    // SAFETY: `c_path` is a valid NUL-terminated path kept alive across the call;
    // owner uid AND group gid are both set to the per-session id.
    let rc = unsafe {
        libc::chown(c_path.as_ptr(), uid as libc::uid_t, uid as libc::gid_t)
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// SLICE 4 (resume/fork): a RECURSIVE `chown` of an EXISTING per-session layer to
/// the new cell uid, so files written under a PRIOR session's uid (the restored
/// `<id>.jsonl`, memory, /work) become readable+writable by this respawn's cell.
/// Shallow `chown_path_to_uid` only re-owns the top inode; the tree walk re-owns
/// every entry. Does not follow symlinked dirs (the layer is our own files).
/// Pass 1: reclaim a (possibly foreign-owned 0700) tree to the DAEMON, TOP-DOWN.
/// The daemon has CAP_CHOWN (chown needs no read) but NOT DAC_READ, so it must
/// chown the dir to itself FIRST, THEN it can read_dir + recurse.
fn chown_tree_reclaim_to_self(path: &std::path::Path) {
    #[cfg(unix)]
    unsafe {
        use std::os::unix::ffi::OsStrExt;
        if let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) {
            libc::chown(c.as_ptr(), libc::geteuid(), libc::getegid());
        }
    }
    if path.is_dir() && !path.is_symlink() {
        if let Ok(rd) = std::fs::read_dir(path) {
            for e in rd.flatten() {
                chown_tree_reclaim_to_self(&e.path());
            }
        }
    }
}

/// Pass 2: assign a now-daemon-owned tree to the cell uid, BOTTOM-UP (recurse
/// while we still own+read the dir, chown the dir itself last).
fn chown_tree_assign(path: &std::path::Path, uid: u32) -> std::io::Result<()> {
    if path.is_dir() && !path.is_symlink() {
        for entry in std::fs::read_dir(path)? {
            chown_tree_assign(&entry?.path(), uid)?;
        }
    }
    chown_path_to_uid(path, uid)
}

/// Recursively re-own an EXISTING per-session layer to the cell uid on resume/
/// fork. TWO passes because the daemon can neither read nor keep-readable a
/// foreign-owned 0700 tree: (1) reclaim to the daemon (top-down) so it becomes
/// readable, (2) assign to the cell uid (bottom-up). Fresh daemon-owned dirs
/// pass through cheaply.
fn chown_tree_to_uid(path: &std::path::Path, uid: u32) -> std::io::Result<()> {
    chown_tree_reclaim_to_self(path);
    chown_tree_assign(path, uid)
}

/// Whether THIS daemon can deliver a real microVM-isolated cell: a Linux build
/// compiled with `sandbox-microvm` AND with `K2_SANDBOX` enabled at runtime —
/// the EXACT condition under which [`resolve_sandbox`] resolves `Microvm` (this
/// is the single source of truth; both consult the same compile-gate + runtime
/// flag, so they can never diverge). Surfaced so the PUBLIC `/v1/sandboxes`
/// route can REFUSE (409) rather than silently degrade to an unsandboxed
/// passthrough cell. On macOS / any feature-off build → `false` (always 409).
pub fn can_sandbox() -> bool {
    #[cfg(all(target_os = "linux", feature = "sandbox-microvm"))]
    {
        return k2_sandbox_enabled();
    }
    #[cfg(not(all(target_os = "linux", feature = "sandbox-microvm")))]
    {
        false
    }
}

/// Handler for `POST /cli/sessions/v2/spawn` — parse the wire body then defer to
/// [`spawn_session`]. Thin wrapper: ALL spawn plumbing lives in `spawn_session`
/// so `/v1/sandboxes` reuses it verbatim with a host-trusted request, and the v2
/// path stays behavior-identical (parse error → the same 400 as before).
pub fn handle_v2_spawn(body: &[u8]) -> HandlerResult {
    let req: SpawnRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return HandlerResult {
                status: "400 Bad Request",
                body: format!(
                    r#"{{"error":"parse v2 SpawnRequest: {}"}}"#,
                    e.to_string().replace('"', "'")
                ),
            }
        }
    };
    spawn_session(req)
}

/// The PROVEN v2 spawn internals: find-or-spawn by `agent_name`, restart
/// recovery, #58 scoped token/UDS + B3a key-injection, the child-exit observer,
/// and microVM cgroup/dir teardown. Extracted UNCHANGED from `handle_v2_spawn`
/// (the only delta: it now takes an already-parsed [`SpawnRequest`] and honors
/// `ephemeral_cwd` teardown) so the existing v2 path is behavior-identical;
/// `/v1/sandboxes` calls it directly with a host-trusted request from the
/// policy-resolver.
pub fn spawn_session(req: SpawnRequest) -> HandlerResult {
    if req.agent_name.is_empty() {
        return HandlerResult {
            status: "400 Bad Request",
            body: r#"{"error":"agent_name required"}"#.into(),
        };
    }

    let __t_total = std::time::Instant::now();

    // Find-or-spawn: existing session wins. The response preserves
    // whatever cols/rows the existing session was opened at — the
    // caller will ResizeObserver-correct if its viewport differs.
    let __t_lookup = std::time::Instant::now();
    let existing = v2_session_map::lookup_by_agent_name(&req.agent_name);
    let lookup_ms = __t_lookup.elapsed().as_secs_f64() * 1000.0;

    // A stale map entry whose child already exited must NOT be reused —
    // doing so would hand the caller a dead PTY. Evict + reap it (the
    // reap is belt-and-suspenders: a child that exited on its own is
    // usually already gone, but a half-dead process group or an
    // unobserved exit could leave a straggler) and fall through to spawn
    // a fresh replacement below.
    if let Some(existing) = existing.as_ref() {
        if !existing.is_child_alive() {
            log_debug!(
                "[v2-spawn] existing session for agent={} has a dead child; evicting + killing before respawn",
                req.agent_name
            );
            // unregister runs the DB/active-session cleanup chokepoint
            // and itself calls kill(); the explicit kill() here is
            // idempotent and guarantees teardown even if the key was
            // already removed by a racing unregister.
            v2_session_map::unregister(&req.agent_name);
            existing.kill();
        }
    }
    let existing = existing.filter(|s| s.is_child_alive());

    if let Some(existing) = existing {
        let (cols, rows) = current_dims(&existing);
        let session_id_str = existing.session_id.to_string();
        let total_ms = __t_total.elapsed().as_secs_f64() * 1000.0;
        log_debug!(
            "[v2-perf] side=daemon SPAWN_SUMMARY session={} agent={} reused=true total_ms={:.3} lookup_ms={:.3} dpty_spawn_ms=0",
            session_id_str, req.agent_name, total_ms, lookup_ms
        );
        let mut out = serde_json::json!({
            "sessionId": session_id_str,
            "agentName": req.agent_name,
            "cols": cols,
            "rows": rows,
            "reused": true,
        });
        // A2/B3a — reuse-echo. Mirror the cold-spawn accept-and-mark rule:
        // echo the EXISTING session's resolved backend name only when the
        // caller asked for a sandbox. Absent ⇒ no field ⇒ response
        // byte-identical to pre-seam (default-path regression guard).
        if req.sandbox.is_some() {
            out["sandbox"] =
                serde_json::Value::String(existing.sandbox.backend().name().to_string());
        }
        return HandlerResult {
            status: "200 OK",
            body: out.to_string(),
        };
    }

    // 0.38.5 — restart-recovery: if the daemon was just restarted (app
    // update / launchctl kickstart / crash) the in-memory
    // v2_session_map is empty but `workspace_tab_sessions` still has
    // the prior spawn's command + args + (claude) session_id. When
    // the renderer's spawn request lands with empty command (which
    // v2 schema makes the default for terminal items), substitute
    // the persisted values so we re-run e.g. `claude --resume <id>`
    // instead of dropping the user into a bare shell. Renderer-side
    // command takes precedence on first spawn; only the empty case
    // consults the table. See `0045_workspace_tab_sessions.sql`.
    let mut command = req.command.clone();
    let mut args = req.args.clone().unwrap_or_default();
    if command.is_none() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(project_id) = k2_core::workspace::agent_identity::resolve_project_id(&conn, &req.cwd) {
            // pinned-chat-identity-ssot PRD §4.3.1 (GH#24): the canonical
            // pinned chat (`agent_name == project_id`) sources its
            // `--resume` id from the SINGLE SOURCE OF TRUTH —
            // `workspace_sessions.session_id` — NOT from
            // `workspace_tab_sessions`. Recovery used to read the tab
            // table's argv-derived id, which is the coupling that let the
            // daemon-owned pinned chat (#683) bypass the canonical column
            // for identity. Command/cwd may still come from the tab row if
            // one happens to exist (legacy/pre-Phase-3), but after Phase 3
            // the pinned row is no longer written, so we default to
            // `claude` + the request cwd and splice `--resume <ssot-id>`.
            // Ad-hoc Cmd+T tabs (`agent_name == tab-<...>`) are untouched
            // below — they have no workspace_sessions row and legitimately
            // recover from workspace_tab_sessions.
            let is_pinned = req.agent_name == project_id;

            // Default the command from the tab row when present; the
            // pinned chat falls back to `claude` so recovery works even
            // with no tab row (Phase 3).
            let tab_row = k2_core::db::schema::WorkspaceTabSession::get_by_agent_name(
                &conn,
                &project_id,
                &req.agent_name,
            )
            .ok()
            .flatten();

            // Source the resume id: SSOT for the pinned chat, the tab
            // row's argv-derived id for ad-hoc tabs.
            let resume_id: Option<String> = if is_pinned {
                k2_core::db::schema::WorkspaceSession::get(&conn, &project_id)
                    .ok()
                    .flatten()
                    .and_then(|row| row.session_id)
                    .filter(|s| !s.is_empty())
            } else {
                tab_row.as_ref().and_then(|r| r.session_id.clone())
            };

            // Pick the command + base args. Tab row wins when it carries a
            // command; the pinned chat's no-tab-row fallback (Phase 3)
            // used to hardcode `claude` — agent-degeneralization S2
            // resolves the workspace/global default agent instead
            // (projects.default_agent → AppSettings.default_agent →
            // claude). The bool tracks whether the recovered command may
            // receive the `--resume` splice below.
            let recovered: Option<(String, Vec<String>, bool)> = if let Some(cmd) =
                tab_row.as_ref().and_then(|r| r.command.clone())
            {
                // Tab-row path — unchanged legacy behavior.
                let saved_args: Vec<String> = tab_row
                    .as_ref()
                    .and_then(|r| r.args_json.as_deref())
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_else(|| {
                        // Tab row without args: the standard pinned-chat
                        // base flags so a recovered claude still runs
                        // headlessly.
                        vec!["--dangerously-skip-permissions".to_string()]
                    });
                Some((cmd, saved_args, true))
            } else if is_pinned {
                let resolved = k2_core::workspace::agent_resolve::resolve_agent_command(
                    &conn,
                    &project_id,
                );
                if resolved.is_claude() {
                    let mut base = resolved.args.clone();
                    // Recovered pinned claude must stay headless even on
                    // a customized preset.
                    k2_core::workspace::agent_resolve::ensure_flag(
                        &mut base,
                        "--dangerously-skip-permissions",
                    );
                    Some((resolved.command, base, true))
                } else {
                    // Slice 3: `--resume` (and the SSOT resume id itself)
                    // are Claude grammar — a non-claude default recovers
                    // the pinned chat with the preset's own command+args
                    // bare (fresh conversation, no resume) until the
                    // ProviderResume adapter lands.
                    Some((resolved.command, resolved.args, false))
                }
            } else {
                None
            };

            if let Some((saved_cmd, mut saved_args, may_splice_resume)) = recovered {
                // If we have a resume id, strip any existing
                // `--session-id` flag (we replace it with `--resume` for
                // unambiguous resumption) and splice in `--resume <id>`.
                // The base args carry --dangerously-skip-permissions and
                // similar flags we want to keep.
                if let Some(sid) = resume_id.as_deref().filter(|_| may_splice_resume) {
                    // Drop any --session-id <value> pair.
                    let mut i = 0;
                    while i + 1 < saved_args.len() {
                        if saved_args[i] == "--session-id" {
                            saved_args.remove(i); // flag
                            saved_args.remove(i); // value
                        } else {
                            i += 1;
                        }
                    }
                    let already_has_resume = saved_args
                        .iter()
                        .any(|a| a == "--resume" || a == "-r");
                    if !already_has_resume {
                        saved_args.push("--resume".to_string());
                        saved_args.push(sid.to_string());
                    }
                }
                log_debug!(
                    "[v2-spawn] restart-recovery: project={} agent={} pinned={} resume_source={} replayed command={} args={:?}",
                    project_id,
                    req.agent_name,
                    is_pinned,
                    if is_pinned { "workspace_sessions(SSOT)" } else { "workspace_tab_sessions" },
                    saved_cmd,
                    saved_args
                );
                command = Some(saved_cmd);
                args = saved_args;
            }
        }
    }

    // 0.38.8 — Cmd+T session continuity. When spawning `claude` with
    // no `--session-id` and no `--resume` (the common Cmd+T-from-the-
    // Tauri-renderer shape), mint a fresh UUID and inject
    // `--session-id <uuid>` so claude persists its conversation to a
    // known-id JSONL. The v2_session_map::register hook reads the
    // injected flag and stamps `workspace_tab_sessions.session_id`,
    // which makes the restart-recovery branch above splice
    // `--resume <uuid>` on the next daemon restart. Net: Cmd+T tabs
    // resume the same conversation after app updates / kickstart.
    if command.as_deref() == Some("claude") {
        let has_session_id = args.iter().any(|a| a == "--session-id");
        let has_resume = args.iter().any(|a| a == "--resume" || a == "-r");
        if !has_session_id && !has_resume {
            let new_sid = uuid::Uuid::new_v4().to_string();
            log_debug!(
                "[v2-spawn] auto-injected --session-id={} for agent={} cwd={}",
                new_sid, req.agent_name, req.cwd
            );
            args.push("--session-id".to_string());
            args.push(new_sid);
        }
    }

    // 2026-07-02 PTY-leak breaker — refuse to HOLD unbounded abandoned
    // bare shells for one workspace. The split-pane restore re-mint loop
    // (client bug, fixed in b339c70; shipped broken since 0.39.39 so
    // released clients still carry it) minted a fresh `tab-<uuid>`
    // agent_name every layout-echo cycle; each landed here with no
    // command, became a bare login/zsh nothing ever attached to, and the
    // box eventually ran out of PTYs (kern.tty.ptmx_max). Defense in
    // depth: cap the number of live NEVER-ATTACHED bare-shell `tab-*`
    // sessions per cwd. Scope is deliberately the leak's exact shape —
    // ad-hoc `tab-*` spawns with NO command. Recovered/claude/heartbeat/
    // pinned spawns (command present) are untouched, and tabs a client
    // ever streamed (`ever_attached`) never count, so a user with many
    // idle-but-viewed terminals is unaffected. 4xx is surfaced
    // immediately by TerminalPane (no client retry loop).
    if command.is_none() && req.agent_name.starts_with("tab-") {
        let cap: usize = std::env::var("K2_V2_BARE_TAB_CAP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(32);
        let req_cwd = std::path::PathBuf::from(&req.cwd);
        let unwatched_bare = v2_session_map::snapshot()
            .into_iter()
            .filter(|(key, s)| {
                key.starts_with("tab-")
                    && s.program.is_none()
                    && s.is_child_alive()
                    && !s
                        .ever_attached
                        .load(std::sync::atomic::Ordering::Relaxed)
                    && s.cwd.as_ref() == Some(&req_cwd)
            })
            .count();
        if unwatched_bare >= cap {
            log_debug!(
                "[v2-spawn] bare-tab cap: REFUSING fresh spawn agent={} cwd={} — {} live never-attached bare shells (cap {})",
                req.agent_name, req.cwd, unwatched_bare, cap
            );
            return HandlerResult {
                status: "429 Too Many Requests",
                body: format!(
                    r#"{{"error":"workspace already holds {} never-attached bare-shell sessions (cap {}) — refusing to spawn another; close or attach existing tabs","code":"bare_tab_cap"}}"#,
                    unwatched_bare, cap
                ),
            };
        }
    }

    // Spawn a fresh session.
    // Phase B: pick the label seed + source. Caller-supplied label
    // (with optional lock) takes priority; otherwise we leave the
    // label empty and let PTY title events fill it (the legacy
    // pre-Phase-B behavior). `label_locked: true` with no label
    // still locks (caller is saying "don't accept ANY PTY title").
    let seed_label = req.label.clone().unwrap_or_default();
    let label_locked_flag = req.label_locked.unwrap_or(false);
    let label_source = if label_locked_flag {
        k2_core::terminal::LabelSource::Locked
    } else if !seed_label.is_empty() {
        k2_core::terminal::LabelSource::Seed
    } else {
        k2_core::terminal::LabelSource::Pty
    };
    // Sandbox P1 — accept-and-mark. `--sandbox`/`{"sandbox":true}` resolves to
    // a backend NAME (Passthrough in P1, no real isolation yet); we echo that
    // name in the response when the caller asked, and never reject. When the
    // request omits `sandbox`, `sandbox_echo` stays `None` and the response is
    // byte-identical to pre-seam (default-path regression guard).
    let sandbox_spec = resolve_sandbox(req.sandbox.unwrap_or(false));
    let sandbox_echo: Option<&'static str> =
        req.sandbox.map(|_| sandbox_spec.backend().name());
    let mut cfg = DaemonPtyConfig {
        // Sandbox v2: honor a HOST-DECIDED session id (workspace-scoped door) so
        // the returned/addressable id equals the persistent overlay-layer key;
        // every other caller mints a fresh one (byte-identical to pre-v2).
        session_id: req.forced_session_id.unwrap_or_else(SessionId::new),
        cols: req.cols,
        rows: req.rows,
        cwd: Some(PathBuf::from(&req.cwd)),
        program: command,
        args,
        env: req.env.unwrap_or_default(),
        drain_on_exit: true,
        label: seed_label,
        label_source,
        sandbox: sandbox_spec,
        // P4-H6: stamped below (the per-session uid the door allocates for a
        // microVM cell, threaded into the worker as the drop target). `None`
        // here; set just before `DaemonPtySession::spawn` for microVM cells only.
        cell_uid: None,
        // Sandbox v2 (PRD §B): the workspace-scoped overlay mount spec (RO base +
        // persistent upper/work + fs_mode). `None` for every non-workspace spawn.
        // Threaded into the worker by `daemon_pty`; chowned at the door below.
        overlay: req.overlay.clone(),
    };
    let session_id_for_response = cfg.session_id;

    // COMPAT-58 (#58 Phase 1) — OPT-IN per-cell scoped token + UDS, gated on
    // K2_HOOK_SCOPED (default OFF → this whole block is skipped and the env is
    // untouched, i.e. ZERO behavior change AND zero extra work — no principal
    // resolution / db lock on the hot spawn path). When ON we mint a per-cell
    // scoped token (NOT the owner token) + the deterministic per-cell socket
    // path and inject them into the child env behind the SAME K2_HOOK_TOKEN
    // key + K2_HOOK_SOCK. The owner token is STILL accepted over TCP
    // (dual-accept); Phase 2 (owner REJECTION) ships separately. The socket
    // is bound + served AFTER spawn below.
    if crate::session_token::scoped_hooks_enabled() {
        let pane_id = session_id_for_response.to_string();
        let principal = crate::session_token::HookPrincipal {
            workspace_uuid: scoped_principal_workspace(&req.agent_name, &req.cwd),
            agent_address: req.agent_name.clone(),
        };
        // B3a — resolve the credential mode + per-workspace key HOST-SIDE.
        // The cell NEVER chooses its mode/key. ApiKey is the only built mode
        // (Subscription is a deferred no-op stub). The PER-WORKSPACE key is
        // staged ONLY for a microVM-backed cell (the jail that isolates it);
        // a non-microVM (passthrough) session resolves NO key → byte-identical
        // default-OFF parity (no DB lookup, no env delta). On Mac / feature-off
        // builds `resolve_sandbox` is always Passthrough, so this never fires.
        let cred_mode = crate::session_token::CredMode::ApiKey;
        let provider = crate::session_token::Provider::Anthropic;
        let microvm_backed =
            matches!(cfg.sandbox, k2_core::terminal::SandboxSpec::Microvm);
        let workspace_api_key: Option<String> = if microvm_backed {
            let key = k2_core::workspace::settings::get_workspace_api_key(&req.cwd);
            // NEVER log the key itself — only its presence.
            if key.is_some() {
                log_debug!(
                    "[hook-scoped] B3a: staging per-workspace ANTHROPIC_API_KEY into microVM cell for session={}",
                    session_id_for_response
                );
            } else {
                log_debug!(
                    "[hook-scoped] B3a: microVM session={} has NO per-workspace API key configured; cell will have no Anthropic cred",
                    session_id_for_response
                );
            }
            key
        } else {
            None
        };
        if let Some(pairs) = crate::session_token::cell_env_pairs(
            &session_id_for_response,
            &pane_id,
            principal,
            cred_mode,
            provider,
            workspace_api_key.as_deref(),
        ) {
            for (k, v) in pairs {
                cfg.env.insert(k, v);
            }
            log_debug!(
                "[hook-scoped] injected scoped K2_HOOK_TOKEN + K2_HOOK_SOCK for session={}",
                session_id_for_response
            );
        }
    }

    // P4-H6 — PER-SESSION worker uid + fail-closed PER-SESSION egress, set up
    // BEFORE the cell boots. Each microVM cell now drops to its OWN distinct uid
    // (allocated here from the reserved range) instead of the shared `k2cell`, so
    // guest-written files are host-owned by a per-session uid (true multi-tenant
    // fs isolation) and the egress nft rule is per-tenant. Three steps, every one
    // fail-closed:
    //   (a) ALLOCATE a distinct uid — exhausted → 503 (a second concurrency bound
    //       on top of H4); we NEVER boot two cells under one uid.
    //   (b) hand the daemon-minted EPHEMERAL workspace to that uid (so the VMM can
    //       write it) — never a caller path; a chown failure frees the uid +
    //       refuses.
    //   (c) install THIS uid's per-session egress allowlist (default-DROP + only
    //       443/53) — a failure frees the uid + refuses.
    // Then thread the uid into the worker (drop target), the socket chown, the
    // peer-cred belt, and the authoritative teardown. `per_session_uid` is the
    // single carrier; `None` ⇒ no allocation happened (every non-microVM spawn) →
    // default-OFF parity. On Mac / feature-off builds `cfg.sandbox` is never
    // `Microvm`, so this whole block is dead there.
    let mut per_session_uid: Option<u32> = None;
    #[cfg(unix)]
    if matches!(cfg.sandbox, k2_core::terminal::SandboxSpec::Microvm) {
        // (a) allocate a DISTINCT per-session uid.
        let cell_uid = match crate::cell_uid_pool::alloc() {
            Some(u) => u,
            None => {
                log_debug!(
                    "[sandbox] P4-H6: uid pool EXHAUSTED for session={}; REFUSING to boot (fail-closed)",
                    session_id_for_response
                );
                return HandlerResult {
                    status: "503 Service Unavailable",
                    body: r#"{"error":"sandbox uid pool exhausted; refusing to boot microVM cell","code":"uid-pool-exhausted"}"#
                        .to_string(),
                };
            }
        };

        // (b) hand the ephemeral workspace to THIS uid. Only the daemon-MINTED
        // ephemeral dir is chowned (never a caller path); a v2-cockpit microVM
        // cell has no ephemeral dir (`ephemeral_cwd` None) → skip, matching
        // pre-H6 (its real workspace is not chowned). Fail-closed.
        if let Some(ws) = req.ephemeral_cwd.as_ref() {
            if let Err(e) = chown_path_to_uid(ws, cell_uid) {
                crate::cell_uid_pool::free(cell_uid);
                log_debug!(
                    "[sandbox] P4-H6 chown ephemeral workspace {} to cell uid {cell_uid} FAILED for session={}: {e}; REFUSING (fail-closed)",
                    ws.display(),
                    session_id_for_response
                );
                return HandlerResult {
                    status: "500 Internal Server Error",
                    body: format!(
                        r#"{{"error":"could not hand ephemeral workspace to sandbox cell uid: {}"}}"#,
                        e.to_string().replace('"', "'")
                    ),
                };
            }
        }

        // (b2) Sandbox v2 (MIRROR model) — hand the two RW mirror dirs to THIS
        // uid: the per-WORKSPACE sandbox home (`~/.k2/sandbox-homes/<ws>/.claude`)
        // and the per-SESSION `/work` scratch
        // (`~/.k2/sandbox-overlays/<ws>/<sid>/work-scratch`). The RO workspace
        // MIRROR (`workspace_ro`) and the RO canonical memory (`canonical_memory_ro`)
        // are DELIBERATELY NOT chowned — they stay owned by the workspace / the
        // daemon and are remounted read-only, so the cell can never corrupt them.
        // The chown makes the RW dirs 0700-owned by the cell uid → cross-tenant
        // unreadable (P4-H6); the daemon audits/lists them host-side via its own
        // privilege (see the fs-mirror PRD §5/§ownership). Fail-closed: any chown
        // failure frees the uid + refuses.
        //
        // RESUME CONSISTENCY (SLICE 4): the pool uid may DIFFER on a later resume;
        // the model is to RE-CHOWN the RW dirs to the fresh uid on every
        // (re)spawn. Here (fresh session) the dirs are newly created — a shallow
        // chown of the dir inodes is sufficient. SLICE 4 (resume, non-empty
        // sandbox home / work) must upgrade this to a RECURSIVE chown so
        // pre-existing files re-own to the new uid. NOTE (flagged for on-box
        // review): the per-workspace sandbox home is SHARED across that
        // workspace's sessions; a single-uid chown makes it effectively
        // single-writer-at-a-time — concurrent same-workspace sessions need the
        // shared-group ownership model (deferred; see the report).
        if let Some(ov) = cfg.overlay.as_ref() {
            for dir in [&ov.sandbox_home_rw, &ov.work_rw] {
                if let Err(e) = chown_tree_to_uid(dir, cell_uid) {
                    crate::cell_uid_pool::free(cell_uid);
                    log_debug!(
                        "[sandbox] P4-H6 chown RW mirror dir {} to cell uid {cell_uid} FAILED for session={}: {e}; REFUSING (fail-closed)",
                        dir.display(),
                        session_id_for_response
                    );
                    return HandlerResult {
                        status: "500 Internal Server Error",
                        body: format!(
                            r#"{{"error":"could not hand RW mirror dir to sandbox cell uid: {}"}}"#,
                            e.to_string().replace('"', "'")
                        ),
                    };
                }
            }
        }

        // (c) install THIS uid's per-session fail-closed egress lockdown.
        if let Err(e) = crate::cell_egress::ensure_egress_policy(cell_uid) {
            crate::cell_uid_pool::free(cell_uid);
            log_debug!(
                "[sandbox] P4-H6 egress lockdown install FAILED for session={} uid={cell_uid}: {e}; REFUSING to boot cell (fail-closed)",
                session_id_for_response
            );
            return HandlerResult {
                status: "500 Internal Server Error",
                body: format!(
                    r#"{{"error":"egress lockdown install failed; refusing to boot microVM cell with open egress: {}"}}"#,
                    e.to_string().replace('"', "'")
                ),
            };
        }
        log_debug!(
            "[sandbox] P4-H6 per-session uid {cell_uid} allocated + egress allowlist installed (skuid {cell_uid}: default-DROP + 443/53) for session={}",
            session_id_for_response
        );

        // Thread the uid into the worker (drop target) + carry it forward.
        cfg.cell_uid = Some(cell_uid);
        per_session_uid = Some(cell_uid);
    }

    let __t_spawn = std::time::Instant::now();
    let session = match DaemonPtySession::spawn(cfg) {
        Ok(s) => s,
        Err(e) => {
            // P4-H6: the cell never booted → free the per-session uid + tear down
            // its egress table HERE (no child-exit observer will fire). `None`
            // for every non-microVM spawn → no-op. Best-effort egress remove.
            if let Some(uid) = per_session_uid {
                let _ = crate::cell_egress::remove_egress_policy(uid);
                crate::cell_uid_pool::free(uid);
                log_debug!(
                    "[sandbox] P4-H6 spawn failed for session={}; freed per-session uid {uid} + removed egress",
                    session_id_for_response
                );
            }
            return HandlerResult {
                status: "500 Internal Server Error",
                body: format!(
                    r#"{{"error":"v2 spawn failed: {}"}}"#,
                    e.to_string().replace('"', "'")
                ),
            }
        }
    };
    let dpty_spawn_ms = __t_spawn.elapsed().as_secs_f64() * 1000.0;

    // COMPAT-58 (#58 Phase 1) — OPT-IN per-cell UDS, gated on K2_HOOK_SCOPED
    // (default OFF → never runs → ZERO behavior change). When ON we bind the
    // cell's socket (0600 in 0700) and hand the listener to the per-cell hook
    // server, which authenticates + serves `/hook/complete` for THIS cell
    // (structural socket binding + scoped-token + peer-cred belt). The scoped
    // token + this socket's path were already injected into the child env
    // above. Bind failure logs + degrades to the TCP hook path (non-fatal).
    #[cfg(unix)]
    if crate::session_token::scoped_hooks_enabled() {
        match crate::cell_uds::bind_cell_socket(&session_id_for_response) {
            Ok(listener) => {
                // Sandbox B2 / P4-H6: per-session tier gating. A microVM-backed
                // cell additionally allows EXACTLY its per-session peer uid (the
                // VMM is the host-socket peer after priv-drop to that uid); a
                // bare-PTY cell does not → the allowed peer-uid set stays
                // `{daemon uid}`, default-OFF parity. `per_session_uid` is `Some`
                // iff this is a microVM cell the door allocated for.
                //
                // BLOCKER 2: the in-jail libkrun unix-proxy does the host-side
                // connect() AS the VMM's per-session uid (no guest→host idmap),
                // so a daemon-owned 0600 socket is EACCES → bytes silently
                // dropped. For a microVM cell, chown the socket inode to EXACTLY
                // that per-session uid (mode left 0600 → reachable by that uid +
                // root, never world). Fail-closed: if the uid is absent (no
                // allocation), leave it daemon-only + log. A bare-PTY cell never
                // enters this branch → socket stays 0600 daemon-only.
                if let Some(cell_uid) = per_session_uid {
                    if let Err(e) = crate::cell_uds::set_cell_socket_owner(
                        &session_id_for_response,
                        cell_uid,
                    ) {
                        log_debug!(
                            "[hook-scoped] WARN chown cell sock to per-session uid {cell_uid} failed for session={}: {e}; socket stays daemon-only",
                            session_id_for_response
                        );
                    } else {
                        log_debug!(
                            "[hook-scoped] chowned cell sock to per-session uid {cell_uid} for microVM session={}",
                            session_id_for_response
                        );
                    }
                }
                // The peer-cred belt allows EXACTLY this cell's per-session uid
                // (None for a bare-PTY cell → `{daemon uid}` only).
                crate::cell_server::serve_cell(session_id_for_response, listener, per_session_uid);
                log_debug!(
                    "[hook-scoped] bound + serving per-cell UDS for session={}",
                    session_id_for_response
                );
            }
            Err(e) => log_debug!(
                "[hook-scoped] WARN per-cell UDS bind failed for session={}: {e}",
                session_id_for_response
            ),
        }
    }

    v2_session_map::register(req.agent_name.clone(), session.clone());

    // Stamp the agent_sessions row's `active_terminal_id` (migration
    // 0037). Best-effort: tab-keyed spawns (`tab-<id>`) won't have
    // a matching workspace_sessions row and the UPDATE no-ops;
    // workspace agent spawns do, and the column lets the next chat
    // tab mount re-attach without walking the in-memory
    // v2_session_map. Mirror of the heartbeat smart-launch stamp
    // (`heartbeat_launch.rs`) and the
    // `agent_heartbeats.active_terminal_id` cleanup hook in
    // `v2_session_map::unregister`.
    //
    // **0.37.5 keying:** the canonical `req.agent_name` is now bare
    // `<project_id>` (UUID). We accept three shapes for back-compat
    // during the cross-version transition window:
    //   1. Bare UUID: the project_id directly. (post-0.37.5 native)
    //   2. `<project_id>:<bare>`: legacy renderer form. Split, take
    //      the prefix.
    //   3. Anything else (tab-XXX, ad-hoc): resolve from cwd.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let project_id_opt = if is_uuid_shape(&req.agent_name) {
            // Native bare-pid canonical key.
            Some(req.agent_name.clone())
        } else if let Some((pid, _bare)) = req.agent_name.split_once(':') {
            // Legacy `<pid>:<agent>` shape from a 0.37.4 renderer.
            if pid.is_empty() {
                k2_core::workspace::agent_identity::resolve_project_id(&conn, &req.cwd)
            } else {
                Some(pid.to_string())
            }
        } else {
            k2_core::workspace::agent_identity::resolve_project_id(&conn, &req.cwd)
        };
        if let Some(project_id) = project_id_opt {
            // Symmetric session-id-keyed stamping — mirrors the
            // heartbeat update below. The pinned tab and any
            // heartbeats may share the same `<workspace>:<agent>`
            // claude session; whoever resumes that session gets
            // their `active_terminal_id` updated to the new PTY.
            // Walking `req.args` catches both `--resume <id>` and
            // `--session-id <id>`. Without keying on the actual
            // resume target, ad-hoc tab spawns (Cmd+T, AI editor)
            // would clobber the pinned tab's stamp on every spawn.
            let args = &session.args;
            let mut resumed_session: Option<&str> = None;
            let mut i = 0;
            while i + 1 < args.len() {
                if (args[i] == "--resume" || args[i] == "--session-id")
                    && !args[i + 1].is_empty()
                {
                    resumed_session = Some(args[i + 1].as_str());
                    break;
                }
                i += 1;
            }
            if let Some(claude_sid) = resumed_session {
                let new_tid = session.session_id.to_string();

                // Pinned-tab pointer: workspace_sessions row whose
                // saved claude session_id matches what this PTY is
                // resuming. Tab spawns that aren't resuming the
                // pinned tab's session no-op.
                let _ = conn.execute(
                    "UPDATE workspace_sessions SET active_terminal_id = ?1 \
                     WHERE project_id = ?2 AND session_id = ?3",
                    rusqlite::params![&new_tid, &project_id, claude_sid],
                );

                // Heartbeat pointer: any heartbeat in this workspace
                // whose `last_session_id` matches what this PTY is
                // resuming. Multiple heartbeats can target the same
                // claude session (and the workspace's pinned chat),
                // so they all get stamped together.
                let _ = conn.execute(
                    "UPDATE workspace_heartbeats SET active_terminal_id = ?1 \
                     WHERE project_id = ?2 AND last_session_id = ?3",
                    rusqlite::params![&new_tid, &project_id, claude_sid],
                );
            }
        }
    }

    // Child-exit observer: subscribe to the session's alacritty event
    // broadcast and call v2_session_map::unregister when ChildExit
    // arrives. The unregister hook (in v2_session_map) is what nulls
    // any matching agent_heartbeats.active_terminal_id and flips
    // surfaced=0 on the agent_sessions row. Without this, claude
    // --print sessions exit cleanly and leave the column pointing at
    // a corpse — which the lazy cleanup on read would catch
    // eventually, but eventually-consistent stale data is the kind
    // of "feels haunted" UX we'd rather avoid. See
    // `heartbeat-active-session-tracking` PRD.
    spawn_child_exit_observer(
        req.agent_name.clone(),
        session.clone(),
        req.ephemeral_cwd.clone(),
        req.principal_key.clone(),
        // P4-H6: the per-session uid this cell holds. The observer is the SINGLE
        // authoritative teardown — it frees the uid + removes the per-session
        // egress table on ChildExit (fires on clean exit AND crash/OOM/kill-9),
        // so neither the uid nor the nft rule can ever leak. `None` for every
        // non-microVM session → no-op.
        per_session_uid,
    );

    // Drain any pending-live signals that were queued while this
    // agent was offline so they become input to the fresh session.
    // Mirrors `crate::spawn::spawn_agent_session`'s legacy drain so
    // wake-queued signals to v2 agents aren't silently lost on boot.
    //
    // Two-phase write per signal — body, settle, `\r` — same pattern
    // `DaemonInjectProvider::inject` and `heartbeat_launch::run_inject`
    // use. A single combined write would be treated as a multi-line
    // paste by the TUI input widget and the queued message would land
    // typed-but-not-sent.
    //
    // 0.37.0: drain under the spawn's key only. The 0.36.14 dual-key
    // (prefixed + bare-name fallback) drain is retired now that every
    // awareness-bus enqueue carries workspace context via signal.to;
    // both ends of the queue use the same `<project_id>:<bare>` key.
    let pending = pending_live::drain_for_agent(&req.agent_name);
    let pending_drained = pending.len();
    for signal in pending {
        let bytes = signal_format::inject_bytes(&signal);
        session.write(bytes.into_bytes());
        std::thread::sleep(std::time::Duration::from_millis(150));
        session.write(b"\r".to_vec());
    }

    let total_ms = __t_total.elapsed().as_secs_f64() * 1000.0;
    log_debug!(
        "[v2-perf] side=daemon SPAWN_SUMMARY session={} agent={} reused=false total_ms={:.3} lookup_ms={:.3} dpty_spawn_ms={:.3} pending_drained={}",
        session_id_for_response,
        req.agent_name,
        total_ms,
        lookup_ms,
        dpty_spawn_ms,
        pending_drained
    );

    let mut out = serde_json::json!({
        "sessionId": session_id_for_response.to_string(),
        "agentName": req.agent_name,
        "cols": req.cols,
        "rows": req.rows,
        "reused": false,
    });
    // Echo the resolved backend NAME only when the caller asked for a sandbox
    // (accept-and-mark). Absent ⇒ no field ⇒ response byte-identical to
    // pre-seam. The UI must render this literal name — it is NOT a bool, so
    // `passthrough` can't be mistaken for real isolation.
    if let Some(name) = sandbox_echo {
        out["sandbox"] = serde_json::Value::String(name.to_string());
    }
    HandlerResult {
        status: "200 OK",
        body: out.to_string(),
    }
}

/// Runtime check for the `K2_SANDBOX` opt-in flag. Only compiled on a Linux
/// build with the microVM backend feature — on every other build the sandbox is
/// unavailable regardless of the env, so the check would be dead code.
#[cfg(all(target_os = "linux", feature = "sandbox-microvm"))]
fn k2_sandbox_enabled() -> bool {
    std::env::var("K2_SANDBOX")
        .map(|v| {
            let v = v.trim();
            !v.is_empty()
                && v != "0"
                && !v.eq_ignore_ascii_case("false")
                && !v.eq_ignore_ascii_case("off")
        })
        .unwrap_or(false)
}

/// Sandbox selection (P2a). Maps a requested-bool to a [`SandboxSpec`].
///
/// The microVM jail only materializes on a Linux build compiled with
/// `sandbox-microvm` AND with the `K2_SANDBOX` env flag enabled at runtime. On
/// EVERY other build/platform — including this macOS build — a request degrades
/// to [`SandboxSpec::Passthrough`] (NO isolation) and logs the downgrade.
///
/// We degrade LOUD, never SILENT, and never reject `--sandbox`: the response
/// echoes the resolved backend name truthfully ("passthrough" when degraded),
/// so the absence of isolation is visible rather than a fail. We never resolve
/// to `Microvm` on a build that can't deliver it.
fn resolve_sandbox(requested: bool) -> k2_core::terminal::SandboxSpec {
    use k2_core::terminal::SandboxSpec;

    if !requested {
        return SandboxSpec::Passthrough;
    }

    #[cfg(all(target_os = "linux", feature = "sandbox-microvm"))]
    {
        if k2_sandbox_enabled() {
            return SandboxSpec::Microvm;
        }
    }

    log_debug!(
        "[sandbox] requested but unavailable on this build/platform → passthrough, no isolation"
    );
    SandboxSpec::Passthrough
}

/// Pure decision for whether `/cli/sessions/v2/close` is allowed to
/// tear a session down.
///
/// GH#22: a remote (or local) client attached over the grid-WS holds
/// a live `ViewerRegistration`, so `subscriber_count > 0` means
/// "someone is watching this session right now". (2026-07-02 Bug 2:
/// the count is the explicit viewer registry, NOT events-channel
/// receivers — internal observers like the child-exit task used to
/// inflate the receiver count so it never reached zero, which made
/// this guard refuse EVERY un-forced close.) The age-out reaper
/// drives `/cli/sessions/v2/close`; if we let it unregister an
/// attached session, the last-`Arc` drop SIGHUPs the child out from
/// under the watching client. So when subscribers are attached we
/// REFUSE — unless the caller explicitly passed `force` (the operator
/// / deliberate-teardown escape hatch).
///
/// Returns `true` when the close should proceed, `false` when it must
/// be refused as still-attached.
fn close_allowed(subscriber_count: usize, force: bool) -> bool {
    force || subscriber_count == 0
}

/// Handler for `POST /cli/sessions/v2/close`.
///
/// Request body: `{"agent_name": "tab-<terminalId>", "force": false}`.
/// (`force` is optional, defaults to `false`.)
/// Response: `{"closed": true|false[, "reason": "..."]}`.
///
/// Unregisters the session from `v2_session_map`. The last `Arc`
/// drop triggers `DaemonPtySession::drop`, which closes the PTY
/// master channel; alacritty's IO thread then exits, the child
/// receives SIGHUP, and the session is cleaned up.
///
/// **GH#22 reaper guard.** Before unregistering, we check the v2
/// session's live viewer count (the grid-WS `ViewerRegistration`
/// registry — see `DaemonPtySession::subscriber_count` for why it is
/// NOT the events channel's receiver count). If a client is still attached
/// (`subscriber_count > 0`) we DO NOT kill the session — we return
/// `{"closed": false, "reason": "session still has attached clients"}`
/// instead. This is defense-in-depth so NO reaper path (the renderer's
/// age-out reaper or any other caller) can reap a session a client is
/// watching, which is exactly what killed remote PTY sessions over
/// K2 Connect. Deliberate teardown can bypass the guard with
/// `"force": true`.
///
/// Called on deliberate tab removal (see A6 wiring in
/// `src/renderer/stores/tabs.ts::removeTab`) and by the age-out
/// reaper. Component unmount does NOT call this; the session survives
/// workspace swap + Tauri restart.
pub fn handle_v2_close(body: &[u8]) -> HandlerResult {
    #[derive(serde::Deserialize)]
    struct CloseRequest {
        agent_name: String,
        #[serde(default)]
        force: bool,
    }

    let req: CloseRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return HandlerResult {
                status: "400 Bad Request",
                body: format!(
                    r#"{{"error":"parse v2 CloseRequest: {}"}}"#,
                    e.to_string().replace('"', "'")
                ),
            }
        }
    };

    // GH#22 guard: refuse to reap a session a client is attached to.
    // Look up first (without unregistering) so we can read the live
    // subscriber count from the session's own broadcast channel.
    if let Some(session) = v2_session_map::lookup_by_agent_name(&req.agent_name) {
        let subscribers = session.subscriber_count();
        if !close_allowed(subscribers, req.force) {
            log_debug!(
                "[daemon/v2-close] REFUSED close for agent={} — {} attached subscriber(s), no force flag (GH#22 reaper guard)",
                req.agent_name,
                subscribers,
            );
            return HandlerResult {
                status: "200 OK",
                body: serde_json::json!({
                    "closed": false,
                    "reason": "session still has attached clients",
                    "subscriberCount": subscribers,
                })
                .to_string(),
            };
        }
    }

    let removed = v2_session_map::unregister(&req.agent_name).is_some();
    HandlerResult {
        status: "200 OK",
        body: serde_json::json!({ "closed": removed }).to_string(),
    }
}

/// Read the current `{cols, rows}` from a session's alacritty Term.
/// Used to populate the response for a reused session so the caller
/// knows the actual dimensions (which may differ from what they
/// requested if an earlier caller already sized the session).
fn current_dims(session: &DaemonPtySession) -> (u16, u16) {
    use k2_core::terminal::Dimensions;
    let term_mutex = session.term();
    let term = term_mutex.lock();
    let cols = term.columns() as u16;
    let rows = term.screen_lines() as u16;
    (cols, rows)
}

/// Subscribe to a freshly-spawned session's alacritty events on a
/// detached tokio task and call `v2_session_map::unregister(agent)`
/// when ChildExit arrives. The unregister hook is what handles the
/// DB cleanup — see `v2_session_map::unregister`. Detached because
/// we don't have a JoinHandle to track and the task is short-lived
/// (only runs until the child dies, which terminates the underlying
/// broadcast channel and ends our `recv()` loop).
///
/// Holds a Weak reference to the session so the observer task
/// doesn't keep the Arc alive past the last legitimate holder. If
/// every other holder drops first, `Weak::upgrade()` returns None
/// and we exit silently.
///
/// 2026-07-02 (PTY-leak Bug 2): the events receiver this task holds
/// for the session's whole life is an INTERNAL observer — it must
/// never count as an attached viewer. That's guaranteed structurally:
/// `subscriber_count()` counts `attach_viewer()` registrations, not
/// events-channel receivers, so this task (and any future internal
/// `subscribe_events()` caller) can't re-inflate the viewer signal.
pub fn spawn_child_exit_observer(
    agent_name: String,
    session: std::sync::Arc<DaemonPtySession>,
    ephemeral_cwd: Option<PathBuf>,
    principal_key: Option<String>,
    // P4-H6: the per-session worker uid this cell holds (`Some` only for a
    // microVM cell the door allocated for). Freed back to the pool + its
    // per-session egress table removed in the ChildExit arm — the SINGLE
    // authoritative teardown point (fires on clean exit AND crash/OOM/kill-9), so
    // the uid + nft rule can never leak. `None` for every non-microVM session.
    cell_uid: Option<u32>,
) {
    use k2_core::terminal::AlacEvent;
    // Capture the session id BEFORE downgrading so the teardown path can
    // revoke the scoped token + remove the per-cell socket even if every
    // strong ref is gone by the time ChildExit lands (#58 Phase 1).
    let session_id = session.session_id;
    // A2/B3b: capture the backend tier too, so the authoritative microVM
    // cgroup/NEWROOT cleanup can run on ChildExit even if every strong ref
    // is gone by then. Inert for the default Passthrough path. The variable
    // is only read inside the linux+feature-gated cleanup block below, so
    // it's unused on every other build — silence the warning there.
    #[cfg_attr(
        not(all(target_os = "linux", feature = "sandbox-microvm")),
        allow(unused_variables)
    )]
    let is_microvm = matches!(session.sandbox, k2_core::terminal::SandboxSpec::Microvm);
    let weak = std::sync::Arc::downgrade(&session);
    drop(session);
    tokio::spawn(async move {
        // Re-acquire briefly to grab a receiver. If the session was
        // already dropped, exit — nothing to observe.
        let mut rx = match weak.upgrade() {
            Some(s) => s.subscribe_events(),
            None => return,
        };
        // Drop the temporary strong reference so we don't keep the
        // Arc alive ourselves; the receiver alone is enough.
        loop {
            match rx.recv().await {
                Ok(AlacEvent::ChildExit(status)) => {
                    log_debug!(
                        "[daemon/v2-exit] ChildExit observed for agent={} code={:?} — unregistering",
                        agent_name,
                        status.code(),
                    );
                    // Flip the session's child_exited flag so any
                    // subsequent lookup_by_agent_name caller (the
                    // spawn-helper idempotency check, the agents
                    // running reaping pass) sees the dead state
                    // immediately. Without this, the small window
                    // between ChildExit and unregister could surface
                    // a stale Arc as "live" to a fast-following
                    // lookup. Fix is cheap; race is rare; bug class
                    // is high-impact.
                    if let Some(s) = weak.upgrade() {
                        s.mark_child_exited();
                    }
                    v2_session_map::unregister(&agent_name);

                    // P3b — drop any per-session STREAM token for this session so
                    // a torn-down API session's grid/bytes token stops
                    // authorizing immediately (in-memory no-op for the vast
                    // majority of sessions that never minted one).
                    crate::stream_token::revoke_for_session(&session_id);

                    // F2 (sandbox API) — evict this session's response log +
                    // ownership entry so the in-memory maps don't accrete a key
                    // per spawn for the daemon's lifetime. In-memory no-op for
                    // any session that never spawned a sandbox cell.
                    // `session_id` is a SessionId here; the F2 maps key on its
                    // canonical string form (the same `sessionId` string
                    // `record_owner` stored at create time — SessionId::Display).
                    crate::sandbox_responses::evict(&session_id.to_string());

                    // COMPAT-58 (#58 Phase 1) — flag-gated teardown. Revoke the
                    // cell's scoped token (epoch bump → next call 403, no
                    // restart) and remove its per-cell socket so the accept
                    // loop stops. Default OFF → skipped → zero behavior change
                    // (no hook-sessions.json write, no socket touch).
                    #[cfg(unix)]
                    if crate::session_token::scoped_hooks_enabled() {
                        crate::session_token::revoke_session(&session_id);
                        let _ = std::fs::remove_file(
                            crate::cell_uds::cell_socket_path(&session_id),
                        );
                        log_debug!(
                            "[hook-scoped] revoked scoped token + removed UDS for session={}",
                            session_id
                        );
                    }

                    // A2/B3b — authoritative microVM teardown. The daemon is
                    // the authority; the worker only cleans up best-effort as a
                    // backstop. `cgroup.kill` (cgroup v2, kernel ≥5.14) atomically
                    // kills any survivors = real crash-containment. Gated on
                    // `is_microvm` so normal (Passthrough) sessions never stat
                    // these paths on teardown (default-OFF parity). Best-effort /
                    // ENOENT-tolerant throughout.
                    #[cfg(all(target_os = "linux", feature = "sandbox-microvm"))]
                    if is_microvm {
                        // The worker names its cgroup leaf + NEWROOT with the
                        // bare session id (k2-vmm-worker.rs); match it EXACTLY.
                        let sid = session_id.to_string();
                        let cg = format!("/sys/fs/cgroup/k2cells/{sid}");
                        // Kill any survivors first (cgroup v2, kernel ≥5.14) =
                        // real crash-containment, then rmdir the leaf. NOTE:
                        // `remove_dir_all` does NOT work on a cgroup dir (it
                        // tries to unlink the virtual control files and fails);
                        // rmdir is the correct call, and it only succeeds once
                        // the cgroup is empty. After a crash (`kill -9`) the
                        // killed tasks vacate the cgroup ASYNCHRONOUSLY, so the
                        // first rmdir can race with EBUSY — retry briefly until
                        // it's empty (or already gone). Best-effort throughout.
                        let _ = std::fs::write(format!("{cg}/cgroup.kill"), "1");
                        for attempt in 0..20u32 {
                            match std::fs::remove_dir(&cg) {
                                Ok(_) => break,
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                                Err(_) => {
                                    // re-assert the kill in case a task was mid-fork
                                    let _ = std::fs::write(format!("{cg}/cgroup.kill"), "1");
                                    if attempt < 19 {
                                        tokio::time::sleep(
                                            std::time::Duration::from_millis(100),
                                        )
                                        .await;
                                    }
                                }
                            }
                        }
                        let _ = std::fs::remove_dir(format!("/run/k2cell-{sid}"));
                    }

                    // P3b — ephemeral workspace teardown. The `/v1/sandboxes`
                    // policy-resolver provisions a throwaway per-session cwd
                    // (`~/.k2/sandbox-sessions/<uuid>`) and stamps it here via
                    // `SpawnRequest::ephemeral_cwd`; remove it now the cell has
                    // exited so per-session disk doesn't accumulate. `None` for
                    // EVERY v2 caller (no stat, default-OFF parity); the path is
                    // daemon-minted (NEVER caller-supplied — `#[serde(skip)]`),
                    // so this can only ever delete a dir the daemon created.
                    // Best-effort / ENOENT-tolerant.
                    if let Some(dir) = ephemeral_cwd.as_ref() {
                        match std::fs::remove_dir_all(dir) {
                            Ok(_) => log_debug!(
                                "[v1-sandbox] removed ephemeral workspace {} for session={}",
                                dir.display(),
                                session_id
                            ),
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                            Err(e) => log_debug!(
                                "[v1-sandbox] WARN remove ephemeral workspace {} for session={} failed: {e}",
                                dir.display(),
                                session_id
                            ),
                        }
                    }

                    // P4-H4 — return this session's concurrent-cell QUOTA slot.
                    // This is the SINGLE authoritative teardown point: it fires
                    // on clean exit AND on crash/OOM/kill-9 (ChildExit always
                    // arrives), so a counted slot can never leak. `None` for
                    // EVERY non-API caller (no acquire happened) → no-op,
                    // default-OFF parity. Saturating in `sandbox_quota::release`.
                    if let Some(pk) = principal_key.as_ref() {
                        crate::sandbox_quota::release(pk);
                        log_debug!(
                            "[v1-sandbox] released concurrent-cell quota slot for principal={} session={}",
                            pk,
                            session_id
                        );
                    }

                    // P4-H6 — FREE the per-session worker uid back to the pool +
                    // REMOVE its per-session egress table. SAME authoritative
                    // point as the quota release / cgroup kill above: fires on
                    // clean exit AND on crash/OOM/kill-9, so the uid + nft rule
                    // can never leak, and the freed uid is reusable by the next
                    // spawn. `None` for every non-microVM session → no-op. Both
                    // are idempotent/saturating (the pool free is range-checked;
                    // the egress remove is `add table; delete table`), so a
                    // double-fire is harmless. Egress remove is best-effort —
                    // never block teardown on it.
                    if let Some(uid) = cell_uid {
                        let _ = crate::cell_egress::remove_egress_policy(uid);
                        crate::cell_uid_pool::free(uid);
                        log_debug!(
                            "[sandbox] P4-H6 freed per-session uid {uid} + removed egress table for session={}",
                            session_id
                        );
                    }
                    return;
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Each test gets its own agent_name so parallel test runs
    // don't stomp on each other's map entries.
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    fn uniq_agent_name() -> String {
        format!("test-v2-{}", NEXT_ID.fetch_add(1, Ordering::SeqCst))
    }

    #[test]
    fn spawn_request_rejects_empty_agent_name() {
        let body = br#"{"agent_name":""}"#;
        let result = handle_v2_spawn(body);
        assert_eq!(result.status, "400 Bad Request");
        assert!(result.body.contains("agent_name required"));
    }

    // ── Sandbox P1 selection ─────────────────────────────────────────────

    #[test]
    fn resolve_sandbox_is_always_passthrough() {
        use k2_core::terminal::SandboxSpec;
        // Accept-and-mark: requested or not, this build resolves to Passthrough.
        assert_eq!(resolve_sandbox(false), SandboxSpec::Passthrough);
        assert_eq!(resolve_sandbox(true), SandboxSpec::Passthrough);
        assert_eq!(resolve_sandbox(true).backend().name(), "passthrough");
    }

    // ── Sandbox P2a — fail-closed selection matrix ───────────────────────

    #[test]
    fn resolve_sandbox_never_microvm_on_this_build() {
        use k2_core::terminal::SandboxSpec;
        // P2a fail-safe: on this (macOS, feature-off) build a sandbox request
        // MUST degrade to Passthrough and MUST NEVER resolve to Microvm — we
        // never report isolation we can't deliver. The truthful echo is
        // therefore "passthrough", never "microvm".
        let resolved = resolve_sandbox(true);
        assert_ne!(
            resolved,
            SandboxSpec::Microvm,
            "macOS/feature-off must NEVER resolve a spawn to Microvm"
        );
        assert_eq!(resolved, SandboxSpec::Passthrough);
        assert_ne!(resolved.backend().name(), "microvm");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_with_sandbox_true_echoes_passthrough() {
        // `{"sandbox":true}` is accepted (never rejected) and the response
        // carries the literal backend NAME "passthrough". (async test: the
        // spawn path's child-exit observer uses `tokio::spawn`.)
        let _ = k2_core::db::init_for_tests();
        let agent = uniq_agent_name();
        let body = format!(
            r#"{{"agent_name":"{agent}","cwd":"/tmp","command":"sleep","args":["30"],"sandbox":true}}"#
        )
        .into_bytes();
        let result = handle_v2_spawn(&body);
        // Clean up the spawned session before asserting so a failure can't
        // leak the child/io-thread.
        let session = crate::v2_session_map::unregister(&agent);
        if let Some(ref s) = session {
            s.kill();
        }
        assert_eq!(result.status, "200 OK", "body={}", result.body);
        assert!(
            result.body.contains(r#""sandbox":"passthrough""#),
            "sandbox:true must echo the passthrough backend name; body={}",
            result.body
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_without_sandbox_omits_the_field() {
        // Default-path regression guard: when the request omits `sandbox`,
        // the response must NOT carry a `sandbox` field (byte-identical to
        // pre-seam behavior). (async test: child-exit observer uses tokio.)
        let _ = k2_core::db::init_for_tests();
        let agent = uniq_agent_name();
        let body = format!(
            r#"{{"agent_name":"{agent}","cwd":"/tmp","command":"sleep","args":["30"]}}"#
        )
        .into_bytes();
        let result = handle_v2_spawn(&body);
        let session = crate::v2_session_map::unregister(&agent);
        if let Some(ref s) = session {
            s.kill();
        }
        assert_eq!(result.status, "200 OK", "body={}", result.body);
        assert!(
            !result.body.contains("sandbox"),
            "absent sandbox request must produce no sandbox field; body={}",
            result.body
        );
    }

    #[test]
    fn spawn_request_rejects_malformed_json() {
        let body = b"not json at all";
        let result = handle_v2_spawn(body);
        assert_eq!(result.status, "400 Bad Request");
        assert!(result.body.contains("parse v2 SpawnRequest"));
    }

    #[test]
    fn close_noop_returns_closed_false() {
        let agent = uniq_agent_name();
        let body =
            format!(r#"{{"agent_name":"{}"}}"#, agent).into_bytes();
        let result = handle_v2_close(&body);
        assert_eq!(result.status, "200 OK");
        assert!(result.body.contains(r#""closed":false"#));
    }

    // GH#22 close-guard decision table (pure logic; no PTY needed).
    // The spawn-backed end-to-end variant (real session + real grid-WS
    // subscriber drives `subscriber_count`) lives in
    // crates/k2so-daemon/tests/reaper_close_guard_integration.rs.
    #[test]
    fn close_allowed_proceeds_when_no_subscribers() {
        // Nobody attached → safe to reap.
        assert!(close_allowed(0, false));
        assert!(close_allowed(0, true));
    }

    #[test]
    fn close_allowed_refuses_attached_without_force() {
        // A client is watching and no force flag → REFUSE.
        assert!(!close_allowed(1, false));
        assert!(!close_allowed(5, false));
    }

    #[test]
    fn close_allowed_force_bypasses_attached_guard() {
        // Deliberate teardown escape hatch overrides the guard.
        assert!(close_allowed(1, true));
        assert!(close_allowed(42, true));
    }

    // Full spawn-then-lookup + spawn-then-reuse tests live in
    // crates/k2so-daemon/tests/ where a running tokio runtime and
    // the ability to fork a shell are available. They gate A7's
    // parity work.
}
