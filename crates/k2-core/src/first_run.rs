//! The setup checklist (prd-setup-checklist-v1; the daemon–app contract is
//! `setup-checklist-contract.md`, which wins on names and shapes).
//!
//! A short list of the setup a person still has to do. **Every row's done
//! state comes from real daemon state, read when asked** ([`Facts`]); the
//! only things stored are per person: "hidden", the rows they skipped and
//! "finished" (`first_run_person`, migration 0290).
//!
//! **Rows, in order** (`meet_k5` waits on K5, so it is never emitted yet):
//!
//! | id | done when | shown to |
//! |---|---|---|
//! | `dev_tools` | git runs (macOS: only while Apple's Command Line Tools are missing, or once an install started here finished; Linux: only while git is missing) | macOS: the owner token; Linux: owner token, Owner, Admin |
//! | `llm_sign_in` | a claude, codex or grok login is `signed_in`, or is an API key | owner token, Owner, Admin |
//! | `first_agent` | a workspace exists besides the audit sentinels (and K5's) | everyone |
//! | `chief_of_staff` | the seat names a live Chief of Staff | owner token, Owner, Admin; **in place of `first_agent` while `k5FirstRun` is on** |
//! | `tunnel` | subdomain set, token held (file or vault), enabled ("running" is not asked) | owner token, Owner login; never in air-gap mode |
//! | `phone` | this person's login has a live paired phone (as `/cli/auth/devices` counts) | everyone |
//! | `own_sign_in` | never (the row goes once the owner links a login) | the owner token, while no login is linked |
//!
//! **Person.** `People::person_key`: the owner token and the logins the
//! owner linked are one person, `owner`, so they share hiding, skips and
//! finished; their phones are every linked login's. Any other login is its
//! own person.
//!
//! **Finished.** When every row shown to the caller is done or skipped, the
//! daemon writes `finished_at` once; it stays until `show`, even if a row
//! comes undone later (that surface handles it, not this list). `show`
//! clears hidden, finished and skips, so a skipped row is to do again.
//!
//! **Existing installs (K2 Q-A).** Migration 0290 sets `existing_cutoff`
//! only on a server already in use. A person with no row then counts as
//! finished when they are `owner` or a login made before the cutoff; a
//! fresh install has no cutoff, so everyone starts unfinished. Making a row
//! for such a person (hide, skip) keeps them finished.
//!
//! **K5 at first run** (prd-setup-checklist-v1 §10, `k5-first-run-contract.md`),
//! all behind the owner's `k5FirstRun` switch, off by default. On, the
//! status also carries `offer` ("meet_k5" while a tool is signed in, there is
//! no live K5, the caller is the owner token, an Owner or an Admin, and this
//! person hasn't said "Not now"), `k5` (the live K5 or null) and
//! `k5Defaults`; and `chief_of_staff` takes `first_agent`'s place. It opens
//! `k5_thread_cos` once K5 exists, else `k5_sheet` with `note: needs_k5`.
//! The seat is `crate::orchestrator`. Off, none of it is sent.
//!
//! **Developer tools** (`dev_tools`, K2 0.46.12). Workspaces need git. On a
//! Mac without Apple's Command Line Tools the row is first, so Apple's
//! long download can run while the person signs in; it opens `dev_tools`
//! (the app POSTs `/cli/git-tools/install`, `xcode-select --install`, which
//! opens Apple's installer on the daemon's own screen, so only the owner
//! token sees it) and carries `note: "installing"` while that installer is
//! up. Once git runs the row reads done, then goes at the next daemon start.
//! On Linux without git it shows the distro's install line as its note and
//! opens nothing (`opens: null`); a person with a shell runs it. Otherwise
//! (git runs, a git that is there but fails, Windows) there is no row.
//!
//! The evaluators are pure ([`rows`], [`status_on`], [`dev_tools_from`]);
//! [`load_facts`] reads the existing readers (`llm_accounts::list`,
//! `projects_list_on`, `tunnel::config`, `connect_users::device_sessions`,
//! `owner_account_prompt::view`, `git_safe::availability`).

