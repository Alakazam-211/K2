//! Side effects of the Sew supervisor, behind one trait.
//!
//! [`RealOps`] talks to the root helper (`sudo -n … engine sew …`),
//! `systemctl show` (as the daemon user: reading unit state needs no root),
//! `/proc`, `statvfs`, socket binds, and Sew's mgmt socket
//! (`/run/sew/mgmt.sock`, HTTP/1.1, `Authorization: Bearer sewsvc_…`).
//! [`fake::FakeOps`] models a box in memory for the machine tests: it
//! records every helper call and flips unit state the way systemd would.

use std::path::Path;

use serde_json::{json, Value};

use crate::root_helper::RootHelperState;

/// What `systemctl show` says about one unit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitInfo {
    /// `active`, `inactive`, `failed`, `activating`, … (`unknown` when the
    /// read failed).
    pub active: String,
    pub result: String,
    pub exec_main_status: i64,
    pub n_restarts: u64,
    pub memory_current: Option<u64>,
    pub memory_max: Option<u64>,
}

impl UnitInfo {
    pub fn is_active(&self) -> bool {
        self.active == "active"
    }
    pub fn json(&self) -> Value {
        json!({
            "active": self.active,
            "result": self.result,
            "execMainStatus": self.exec_main_status,
            "nRestarts": self.n_restarts,
            "memoryCurrent": self.memory_current,
            "memoryMax": self.memory_max,
        })
    }
}

/// A mgmt call that did not produce a 2xx JSON answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MgmtError {
    /// Socket missing / refused / timed out.
    Unreachable(String),
    /// 401: the token is stale (re-fetch it once).
    Unauthorized,
    /// Another HTTP status with Sew's error body (first 400 bytes).
    Status(u16, String),
}

impl std::fmt::Display for MgmtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MgmtError::Unreachable(e) => write!(f, "Sew's mgmt socket is unreachable: {e}"),
            MgmtError::Unauthorized => write!(f, "Sew refused K2's mgmt token (401)"),
            MgmtError::Status(c, b) => write!(f, "Sew mgmt answered {c}: {b}"),
        }
    }
}

pub trait SewOps: Send + Sync {
    fn linux(&self) -> bool;
    fn arch(&self) -> String;
    fn helper_state(&self) -> RootHelperState;
    /// `sudo -n k2-root-helper <argv>`; the JSON it printed.
    fn helper(&self, argv: &[String], stdin: Option<&[u8]>) -> Result<Value, String>;
    fn unit(&self, unit: &str) -> UnitInfo;
    /// A file the daemon user can read (`--tarball`, `--recovery-key-file`).
    fn read_file(&self, path: &Path, cap: usize) -> Result<Vec<u8>, String>;
    fn path_exists(&self, path: &str) -> bool;
    /// Whether `path` exists as the daemon user sees it: `Some(true)` /
    /// `Some(false)`, `None` when it can't tell (a parent it may not enter).
    fn path_probe(&self, path: &str) -> Option<bool>;
    fn systemd_version(&self) -> Option<u32>;
    /// Postgres majors with `postgres`, `initdb`, `pg_dump`, `pg_restore`
    /// under `/usr/lib/postgresql/<M>/bin/`.
    fn pg_majors(&self) -> Vec<u32>;
    fn mem_total(&self) -> Option<u64>;
    fn disk_free(&self, path: &str) -> Option<u64>;
    /// Something already listens on `addr:port` (TCP) / is bound (UDP).
    fn port_taken(&self, addr: &str, port: u16, udp: bool) -> bool;
    /// The daemon's own HTTP port (for S28's JWKS URL); `None` before bind.
    fn daemon_port(&self) -> Option<u16>;
    fn hostname(&self) -> String;
    fn mgmt(&self, method: &str, path: &str, token: &str, body: Option<&Value>) -> Result<Value, MgmtError>;
    fn now(&self) -> i64;
    fn sleep_ms(&self, ms: u64);
    /// `GET /api/v1/health` on Sew's loopback client listener (Sew 0.3.1
    /// readiness, [`super::health`]): `(status, body)`, or why nothing
    /// answered.
    fn api_health(&self) -> Result<(u16, Value), String> {
        Err("this build has no client-listener probe".into())
    }

    // ── voice (K13) ──────────────────────────────────────────────────────
    /// This box's k2.dev label (the tunnel's primary label), `None` without
    /// K2 Connect.
    fn box_label(&self) -> Option<String>;
    /// Custom domains bound to this box (apexes, lowercase).
    fn bound_apexes(&self) -> Vec<String>;
    /// Public addresses on this box's interfaces (IPv4 and global IPv6).
    fn public_ips(&self) -> Vec<String>;
    /// One call to the K2 Connect control plane with the box's tunnel
    /// token (`PUT|DELETE /turn/dns/<label>`, `POST /cert` turn mode).
    fn cp(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, CpError>;
    /// Point a box-level nested label at `target` (`None` deletes it).
    fn nested_label(&self, label: &str, target: Option<&str>) -> Result<(), String>;
    /// The leaf sha256 `connect:port` presents for SNI `sni` (`None` = no
    /// handshake).
    fn served_leaf(&self, connect: &str, sni: &str, port: u16) -> Option<String>;
    /// frpc logins since the daemon started (a TURN DNS `ip_unverified`
    /// retries after the next one).
    fn frpc_logins(&self) -> u64;
    /// The voice state record (`~/.k2/sew/voice.json`, 0600); `None` = none.
    fn voice_load(&self) -> Option<Value>;
    /// Write the voice record (`None` removes it).
    fn voice_save(&self, v: Option<&Value>) -> Result<(), String>;
    /// One of K2's small Sew records (`~/.k2/sew/<name>.json`, 0600):
    /// `server` (the public URL K2 set), `meet` (the `meet` label bind).
    fn record_load(&self, name: &str) -> Option<Value>;
    /// Write a record (`None` removes it).
    fn record_save(&self, name: &str, v: Option<&Value>) -> Result<(), String>;

    /// front443 holds this box's :443 and routes by SNI (0.46.1): voice
    /// then runs the `passthrough` TURN tier behind it (TURN on 443 only).
    fn front_serving(&self) -> bool {
        false
    }

    /// The Sew this box runs knows the `passthrough` tier (0.2.1+; the
    /// helper refuses it below that whatever the daemon thinks).
    fn passthrough_supported(&self) -> bool {
        k2_root_proto::sew::pinned_media_supports_passthrough()
    }

    /// Voice's state changed in a way the front routes on (a passthrough
    /// install): rebuild its table now.
    fn front_routes_changed(&self) {}

    /// This build pins a published Sew release
    /// (`k2_root_proto::sew::sew_release_pinned`). While false every
    /// install path refuses `sew_not_released`. Tests override it.
    fn sew_released(&self) -> bool {
        k2_root_proto::sew::sew_release_pinned()
    }

    /// Sew's mgmt socket, for S27's grant-hook target (tests point it at a
    /// fake Sew).
    fn mgmt_socket(&self) -> String {
        k2_root_proto::sew::SEW_MGMT_SOCKET.to_string()
    }

    // ── S27's grant hook (k2-core `tool_hook`) ───────────────────────────
    /// The grant-hook targets that exist (kinds). Never creates `skin.db`.
    fn hook_targets(&self) -> Result<Vec<String>, String> {
        Ok(k2_core::tool_hook::list_targets()?.into_iter().map(|t| t.kind).collect())
    }
    /// Turn the chat target on (with its socket and token file) or off.
    fn hook_set(&self, enabled: bool, socket: Option<&str>, token_file: Option<&str>, actor: &str) -> Result<(), String> {
        k2_core::tool_hook::set_target(k2_core::tool_hook::CHAT_KIND, enabled, socket, token_file, actor).map(|_| ())
    }
    /// Delete every chat grant and skip the chat target's open rows.
    fn hook_drop(&self) -> Result<Value, String> {
        k2_core::tool_hook::drop_target_grants(k2_core::tool_hook::CHAT_KIND).map(|r| json!(r))
    }
}

/// A control-plane answer that was not 2xx (`status` 0 = unreachable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpError {
    pub status: u16,
    pub code: String,
    pub detail: String,
}

impl std::fmt::Display for CpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.status == 0 {
            write!(f, "K2 Connect is unreachable: {}", self.detail)
        } else {
            write!(f, "K2 Connect answered {} {}: {}", self.status, self.code, self.detail)
        }
    }
}

