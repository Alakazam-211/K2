//! System effects for the Postgres sidecar supervisor, behind one trait.
//!
//! Unlike mail, this trait has **no download / extract_tar_gz**. Bake
//! installs distro packages; enable never apt-gets. Methods: unit
//! is-active, helper argv, file read/write for secrets, client argv
//! (`psql` / `pg_dump` / `pg_restore` as a non-superuser role).

use std::path::Path;

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Mutex;

pub const HELPER_PATH: &str = "/usr/local/libexec/k2-pg-helper";
pub const PSQL_PATH: &str = "/usr/bin/psql";
pub const PG_DUMP_PATH: &str = "/usr/bin/pg_dump";
pub const PG_RESTORE_PATH: &str = "/usr/bin/pg_restore";
pub const PG_UNIT: &str = "postgresql";
pub const PG_UNIT_PATHS: &[&str] = &[
    "/lib/systemd/system/postgresql.service",
    "/usr/lib/systemd/system/postgresql.service",
];

/// Distro-owned unit — we DROP-IN, we never rewrite postgresql.service.
/// Template drop-in is load-bearing on Debian/Ubuntu: `postgresql.service`
/// is a oneshot wrapper; the postmaster lives in `postgresql@.service`.
pub const PG_RAM_DROPIN_PATH: &str = "/etc/systemd/system/postgresql.service.d/k2-ram.conf";
pub const PG_RAM_TEMPLATE_DROPIN_PATH: &str =
    "/etc/systemd/system/postgresql@.service.d/k2-ram.conf";

/// D29 OS fence — fraction of box RAM, never 100%. Leaves the daemon + ≥1 agent.
pub const RAM_MEMORY_HIGH: &str = "25%";
pub const RAM_MEMORY_MAX: &str = "40%";
pub const RAM_CPU_WEIGHT: &str = "50";

/// systemd drop-in body (D29). `OOMPolicy=kill` = this cgroup only.
/// Must match `scripts/k2-pg-helper` `install-ram-fence` byte-for-byte
/// (no indent — rust `\` continuations would keep leading spaces).
pub fn ram_dropin() -> &'static str {
    "[Service]\nMemoryAccounting=yes\nMemoryHigh=25%\nMemoryMax=40%\nCPUWeight=50\nOOMPolicy=kill\n"
}

/// Engine fence (D29). Modest shared_buffers — not 25% of RAM.
/// 15 GB→4 GB is usually `work_mem × connections × hash/sort`.
pub const GUC_PINNED: &[(&str, &str)] = &[
    ("shared_buffers", "128MB"),
    ("work_mem", "8MB"),
    ("max_connections", "50"),
    ("max_parallel_workers", "2"),
    ("statement_timeout", "60s"),
    ("idle_in_transaction_session_timeout", "60s"),
    ("temp_file_limit", "1GB"),
];

#[cfg(test)]
pub fn guc_conf() -> &'static str {
    "# K2 D29 RAM fence — one query must not explode host RAM.\n\
     shared_buffers = 128MB\n\
     work_mem = 8MB\n\
     max_connections = 50\n\
     max_parallel_workers = 2\n\
     max_parallel_workers_per_gather = 1\n\
     statement_timeout = 60s\n\
     idle_in_transaction_session_timeout = 60s\n\
     temp_file_limit = 1GB\n"
}

/// ALTER SYSTEM block applied via helper `psql` (stdin, never `-c`).
pub fn pin_gucs_sql() -> String {
    let mut sql = String::new();
    for (name, value) in GUC_PINNED {
        sql.push_str("ALTER SYSTEM SET ");
        sql.push_str(name);
        sql.push_str(" = '");
        sql.push_str(value);
        sql.push_str("';\n");
    }
    sql.push_str("SELECT pg_reload_conf();\n");
    sql
}

/// `SELECT name, current_setting(name)` for the D29 GUC set.
pub fn guc_show_sql() -> String {
    let mut sql = String::from("SELECT name, current_setting(name) FROM (VALUES ");
    for (i, (name, _)) in GUC_PINNED.iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        sql.push('(');
        sql.push('\'');
        sql.push_str(name);
        sql.push_str("')");
    }
    sql.push_str(") AS t(name);\n");
    sql
}

