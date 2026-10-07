//! IMAP listener template + boot-time reconcile for already-enabled
//! hosted mail.
//!
//! `hostmail enable`'s `server-config` step (0.40.147+) creates two
//! listeners in Stalwart's config store: `imap` on `[::]:143`
//! (STARTTLS) and `imaps` on `[::]:993` (implicit TLS). A server that
//! was enabled before that template existed never re-runs
//! `server-config`, and a Stalwart restart only reloads the stored set,
//! so those boxes have no IMAP.
//!
//! On daemon start, when hosted mail is already enabled, this module:
//! 1. reads Stalwart's stored listeners (read-only `x:NetworkListener/get`);
//! 2. for each IMAP template decides Present / Missing / Conflict — a
//!    match is protocol + bind + TLS mode, NOT the listener name or
//!    byte-equality (a hand-made registry create still counts);
//! 3. creates ONLY the missing ones (create-only `x:NetworkListener/set`
//!    — no update, no destroy; never `listeners_apply`, which retargets
//!    https and destroys pop3s/sieve);
//! 4. restarts Stalwart once, and only after a create, through the
//!    first door that is open ([`restart_once`]): the mail helper's
//!    `systemctl restart stalwart` verb, else a plain
//!    `sudo -n /usr/bin/systemctl restart stalwart` when sudoers already
//!    allows it (older boxes), else nothing — the created listeners stay
//!    stored and bind on the next Stalwart restart (logged with the root
//!    command).
//!
//! Steps 1–3 are management-API calls to the local Stalwart and need no
//! root, so the helper is never probed when there is nothing to create.
//! Stalwart has no API that binds new listeners without a process
//! restart (`ReloadSettings` does not move sockets — see `jmap.rs`
//! module doc, verified), so there is no reload door.
//!
//! A listener that exists but differs (same name, other bind/TLS mode,
//! or another listener on the same port) is never overwritten: it is
//! logged and left. Every failure is logged; daemon boot never blocks.
//!
//! The decision + store diff are pure and platform-independent; only
//! [`spawn_startup_reconcile`] is Linux-gated (`mail_supported`).

use super::jmap::StalwartClient;

/// One listener the daemon owns the definition of. `body()` is the
/// exact registry-create object `server-config` sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenerTemplate {
    pub name: &'static str,
    pub bind: &'static str,
    pub protocol: &'static str,
    pub tls_implicit: bool,
}

/// IMAP on :143 with STARTTLS.
pub const IMAP_STARTTLS: ListenerTemplate = ListenerTemplate {
    name: "imap",
    bind: "[::]:143",
    protocol: "imap",
    tls_implicit: false,
};

/// IMAPS on :993 with implicit TLS (Mail.app).
pub const IMAPS_IMPLICIT: ListenerTemplate = ListenerTemplate {
    name: "imaps",
    bind: "[::]:993",
    protocol: "imap",
    tls_implicit: true,
};

/// The pair the boot reconcile guarantees, in create order.
pub const IMAP_LISTENERS: [ListenerTemplate; 2] = [IMAP_STARTTLS, IMAPS_IMPLICIT];

impl ListenerTemplate {
    /// The registry-create body (shared by `server-config` and the boot
    /// reconcile — one definition).
    pub fn body(&self) -> serde_json::Value {
        let mut bind = serde_json::Map::new();
        bind.insert(self.bind.to_string(), serde_json::Value::Bool(true));
        serde_json::json!({
            "name": self.name,
            "bind": bind,
            "protocol": self.protocol,
            "useTls": true,
            "tlsImplicit": self.tls_implicit,
        })
    }

    pub fn port(&self) -> u16 {
        bind_port(self.bind).expect("template bind carries a port")
    }

    fn tls_label(&self) -> &'static str {
        if self.tls_implicit {
            "implicit TLS"
        } else {
            "STARTTLS"
        }
    }

    fn describe(&self) -> String {
        format!("{} ({} {})", self.name, self.bind, self.tls_label())
    }
}

/// Port of a `host:port` / `[v6]:port` bind string.
fn bind_port(bind: &str) -> Option<u16> {
    bind.trim().rsplit_once(':')?.1.parse().ok()
}

/// One stored listener as `x:NetworkListener/get` returns it. Fields
/// Stalwart omitted stay `None` (unknown, never guessed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerRow {
    pub id: String,
    pub name: String,
    pub binds: Vec<String>,
    pub protocol: Option<String>,
    pub use_tls: Option<bool>,
    pub tls_implicit: Option<bool>,
}

/// Pure `x:NetworkListener/get` parser with bind / protocol / TLS mode.
/// `bind` may be a `{addr: true}` map (registry shape), an array, or a
/// single string; map entries set to `false` are not bound.
pub fn parse_listener_rows(args: &serde_json::Value) -> Result<Vec<ListenerRow>, String> {
    let list = args
        .get("list")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "x:NetworkListener/get: reply has no list".to_string())?;
    list.iter()
        .map(|e| {
            let id = e
                .get("id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "x:NetworkListener/get: entry without an id".to_string())?;
            let name = e.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            let binds = match e.get("bind") {
                Some(serde_json::Value::Object(m)) => m
                    .iter()
                    .filter(|(_, v)| v.as_bool() != Some(false))
                    .map(|(k, _)| k.trim().to_string())
                    .collect(),
                Some(serde_json::Value::Array(a)) => a
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .collect(),
                Some(serde_json::Value::String(s)) => vec![s.trim().to_string()],
                _ => Vec::new(),
            };
            Ok(ListenerRow {
                id: id.to_string(),
                name: name.to_string(),
                binds,
                protocol: e
                    .get("protocol")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                use_tls: e.get("useTls").and_then(|v| v.as_bool()),
                tls_implicit: e.get("tlsImplicit").and_then(|v| v.as_bool()),
            })
        })
        .collect()
}

