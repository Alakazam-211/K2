//! The container database door (S21; prd-app-database-integrations-v1
//! §4.3, slice S1).
//!
//! A guarded or relaxed container has its own loopback: nothing answers on
//! its `127.0.0.1:5432` unless K2 forwards it. The engine renders pasta's
//! `-T 5432:<px>`, where `<px>` is the App's own egress proxy port on host
//! loopback (`StartSpec::db_port`). The daemon's listener there is the
//! App's identity (pasta makes every App look like host loopback, so
//! `pg_hba` can't tell Apps apart); it hands a connection to this door when
//! its first byte is a Postgres length prefix (`0x00`) or a TLS record
//! (`0x16`), and keeps everything else for the HTTP proxy. The per-App nft
//! rule already admits that port to that App's cgroup only, so no App
//! reaches another App's door, and no root-helper change was needed.
//!
//! Per connection:
//! 1. limits: at most [`MAX_CONNS`] at once per App; the startup within
//!    [`STARTUP_TIMEOUT`], at most [`MAX_STARTUP`] bytes (Postgres's own
//!    limit);
//! 2. `SSLRequest` and `GSSENCRequest` get `N` (no TLS inside the wall: the
//!    in-wall URL says `sslmode=disable`); a direct TLS ClientHello
//!    (Postgres 17's `sslnegotiation=direct`) is refused;
//! 3. `CancelRequest` goes to the upstream that issued that backend's key,
//!    and only for a key one of this App's own sessions was given;
//! 4. the startup packet (protocol 3.x; 3.0 and 3.2 pass untouched) must
//!    name a role and a database this App may use ([`admitted`]): a
//!    database its workspace owns or was granted, and one of K2's roles for
//!    it. Never the superuser, never replication, never a name twice (the
//!    server keeps the last one, the door would have checked the first);
//! 5. anything else gets a Postgres `ErrorResponse` that names the reason
//!    (`[k2:<code>]` in the message), then the door closes;
//! 6. the packet goes upstream as it came, to the cluster K2's records name
//!    for that database. Auth (SCRAM) stays end to end between the App and
//!    Postgres: the door never sees a password;
//! 7. the door watches the server's messages until the first
//!    `ReadyForQuery` (for `BackendKeyData`), then splices raw;
//! 8. open sessions are cut at once when their grant goes ([`recheck_open`],
//!    run after every `k2 db` change) or the App is uninstalled; a re-host
//!    keeps the listener, so the door survives it;
//! 9. one JSON line per connection goes to `~/.k2/apps/<id>/logs/db.log`
//!    (role, database, upstream port, bytes, time; never SQL).
//!
//! A door session doesn't hold a daemon hand-off's drain open (pooled
//! connections idle for hours): the old daemon's sessions end when it
//! exits, and the App's pool reconnects through the handed-off listener.

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value as J};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::watch;

/// Concurrent door connections per App (its roles have their own
/// connection limits in Postgres).
pub const MAX_CONNS: usize = 64;
/// Postgres's own `MAX_STARTUP_PACKET_LENGTH`.
pub const MAX_STARTUP: usize = 10_000;
/// The whole startup (negotiation included) arrives within this.
#[cfg(not(test))]
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
pub const STARTUP_TIMEOUT: Duration = Duration::from_millis(1500);
/// Dialing the cluster.
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(5);
/// Postgres's longest identifier (NAMEDATALEN - 1): a longer name would be
/// cut by the server, so the door refuses it.
pub const MAX_NAME: usize = 63;
/// Most startup parameters the door reads.
const MAX_PARAMS: usize = 64;
/// The log rolls to `.1` at this size.
const LOG_ROLL_BYTES: u64 = 4 * 1024 * 1024;

const SSL_REQUEST: u32 = 80_877_103;
const GSSENC_REQUEST: u32 = 80_877_104;
const CANCEL_REQUEST: u32 = 80_877_102;
/// Protocol 3.2's longest cancel key (Postgres 18).
const MAX_CANCEL_KEY: usize = 256;

/// The cluster's superuser: never through the door.
pub const SUPERUSER: &str = "postgres";

