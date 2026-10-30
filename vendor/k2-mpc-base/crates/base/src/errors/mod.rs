//! Stable error ids: one registry of every refusal the kit can return, and what a caller
//! with no context needs to recover from one (`docs/ERRORS.md`, generated from it).
//!
//! - **The registry** ([`KIT`], [`ENGINE`]): one [`ErrorEntry`] per refusal, with a stable
//!   id (`MPC-Ennn` for the kit, `MPCE-Ennn` reserved for the engine), a short name, the exit
//!   code (the kit's table, [`crate::tools::exit`]; an id never changes it), a one-line
//!   message template, what to do next, the rule it enforces, and the verbs that can return
//!   it. Ids are grouped by [`Area`]: the hundreds digit. **Ids are stable forever**: never
//!   reused, never removed; a retired id stays listed ([`Status::Retired`]). A test holds
//!   the registry append-only against a committed snapshot.
//! - **Every refusal carries its id**: [`Refusal::report`] on every error type
//!   ([`crate::session::SessionError`], [`crate::tools::ToolError`],
//!   [`crate::tools::ToolRunError`], the store's, the blob store's, the doc's, a pack's
//!   [`crate::session::Declined`]), and `refusal()` on the outcomes that refuse without an
//!   error (a `write --base` conflict, an undo conflict, a `patch` or exact `replace` that
//!   applied nothing: [`crate::tools::ToolOutput::refusal`]).
//! - **[`ErrorReport`]**: what a host prints. [`ErrorReport::plain`] is today's wording with
//!   the id appended, `message (MPC-E301)`; [`ErrorReport::to_json`] is the stable object
//!   `{"id", "exit", "message", "next", "rule", "details"}`, `details` structured (never
//!   prose).
//! - **`explain` and `capabilities`** ([`run_meta`]): verbs that print an entry, or what this
//!   build can do (verbs, flags, parity rows, the ids each verb can return, the limits in
//!   effect, the kit version), as text or JSON. Both are generated from the registries.
//!
//! The kit knows nothing of the host's own lines (a permission card's `Ask with: …`): a host
//! prints those as it always did, beside the id, never instead of it.

mod json;
mod map;
mod meta;
mod redact;
pub(crate) mod registry;

pub use json::Value;
pub use meta::{
    capabilities, errors_markdown, explain, explain_json, run_meta, MetaOutput, META_VERBS,
};
pub use redact::redact;
pub use registry::{ENGINE, KIT};

pub(crate) use registry as ids;

/// The kit's id prefix: `MPC-E` and three digits.
pub const KIT_PREFIX: &str = "MPC-E";
/// The engine's id prefix (reserved; the engine allocates its ranges): `MPCE-E` and three
/// digits. Engine codes cross the seam as these strings, never as enum ordinals.
pub const ENGINE_PREFIX: &str = "MPCE-E";

/// The area an id belongs to: its hundreds digit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Area {
    /// 0xx: exports (a host's export of a doc's version, [`crate::export`]).
    Export,
    /// 1xx: joining, rights, edits, `write --base`, approvals, the text tools, the sync wire.
    Session,
    /// 2xx: undo, redo, revert, versions.
    History,
    /// 3xx: claims.
    Presence,
    /// 4xx: the file on disk, declined files, outside edits.
    Disk,
    /// 5xx: the doc store and a session's lifecycle.
    Store,
    /// 6xx: purge.
    Erasure,
    /// 7xx: tier 0 (any file) and the blob store.
    Blob,
    /// 8xx: structured docs and CSV grids.
    Structured,
    /// 9xx: rate limits, caps and bounds.
    Limits,
}

impl Area {
    pub const ALL: &'static [Area] = &[
        Area::Export,
        Area::Session,
        Area::History,
        Area::Presence,
        Area::Disk,
        Area::Store,
        Area::Erasure,
        Area::Blob,
        Area::Structured,
        Area::Limits,
    ];

    /// The hundreds digit of its ids.
    pub const fn digit(self) -> u8 {
        match self {
            Area::Export => 0,
            Area::Session => 1,
            Area::History => 2,
            Area::Presence => 3,
            Area::Disk => 4,
            Area::Store => 5,
            Area::Erasure => 6,
            Area::Blob => 7,
            Area::Structured => 8,
            Area::Limits => 9,
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Area::Export => "exports",
            Area::Session => "sessions and edits",
            Area::History => "history and undo",
            Area::Presence => "presence and claims",
            Area::Disk => "disk and outside edits",
            Area::Store => "store",
            Area::Erasure => "erasure",
            Area::Blob => "tier 0 and blobs",
            Area::Structured => "structured and CSV",
            Area::Limits => "limits",
        }
    }
}