/// The control plane's base URL: the cert broker's (`K2_CERT_BROKER_URL`,
/// default `https://cert.k2.dev/cert`) without its `/cert` path.
pub fn cp_base_url() -> String {
    let b = k2_core::tunnel::cert_broker::broker_url();
    b.strip_suffix("/cert").unwrap_or(&b).trim_end_matches('/').to_string()
}

/// `PUT|DELETE|POST <base><path>` with `Authorization: Bearer <token>`, on
/// a thread with no ambient tokio runtime (reqwest blocking).
pub fn cp_http(method: &str, path: &str, token: &str, body: Option<&Value>) -> Result<Value, CpError> {
    let url = format!("{}{path}", cp_base_url());
    let (m, t, b) = (method.to_string(), token.to_string(), body.cloned());
    let joined = std::thread::Builder::new()
        .name("k2-sew-cp".into())
        .spawn(move || -> Result<(u16, String), String> {
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(90))
                .build()
                .map_err(|e| e.to_string())?;
            let method = reqwest::Method::from_bytes(m.as_bytes()).map_err(|e| e.to_string())?;
            let mut req = client.request(method, &url).header("Authorization", format!("Bearer {t}"));
            if let Some(b) = b {
                req = req.header("Content-Type", "application/json").body(b.to_string());
            }
            let resp = req.send().map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            Ok((status, resp.text().unwrap_or_default()))
        })
        .map_err(|e| CpError { status: 0, code: "spawn".into(), detail: e.to_string() })?
        .join()
        .map_err(|_| CpError { status: 0, code: "panic".into(), detail: "control-plane request thread panicked".into() })?;
    let (status, text) = joined.map_err(|e| CpError { status: 0, code: "unreachable".into(), detail: e })?;
    cp_answer(status, &text)
}

/// Map a control-plane answer: 2xx JSON, else `{error, detail}`.
pub fn cp_answer(status: u16, text: &str) -> Result<Value, CpError> {
    let v: Option<Value> = serde_json::from_str(text).ok();
    if (200..300).contains(&status) {
        return v.ok_or_else(|| CpError { status, code: "bad_answer".into(), detail: "answer is not JSON".into() });
    }
    let code = v.as_ref().and_then(|v| v.get("error")).and_then(Value::as_str).unwrap_or("http_error").to_string();
    let detail = v
        .as_ref()
        .and_then(|v| v.get("detail"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| text.chars().take(200).collect());
    Err(CpError { status, code, detail })
}

/// `~/.k2/sew/voice.json`.
pub fn voice_state_path() -> std::path::PathBuf {
    k2_core::paths::k2_home().join("sew").join("voice.json")
}

/// `~/.k2/sew/<name>.json` for [`SewOps::record_load`] (`name` is one of
/// K2's fixed record names, never input).
pub fn record_path(name: &str) -> std::path::PathBuf {
    k2_core::paths::k2_home().join("sew").join(format!("{name}.json"))
}

fn write_json_file(p: &std::path::Path, v: Option<&Value>) -> Result<(), String> {
    match v {
        Some(v) => {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("mkdir {}: {e}", d.display()))?;
            }
            k2_core::fs_atomic::atomic_write_mode(p, v.to_string().as_bytes(), 0o600).map_err(|e| format!("write {}: {e}", p.display()))
        }
        None => match std::fs::remove_file(p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("remove {}: {e}", p.display())),
        },
    }
}

// ── production ───────────────────────────────────────────────────────────

pub struct RealOps;

pub const MGMT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

impl SewOps for RealOps {
    fn linux(&self) -> bool {
        super::sew_supported()
    }
    fn arch(&self) -> String {
        std::env::consts::ARCH.to_string()
    }
    fn helper_state(&self) -> RootHelperState {
        crate::root_helper::state()
    }
    fn helper(&self, argv: &[String], stdin: Option<&[u8]>) -> Result<Value, String> {
        let out = crate::root_helper::call(argv, stdin)?;
        let text = String::from_utf8_lossy(&out);
        serde_json::from_str(text.trim()).map_err(|_| format!("root helper {}: answer is not JSON", argv.join(" ")))
    }
    fn unit(&self, unit: &str) -> UnitInfo {
        let out = std::process::Command::new("/usr/bin/systemctl")
            .args(["show", unit, "-p", "ActiveState,Result,ExecMainStatus,NRestarts,MemoryCurrent,MemoryMax"])
            .stdin(std::process::Stdio::null())
            .output();
        match out {
            Ok(o) if o.status.success() => parse_unit_show(&String::from_utf8_lossy(&o.stdout)),
            _ => UnitInfo { active: "unknown".into(), ..Default::default() },
        }
    }
    fn read_file(&self, path: &Path, cap: usize) -> Result<Vec<u8>, String> {
        use std::io::Read;
        let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let meta = f.metadata().map_err(|e| format!("{}: {e}", path.display()))?;
        if !meta.is_file() {
            return Err(format!("{} is not a regular file", path.display()));
        }
        let mut buf = Vec::new();
        f.take(cap as u64 + 1).read_to_end(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if buf.len() > cap {
            return Err(format!("{} is larger than {} bytes", path.display(), cap));
        }
        Ok(buf)
    }
    fn path_exists(&self, path: &str) -> bool {
        std::fs::symlink_metadata(path).is_ok()
    }
    fn path_probe(&self, path: &str) -> Option<bool> {
        match std::fs::symlink_metadata(path) {
            Ok(_) => Some(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
            // ENOTDIR: a parent is a file, so the path can't exist.
            Err(e) if e.raw_os_error() == Some(20) => Some(false),
            Err(_) => None,
        }
    }
    fn systemd_version(&self) -> Option<u32> {
        let o = std::process::Command::new("/usr/bin/systemctl").arg("--version").output().ok()?;
        parse_systemd_version(&String::from_utf8_lossy(&o.stdout))
    }
    fn pg_majors(&self) -> Vec<u32> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir("/usr/lib/postgresql") else { return out };
        for e in rd.flatten() {
            let Some(m) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            let bin = e.path().join("bin");
            if k2_root_proto::sew::SEW_PG_BINARIES.iter().all(|b| bin.join(b).is_file()) {
                out.push(m);
            }
        }
        out.sort_unstable();
        out
    }
    fn mem_total(&self) -> Option<u64> {
        parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
    }
    fn disk_free(&self, path: &str) -> Option<u64> {
        k2_core::fs_commands::free_disk_space(std::path::Path::new(path))
    }
    fn port_taken(&self, addr: &str, port: u16, udp: bool) -> bool {
        let sa = format!("{addr}:{port}");
        if udp {
            return matches!(std::net::UdpSocket::bind(&sa), Err(e) if e.kind() == std::io::ErrorKind::AddrInUse);
        }
        if port < 1024 {
            // The daemon user can't bind a low port: ask whether anything
            // answers there instead.
            let target = if addr == "0.0.0.0" { format!("127.0.0.1:{port}") } else { sa };
            return target
                .parse()
                .ok()
                .is_some_and(|a| std::net::TcpStream::connect_timeout(&a, std::time::Duration::from_millis(500)).is_ok());
        }
        matches!(std::net::TcpListener::bind(&sa), Err(e) if e.kind() == std::io::ErrorKind::AddrInUse)
    }
    fn daemon_port(&self) -> Option<u16> {
        Some(k2_core::hook_config::get_port()).filter(|p| *p != 0)
    }
    fn hostname(&self) -> String {
        std::fs::read_to_string("/proc/sys/kernel/hostname")
            .or_else(|_| std::fs::read_to_string("/etc/hostname"))
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_default()
    }
    fn mgmt(&self, method: &str, path: &str, token: &str, body: Option<&Value>) -> Result<Value, MgmtError> {
        mgmt_call(Path::new(k2_root_proto::sew::SEW_MGMT_SOCKET), method, path, token, body)
    }
    fn api_health(&self) -> Result<(u16, Value), String> {
        api_health_call(&client_addr())
    }
    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }
    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    fn box_label(&self) -> Option<String> {
        let primary = k2_core::tunnel::subdomains::current().primary.trim().to_ascii_lowercase();
        let label = if primary.is_empty() {
            k2_core::tunnel::config::load().ok()?.subdomain.trim().to_ascii_lowercase()
        } else {
            primary
        };
        let label = label.strip_suffix(".k2.dev").map(str::to_string).unwrap_or(label);
        (!label.is_empty() && !label.contains('.')).then_some(label)
    }
    fn bound_apexes(&self) -> Vec<String> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::domains::list_bindings(&conn)
            .map(|b| b.into_iter().map(|d| d.apex.to_ascii_lowercase()).collect())
            .unwrap_or_default()
    }
    fn public_ips(&self) -> Vec<String> {
        let (v4, v6) = crate::mail::cert_names::local_interface_addrs();
        v4.iter().map(ToString::to_string).chain(v6.iter().map(ToString::to_string)).collect()
    }
    fn cp(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, CpError> {
        let cfg = k2_core::tunnel::config::load()
            .map_err(|e| CpError { status: 0, code: "no_tunnel_config".into(), detail: e })?;
        if let Some(e) = cfg.token_read_error.as_deref() {
            return Err(CpError { status: 0, code: "vault_unreadable".into(), detail: e.to_string() });
        }
        let token = cfg.token.trim().to_string();
        if token.is_empty() {
            return Err(CpError { status: 0, code: "no_tunnel_token".into(), detail: "K2 Connect is not set up on this box".into() });
        }
        cp_http(method, path, &token, body)
    }
    fn nested_label(&self, label: &str, target: Option<&str>) -> Result<(), String> {
        if k2_core::airgap::enabled() {
            return Err("air-gapped: no K2 Connect nested labels".into());
        }
        let cfg = k2_core::tunnel::config::load()?;
        let token = cfg.token.trim();
        if token.is_empty() {
            return Err("K2 Connect is not set up on this box".into());
        }
        let owner = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::db::schema::SubdomainWorkspace::owner_of(&conn, label).map_err(|e| e.to_string())?
        };
        if owner.is_some() {
            return Err(format!("'{label}' is attributed to a workspace on this server; release it first (k2 publish subdomain unclaim {label})"));
        }
        match target {
            Some(t) => match k2_core::tunnel::subdomains::create_subdomain(token, label, t) {
                Ok(()) => Ok(()),
                Err(e) if e.contains("label_taken") => k2_core::tunnel::subdomains::point_subdomain(token, label, t),
                Err(e) => Err(e),
            },
            None => k2_core::tunnel::subdomains::delete_subdomain(token, label),
        }
    }
    fn served_leaf(&self, connect: &str, sni: &str, port: u16) -> Option<String> {
        match crate::domains::status::served_leaf_at(connect, sni, port) {
            crate::domains::status::Served::Leaf(s) => Some(s),
            crate::domains::status::Served::Nothing => None,
        }
    }
    fn front_serving(&self) -> bool {
        crate::front443::serving()
    }
    fn front_routes_changed(&self) {
        crate::front443::rebuild();
    }
    fn frpc_logins(&self) -> u64 {
        k2_core::tunnel::connector::frpc_login_count()
    }
    fn voice_load(&self) -> Option<Value> {
        let text = std::fs::read_to_string(voice_state_path()).ok()?;
        serde_json::from_str(&text).ok()
    }
    fn record_load(&self, name: &str) -> Option<Value> {
        let text = std::fs::read_to_string(record_path(name)).ok()?;
        serde_json::from_str(&text).ok()
    }
    fn record_save(&self, name: &str, v: Option<&Value>) -> Result<(), String> {
        write_json_file(&record_path(name), v)
    }
    fn voice_save(&self, v: Option<&Value>) -> Result<(), String> {
        let p = voice_state_path();
        match v {
            Some(v) => {
                if let Some(d) = p.parent() {
                    std::fs::create_dir_all(d).map_err(|e| format!("mkdir {}: {e}", d.display()))?;
                }
                k2_core::fs_atomic::atomic_write_mode(&p, v.to_string().as_bytes(), 0o600)
                    .map_err(|e| format!("write {}: {e}", p.display()))
            }
            None => match std::fs::remove_file(&p) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("remove {}: {e}", p.display())),
            },
        }
    }
}

