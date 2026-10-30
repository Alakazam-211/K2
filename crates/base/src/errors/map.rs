//! Every refusal to its id: one exhaustive `match` per error type, so a new variant doesn't
//! compile until it has an id. Details are structured values, never prose; the message is
//! the error's own wording.

use super::ids as r;
use super::{ErrorEntry, ErrorReport, Refusal, Value};
use crate::blob::BlobError;
use crate::doc::DocError;
use crate::session::{
    ConflictKind, ConflictRegion, Declined, Need, PackError, SessionError, UndoOutcome,
    WriteOutcome,
};
use crate::store::StoreError;
use crate::text::diff::HunkError;
use crate::text::merge::Conflict;
use crate::tools::{ToolError, ToolRunError};

fn rep(entry: &'static ErrorEntry, message: impl Into<String>) -> ErrorReport {
    ErrorReport::new(entry, message)
}

/// A right's name in details (`need`).
pub(crate) fn need_word(n: Need) -> &'static str {
    match n {
        Need::Read => "read",
        Need::Edit => "edit",
        Need::History => "history",
        Need::Approver => "approver",
        Need::Owner => "owner",
        Need::AuthorOrOwner => "author_or_owner",
    }
}

impl Refusal for SessionError {
    fn report(&self) -> ErrorReport {
        use SessionError as S;
        let msg = self.to_string();
        match self {
            S::Forbidden { need } => rep(
                match need {
                    Need::Read => &r::FORBIDDEN_READ,
                    Need::Edit => &r::FORBIDDEN_EDIT,
                    Need::History => &r::FORBIDDEN_HISTORY,
                    Need::Approver => &r::FORBIDDEN_APPROVER,
                    Need::Owner => &r::FORBIDDEN_OWNER,
                    Need::AuthorOrOwner => &r::FORBIDDEN_AUTHOR_OR_OWNER,
                },
                msg,
            )
            .with("need", need_word(*need)),
            S::NotJoined { conn } => rep(&r::NOT_JOINED, msg).with("conn", conn.to_string()),
            S::Watching { max } => rep(&r::WATCHING, msg).with("max", *max as u64),
            S::Stale { expected, actual } => rep(&r::STALE, msg)
                .with("expected", *expected)
                .with("actual", *actual),
            S::RateLimited { retry_after_ms } => {
                rep(&r::RATE_LIMITED, msg).with("retry_after_ms", *retry_after_ms)
            }
            S::Declined(d) => declined(d, msg),
            S::NotFound(key) => rep(&r::NO_DOC, msg).with(
                "key",
                Value::obj([
                    ("space", Value::from(&key.space)),
                    ("path", Value::from(&key.path)),
                ]),
            ),
            S::PackMismatch { stored, pack } => rep(&r::PACK_MISMATCH, msg)
                .with("stored", stored)
                .with("pack", pack),
            S::BadParticipant(_) => rep(&r::BAD_PARTICIPANT, msg),
            S::BadPosition(_) => rep(&r::BAD_POSITION, msg),
            S::BadEdit(_) => rep(&r::BAD_EDIT, msg),
            S::NoApproval { seq } => rep(&r::NO_APPROVAL, msg).with("seq", *seq),
            S::BadApproval(_) => rep(&r::BAD_APPROVAL, msg),
            S::BadPath(_) => rep(&r::BAD_PATH, msg),
            S::Held { reason } => rep(&r::HELD_BACKLOG, msg).with("reason", *reason),
            S::RevertTooOld { seq } => rep(&r::REVERT_TOO_OLD, msg).with("seq", *seq),
            S::NoSuchChange(n) => rep(&r::NO_SUCH_CHANGE, msg).with("seq", *n),
            S::Disk(_) => rep(&r::DISK, msg),
            S::Leased {
                holder,
                name,
                expires_ms,
            } => rep(&r::LEASED, msg)
                .with("holder", holder)
                .with("name", name)
                .with("expires_ms", *expires_ms),
            S::NoLease => rep(&r::NO_LEASE, msg),
            S::Claimed {
                id,
                by,
                name,
                lines,
                remaining_ms,
                note,
            } => rep(&r::CLAIMED, msg)
                .with("claim", id.to_string())
                .with("by", by)
                .with("name", name)
                .with("lines", *lines)
                .with("remaining_ms", *remaining_ms)
                .with("note", note.clone()),
            S::NoClaim(id) => rep(&r::NO_CLAIM, msg).with("claim", id.map(|i| i.to_string())),
            S::ClaimContended {
                id,
                waiters,
                remaining_ms,
            } => rep(&r::CLAIM_CONTENDED, msg)
                .with("claim", id.to_string())
                .with("waiters", *waiters)
                .with("remaining_ms", *remaining_ms),
            S::BadClaim(m) => bad_claim(m, msg),
            S::Blob(e) => blob(e, msg),
            S::NoBlobStore => rep(&r::NO_BLOB_STORE, msg),
            S::Doc(e) => doc(e, msg),
            S::Store(e) => store(e, msg),
            S::Pack(_) => rep(&r::PACK_ERROR, msg),
            S::Unloaded => rep(&r::UNLOADED, msg),
            S::Released => rep(&r::RELEASED, msg),
            S::Deleted => rep(&r::DELETED, msg),
            S::Broken(_) => rep(&r::BROKEN, msg),
        }
    }
}

