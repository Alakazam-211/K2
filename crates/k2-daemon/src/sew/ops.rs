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
            if ["postgres", "initdb", "pg_dump", "pg_restore"].iter().all(|b| bin.join(b).is_file()) {
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
        let c = std::ffi::CString::new(path).ok()?;
        // SAFETY: valid C string; statvfs fills the zeroed struct.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        Some(st.f_bavail as u64 * st.f_frsize as u64)
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
    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }
    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
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
pub fn mgmt_call(sock: &Path, method: &str, path: &str, token: &str, body: Option<&Value>) -> Result<Value, MgmtError> {
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
        pub on_disk: Mutex<Option<String>>,
        pub mgmt_health_ok: Mutex<bool>,
        pub mgmt_status: Mutex<Value>,
        pub mgmt_calls: Mutex<Vec<String>>,
        pub files: Mutex<HashMap<String, Vec<u8>>>,
        pub exists: Mutex<Vec<String>>,
        pub taken: Mutex<Vec<(u16, bool)>>,
        pub systemd: Mutex<Option<u32>>,
        pub pg: Mutex<Vec<u32>>,
        pub mem: Mutex<Option<u64>>,
        pub disk: Mutex<Option<u64>>,
        pub port: Mutex<Option<u16>>,
        pub clock: Mutex<i64>,
        pub linux: bool,
    }

    pub const GIB: u64 = 1024 * 1024 * 1024;
    pub const INSTALL_ID: &str = "48831765147549701";
    pub const TOKEN: &str = "sewsvc_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCdE";

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
                mgmt_health_ok: Mutex::new(true),
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
                files: Mutex::new(HashMap::new()),
                exists: Mutex::new(vec![]),
                taken: Mutex::new(vec![]),
                systemd: Mutex::new(Some(255)),
                pg: Mutex::new(vec![16]),
                mem: Mutex::new(Some(64 * GIB)),
                disk: Mutex::new(Some(500 * GIB)),
                port: Mutex::new(Some(38471)),
                clock: Mutex::new(1_800_000_000),
                linux: true,
            }
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
        fn default_answer(&self, argv: &str) -> Result<Value, String> {
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
                        "binary": match &v {
                            None => json!({"present": false}),
                            Some(v) if v == "unknown" => json!({"present": true, "known": false, "version": null}),
                            Some(v) => json!({"present": true, "known": true, "version": v}),
                        },
                        "staged": {"core": null},
                        "units": {"sewDb": st(UNIT_DB), "sew": st(UNIT_SEW)},
                    })
                }
                "engine sew stage" => json!({"staged": pin}),
                "engine sew unstage" => json!({"removed": []}),
                "engine sew install-bin" => {
                    *self.on_disk.lock().unwrap() = Some(pin.into());
                    json!({"installed": pin, "changed": true})
                }
                "engine sew install" => {
                    let mut v = Self::sew_exit(0, json!({"versions": {"sew": pin}}));
                    v["allowUid"] = json!({"uid": 1001, "changed": true});
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
                    *self.on_disk.lock().unwrap() = Some(pin.into());
                    Self::sew_exit(0, json!({"state": "upgraded", "from": from, "to": pin}))
                }
                "engine sew rollback" => Self::sew_exit(0, json!({"state": "rolled_back", "from": pin, "to": "0.0.1"})),
                "engine sew uninstall" => {
                    inactive(UNIT_SEW);
                    inactive(UNIT_DB);
                    Self::sew_exit(0, json!({"removed": ["units"]}))
                }
                a if a.starts_with("engine sew purge ") => Self::sew_exit(0, json!({"purged": true})),
                "engine sew doctor" => Self::sew_exit(0, json!({"ok": true, "checks": []})),
                "engine sew rotate-master" => Self::sew_exit(0, json!({"restarted": true})),
                "engine sew drop-prev" => Self::sew_exit(0, json!({"dropped": true})),
                "engine sew backup" => Self::sew_exit(0, json!({"ok": true})),
                "engine sew backups" => Self::sew_exit(
                    0,
                    json!([
                        {"id": "20260101T031500Z", "kind": "daily", "created_at": "2026-01-01T03:15:00Z", "bytes": 100},
                        {"id": "20260102T031500Z", "kind": "daily", "created_at": "2026-01-02T03:15:00Z", "bytes": 120},
                    ]),
                ),
                a if a.starts_with("engine sew restore ") => {
                    let mut v = Self::sew_exit(0, json!({"restored": true, "safety_bundle": "20260103T000000Z-safety"}));
                    v["restarted"] = json!(true);
                    v
                }
                other => return Err(format!("fake helper: no answer for `{other}`")),
            })
        }
    }

    impl SewOps for FakeOps {
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
                None => self.default_answer(&joined),
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
        fn mgmt(&self, method: &str, path: &str, token: &str, _body: Option<&Value>) -> Result<Value, MgmtError> {
            self.mgmt_calls.lock().unwrap().push(format!("{method} {path}"));
            if token != TOKEN {
                return Err(MgmtError::Unauthorized);
            }
            if !self.unit(k2_root_proto::sew::UNIT_SEW).is_active() {
                return Err(MgmtError::Unreachable("connection refused".into()));
            }
            match path {
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