pub fn parse_unit_show(text: &str) -> UnitInfo {
    let mut u = UnitInfo { active: "unknown".into(), ..Default::default() };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "ActiveState" if !v.is_empty() => u.active = v.to_string(),
            "Result" => u.result = v.to_string(),
            "ExecMainStatus" => u.exec_main_status = v.parse().unwrap_or(0),
            "NRestarts" => u.n_restarts = v.parse().unwrap_or(0),
            "MemoryCurrent" => u.memory_current = v.parse().ok(),
            "MemoryMax" => u.memory_max = v.parse().ok(),
            _ => {}
        }
    }
    u
}

pub fn parse_systemd_version(text: &str) -> Option<u32> {
    text.lines().next()?.split_whitespace().nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

pub fn parse_meminfo(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// One HTTP/1.1 request on Sew's mgmt socket (`Connection: close`).
/// Sew's client listener as `host:port`: the loopback URL S29 recorded
/// after enable, else `[server] listen` in `/etc/sew/sew.toml`, else the
/// default 127.0.0.1:8470.
pub fn client_addr() -> String {
    if let Some(u) = crate::chat_pass::link::client_url() {
        return u.trim().trim_start_matches("http://").trim_end_matches('/').to_string();
    }
    use k2_core::deploy::box_service as bs;
    let toml = std::fs::read_to_string(bs::SEW_CONFIG).ok();
    let port = bs::preset("sew-api").and_then(|p| bs::resolve_preset(p, toml.as_deref()).ok()).map(|r| r.port).unwrap_or(8470);
    format!("127.0.0.1:{port}")
}

/// One plain `GET /api/v1/health` to `addr` (loopback, 2 s timeouts): the
/// status and JSON body whatever the status (a 503 carries the reason).
pub fn api_health_call(addr: &str) -> Result<(u16, Value), String> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    let t = std::time::Duration::from_secs(2);
    let sa = addr
        .to_socket_addrs()
        .map_err(|e| format!("{addr}: {e}"))?
        .next()
        .ok_or_else(|| format!("{addr}: no address"))?;
    if !sa.ip().is_loopback() {
        return Err(format!("{addr} is not loopback; K2 probes Sew on this box only"));
    }
    let mut s = TcpStream::connect_timeout(&sa, t).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(t)).map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(t)).map_err(|e| e.to_string())?;
    s.write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: sew\r\nAccept: application/json\r\nConnection: close\r\n\r\n")
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.take(64 * 1024).read_to_end(&mut raw).map_err(|e| e.to_string())?;
    parse_status_and_body(&raw)
}

/// `(status, JSON body)` of a raw HTTP answer (any status; an empty or
/// non-JSON body reads as `{}`).
pub fn parse_status_and_body(raw: &[u8]) -> Result<(u16, Value), String> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("truncated HTTP answer")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut lines = head.lines();
    let code: u16 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .ok_or("bad HTTP status line")?;
    let chunked = lines.any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    let mut body = raw[split + 4..].to_vec();
    if chunked {
        body = dechunk(&body).ok_or("bad chunked body")?;
    }
    Ok((code, serde_json::from_slice(&body).unwrap_or_else(|_| json!({}))))
}

pub fn mgmt_call(sock: &Path, method: &str, path: &str, token: &str, body: Option<&Value>) -> Result<Value, MgmtError> {
    #[cfg(unix)]
    use std::io::{Read, Write};
    #[cfg(unix)]
    {
        let mut s = std::os::unix::net::UnixStream::connect(sock).map_err(|e| MgmtError::Unreachable(e.to_string()))?;
        s.set_read_timeout(Some(MGMT_TIMEOUT)).map_err(|e| MgmtError::Unreachable(e.to_string()))?;
        s.set_write_timeout(Some(MGMT_TIMEOUT)).map_err(|e| MgmtError::Unreachable(e.to_string()))?;
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: sew\r\nAuthorization: Bearer {token}\r\nConnection: close\r\nSew-Actor: k2\r\n");
        if method == "POST" {
            req.push_str(&format!("Idempotency-Key: k2s29_{}\r\n", idem()));
            req.push_str("Content-Type: application/json\r\n");
            req.push_str(&format!("Content-Length: {}\r\n", payload.len()));
        }
        req.push_str("\r\n");
        req.push_str(&payload);
        s.write_all(req.as_bytes()).map_err(|e| MgmtError::Unreachable(e.to_string()))?;
        let mut raw = Vec::new();
        s.take(8 * 1024 * 1024).read_to_end(&mut raw).map_err(|e| MgmtError::Unreachable(e.to_string()))?;
        parse_http_response(&raw)
    }
    #[cfg(not(unix))]
    {
        let _ = (sock, method, path, token, body);
        Err(MgmtError::Unreachable("not a unix box".into()))
    }
}