pub mod codes {
    /// The App's workspace owns no database here and was granted none.
    pub const NO_DATABASE: &str = "db_door_no_database";
    /// The startup names a database this App may not open.
    pub const DATABASE_REFUSED: &str = "db_door_database_refused";
    /// The startup names a role this App may not use there.
    pub const ROLE_REFUSED: &str = "db_door_role_refused";
    pub const SUPERUSER_REFUSED: &str = "db_door_superuser_refused";
    pub const REPLICATION_REFUSED: &str = "db_door_replication_refused";
    /// Not a startup packet the door can read.
    pub const BAD_STARTUP: &str = "db_door_bad_startup";
    pub const STARTUP_TOO_LARGE: &str = "db_door_startup_too_large";
    pub const TLS_REFUSED: &str = "db_door_tls_refused";
    pub const PROTOCOL_UNSUPPORTED: &str = "db_door_protocol_unsupported";
    pub const UPSTREAM: &str = "db_door_upstream_unreachable";
    pub const TOO_MANY: &str = "db_door_too_many_connections";
}

// ── what an App may open ─────────────────────────────────────────────

/// One database this App may open through its door, the roles it may log
/// in as there, and the cluster port the door dials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admit {
    pub database: String,
    pub roles: Vec<String>,
    pub port: u16,
    /// Its workspace owns it (else: granted to the workspace).
    pub owned: bool,
}

/// The NOLOGIN group role an App gets on a database (S2 makes it):
/// `<db>_app_<app id with - as _>` (an App id never holds `_`, so the map
/// is one to one).
pub fn app_group_role(db: &str, app: &str) -> String {
    format!("{db}_app_{}", app.replace('-', "_"))
}

/// The App's own roles on `db`: its group role and its login generations
/// `<group>__g1`, `__g2`, … (S2/S3). Never another App's. The PRD wrote
/// `_g1`, which is ambiguous: App `x-g1`'s group role would be App `x`'s
/// first login. An App id never holds `--`, so its mapped form never holds
/// `__`, and `__g<n>` belongs to one App only.
pub fn is_app_role(role: &str, db: &str, app: &str) -> bool {
    let base = app_group_role(db, app);
    if role == base {
        return true;
    }
    role.strip_prefix(&base)
        .and_then(|r| r.strip_prefix("__g"))
        .is_some_and(|n| !n.is_empty() && n.len() <= 4 && !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit()))
}

impl Admit {
    pub fn allows(&self, role: &str, app: &str) -> bool {
        self.roles.iter().any(|r| r == role) || is_app_role(role, &self.database, app)
    }
}

/// What `app` may open, from K2's records (live: a grant or revoke counts
/// at the next connection): databases its owner workspace owns (active or
/// its test database) with `{db}_agent` and `{db}_migrator`, and databases
/// granted to the workspace with the workspace's grantee role; each on a
/// cluster K2 made or adopted, in the band. The App's own roles (S2) on
/// each are admitted by name ([`is_app_role`]).
pub fn admitted_in(conn: &rusqlite::Connection, app_id: &str) -> Vec<Admit> {
    let Some(row) = k2_core::deploy::store::get_app(conn, app_id).ok().flatten() else { return Vec::new() };
    let ws = row.owner_workspace.as_str();
    if ws.is_empty() {
        return Vec::new();
    }
    let names = |sql: &str| -> Vec<String> {
        conn.prepare(sql)
            .and_then(|mut st| st.query_map(rusqlite::params![ws], |r| r.get::<_, String>(0)).map(|it| it.flatten().collect()))
            .unwrap_or_default()
    };
    let owned = names("SELECT name FROM sql_databases WHERE project_id = ?1 AND status IN ('active', 'test') ORDER BY name");
    let granted = names(
        "SELECT d.name FROM sql_grants g JOIN sql_databases d ON d.id = g.database_id \
         WHERE g.project_id = ?1 AND d.status = 'active' AND d.project_id != ?1 ORDER BY d.name",
    );
    let grantee = crate::sql::ops::default_agent_role(ws);
    let mut out: Vec<Admit> = Vec::new();
    for (name, is_owned) in owned.into_iter().map(|n| (n, true)).chain(granted.into_iter().map(|n| (n, false))) {
        if out.iter().any(|a| a.database == name) {
            continue;
        }
        let Ok(c) = crate::sql::cluster::for_db_in(conn, &name) else { continue };
        if !c.holds_k2_databases() || c.major == 0 || !k2_root_proto::pg_major::in_band(c.port) {
            continue;
        }
        let roles = if is_owned { vec![format!("{name}_agent"), format!("{name}_migrator")] } else { vec![grantee.clone()] };
        out.push(Admit { database: name, roles, port: c.port, owned: is_owned });
    }
    out
}

