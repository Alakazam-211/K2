//! The container database door (S21; prd-app-database-integrations-v1
//! §4.3, slice S1). **Not built yet**: this is the API the tests are
//! written against; nothing answers Postgres on the App's listener, and
//! nothing is admitted. The next commit builds it.

use serde_json::Value as J;
// The door writes its log with it; its tests reach it through `super::*`.
#[allow(unused_imports)]
use std::io::Write as _;
use std::time::Duration;

pub const MAX_CONNS: usize = 64;
pub const MAX_STARTUP: usize = 10_000;
#[cfg(not(test))]
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
pub const STARTUP_TIMEOUT: Duration = Duration::from_millis(1500);
pub const MAX_NAME: usize = 63;
pub const SUPERUSER: &str = "postgres";

pub mod codes {
    pub const NO_DATABASE: &str = "db_door_no_database";
    pub const DATABASE_REFUSED: &str = "db_door_database_refused";
    pub const ROLE_REFUSED: &str = "db_door_role_refused";
    pub const SUPERUSER_REFUSED: &str = "db_door_superuser_refused";
    pub const REPLICATION_REFUSED: &str = "db_door_replication_refused";
    pub const BAD_STARTUP: &str = "db_door_bad_startup";
    pub const STARTUP_TOO_LARGE: &str = "db_door_startup_too_large";
    pub const TLS_REFUSED: &str = "db_door_tls_refused";
    pub const PROTOCOL_UNSUPPORTED: &str = "db_door_protocol_unsupported";
    pub const UPSTREAM: &str = "db_door_upstream_unreachable";
    pub const TOO_MANY: &str = "db_door_too_many_connections";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admit {
    pub database: String,
    pub roles: Vec<String>,
    pub port: u16,
    pub owned: bool,
}

pub fn app_group_role(db: &str, app: &str) -> String {
    let _ = (db, app);
    String::new()
}

pub fn is_app_role(role: &str, db: &str, app: &str) -> bool {
    let _ = (role, db, app);
    false
}

impl Admit {
    pub fn allows(&self, role: &str, app: &str) -> bool {
        let _ = (role, app);
        false
    }
}

pub fn admitted_in(conn: &rusqlite::Connection, app_id: &str) -> Vec<Admit> {
    let _ = (conn, app_id);
    Vec::new()
}

pub fn admitted(app_id: &str) -> Vec<Admit> {
    let _ = app_id;
    Vec::new()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub sqlstate: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    Ssl,
    GssEnc,
    Cancel { pid: u32, key: Vec<u8> },
    Startup { version: u32, params: Vec<(String, String)> },
}

fn unbuilt() -> Refusal {
    Refusal { code: "", sqlstate: "", message: "not built".into() }
}

pub fn packet_len(word: [u8; 4]) -> Result<usize, Refusal> {
    let _ = word;
    Err(unbuilt())
}

pub fn parse_packet(body: &[u8]) -> Result<Packet, Refusal> {
    let _ = body;
    Err(unbuilt())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Granted {
    pub role: String,
    pub database: String,
    pub port: u16,
}

pub fn decide(app: &str, params: &[(String, String)], admits: &[Admit]) -> Result<Granted, Refusal> {
    let _ = (app, params, admits);
    Err(unbuilt())
}

pub fn error_response(app: &str, r: &Refusal) -> Vec<u8> {
    let _ = (app, r);
    Vec::new()
}

pub fn in_flight(app: &str) -> usize {
    let _ = app;
    0
}

pub fn open_sessions(app: &str) -> usize {
    let _ = app;
    0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cut {
    pub app: String,
    pub database: String,
    pub role: String,
}

pub fn recheck_open() -> Vec<Cut> {
    Vec::new()
}

pub fn log_path(app: &str) -> std::path::PathBuf {
    k2_core::deploy::app_dir(app).join("logs").join("db.log")
}

pub fn tail(app: &str, n: usize) -> Vec<J> {
    let _ = (app, n);
    Vec::new()
}

pub fn record_live(app: &str, answer: &J) {
    let _ = (app, answer);
}

pub fn forget_live(app: &str) {
    let _ = app;
}

pub fn live_door(app: &str) -> Option<u16> {
    let _ = app;
    None
}

pub fn engine_serves_door(version: &J) -> bool {
    let _ = version;
    false
}

pub fn door_port_for(px_planned: Option<u16>, version: Option<&J>) -> Option<u16> {
    let _ = (px_planned, version);
    None
}

pub fn engine_ready() -> bool {
    false
}

#[cfg(test)]
pub(crate) mod test_seams {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::Admit;

    pub type AdmitFn = Arc<dyn Fn() -> Vec<Admit> + Send + Sync>;

    #[derive(Clone)]
    pub struct Seam {
        pub admit: AdmitFn,
        pub upstream: HashMap<u16, u16>,
    }

    pub fn set(app: &str, s: Option<Seam>) {
        let _ = (app, s);
    }
    pub fn set_engine_ready(v: bool) {
        let _ = v;
    }
}

#[cfg(test)]
#[path = "db_door_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "db_door_real_pg.rs"]
mod real_pg_door;