/// Whether an id is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Returned by this build.
    Live,
    /// Allocated for work that hasn't landed (named here); nothing returns it yet.
    Reserved(&'static str),
    /// No longer returned since this kit version. Never reused; still explained.
    Retired(&'static str),
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Live => "live",
            Status::Reserved(_) => "reserved",
            Status::Retired(_) => "retired",
        }
    }
}

/// How the reachability test reaches an id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Through the public API, as a host's verb would: the test produces it.
    Verb,
    /// Only by a fault, the sync wire or a pack (the reason): the test maps a constructed
    /// error to it instead.
    Indirect(&'static str),
}

/// One refusal: the registry's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorEntry {
    /// `MPC-Ennn` (or `MPCE-Ennn`). Stable forever.
    pub id: &'static str,
    /// A short snake_case name, unique.
    pub name: &'static str,
    pub area: Area,
    /// The exit code (the kit's table; an id never changes it).
    pub exit: u8,
    /// One line, with `{key}` for the report's details.
    pub message: &'static str,
    /// What to do next.
    pub next: &'static str,
    /// The rule it enforces: requirement ids (`K-…`), undo spec rules (`U…`), conformance
    /// rules (`hist.…`), amendments (`A13`: `docs/SESSIONS.md`).
    pub rule: &'static str,
    /// The verbs that can return it (registry names), or a group: `*` (any verb on a doc),
    /// `@change` (every verb that changes the doc), `@history` (every history verb),
    /// `@run_tool` (every verb `run_tool` runs), `@pack` (a pack's own verbs), `(sync)` (an
    /// editor's sync connection, no verb), `(host)` (a host call, no verb).
    pub verbs: &'static [&'static str],
    pub status: Status,
    pub reach: Reach,
}

impl ErrorEntry {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        id: &'static str,
        name: &'static str,
        area: Area,
        exit: u8,
        rule: &'static str,
        verbs: &'static [&'static str],
        message: &'static str,
        next: &'static str,
    ) -> Self {
        ErrorEntry {
            id,
            name,
            area,
            exit,
            message,
            next,
            rule,
            verbs,
            status: Status::Live,
            reach: Reach::Verb,
        }
    }

    pub(crate) const fn indirect(self, why: &'static str) -> Self {
        ErrorEntry {
            reach: Reach::Indirect(why),
            ..self
        }
    }

    pub(crate) const fn reserved(self, by: &'static str) -> Self {
        ErrorEntry {
            status: Status::Reserved(by),
            reach: Reach::Indirect("reserved"),
            ..self
        }
    }

    /// Whether it is the engine's (`MPCE-`).
    pub fn is_engine(&self) -> bool {
        self.id.starts_with(ENGINE_PREFIX)
    }
}

/// Every entry: the kit's, then the engine's.
pub fn entries() -> impl Iterator<Item = &'static ErrorEntry> {
    KIT.iter().chain(ENGINE.iter()).copied()
}

/// The entry for `id`: `MPC-E301`, `mpc-e301`, `E301` or `301` (the last two: the kit's);
/// `MPCE-E…` for the engine's.
pub fn lookup(id: &str) -> Option<&'static ErrorEntry> {
    let up = id.trim().to_ascii_uppercase();
    let full = if up.starts_with("MPC") {
        up
    } else if let Some(n) = up.strip_prefix('E') {
        format!("{KIT_PREFIX}{n}")
    } else {
        format!("{KIT_PREFIX}{up}")
    };
    entries().find(|e| e.id == full)
}

/// What a host prints for a refusal: an entry, the error's own message (today's wording),
/// and structured details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorReport {
    entry: &'static ErrorEntry,
    message: String,
    details: Vec<(&'static str, Value)>,
    /// The host's own `next` texts, by surface (K6): [`ErrorReport::with_next`].
    next_for: Vec<(String, String)>,
}

impl ErrorReport {
    /// A report of `entry` with this message.
    pub fn new(entry: &'static ErrorEntry, message: impl Into<String>) -> Self {
        ErrorReport {
            entry,
            message: message.into(),
            details: Vec::new(),
            next_for: Vec::new(),
        }
    }