/// `BadClaim` carries prose; its three caps have their own ids (limits), told apart by the
/// fixed wording `claims.rs` gives them (the reachability test pins each).
fn bad_claim(m: &str, msg: String) -> ErrorReport {
    let count = || {
        m.split(|c: char| !c.is_ascii_digit())
            .find(|s| !s.is_empty())
            .and_then(|s| s.parse::<u64>().ok())
    };
    if m.starts_with("this doc has ") && m.ends_with("live claims (the most)") {
        rep(&r::CLAIMS_DOC_CAP, msg).with("count", count())
    } else if m.starts_with("you hold ") && m.ends_with("claims here (the most); release one") {
        rep(&r::CLAIMS_PARTICIPANT_CAP, msg).with("count", count())
    } else if m.contains(" already waiting for ") {
        rep(&r::CLAIM_WAITERS_CAP, msg).with("count", count())
    } else {
        rep(&r::BAD_CLAIM, msg)
    }
}

fn declined(d: &Declined, msg: String) -> ErrorReport {
    let entry = match d.code {
        "too_large" => &r::DECLINED_TOO_LARGE,
        "not_utf8" => &r::DECLINED_NOT_UTF8,
        "binary" => &r::DECLINED_BINARY,
        "mixed_line_endings" => &r::DECLINED_MIXED_EOL,
        "declined" => &r::DECLINED_OFF,
        "unreadable" => &r::DECLINED_UNREADABLE,
        "hard_linked" => &r::DECLINED_HARD_LINKED,
        _ => &r::DECLINED_PACK,
    };
    rep(entry, msg).with("code", d.code)
}

fn store(e: &StoreError, msg: String) -> ErrorReport {
    use StoreError as S;
    match e {
        S::Db(_) => rep(&r::STORE_FAILED, msg),
        S::Newer { found } => rep(&r::STORE_NEWER, msg)
            .with("found", *found)
            .with("max", crate::store::STORE_FORMAT),
        S::NewerData { table } => rep(&r::STORE_NEWER, msg).with("table", table.as_str()),
        S::Corrupt(what) => rep(&r::STORE_CORRUPT, msg).with("what", *what),
        S::NoDoc(d) => rep(&r::STORE_NO_DOC, msg).with("doc", *d),
        S::KeyTaken(k) => rep(&r::PATH_TAKEN, msg).with(
            "key",
            Value::obj([
                ("space", Value::from(&k.space)),
                ("path", Value::from(&k.path)),
            ]),
        ),
        S::NoVersion(d, v) => rep(&r::STORE_NO_VERSION, msg)
            .with("doc", *d)
            .with("version", *v),
        S::Locked => rep(&r::STORE_LOCKED, msg),
        S::NamedVersion(d, v) => rep(&r::NAMED_VERSION, msg)
            .with("doc", *d)
            .with("version", *v),
        S::KeyChange => rep(&r::KEY_CHANGE, msg),
        S::NoApproval(d, s) => rep(&r::STORE_NO_APPROVAL, msg)
            .with("doc", *d)
            .with("seq", *s),
        S::LogMoved(d, want, found) => rep(&r::LOG_MOVED, msg)
            .with("doc", *d)
            .with("expected", *want)
            .with("found", *found),
        S::NoBlobSpace => rep(&r::NO_BLOB_SPACE, msg),
        S::NoBlob(b) => rep(&r::NO_BLOB, msg).with("blob", b),
        S::CurrentVersion(d, v) => rep(&r::PURGE_CURRENT, msg)
            .with("doc", *d)
            .with("version", *v),
        S::NoRandom(_) => rep(&r::NO_RANDOM, msg),
        S::PurgedVersion(d, v) => rep(&r::PURGED_VERSION, msg)
            .with("doc", *d)
            .with("version", *v),
    }
}

