//! Filesystem inbox for `Delivery::Inbox` signals.
//!
//! E2 of Phase 3. Pi-Messenger-style atomic-rename inbox at
//! `<inbox_root>/<agent>/<ns>-<uuid>.json`. Durable, ordered by
//! write-time, immune to concurrent-write collisions.
//!
//! **Inbox semantics — sender's deliberate choice.** A sender
//! writes to the inbox when they want intentional async delivery
//! (email semantics): the target reads on their own schedule, never
//! interrupted. The bus never writes to inbox on behalf of a `Live`
//! sender — that path wakes + injects. See the Phase 3 plan at
//! `~/.claude/plans/happy-hatching-locket.md` for the egress matrix.
//!
//! **Filename format:** `<ns_since_epoch>-<signal_uuid>.json`. The
//! ns timestamp is fixed-width (19 digits for any time past 2001-09-09,
//! 20 digits after 2286-11-20) so lexical sort = temporal sort for
//! the lifetime of humanity. The uuid suffix prevents collisions
//! between two signals that land on the same nanosecond.
//!
//! **Atomicity:** writes use `fs_atomic::atomic_write` — sibling
//! tempfile + fsync + rename. A `drain()` concurrent with a `write()`
//! either sees the previous state or the new file fully written;
//! never a half-JSON.
//!
//! **Drain semantics:** read, parse, delete. If parse fails (corrupt
//! file from some other source writing to our directory), log and
//! skip; leave the file in place so a human can eyeball it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::awareness::AgentSignal;
use crate::fs_atomic::atomic_write;
use crate::log_debug;

/// Write `signal` into the inbox for `target_agent`. Caller resolves
/// the target name (e.g. from `signal.to`) and the inbox root
/// (typically `<project>/.k2so/awareness/inbox/` or
/// `~/.k2so/awareness/inbox/` for workspace-less daemons).
///
/// Returns the path the file landed at — useful for tests and for
/// the `DeliveryReport` returned by `egress::deliver`.
///
/// The target agent name is taken as-is. Callers must validate the
/// name against their own allowlist — this function does no such
/// check and would happily create a directory named `../../evil`
/// if given that input. Current caller (E4 egress) passes names
/// pulled from `agent_sessions` DB which are already validated.
pub fn write(
    inbox_root: &Path,
    target_agent: &str,
    signal: &AgentSignal,
) -> io::Result<PathBuf> {
    let agent_dir = inbox_root.join(sanitize_agent_name(target_agent));
    let filename = build_filename(signal);
    let path = agent_dir.join(filename);
    let json = serde_json::to_vec_pretty(signal).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("serialize signal: {e}"),
        )
    })?;
    atomic_write(&path, &json)?;
    Ok(path)
}

/// Drain every pending signal for `agent` from the inbox. Returns
/// signals in temporal order (oldest first). Each file is deleted
/// after a successful parse; files that fail to parse are left in
/// place with a debug log for human triage.
///
/// `inbox_root` is the root (not the agent's subdirectory).
/// Missing subdirectory → empty Vec (not an error). This matches
/// the "agents poll every heartbeat, no-op when nothing's there"
/// usage pattern.
pub fn drain(inbox_root: &Path, agent: &str) -> Vec<AgentSignal> {
    let agent_dir = inbox_root.join(sanitize_agent_name(agent));
    let entries = match fs::read_dir(&agent_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            log_debug!("[awareness/inbox] read_dir {:?} failed: {}", agent_dir, e);
            return Vec::new();
        }
    };

    // Collect + sort lexically = temporal order (filename prefix is ns ts).
    let mut files: Vec<PathBuf> = entries
        .filter_map(|res| res.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".json"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();

    let mut out = Vec::with_capacity(files.len());
    for path in files {
        match parse_signal(&path) {
            Ok(signal) => {
                if let Err(e) = fs::remove_file(&path) {
                    log_debug!(
                        "[awareness/inbox] post-parse remove {:?} failed: {}",
                        path,
                        e
                    );
                    // Still deliver — the signal parsed, so it IS a
                    // delivery even if cleanup couldn't finish.
                }
                out.push(signal);
            }
            Err(e) => {
                log_debug!(
                    "[awareness/inbox] parse {:?} failed: {} — leaving in place",
                    path,
                    e
                );
            }
        }
    }
    out
}

/// Peek at pending signal count without consuming them. Useful for
/// dashboards / telemetry / the `k2so connections list` output. Same
/// directory-missing-is-empty semantics as `drain`.
pub fn pending_count(inbox_root: &Path, agent: &str) -> usize {
    let agent_dir = inbox_root.join(sanitize_agent_name(agent));
    match fs::read_dir(&agent_dir) {
        Ok(entries) => entries
            .filter_map(|r| r.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.ends_with(".json"))
                    .unwrap_or(false)
            })
            .count(),
        Err(_) => 0,
    }
}

/// Generate the filename for a signal: `<ns>-<uuid>.json`.
/// Nanosecond precision prefix sorts lex=temporal. UUID suffix
/// de-collides two writes on the same nanosecond.
fn build_filename(signal: &AgentSignal) -> String {
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{ns:020}-{}.json", signal.id)
}

/// Refuse path-traversal inputs. An agent name with `/`, `\`,
/// `..`, or control chars gets reduced to a safe token. This is
/// defense-in-depth — callers should also validate against their
/// own allowlist (e.g. `agent_sessions.agent_name` from DB), but
/// we don't trust our caller blindly.
fn sanitize_agent_name(name: &str) -> String {
    if name.is_empty() || name == "." || name == ".." {
        return "_invalid".to_string();
    }
    let clean: String = name
        .chars()
        .map(|c| match c {
            c if c.is_control() => '_',
            '/' | '\\' | ':' | '\0' => '_',
            _ => c,
        })
        .collect();
    if clean.starts_with('.') {
        format!("_{}", clean)
    } else {
        clean
    }
}

fn parse_signal(path: &Path) -> io::Result<AgentSignal> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("parse {path:?}: {e}"),
        )
    })
}