/// [`admitted_in`] on the daemon's database (a seam per App in tests,
/// where the test's database lives on the test's own thread).
pub fn admitted(app_id: &str) -> Vec<Admit> {
    #[cfg(test)]
    if let Some(a) = test_seams::admit(app_id) {
        return a;
    }
    let h = k2_core::db::shared();
    let conn = h.lock();
    admitted_in(&conn, app_id)
}

// ── the startup packet (pure) ────────────────────────────────────────

/// Why the door refused, as the App's driver will show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub sqlstate: &'static str,
    pub message: String,
}

fn refusal(code: &'static str, sqlstate: &'static str, message: impl Into<String>) -> Refusal {
    Refusal { code, sqlstate, message: message.into() }
}

fn bad(m: impl Into<String>) -> Refusal {
    refusal(codes::BAD_STARTUP, "08P01", m)
}

/// One packet the door reads before the session starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    Ssl,
    GssEnc,
    Cancel { pid: u32, key: Vec<u8> },
    /// A protocol 3.x startup: its version and parameters, in order.
    Startup { version: u32, params: Vec<(String, String)> },
}

/// Check a packet's length word (the first four bytes): `Ok(len)` with
/// the whole packet's length, length word included.
pub fn packet_len(word: [u8; 4]) -> Result<usize, Refusal> {
    if word[0] == 0x16 {
        return Err(refusal(
            codes::TLS_REFUSED,
            "08P01",
            "TLS isn't spoken inside the wall: connect with sslmode=disable (sslnegotiation=direct is refused)",
        ));
    }
    let n = u32::from_be_bytes(word) as usize;
    if n < 8 {
        return Err(bad(format!("a startup packet of {n} bytes (at least 8)")));
    }
    if n > MAX_STARTUP {
        return Err(refusal(codes::STARTUP_TOO_LARGE, "08P01", format!("a startup packet over {MAX_STARTUP} bytes")));
    }
    Ok(n)
}

/// Parse one packet's body (after the length word).
pub fn parse_packet(body: &[u8]) -> Result<Packet, Refusal> {
    let code = body.get(..4).map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]])).ok_or_else(|| bad("a startup packet without a code"))?;
    let rest = &body[4..];
    match code {
        SSL_REQUEST if rest.is_empty() => Ok(Packet::Ssl),
        GSSENC_REQUEST if rest.is_empty() => Ok(Packet::GssEnc),
        SSL_REQUEST | GSSENC_REQUEST => Err(bad("an SSLRequest or GSSENCRequest with a body")),
        CANCEL_REQUEST => {
            if rest.len() < 8 || rest.len() > 4 + MAX_CANCEL_KEY {
                return Err(bad("a CancelRequest of the wrong length"));
            }
            Ok(Packet::Cancel { pid: u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]), key: rest[4..].to_vec() })
        }
        v if v >> 16 == 3 => Ok(Packet::Startup { version: v, params: parse_params(rest)? }),
        v => Err(refusal(
            codes::PROTOCOL_UNSUPPORTED,
            "0A000",
            format!("frontend protocol {}.{} isn't served (the door speaks protocol 3)", v >> 16, v & 0xffff),
        )),
    }
}

fn parse_params(mut b: &[u8]) -> Result<Vec<(String, String)>, Refusal> {
    let mut out: Vec<(String, String)> = Vec::new();
    let cstr = |b: &mut &[u8]| -> Result<String, Refusal> {
        let i = b.iter().position(|c| *c == 0).ok_or_else(|| bad("a startup parameter without its terminator"))?;
        let s = std::str::from_utf8(&b[..i]).map_err(|_| bad("a startup parameter that isn't UTF-8"))?.to_string();
        *b = &b[i + 1..];
        Ok(s)
    };
    loop {
        if b.first() == Some(&0) {
            if b.len() != 1 {
                return Err(bad("bytes after the startup parameters' end"));
            }
            return Ok(out);
        }
        if b.is_empty() {
            return Err(bad("startup parameters without their end"));
        }
        let k = cstr(&mut b)?;
        let v = cstr(&mut b)?;
        if k.is_empty() {
            return Err(bad("an empty startup parameter name"));
        }
        if out.iter().any(|(x, _)| *x == k) {
            return Err(bad(format!("the startup parameter {k} twice")));
        }
        if out.len() >= MAX_PARAMS {
            return Err(bad(format!("more than {MAX_PARAMS} startup parameters")));
        }
        out.push((k, v));
    }
}

/// The admitted session a startup asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Granted {
    pub role: String,
    pub database: String,
    pub port: u16,
}

