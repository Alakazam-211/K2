//! The App-area refusal registry (prd-tauri-core-0467-v1 TC-18, TC-19;
//! vs-live TA-19): one row per refusal code an App route can answer, so an
//! agent turns any refusal into a fix without trial and error.
//!
//! - **Ids never change.** A row's `id` is the snake_case code the route
//!   already sends (`error.code`). A code K2 stops sending stays as
//!   `retired` (older daemons still send it). A renamed code keeps its old
//!   id as an alias ([`Refusal::aliases`]).
//! - **One table.** `k2 explain <code>` prints a row; `k2 explain` lists the
//!   ids by area; `k2 study app-errors` is generated from it; the CLI adds
//!   `next` and `explain` to an App refusal's JSON error object and a
//!   `fix:` line to the plain form, from the same rows. All three read a
//!   block `contract-gen` writes into `cli/k2` ([`cli_block`]), so they work
//!   with no daemon.
//! - **Exit codes are the CLI's.** [`cli_exit`] mirrors `_compute_fail` in
//!   `cli/k2` (400 and 413 → 2, everything else → 3); a row whose surface
//!   prints another code says so ([`Refusal::exit`]) and a test pins both.
//! - **Complete.** k2-daemon's `refusal_registry` test scans the App route
//!   modules (`apps/`, `app_pairing/`, `app_push/`) for every code they can
//!   send (`no("…"`, `refusal(…, "…"`, `"code": "…"`, `codes::X`) and fails
//!   on one with no row here.
//! - The kit's own ids (`MPC-Ennn`, `MPCE-Ennn`) are the kit's registry:
//!   `k2 explain` hands them to `k2 doc explain` once Markdown tier 0 is
//!   on main (kit-HOST-ERRORS), so there is one `explain` door.

mod rows;

use std::collections::BTreeMap;

use serde_json::{json, Value};

/// Who can fix it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    /// The caller (an agent, a script): change the call, run a command.
    Agent,
    /// Only the server's owner.
    Owner,
    /// The owner or an Admin.
    Admin,
    /// The person behind a pass (sign in again, answer, confirm).
    Person,
    /// K2 itself: wait, retry, update, or report it.
    K2,
}

impl Who {
    pub fn as_str(self) -> &'static str {
        match self {
            Who::Agent => "agent",
            Who::Owner => "owner",
            Who::Admin => "admin",
            Who::Person => "person",
            Who::K2 => "k2",
        }
    }
}

/// Whether K2 still sends the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Live,
    /// Older daemons send it; this one doesn't.
    Retired,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Live => "live",
            Status::Retired => "retired",
        }
    }
}

/// One refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal {
    /// The code the route sends (`error.code`), never renamed.
    pub id: &'static str,
    /// Where it comes from: `deploy`, `runtime`, `access`, `door`, `data`,
    /// `egress`, `box`, `domains`, `approvals`, `handover`, `push`,
    /// `pairing`, `links`, `updates`, `static`, `general`.
    pub area: &'static str,
    /// The HTTP status it usually comes with.
    pub http: u16,
    /// The exit code the CLI prints for it.
    pub exit: u8,
    /// What happened, in one or two plain sentences.
    pub what: &'static str,
    /// The fix: a command or the step to take.
    pub fix: &'static str,
    /// Who can fix it.
    pub who: Who,
    /// `k2 permission request` can ask for it (the error ends with
    /// `Ask with: …` while permission cards are on).
    pub askable: bool,
    pub status: Status,
    /// Old ids that mean this row (a renamed code keeps working).
    pub aliases: &'static [&'static str],
}

/// The CLI's exit code for an App route's HTTP status (`_compute_fail`).
pub const fn cli_exit(http: u16) -> u8 {
    match http {
        400 | 413 => 2,
        _ => 3,
    }
}

/// Rows the CLI prints another exit for than [`cli_exit`]`(http)`: a
/// deployment that ended `failed` exits 1 (`_app_rt_deployment_exit` in
/// `cli/k2`), and so does a build or deploy refused with 422.
pub const EXIT_OVERRIDES: &[(&str, u8)] = &[
    ("health_failed", 1),
    ("crashlooping", 1),
    // `k2 app build` and `k2 app deploy` print the report and exit 1 on a 422.
    ("build_failed", 1),
    ("lockfile_required", 1),
    ("seed_invalid", 1),
];