use std::collections::BTreeSet;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::connect_users::Role;
use crate::notify_prefs::{People, OWNER};

/// Every row id, in the order shown. v1 emits only [`V1_ROWS`].
/// `k2_cli` ("Try the k2 command") was dropped 11-10; a skip of it stored
/// before then changes nothing.
/// `meet_k5` became the offer (`offer: "meet_k5"`), never a row.
pub const ROW_IDS: &[&str] =
    &["dev_tools", "llm_sign_in", "meet_k5", "first_agent", "chief_of_staff", "tunnel", "phone", "own_sign_in"];

/// The rows this version computes with `k5FirstRun` off, when git runs.
/// While it doesn't, `dev_tools` comes before them ([`DevTools`]).
pub const V1_ROWS: &[&str] = &["llm_sign_in", "first_agent", "tunnel", "phone", "own_sign_in"];

/// The rows with `k5FirstRun` on: `chief_of_staff` in `first_agent`'s place
/// (and `dev_tools` first while git doesn't run).
pub const K5_ROWS: &[&str] = &["llm_sign_in", "chief_of_staff", "tunnel", "phone", "own_sign_in"];

/// `phone.note` when the tunnel isn't on yet.
pub const NOTE_NEEDS_TUNNEL: &str = "needs_tunnel";

/// `chief_of_staff.note` while there is no K5 to hire it with.
pub const NOTE_NEEDS_K5: &str = "needs_k5";

/// `dev_tools.note` while Apple's installer is open.
pub const NOTE_INSTALLING: &str = "installing";

/// The offer the LLM sign-in sheet leads to.
pub const OFFER_MEET_K5: &str = "meet_k5";

/// One row's state on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Done,
    Todo,
    Skipped,
}

impl State {
    fn complete(self) -> bool {
        matches!(self, State::Done | State::Skipped)
    }
}

/// One row as `GET /cli/first-run/status` sends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub id: &'static str,
    pub state: State,
    /// What the row opens: `dev_tools` | `llm_sign_in` | `add_workspace` |
    /// `tunnel` | `phone` | `owner_account_prompt` | `k5_sheet` |
    /// `k5_thread_cos`. Null for a row with nothing to open (Linux's
    /// `dev_tools`, whose note is the command to run).
    pub opens: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

/// The whole answer (`GET /cli/first-run/status` and every POST).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    /// The server switch (`setupChecklist`), off by default.
    pub enabled: bool,
    pub hidden: bool,
    pub finished: bool,
    pub rows: Vec<Row>,
    /// `offer`, `k5` and `k5Defaults`, present only while `k5FirstRun` is on.
    #[serde(flatten)]
    pub k5: Option<K5Status>,
    /// This call wrote `finished_at` (the route sends `first_run_changed`).
    #[serde(skip)]
    pub newly_finished: bool,
}

/// The K5 part of the status (`k5FirstRun` on).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct K5Status {
    /// `"meet_k5"` or null.
    pub offer: Option<&'static str>,
    /// The live K5, or null.
    pub k5: Option<K5View>,
    pub k5_defaults: K5Defaults,
}

/// A live K5 on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct K5View {
    pub workspace_id: String,
    pub name: String,
    pub path: String,
}

/// What the K5 sheet starts from: "K5", `k5`, `<hire root>/k5`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct K5Defaults {
    pub name: String,
    pub handle: String,
    pub path: String,
}

/// Who is asking, for row visibility. The route resolves it from the
/// credential, never the body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Viewer {
    /// The owner daemon token (the desktop app and the CLI on the server).
    OwnerToken,
    /// A Connect login with this role.
    Login(Role),
}

impl Viewer {
    fn at_least(self, role: Role) -> bool {
        match self {
            Viewer::OwnerToken => true,
            Viewer::Login(r) => r >= role,
        }
    }
}