/// Decide a startup for `app` against what it may open.
pub fn decide(app: &str, params: &[(String, String)], admits: &[Admit]) -> Result<Granted, Refusal> {
    let get = |k: &str| params.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
    let role = get("user").filter(|u| !u.is_empty()).ok_or_else(|| bad("a startup without a user"))?;
    // Postgres: an absent or empty database is the user's name.
    let database = get("database").filter(|d| !d.is_empty()).unwrap_or(role);
    for (what, n) in [("role", role), ("database", database)] {
        if n.len() > MAX_NAME {
            return Err(bad(format!("a {what} name over {MAX_NAME} bytes")));
        }
    }
    if let Some(r) = get("replication") {
        if !matches!(r.to_ascii_lowercase().as_str(), "false" | "off" | "no" | "0") {
            return Err(refusal(codes::REPLICATION_REFUSED, "28000", format!("App {app} may not open a replication connection")));
        }
    }
    if role == SUPERUSER || role.starts_with("pg_") {
        return Err(refusal(
            codes::SUPERUSER_REFUSED,
            "28000",
            format!("App {app} may not log in as {role}: it is the cluster's own role, never an App's"),
        ));
    }
    if admits.is_empty() {
        return Err(refusal(
            codes::NO_DATABASE,
            "28000",
            format!("App {app} has no database on this box: its workspace owns none and was granted none (k2 db create, or k2 db grant)"),
        ));
    }
    let Some(a) = admits.iter().find(|a| a.database == database) else {
        return Err(refusal(
            codes::DATABASE_REFUSED,
            "28000",
            format!(
                "App {app} may not open database {database}: it isn't its workspace's and wasn't granted to it (its databases: {})",
                admits.iter().map(|a| a.database.as_str()).collect::<Vec<_>>().join(", ")
            ),
        ));
    };
    if !a.allows(role, app) {
        return Err(refusal(
            codes::ROLE_REFUSED,
            "28000",
            format!("App {app} may not log in to {database} as {role}: the door admits {} there", a.roles.join(", ")),
        ));
    }
    Ok(Granted { role: role.to_string(), database: database.to_string(), port: a.port })
}

/// A Postgres `ErrorResponse` (FATAL) carrying the refusal.
pub fn error_response(app: &str, r: &Refusal) -> Vec<u8> {
    let mut f: Vec<u8> = Vec::new();
    let mut field = |t: u8, v: &str| {
        f.push(t);
        f.extend_from_slice(v.replace('\0', "").as_bytes());
        f.push(0);
    };
    field(b'S', "FATAL");
    field(b'V', "FATAL");
    field(b'C', r.sqlstate);
    field(b'M', &format!("K2 database door: {} [k2:{}]", r.message, r.code));
    field(b'H', &format!("k2 app doctor {app} shows what the App may reach"));
    f.push(0);
    let mut out = vec![b'E'];
    out.extend_from_slice(&((f.len() + 4) as u32).to_be_bytes());
    out.extend_from_slice(&f);
    out
}

// ── sessions, keys, limits ───────────────────────────────────────────

struct Session {
    app: String,
    database: String,
    role: String,
    cut: watch::Sender<bool>,
}

fn sessions() -> &'static Mutex<HashMap<u64, Session>> {
    static S: OnceLock<Mutex<HashMap<u64, Session>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Backend keys this App's sessions were given: (app, pid) → (key, port).
fn keys() -> &'static Mutex<HashMap<(String, u32), (Vec<u8>, u16)>> {
    static K: OnceLock<Mutex<HashMap<(String, u32), (Vec<u8>, u16)>>> = OnceLock::new();
    K.get_or_init(Default::default)
}

fn active() -> &'static Mutex<HashMap<String, usize>> {
    static A: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    A.get_or_init(Default::default)
}

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Slot(String);
impl Drop for Slot {
    fn drop(&mut self) {
        let mut g = active().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = g.get_mut(&self.0) {
            *n = n.saturating_sub(1);
        }
    }
}

fn take_slot(app: &str) -> Option<Slot> {
    let mut g = active().lock().unwrap_or_else(|e| e.into_inner());
    let n = g.entry(app.to_string()).or_insert(0);
    if *n >= MAX_CONNS {
        return None;
    }
    *n += 1;
    Some(Slot(app.to_string()))
}

/// Door connections of `app` still being served (startup, session or
/// refusal), whose log line may not be written yet.
pub fn in_flight(app: &str) -> usize {
    active().lock().unwrap_or_else(|e| e.into_inner()).get(app).copied().unwrap_or(0)
}

