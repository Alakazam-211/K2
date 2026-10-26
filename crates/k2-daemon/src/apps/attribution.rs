//! Which published service made this owner-token call? (0.46.0 grace)
//!
//! Every published `--cmd` service leads its own session (`setsid`), so
//! everything it starts shares its session id. For a loopback request that
//! carries the owner token, the daemon finds the client socket in
//! `/proc/net/tcp{,6}` (local port = the request's peer port, remote port =
//! the daemon's), then the process holding that socket inode among the
//! processes whose session is a live service's. The hit is counted per
//! (service, route) in `app_owner_token_uses`, and refused when the service
//! runs in clean mode (how an App runs, opted into early).
//!
//! **Linux only.** On macOS nothing is attributed (Apps run walled on
//! Linux only); the warnings there rely on the code scan.
//! **Honest limit:** a child that calls `setsid` itself leaves the
//! service's session and isn't attributed; the scan still names any
//! `daemon.token` read in its code.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// How long a peer port's answer is reused (keep-alive connections send
/// many requests on one port).
const CACHE_TTL: Duration = Duration::from_secs(10);
const CACHE_MAX: usize = 4096;

type Cache = HashMap<u16, (Option<String>, Instant)>;

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What the dispatcher does with an owner-token call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Not from a published service (or not attributable): carry on.
    NotAnApp,
    /// From service `id`: counted; carry on (0.46.0 removes nothing).
    Counted { service_id: String },
    /// From a clean-mode service: refuse.
    Refuse { service_id: String, name: String },
}

/// The live `--cmd` services: session id (= the child's pid) → service id.
fn live_cmd_sessions() -> HashMap<i32, String> {
    let children = crate::publish_runtime::live_children();
    if children.is_empty() {
        return HashMap::new();
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut out = HashMap::new();
    for (project, name, pid) in children {
        if let Ok(Some(row)) = k2_core::published_services::get_by_project_name(&conn, &project, &name) {
            if row.kind != "skin" && row.cmd != k2_core::published_services::CMD_APP_SENTINEL {
                out.insert(pid, row.id);
            }
        }
    }
    out
}

/// Owner-token tracing on this box, for status and the CLI: `"on"` on
/// Linux; elsewhere it says it isn't available (Rosson 2026-10-16: Linux
/// only for now; never silence on a Mac).
pub const fn availability() -> &'static str {
    if cfg!(target_os = "linux") {
        "on"
    } else if cfg!(target_os = "macos") {
        "not available on macOS"
    } else {
        "not available on this OS (Linux only)"
    }
}

/// Said next to [`availability`] when tracing is off: what still works.
pub const UNAVAILABLE_NOTE: &str = "owner-token calls are traced on Linux only for now; \
the code scan still names any read of ~/.k2/daemon.token; clean mode still cleans the environment but can't refuse \
owner-token calls it can't trace";

/// Cheap, non-blocking: could this call come from a published service at
/// all? (Linux, loopback, at least one live child.) The dispatcher skips
/// the blocking lookup otherwise.
pub fn worth_checking(peer: SocketAddr) -> bool {
    cfg!(target_os = "linux") && peer.ip().is_loopback() && !crate::publish_runtime::live_children().is_empty()
}

/// Attribute (and count) an owner-token call that arrived on `local` from
/// `peer`. Blocking (reads `/proc`): call from a blocking task.
pub fn owner_token_call(peer: SocketAddr, local: SocketAddr, path: &str) -> Verdict {
    if !peer.ip().is_loopback() {
        return Verdict::NotAnApp;
    }
    let now = Instant::now();
    let cached = {
        let c = cache().lock();
        c.get(&peer.port()).filter(|(_, at)| now.duration_since(*at) < CACHE_TTL).map(|(v, _)| v.clone())
    };
    let service = match cached {
        Some(v) => v,
        None => {
            let sessions = live_cmd_sessions();
            if sessions.is_empty() {
                return Verdict::NotAnApp;
            }
            let v = os::find_service(peer.port(), local.port(), &sessions);
            let mut c = cache().lock();
            if c.len() >= CACHE_MAX {
                c.retain(|_, (_, at)| now.duration_since(*at) < CACHE_TTL);
            }
            c.insert(peer.port(), (v.clone(), now));
            v
        }
    };
    let Some(service_id) = service else { return Verdict::NotAnApp };
    let clean = crate::vault::publish_env::is_clean_mode(&service_id);
    record(&service_id, path, clean);
    if clean {
        let name = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::published_services::get_by_id(&conn, &service_id).ok().flatten().map(|r| r.name).unwrap_or_default()
        };
        return Verdict::Refuse { service_id, name };
    }
    Verdict::Counted { service_id }
}

fn record(service_id: &str, path: &str, refused: bool) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = k2_core::app_grace::token_uses::record(&conn, service_id, path, refused, k2_core::deploy::now()) {
        k2_core::log_debug!("[apps] owner-token use for {service_id}: {e}");
    }
    k2_core::log_debug!(
        "[apps] owner token used by published service {service_id} on {}{}",
        k2_core::app_grace::token_uses::clean_route(path),
        if refused { " (refused: clean mode)" } else { "" }
    );
}

/// The refusal body for a clean-mode service.
pub fn refusal_body(service_id: &str, name: &str) -> String {
    serde_json::json!({
        "error": {
            "code": "owner_token_from_app",
            "hint": format!(
                "{name} runs in clean mode (how an App runs): Apps can't use the owner token. \
                 Use the App key K2 granted it ($K2_SECRETS_DIR/K2_APP_KEY) on /v1/apps/$K2_APP_ID/…, \
                 or turn clean mode off: k2 app clean-mode {name} off"
            ),
            "app": service_id,
        }
    })
    .to_string()
}