/// What the boot reconcile decided for one template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// A stored listener already serves it (protocol + bind + TLS mode).
    Present { id: String },
    /// Nothing serves it and nothing else claims its name or port.
    Missing,
    /// Something exists but differs — left alone, never overwritten.
    Conflict { reason: String },
}

/// Protocol + bind + TLS mode. A field Stalwart did not report is not a
/// contradiction (a "present" verdict only ever means "write nothing").
fn row_serves(row: &ListenerRow, t: &ListenerTemplate) -> bool {
    row.binds.iter().any(|b| b == t.bind)
        && row
            .protocol
            .as_deref()
            .is_none_or(|p| p.eq_ignore_ascii_case(t.protocol))
        && row.use_tls != Some(false)
        && row.tls_implicit.is_none_or(|v| v == t.tls_implicit)
}

fn row_summary(row: &ListenerRow) -> String {
    let tls = match (row.use_tls, row.tls_implicit) {
        (Some(false), _) => "no TLS",
        (_, Some(true)) => "implicit TLS",
        (_, Some(false)) => "STARTTLS",
        (_, None) => "TLS mode unreported",
    };
    format!(
        "'{}' (id {}, bind [{}], protocol {}, {tls})",
        row.name,
        row.id,
        row.binds.join(", "),
        row.protocol.as_deref().unwrap_or("unreported"),
    )
}

/// Pure decision for one template over the stored rows.
pub fn verdict_for(rows: &[ListenerRow], t: &ListenerTemplate) -> Verdict {
    if let Some(row) = rows.iter().find(|r| row_serves(r, t)) {
        return Verdict::Present { id: row.id.clone() };
    }
    if let Some(row) = rows.iter().find(|r| r.name == t.name) {
        return Verdict::Conflict {
            reason: format!(
                "listener {} differs from the template {} — left as is",
                row_summary(row),
                t.describe()
            ),
        };
    }
    let port = t.port();
    if let Some(row) = rows
        .iter()
        .find(|r| r.binds.iter().any(|b| bind_port(b) == Some(port)))
    {
        return Verdict::Conflict {
            reason: format!(
                "port {port} is already bound by listener {} — left as is",
                row_summary(row)
            ),
        };
    }
    Verdict::Missing
}

/// Pure plan over both IMAP templates.
pub fn plan(rows: &[ListenerRow]) -> Vec<(ListenerTemplate, Verdict)> {
    IMAP_LISTENERS
        .iter()
        .map(|t| (*t, verdict_for(rows, t)))
        .collect()
}

/// Pure create-only `x:NetworkListener/set` args: `create` keyed by the
/// template name, and nothing else (no `update`, no `destroy`).
pub fn create_only_set_args(templates: &[ListenerTemplate]) -> Result<serde_json::Value, String> {
    if templates.is_empty() {
        return Err("create_only_set_args: nothing to create".to_string());
    }
    let mut create = serde_json::Map::new();
    for t in templates {
        create.insert(t.name.to_string(), t.body());
    }
    Ok(serde_json::json!({ "create": create }))
}

/// The two store operations the reconcile needs (read, create-only).
pub trait ListenerStore {
    fn listener_rows(&self) -> Result<Vec<ListenerRow>, String>;
    fn create_listeners(&self, templates: &[ListenerTemplate]) -> Result<(), String>;
}

impl ListenerStore for StalwartClient {
    fn listener_rows(&self) -> Result<Vec<ListenerRow>, String> {
        StalwartClient::listener_rows(self)
    }
    fn create_listeners(&self, templates: &[ListenerTemplate]) -> Result<(), String> {
        self.listeners_create(templates)
    }
}

/// A door that restarted Stalwart after a create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPath {
    /// `sudo -n k2-mail-helper systemctl restart stalwart`.
    MailHelper,
    /// `sudo -n /usr/bin/systemctl restart stalwart` — boxes whose
    /// sudoers already lets the daemon user run systemctl (pre-helper
    /// fleet), or a root daemon.
    PlainSudo,
}

/// Absolute systemctl for the plain-sudo door (sudoers matches paths).
pub const SYSTEMCTL_PATH: &str = "/usr/bin/systemctl";

/// What happened to the Stalwart restart after a create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartReport {
    Restarted(RestartPath),
    /// No door open: the created listeners stay stored and bind on the
    /// next Stalwart restart. Not a failure, nothing is rolled back.
    NotRestarted { helper: super::helper::HelperState },
}

/// The restart doors, probed lazily (only after a create).
pub trait RestartDoors {
    /// `sudo -n -l <helper>` probe.
    fn helper_state(&mut self) -> super::helper::HelperState;
    /// `sudo -n -l /usr/bin/systemctl restart stalwart` probe (never
    /// prompts, never runs systemctl).
    fn plain_sudo_allowed(&mut self) -> bool;
    /// Restart through `path` and wait for the unit to be active.
    fn restart(&mut self, path: RestartPath) -> Result<(), String>;
}

/// Pick the first open door and restart once: helper -> plain sudo ->
/// none. The plain probe runs only when the helper is unusable.
pub fn restart_once(doors: &mut dyn RestartDoors) -> Result<RestartReport, String> {
    let helper = doors.helper_state();
    let path = if helper == super::helper::HelperState::Installed {
        RestartPath::MailHelper
    } else if doors.plain_sudo_allowed() {
        RestartPath::PlainSudo
    } else {
        return Ok(RestartReport::NotRestarted { helper });
    };
    doors.restart(path)?;
    Ok(RestartReport::Restarted(path))
}

/// Result of one reconcile pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Created these, then restarted Stalwart once (or could not — see
    /// `restart`).
    Added {
        added: Vec<ListenerTemplate>,
        skipped: Vec<String>,
        restart: RestartReport,
    },
    /// No write, no restart, no helper probe.
    NothingToDo { skipped: Vec<String> },
}

