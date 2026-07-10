//! System effects for the Stalwart supervisor, behind one trait.
//!
//! EVERY filesystem / systemd / download / journald effect the S1
//! install+lifecycle flows perform goes through [`SystemOps`] so the
//! full sequence is unit-tested on macOS as a call-order assertion
//! with ZERO side effects (house rule: no real systemd/fs writes in
//! tests, nothing below port 1024, no network).
//!
//! The real implementation shells out for the Linux-only pieces
//! (`useradd`, `systemctl`, `journalctl`, `tar`, `chown`) — all
//! guaranteed present on any systemd box, and it keeps k2-daemon free
//! of tar/gzip deps. It is only ever invoked behind
//! [`super::supervisor::mail_supported`].

use std::path::Path;

pub trait SystemOps: Send + Sync {
    /// Download `url` fully into memory (the pinned tarball is ~40 MB;
    /// the sha256 check runs over these exact bytes before anything
    /// touches disk — update_routes precedent).
    fn download(&self, url: &str) -> Result<Vec<u8>, String>;
    /// Write `contents` to `path` (parent dirs NOT created — call
    /// `create_dir_all` explicitly so the test sequence shows it),
    /// then chmod to `mode`.
    fn write_file(&self, path: &str, contents: &[u8], mode: u32) -> Result<(), String>;
    fn create_dir_all(&self, path: &str) -> Result<(), String>;
    /// Recursive, missing-is-ok removal.
    fn remove_path(&self, path: &str) -> Result<(), String>;
    fn path_exists(&self, path: &str) -> bool;
    /// Extract the single archive member `member` from the in-memory
    /// gzipped tarball to `dest` with `mode`.
    fn extract_tar_gz_member(
        &self,
        archive: &[u8],
        member: &str,
        dest: &str,
        mode: u32,
    ) -> Result<(), String>;
    /// Idempotently ensure a no-login system user exists.
    fn ensure_system_user(&self, user: &str) -> Result<(), String>;
    /// `chown -R user:user path`.
    fn chown_recursive(&self, path: &str, user: &str) -> Result<(), String>;
    /// `systemctl <args…>`; returns trimmed stdout. An exit-failure is
    /// an `Err` carrying stderr EXCEPT for state-query verbs
    /// (`is-active` etc.) — callers use [`Self::systemctl_query`] for
    /// those.
    fn systemctl(&self, args: &[&str]) -> Result<String, String>;
    /// `systemctl` where a non-zero exit is an ANSWER, not an error
    /// (`is-active` exits 3 for "inactive"). Returns trimmed stdout.
    fn systemctl_query(&self, args: &[&str]) -> String;
    /// Sleep — injected so retry loops are instant in tests.
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
    fn download(&self, url: &str) -> Result<Vec<u8>, String> {
        let resp = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .map_err(|e| format!("http client: {e}"))?
            .get(url)
            .send()
            .map_err(|e| format!("GET {url}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("GET {url}: HTTP {}", resp.status()));
        }
        resp.bytes()
            .map(|b| b.to_vec())
            .map_err(|e| format!("GET {url}: read body: {e}"))
    }

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

    fn extract_tar_gz_member(
        &self,
        archive: &[u8],
        member: &str,
        dest: &str,
        mode: u32,
    ) -> Result<(), String> {
        // Stage the verified bytes, extract the one member with the
        // system tar (always present on a systemd box), move into
        // place. All under a private staging dir.
        let staging = format!("/tmp/k2-mail-extract-{}", std::process::id());
        self.create_dir_all(&staging)?;
        let result = (|| {
            let tarball = format!("{staging}/archive.tar.gz");
            std::fs::write(&tarball, archive).map_err(|e| format!("write {tarball}: {e}"))?;
            Self::run_ok("tar", &["-xzf", &tarball, "-C", &staging, member])?;
            let extracted = format!("{staging}/{member}");
            std::fs::rename(&extracted, dest).or_else(|_| {
                // Cross-device fallback: copy + remove.
                std::fs::copy(&extracted, dest)
                    .map(|_| ())
                    .map_err(|e| format!("install {member} -> {dest}: {e}"))
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(dest, std::fs::Permissions::from_mode(mode))
                    .map_err(|e| format!("chmod {dest}: {e}"))?;
            }
            #[cfg(not(unix))]
            let _ = mode;
            Ok(())
        })();
        let _ = std::fs::remove_dir_all(&staging);
        result
    }

    fn ensure_system_user(&self, user: &str) -> Result<(), String> {
        // Exists already? fine (idempotent/resumable install).
        if Self::run_ok("id", &["-u", user]).is_ok() {
            return Ok(());
        }
        Self::run_ok(
            "useradd",
            &["--system", "--no-create-home", "--shell", "/usr/sbin/nologin", user],
        )
        .map(|_| ())
    }

    fn chown_recursive(&self, path: &str, user: &str) -> Result<(), String> {
        Self::run_ok("chown", &["-R", &format!("{user}:{user}"), path]).map(|_| ())
    }

    fn systemctl(&self, args: &[&str]) -> Result<String, String> {
        Self::run_ok("systemctl", args)
    }

    fn systemctl_query(&self, args: &[&str]) -> String {
        Self::run("systemctl", args)
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default()
    }

    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

// ── Test fake ───────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Recording fake: every effect appends one line to `ops` so a
    /// whole flow asserts as a sequence; responses are configurable
    /// per-call-site. ZERO real side effects.
    pub struct FakeSystemOps {
        pub ops: Mutex<Vec<String>>,
        /// Bytes returned by `download` (the tests hash these).
        pub download_body: Vec<u8>,
        /// `Err` to simulate a failed download.
        pub download_error: Option<String>,
        /// Paths reported as existing.
        pub existing_paths: Vec<String>,
        /// Canned `systemctl_query` answers keyed by joined args.
        pub query_answers: HashMap<String, String>,
    }

    impl Default for FakeSystemOps {
        fn default() -> Self {
            Self {
                ops: Mutex::new(Vec::new()),
                download_body: Vec::new(),
                download_error: None,
                existing_paths: Vec::new(),
                query_answers: HashMap::new(),
            }
        }
    }

    impl FakeSystemOps {
        pub fn record(&self, line: String) {
            self.ops.lock().unwrap_or_else(|p| p.into_inner()).push(line);
        }
        pub fn recorded(&self) -> Vec<String> {
            self.ops.lock().unwrap_or_else(|p| p.into_inner()).clone()
        }
    }

    impl SystemOps for FakeSystemOps {
        fn download(&self, url: &str) -> Result<Vec<u8>, String> {
            self.record(format!("download {url}"));
            match &self.download_error {
                Some(e) => Err(e.clone()),
                None => Ok(self.download_body.clone()),
            }
        }
        fn write_file(&self, path: &str, contents: &[u8], mode: u32) -> Result<(), String> {
            self.record(format!("write {path} ({} bytes, mode {mode:o})", contents.len()));
            Ok(())
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
        }
        fn extract_tar_gz_member(
            &self,
            _archive: &[u8],
            member: &str,
            dest: &str,
            mode: u32,
        ) -> Result<(), String> {
            self.record(format!("extract {member} -> {dest} (mode {mode:o})"));
            Ok(())
        }
        fn ensure_system_user(&self, user: &str) -> Result<(), String> {
            self.record(format!("useradd {user}"));
            Ok(())
        }
        fn chown_recursive(&self, path: &str, user: &str) -> Result<(), String> {
            self.record(format!("chown {user} {path}"));
            Ok(())
        }
        fn systemctl(&self, args: &[&str]) -> Result<String, String> {
            self.record(format!("systemctl {}", args.join(" ")));
            Ok(String::new())
        }
        fn systemctl_query(&self, args: &[&str]) -> String {
            let key = args.join(" ");
            self.record(format!("systemctl? {key}"));
            self.query_answers.get(&key).cloned().unwrap_or_default()
        }
        fn sleep_ms(&self, _ms: u64) {
            // Instant in tests.
        }
    }
}
