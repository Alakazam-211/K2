//! [`ClusterOps`] over the root helper's protocol-9 `pg` verbs (sudo door).
//! Every argv parses against the catalog before it is sent
//! (`root_helper::call_with`). The only things the daemon does itself:
//! `statvfs` of the data filesystem, a look for an extension's control
//! file, and a role's own TCP login (its vaulted password, never root).

use std::path::Path;

use serde_json::{json, Value};

use k2_root_proto::pg_major::{ClusterRow, PgMajor, PG_LIB};

use super::{ClusterOps, ConnMode, Major};
use crate::root_helper::{call_with, Door};

pub const PG_DATA_ROOT: &str = "/var/lib/postgresql";

pub struct HelperOps<'a> {
    pub door: &'a dyn Door,
}

fn tok(m: Major) -> Result<String, String> {
    PgMajor::new(m).map(PgMajor::token).ok_or_else(|| format!("unknown Postgres major {m}"))
}

fn sv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// `pg clusters`' `clusters[]` rows.
pub fn rows_from(v: &Value) -> Vec<ClusterRow> {
    v["clusters"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some(ClusterRow {
                        major: c["major"].as_u64()? as u32,
                        name: c["name"].as_str()?.to_string(),
                        port: c["port"].as_u64()? as u16,
                        status: c["status"].as_str()?.to_string(),
                        owner: c["owner"].as_str().unwrap_or_default().to_string(),
                        data_dir: c["dataDir"].as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn ext_ok(x: &str) -> bool {
    !x.is_empty() && x.len() <= 63 && x.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
}

impl HelperOps<'_> {
    fn call(&self, argv: Vec<String>, stdin: Option<&[u8]>) -> Result<Vec<u8>, String> {
        call_with(self.door, &argv, stdin)
    }
    fn json(&self, argv: Vec<String>, stdin: Option<&[u8]>) -> Result<Value, String> {
        let out = self.call(argv, stdin)?;
        serde_json::from_slice(&out).map_err(|_| "root helper answer is not JSON".to_string())
    }
    fn clusters_json(&self) -> Result<Value, String> {
        self.json(sv(&["pg", "clusters"]), None)
    }
}

impl ClusterOps for HelperOps<'_> {
    fn clusters(&self) -> Result<Vec<ClusterRow>, String> {
        Ok(rows_from(&self.clusters_json()?))
    }
    fn installed(&self) -> Result<Vec<Major>, String> {
        let v = self.clusters_json()?;
        Ok(v["installed"].as_array().map(|a| a.iter().filter_map(|m| m.as_u64().map(|m| m as u32)).collect()).unwrap_or_default())
    }
    fn band_listening(&self) -> Result<Vec<u16>, String> {
        let v = self.clusters_json()?;
        Ok(v["bandListening"].as_array().map(|a| a.iter().filter_map(|m| m.as_u64().map(|m| m as u16)).collect()).unwrap_or_default())
    }
    fn add_major(&self, m: Major) -> Result<(), String> {
        self.json(vec!["pg".into(), "add-major".into(), tok(m)?], None).map(|_| ())
    }
    fn ensure_cluster(&self, m: Major, port: u16) -> Result<(), String> {
        self.json(vec!["pg".into(), "ensure-cluster".into(), tok(m)?, port.to_string()], None).map(|_| ())
    }
    fn sql(&self, m: Major, db: &str, sql: &str) -> Result<String, String> {
        let argv = vec![
            "pg".into(),
            "psql".into(),
            "--cluster".into(),
            tok(m)?,
            "-d".into(),
            db.to_string(),
            "-v".into(),
            "ON_ERROR_STOP=1".into(),
            "-tA".into(),
        ];
        let out = self.call(argv, Some(sql.as_bytes()))?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }
    fn copy_roles(&self, from: Major, to: Major, roles: &[String], dry_run: bool) -> Result<Value, String> {
        let mut argv = vec!["pg".into(), "copy-roles".into(), tok(from)?, tok(to)?];
        if dry_run {
            argv.push("--dry-run".into());
        }
        let body = serde_json::to_vec(&json!({ "roles": roles })).map_err(|e| e.to_string())?;
        self.json(argv, Some(&body))
    }
    fn allow_conn(&self, m: Major, db: &str, mode: ConnMode) -> Result<(), String> {
        self.json(vec!["pg".into(), "allow-conn".into(), tok(m)?, db.into(), mode.token().into()], None).map(|_| ())
    }
    fn dump(&self, from: Major, to: Major, db: &str, tag: &str) -> Result<u64, String> {
        let v = self.json(vec!["pg".into(), "move-dump".into(), tok(from)?, tok(to)?, db.into(), tag.into()], None)?;
        v["bytes"].as_u64().ok_or_else(|| "move-dump answered no size".into())
    }
    fn restore(&self, to: Major, db: &str, tag: &str, role: &str, password: &str) -> Result<(), String> {
        let body = serde_json::to_vec(&json!({ "role": role, "password": password })).map_err(|e| e.to_string())?;
        self.json(vec!["pg".into(), "move-restore".into(), tok(to)?, db.into(), tag.into()], Some(&body)).map(|_| ())
    }
    fn drop_db(&self, m: Major, db: &str) -> Result<bool, String> {
        let v = self.json(vec!["pg".into(), "drop-db".into(), tok(m)?, db.into()], None)?;
        Ok(v["dropped"].as_bool().unwrap_or(false))
    }
    fn remove_dump(&self, db: &str, tag: &str) -> Result<(), String> {
        self.json(sv(&["pg", "move-file-remove", db, tag]), None).map(|_| ())
    }
    fn free_bytes(&self) -> Result<u64, String> {
        free_bytes(Path::new(PG_DATA_ROOT))
    }
    fn extension_available(&self, m: Major, ext: &str) -> Option<bool> {
        extension_available_at(Path::new("/"), m, ext)
    }
    fn login(&self, m: Major, port: u16, db: &str, role: &str, password: &str) -> Result<(), String> {
        role_login(m, port, db, role, password)
    }
    fn moves_bytes(&self) -> u64 {
        self.clusters_json().ok().and_then(|v| v["movesBytes"].as_u64()).unwrap_or(0)
    }
}

/// `/usr/share/postgresql/<M>/extension/<x>.control` under `root`.
pub fn extension_available_at(root: &Path, m: Major, ext: &str) -> Option<bool> {
    if !ext_ok(ext) {
        return Some(false);
    }
    if !root.join(format!("{}/{m}/bin/postgres", PG_LIB.trim_start_matches('/'))).exists() {
        return None;
    }
    Some(root.join(format!("usr/share/postgresql/{m}/extension/{ext}.control")).exists())
}

pub fn free_bytes(path: &Path) -> Result<u64, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| "statvfs: bad path".to_string())?;
        // SAFETY: zeroed statvfs is a valid out-param; `c` is NUL-terminated.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
        if rc != 0 {
            return Err(format!("statvfs {}: {}", path.display(), std::io::Error::last_os_error()));
        }
        #[allow(clippy::unnecessary_cast)]
        Ok((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err("statvfs: unsupported".into())
    }
}

/// A 0600 `.pgpass` in a fresh 0700 directory: the password never sits in
/// a process environment other processes of the daemon's user can read
/// (security review #1 info). Removed when dropped.
pub struct PassFile {
    dir: std::path::PathBuf,
    pub path: std::path::PathBuf,
}

impl PassFile {
    pub fn new(port: u16, db: &str, role: &str, password: &str) -> Result<Self, String> {
        use std::io::Write;
        #[cfg(unix)]
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let mut rnd = [0u8; 8];
        getrandom::getrandom(&mut rnd).map_err(|e| format!("random: {e}"))?;
        let name: String = rnd.iter().map(|b| format!("{b:02x}")).collect();
        let dir = std::env::temp_dir().join(format!("k2-pgpass-{name}"));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut b = std::fs::DirBuilder::new();
        #[cfg(unix)]
        b.mode(0o700);
        b.create(&dir).map_err(|e| format!("pgpass dir: {e}"))?;
        let path = dir.join("pgpass");
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        o.mode(0o600);
        let esc = |s: &str| s.replace('\\', "\\\\").replace(':', "\\:");
        let line = format!("127.0.0.1:{port}:{}:{}:{}\n", esc(db), esc(role), esc(password));
        let res = o.open(&path).and_then(|mut f| f.write_all(line.as_bytes()));
        let pf = PassFile { dir, path };
        res.map_err(|e| format!("pgpass: {e}"))?;
        Ok(pf)
    }
}

impl Drop for PassFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.dir);
    }
}

/// The role's own login over TCP (scram), with that major's psql.
pub fn role_login(m: Major, port: u16, db: &str, role: &str, password: &str) -> Result<(), String> {
    let psql = PgMajor::new(m).ok_or("unknown major")?.bin("psql");
    let pass = PassFile::new(port, db, role, password)?;
    let out = k2_core::bg_proc::command(&psql)
        .args(["-X", "-h", "127.0.0.1", "-p", &port.to_string(), "-U", role, "-d", db, "-tA", "-w", "-c", "SELECT 1"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("PGPASSFILE", &pass.path)
        .env("PGCONNECT_TIMEOUT", "10")
        .output()
        .map_err(|e| format!("{psql}: {e}"))?;
    if out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "1" {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.lines().filter(|l| !l.to_ascii_lowercase().contains("password")).collect::<Vec<_>>().join(" ").trim().to_string()
            + if err.to_ascii_lowercase().contains("password") { " (password authentication failed)" } else { "" })
    }
}