fn blob(e: &BlobError, msg: String) -> ErrorReport {
    use BlobError as B;
    match e {
        B::TooLarge { limit } => rep(&r::BLOB_TOO_LARGE, msg).with("limit", *limit),
        B::NotFound(b) => rep(&r::BLOB_NOT_FOUND, msg).with("blob", b),
        B::Damaged(b) => rep(&r::BLOB_DAMAGED, msg).with("blob", b),
        B::Io(_) => rep(&r::BLOB_IO, msg),
        B::Store(s) => store(s, msg),
        B::NoRandom(_) => rep(&r::BLOB_NO_RANDOM, msg),
    }
}

fn doc(e: &DocError, msg: String) -> ErrorReport {
    use DocError as D;
    match e {
        D::Decode(_) => rep(&r::DECODE, msg),
        D::ForeignClient { allowed, found } => rep(&r::FOREIGN_CLIENT, msg)
            .with("allowed", *allowed)
            .with("found", *found),
        D::MissingDeps { client, from, have } => rep(&r::MISSING_DEPS, msg)
            .with("client", *client)
            .with("from", *from)
            .with("have", *have),
        D::Pending => rep(&r::PENDING_DEPS, msg),
        D::Apply(_) => rep(&r::APPLY_FAILED, msg),
        D::Panic => rep(&r::ENGINE_PANIC, msg),
        D::Load(_) => rep(&r::LOAD_FAILED, msg),
        D::Unaccepted => rep(&r::UNACCEPTED, msg),
        D::Quarantined => rep(&r::QUARANTINED, msg),
        D::BadClient(c) => rep(&r::BAD_CLIENT, msg).with("client", *c),
        D::NoRoot { schema, root } => rep(&r::NO_ROOT, msg)
            .with("schema", *schema)
            .with("root", root),
        D::WrongKind {
            root,
            wanted,
            actual,
        } => rep(&r::WRONG_KIND, msg)
            .with("root", root)
            .with("wanted", format!("{wanted:?}"))
            .with("actual", format!("{actual:?}")),
        D::Unsupported(k) => rep(&r::UNSUPPORTED_KIND, msg).with("kind", format!("{k:?}")),
        D::Hunk(h) => {
            let (why, index) = match h {
                HunkError::Order { index } => ("order", index),
                HunkError::Range { index } => ("range", index),
                HunkError::Empty { index } => ("empty", index),
            };
            rep(&r::BAD_HUNK, msg)
                .with("hunk", *index)
                .with("problem", why)
        }
    }
}

impl Refusal for DocError {
    fn report(&self) -> ErrorReport {
        doc(self, self.to_string())
    }
}

impl Refusal for StoreError {
    fn report(&self) -> ErrorReport {
        store(self, self.to_string())
    }
}

impl Refusal for BlobError {
    fn report(&self) -> ErrorReport {
        blob(self, self.to_string())
    }
}

impl Refusal for Declined {
    /// A file a pack declined (`DiskCheck::Declined`, or a host's own open): the message is
    /// the decline as it reads (`reason (code)`).
    fn report(&self) -> ErrorReport {
        declined(self, format!("live editing is off: {self}"))
    }
}

impl Refusal for PackError {
    fn report(&self) -> ErrorReport {
        rep(&r::PACK_ERROR, self.to_string())
    }
}

impl Refusal for ToolError {
    fn report(&self) -> ErrorReport {
        let msg = self.to_string();
        match self {
            ToolError::Unsupported { flag, .. } => {
                rep(&r::TOOL_UNSUPPORTED, msg).with("flag", flag)
            }
            ToolError::Refused { what, .. } => rep(&r::TOOL_REFUSED, msg).with("what", what),
            ToolError::Usage(_) => rep(&r::TOOL_USAGE, msg),
            ToolError::Limit(_) => rep(&r::TOOL_LIMIT, msg),
        }
    }
}

impl Refusal for ToolRunError {
    fn report(&self) -> ErrorReport {
        match self {
            ToolRunError::Tool(e) => e.report(),
            ToolRunError::Session(e) => e.report(),
        }
    }
}

impl ToolRunError {
    /// [`Refusal::report`]: the id, details and plain text a host prints.
    pub fn report(&self) -> ErrorReport {
        Refusal::report(self)
    }
}

impl SessionError {
    /// [`Refusal::report`]: the id, details and plain text a host prints.
    pub fn report(&self) -> ErrorReport {
        Refusal::report(self)
    }
}

impl ToolError {
    /// [`Refusal::report`].
    pub fn report(&self) -> ErrorReport {
        Refusal::report(self)
    }
}