/// A live row with the CLI's exit for `http`.
pub(crate) const fn r(id: &'static str, area: &'static str, http: u16, who: Who, what: &'static str, fix: &'static str) -> Refusal {
    Refusal { id, area, http, exit: cli_exit(http), what, fix, who, askable: false, status: Status::Live, aliases: &[] }
}

impl Refusal {
    /// `k2 permission request` can ask for it.
    pub(crate) const fn ask(mut self) -> Self {
        self.askable = true;
        self
    }
    /// K2 no longer sends it.
    pub(crate) const fn retired(mut self) -> Self {
        self.status = Status::Retired;
        self
    }
    /// The CLI prints another exit for it (a deployment result's 1, a
    /// pending 2): the test checks each one against its surface.
    pub(crate) const fn exit(mut self, exit: u8) -> Self {
        self.exit = exit;
        self
    }
    pub(crate) const fn aliases(mut self, aliases: &'static [&'static str]) -> Self {
        self.aliases = aliases;
        self
    }

    /// `k2 explain <id>`.
    pub fn explain_cmd(&self) -> String {
        format!("k2 explain {}", self.id)
    }

    /// The row as JSON (`k2 explain <id> --json`, the CLI block).
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "area": self.area,
            "http": self.http,
            "exit": self.exit,
            "what": self.what,
            "fix": self.fix,
            "who": self.who.as_str(),
            "askable": self.askable,
            "status": self.status.as_str(),
            "aliases": self.aliases,
        })
    }
}

/// Every row, in table order.
pub fn all() -> &'static [Refusal] {
    rows::ROWS
}

/// The row for `id` (or one of its aliases), ignoring case and spaces.
pub fn lookup(id: &str) -> Option<&'static Refusal> {
    let want = id.trim().to_ascii_lowercase();
    all().iter().find(|r| r.id == want).or_else(|| all().iter().find(|r| r.aliases.contains(&want.as_str())))
}

/// Ids by area (`k2 explain` with no id).
pub fn by_area() -> BTreeMap<&'static str, Vec<&'static Refusal>> {
    let mut m: BTreeMap<&'static str, Vec<&'static Refusal>> = BTreeMap::new();
    for row in all() {
        m.entry(row.area).or_default().push(row);
    }
    m
}

/// The areas, in the order `k2 explain` lists them.
pub const AREAS: &[(&str, &str)] = &[
    ("deploy", "validate, build, deploy, rollback"),
    ("runtime", "the App engine, the box, health"),
    ("access", "who may do what: switches, owners, workspaces"),
    ("approvals", "held requests the owner answers"),
    ("door", "passes, App keys, quotas, suspend"),
    ("data", "k2 app data and exec"),
    ("egress", "where an App may connect out"),
    ("box", "box prep, box services, Podman"),
    ("domains", "labels and custom domains"),
    ("handover", "moving Apps between workspaces and boxes"),
    ("push", "notifications to an App's users"),
    ("pairing", "pairing a phone with an App"),
    ("links", "iOS and Android link files"),
    ("updates", "a desktop app's update feed"),
    ("static", "static Apps (a built folder, no process)"),
    ("general", "any App route"),
];

/// `k2 explain` with no id: ids by area.
pub fn list_text() -> String {
    let by = by_area();
    let mut s = String::from("Refusal ids by area. k2 explain <id> prints what happened and the fix.\n");
    for (area, about) in AREAS {
        let Some(rows) = by.get(area) else { continue };
        s.push_str(&format!("\n{area} ({about}):\n"));
        let ids: Vec<String> = rows.iter().map(|r| if r.status == Status::Retired { format!("{} (retired)", r.id) } else { r.id.to_string() }).collect();
        s.push_str(&wrap(&ids.join(", "), 76, "  "));
    }
    s
}