/// Open door sessions of `app`.
pub fn open_sessions(app: &str) -> usize {
    sessions().lock().unwrap_or_else(|e| e.into_inner()).values().filter(|s| s.app == app).count()
}

/// Sessions cut because their grant went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cut {
    pub app: String,
    pub database: String,
    pub role: String,
}

/// After a database change: cut every open session whose role or
/// database this App may no longer use (a revoke, a dropped database).
/// Postgres keeps a revoked role's open sessions; the door doesn't.
pub fn recheck_open() -> Vec<Cut> {
    let mut apps: Vec<String> = sessions().lock().unwrap_or_else(|e| e.into_inner()).values().map(|s| s.app.clone()).collect();
    // Tests share one process: each sees only the Apps it set up.
    #[cfg(test)]
    apps.retain(|a| test_seams::mine(a));
    apps.sort();
    apps.dedup();
    let mut allowed: HashMap<String, Vec<Admit>> = HashMap::new();
    for a in apps {
        let ad = admitted(&a);
        allowed.insert(a, ad);
    }
    let g = sessions().lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    for s in g.values() {
        let Some(ad) = allowed.get(&s.app) else { continue };
        if !ad.iter().any(|a| a.database == s.database && a.allows(&s.role, &s.app)) {
            let _ = s.cut.send(true);
            out.push(Cut { app: s.app.clone(), database: s.database.clone(), role: s.role.clone() });
        }
    }
    out.sort_by(|a, b| (&a.app, &a.database, &a.role).cmp(&(&b.app, &b.database, &b.role)));
    out
}

// ── the listener's hand-off ──────────────────────────────────────────

/// Does this connection speak Postgres (or TLS, which the door refuses)?
/// Peeks at the first byte within the startup timeout; a client that sends
/// nothing stays the proxy's (it times out there as before).
pub async fn is_door(c: &TcpStream) -> bool {
    let mut b = [0u8; 1];
    matches!(tokio::time::timeout(STARTUP_TIMEOUT, c.peek(&mut b)).await, Ok(Ok(1)) if b[0] == 0x00 || b[0] == 0x16)
}

fn upstream_port(app: &str, port: u16) -> u16 {
    #[cfg(test)]
    if let Some(p) = test_seams::upstream(app, port) {
        return p;
    }
    let _ = app;
    port
}

async fn read_packet(c: &mut TcpStream, deadline: tokio::time::Instant) -> Result<(Vec<u8>, Packet), Refusal> {
    let mut word = [0u8; 4];
    match tokio::time::timeout_at(deadline, c.read_exact(&mut word)).await {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => return Err(bad("closed before the startup packet ended")),
        Err(_) => return Err(bad(format!("no startup packet within {} s", STARTUP_TIMEOUT.as_secs_f32()))),
    }
    let n = packet_len(word)?;
    let mut body = vec![0u8; n - 4];
    match tokio::time::timeout_at(deadline, c.read_exact(&mut body)).await {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => return Err(bad("closed before the startup packet ended")),
        Err(_) => return Err(bad(format!("no startup packet within {} s", STARTUP_TIMEOUT.as_secs_f32()))),
    }
    let p = parse_packet(&body)?;
    let mut raw = word.to_vec();
    raw.extend_from_slice(&body);
    Ok((raw, p))
}