/// `lines a–b` (or `line a`) for each region, joined with `, `.
fn where_lines(lines: impl Iterator<Item = (usize, usize)>) -> String {
    lines
        .map(crate::session::pair_text)
        .collect::<Vec<_>>()
        .join(", ")
}

fn kind_word(k: ConflictKind) -> &'static str {
    match k {
        ConflictKind::Overlap => "overlap",
        ConflictKind::Taint => "taint",
        ConflictKind::Erased => "erased",
    }
}

fn regions(cs: &[ConflictRegion]) -> Value {
    Value::List(
        cs.iter()
            .map(|c| {
                Value::obj([
                    ("root", Value::from(&c.root)),
                    ("lines", Value::from(c.lines)),
                    ("kind", Value::from(kind_word(c.kind))),
                    ("base", Value::from(&c.base)),
                    ("ours", Value::from(&c.ours)),
                    ("theirs", Value::from(&c.theirs)),
                ])
            })
            .collect(),
    )
}

impl UndoOutcome {
    /// The refusal this outcome is, if any (it exits 1, or it undid nothing for a reason the
    /// caller must know): a conflict, a blocked step, regions `--partial` skipped, history
    /// lost or cleared by a purge. `None` when it applied cleanly, dropped a step, or there
    /// was nothing to undo.
    pub fn refusal(&self) -> Option<ErrorReport> {
        Some(match self {
            UndoOutcome::Conflicts { step, conflicts } => {
                let w = where_lines(conflicts.iter().map(|c| c.lines));
                rep(
                    &r::UNDO_CONFLICT,
                    format!(
                        "step {step} conflicts in {} region(s) at {w}; nothing applied",
                        conflicts.len()
                    ),
                )
                .with("step", *step)
                .with("conflicts", regions(conflicts))
            }
            UndoOutcome::Blocked { conflicts } => {
                let w = where_lines(conflicts.iter().map(|c| c.lines));
                rep(
                    &r::UNDO_BLOCKED,
                    format!(
                        "every region of the top step conflicts ({} at {w}); nothing applied",
                        conflicts.len()
                    ),
                )
                .with("conflicts", regions(conflicts))
            }
            UndoOutcome::Applied {
                steps,
                result,
                skipped,
                blocked,
            } if !skipped.is_empty() || blocked.is_some() => {
                let all: Vec<&ConflictRegion> =
                    skipped.iter().chain(blocked.iter().flatten()).collect();
                let w = where_lines(all.iter().map(|c| c.lines));
                rep(
                    &r::UNDO_SKIPPED,
                    format!(
                        "undid {steps} step(s); {} region(s) were left alone because others \
                         changed them: {w}",
                        all.len()
                    ),
                )
                .with("steps", *steps)
                .with("version", result.version)
                .with("skipped", regions(skipped))
                .with("blocked", blocked.as_deref().map(regions))
            }
            UndoOutcome::HistoryLost => rep(
                &r::HISTORY_LOST,
                "nothing to undo: your undo history was lost when the doc reloaded",
            ),
            UndoOutcome::ClearedByPurge => rep(
                &r::CLEARED_BY_PURGE,
                "nothing to undo: a purge cleared the undo history",
            ),
            _ => return None,
        })
    }
}

fn write_conflicts(cs: &[Conflict]) -> Value {
    Value::List(
        cs.iter()
            .map(|c| {
                Value::obj([
                    ("lines", Value::from(c.lines)),
                    ("base", Value::from(&c.base_text)),
                    ("ours", Value::from(&c.ours)),
                    ("theirs", Value::from(&c.theirs)),
                ])
            })
            .collect(),
    )
}

impl WriteOutcome {
    /// The refusal this outcome is (`write --base` from version `base`): a conflict (exit 1)
    /// or an expired base (exit 8). `None` when it applied. Conflict lines are the base's.
    pub fn refusal(&self, base: crate::store::Seq) -> Option<ErrorReport> {
        Some(match self {
            WriteOutcome::Applied(_) => return None,
            WriteOutcome::Conflicts(cs) => {
                let w = where_lines(cs.iter().map(|c| c.lines));
                rep(
                    &r::WRITE_CONFLICT,
                    format!(
                        "your write overlaps changes made since version {base}: {} region(s) \
                         at {w}; nothing applied",
                        cs.len()
                    ),
                )
                .with("base", base)
                .with("conflicts", write_conflicts(cs))
            }
            WriteOutcome::BaseExpired => rep(
                &r::BASE_EXPIRED,
                format!("base version {base} is unknown or no longer cached; nothing applied"),
            )
            .with("base", base),
        })
    }
}