/// The existing state the rows are computed from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    /// A claude, codex or grok login is `signed_in`, or is an API key.
    pub llm_signed_in: bool,
    /// A workspace exists besides the audit sentinels.
    pub has_agent: bool,
    /// The tunnel is configured: subdomain, token held, enabled.
    pub tunnel_done: bool,
    /// Air-gap mode: the tunnel row is never shown.
    pub airgap: bool,
    /// The owner linked a login (`owner_account_prompt::view` is `Linked`).
    pub owner_linked: bool,
    /// Live paired phones for this person.
    pub phones: usize,
    /// The owner's `k5FirstRun` switch.
    pub k5_first_run: bool,
    /// The seat's live K5.
    pub k5: Option<crate::orchestrator::Live>,
    /// The seat names a live Chief of Staff.
    pub chief_of_staff: bool,
    /// Where a new K5 goes by default (`<hire root>/k5`).
    pub k5_default_path: String,
    /// Git and Apple's installer ([`dev_tools_from`]).
    pub dev_tools: DevTools,
}

/// What the `dev_tools` row knows: git, and Apple's installer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DevTools {
    /// No row: git runs (and no install started here), a git that is there
    /// fails (installing won't fix it; `k2 git-tools` says how), or an OS
    /// with no step (Windows).
    #[default]
    NoRow,
    /// macOS without Apple's Command Line Tools. `installing`: Apple's
    /// installer is open (`git_tools::install_state` says `installing`).
    AppleMissing { installing: bool },
    /// macOS: an install started here (this daemon run) and git runs now.
    AppleDone,
    /// Linux without git: the distro's install line.
    LinuxMissing { command: &'static str },
}

/// Pure: the `dev_tools` fact from git's probe and what the daemon knows.
/// `os` is `std::env::consts::OS`; `started` is
/// `git_tools::install_started_at`; `installer_running`, Apple's installer
/// window is up; `os_release`, `/etc/os-release` (Linux).
pub fn dev_tools_from(
    probe: &crate::git_safe::GitProbe,
    os: &str,
    started: Option<std::time::SystemTime>,
    now: std::time::SystemTime,
    installer_running: bool,
    os_release: &str,
) -> DevTools {
    use crate::git_safe::GitProbe;
    match (os, probe) {
        ("macos", GitProbe::Ready { .. }) if started.is_some() => DevTools::AppleDone,
        ("macos", GitProbe::NeedsAppleClt) => DevTools::AppleMissing {
            installing: crate::git_tools::install_state(probe, true, started, now, installer_running)
                == crate::git_tools::InstallState::Installing,
        },
        ("linux", GitProbe::Missing) => {
            DevTools::LinuxMissing { command: crate::git_tools::linux_install_command(os_release) }
        }
        _ => DevTools::NoRow,
    }
}

#[cfg(any(test, feature = "test-util"))]
thread_local! {
    static DEV_TOOLS_FOR_TEST: std::cell::Cell<Option<DevTools>> = const { std::cell::Cell::new(None) };
}

/// Test-only: [`load_facts`] on this thread reports `d` instead of probing
/// this machine's git, until the guard drops.
#[cfg(any(test, feature = "test-util"))]
pub fn dev_tools_for_test(d: DevTools) -> DevToolsTestGuard {
    let prev = DEV_TOOLS_FOR_TEST.with(|c| c.replace(Some(d)));
    DevToolsTestGuard { prev }
}

/// Restores what [`load_facts`] reports for `dev_tools` on drop.
#[cfg(any(test, feature = "test-util"))]
pub struct DevToolsTestGuard {
    prev: Option<DevTools>,
}

#[cfg(any(test, feature = "test-util"))]
impl Drop for DevToolsTestGuard {
    fn drop(&mut self) {
        DEV_TOOLS_FOR_TEST.with(|c| c.set(self.prev));
    }
}