/// Serve one door connection on `app`'s listener. `stop` is the
/// listener's (the App uninstalled).
pub(crate) async fn serve(mut c: TcpStream, app: String, stop: watch::Receiver<bool>) {
    let t0 = Instant::now();
    let Some(_slot) = take_slot(&app) else {
        let r = refusal(codes::TOO_MANY, "53300", format!("at most {MAX_CONNS} database connections at once per App"));
        return refuse(&mut c, &app, &r, None, t0).await;
    };
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    let (mut ssl, mut gss) = (false, false);
    let (raw, params) = loop {
        match read_packet(&mut c, deadline).await {
            Err(r) => return refuse(&mut c, &app, &r, None, t0).await,
            Ok((_, Packet::Ssl)) if !ssl => {
                ssl = true;
                if c.write_all(b"N").await.is_err() {
                    return;
                }
            }
            Ok((_, Packet::GssEnc)) if !gss => {
                gss = true;
                if c.write_all(b"N").await.is_err() {
                    return;
                }
            }
            Ok((_, Packet::Ssl | Packet::GssEnc)) => {
                return refuse(&mut c, &app, &bad("the same encryption request twice"), None, t0).await;
            }
            Ok((raw, Packet::Cancel { pid, key })) => return cancel(&app, &raw, pid, &key, t0).await,
            Ok((raw, Packet::Startup { params, .. })) => break (raw, params),
        }
    };
    let a = app.clone();
    let admits = match tokio::task::spawn_blocking(move || admitted(&a)).await {
        Ok(v) => v,
        Err(_) => Vec::new(),
    };
    let g = match decide(&app, &params, &admits) {
        Ok(g) => g,
        Err(r) => {
            let who = params.iter().find(|(k, _)| k == "user").map(|(_, v)| v.clone());
            let db = params.iter().find(|(k, _)| k == "database").map(|(_, v)| v.clone()).or_else(|| who.clone());
            return refuse(&mut c, &app, &r, Some((who.unwrap_or_default(), db.unwrap_or_default())), t0).await;
        }
    };
    let port = upstream_port(&app, g.port);
    let up = match tokio::time::timeout(UPSTREAM_TIMEOUT, TcpStream::connect(("127.0.0.1", port))).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            let r = refusal(codes::UPSTREAM, "08006", format!("the cluster for {} on port {} didn't answer ({})", g.database, g.port, e.kind()));
            return refuse(&mut c, &app, &r, Some((g.role.clone(), g.database.clone())), t0).await;
        }
        Err(_) => {
            let r = refusal(codes::UPSTREAM, "08006", format!("the cluster for {} on port {} didn't answer in time", g.database, g.port));
            return refuse(&mut c, &app, &r, Some((g.role.clone(), g.database.clone())), t0).await;
        }
    };
    let _ = up.set_nodelay(true);
    let _ = c.set_nodelay(true);
    let (cut_tx, cut_rx) = watch::channel(false);
    let id = NEXT.fetch_add(1, Ordering::SeqCst);
    sessions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, Session { app: app.clone(), database: g.database.clone(), role: g.role.clone(), cut: cut_tx });
    let (upb, downb, how, pid) = relay(c, up, &raw, &app, g.port, stop, cut_rx).await;
    // The line first: an App with no open session has its lines written.
    log_line(
        &app,
        json!({
            "kind": "session", "role": g.role, "database": g.database, "upstream": g.port, "result": "ok", "end": how,
            "up": upb, "down": downb, "ms": t0.elapsed().as_millis() as u64,
        }),
    );
    if let Some(pid) = pid {
        keys().lock().unwrap_or_else(|e| e.into_inner()).remove(&(app.clone(), pid));
    }
    sessions().lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
}

async fn refuse(c: &mut TcpStream, app: &str, r: &Refusal, who: Option<(String, String)>, t0: Instant) {
    // The line first: once the App sees the answer, it is written.
    let mut line = json!({ "kind": "session", "result": "refused", "code": r.code, "ms": t0.elapsed().as_millis() as u64 });
    if let Some((role, db)) = who {
        line["role"] = json!(role.chars().take(MAX_NAME + 1).collect::<String>());
        line["database"] = json!(db.chars().take(MAX_NAME + 1).collect::<String>());
    }
    log_line(app, line);
    let _ = c.write_all(&error_response(app, r)).await;
    let _ = c.shutdown().await;
}

/// A CancelRequest: to the upstream that issued this key to one of this
/// App's sessions; anything else is dropped (Postgres answers a cancel
/// with nothing either way).
async fn cancel(app: &str, raw: &[u8], pid: u32, key: &[u8], t0: Instant) {
    let to = keys().lock().unwrap_or_else(|e| e.into_inner()).get(&(app.to_string(), pid)).filter(|(k, _)| k == key).map(|(_, p)| *p);
    let result = match to {
        Some(port) => match tokio::time::timeout(UPSTREAM_TIMEOUT, TcpStream::connect(("127.0.0.1", upstream_port(app, port)))).await {
            Ok(Ok(mut s)) => {
                let _ = s.write_all(raw).await;
                let _ = s.shutdown().await;
                "sent"
            }
            _ => "upstream_unreachable",
        },
        None => "unknown_key",
    };
    log_line(app, json!({ "kind": "cancel", "result": result, "ms": t0.elapsed().as_millis() as u64 }));
}

async fn pump<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(r: &mut R, w: &mut W, n: &AtomicU64) -> std::io::Result<()> {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let k = r.read(&mut buf).await?;
        if k == 0 {
            let _ = w.shutdown().await;
            return Ok(());
        }
        w.write_all(&buf[..k]).await?;
        n.fetch_add(k as u64, Ordering::Relaxed);
    }
}