#[cfg(target_os = "linux")]
mod os {
    use std::collections::HashMap;

    /// The inode of the client end of a loopback TCP connection: a row
    /// whose local port is `client_port` and whose remote port is
    /// `server_port`.
    pub(super) fn client_inode(table: &str, client_port: u16, server_port: u16) -> Option<u64> {
        for line in table.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 {
                continue;
            }
            let port = |a: &str| a.rsplit_once(':').and_then(|(_, p)| u16::from_str_radix(p, 16).ok());
            if port(f[1]) == Some(client_port) && port(f[2]) == Some(server_port) {
                return f[9].parse().ok().filter(|i| *i != 0);
            }
        }
        None
    }

    fn session_of(pid: &str) -> Option<i32> {
        let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // After the `(comm)` field: state ppid pgrp session …
        let rest = &s[s.rfind(')')? + 1..];
        rest.split_whitespace().nth(3)?.parse().ok()
    }

    fn holds_inode(pid: &str, needle: &str) -> bool {
        let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { return false };
        rd.flatten().any(|e| std::fs::read_link(e.path()).is_ok_and(|l| l.as_os_str() == needle))
    }

    pub(super) fn find_service(client_port: u16, server_port: u16, sessions: &HashMap<i32, String>) -> Option<String> {
        let inode = ["/proc/net/tcp", "/proc/net/tcp6"]
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .find_map(|t| client_inode(&t, client_port, server_port))?;
        let needle = format!("socket:[{inode}]");
        let rd = std::fs::read_dir("/proc").ok()?;
        for e in rd.flatten() {
            let name = e.file_name();
            let pid = name.to_string_lossy();
            if !pid.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let Some(sid) = session_of(&pid) else { continue };
            let Some(service) = sessions.get(&sid) else { continue };
            if holds_inode(&pid, &needle) {
                return Some(service.clone());
            }
        }
        None
    }
}

#[cfg(not(target_os = "linux"))]
mod os {
    use std::collections::HashMap;

    pub(super) fn find_service(_client_port: u16, _server_port: u16, _sessions: &HashMap<i32, String>) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_attributed_without_live_services_or_off_loopback() {
        let lan: SocketAddr = "192.168.1.9:50000".parse().unwrap();
        let local: SocketAddr = "127.0.0.1:4000".parse().unwrap();
        assert_eq!(owner_token_call(lan, local, "/cli/thread"), Verdict::NotAnApp);
    }

    #[test]
    fn tracing_is_linux_only_and_a_mac_says_so() {
        // Rosson 2026-10-16: Linux only for now; never silence elsewhere.
        if cfg!(target_os = "linux") {
            assert_eq!(availability(), "on");
        } else if cfg!(target_os = "macos") {
            assert_eq!(availability(), "not available on macOS");
            let peer: SocketAddr = "127.0.0.1:50000".parse().unwrap();
            assert!(!worth_checking(peer), "nothing is traced on a Mac");
        } else {
            assert!(availability().starts_with("not available"), "{}", availability());
        }
        assert!(UNAVAILABLE_NOTE.contains("Linux only"), "{UNAVAILABLE_NOTE}");
    }

    #[test]
    fn refusal_names_the_app_key_and_the_way_back() {
        let b = refusal_body("svc-1", "jobs");
        let v: serde_json::Value = serde_json::from_str(&b).unwrap();
        assert_eq!(v["error"]["code"], "owner_token_from_app");
        let hint = v["error"]["hint"].as_str().unwrap();
        assert!(hint.contains("K2_APP_KEY") && hint.contains("k2 app clean-mode jobs off"), "{hint}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proc_net_tcp_rows_find_the_client_end() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:0FA0 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 11111 1 0 20 4 30 10 -1\n\
   1: 0100007F:C350 0100007F:0FA0 01 00000000:00000000 00:00000000 00000000  1000        0 22222 1 0 20 4 30 10 -1\n";
        // 0x0FA0 = 4000 (daemon), 0xC350 = 50000 (client).
        assert_eq!(os::client_inode(table, 50000, 4000), Some(22222));
        assert_eq!(os::client_inode(table, 50001, 4000), None);
    }

    /// Linux: a real child in its own session connects to a real listener;
    /// the attribution finds the child's service through /proc.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_child_in_a_service_session_is_found() {
        use std::io::Read;
        use std::os::unix::process::CommandExt;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let mut cmd = std::process::Command::new("python3");
        cmd.arg("-c").arg(format!(
            "import socket,time\ns=socket.create_connection(('127.0.0.1',{port}))\ntime.sleep(5)\n"
        ));
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = cmd.spawn().expect("python3 on the Linux test box");
        let (mut conn, peer) = l.accept().unwrap();
        let sessions: HashMap<i32, String> = [(child.id() as i32, "svc-linux".to_string())].into_iter().collect();
        let found = os::find_service(peer.port(), port, &sessions);
        let other: HashMap<i32, String> = [(1, "init".to_string())].into_iter().collect();
        let not_found = os::find_service(peer.port(), port, &other);
        let _ = child.kill();
        let _ = child.wait();
        let mut b = [0u8; 1];
        let _ = conn.read(&mut b);
        assert_eq!(found.as_deref(), Some("svc-linux"));
        assert_eq!(not_found, None, "another session is not this service");
    }
}