/// `k2 explain <id>`: one row in plain text.
pub fn row_text(r: &Refusal) -> String {
    let mut s = format!("{}  ({} · HTTP {} · exit {})\n", r.id, r.area, r.http, r.exit);
    if r.status == Status::Retired {
        s.push_str("retired: this K2 no longer sends it; older K2 servers may.\n");
    }
    s.push_str(&wrap(r.what, 76, ""));
    s.push_str(&wrap(&format!("fix: {}", r.fix), 76, "     ").replacen("     fix: ", "fix: ", 1));
    let who = match r.who {
        Who::Agent => "you (the caller)",
        Who::Owner => "the server's owner",
        Who::Admin => "the owner or an Admin",
        Who::Person => "the person signed in",
        Who::K2 => "K2 (wait, retry or update)",
    };
    s.push_str(&format!("who fixes it: {who}\n"));
    if r.askable {
        s.push_str("can be asked for: yes (the error ends with Ask with: k2 permission request …; run it as printed)\n");
    }
    if !r.aliases.is_empty() {
        s.push_str(&format!("also sent as: {}\n", r.aliases.join(", ")));
    }
    s
}

fn wrap(text: &str, width: usize, indent: &str) -> String {
    let mut out = String::new();
    let mut line = String::from(indent);
    for word in text.split_whitespace() {
        if line.len() > indent.len() && line.len() + 1 + word.len() > width {
            out.push_str(line.trim_end());
            out.push('\n');
            line = String::from(indent);
        }
        if line.len() > indent.len() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if line.len() > indent.len() {
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// `k2 study app-errors`: generated from the table (TC-18).
pub fn study_page() -> String {
    let mut s = String::new();
    for line in [
        "k2 study app-errors — every App refusal: what happened, the fix, who fixes it",
        "",
        "Every App route refuses with a stable id (error.code). The id never",
        "changes, so a script or an agent can match on it. Look one up:",
        "  k2 explain <id>          what happened, the fix, who fixes it",
        "  k2 explain <id> --json   the same as JSON",
        "  k2 explain               every id, by area",
        "An App command that is refused prints fix: <the fix> before K2's own",
        "Ask with: line, and its --json error object carries next (the fix) and",
        "explain (\"k2 explain <id>\"). Exit codes: 3 refused, 2 a bad request",
        "or still pending, 1 a deployment that failed, 4 an unknown id here.",
        "\"retired\" ids are ones older K2 servers send.",
    ] {
        s.push_str(line);
        s.push('\n');
    }
    let by = by_area();
    for (area, about) in AREAS {
        let Some(rows) = by.get(area) else { continue };
        s.push_str(&format!("\n{} ({about})\n", area.to_ascii_uppercase()));
        for row in rows {
            let tag = if row.status == Status::Retired { " [retired]" } else { "" };
            s.push_str(&format!("  {}{tag} (exit {})\n", row.id, row.exit));
            s.push_str(&wrap(&format!("fix: {}", row.fix), 76, "      "));
        }
    }
    s
}

/// The begin marker of the `cli/k2` block.
pub const CLI_BEGIN: &str = "# >>> generated by contract-gen: refusals";
/// The snapshot key of the block (`contract::gen::file_of`).
pub const CLI_KEY: &str = "cli/k2#refusals";

/// The `cli/k2` block: `_k2_refusals_json` (the rows, one JSON line) for
/// `k2 explain` and the refusal printer, and `_study_page_app_errors`.
pub fn cli_block(regen: &str, end: &str) -> String {
    let rows: Vec<Value> = all().iter().map(Refusal::to_json).collect();
    let areas: Vec<Value> = AREAS.iter().map(|(a, about)| json!({ "area": a, "about": about })).collect();
    let data = json!({ "schema": 1, "areas": areas, "rows": rows }).to_string();
    let page = study_page();
    assert!(!page.contains("\nEOF\n") && !data.contains("\nEOF\n"), "the block can't hold a heredoc terminator");
    format!(
        "{CLI_BEGIN} (do not edit; {regen})\n\
_k2_refusals_json() {{\n    cat <<'EOF'\n{data}\nEOF\n}}\n\
_study_page_app_errors() {{\n    cat <<'EOF'\n{page}EOF\n}}\n{end}\n"
    )
}

#[cfg(test)]
mod tests;