/// The server's messages up to its first `ReadyForQuery`, forwarded as
/// they are; `BackendKeyData` is remembered for cancels. Returns the
/// backend's pid when the server sent one.
async fn server_until_ready<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    r: &mut R,
    w: &mut W,
    n: &AtomicU64,
    app: &str,
    port: u16,
    pid_out: &Mutex<Option<u32>>,
) -> std::io::Result<()> {
    let invalid = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string());
    loop {
        let mut h = [0u8; 5];
        r.read_exact(&mut h).await?;
        let len = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
        if len < 4 {
            return Err(invalid("a server message shorter than its length word"));
        }
        w.write_all(&h).await?;
        let body = len - 4;
        if h[0] == b'K' && (8..=4 + MAX_CANCEL_KEY).contains(&body) {
            let mut b = vec![0u8; body];
            r.read_exact(&mut b).await?;
            w.write_all(&b).await?;
            let pid = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
            keys().lock().unwrap_or_else(|e| e.into_inner()).insert((app.to_string(), pid), (b[4..].to_vec(), port));
            *pid_out.lock().unwrap_or_else(|e| e.into_inner()) = Some(pid);
        } else {
            let copied = tokio::io::copy(&mut (&mut *r).take(body as u64), w).await?;
            if copied != body as u64 {
                return Err(invalid("the server closed inside a message"));
            }
        }
        n.fetch_add(len as u64 + 1, Ordering::Relaxed);
        if h[0] == b'Z' {
            return Ok(());
        }
    }
}

async fn stopped(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Send the startup upstream, then relay both ways until either side
/// closes, the App is uninstalled, or its grant goes. Returns (bytes up,
/// bytes down, why it ended, the backend's pid).
async fn relay(
    c: TcpStream,
    mut up: TcpStream,
    startup: &[u8],
    app: &str,
    port: u16,
    mut stop: watch::Receiver<bool>,
    mut cut: watch::Receiver<bool>,
) -> (u64, u64, &'static str, Option<u32>) {
    let (upb, downb) = (AtomicU64::new(0), AtomicU64::new(0));
    if up.write_all(startup).await.is_err() {
        return (0, 0, "upstream_closed", None);
    }
    upb.fetch_add(startup.len() as u64, Ordering::Relaxed);
    let pid = Mutex::new(None);
    let (mut cr, mut cw) = c.into_split();
    let (mut ur, mut uw) = up.into_split();
    let c2u = pump(&mut cr, &mut uw, &upb);
    let u2c = async {
        server_until_ready(&mut ur, &mut cw, &downb, app, port, &pid).await?;
        pump(&mut ur, &mut cw, &downb).await
    };
    tokio::pin!(c2u, u2c);
    let mut client_done = false;
    let how = loop {
        tokio::select! {
            r = &mut c2u, if !client_done => {
                client_done = true;
                if r.is_err() {
                    break "closed";
                }
            }
            _ = &mut u2c => break "closed",
            _ = stopped(&mut stop) => break "app_stopped",
            _ = stopped(&mut cut) => break "revoked",
        }
    };
    let pid = *pid.lock().unwrap_or_else(|e| e.into_inner());
    (upb.load(Ordering::Relaxed), downb.load(Ordering::Relaxed), how, pid)
}

// ── the log ──────────────────────────────────────────────────────────

fn log_lock() -> &'static Mutex<()> {
    static L: Mutex<()> = Mutex::new(());
    &L
}

/// `~/.k2/apps/<id>/logs/db.log`.
pub fn log_path(app: &str) -> std::path::PathBuf {
    k2_core::deploy::app_dir(app).join("logs").join("db.log")
}

fn log_line(app: &str, mut v: J) {
    v["at"] = json!(k2_core::deploy::now());
    let p = log_path(app);
    let _g = log_lock().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if std::fs::metadata(&p).map(|m| m.len() > LOG_ROLL_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&p, p.with_extension("log.1"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{v}");
    }
}

/// The last `n` log lines (newest last), parsed.
pub fn tail(app: &str, n: usize) -> Vec<J> {
    let text = std::fs::read_to_string(log_path(app)).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n.clamp(1, 2000))..].iter().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

// ── the running deployment's door ────────────────────────────────────

/// The doors running deployments have (process-wide; per test thread in
/// tests, which share App names).
fn with_live<T>(f: impl FnOnce(&mut HashMap<String, u16>) -> T) -> T {
    #[cfg(not(test))]
    {
        static L: OnceLock<Mutex<HashMap<String, u16>>> = OnceLock::new();
        f(&mut L.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()))
    }
    #[cfg(test)]
    {
        thread_local! {
            static L: std::cell::RefCell<HashMap<String, u16>> = std::cell::RefCell::new(HashMap::new());
        }
        L.with(|l| f(&mut l.borrow_mut()))
    }
}