    /// **The host's guidance for one surface** (K6, v0.9.8): what [`ErrorReport::next_for`]
    /// and [`ErrorReport::to_json_for`] give on `surface` (a name the host chooses: `tab`,
    /// `app`...) instead of the registry's `next`, which is written for an agent with only the
    /// CLI. A second call for the same surface replaces the first. Nothing else changes: the
    /// id, the exit code, the message, the rule and the details are the registry's and the
    /// refusal's, on every surface ([`ErrorReport::next`] and [`ErrorReport::to_json`] stay
    /// the registry's).
    pub fn with_next(mut self, surface: impl Into<String>, text: impl Into<String>) -> Self {
        let surface = surface.into();
        self.next_for.retain(|(s, _)| *s != surface);
        self.next_for.push((surface, text.into()));
        self
    }

    /// The guidance on `surface`: the host's ([`ErrorReport::with_next`]), else the
    /// registry's.
    pub fn next_for(&self, surface: &str) -> &str {
        self.next_for
            .iter()
            .find(|(s, _)| s == surface)
            .map_or(self.entry.next, |(_, t)| t.as_str())
    }

    /// [`ErrorReport::to_json`] as shown on `surface`: the same object, `next` from
    /// [`ErrorReport::next_for`].
    pub fn to_json_for(&self, surface: &str) -> String {
        self.to_value_for(surface).to_json()
    }

    /// [`ErrorReport::to_value`] as shown on `surface`.
    pub fn to_value_for(&self, surface: &str) -> Value {
        self.value_with_next(self.next_for(surface))
    }

    /// **Paths made safe to show** (K7, v0.9.8): the message and every string in the
    /// details through [`redact`]: a path inside `root` becomes relative to it (`root`
    /// itself, `.`), any other absolute path `<outside>`. The kit already does this where
    /// it knows the root (a file session's disk errors, `open_file`); a host passes its
    /// workspace root for the rest. The id, exit code, rule and the details' shape don't
    /// change.
    pub fn redacted(mut self, root: &std::path::Path) -> Self {
        self.message = redact(&self.message, root);
        for (_, v) in &mut self.details {
            redact::value(v, root);
        }
        for (_, t) in &mut self.next_for {
            *t = redact(t, root);
        }
        self
    }

    /// Add a detail (structured: a number, a string value, a list, an object).
    pub fn with(mut self, key: &'static str, value: impl Into<Value>) -> Self {
        self.details.push((key, value.into()));
        self
    }

    /// A refusal the engine returned across the seam, by its stable code (`MPCE-E…`). A code
    /// this build's catalog doesn't list keeps its code in `details.code` under
    /// `engine_unlisted`.
    pub fn engine(code: &str, message: impl Into<String>) -> Self {
        match ENGINE.iter().find(|e| e.id == code) {
            Some(e) => ErrorReport::new(e, message),
            None => ErrorReport::new(&registry::ENGINE_UNLISTED, message).with("code", code),
        }
    }

    pub fn entry(&self) -> &'static ErrorEntry {
        self.entry
    }
    pub fn id(&self) -> &'static str {
        self.entry.id
    }
    pub fn exit(&self) -> u8 {
        self.entry.exit
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn next(&self) -> &'static str {
        self.entry.next
    }
    pub fn rule(&self) -> &'static str {
        self.entry.rule
    }
    pub fn details(&self) -> &[(&'static str, Value)] {
        &self.details
    }
    /// A detail by key.
    pub fn detail(&self, key: &str) -> Option<&Value> {
        self.details.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    /// Plain text: the message as it always read, then the id, `message (MPC-E301)`.
    pub fn plain(&self) -> String {
        format!("{} ({})", self.message, self.entry.id)
    }

    /// The stable JSON object, one line: `{"id", "exit", "message", "next", "rule",
    /// "details"}`.
    pub fn to_json(&self) -> String {
        self.to_value().to_json()
    }

    pub fn to_value(&self) -> Value {
        self.value_with_next(self.entry.next)
    }

    fn value_with_next(&self, next: &str) -> Value {
        Value::obj([
            ("id", Value::from(self.entry.id)),
            ("exit", Value::from(self.entry.exit)),
            ("message", Value::from(self.message.as_str())),
            ("next", Value::from(next)),
            ("rule", Value::from(self.entry.rule)),
            (
                "details",
                Value::Obj(
                    self.details
                        .iter()
                        .map(|(k, v)| ((*k).to_owned(), v.clone()))
                        .collect(),
                ),
            ),
        ])
    }
}

impl std::fmt::Display for ErrorReport {
    /// [`ErrorReport::plain`].
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.plain())
    }
}

/// A refusal's id and details: every error type the kit returns implements it.
pub trait Refusal {
    fn report(&self) -> ErrorReport;
}

#[cfg(test)]
mod tests;