/// Read -> decide -> create the missing ones -> restart once (only after
/// a create). `port_answers(port)` reports a foreign process already
/// accepting on that port (Stalwart has no listener there, so anything
/// answering is not Stalwart) — such a template is skipped. The read and
/// create are management-API calls; `doors` (root) is touched only after
/// a create.
pub fn reconcile_with(
    store: &dyn ListenerStore,
    port_answers: &dyn Fn(u16) -> bool,
    doors: &mut dyn RestartDoors,
) -> Result<Outcome, String> {
    let rows = store.listener_rows()?;
    let mut add = Vec::new();
    let mut skipped = Vec::new();
    for (t, v) in plan(&rows) {
        match v {
            Verdict::Present { .. } => {}
            Verdict::Conflict { reason } => skipped.push(format!("{}: {reason}", t.name)),
            Verdict::Missing => {
                if port_answers(t.port()) {
                    skipped.push(format!(
                        "{}: port {} already answers on 127.0.0.1 but no Stalwart \
                         listener claims it — another process holds it; not added",
                        t.name,
                        t.port()
                    ));
                } else {
                    add.push(t);
                }
            }
        }
    }
    if add.is_empty() {
        return Ok(Outcome::NothingToDo { skipped });
    }
    store.create_listeners(&add)?;
    let names: Vec<String> = add.iter().map(|t| t.describe()).collect();
    let restart = restart_once(doors).map_err(|e| {
        format!(
            "added {} but the stalwart restart failed — the sockets bind on the next \
             restart: {e}",
            names.join(", ")
        )
    })?;
    Ok(Outcome::Added { added: add, skipped, restart })
}

/// One boot pass after the gates (status, admin API up, enable lock):
/// reconcile, then the log lines.
pub fn startup_pass(
    store: &dyn ListenerStore,
    port_answers: &dyn Fn(u16) -> bool,
    doors: &mut dyn RestartDoors,
) -> Vec<String> {
    log_lines(&reconcile_with(store, port_answers, doors))
}

const LOG_PREFIX: &str = "[mail/imap-listeners]";

/// One clear log line per outcome (plus one per skipped template).
pub fn log_lines(result: &Result<Outcome, String>) -> Vec<String> {
    let mut lines = Vec::new();
    let skipped = match result {
        Ok(Outcome::Added { added, skipped, restart }) => {
            let names = added
                .iter()
                .map(|t| t.describe())
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(match restart {
                RestartReport::Restarted(RestartPath::MailHelper) => {
                    format!("{LOG_PREFIX} added {names}; restarted stalwart")
                }
                RestartReport::Restarted(RestartPath::PlainSudo) => format!(
                    "{LOG_PREFIX} added {names}; restarted stalwart \
                     (sudo -n {SYSTEMCTL_PATH} restart stalwart)"
                ),
                RestartReport::NotRestarted { helper } => not_restarted_line(&names, *helper),
            });
            skipped
        }
        Ok(Outcome::NothingToDo { skipped }) => {
            if skipped.is_empty() {
                lines.push(format!(
                    "{LOG_PREFIX} nothing to do — imap :143 STARTTLS and imaps :993 \
                     implicit TLS are present"
                ));
            } else {
                lines.push(format!(
                    "{LOG_PREFIX} nothing written, no restart — every missing IMAP \
                     listener was skipped"
                ));
            }
            skipped
        }
        Err(e) => {
            lines.push(format!("{LOG_PREFIX} FAILED (daemon carries on): {e}"));
            return lines;
        }
    };
    for s in skipped {
        lines.push(format!("{LOG_PREFIX} skipped {s}"));
    }
    lines
}

/// Why the boot reconcile must not run, or `None` when it should. Pure.
pub fn startup_gate(status: Option<&str>, enable_completed: bool) -> Option<String> {
    let Some(status) = status else {
        return Some("hosted mail is not installed".to_string());
    };
    if !matches!(status, "running" | "degraded" | "stopped") {
        return Some(format!("hosted mail status is '{status}'"));
    }
    if !enable_completed {
        return Some("the enable has not completed (server-config owns the listeners)".into());
    }
    None
}

/// The `NOT restarted` line: the listeners are stored, nothing binds
/// until Stalwart restarts. The installer alone does not restart
/// Stalwart, so the line names both the restart and the helper fix.
pub fn not_restarted_line(names: &str, helper: super::helper::HelperState) -> String {
    format!(
        "{LOG_PREFIX} added {names}; NOT restarted — the new listeners bind on the next \
         Stalwart restart. To restart now, run as root: systemctl restart stalwart \
         (no restart door: mail helper {}, `sudo -n {SYSTEMCTL_PATH} restart stalwart` \
         not allowed). To let the daemon restart Stalwart itself, run as root: {}",
        helper.as_str(),
        super::helper::install_command()
    )
}

/// Boot hook (main.rs, next to the mail health loop). Linux only; one
/// detached thread; panics contained; never blocks boot.
pub fn spawn_startup_reconcile() {
    if !super::supervisor::mail_supported() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("mail-imap-listeners".into())
        .spawn(|| {
            let res = std::panic::catch_unwind(run_startup_reconcile_live);
            if res.is_err() {
                k2_core::log_debug!("{LOG_PREFIX} FAILED (daemon carries on): panicked");
            }
        });
    if let Err(e) = spawned {
        k2_core::log_debug!("{LOG_PREFIX} FAILED (daemon carries on): thread spawn: {e}");
    }
}

/// Seconds between admin-API probes while Stalwart comes up at boot.
const API_WAIT_STEP_SECS: u64 = 3;
/// Probes before giving up (~3 min).
const API_WAIT_TRIES: u32 = 60;