/// Record the door the engine rendered for `app`'s running deployment
/// (its start or adopt answer's `dbPort`; `None`: it has none).
pub fn record_live(app: &str, answer: &J) {
    let p = answer["dbPort"].as_u64().and_then(|p| u16::try_from(p).ok());
    with_live(|g| match p {
        Some(p) => {
            g.insert(app.to_string(), p);
        }
        None => {
            g.remove(app);
        }
    });
}

pub fn forget_live(app: &str) {
    with_live(|g| g.remove(app));
}

/// The door port `app`'s running deployment has, if any.
pub fn live_door(app: &str) -> Option<u16> {
    with_live(|g| g.get(app).copied())
}

/// Does this engine `version` answer serve the door ([`FEATURE_DB_DOOR`])?
///
/// [`FEATURE_DB_DOOR`]: k2_root_proto::engine::FEATURE_DB_DOOR
pub fn engine_serves_door(version: &J) -> bool {
    version["features"].as_array().is_some_and(|f| f.iter().any(|x| x.as_str() == Some(k2_root_proto::engine::FEATURE_DB_DOOR)))
}

/// The `dbPort` a start sends: the App's proxy port, when the engine that
/// prepared it serves the door. `None` otherwise (an older engine: the App
/// starts without a door, and K2 says so).
pub fn door_port_for(px_planned: Option<u16>, version: Option<&J>) -> Option<u16> {
    let px = px_planned.filter(|p| k2_root_proto::engine::db_port_ok(*p))?;
    version.filter(|v| engine_serves_door(v)).map(|_| px)
}

/// Does this box's App engine serve the door now? (Its `version` answer,
/// kept for a minute; a test seam.)
pub fn engine_ready() -> bool {
    #[cfg(test)]
    {
        test_seams::engine_ready()
    }
    #[cfg(not(test))]
    {
        static C: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
        if let Some((at, v)) = *C.lock().unwrap_or_else(|e| e.into_inner()) {
            if at.elapsed() < Duration::from_secs(60) {
                return v;
            }
        }
        let v = super::guarded::engine_version().is_ok_and(|v| engine_serves_door(&v));
        *C.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), v));
        v
    }
}

#[cfg(test)]
pub(crate) mod test_seams {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::Admit;

    pub type AdmitFn = Arc<dyn Fn() -> Vec<Admit> + Send + Sync>;

    /// Per App: what it may open (the test's database runs on the test's
    /// thread), and cluster port → the fake upstream's port.
    #[derive(Clone)]
    pub struct Seam {
        pub admit: AdmitFn,
        pub upstream: HashMap<u16, u16>,
    }

    /// App → (its seam, the test thread that set it).
    static SEAMS: Mutex<Option<HashMap<String, (Seam, std::thread::ThreadId)>>> = Mutex::new(None);
    thread_local! {
        /// Does "this box's engine" serve the door (per test thread)?
        static ENGINE_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    pub fn set(app: &str, s: Option<Seam>) {
        let mut g = SEAMS.lock().unwrap_or_else(|e| e.into_inner());
        let m = g.get_or_insert_with(HashMap::new);
        match s {
            Some(s) => {
                m.insert(app.to_string(), (s, std::thread::current().id()));
            }
            None => {
                m.remove(app);
            }
        }
    }
    fn get(app: &str) -> Option<Seam> {
        SEAMS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(app).map(|(s, _)| s.clone()))
    }
    /// Did this test thread set `app`'s seam?
    pub(super) fn mine(app: &str) -> bool {
        let me = std::thread::current().id();
        SEAMS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|m| m.get(app).is_some_and(|(_, t)| *t == me))
    }
    pub(super) fn admit(app: &str) -> Option<Vec<Admit>> {
        get(app).map(|s| (s.admit)())
    }
    pub(super) fn upstream(app: &str, port: u16) -> Option<u16> {
        get(app).and_then(|s| s.upstream.get(&port).copied())
    }
    pub fn set_engine_ready(v: bool) {
        ENGINE_READY.with(|c| c.set(v));
    }
    pub(super) fn engine_ready() -> bool {
        ENGINE_READY.with(|c| c.get())
    }
}

#[cfg(test)]
#[path = "db_door_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "db_door_real_pg.rs"]
mod real_pg_door;