#[allow(dead_code)]
pub trait SystemOps: Send + Sync {
    fn write_file(&self, path: &str, contents: &[u8], mode: u32) -> Result<(), String>;
    fn read_file(&self, path: &str) -> Result<Vec<u8>, String>;
    fn create_dir_all(&self, path: &str) -> Result<(), String>;
    fn remove_path(&self, path: &str) -> Result<(), String>;
    fn path_exists(&self, path: &str) -> bool;
    /// `systemctl <args…>` — mutating verbs. Fail on non-zero.
    fn systemctl(&self, args: &[&str]) -> Result<String, String>;
    /// `systemctl` where non-zero is an answer (`is-active` exits 3).
    fn systemctl_query(&self, args: &[&str]) -> String;
    /// Privileged helper (sudoers stub). `args[0]` is `systemctl`, `psql`,
    /// or `install-ram-fence`. Never pass agent SQL — callers construct
    /// CREATE DATABASE/ROLE / ALTER SYSTEM only.
    fn run_helper(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String>;
    /// Unprivileged client tool (`psql` / `pg_dump` / `pg_restore`) as a
    /// workspace role on loopback. Env is `KEY=VAL` pairs (PGPASSWORD).
    fn run_cmd(
        &self,
        cmd: &str,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&[u8]>,
    ) -> Result<Vec<u8>, String>;
    fn sleep_ms(&self, ms: u64);
}

/// Production implementation.
pub struct RealSystemOps;

impl RealSystemOps {
    fn run(cmd: &str, args: &[&str]) -> Result<std::process::Output, String> {
        std::process::Command::new(cmd)
            .args(args)
            .output()
            .map_err(|e| format!("{cmd} {}: {e}", args.join(" ")))
    }