fn run_startup_reconcile_live() {
    use super::supervisor;
    use std::sync::atomic::Ordering;

    let status = supervisor::current_status();
    if let Some(why) = startup_gate(status.as_deref(), supervisor::enable_completed()) {
        k2_core::log_debug!("{LOG_PREFIX} skipped because {why}");
        return;
    }
    let client = match supervisor::mgmt_client_from_row() {
        Ok(c) => c,
        Err(e) => {
            k2_core::log_debug!("{LOG_PREFIX} skipped because no management client: {e}");
            return;
        }
    };
    // Stalwart boots alongside the daemon — wait for its admin API.
    let mut last_err = String::new();
    let mut up = false;
    for attempt in 0..API_WAIT_TRIES {
        if supervisor::enable_running().load(Ordering::SeqCst) {
            k2_core::log_debug!("{LOG_PREFIX} skipped because an enable is running");
            return;
        }
        match client.ping() {
            Ok(()) => {
                up = true;
                break;
            }
            Err(e) => last_err = e,
        }
        if attempt + 1 < API_WAIT_TRIES {
            std::thread::sleep(std::time::Duration::from_secs(API_WAIT_STEP_SECS));
        }
    }
    if !up {
        k2_core::log_debug!(
            "{LOG_PREFIX} skipped because the stalwart admin API did not answer within \
             {}s: {last_err}",
            API_WAIT_STEP_SECS * API_WAIT_TRIES as u64
        );
        return;
    }
    // Hold the enable lock so a concurrent enable cannot race the set.
    if !supervisor::try_begin_enable() {
        k2_core::log_debug!("{LOG_PREFIX} skipped because an enable is running");
        return;
    }
    struct EndEnable;
    impl Drop for EndEnable {
        fn drop(&mut self) {
            super::supervisor::end_enable();
        }
    }
    let _guard = EndEnable;

    let port_answers = |port: u16| -> bool {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).is_ok()
    };
    for line in startup_pass(&client, &port_answers, &mut LiveDoors { why: "IMAP listener" }) {
        k2_core::log_debug!("{line}");
    }
}

/// Production restart doors (tests use fakes). `why` labels the wait
/// error ("IMAP listener", "upgrade", "TLS reload").
pub(crate) struct LiveDoors {
    pub why: &'static str,
}

impl RestartDoors for LiveDoors {
    fn helper_state(&mut self) -> super::helper::HelperState {
        super::supervisor::mail_helper_state()
    }