/// The `dev_tools` fact on this machine now. Apple's installer is looked
/// for (`pgrep`) only while an install started here and git doesn't run.
pub fn dev_tools_now() -> DevTools {
    #[cfg(any(test, feature = "test-util"))]
    if let Some(d) = DEV_TOOLS_FOR_TEST.with(|c| c.get()) {
        return d;
    }
    let os = std::env::consts::OS;
    let probe = crate::git_safe::availability();
    let started = crate::git_tools::install_started_at();
    let installer_running = os == "macos"
        && started.is_some()
        && matches!(probe, crate::git_safe::GitProbe::NeedsAppleClt)
        && crate::git_tools::apple_installer_running();
    let os_release = if os == "linux" && matches!(probe, crate::git_safe::GitProbe::Missing) {
        std::fs::read_to_string("/etc/os-release").unwrap_or_default()
    } else {
        String::new()
    };
    dev_tools_from(&probe, os, started, std::time::SystemTime::now(), installer_running, &os_release)
}

/// A person's stored record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PersonRecord {
    pub hidden_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub skipped: BTreeSet<String>,
    /// "Not now" on the "Meet K5" offer.
    pub offer_dismissed_at: Option<i64>,
}

/// A change a person asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Hide,
    /// Clears hidden, finished and skips: every row not done is to do
    /// again (otherwise the next read would finish the list at once).
    Show,
    Skip(String),
    /// "Not now" on the "Meet K5" offer: stored for this person, kept by `show`.
    DismissOffer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `skip` named a row this caller doesn't see (or no row at all).
    UnknownRow(String),
    Store(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::UnknownRow(r) => write!(f, "{r:?} is not a row on your setup list"),
            Error::Store(e) => write!(f, "{e}"),
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Store(e.to_string())
    }
}

/// True when a tunnel config counts as set up: a subdomain, a token held
/// in the file or the vault (never read here), and not disabled. Whether
/// frpc is running is not asked: it can drop and recover by itself.
pub fn tunnel_configured(cfg: &crate::tunnel::TunnelConfig) -> bool {
    let token_held = !cfg.token.trim().is_empty()
        || cfg.token_home.as_deref() == Some(crate::tunnel::config::TOKEN_HOME_VAULT);
    !cfg.subdomain.trim().is_empty() && token_held && cfg.enabled
}

/// True when any live claude, codex or grok login is signed in or is an
/// API key.
pub fn llm_signed_in(entries: &[crate::llm_accounts::Entry]) -> bool {
    use crate::llm_accounts::Tool;
    entries.iter().any(|e| {
        matches!(e.tool(), Some(Tool::Claude | Tool::Codex | Tool::Grok))
            && (e.state == crate::llm_accounts::state::SIGNED_IN || e.is_api_key())
    })
}