#[cfg(unix)]
fn idem() -> String {
    let mut b = [0u8; 12];
    let _ = getrandom::getrandom(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Status line + headers + body (Content-Length, chunked, or to EOF).
pub fn parse_http_response(raw: &[u8]) -> Result<Value, MgmtError> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| MgmtError::Unreachable("truncated HTTP answer".into()))?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut body = raw[split + 4..].to_vec();
    let mut lines = head.lines();
    let code: u16 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| MgmtError::Unreachable("bad HTTP status line".into()))?;
    let chunked = lines.clone().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    if chunked {
        body = dechunk(&body).ok_or_else(|| MgmtError::Unreachable("bad chunked body".into()))?;
    }
    if code == 401 {
        return Err(MgmtError::Unauthorized);
    }
    if !(200..300).contains(&code) {
        let b: String = String::from_utf8_lossy(&body).chars().take(400).collect();
        return Err(MgmtError::Status(code, b));
    }
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&body).map_err(|_| MgmtError::Status(code, "answer is not JSON".into()))
}

fn dechunk(mut b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let nl = b.windows(2).position(|w| w == b"\r\n")?;
        let size_txt = std::str::from_utf8(&b[..nl]).ok()?;
        let size = usize::from_str_radix(size_txt.split(';').next()?.trim(), 16).ok()?;
        b = &b[nl + 2..];
        if size == 0 {
            return Some(out);
        }
        if b.len() < size + 2 {
            return None;
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
    }
}

// ── test fake ────────────────────────────────────────────────────────────

#[cfg(test)]
pub mod fake {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    use super::*;
    use crate::root_helper::{Kind, RootHelperState};