    fn plain_sudo_allowed(&mut self) -> bool {
        // `-l <cmd>`: exit 0 iff sudoers lets this user run exactly that
        // command without a password. `-n` never prompts.
        std::process::Command::new(super::helper::SUDO_PATH)
            .args(["-n", "-l", SYSTEMCTL_PATH, "restart", super::supervisor::STALWART_UNIT])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|st| st.success())
    }

    fn restart(&mut self, path: RestartPath) -> Result<(), String> {
        use super::supervisor;
        match path {
            RestartPath::MailHelper => supervisor::restart_stalwart_and_wait(self.why),
            RestartPath::PlainSudo => {
                let out = std::process::Command::new(super::helper::SUDO_PATH)
                    .args(["-n", SYSTEMCTL_PATH, "restart", supervisor::STALWART_UNIT])
                    .stdin(std::process::Stdio::null())
                    .output()
                    .map_err(|e| format!("sudo -n {SYSTEMCTL_PATH} restart stalwart: {e}"))?;
                if !out.status.success() {
                    let err = String::from_utf8_lossy(&out.stderr);
                    let err: String = err.trim().chars().take(400).collect();
                    return Err(format!(
                        "sudo -n {SYSTEMCTL_PATH} restart stalwart: exit {:?}: {err}",
                        out.status.code()
                    ));
                }
                supervisor::wait_stalwart_active_with(&super::sysops::RealSystemOps, self.why)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::helper::{install_command, HelperState};
    use std::cell::RefCell;

    fn row(
        id: &str,
        name: &str,
        bind: &str,
        protocol: Option<&str>,
        tls_implicit: Option<bool>,
    ) -> ListenerRow {
        ListenerRow {
            id: id.into(),
            name: name.into(),
            binds: vec![bind.into()],
            protocol: protocol.map(Into::into),
            use_tls: Some(true),
            tls_implicit,
        }
    }

    /// A post-enable store from an OLD enable (pre-IMAP template):
    /// smtp/submissions/submission/https/http-mgmt only.
    fn old_enable_rows() -> Vec<ListenerRow> {
        vec![
            row("L-smtp", "smtp", "[::]:25", Some("smtp"), Some(false)),
            row("L-subs", "submissions", "[::]:465", Some("smtp"), Some(true)),
            row("L-sub", "submission", "[::]:587", Some("smtp"), Some(false)),
            row("L-https", "https", "[::]:443", Some("http"), Some(true)),
            row("L-http", "http", "127.0.0.1:8180", Some("http"), None),
        ]
    }

    #[derive(Default)]
    struct FakeStore {
        rows: Vec<ListenerRow>,
        created: RefCell<Vec<Vec<ListenerTemplate>>>,
        create_err: Option<String>,
    }
    impl ListenerStore for FakeStore {
        fn listener_rows(&self) -> Result<Vec<ListenerRow>, String> {
            Ok(self.rows.clone())
        }
        fn create_listeners(&self, t: &[ListenerTemplate]) -> Result<(), String> {
            self.created.borrow_mut().push(t.to_vec());
            match &self.create_err {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }
    }

    /// Recording restart doors. Every probe and restart is counted so a
    /// test can prove the helper was never asked.
    struct FakeDoors {
        helper: HelperState,
        plain_allowed: bool,
        restart_err: Option<String>,
        helper_probes: u32,
        plain_probes: u32,
        restarts: Vec<RestartPath>,
    }
    impl FakeDoors {
        fn new(helper: HelperState, plain_allowed: bool) -> Self {
            Self {
                helper,
                plain_allowed,
                restart_err: None,
                helper_probes: 0,
                plain_probes: 0,
                restarts: Vec::new(),
            }
        }
        /// Today's box: helper installed.
        fn helper() -> Self {
            Self::new(HelperState::Installed, false)
        }
    }
    impl RestartDoors for FakeDoors {
        fn helper_state(&mut self) -> HelperState {
            self.helper_probes += 1;
            self.helper
        }
        fn plain_sudo_allowed(&mut self) -> bool {
            self.plain_probes += 1;
            self.plain_allowed
        }
        fn restart(&mut self, path: RestartPath) -> Result<(), String> {
            self.restarts.push(path);
            match &self.restart_err {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }
    }

    const HELPER_RESTART: RestartReport = RestartReport::Restarted(RestartPath::MailHelper);

    fn run(store: &FakeStore) -> (Result<Outcome, String>, u32) {
        let mut doors = FakeDoors::helper();
        let res = reconcile_with(store, &|_| false, &mut doors);
        (res, doors.restarts.len() as u32)
    }

    #[test]
    fn templates_are_the_server_config_definitions() {
        assert_eq!(
            IMAP_STARTTLS.body(),
            serde_json::json!({
                "name": "imap", "bind": { "[::]:143": true }, "protocol": "imap",
                "useTls": true, "tlsImplicit": false,
            })
        );
        assert_eq!(
            IMAPS_IMPLICIT.body(),
            serde_json::json!({
                "name": "imaps", "bind": { "[::]:993": true }, "protocol": "imap",
                "useTls": true, "tlsImplicit": true,
            })
        );
        assert_eq!(IMAP_STARTTLS.port(), 143);
        assert_eq!(IMAPS_IMPLICIT.port(), 993);
    }

    #[test]
    fn both_absent_adds_both_and_restarts_once() {
        let store = FakeStore { rows: old_enable_rows(), ..Default::default() };
        let (res, restarts) = run(&store);
        assert_eq!(
            res.expect("ok"),
            Outcome::Added {
                added: vec![IMAP_STARTTLS, IMAPS_IMPLICIT],
                skipped: vec![],
                restart: HELPER_RESTART,
            }
        );
        assert_eq!(*store.created.borrow(), vec![vec![IMAP_STARTTLS, IMAPS_IMPLICIT]]);
        assert_eq!(restarts, 1);
    }

    #[test]
    fn one_absent_adds_only_that_one_and_restarts_once() {
        let mut rows = old_enable_rows();
        rows.push(row("L-imaps", "imaps", "[::]:993", Some("imap"), Some(true)));
        let store = FakeStore { rows, ..Default::default() };
        let (res, restarts) = run(&store);
        assert_eq!(
            res.expect("ok"),
            Outcome::Added { added: vec![IMAP_STARTTLS], skipped: vec![], restart: HELPER_RESTART }
        );
        assert_eq!(*store.created.borrow(), vec![vec![IMAP_STARTTLS]]);
        assert_eq!(restarts, 1);

        let mut rows = old_enable_rows();
        rows.push(row("L-imap", "imap", "[::]:143", Some("imap"), Some(false)));
        let store = FakeStore { rows, ..Default::default() };
        let (res, restarts) = run(&store);
        assert_eq!(
            res.expect("ok"),
            Outcome::Added { added: vec![IMAPS_IMPLICIT], skipped: vec![], restart: HELPER_RESTART }
        );
        assert_eq!(restarts, 1);
    }

    #[test]
    fn both_present_is_a_noop_without_write_or_restart() {
        let mut rows = old_enable_rows();
        rows.push(row("L-imap", "imap", "[::]:143", Some("imap"), Some(false)));
        rows.push(row("L-imaps", "imaps", "[::]:993", Some("imap"), Some(true)));
        let store = FakeStore { rows, ..Default::default() };
        let (res, restarts) = run(&store);
        assert_eq!(res.expect("ok"), Outcome::NothingToDo { skipped: vec![] });
        assert!(store.created.borrow().is_empty(), "no write");
        assert_eq!(restarts, 0, "no restart");
    }

    /// akzm shape: repaired by hand via registry create on 2026-10-05.
    /// Field order differs, Stalwart echoes extra defaults, names could
    /// differ — protocol + bind + TLS mode is what counts.
    #[test]
    fn hand_made_registry_rows_count_as_present() {
        let reply = serde_json::json!({
            "list": [
                { "id": "a1", "name": "smtp", "bind": { "[::]:25": true }, "protocol": "smtp" },
                { "tlsImplicit": false, "useTls": true, "protocol": "imap",
                  "bind": { "[::]:143": true }, "id": "hand-143", "name": "imap",
                  "maxConnections": 8192, "proxyProtocol": false, "tlsTimeout": "1m" },
                { "protocol": "IMAP", "bind": { "[::]:993": true }, "id": "hand-993",
                  "name": "imaps-mailapp", "useTls": true, "tlsImplicit": true,
                  "socket": { "backlog": 1024, "nodelay": true } },
            ],
            "notFound": []
        });
        let rows = parse_listener_rows(&reply).expect("parsed");
        assert_eq!(
            verdict_for(&rows, &IMAP_STARTTLS),
            Verdict::Present { id: "hand-143".into() }
        );
        assert_eq!(
            verdict_for(&rows, &IMAPS_IMPLICIT),
            Verdict::Present { id: "hand-993".into() }
        );
        let store = FakeStore { rows, ..Default::default() };
        let (res, restarts) = run(&store);
        assert_eq!(res.expect("ok"), Outcome::NothingToDo { skipped: vec![] });
        assert!(store.created.borrow().is_empty());
        assert_eq!(restarts, 0);
    }

    #[test]
    fn conflicting_listener_is_never_overwritten() {
        // Same name, other bind; same name, other TLS mode.
        let mut rows = old_enable_rows();
        rows.push(row("L-imap", "imap", "127.0.0.1:143", Some("imap"), Some(false)));
        rows.push(row("L-imaps", "imaps", "[::]:993", Some("imap"), Some(false)));
        let store = FakeStore { rows, ..Default::default() };
        let (res, restarts) = run(&store);
        let Outcome::NothingToDo { skipped } = res.expect("ok") else {
            panic!("conflicts must not write");
        };
        assert_eq!(skipped.len(), 2, "{skipped:?}");
        assert!(skipped[0].starts_with("imap: listener 'imap' (id L-imap"), "{skipped:?}");
        assert!(skipped[1].contains("STARTTLS") && skipped[1].contains("implicit TLS"));
        assert!(store.created.borrow().is_empty(), "never overwritten");
        assert_eq!(restarts, 0);

        // Other name holding the template's port with another bind form.
        let mut rows = old_enable_rows();
        rows.push(row("L-x", "legacy-imap", "0.0.0.0:143", Some("imap"), Some(false)));
        let v = verdict_for(&rows, &IMAP_STARTTLS);
        let Verdict::Conflict { reason } = v else { panic!("port taken: {v:?}") };
        assert!(reason.contains("port 143") && reason.contains("legacy-imap"), "{reason}");
        // ... while the other one is still added.
        let store = FakeStore { rows, ..Default::default() };
        let (res, restarts) = run(&store);
        let Outcome::Added { added, skipped, .. } = res.expect("ok") else {
            panic!("imaps still missing");
        };
        assert_eq!(added, vec![IMAPS_IMPLICIT]);
        assert_eq!(skipped.len(), 1);
        assert_eq!(restarts, 1);

        // Plain IMAP (useTls false) on [::]:143 is not the template.
        let mut r = row("L-plain", "imap", "[::]:143", Some("imap"), None);
        r.use_tls = Some(false);
        assert!(matches!(verdict_for(&[r], &IMAP_STARTTLS), Verdict::Conflict { .. }));
    }

    #[test]
    fn foreign_process_on_the_port_skips_the_create() {
        let store = FakeStore { rows: old_enable_rows(), ..Default::default() };
        let mut doors = FakeDoors::helper();
        let res = reconcile_with(&store, &|p| p == 993, &mut doors);
        let Outcome::Added { added, skipped, .. } = res.expect("ok") else { panic!() };
        assert_eq!(added, vec![IMAP_STARTTLS]);
        assert!(skipped[0].starts_with("imaps: port 993 already answers"), "{skipped:?}");
        assert_eq!(doors.restarts, vec![RestartPath::MailHelper]);
    }

    #[test]
    fn create_failure_is_an_error_and_skips_restart() {
        let store = FakeStore {
            rows: old_enable_rows(),
            create_err: Some("x:NetworkListener/set: notCreated: {}".into()),
            ..Default::default()
        };
        let (res, restarts) = run(&store);
        let err = res.expect_err("must fail");
        assert!(err.contains("notCreated"), "{err}");
        assert_eq!(restarts, 0);
        let lines = log_lines(&Err(err));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("FAILED (daemon carries on)"), "{lines:?}");
    }

    #[test]
    fn restart_failure_reports_what_was_added() {
        let store = FakeStore { rows: old_enable_rows(), ..Default::default() };
        let mut doors = FakeDoors::helper();
        doors.restart_err = Some("unit failed".into());
        let res = reconcile_with(&store, &|_| false, &mut doors);
        let err = res.expect_err("restart failure surfaces");
        assert!(err.starts_with("added imap ([::]:143 STARTTLS), imaps ([::]:993 implicit TLS)"));
        assert!(err.contains("unit failed"), "{err}");
    }

    #[test]
    fn create_only_args_have_no_update_or_destroy() {
        let v = create_only_set_args(&[IMAP_STARTTLS, IMAPS_IMPLICIT]).expect("args");
        let obj = v.as_object().expect("object");
        assert_eq!(obj.keys().collect::<Vec<_>>(), vec!["create"]);
        assert_eq!(v["create"]["imap"], IMAP_STARTTLS.body());
        assert_eq!(v["create"]["imaps"], IMAPS_IMPLICIT.body());
        assert!(create_only_set_args(&[]).is_err());
    }

    #[test]
    fn log_lines_name_each_outcome() {
        let added = log_lines(&Ok(Outcome::Added {
            added: vec![IMAP_STARTTLS],
            skipped: vec!["imaps: port 993 ...".into()],
            restart: HELPER_RESTART,
        }));
        assert_eq!(
            added,
            vec![
                "[mail/imap-listeners] added imap ([::]:143 STARTTLS); restarted stalwart",
                "[mail/imap-listeners] skipped imaps: port 993 ...",
            ]
        );
        let noop = log_lines(&Ok(Outcome::NothingToDo { skipped: vec![] }));
        assert_eq!(noop.len(), 1);
        assert!(noop[0].contains("nothing to do"), "{noop:?}");
    }

    #[test]
    fn startup_gate_only_passes_completed_enabled_servers() {
        assert_eq!(startup_gate(Some("running"), true), None);
        assert_eq!(startup_gate(Some("degraded"), true), None);
        assert_eq!(startup_gate(Some("stopped"), true), None);
        assert!(startup_gate(None, true).is_some());
        for s in ["disabled", "installing", "error", "not-installed"] {
            assert!(startup_gate(Some(s), true).is_some(), "{s}");
        }
        assert!(startup_gate(Some("running"), false).is_some(), "incomplete enable");
    }

    /// akzm (2026-10-05, daemon 0.44.1): Stalwart already has 143 + 993,
    /// the helper is not installed. The pass must read, log `nothing to
    /// do`, and never probe the helper. The old code checked the helper
    /// first and logged "skipped because mail helper not installed".
    #[test]
    fn helper_missing_both_present_is_nothing_to_do_without_helper_probe() {
        let mut rows = old_enable_rows();
        rows.push(row("L-imap", "imap", "[::]:143", Some("imap"), Some(false)));
        rows.push(row("L-imaps", "imaps", "[::]:993", Some("imap"), Some(true)));
        let store = FakeStore { rows, ..Default::default() };
        let mut doors = FakeDoors::new(HelperState::Missing, false);
        let lines = startup_pass(&store, &|_| false, &mut doors);
        assert_eq!(
            lines,
            vec![
                "[mail/imap-listeners] nothing to do — imap :143 STARTTLS and imaps :993 \
                 implicit TLS are present"
                    .to_string()
            ]
        );
        assert_eq!(doors.helper_probes, 0, "no helper probe");
        assert_eq!(doors.plain_probes, 0, "no sudo probe");
        assert!(doors.restarts.is_empty(), "no restart");
        assert!(store.created.borrow().is_empty(), "no write");
    }

    #[test]
    fn helper_missing_one_missing_restarts_through_plain_sudo() {
        let mut rows = old_enable_rows();
        rows.push(row("L-imaps", "imaps", "[::]:993", Some("imap"), Some(true)));
        let store = FakeStore { rows, ..Default::default() };
        let mut doors = FakeDoors::new(HelperState::Missing, true);
        let res = reconcile_with(&store, &|_| false, &mut doors);
        assert_eq!(
            res.clone().expect("ok"),
            Outcome::Added {
                added: vec![IMAP_STARTTLS],
                skipped: vec![],
                restart: RestartReport::Restarted(RestartPath::PlainSudo),
            }
        );
        assert_eq!(*store.created.borrow(), vec![vec![IMAP_STARTTLS]], "create first");
        assert_eq!(doors.helper_probes, 1);
        assert_eq!(doors.plain_probes, 1);
        assert_eq!(doors.restarts, vec![RestartPath::PlainSudo], "exactly one restart");
        assert_eq!(
            log_lines(&res),
            vec![
                "[mail/imap-listeners] added imap ([::]:143 STARTTLS); restarted stalwart \
                 (sudo -n /usr/bin/systemctl restart stalwart)"
                    .to_string()
            ]
        );

        // Helper present but refused by sudoers: same plain fallback.
        let store = FakeStore { rows: old_enable_rows(), ..Default::default() };
        let mut doors = FakeDoors::new(HelperState::NotAllowed, true);
        let res = reconcile_with(&store, &|_| false, &mut doors).expect("ok");
        assert!(matches!(
            res,
            Outcome::Added { restart: RestartReport::Restarted(RestartPath::PlainSudo), .. }
        ));
        assert_eq!(doors.restarts, vec![RestartPath::PlainSudo]);
    }

    #[test]
    fn helper_missing_and_no_sudo_keeps_the_create_and_logs_the_hint() {
        let store = FakeStore { rows: old_enable_rows(), ..Default::default() };
        let mut doors = FakeDoors::new(HelperState::Missing, false);
        let res = reconcile_with(&store, &|_| false, &mut doors);
        assert_eq!(
            res.clone().expect("not a failure"),
            Outcome::Added {
                added: vec![IMAP_STARTTLS, IMAPS_IMPLICIT],
                skipped: vec![],
                restart: RestartReport::NotRestarted { helper: HelperState::Missing },
            }
        );
        assert_eq!(
            *store.created.borrow(),
            vec![vec![IMAP_STARTTLS, IMAPS_IMPLICIT]],
            "the create stays (no rollback)"
        );
        assert!(doors.restarts.is_empty(), "no restart");
        assert_eq!(doors.plain_probes, 1);
        let lines = log_lines(&res);
        assert_eq!(lines.len(), 1, "{lines:?}");
        let l = &lines[0];
        assert!(
            l.starts_with(
                "[mail/imap-listeners] added imap ([::]:143 STARTTLS), imaps ([::]:993 \
                 implicit TLS); NOT restarted — the new listeners bind on the next Stalwart \
                 restart. To restart now, run as root: systemctl restart stalwart"
            ),
            "{l}"
        );
        assert!(l.contains("mail helper missing"), "{l}");
        assert!(l.ends_with(&install_command()), "{l}");
        assert!(!l.contains("FAILED"), "{l}");
    }

    #[test]
    fn helper_installed_restarts_through_the_helper_without_sudo_probe() {
        let store = FakeStore { rows: old_enable_rows(), ..Default::default() };
        let mut doors = FakeDoors::new(HelperState::Installed, true);
        let res = reconcile_with(&store, &|_| false, &mut doors).expect("ok");
        assert!(matches!(res, Outcome::Added { restart: HELPER_RESTART, .. }));
        assert_eq!(doors.helper_probes, 1);
        assert_eq!(doors.plain_probes, 0, "helper wins; plain sudo never probed");
        assert_eq!(doors.restarts, vec![RestartPath::MailHelper]);
    }

    #[test]
    fn plain_sudo_restart_failure_reports_what_was_added() {
        let store = FakeStore { rows: old_enable_rows(), ..Default::default() };
        let mut doors = FakeDoors::new(HelperState::Missing, true);
        doors.restart_err = Some("exit Some(1): unit failed".into());
        let err = reconcile_with(&store, &|_| false, &mut doors).expect_err("surfaces");
        assert!(err.starts_with("added imap ([::]:143 STARTTLS), imaps"), "{err}");
        assert!(err.contains("unit failed"), "{err}");
        assert_eq!(store.created.borrow().len(), 1, "created once, not rolled back");
    }

    #[test]
    fn parse_rows_handles_bind_shapes_and_rejects_bad_replies() {
        let rows = parse_listener_rows(&serde_json::json!({ "list": [
            { "id": "a", "name": "x", "bind": { "[::]:1": true, "[::]:2": false } },
            { "id": "b", "name": "y", "bind": ["[::]:3", "127.0.0.1:4"] },
            { "id": "c", "name": "z", "bind": "[::]:5" },
            { "id": "d", "name": "w" },
        ]}))
        .expect("parsed");
        assert_eq!(rows[0].binds, vec!["[::]:1"]);
        assert_eq!(rows[1].binds, vec!["[::]:3", "127.0.0.1:4"]);
        assert_eq!(rows[2].binds, vec!["[::]:5"]);
        assert!(rows[3].binds.is_empty());
        assert_eq!(rows[3].protocol, None);
        assert!(parse_listener_rows(&serde_json::json!({})).is_err());
        assert!(parse_listener_rows(&serde_json::json!({ "list": [{ "name": "x" }] })).is_err());
    }

    // ── Integration: real StalwartClient against the loopback mock ──

    /// The full stored set of a box enabled before the IMAP template
    /// (registry ids random, like live).
    fn old_box_get_reply() -> String {
        serde_json::json!({
            "methodResponses": [["x:NetworkListener/get", {
                "accountId": "b",
                "list": [
                    { "id": "id-smtp", "name": "smtp", "bind": { "[::]:25": true }, "protocol": "smtp", "useTls": true, "tlsImplicit": false },
                    { "id": "id-subs", "name": "submissions", "bind": { "[::]:465": true }, "protocol": "smtp", "useTls": true, "tlsImplicit": true },
                    { "id": "id-sub", "name": "submission", "bind": { "[::]:587": true }, "protocol": "smtp", "useTls": true, "tlsImplicit": false },
                    { "id": "id-pop3s", "name": "pop3s", "bind": { "[::]:995": true }, "protocol": "pop3", "useTls": true, "tlsImplicit": true },
                    { "id": "id-sieve", "name": "sieve", "bind": { "[::]:4190": true }, "protocol": "manageSieve", "useTls": true, "tlsImplicit": false },
                    { "id": "id-https", "name": "https", "bind": { "[::]:443": true }, "protocol": "http", "useTls": true, "tlsImplicit": true },
                    { "id": "id-http", "name": "http", "bind": { "127.0.0.1:8180": true }, "protocol": "http", "useTls": false },
                ],
                "notFound": [],
            }, "0"]],
        })
        .to_string()
    }

    #[test]
    fn integration_old_box_gets_create_only_set_and_one_restart() {
        use crate::mail::jmap::tests::{body_json, spawn_mock_server, NORMAL_SESSION_FIXTURE};
        let set_reply = serde_json::json!({
            "methodResponses": [["x:NetworkListener/set", {
                "accountId": "b",
                "created": { "imap": { "id": "new-143" }, "imaps": { "id": "new-993" } },
            }, "0"]],
        })
        .to_string();
        let (port, rx) = spawn_mock_server(vec![
            NORMAL_SESSION_FIXTURE.to_string(),
            old_box_get_reply(),
            set_reply,
        ]);
        let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        let mut doors = FakeDoors::helper();
        let res = reconcile_with(&client, &|_| false, &mut doors);
        assert_eq!(
            res.expect("reconcile"),
            Outcome::Added {
                added: vec![IMAP_STARTTLS, IMAPS_IMPLICIT],
                skipped: vec![],
                restart: HELPER_RESTART,
            }
        );
        assert_eq!(doors.restarts.len(), 1, "exactly one restart");

        let sess = rx.recv().expect("session request");
        assert!(sess.starts_with("GET /jmap/session"), "{sess}");
        let get = body_json(&rx.recv().expect("get request"));
        assert_eq!(get["methodCalls"][0][0], "x:NetworkListener/get");
        let set_raw = rx.recv().expect("set request");
        let set = body_json(&set_raw);
        assert_eq!(set["methodCalls"].as_array().expect("calls").len(), 1);
        assert_eq!(set["methodCalls"][0][0], "x:NetworkListener/set");
        let args = set["methodCalls"][0][1].as_object().expect("args");
        let mut keys: Vec<&str> = args.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["accountId", "create"], "create-only: no update, no destroy");
        let create = args["create"].as_object().expect("create");
        assert_eq!(create.len(), 2);
        assert_eq!(create["imap"], IMAP_STARTTLS.body());
        assert_eq!(create["imaps"], IMAPS_IMPLICIT.body());
        // listeners_apply would update http/https and destroy pop3s/sieve;
        // none of the stored rows (25/465/587/443/8180/pop3s/sieve) is touched.
        let body = &set_raw[set_raw.find("\r\n\r\n").expect("body")..];
        for id in ["id-smtp", "id-subs", "id-sub", "id-pop3s", "id-sieve", "id-https", "id-http"] {
            assert!(!body.contains(id), "{id} must be untouched: {body}");
        }
        for bind in ["127.0.0.1:8180", "127.0.0.1:8443", "[::]:443", "[::]:25", "[::]:465", "[::]:587"] {
            assert!(!body.contains(bind), "{bind} must not be rewritten: {body}");
        }
        assert!(rx.recv().is_err(), "no further request (no second set, no apply)");
    }

    #[test]
    fn integration_akzm_shape_both_present_reads_only() {
        use crate::mail::jmap::tests::{body_json, spawn_mock_server, NORMAL_SESSION_FIXTURE};
        let mut v: serde_json::Value = serde_json::from_str(&old_box_get_reply()).expect("json");
        let list = v["methodResponses"][0][1]["list"].as_array_mut().expect("list");
        // Hand-made rows: other field order, extra defaults echoed back.
        list.push(serde_json::json!({
            "tlsImplicit": false, "protocol": "imap", "id": "akzm-143", "useTls": true,
            "bind": { "[::]:143": true }, "name": "imap", "maxConnections": 8192,
        }));
        list.push(serde_json::json!({
            "bind": { "[::]:993": true }, "useTls": true, "name": "imaps", "id": "akzm-993",
            "protocol": "imap", "tlsImplicit": true, "proxyProtocol": false,
        }));
        let (port, rx) = spawn_mock_server(vec![NORMAL_SESSION_FIXTURE.to_string(), v.to_string()]);
        let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
        // akzm: helper not installed. Reading needs no helper.
        let mut doors = FakeDoors::new(HelperState::Missing, false);
        let lines = startup_pass(&client, &|_| false, &mut doors);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("nothing to do"), "{lines:?}");
        assert_eq!(doors.helper_probes, 0, "no helper probe");
        assert!(doors.restarts.is_empty(), "no restart");
        let _sess = rx.recv().expect("session");
        let get = body_json(&rx.recv().expect("get"));
        assert_eq!(get["methodCalls"][0][0], "x:NetworkListener/get");
        assert!(rx.recv().is_err(), "no write");
    }
}