/// The rows `viewer` sees, in order, with `skipped` applied.
pub fn rows(viewer: Viewer, facts: &Facts, skipped: &BTreeSet<String>) -> Vec<Row> {
    // A Voice & Chat only login never gets the list (its gate replaces
    // the whole app, and the route refuses it at the Member floor).
    if viewer == Viewer::Login(Role::VoiceChat) {
        return Vec::new();
    }
    let state = |id: &str, done: bool| {
        if done {
            State::Done
        } else if skipped.contains(id) {
            State::Skipped
        } else {
            State::Todo
        }
    };
    let mut out = Vec::new();
    // Developer tools first: Apple's download runs while the person signs
    // in. Its installer opens on the daemon's own screen, so on a Mac only
    // the owner token can use it; Linux's line is for anyone with a shell.
    match facts.dev_tools {
        DevTools::NoRow => {}
        DevTools::AppleMissing { installing } => {
            if viewer == Viewer::OwnerToken {
                let note = installing.then_some(NOTE_INSTALLING);
                out.push(Row { id: "dev_tools", state: state("dev_tools", false), opens: Some("dev_tools"), note });
            }
        }
        DevTools::AppleDone => {
            if viewer == Viewer::OwnerToken {
                out.push(Row { id: "dev_tools", state: State::Done, opens: Some("dev_tools"), note: None });
            }
        }
        DevTools::LinuxMissing { command } => {
            if viewer.at_least(Role::Admin) {
                out.push(Row { id: "dev_tools", state: state("dev_tools", false), opens: None, note: Some(command) });
            }
        }
    }
    if viewer.at_least(Role::Admin) {
        out.push(Row { id: "llm_sign_in", state: state("llm_sign_in", facts.llm_signed_in), opens: Some("llm_sign_in"), note: None });
    }
    if facts.k5_first_run {
        // K5 at first run: the Chief of Staff in `first_agent`'s place, for
        // the people who can make K5.
        if viewer.at_least(Role::Admin) {
            let (opens, note) =
                if facts.k5.is_some() { (Some("k5_thread_cos"), None) } else { (Some("k5_sheet"), Some(NOTE_NEEDS_K5)) };
            out.push(Row { id: "chief_of_staff", state: state("chief_of_staff", facts.chief_of_staff), opens, note });
        }
    } else {
        out.push(Row { id: "first_agent", state: state("first_agent", facts.has_agent), opens: Some("add_workspace"), note: None });
    }
    let tunnel_visible = viewer.at_least(Role::Owner) && !facts.airgap;
    let tunnel_state = state("tunnel", facts.tunnel_done);
    if tunnel_visible {
        out.push(Row { id: "tunnel", state: tunnel_state, opens: Some("tunnel"), note: None });
    }
    let phone_state = state("phone", facts.phones > 0);
    // Q-C: the phone row stays open before the tunnel is on, with a note,
    // and takes the person to the tunnel step first when they can do it.
    // Air-gap has no tunnel to point at; a done row needs no note.
    let (opens, note) = if facts.tunnel_done || facts.airgap || phone_state == State::Done {
        ("phone", None)
    } else if tunnel_visible && tunnel_state == State::Todo {
        ("tunnel", Some(NOTE_NEEDS_TUNNEL))
    } else {
        ("phone", Some(NOTE_NEEDS_TUNNEL))
    };
    out.push(Row { id: "phone", state: phone_state, opens: Some(opens), note });
    if viewer == Viewer::OwnerToken && !facts.owner_linked {
        out.push(Row { id: "own_sign_in", state: state("own_sign_in", false), opens: Some("owner_account_prompt"), note: None });
    }
    out
}

/// The existing-install cutoff (unix seconds), when this server had one.
pub fn cutoff(conn: &Connection) -> Result<Option<i64>, Error> {
    Ok(conn
        .query_row("SELECT existing_cutoff FROM first_run_meta WHERE id = 1", [], |r| r.get::<_, Option<i64>>(0))
        .optional()?
        .flatten())
}

/// Whether a person with no record counts as finished: there is a cutoff,
/// and they are the owner or a login made before it.
pub fn treated_finished(cutoff: Option<i64>, people: &People, person: &str) -> bool {
    let Some(cut) = cutoff else { return false };
    person == OWNER || people.user(person).is_some_and(|u| u.created_at.timestamp() < cut)
}

