//! System effects for the Stalwart supervisor, behind one trait.
//!
//! EVERY filesystem / systemd / download / journald effect the S1
//! install+lifecycle flows perform goes through [`SystemOps`] so the
//! full sequence is unit-tested on macOS as a call-order assertion
//! with ZERO side effects (house rule: no real systemd/fs writes in
//! tests, nothing below port 1024, no network).
//!
//! [`RealSystemOps`] sends privileged effects through
//! `sudo -n /usr/local/libexec/k2-mail-helper` ([`super::helper`]).
//! There is no raw `useradd` / `chown` / `systemctl` fallback.
//! `systemctl_query` (`is-active`) stays a direct non-root `systemctl`.
//! It is only ever invoked behind [`super::supervisor::mail_supported`].

use std::path::Path;

use super::helper;

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
    /// Extract archive member `member` to `dest` with `mode`.
    /// Production accepts only member `stalwart`, dest
    /// `/usr/local/bin/stalwart`, mode `0o755`, and pipes the original
    /// tarball bytes to the mail helper. It does not stage under `/tmp`.
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
    /// Can this daemon user `sudo -n` the mail helper? Read-only probe;
    /// every privileged flow checks it BEFORE its first effect so a box
    /// without the helper stops with the install command, not EACCES.
    fn mail_helper_state(&self) -> helper::HelperState;
    /// Read-only helper query (`version` | `usage`): its stdout. An older
    /// helper answers `version` with an error ("refused arguments").
    fn helper_query(&self, verb: &str) -> Result<String, String>;
    /// Helper `snapshot-data` (fixed paths, refuses unless Stalwart is
    /// stopped). Returns the helper's JSON report.
    fn snapshot_data(&self) -> Result<String, String>;
    /// Helper `restore-data` (fixed paths, refuses unless Stalwart is
    /// stopped and a complete snapshot exists).
    fn restore_data(&self) -> Result<String, String>;
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

    /// `sudo -n` the mail helper. Stdin is not copied into the error.
    fn run_mail_helper(args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(helper::SUDO_PATH);
        cmd.arg("-n").arg(helper::HELPER_PATH).args(args);
        let out = if let Some(bytes) = stdin {
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| helper_spawn_error(&e))?;
            {
                let mut sin = child
                    .stdin
                    .take()
                    .ok_or_else(|| "mail helper: stdin".to_string())?;
                sin.write_all(bytes)
                    .map_err(|_| "mail helper: stdin write failed".to_string())?;
            }
            child
                .wait_with_output()
                .map_err(|e| helper_spawn_error(&e))?
        } else {
            cmd.stdin(Stdio::null())
                .output()
                .map_err(|e| helper_spawn_error(&e))?
        };
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            if let Some(mapped) = helper::map_helper_failure(None, &err) {
                return Err(mapped);
            }
            let trimmed = err.trim();
            let mut end = trimmed.len().min(400);
            while end > 0 && !trimmed.is_char_boundary(end) {
                end -= 1;
            }
            return Err(format!(
                "mail helper {}: exit {:?}: {}",
                args.join(" "),
                out.status.code(),
                &trimmed[..end]
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

fn helper_spawn_error(err: &std::io::Error) -> String {
    helper::map_helper_failure(Some(err), "").unwrap_or_else(|| format!("mail helper: {err}"))
}

fn helper_argv(args: &[&str]) -> Result<Vec<&'static str>, String> {
    Ok(helper::parse_argv(args)?.argv())
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

    fn write_file(&self, path: &str, contents: &[u8], _mode: u32) -> Result<(), String> {
        let argv = helper_argv(&["write", path])?;
        // The helper forces mode and refuses bytes that are not a canned
        // unit or drop-in. Check here too so a mismatch never reaches sudo
        // and the secret stays out of this error.
        if let helper::HelperCommand::Write(canon) = helper::parse_argv(&argv)? {
            helper::write_bytes_accepted(canon, contents)?;
        }
        Self::run_mail_helper(&argv, Some(contents)).map(|_| ())
    }

    fn create_dir_all(&self, path: &str) -> Result<(), String> {
        let argv = helper_argv(&["mkdir", path])?;
        Self::run_mail_helper(&argv, None).map(|_| ())
    }

    fn remove_path(&self, path: &str) -> Result<(), String> {
        let argv = helper_argv(&["remove", path])?;
        Self::run_mail_helper(&argv, None).map(|_| ())
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
        // The daemon already verified the tarball. Pipe those bytes.
        // Do not stage under /tmp and do not call create_dir_all for it.
        helper::extract_request_allowed(member, dest, mode)?;
        let argv = helper_argv(&["install-bin"])?;
        Self::run_mail_helper(&argv, Some(archive)).map(|_| ())
    }

    fn ensure_system_user(&self, user: &str) -> Result<(), String> {
        helper::require_stalwart_user(user)?;
        let argv = helper_argv(&["ensure-user"])?;
        Self::run_mail_helper(&argv, None).map(|_| ())
    }

    fn chown_recursive(&self, path: &str, user: &str) -> Result<(), String> {
        helper::require_stalwart_user(user)?;
        let argv = helper_argv(&["chown", path])?;
        Self::run_mail_helper(&argv, None).map(|_| ())
    }

    fn systemctl(&self, args: &[&str]) -> Result<String, String> {
        let mut argv = Vec::with_capacity(1 + args.len());
        argv.push("systemctl");
        argv.extend_from_slice(args);
        let canon = helper_argv(&argv)?;
        Self::run_mail_helper(&canon, None)
    }

    fn systemctl_query(&self, args: &[&str]) -> String {
        Self::run("systemctl", args)
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default()
    }

    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    fn mail_helper_state(&self) -> helper::HelperState {
        helper::probe_state()
    }

    fn helper_query(&self, verb: &str) -> Result<String, String> {
        match helper::parse_argv(&[verb])? {
            cmd @ (helper::HelperCommand::Version | helper::HelperCommand::Usage) => {
                Self::run_mail_helper(&cmd.argv(), None)
            }
            _ => Err(format!("mail helper: '{verb}' is not a query verb")),
        }
    }

    fn snapshot_data(&self) -> Result<String, String> {
        let argv = helper_argv(&["snapshot-data"])?;
        Self::run_mail_helper(&argv, None)
    }

    fn restore_data(&self) -> Result<String, String> {
        let argv = helper_argv(&["restore-data"])?;
        Self::run_mail_helper(&argv, None)
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
        /// Last bytes written per path (C20 recovery-admin strip).
        pub written: Mutex<HashMap<String, Vec<u8>>>,
        /// `mail_helper_state` answer (default: installed). Not recorded
        /// in `ops` — it is a read, and flows assert effect sequences.
        pub helper_state: helper::HelperState,
        /// Per-URL `download` bodies (falls back to `download_body`).
        pub download_bodies: HashMap<String, Vec<u8>>,
        /// `helper_query` stdout per verb. A missing verb answers like a
        /// protocol-1 helper: "mail helper: refused arguments".
        pub helper_answers: HashMap<String, String>,
        /// Recorded effect lines that FAIL. Each entry fails ONE matching
        /// call (listed twice = fails twice); the line is still recorded.
        pub fail_on: Mutex<Vec<String>>,
        /// `Some` = a stateful unit: `is-active stalwart` answers from it,
        /// and stop / restart / enable --now / disable --now move it.
        pub unit_active: Mutex<Option<bool>>,
        /// Tarball bytes passed to `extract_tar_gz_member`, in order.
        pub extracted: Mutex<Vec<Vec<u8>>>,
    }

    impl Default for FakeSystemOps {
        fn default() -> Self {
            Self {
                ops: Mutex::new(Vec::new()),
                download_body: Vec::new(),
                download_error: None,
                existing_paths: Vec::new(),
                query_answers: HashMap::new(),
                written: Mutex::new(HashMap::new()),
                helper_state: helper::HelperState::Installed,
                download_bodies: HashMap::new(),
                helper_answers: HashMap::new(),
                fail_on: Mutex::new(Vec::new()),
                unit_active: Mutex::new(None),
                extracted: Mutex::new(Vec::new()),
            }
        }
    }

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
        /// Record `line`, then fail it if `fail_on` lists it (consumed).
        pub fn effect(&self, line: String) -> Result<(), String> {
            self.record(line.clone());
            let mut fails = self.fail_on.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(i) = fails.iter().position(|l| *l == line) {
                fails.remove(i);
                return Err(format!("injected failure: {line}"));
            }
            Ok(())
        }
        pub fn fail(&self, line: &str) {
            self.fail_on
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(line.to_string());
        }
        pub fn set_unit_active(&self, active: Option<bool>) {
            *self.unit_active.lock().unwrap_or_else(|p| p.into_inner()) = active;
        }
        pub fn unit_is_active(&self) -> Option<bool> {
            *self.unit_active.lock().unwrap_or_else(|p| p.into_inner())
        }
    }

    impl SystemOps for FakeSystemOps {
        fn download(&self, url: &str) -> Result<Vec<u8>, String> {
            self.record(format!("download {url}"));
            match &self.download_error {
                Some(e) => Err(e.clone()),
                None => Ok(self
                    .download_bodies
                    .get(url)
                    .cloned()
                    .unwrap_or_else(|| self.download_body.clone())),
            }
        }
        fn write_file(&self, path: &str, contents: &[u8], mode: u32) -> Result<(), String> {
            self.effect(format!(
                "write {path} ({} bytes, mode {mode:o})",
                contents.len()
            ))?;
            self.written
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(path.to_string(), contents.to_vec());
            Ok(())
        }
        fn create_dir_all(&self, path: &str) -> Result<(), String> {
            self.effect(format!("mkdir {path}"))
        }
        fn remove_path(&self, path: &str) -> Result<(), String> {
            self.effect(format!("rm {path}"))
        }
        fn path_exists(&self, path: &str) -> bool {
            self.existing_paths.iter().any(|p| p == path)
                || self
                    .written
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .contains_key(path)
        }
        fn extract_tar_gz_member(
            &self,
            archive: &[u8],
            member: &str,
            dest: &str,
            mode: u32,
        ) -> Result<(), String> {
            self.extracted
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(archive.to_vec());
            self.effect(format!("extract {member} -> {dest} (mode {mode:o})"))
        }
        fn ensure_system_user(&self, user: &str) -> Result<(), String> {
            self.effect(format!("useradd {user}"))
        }
        fn chown_recursive(&self, path: &str, user: &str) -> Result<(), String> {
            self.effect(format!("chown {user} {path}"))
        }
        fn systemctl(&self, args: &[&str]) -> Result<String, String> {
            self.effect(format!("systemctl {}", args.join(" ")))?;
            let mut unit = self.unit_active.lock().unwrap_or_else(|p| p.into_inner());
            if unit.is_some() {
                match args {
                    ["stop", _] | ["disable", "--now", _] => *unit = Some(false),
                    ["restart", _] | ["enable", "--now", _] => *unit = Some(true),
                    _ => {}
                }
            }
            Ok(String::new())
        }
        fn systemctl_query(&self, args: &[&str]) -> String {
            let key = args.join(" ");
            self.record(format!("systemctl? {key}"));
            if key == "is-active stalwart" {
                if let Some(active) = self.unit_is_active() {
                    return if active { "active" } else { "inactive" }.to_string();
                }
            }
            self.query_answers.get(&key).cloned().unwrap_or_default()
        }
        fn sleep_ms(&self, _ms: u64) {
            // Instant in tests.
        }
        fn mail_helper_state(&self) -> helper::HelperState {
            self.helper_state
        }
        fn helper_query(&self, verb: &str) -> Result<String, String> {
            self.effect(format!("helper {verb}"))?;
            self.helper_answers
                .get(verb)
                .cloned()
                .ok_or_else(|| "mail helper: refused arguments".to_string())
        }
        fn snapshot_data(&self) -> Result<String, String> {
            self.effect("snapshot-data".into())?;
            Ok("{}".into())
        }
        fn restore_data(&self) -> Result<String, String> {
            self.effect("restore-data".into())?;
            Ok("{}".into())
        }
    }
}