    /// One recorded helper call.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Call {
        pub argv: String,
        pub stdin: Option<Vec<u8>>,
    }

    pub struct FakeOps {
        pub calls: Mutex<Vec<Call>>,
        /// argv (joined) → queued answers; the default answer is
        /// [`FakeOps::default_answer`].
        pub answers: Mutex<HashMap<String, VecDeque<Result<Value, String>>>>,
        pub units: Mutex<HashMap<String, UnitInfo>>,
        pub helper: Mutex<RootHelperState>,
        /// Version on disk (`engine sew status` → binary.version).
        /// `unknown` = a binary no table knows (sha [`UNKNOWN_SHA`]);
        /// `testbuild` = the upgrade-only fixture (item 12).
        pub on_disk: Mutex<Option<String>>,
        /// item 12: a pre-0.46.5 helper (no `upgrade.acceptBinary` in its
        /// status; it refuses the `upgrade accept-binary` argv).
        pub helper_no_accept: Mutex<bool>,
        pub mgmt_health_ok: Mutex<bool>,
        pub mgmt_status: Mutex<Value>,
        pub mgmt_calls: Mutex<Vec<String>>,
        /// "METHOD path" → queued mgmt answers (before the canned ones).
        pub mgmt_answers: Mutex<HashMap<String, VecDeque<Result<Value, MgmtError>>>>,
        /// SW15: the bundles `GET /mgmt/v1/backups` lists, newest first.
        pub mgmt_backups: Mutex<Vec<Value>>,
        pub files: Mutex<HashMap<String, Vec<u8>>>,
        pub exists: Mutex<Vec<String>>,
        /// Directories the daemon user may not enter (`path_probe` → `None`
        /// for anything under them).
        pub unreadable: Mutex<Vec<String>>,
        pub taken: Mutex<Vec<(u16, bool)>>,
        pub systemd: Mutex<Option<u32>>,
        pub pg: Mutex<Vec<u32>>,
        pub mem: Mutex<Option<u64>>,
        pub disk: Mutex<Option<u64>>,
        pub port: Mutex<Option<u16>>,
        pub clock: Mutex<i64>,
        pub linux: bool,
        /// Bodies of every mgmt call, in order (`None` = no body).
        pub mgmt_bodies: Mutex<Vec<Option<Value>>>,
        // voice (K13)
        pub label: Mutex<Option<String>>,
        pub apexes: Mutex<Vec<String>>,
        pub ips: Mutex<Vec<String>>,
        /// "METHOD path" of every control-plane call, with its body.
        pub cp_calls: Mutex<Vec<(String, Option<Value>)>>,
        /// "METHOD path" → queued control-plane answers (before the canned).
        pub cp_answers: Mutex<HashMap<String, VecDeque<Result<Value, CpError>>>>,
        /// The IPv4 the control plane observes for this box.
        pub cp_ip: Mutex<String>,
        /// Days the canned turn certificate is valid from `clock`.
        pub cert_days: Mutex<i64>,
        pub nested: Mutex<Vec<(String, Option<String>)>>,
        /// What the TURN TLS port serves: `Some(true)` = the last cert K2
        /// issued, `Some(false)` = another cert, `None` = nothing.
        pub serves_ours: Mutex<Option<bool>>,
        pub logins: Mutex<u64>,
        pub voice: Mutex<Option<Value>>,
        /// [`SewOps::record_load`] records by name.
        pub records: Mutex<HashMap<String, Value>>,
        /// The leaf sha256 of the last canned turn cert.
        pub last_leaf: Mutex<Option<String>>,
        /// `engine sew stage-media` happened (status reports it staged).
        pub media_staged: Mutex<bool>,
        /// sew-media versions the helper keeps (an older release's engine
        /// staged for a rollback, the pin's after a media install/upgrade).
        pub kept_media: Mutex<Vec<String>>,
        pub media_state: Mutex<Value>,
        /// [`SewOps::sew_released`] (true: the test-signer fixture pin).
        pub released: Mutex<bool>,
        /// [`SewOps::mgmt_socket`] (a fake Sew's socket in hook tests).
        pub mgmt_socket: Mutex<String>,
        /// Grant-hook calls, in order (`on <socket> <token file>`, `off`,
        /// `drop`). They reach the real `tool_hook` only with `real_hook`
        /// (a test that holds a temp HOME); otherwise they are recorded.
        pub hook_calls: Mutex<Vec<String>>,
        pub real_hook: bool,
        /// [`SewOps::front_serving`].
        pub front: Mutex<bool>,
        /// [`SewOps::passthrough_supported`] (true: as if Sew were 0.2.1+).
        pub passthrough: Mutex<bool>,
        /// Every port the served-leaf check probed.
        pub served_ports: Mutex<Vec<u16>>,
        /// [`SewOps::front_routes_changed`] calls (front443 table rebuilds).
        pub front_rebuilds: Mutex<u32>,
        /// Queued `/api/v1/health` answers (default: 200 ready).
        pub api_health_q: Mutex<VecDeque<Result<(u16, Value), String>>>,
        pub api_health_n: Mutex<usize>,
    }

    pub const GIB: u64 = 1024 * 1024 * 1024;
    pub const INSTALL_ID: &str = "48831765147549701";
    pub const TOKEN: &str = "sewsvc_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCdE";
    /// item 12: the sha256 of the fake's `unknown` binary.
    pub const UNKNOWN_SHA: &str = "0f0e0d0c0b0a09080706050403020100f0e0d0c0b0a09080706050403020100f";
    /// item 12: `on_disk` for the upgrade-only fixture build.
    pub const TESTBUILD: &str = "testbuild";

    pub fn helper_ok(protocol: u32, sew_ns: bool, pin: &str) -> RootHelperState {
        RootHelperState {
            kind: Kind::Root,
            version: Some("0.46.1".into()),
            protocol,
            path: k2_root_proto::paths::HELPER_PATH.into(),
            package_owned: false,
            version_json: Some(json!({"protocol": protocol, "namespaces": {"sew": sew_ns}, "sewPin": pin})),
            error: None,
        }
    }

    impl FakeOps {
        /// A healthy, prepared Ubuntu 24.04 box with the root helper at
        /// protocol 7, nothing of Sew on it yet.
        pub fn new() -> Self {
            let pin = k2_root_proto::sew::pinned().version;
            FakeOps {
                calls: Mutex::new(vec![]),
                answers: Mutex::new(HashMap::new()),
                units: Mutex::new(HashMap::new()),
                helper: Mutex::new(helper_ok(7, true, pin)),
                on_disk: Mutex::new(None),
                helper_no_accept: Mutex::new(false),
                mgmt_health_ok: Mutex::new(true),
                api_health_q: Mutex::new(VecDeque::new()),
                api_health_n: Mutex::new(0),
                mgmt_status: Mutex::new(json!({
                    "provider": "k2",
                    "version": format!("{pin}+g1234567"),
                    "install_id": INSTALL_ID,
                    "db": {"ok": true},
                    "alarms": [],
                    "keys": {"rewrap_pending": 0, "retiring_kek_ids": []},
                    "identity": {"provider": "k2"},
                    "client_listen": "127.0.0.1:8470",
                })),
                mgmt_calls: Mutex::new(vec![]),
                mgmt_answers: Mutex::new(HashMap::new()),
                mgmt_backups: Mutex::new(vec![
                    json!({"id": "20260102T031500Z", "kind": "scheduled", "created_at": "2026-01-02T03:15:00Z", "bytes": 120, "schema_version": 920002, "sew_version": pin}),
                    json!({"id": "20260101T031500Z", "kind": "scheduled", "created_at": "2026-01-01T03:15:00Z", "bytes": 100, "schema_version": 920002, "sew_version": pin}),
                ]),
                // A fresh box: no `sew` user yet.
                files: Mutex::new(HashMap::from([(
                    crate::sew::service_user::PASSWD.to_string(),
                    b"root:x:0:0:root:/root:/bin/bash\nk2:x:1001:1001::/home/k2:/bin/bash\n".to_vec(),
                )])),
                exists: Mutex::new(vec![]),
                unreadable: Mutex::new(vec![]),
                taken: Mutex::new(vec![]),
                systemd: Mutex::new(Some(255)),
                pg: Mutex::new(vec![16]),
                mem: Mutex::new(Some(64 * GIB)),
                disk: Mutex::new(Some(500 * GIB)),
                port: Mutex::new(Some(38471)),
                clock: Mutex::new(1_800_000_000),
                linux: true,
                mgmt_bodies: Mutex::new(vec![]),
                label: Mutex::new(Some("lztek".into())),
                apexes: Mutex::new(vec!["acme.dev".into()]),
                ips: Mutex::new(vec!["203.0.113.7".into()]),
                cp_calls: Mutex::new(vec![]),
                cp_answers: Mutex::new(HashMap::new()),
                cp_ip: Mutex::new("203.0.113.7".into()),
                cert_days: Mutex::new(90),
                nested: Mutex::new(vec![]),
                serves_ours: Mutex::new(Some(true)),
                logins: Mutex::new(1),
                voice: Mutex::new(None),
                records: Mutex::new(HashMap::new()),
                last_leaf: Mutex::new(None),
                media_staged: Mutex::new(false),
                kept_media: Mutex::new(Vec::new()),
                media_state: Mutex::new(json!({"installed": true, "enabled": true, "state": "running", "alarms": ["port_443_unavailable"]})),
                released: Mutex::new(true),
                mgmt_socket: Mutex::new(k2_root_proto::sew::SEW_MGMT_SOCKET.to_string()),
                hook_calls: Mutex::new(vec![]),
                real_hook: false,
                front: Mutex::new(false),
                passthrough: Mutex::new(true),
                served_ports: Mutex::new(vec![]),
                front_rebuilds: Mutex::new(0),
            }
        }

        pub fn queue_cp(&self, call: &str, answer: Result<Value, CpError>) {
            self.cp_answers.lock().unwrap().entry(call.to_string()).or_default().push_back(answer);
        }

        pub fn cp_log(&self) -> Vec<String> {
            self.cp_calls.lock().unwrap().iter().map(|(c, _)| c.clone()).collect()
        }

        /// A certificate the fake control plane signs for `csr_pem` (rcgen,
        /// self-signed by a throwaway CA; the SAN and key are the CSR's).
        pub fn sign_csr(&self, csr_pem: &str) -> String {
            use chrono::Datelike;
            use x509_parser::extensions::{GeneralName, ParsedExtension};
            use x509_parser::prelude::FromDer;
            struct Foreign(Vec<u8>);
            impl rcgen::PublicKeyData for Foreign {
                fn der_bytes(&self) -> &[u8] {
                    &self.0
                }
                fn algorithm(&self) -> &rcgen::SignatureAlgorithm {
                    &rcgen::PKCS_ECDSA_P256_SHA256
                }
            }
            let mut reader = csr_pem.as_bytes();
            let (pem, _) = x509_parser::pem::Pem::read(std::io::Cursor::new(&mut reader)).expect("CSR PEM");
            let (_, csr) = x509_parser::certification_request::X509CertificationRequest::from_der(&pem.contents).expect("CSR DER");
            let info = &csr.certification_request_info;
            let raw = info.subject_pki.subject_public_key.data.to_vec();
            let mut sans = Vec::new();
            for ext in csr.requested_extensions().into_iter().flatten() {
                if let ParsedExtension::SubjectAlternativeName(san) = ext {
                    for n in &san.general_names {
                        if let GeneralName::DNSName(d) = n {
                            sans.push(d.to_string());
                        }
                    }
                }
            }
            let ca_key = rcgen::KeyPair::generate().unwrap();
            let mut ca_params = rcgen::CertificateParams::new(vec!["fake-ca.k2.dev".to_string()]).unwrap();
            ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
            let ca = ca_params.self_signed(&ca_key).unwrap();
            let now = *self.clock.lock().unwrap();
            let days = *self.cert_days.lock().unwrap();
            let ymd = |t: i64| {
                let d = chrono::DateTime::from_timestamp(t, 0).unwrap().date_naive();
                rcgen::date_time_ymd(d.year(), d.month() as u8, d.day() as u8)
            };
            let mut params = rcgen::CertificateParams::new(sans).unwrap();
            params.not_before = ymd(now - 86_400);
            params.not_after = ymd(now + days * 86_400);
            let leaf = params.signed_by(&Foreign(raw), &ca, &ca_key).unwrap();
            let chain = format!("{}{}", leaf.pem(), ca.pem());
            *self.last_leaf.lock().unwrap() = crate::domains::status::pem_leaf_sha256(&chain);
            chain
        }

        pub fn queue(&self, argv: &str, answer: Result<Value, String>) {
            self.answers.lock().unwrap().entry(argv.to_string()).or_default().push_back(answer);
        }

        pub fn argvs(&self) -> Vec<String> {
            self.calls.lock().unwrap().iter().map(|c| c.argv.clone()).collect()
        }

        /// Helper calls minus the read-only `engine sew status`.
        pub fn effects(&self) -> Vec<String> {
            self.argvs().into_iter().filter(|a| a != "engine sew status").collect()
        }

        pub fn clear(&self) {
            self.calls.lock().unwrap().clear();
            self.mgmt_calls.lock().unwrap().clear();
        }

        pub fn set_unit(&self, unit: &str, active: &str, exec_main_status: i64) {
            self.units.lock().unwrap().insert(
                unit.to_string(),
                UnitInfo { active: active.into(), exec_main_status, ..Default::default() },
            );
        }

        fn sew_exit(code: i64, sew: Value) -> Value {
            json!({"sewExit": code, "sew": sew, "stderrTail": ""})
        }

        /// What a real box answers, and the systemd state it leaves.
        fn default_answer(&self, argv: &str, stdin: Option<&[u8]>) -> Result<Value, String> {
            use k2_root_proto::sew::*;
            let pin = pinned().version;
            let active = |u: &str| self.set_unit(u, "active", 0);
            let inactive = |u: &str| self.set_unit(u, "inactive", 0);
            Ok(match argv {
                "engine sew status" => {
                    let units = self.units.lock().unwrap();
                    let st = |u: &str| units.get(u).map(|i| i.active.clone()).unwrap_or_else(|| "inactive".into());
                    let v = self.on_disk.lock().unwrap().clone();
                    json!({
                        "pin": {"version": pin, "placeholder": false},
                        "upgrade": if *self.helper_no_accept.lock().unwrap() { Value::Null } else { json!({"upgradeOnly": true, "acceptBinary": true}) },
                        "binary": match &v {
                            None => json!({"present": false}),
                            Some(v) if v == "unknown" => json!({"present": true, "known": false, "version": null, "sha256": UNKNOWN_SHA}),
                            Some(v) if v == TESTBUILD => {
                                let b = &upgrade_only_builds()[0];
                                json!({"present": true, "known": false, "version": null, "sha256": b.binary_sha256,
                                    "upgradeOnly": {"version": b.version, "label": b.label, "schema": b.schema}})
                            }
                            // What the real helper answers for a binary it
                            // refuses to hash (owner / mode).
                            Some(v) if v == "not-root" => json!({"present": true, "known": false,
                                "error": "sew helper: /usr/local/bin/sew is not root-owned or is writable by others"}),
                            Some(v) => json!({"present": true, "known": true, "version": v}),
                        },
                        "staged": {"keptMedia": self.kept_media.lock().unwrap().clone(), "core": null, "media": if *self.media_staged.lock().unwrap() {
                            k2_root_proto::sew::pinned().media.as_ref().map(|m| json!(m.version)).unwrap_or(Value::Null)
                        } else { Value::Null }},
                        "units": {"sewDb": st(UNIT_DB), "sew": st(UNIT_SEW)},
                    })
                }
                "engine sew stage" => json!({"staged": pin}),
                "engine sew stage-media" => {
                    // An older release's engine is kept, as the real helper does.
                    let bytes = stdin.unwrap_or_default();
                    let older = releases()
                        .iter()
                        .filter(|r| r.version != pin)
                        .filter_map(|r| r.media.as_ref())
                        .find(|m| m.artifacts.iter().any(|a| crate::update_routes::verify_sha256(bytes, a.tarball_sha256)))
                        .map(|m| m.version);
                    if let Some(v) = older {
                        self.kept_media.lock().unwrap().push(v.to_string());
                        return Ok(json!({"kept": v, "kind": "media-previous"}));
                    }
                    *self.media_staged.lock().unwrap() = true;
                    json!({"staged": pin, "kind": "media"})
                }
                "engine sew media install" => {
                    active(UNIT_MEDIA);
                    Self::sew_exit(0, json!({"changed": ["sew-media"], "warnings": []}))
                }
                "engine sew media reconfigure" | "engine sew media reconfigure restart-now" => {
                    Self::sew_exit(0, json!({"changed": ["/etc/sew/sew.toml [voice]"]}))
                }
                "engine sew media turn-cert" => Self::sew_exit(0, json!({"changed": ["turn-tls/fullchain.pem"]})),
                "engine sew media status" => Self::sew_exit(0, self.media_state.lock().unwrap().clone()),
                "engine sew media rollback" => {
                    let core = self.on_disk.lock().unwrap().clone().unwrap_or_default();
                    match media_artifact_of(&core, "x86_64") {
                        Ok((mv, _)) if self.kept_media.lock().unwrap().iter().any(|k| k == mv) => {
                            let mut v = Self::sew_exit(0, json!({"changed": ["/usr/local/lib/sew-media/sew-media"]}));
                            v["to"] = json!(mv);
                            v
                        }
                        _ => return Err(format!("root helper engine sew media rollback: exit 4: media rollback refused: not kept (core {core})")),
                    }
                }
                "engine sew media upgrade" => {
                    let to = pinned().media.as_ref().map(|m| m.version).unwrap_or(pin);
                    Self::sew_exit(0, json!({"changed": ["/usr/local/lib/sew-media/sew-media"], "to": to, "mode": "now"}))
                }
                "engine sew media uninstall" => {
                    inactive(UNIT_MEDIA);
                    Self::sew_exit(0, json!({"removed": ["sew-media"]}))
                }
                a if a.starts_with("engine sew media probe-record ") => Self::sew_exit(0, json!({"recorded": true})),
                "engine sew config" => {
                    // Sew reports the JWKS URL it was set to (status.identity).
                    let req: Value = stdin.and_then(|b| serde_json::from_slice(b).ok()).unwrap_or(Value::Null);
                    if let Some(u) = req.get("identity.k2.jwks_url").and_then(Value::as_str) {
                        self.mgmt_status.lock().unwrap()["identity"]["jwks_url"] = json!(u);
                    }
                    json!({"sewExit": 0, "results": []})
                }
                "engine sew unstage" => json!({"removed": []}),
                "engine sew install-bin" => {
                    *self.on_disk.lock().unwrap() = Some(pin.into());
                    json!({"installed": pin, "changed": true})
                }
                "engine sew install" => {
                    let mut v = Self::sew_exit(0, json!({"versions": {"sew": pin}}));
                    let up = self.unit(UNIT_SEW).is_active();
                    v["allowUid"] = json!({"uid": 1001, "restartNeeded": up});
                    v
                }
                "engine sew systemctl enable" => json!({"ok": true}),
                "engine sew systemctl start-db" => {
                    active(UNIT_DB);
                    json!({"ok": true})
                }
                "engine sew systemctl migrate" => {
                    self.set_unit(UNIT_MIGRATE, "inactive", 0);
                    json!({"ok": true})
                }
                "engine sew systemctl start" | "engine sew systemctl restart" => {
                    active(UNIT_SEW);
                    json!({"ok": true})
                }
                "engine sew systemctl stop" => {
                    inactive(UNIT_SEW);
                    json!({"ok": true})
                }
                "engine sew systemctl disable-now" => {
                    inactive(UNIT_SEW);
                    inactive(UNIT_DB);
                    json!({"ok": true})
                }
                "engine sew token" => json!({"token": TOKEN}),
                "engine sew upgrade" => {
                    let from = self.on_disk.lock().unwrap().clone();
                    // The real helper never runs a binary it doesn't know.
                    if from.as_deref() == Some("unknown") {
                        return Err("root helper engine sew upgrade: exit 3: sew helper: unknown sew binary".into());
                    }
                    if from.as_deref() == Some(TESTBUILD) {
                        *self.on_disk.lock().unwrap() = Some(pin.into());
                        return Ok(Self::sew_exit(0, json!({"state": "upgraded", "from": upgrade_only_builds()[0].version, "to": pin})));
                    }
                    *self.on_disk.lock().unwrap() = Some(pin.into());
                    Self::sew_exit(0, json!({"state": "upgraded", "from": from, "to": pin}))
                }
                a if a.starts_with("engine sew upgrade accept-binary ") => {
                    if *self.helper_no_accept.lock().unwrap() {
                        return Err(format!("root helper {a}: exit 2: refused argv"));
                    }
                    let sha = a.rsplit(' ').next().unwrap_or("");
                    let disk = self.on_disk.lock().unwrap().clone();
                    let disk_sha = match disk.as_deref() {
                        Some("unknown") => UNKNOWN_SHA,
                        Some(TESTBUILD) => upgrade_only_builds()[0].binary_sha256,
                        _ => return Err(format!("root helper {a}: exit 3: accept-binary refused: a release K2 knows")),
                    };
                    if sha != disk_sha {
                        return Err(format!("root helper {a}: exit 3: accept-binary refused: /usr/local/bin/sew is sha256 {disk_sha}, not the {sha} you accepted"));
                    }
                    *self.on_disk.lock().unwrap() = Some(pin.into());
                    Self::sew_exit(0, json!({"state": "upgraded", "to": pin}))
                }
                "engine sew rollback" => Self::sew_exit(0, json!({"state": "rolled_back", "from": pin, "to": "0.0.1"})),
                "engine sew uninstall" => {
                    // The real helper never runs an unrecognised binary.
                    if self.on_disk.lock().unwrap().as_deref() == Some("unknown") {
                        return Err("root helper engine sew uninstall: exit 3: sew helper: unknown sew binary".into());
                    }
                    inactive(UNIT_SEW);
                    inactive(UNIT_DB);
                    Self::sew_exit(0, json!({"removed": ["units"]}))
                }
                a if a.starts_with("engine sew purge ") => {
                    let kept_at = "/var/lib/sew-backups-20260103T000000Z";
                    let keep = a.ends_with(" keep-backups");
                    let disk = self.on_disk.lock().unwrap().clone();
                    match disk.as_deref() {
                        // Missing or unrecognised: the 0.46.3 helper's own
                        // teardown (no sewExit: nothing of Sew's ran).
                        None | Some("unknown") => {
                            inactive(UNIT_SEW);
                            inactive(UNIT_DB);
                            inactive(UNIT_MEDIA);
                            *self.on_disk.lock().unwrap() = None;
                            json!({
                                "teardown": "k2",
                                "binary": if disk.is_some() { json!({"present": true, "known": false, "executed": false}) } else { json!({"present": false}) },
                                "installIdChecked": false,
                                "removed": [SEW_CONFIG_DIR, SEW_DATA_DIR, SEW_BIN],
                                "units": {"found": [UNIT_SEW, UNIT_DB]},
                                "sysctl": {"state": "restored"},
                                "accounts": [{"name": "sew", "loginLike": true, "reasons": ["uid 1001"]}],
                                "keptBackups": if keep { json!(kept_at) } else { Value::Null },
                                "left": [format!("{SEW_RECOVERY_FILE} (kept)")],
                            })
                        }
                        Some("not-root") => {
                            return Err("root helper engine sew purge: exit 3: sew helper: /usr/local/bin/sew is not root-owned or is writable by others".into())
                        }
                        _ => {
                            // Sew moves kept backups aside (sew-ops install.rs).
                            let mut v = Self::sew_exit(0, json!({"purged": true, "kept_backups": keep.then_some(kept_at)}));
                            v["teardown"] = json!("sew");
                            v
                        }
                    }
                }
                "engine sew doctor" => {
                    // Sew's media checks: the engine matches the installed core.
                    let core = self.on_disk.lock().unwrap().clone().unwrap_or_default();
                    let mv = media_artifact_of(&core, "x86_64").map(|(v, _)| v).unwrap_or("?");
                    Self::sew_exit(0, json!({"ok": true, "checks": [
                        {"id": "media.binary", "status": "ok", "detail": "matches the pin", "hint": ""},
                        {"id": "media.version", "status": "ok", "detail": format!("sew-media {mv}"), "hint": ""},
                    ]}))
                }
                "engine sew rotate-master" => Self::sew_exit(0, json!({"restarted": true})),
                "engine sew drop-prev" => Self::sew_exit(0, json!({"dropped": true})),
                a if a.starts_with("engine sew restore ") => {
                    // The helper always starts sew again.
                    active(UNIT_SEW);
                    let mut v = Self::sew_exit(0, json!({"restored": true, "safety_bundle": "20260103T000000Z-safety"}));
                    v["restarted"] = json!(true);
                    v
                }
                other => return Err(format!("fake helper: no answer for `{other}`")),
            })
        }
    }

    impl FakeOps {
        /// Queue `/api/v1/health` answers (then the default: ready).
        pub fn queue_api_health(&self, v: Vec<Result<(u16, Value), String>>) {
            self.api_health_q.lock().unwrap().extend(v);
        }
        pub fn api_health_calls(&self) -> usize {
            *self.api_health_n.lock().unwrap()
        }
    }

    impl SewOps for FakeOps {
        fn sew_released(&self) -> bool {
            *self.released.lock().unwrap()
        }
        fn mgmt_socket(&self) -> String {
            self.mgmt_socket.lock().unwrap().clone()
        }
        fn hook_targets(&self) -> Result<Vec<String>, String> {
            if self.real_hook {
                return Ok(k2_core::tool_hook::list_targets()?.into_iter().map(|t| t.kind).collect());
            }
            let calls = self.hook_calls.lock().unwrap();
            Ok(if calls.iter().any(|c| c.starts_with("on ")) { vec![k2_core::tool_hook::CHAT_KIND.to_string()] } else { vec![] })
        }
        fn hook_set(&self, enabled: bool, socket: Option<&str>, token_file: Option<&str>, actor: &str) -> Result<(), String> {
            self.hook_calls.lock().unwrap().push(if enabled {
                format!("on {} {}", socket.unwrap_or("-"), token_file.unwrap_or("-"))
            } else {
                "off".to_string()
            });
            if self.real_hook {
                k2_core::tool_hook::set_target(k2_core::tool_hook::CHAT_KIND, enabled, socket, token_file, actor)?;
            }
            Ok(())
        }
        fn hook_drop(&self) -> Result<Value, String> {
            self.hook_calls.lock().unwrap().push("drop".into());
            if self.real_hook {
                return k2_core::tool_hook::drop_target_grants(k2_core::tool_hook::CHAT_KIND).map(|r| json!(r));
            }
            Ok(json!({"grantsDeleted": 0, "rowsSkipped": 0}))
        }
        fn linux(&self) -> bool {
            self.linux
        }
        fn arch(&self) -> String {
            "x86_64".into()
        }
        fn helper_state(&self) -> RootHelperState {
            self.helper.lock().unwrap().clone()
        }
        fn helper(&self, argv: &[String], stdin: Option<&[u8]>) -> Result<Value, String> {
            let joined = argv.join(" ");
            // Defense in depth, like the real door: off-catalog argv never goes.
            let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
            k2_root_proto::verbs::Verb::parse(&refs).map_err(|e| format!("root helper: {e} (not sent)"))?;
            self.calls.lock().unwrap().push(Call { argv: joined.clone(), stdin: stdin.map(<[u8]>::to_vec) });
            let queued = self.answers.lock().unwrap().get_mut(&joined).and_then(VecDeque::pop_front);
            match queued {
                Some(a) => a,
                None => self.default_answer(&joined, stdin),
            }
        }
        fn unit(&self, unit: &str) -> UnitInfo {
            self.units
                .lock()
                .unwrap()
                .get(unit)
                .cloned()
                .unwrap_or(UnitInfo { active: "inactive".into(), ..Default::default() })
        }
        fn read_file(&self, path: &Path, cap: usize) -> Result<Vec<u8>, String> {
            let b = self
                .files
                .lock()
                .unwrap()
                .get(&path.display().to_string())
                .cloned()
                .ok_or_else(|| format!("{}: No such file or directory", path.display()))?;
            if b.len() > cap {
                return Err("too large".into());
            }
            Ok(b)
        }
        fn path_exists(&self, path: &str) -> bool {
            self.exists.lock().unwrap().iter().any(|p| p == path)
        }
        fn path_probe(&self, path: &str) -> Option<bool> {
            let hidden = self.unreadable.lock().unwrap().iter().any(|d| path.starts_with(&format!("{}/", d.trim_end_matches('/'))));
            if hidden {
                None
            } else {
                Some(self.path_exists(path))
            }
        }
        fn systemd_version(&self) -> Option<u32> {
            *self.systemd.lock().unwrap()
        }
        fn pg_majors(&self) -> Vec<u32> {
            self.pg.lock().unwrap().clone()
        }
        fn mem_total(&self) -> Option<u64> {
            *self.mem.lock().unwrap()
        }
        fn disk_free(&self, _path: &str) -> Option<u64> {
            *self.disk.lock().unwrap()
        }
        fn port_taken(&self, _addr: &str, port: u16, udp: bool) -> bool {
            self.taken.lock().unwrap().contains(&(port, udp))
        }
        fn daemon_port(&self) -> Option<u16> {
            *self.port.lock().unwrap()
        }
        fn hostname(&self) -> String {
            "lztek".into()
        }
        fn api_health(&self) -> Result<(u16, Value), String> {
            *self.api_health_n.lock().unwrap() += 1;
            self.api_health_q.lock().unwrap().pop_front().unwrap_or_else(|| Ok((200, json!({"ok": true, "ready": true}))))
        }
        fn mgmt(&self, method: &str, path: &str, token: &str, body: Option<&Value>) -> Result<Value, MgmtError> {
            self.mgmt_calls.lock().unwrap().push(format!("{method} {path}"));
            self.mgmt_bodies.lock().unwrap().push(body.cloned());
            if token != TOKEN {
                return Err(MgmtError::Unauthorized);
            }
            if !self.unit(k2_root_proto::sew::UNIT_SEW).is_active() {
                return Err(MgmtError::Unreachable("connection refused".into()));
            }
            let queued = self.mgmt_answers.lock().unwrap().get_mut(&format!("{method} {path}")).and_then(VecDeque::pop_front);
            if let Some(a) = queued {
                return a;
            }
            match path {
                // SW15: the backup job runs to completion at once.
                "/mgmt/v1/backup" if method == "POST" => {
                    let id = format!("20260103T{:06}Z", self.mgmt_backups.lock().unwrap().len());
                    let pin = k2_root_proto::sew::pinned().version;
                    self.mgmt_backups.lock().unwrap().insert(
                        0,
                        json!({"id": id, "kind": "manual", "created_at": "2026-01-03T00:00:00Z", "bytes": 140, "schema_version": 920002, "sew_version": pin}),
                    );
                    Ok(json!({"job_id": format!("bk_{id}")}))
                }
                "/mgmt/v1/backups" => Ok(json!({"items": *self.mgmt_backups.lock().unwrap()})),
                p if p.starts_with("/mgmt/v1/jobs/bk_") => {
                    let bundle = &p["/mgmt/v1/jobs/bk_".len()..];
                    Ok(json!({
                        "id": &p[14..], "kind": "backup", "state": "succeeded", "attempts": 1,
                        "created_at": "2026-01-03T00:00:00Z", "finished_at": "2026-01-03T00:00:01Z",
                        "result": {"id": bundle, "bytes": 140, "duration_ms": 900, "linked": 2, "copied": 1},
                    }))
                }
                "/mgmt/v1/health" => {
                    if *self.mgmt_health_ok.lock().unwrap() {
                        Ok(json!({"ok": true}))
                    } else {
                        Err(MgmtError::Status(503, "{\"ok\":false}".into()))
                    }
                }
                "/mgmt/v1/status" => {
                    let mut st = self.mgmt_status.lock().unwrap().clone();
                    if let Some(v) = self.on_disk.lock().unwrap().clone() {
                        st["version"] = json!(format!("{v}+g1234567"));
                    }
                    Ok(st)
                }
                "/mgmt/v1/keys/rotate-master" => Ok(json!({"job_id": "job_rot1"})),
                p if p.starts_with("/mgmt/v1/jobs/") => Ok(json!({"id": &p[14..], "state": "done"})),
                _ => Err(MgmtError::Status(404, "unknown_route".into())),
            }
        }
        fn now(&self) -> i64 {
            *self.clock.lock().unwrap()
        }
        fn sleep_ms(&self, ms: u64) {
            *self.clock.lock().unwrap() += (ms / 1000).max(1) as i64;
        }
        fn box_label(&self) -> Option<String> {
            self.label.lock().unwrap().clone()
        }
        fn bound_apexes(&self) -> Vec<String> {
            self.apexes.lock().unwrap().clone()
        }
        fn public_ips(&self) -> Vec<String> {
            self.ips.lock().unwrap().clone()
        }
        fn cp(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, CpError> {
            let call = format!("{method} {path}");
            self.cp_calls.lock().unwrap().push((call.clone(), body.cloned()));
            if let Some(a) = self.cp_answers.lock().unwrap().get_mut(&call).and_then(VecDeque::pop_front) {
                return a;
            }
            let label = self.label.lock().unwrap().clone().unwrap_or_default();
            if method == "PUT" && path == format!("/turn/dns/{label}") {
                let ip = body
                    .and_then(|b| b.get("ipv4"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| self.cp_ip.lock().unwrap().clone());
                return Ok(json!({"host": format!("turn-{label}.k2.dev"), "ipv4": ip, "ipv6": null, "proxied": false,
                    "ttl": 60, "verified_by": {"ipv4": "request_source"}, "changes": {"a": "created", "aaaa": null}}));
            }
            if method == "DELETE" && path == format!("/turn/dns/{label}") {
                return Ok(json!({"host": format!("turn-{label}.k2.dev"), "removed": ["A"]}));
            }
            if method == "POST" && path == "/cert" {
                let b = body.expect("cert body");
                assert_eq!(b["mode"], "turn");
                assert_eq!(b["subdomain"], label.as_str());
                return Ok(json!({"cert": self.sign_csr(b["csr"].as_str().unwrap())}));
            }
            Err(CpError { status: 404, code: "not_found".into(), detail: call })
        }
        fn nested_label(&self, label: &str, target: Option<&str>) -> Result<(), String> {
            self.nested.lock().unwrap().push((label.to_string(), target.map(str::to_string)));
            Ok(())
        }
        fn served_leaf(&self, _connect: &str, _sni: &str, port: u16) -> Option<String> {
            self.served_ports.lock().unwrap().push(port);
            match *self.serves_ours.lock().unwrap() {
                Some(true) => self.last_leaf.lock().unwrap().clone(),
                Some(false) => Some("0".repeat(64)),
                None => None,
            }
        }
        fn front_serving(&self) -> bool {
            *self.front.lock().unwrap()
        }
        fn front_routes_changed(&self) {
            *self.front_rebuilds.lock().unwrap() += 1;
        }
        fn passthrough_supported(&self) -> bool {
            *self.passthrough.lock().unwrap()
        }
        fn frpc_logins(&self) -> u64 {
            *self.logins.lock().unwrap()
        }
        fn voice_load(&self) -> Option<Value> {
            self.voice.lock().unwrap().clone()
        }
        fn voice_save(&self, v: Option<&Value>) -> Result<(), String> {
            *self.voice.lock().unwrap() = v.cloned();
            Ok(())
        }
        fn record_load(&self, name: &str) -> Option<Value> {
            self.records.lock().unwrap().get(name).cloned()
        }
        fn record_save(&self, name: &str, v: Option<&Value>) -> Result<(), String> {
            let mut r = self.records.lock().unwrap();
            match v {
                Some(v) => r.insert(name.to_string(), v.clone()),
                None => r.remove(name),
            };
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsers_read_systemd_proc_and_http() {
        let u = parse_unit_show("ActiveState=failed\nResult=exit-code\nExecMainStatus=78\nNRestarts=3\nMemoryCurrent=[not set]\nMemoryMax=2415919104\n");
        assert_eq!(u.active, "failed");
        assert_eq!(u.exec_main_status, 78);
        assert_eq!(u.n_restarts, 3);
        assert_eq!(u.memory_current, None);
        assert_eq!(u.memory_max, Some(2415919104));
        assert_eq!(parse_systemd_version("systemd 255 (255.4-1ubuntu8)\n+PAM"), Some(255));
        assert_eq!(parse_systemd_version("systemd 249 (249.11-0ubuntu3)"), Some(249));
        assert_eq!(parse_meminfo("MemTotal:       65777152 kB\nMemFree: 1 kB\n"), Some(65777152 * 1024));
        let ok = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"ok\":true}";
        assert_eq!(parse_http_response(ok).unwrap()["ok"], true);
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n4\r\n:12}\r\n0\r\n\r\n";
        assert_eq!(parse_http_response(chunked).unwrap()["a"], 12);
        assert_eq!(parse_http_response(b"HTTP/1.1 401 Unauthorized\r\n\r\n").unwrap_err(), MgmtError::Unauthorized);
        assert!(matches!(parse_http_response(b"HTTP/1.1 404 Not Found\r\n\r\n{\"error\":\"unknown_route\"}"), Err(MgmtError::Status(404, _))));
        assert!(parse_http_response(b"garbage").is_err());
    }

    /// The real mgmt client against a real unix socket: request shape
    /// (bearer, idempotency key on POST, Sew-Actor) and the 401 mapping.
    #[cfg(unix)]
    #[test]
    fn mgmt_call_speaks_http_over_a_real_unix_socket() {
        use std::io::{Read, Write};
        // Short path: unix socket names are capped near 104 bytes on macOS.
        let dir = std::path::PathBuf::from(format!("/tmp/k2sew-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("mgmt.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for answer in [
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}",
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 17\r\n\r\n{\"job_id\":\"j_1\"}\n",
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n",
            ] {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap();
                seen.push(String::from_utf8_lossy(&buf[..n]).to_string());
                s.write_all(answer.as_bytes()).unwrap();
            }
            seen
        });
        let tok = "sewsvc_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCdE";
        assert_eq!(mgmt_call(&sock, "GET", "/mgmt/v1/health", tok, None).unwrap()["ok"], true);
        assert_eq!(mgmt_call(&sock, "POST", "/mgmt/v1/keys/rotate-master", tok, Some(&json!({}))).unwrap()["job_id"], "j_1");
        assert_eq!(mgmt_call(&sock, "GET", "/mgmt/v1/status", "sewsvc_stale", None).unwrap_err(), MgmtError::Unauthorized);
        let seen = server.join().unwrap();
        assert!(seen[0].starts_with("GET /mgmt/v1/health HTTP/1.1\r\n"));
        assert!(seen[0].contains(&format!("Authorization: Bearer {tok}\r\n")));
        assert!(!seen[0].contains("Idempotency-Key"), "GETs carry none");
        assert!(seen[1].contains("Idempotency-Key: k2s29_"));
        assert!(seen[1].ends_with("\r\n\r\n{}"));
        assert!(seen.iter().all(|r| r.contains("Sew-Actor: k2\r\n")));
        assert!(matches!(mgmt_call(&dir.join("absent.sock"), "GET", "/mgmt/v1/health", tok, None), Err(MgmtError::Unreachable(_))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Linux-only: the real box probes the preflight reads.
    #[cfg(target_os = "linux")]
    #[test]
    fn real_probes_on_linux() {
        let ops = RealOps;
        assert!(ops.linux());
        assert!(ops.mem_total().is_some_and(|m| m > 0), "MemTotal");
        assert!(ops.disk_free("/").is_some(), "statvfs /");
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = tcp.local_addr().unwrap().port();
        assert!(ops.port_taken("127.0.0.1", port, false), "a bound TCP port is taken");
        drop(tcp);
        assert!(!ops.port_taken("127.0.0.1", port, false), "and free again");
        let udp = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let uport = udp.local_addr().unwrap().port();
        assert!(ops.port_taken("0.0.0.0", uport, true), "a bound UDP port is taken");
        assert!(!ops.hostname().is_empty());
        // A unit that can't exist reads as inactive/unknown, never active.
        assert!(!ops.unit("k2-sew-test-no-such-unit.service").is_active());
    }
}