/// A person's record, `None` when they have none.
pub fn load_record(conn: &Connection, person: &str) -> Result<Option<PersonRecord>, Error> {
    let row = conn
        .query_row(
            "SELECT hidden_at, finished_at, skipped_json, offer_dismissed_at FROM first_run_person WHERE person_key = ?1",
            params![person],
            |r| {
                Ok((
                    r.get::<_, Option<i64>>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((hidden_at, finished_at, skipped_json, offer_dismissed_at)) = row else { return Ok(None) };
    let skipped: BTreeSet<String> = serde_json::from_str(&skipped_json)
        .map_err(|e| Error::Store(format!("first_run_person {person}: skipped_json: {e}")))?;
    Ok(Some(PersonRecord { hidden_at, finished_at, skipped, offer_dismissed_at }))
}

/// Make the person's record if it doesn't exist, finished already when the
/// cutoff says they are.
fn ensure_record(conn: &Connection, people: &People, person: &str, now: i64) -> Result<(), Error> {
    let finished_at = treated_finished(cutoff(conn)?, people, person).then_some(now);
    conn.execute(
        "INSERT OR IGNORE INTO first_run_person (person_key, hidden_at, finished_at, skipped_json) VALUES (?1, NULL, ?2, '[]')",
        params![person, finished_at],
    )?;
    Ok(())
}

/// The status for `person` as `viewer`, writing `finished_at` once when
/// every visible row is complete. Pure apart from the database.
pub fn status_on(
    conn: &Connection,
    people: &People,
    person: &str,
    viewer: Viewer,
    facts: &Facts,
    enabled: bool,
    now: i64,
) -> Result<Status, Error> {
    let record = load_record(conn, person)?;
    let (hidden, mut finished, skipped) = match &record {
        Some(r) => (r.hidden_at.is_some(), r.finished_at.is_some(), r.skipped.clone()),
        None => (false, treated_finished(cutoff(conn)?, people, person), BTreeSet::new()),
    };
    let dismissed = record.as_ref().is_some_and(|r| r.offer_dismissed_at.is_some());
    let k5 = k5_status(viewer, facts, dismissed);
    let rows = rows(viewer, facts, &skipped);
    let mut newly_finished = false;
    if !finished && !rows.is_empty() && rows.iter().all(|r| r.state.complete()) {
        ensure_record(conn, people, person, now)?;
        conn.execute(
            "UPDATE first_run_person SET finished_at = ?2 WHERE person_key = ?1 AND finished_at IS NULL",
            params![person, now],
        )?;
        finished = true;
        newly_finished = true;
    }
    Ok(Status { enabled, hidden, finished, rows, k5, newly_finished })
}

/// The `offer`, `k5` and `k5Defaults` part, `None` while `k5FirstRun` is off.
pub fn k5_status(viewer: Viewer, facts: &Facts, dismissed: bool) -> Option<K5Status> {
    if !facts.k5_first_run {
        return None;
    }
    let offer = (facts.llm_signed_in && facts.k5.is_none() && viewer.at_least(Role::Admin) && !dismissed)
        .then_some(OFFER_MEET_K5);
    Some(K5Status {
        offer,
        k5: facts.k5.as_ref().map(|l| K5View { workspace_id: l.workspace_id.clone(), name: l.name.clone(), path: l.path.clone() }),
        k5_defaults: K5Defaults {
            name: crate::orchestrator::DEFAULT_NAME.to_string(),
            handle: crate::orchestrator::DEFAULT_HANDLE.to_string(),
            path: facts.k5_default_path.clone(),
        },
    })
}

/// Apply `action` for `person`, then answer the new status. `skip` must
/// name a row `viewer` sees today.
#[allow(clippy::too_many_arguments)]
pub fn apply_on(
    conn: &Connection,
    people: &People,
    person: &str,
    viewer: Viewer,
    facts: &Facts,
    enabled: bool,
    action: &Action,
    now: i64,
) -> Result<Status, Error> {
    ensure_record(conn, people, person, now)?;
    match action {
        Action::Hide => {
            conn.execute(
                "UPDATE first_run_person SET hidden_at = ?2 WHERE person_key = ?1",
                params![person, now],
            )?;
        }
        Action::Show => {
            conn.execute(
                "UPDATE first_run_person SET hidden_at = NULL, finished_at = NULL, skipped_json = '[]' WHERE person_key = ?1",
                params![person],
            )?;
        }
        Action::Skip(row) => {
            let row = row.trim();
            let current = load_record(conn, person)?.unwrap_or_default();
            if !rows(viewer, facts, &current.skipped).iter().any(|r| r.id == row) {
                return Err(Error::UnknownRow(row.to_string()));
            }
            let mut skipped = current.skipped;
            skipped.insert(row.to_string());
            let json = serde_json::to_string(&skipped).map_err(|e| Error::Store(e.to_string()))?;
            conn.execute(
                "UPDATE first_run_person SET skipped_json = ?2 WHERE person_key = ?1",
                params![person, json],
            )?;
        }
        Action::DismissOffer => {
            conn.execute(
                "UPDATE first_run_person SET offer_dismissed_at = ?2 WHERE person_key = ?1",
                params![person, now],
            )?;
        }
    }
    status_on(conn, people, person, viewer, facts, enabled, now)
}

/// Live paired phones for `person`: the owner's are every linked login's
/// (none when nothing is linked); any other person's are their login's.
/// Counted the way `GET /cli/auth/devices` lists them.
pub fn phones_for(people: &People, person: &str) -> Result<usize, String> {
    let logins = if person == OWNER { people.owner_aliases() } else { vec![person.to_string()] };
    let mut n = 0;
    for login in logins {
        n += crate::connect_users::device_sessions(Some(&login))?.len();
    }
    Ok(n)
}

/// Read every row's existing state for `person` (the server-wide rows are
/// the same for everyone).
pub fn load_facts(conn: &Connection, people: &People, settings: &crate::app_settings::AppSettings, person: &str) -> Result<Facts, String> {
    let entries = crate::llm_accounts::list(conn).map_err(|e| e.to_string())?;
    let k5 = crate::orchestrator::live_k5(conn).map_err(|e| e.to_string())?;
    let chief_of_staff = crate::orchestrator::live_chief_of_staff(conn).map_err(|e| e.to_string())?.is_some();
    // K5 is K2's own agent, not the person's first: it doesn't count.
    let k5_id = k5.as_ref().map(|l| l.workspace_id.as_str());
    let has_agent = crate::projects_ops::projects_list_on(conn)?.iter().any(|p| Some(p.id.as_str()) != k5_id);
    let k5_default_path = k5_default_path(conn);
    let tunnel = crate::tunnel::config::load_without_token()?;
    let prompt = crate::owner_account_prompt::view(
        people,
        &settings.owner_account_prompt,
        None,
        None,
        env!("CARGO_PKG_VERSION"),
    );
    Ok(Facts {
        llm_signed_in: llm_signed_in(&entries),
        has_agent,
        tunnel_done: tunnel_configured(&tunnel),
        airgap: crate::airgap::enabled(),
        owner_linked: prompt.reason == crate::owner_account_prompt::Reason::Linked,
        phones: phones_for(people, person)?,
        k5_first_run: settings.k5_first_run,
        k5,
        chief_of_staff,
        k5_default_path,
        dev_tools: dev_tools_now(),
    })
}

/// `<hire root>/k5`, from the stored root (else `~/k2-agents`).
pub fn k5_default_path(conn: &Connection) -> String {
    let root = match crate::hire::root::stored_root(conn) {
        Some(r) => std::path::PathBuf::from(r),
        None => crate::paths::home_dir().unwrap_or_default().join(crate::hire::root::DEFAULT_ROOT_DIR),
    };
    root.join(crate::orchestrator::DEFAULT_HANDLE).to_string_lossy().into_owned()
}

/// The claude, codex and grok tools signed in (as [`llm_signed_in`] counts).
pub fn signed_in_tools(entries: &[crate::llm_accounts::Entry]) -> Vec<&'static str> {
    use crate::llm_accounts::Tool;
    let mut out = Vec::new();
    for t in [Tool::Claude, Tool::Codex, Tool::Grok] {
        if llm_signed_in(&entries.iter().filter(|e| e.tool() == Some(t)).cloned().collect::<Vec<_>>()) {
            out.push(t.as_str());
        }
    }
    out
}

#[cfg(test)]
#[path = "first_run_tests.rs"]
mod tests;
