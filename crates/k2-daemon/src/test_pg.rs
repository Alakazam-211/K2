//! A throwaway Postgres server a real-Postgres test makes itself (under
//! `/tmp`, its own socket directory and port), and the `SystemOps` that
//! points K2's SQL at it: the root helper's superuser psql over the socket,
//! the daemon's own psql over TCP. Never the box's own server.
//!
//! `REALPG_BIN`: the server's `bin` (`initdb`, `pg_ctl`, `postgres`,
//! `psql`); default `/usr/lib/postgresql/16/bin`. A compute job may unpack
//! the distro's packages and point there.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::sql::sysops::{RealSystemOps, SystemOps};

pub(crate) fn bin() -> String {
    std::env::var("REALPG_BIN").unwrap_or_else(|_| "/usr/lib/postgresql/16/bin".into())
}

pub(crate) fn run(prog: &str, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Result<String, String> {
    let mut e: Vec<(&str, &str)> = vec![("PSQLRC", "/dev/null"), ("LC_ALL", "C")];
    e.extend_from_slice(env);
    let out = RealSystemOps.run_cmd(prog, args, &e, stdin)?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

pub(crate) struct Server {
    pub root: PathBuf,
    pub port: u16,
}

impl Server {
    /// initdb + start (socket trust for any role, TCP scram on 127.0.0.1).
    /// Leaked: the route seam wants `'static`. Hold the returned [`Stop`].
    pub(crate) fn start(port: u16, tag: &str) -> (&'static Server, Stop) {
        assert!(Path::new(&format!("{}/initdb", bin())).exists(), "{}/initdb: install Postgres (REALPG_BIN)", bin());
        let root = PathBuf::from(format!("/tmp/k2{tag}-{}", &uuid::Uuid::new_v4().to_string()[..8]));
        std::fs::create_dir_all(root.join("sock")).unwrap();
        let srv: &'static Server = Box::leak(Box::new(Server { root, port }));
        let stop = Stop(srv);
        run(&format!("{}/initdb", bin()), &["-D", &srv.data(), "-U", "postgres", "--auth-local=trust", "--auth-host=scram-sha-256", "-E", "UTF8"], &[], None)
            .expect("initdb");
        let opts = format!("-p {port} -k {} -c listen_addresses=127.0.0.1 -c password_encryption=scram-sha-256", srv.sock());
        run(&format!("{}/pg_ctl", bin()), &["-D", &srv.data(), "-o", &opts, "-l", &format!("{}.log", srv.data()), "-w", "start"], &[], None)
            .expect("pg_ctl start");
        (srv, stop)
    }
    pub(crate) fn major(&self) -> u32 {
        let v = run(&format!("{}/postgres", bin()), &["--version"], &[], None).expect("postgres --version");
        v.split_whitespace().last().and_then(|v| v.split('.').next()).and_then(|m| m.parse().ok()).expect("a major")
    }
    pub(crate) fn sock(&self) -> String {
        self.root.join("sock").to_string_lossy().into_owned()
    }
    pub(crate) fn data(&self) -> String {
        self.root.join("data").to_string_lossy().into_owned()
    }
    /// A script as `role` over the socket (trust): one session, every
    /// statement's rows, one per line. `Err` carries Postgres's message.
    pub(crate) fn script(&self, role: &str, db: &str, sql: &str) -> Result<Vec<String>, String> {
        let p = self.port.to_string();
        let out = run(
            &format!("{}/psql", bin()),
            &["-X", "-q", "-tA", "-h", &self.sock(), "-p", &p, "-U", role, "-d", db, "-v", "ON_ERROR_STOP=1"],
            &[],
            Some(sql.as_bytes()),
        )?;
        Ok(out.lines().map(str::to_string).collect())
    }
    pub(crate) fn su(&self, db: &str, sql: &str) -> Vec<String> {
        self.script("postgres", db, sql).unwrap_or_else(|e| panic!("superuser SQL failed: {e}\n{sql}"))
    }
}

pub(crate) struct Stop(&'static Server);

impl Drop for Stop {
    fn drop(&mut self) {
        let _ = run(&format!("{}/pg_ctl", bin()), &["-D", &self.0.data(), "-m", "immediate", "-w", "stop"], &[], None);
        let _ = std::fs::remove_dir_all(&self.0.root);
    }
}

impl SystemOps for Server {
    fn write_file(&self, path: &str, contents: &[u8], mode: u32) -> Result<(), String> {
        RealSystemOps.write_file(path, contents, mode)
    }
    fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        RealSystemOps.read_file(path)
    }
    fn create_dir_all(&self, path: &str) -> Result<(), String> {
        RealSystemOps.create_dir_all(path)
    }
    fn remove_path(&self, path: &str) -> Result<(), String> {
        RealSystemOps.remove_path(path)
    }
    fn path_exists(&self, path: &str) -> bool {
        RealSystemOps.path_exists(path)
    }
    fn systemctl(&self, args: &[&str]) -> Result<String, String> {
        Err(format!("systemctl {} is not part of the test", args.join(" ")))
    }
    fn systemctl_query(&self, _args: &[&str]) -> String {
        "active".into()
    }
    /// The root helper's `psql [--cluster M] -d <db> …`: superuser, over the
    /// socket.
    fn run_helper(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        if args.first().copied() != Some("psql") {
            return Err(format!("helper {} is not part of the test", args.join(" ")));
        }
        let mut rest: Vec<&str> = Vec::new();
        let mut it = args[1..].iter();
        while let Some(a) = it.next() {
            if *a == "--cluster" {
                it.next();
                continue;
            }
            rest.push(a);
        }
        let p = self.port.to_string();
        let sock = self.sock();
        let mut full: Vec<&str> = vec!["-X", "-h", &sock, "-p", &p, "-U", "postgres"];
        full.extend(rest);
        run(&format!("{}/psql", bin()), &full, &[], stdin)
    }
    /// The daemon's own psql: this server's binaries and port.
    fn run_cmd(&self, cmd: &str, args: &[&str], env: &[(&str, &str)], stdin: Option<&[u8]>) -> Result<Vec<u8>, String> {
        let tool = Path::new(cmd).file_name().and_then(|n| n.to_str()).unwrap_or(cmd);
        let mut env: Vec<(&str, &str)> = env.to_vec();
        env.push(("PSQLRC", "/dev/null"));
        let p = self.port.to_string();
        let mut mapped: Vec<&str> = args.to_vec();
        for i in 1..mapped.len() {
            if mapped[i - 1] == "-p" {
                mapped[i] = &p;
            }
        }
        RealSystemOps.run_cmd(&format!("{}/{tool}", bin()), &mapped, &env, stdin)
    }
    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// One named check: printed PASS/FAIL, collected; the test asserts none failed.
pub(crate) fn check(name: &str, ok: bool, detail: impl std::fmt::Debug, fails: &mut Vec<String>) {
    if ok {
        eprintln!("PASS {name}");
    } else {
        eprintln!("FAIL {name}: {detail:?}");
        fails.push(format!("{name}: {detail:?}"));
    }
}