    #[allow(dead_code)]
    fn run_ok(cmd: &str, args: &[&str]) -> Result<String, String> {
        let out = Self::run(cmd, args)?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!(
                "{cmd} {}: exit {:?}: {}",
                args.join(" "),
                out.status.code(),
                err.trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

impl SystemOps for RealSystemOps {
    fn write_file(&self, path: &str, contents: &[u8], mode: u32) -> Result<(), String> {
        std::fs::write(path, contents).map_err(|e| format!("write {path}: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                .map_err(|e| format!("chmod {path}: {e}"))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        Ok(())
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        std::fs::read(path).map_err(|e| format!("read {path}: {e}"))
    }

    fn create_dir_all(&self, path: &str) -> Result<(), String> {
        std::fs::create_dir_all(path).map_err(|e| format!("mkdir -p {path}: {e}"))
    }

    fn remove_path(&self, path: &str) -> Result<(), String> {
        let p = Path::new(path);
        if !p.exists() {
            return Ok(());
        }
        let res = if p.is_dir() {
            std::fs::remove_dir_all(p)
        } else {
            std::fs::remove_file(p)
        };
        res.map_err(|e| format!("remove {path}: {e}"))
    }

    fn path_exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn systemctl(&self, args: &[&str]) -> Result<String, String> {
        Self::run_ok("systemctl", args)
    }

    fn systemctl_query(&self, args: &[&str]) -> String {
        Self::run("systemctl", args)
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default()
    }

    fn run_helper(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        let mut cmd = std::process::Command::new("sudo");
        cmd.args(["-n", HELPER_PATH]).args(args);
        let out = if let Some(bytes) = stdin {
            cmd.stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            let mut child = cmd
                .spawn()
                .map_err(|e| format!("sudo {HELPER_PATH}: {e}"))?;
            if let Some(mut sin) = child.stdin.take() {
                use std::io::Write;
                sin.write_all(bytes)
                    .map_err(|e| format!("helper stdin: {e}"))?;
            }
            child
                .wait_with_output()
                .map_err(|e| format!("sudo {HELPER_PATH}: {e}"))?
        } else {
            cmd.output()
                .map_err(|e| format!("sudo {HELPER_PATH}: {e}"))?
        };
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!(
                "helper {}: exit {:?}: {}",
                args.join(" "),
                out.status.code(),
                err.trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn run_cmd(
        &self,
        cmd: &str,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&[u8]>,
    ) -> Result<Vec<u8>, String> {
        let mut c = std::process::Command::new(cmd);
        c.args(args);
        for (k, v) in env {
            c.env(k, v);
        }
        let out = if let Some(bytes) = stdin {
            c.stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            let mut child = c.spawn().map_err(|e| format!("{cmd}: {e}"))?;
            if let Some(mut sin) = child.stdin.take() {
                use std::io::Write;
                sin.write_all(bytes)
                    .map_err(|e| format!("{cmd} stdin: {e}"))?;
            }
            child
                .wait_with_output()
                .map_err(|e| format!("{cmd}: {e}"))?
        } else {
            c.output().map_err(|e| format!("{cmd}: {e}"))?
        };
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!(
                "{cmd} {}: exit {:?}: {}",
                args.join(" "),
                out.status.code(),
                err.trim()
            ));
        }
        Ok(out.stdout)
    }

    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// In-memory Postgres stand-in used by [`FakeSystemOps`]. Unit tests
/// never talk to a live cluster.
#[cfg(test)]
pub struct FakePg {
    pub dbs: Vec<String>,
    pub roles: Vec<String>,
    pub role_sql: Vec<String>,
    pub helper_sql: Vec<String>,
    /// (version, checksum) rows per database name.
    pub migrations: HashMap<String, Vec<(String, String)>>,
    /// Unaligned `psql -tA` field separator. Real psql defaults to `|`
    /// when `-F` is omitted; tests must not hide the tab-split bug.
    pub select_field_sep: &'static str,
    pub store: HashMap<(String, String), HashMap<String, serde_json::Value>>,
    pub dump_marker: Vec<u8>,
    /// Live GUC map (`current_setting`). Defaults look like stock Postgres
    /// so doctor fails loudly until `install-ram-fence` / ALTER SYSTEM pins.
    pub gucs: HashMap<String, String>,
}

#[cfg(test)]
impl Default for FakePg {
    fn default() -> Self {
        let mut gucs = HashMap::new();
        gucs.insert("shared_buffers".into(), "128MB".into());
        gucs.insert("work_mem".into(), "4MB".into());
        gucs.insert("max_connections".into(), "100".into());
        gucs.insert("max_parallel_workers".into(), "8".into());
        gucs.insert("statement_timeout".into(), "0".into());
        gucs.insert("idle_in_transaction_session_timeout".into(), "0".into());
        gucs.insert("temp_file_limit".into(), "-1".into());
        Self {
            dbs: Vec::new(),
            roles: Vec::new(),
            role_sql: Vec::new(),
            helper_sql: Vec::new(),
            migrations: HashMap::new(),
            select_field_sep: "|",
            store: HashMap::new(),
            dump_marker: Vec::new(),
            gucs,
        }
    }
}

#[cfg(test)]
pub fn pin_k2_gucs(gucs: &mut HashMap<String, String>) {
    for (k, v) in GUC_PINNED {
        gucs.insert((*k).to_string(), (*v).to_string());
    }
}

#[cfg(test)]
impl FakePg {
    pub(crate) fn exec_sql(&mut self, db: Option<&str>, sql: &str) -> Result<String, String> {
        let trimmed = sql.trim();
        if trimmed.is_empty() {
            return Ok(String::new());
        }
        if let Some(name) = extract_quoted_after(trimmed, "CREATE DATABASE") {
            if !self.dbs.iter().any(|d| d == &name) {
                self.dbs.push(name);
            }
            return Ok(String::new());
        }
        if let Some(name) = extract_quoted_after(trimmed, "CREATE ROLE") {
            if trimmed.to_ascii_uppercase().contains("SUPERUSER")
                && !trimmed.to_ascii_uppercase().contains("NOSUPERUSER")
            {
                return Err("refusing SUPERUSER in CREATE ROLE".into());
            }
            if !self.roles.iter().any(|r| r == &name) {
                self.roles.push(name);
            }
            self.role_sql.push(trimmed.to_string());
            return Ok(String::new());
        }
        let up = trimmed.to_ascii_uppercase();
        if up.contains("ALTER SYSTEM SET") {
            for stmt in trimmed.split(';') {
                let s = stmt.trim();
                if s.is_empty() {
                    continue;
                }
                let su = s.to_ascii_uppercase();
                if !su.contains("ALTER SYSTEM SET") {
                    continue;
                }
                if let Some((name, value)) = parse_alter_system(s) {
                    self.gucs.insert(name, value);
                }
            }
            return Ok(String::new());
        }
        if up.contains("CURRENT_SETTING") || up.contains("PG_SETTINGS") {
            let mut lines = Vec::new();
            for (name, _) in GUC_PINNED {
                if let Some(v) = self.gucs.get(*name) {
                    lines.push(format!("{name}|{v}"));
                }
            }
            return Ok(lines.join("\n"));
        }
        if up.contains("SHOW SERVER_VERSION_NUM") {
            return Ok("160003".into());
        }
        if up.contains("SELECT 1") {
            return Ok("1".into());
        }
        if trimmed.to_ascii_uppercase().contains("PG_DATABASE_SIZE") {
            return Ok("8192".into());
        }
        let db_name = db.unwrap_or("postgres");
        if trimmed.contains("_k2_migrations") {
            let up = trimmed.to_ascii_uppercase();
            if up.starts_with("SELECT") {
                let sep = self.select_field_sep;
                let vers = self.migrations.entry(db_name.to_string()).or_default();
                if up.contains("CHECKSUM") {
                    return Ok(vers
                        .iter()
                        .map(|(v, c)| {
                            if up.contains("APPLIED_AT") {
                                format!("{v}{sep}{c}{sep}2026-01-01 00:00:00+00")
                            } else {
                                format!("{v}{sep}{c}")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n"));
                }
                return Ok(vers
                    .iter()
                    .map(|(v, _)| v.clone())
                    .collect::<Vec<_>>()
                    .join("\n"));
            }
            if up.starts_with("INSERT") {
                if let Some(ver) = nth_sql_string(trimmed, 0) {
                    let checksum = nth_sql_string(trimmed, 1).unwrap_or_default();
                    let vers = self.migrations.entry(db_name.to_string()).or_default();
                    if vers.iter().any(|(v, _)| v == &ver) {
                        return Err(format!(
                            "duplicate key value violates unique constraint \"_k2_migrations_pkey\""
                        ));
                    }
                    vers.push((ver, checksum));
                }
                return Ok(String::new());
            }
            return Ok(String::new());
        }
        if trimmed.contains("_k2_store") {
            return self.exec_store(db_name, trimmed);
        }
        Ok(String::new())
    }

    fn exec_store(&mut self, db: &str, sql: &str) -> Result<String, String> {
        let up = sql.to_ascii_uppercase();
        if up.contains("CREATE TABLE") {
            return Ok(String::new());
        }
        if up.starts_with("INSERT")
            || up.starts_with("UPDATE")
            || (up.contains("ON CONFLICT") && up.contains("INSERT"))
        {
            let coll = extract_named(sql, "collection").or_else(|| nth_sql_string(sql, 0));
            let id = extract_named(sql, "id").or_else(|| nth_sql_string(sql, 1));
            let doc = extract_jsonb(sql);
            if let (Some(c), Some(i), Some(d)) = (coll, id, doc) {
                self.store
                    .entry((db.to_string(), c))
                    .or_default()
                    .insert(i, d);
            }
            return Ok(String::new());
        }
        if up.starts_with("SELECT") && up.contains("COLLECTION") {
            let coll = nth_sql_string(sql, 0).unwrap_or_default();
            if up.contains(" AND ID") || up.contains(" ID =") {
                let id = nth_sql_string(sql, 1).unwrap_or_default();
                if let Some(doc) = self
                    .store
                    .get(&(db.to_string(), coll))
                    .and_then(|m| m.get(&id))
                {
                    return Ok(doc.to_string());
                }
                return Ok(String::new());
            }
            let map = self
                .store
                .get(&(db.to_string(), coll))
                .cloned()
                .unwrap_or_default();
            let mut rows = Vec::new();
            for (id, doc) in map {
                rows.push(serde_json::json!({"id": id, "doc": doc}));
            }
            return Ok(serde_json::Value::Array(rows).to_string());
        }
        if up.starts_with("DELETE") {
            let coll = nth_sql_string(sql, 0).unwrap_or_default();
            if let Some(id) = nth_sql_string(sql, 1) {
                if let Some(m) = self.store.get_mut(&(db.to_string(), coll)) {
                    m.remove(&id);
                }
            } else {
                self.store.remove(&(db.to_string(), coll));
            }
            return Ok(String::new());
        }
        Ok(String::new())
    }
}

#[cfg(test)]
fn extract_quoted_after(sql: &str, keyword: &str) -> Option<String> {
    let idx = sql
        .to_ascii_uppercase()
        .find(&keyword.to_ascii_uppercase())?;
    let rest = sql[idx + keyword.len()..].trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        return Some(stripped[..end].to_string());
    }
    let ident = rest
        .split(|c: char| c.is_whitespace() || c == ';')
        .next()
        .unwrap_or("");
    if ident.is_empty() {
        None
    } else {
        Some(ident.to_string())
    }
}

#[cfg(test)]
fn nth_sql_string(sql: &str, n: usize) -> Option<String> {
    let mut out = Vec::new();
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            i += 1;
            let mut s = String::new();
            while i < bytes.len() {
                if bytes[i] == b'\'' {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                        s.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                s.push(bytes[i] as char);
                i += 1;
            }
            out.push(s);
        }
        i += 1;
    }
    out.into_iter().nth(n)
}

#[cfg(test)]
fn extract_named(_sql: &str, _key: &str) -> Option<String> {
    None
}

#[cfg(test)]
fn extract_jsonb(sql: &str) -> Option<serde_json::Value> {
    let start = sql.find('{')?;
    let end = sql.rfind('}')?;
    serde_json::from_str(&sql[start..=end]).ok()
}

#[cfg(test)]
fn parse_alter_system(stmt: &str) -> Option<(String, String)> {
    let s = stmt.trim();
    let up = s.to_ascii_uppercase();
    let idx = up.find("ALTER SYSTEM SET")?;
    let rest = s[idx + "ALTER SYSTEM SET".len()..].trim();
    let rest_up = rest.to_ascii_uppercase();
    let sep = rest_up.find(" TO ").or_else(|| rest.find('='))?;
    let name = rest[..sep].trim().to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    let mut val = rest[sep..].trim();
    if let Some(stripped) = val.strip_prefix('=') {
        val = stripped.trim();
    } else if val.len() >= 2 && val[..2].eq_ignore_ascii_case("to") {
        val = val[2..].trim();
    }
    let value = val.trim_matches(|c: char| c == '\'' || c == '"' || c == ';');
    if name.is_empty() || value.is_empty() {
        None
    } else {
        Some((name, value.to_string()))
    }
}

/// Recording fake: every effect appends one line to `ops`. ZERO real
/// systemd / apt / network. Optional in-memory Postgres for create /
/// migrate / store unit tests.
#[cfg(test)]
pub struct FakeSystemOps {
    pub ops: Mutex<Vec<String>>,
    pub existing_paths: Vec<String>,
    pub query_answers: HashMap<String, String>,
    pub files: Mutex<HashMap<String, Vec<u8>>>,
    pub pg: Mutex<FakePg>,
}

#[cfg(test)]
impl Default for FakeSystemOps {
    fn default() -> Self {
        Self {
            ops: Mutex::new(Vec::new()),
            existing_paths: Vec::new(),
            query_answers: HashMap::new(),
            files: Mutex::new(HashMap::new()),
            pg: Mutex::new(FakePg {
                dump_marker: b"FAKE-PG-DUMP".to_vec(),
                ..FakePg::default()
            }),
        }
    }
}

#[cfg(test)]
impl FakeSystemOps {
    pub fn record(&self, line: String) {
        self.ops
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(line);
    }
    pub fn recorded(&self) -> Vec<String> {
        self.ops.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
    pub fn baked() -> Self {
        let mut query_answers = HashMap::new();
        query_answers.insert("is-active postgresql".into(), "active".into());
        query_answers.insert(
            "list-units --type=service --state=active --no-legend --plain postgresql@*".into(),
            "postgresql@16-main.service loaded active running PostgreSQL Cluster 16-main".into(),
        );
        query_answers.insert(
            "show postgresql@16-main.service --property=MemoryCurrent,MemoryHigh,MemoryMax,MemoryAccounting".into(),
            "MemoryCurrent=268435456\nMemoryHigh=4294967296\nMemoryMax=6871947673\nMemoryAccounting=yes".into(),
        );
        query_answers.insert(
            "show postgresql --property=MemoryCurrent,MemoryHigh,MemoryMax,MemoryAccounting".into(),
            "MemoryCurrent=0\nMemoryHigh=4294967296\nMemoryMax=6871947673\nMemoryAccounting=yes"
                .into(),
        );
        Self {
            existing_paths: vec![
                PSQL_PATH.to_string(),
                PG_UNIT_PATHS[0].to_string(),
                HELPER_PATH.to_string(),
            ],
            query_answers,
            ..Self::default()
        }
    }
}

#[cfg(test)]
impl SystemOps for FakeSystemOps {
    fn write_file(&self, path: &str, contents: &[u8], mode: u32) -> Result<(), String> {
        self.record(format!(
            "write {path} ({} bytes, mode {mode:o})",
            contents.len()
        ));
        self.files
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(path.to_string(), contents.to_vec());
        Ok(())
    }
    fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        self.record(format!("read {path}"));
        self.files
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(path)
            .cloned()
            .ok_or_else(|| format!("read {path}: missing"))
    }
    fn create_dir_all(&self, path: &str) -> Result<(), String> {
        self.record(format!("mkdir {path}"));
        Ok(())
    }
    fn remove_path(&self, path: &str) -> Result<(), String> {
        self.record(format!("rm {path}"));
        Ok(())
    }
    fn path_exists(&self, path: &str) -> bool {
        self.existing_paths.iter().any(|p| p == path)
            || self
                .files
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains_key(path)
    }
    fn systemctl(&self, args: &[&str]) -> Result<String, String> {
        let line = format!("systemctl {}", args.join(" "));
        if line.contains("apt") {
            return Err("apt-get is forbidden on the sql sidecar".into());
        }
        self.record(line);
        Ok(String::new())
    }
    fn systemctl_query(&self, args: &[&str]) -> String {
        let key = args.join(" ");
        self.record(format!("systemctl? {key}"));
        self.query_answers.get(&key).cloned().unwrap_or_default()
    }
    fn run_helper(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        let joined = args.join(" ");
        if joined.contains("apt") {
            return Err("apt-get is forbidden on the sql sidecar".into());
        }
        self.record(format!(
            "helper {joined} stdin={}",
            stdin.map(|s| s.len()).unwrap_or(0)
        ));
        if args.first().copied() == Some("install-ram-fence") {
            let body = ram_dropin().as_bytes();
            self.write_file(PG_RAM_DROPIN_PATH, body, 0o644)?;
            self.write_file(PG_RAM_TEMPLATE_DROPIN_PATH, body, 0o644)?;
            pin_k2_gucs(&mut self.pg.lock().unwrap_or_else(|p| p.into_inner()).gucs);
            return Ok(String::new());
        }
        if args.first().copied() == Some("systemctl") {
            if args.get(1) == Some(&"is-active") {
                let key = args[1..].join(" ");
                return Ok(self
                    .query_answers
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| "active".into()));
            }
            return Ok(String::new());
        }
        let sql = stdin
            .map(|b| String::from_utf8_lossy(b).to_string())
            .unwrap_or_default();
        let mut pg = self.pg.lock().unwrap_or_else(|p| p.into_inner());
        if !sql.is_empty() {
            pg.helper_sql.push(sql.clone());
        }
        let db = args
            .windows(2)
            .find(|w| w[0] == "-d")
            .map(|w| w[1].to_string());
        pg.exec_sql(db.as_deref(), &sql)
    }
    fn run_cmd(
        &self,
        cmd: &str,
        args: &[&str],
        _env: &[(&str, &str)],
        stdin: Option<&[u8]>,
    ) -> Result<Vec<u8>, String> {
        self.record(format!("cmd {cmd} {}", args.join(" ")));
        if cmd.ends_with("pg_dump") {
            let dest = args
                .windows(2)
                .find(|w| w[0] == "-f")
                .map(|w| w[1].to_string());
            let marker = self
                .pg
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .dump_marker
                .clone();
            if let Some(path) = dest {
                self.files
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(path, marker.clone());
            }
            return Ok(marker);
        }
        if cmd.ends_with("pg_restore") {
            return Ok(Vec::new());
        }
        let sql = stdin
            .map(|b| String::from_utf8_lossy(b).to_string())
            .unwrap_or_default();
        let db = args
            .windows(2)
            .find(|w| w[0] == "-d")
            .map(|w| w[1].to_string());
        let mut pg = self.pg.lock().unwrap_or_else(|p| p.into_inner());
        pg.exec_sql(db.as_deref(), &sql).map(|s| s.into_bytes())
    }
    fn sleep_ms(&self, _ms: u64) {}
}

/// Distro packages + unit present (bake `--with-db`). Never apt-get.
pub fn is_baked(ops: &dyn SystemOps) -> bool {
    ops.path_exists(PSQL_PATH) && PG_UNIT_PATHS.iter().any(|p| ops.path_exists(p))
}
