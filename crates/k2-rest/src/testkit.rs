//! A throwaway Postgres server for real-Postgres tests (feature `testkit`,
//! never shipped).
//!
//! The server's binaries come from `REALPG_BIN` (default
//! `/usr/lib/postgresql/16/bin`, what `--needs sys:postgresql` installs on
//! an Ubuntu 24.04 compute node). It lives under `/tmp/k2rest-<pid>-<n>`,
//! listens on 127.0.0.1 at a port picked free, takes SCRAM passwords over
//! TCP and trusts its own Unix socket (the superuser's lane). Nothing here
//! skips: a missing binary or a failed start panics with the reason.
//!
//! Tests share one server per process ([`shared`]) and make their own
//! database each ([`Server::fresh_db`]), so they run in parallel.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

/// Where the server's binaries are.
pub fn bin_dir() -> String {
    std::env::var("REALPG_BIN").unwrap_or_else(|_| "/usr/lib/postgresql/16/bin".into())
}

fn run(prog: &str, args: &[&str], env: &[(&str, &str)]) -> Result<String, String> {
    let mut c = Command::new(prog);
    c.args(args).env("LC_ALL", "C").stdin(Stdio::null());
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c.output().map_err(|e| format!("{prog}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        Err(format!("{prog} {args:?}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_string())
    }
}

/// One running throwaway server.
pub struct Server {
    pub root: PathBuf,
    pub port: u16,
    pub major: String,
}

static SEQ: AtomicU32 = AtomicU32::new(0);

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
    l.local_addr().expect("local addr").port()
}

impl Server {
    /// initdb + start. Panics (with the server's own words) on any failure.
    pub fn start() -> Server {
        let bin = bin_dir();
        assert!(Path::new(&format!("{bin}/initdb")).exists(), "{bin}/initdb is missing: install Postgres or set REALPG_BIN");
        let version = run(&format!("{bin}/postgres"), &["--version"], &[]).expect("postgres --version");
        let major = version.split_whitespace().nth(2).unwrap_or("").split('.').next().unwrap_or("").to_string();
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        // Short: a Unix socket path must stay under 108 bytes.
        let root = PathBuf::from(format!("/tmp/k2rest-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sock")).expect("make the server folder");
        let data = root.join("data").to_string_lossy().into_owned();
        run(
            &format!("{bin}/initdb"),
            &["-D", &data, "-U", "postgres", "--auth-local=trust", "--auth-host=scram-sha-256", "-E", "UTF8", "--locale=C"],
            &[],
        )
        .expect("initdb");
        let port = free_port();
        let opts = format!(
            "-p {port} -k {} -c listen_addresses=127.0.0.1 -c password_encryption=scram-sha-256 -c max_connections=200 -c fsync=off -c jit=off",
            root.join("sock").display()
        );
        let log = root.join("server.log").to_string_lossy().into_owned();
        run(&format!("{bin}/pg_ctl"), &["-D", &data, "-o", &opts, "-l", &log, "-w", "start"], &[])
            .unwrap_or_else(|e| panic!("pg_ctl start: {e}\n{}", std::fs::read_to_string(&log).unwrap_or_default()));
        eprintln!("[testkit] {version} on 127.0.0.1:{port} ({})", root.display());
        let s = Server { root, port, major };
        // The three standard roles, once per cluster (as lane A makes them;
        // per database would race between tests).
        s.su(
            "postgres",
            &["CREATE ROLE anon NOLOGIN", "CREATE ROLE authenticated NOLOGIN", "CREATE ROLE service_role NOLOGIN BYPASSRLS"],
        );
        s
    }

    fn sock(&self) -> String {
        self.root.join("sock").to_string_lossy().into_owned()
    }

    /// SQL as the superuser over the socket, one `psql -c` per statement,
    /// stop at the first error. Returns the last statement's rows
    /// (unaligned, tab-separated, no headers).
    pub fn su(&self, db: &str, sqls: &[&str]) -> String {
        let bin = bin_dir();
        let port = self.port.to_string();
        let sock = self.sock();
        let mut last = String::new();
        for sql in sqls {
            last = run(
                &format!("{bin}/psql"),
                &["-h", &sock, "-p", &port, "-U", "postgres", "-d", db, "-v", "ON_ERROR_STOP=1", "-X", "-q", "-tA", "-F", "\t", "-c", sql],
                &[],
            )
            .unwrap_or_else(|e| panic!("superuser SQL failed on {db}: {e}\n{sql}"));
        }
        last
    }

    /// A new empty database named `<prefix>_<n>`, owned by a new login
    /// `<name>_migrator` (password `mig-pass`), with K2's Supabase-compat
    /// roles in place as the fixture makes them ([`compat_roles`]).
    pub fn fresh_db(&self, prefix: &str) -> String {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let name = format!("{prefix}_{n}");
        self.su(
            "postgres",
            &[
                &format!("CREATE ROLE {name}_migrator LOGIN PASSWORD 'mig-pass'"),
                &format!("CREATE DATABASE {name} OWNER {name}_migrator"),
                &format!("REVOKE CONNECT ON DATABASE {name} FROM PUBLIC"),
                &format!("GRANT CONNECT ON DATABASE {name} TO {name}_migrator"),
            ],
        );
        compat_roles(self, &name);
        name
    }

    /// A libpq conninfo for `user` on `db` over TCP.
    pub fn conninfo(&self, db: &str, user: &str, password: &str) -> String {
        format!("host=127.0.0.1 port={} user={user} password={password} dbname={db}", self.port)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let data = self.root.join("data").to_string_lossy().into_owned();
        let _ = run(&format!("{}/pg_ctl", bin_dir()), &["-D", &data, "-m", "immediate", "-w", "stop"], &[]);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The authenticator login's fixture password.
pub const AUTHENTICATOR_PASSWORD: &str = "auth-pass";

/// The roles the compat layer stands on (PRD §5.1), as a fixture: `anon`,
/// `authenticated` and `service_role` (BYPASSRLS) NOLOGIN once per cluster,
/// and `<db>_authenticator` (NOINHERIT, a member of the three, CONNECT on
/// its own database). Lane A makes the real ones; tests stand on these
/// until it lands. Also the minimal `auth` helpers of the §5.2 contract,
/// read from `request.jwt.claims` as Supabase defines them, and only for a
/// session logged in as the authenticator (lane A's rule).
pub fn compat_roles(s: &Server, db: &str) {
    s.su(
        "postgres",
        &[
            &format!("CREATE ROLE {db}_authenticator LOGIN NOINHERIT PASSWORD '{AUTHENTICATOR_PASSWORD}'"),
            &format!("GRANT anon, authenticated, service_role TO {db}_authenticator"),
            &format!("GRANT CONNECT ON DATABASE {db} TO {db}_authenticator"),
        ],
    );
    // Lane A's contract: the helpers read the request's claims only when
    // the session's login is the database's authenticator (the doors' pool).
    let claims = format!("(CASE WHEN session_user::text = '{db}_authenticator' THEN nullif(current_setting('request.jwt.claims', true), '') END)");
    let sub = format!("(CASE WHEN session_user::text = '{db}_authenticator' THEN nullif(current_setting('request.jwt.claim.sub', true), '') END)");
    s.su(
        db,
        &[
            "CREATE SCHEMA IF NOT EXISTS auth",
            "GRANT USAGE ON SCHEMA auth TO anon, authenticated, service_role",
            "GRANT USAGE ON SCHEMA public TO anon, authenticated, service_role",
            &format!("CREATE OR REPLACE FUNCTION auth.jwt() RETURNS jsonb LANGUAGE sql STABLE AS $$ SELECT {claims}::jsonb $$"),
            &format!("CREATE OR REPLACE FUNCTION auth.uid() RETURNS uuid LANGUAGE sql STABLE AS $$ SELECT coalesce({sub}, {claims}::jsonb ->> 'sub')::uuid $$"),
            &format!("CREATE OR REPLACE FUNCTION auth.role() RETURNS text LANGUAGE sql STABLE AS $$ SELECT ({claims}::jsonb ->> 'role')::text $$"),
            &format!("CREATE OR REPLACE FUNCTION auth.email() RETURNS text LANGUAGE sql STABLE AS $$ SELECT ({claims}::jsonb ->> 'email')::text $$"),
            "GRANT USAGE ON SCHEMA auth TO PUBLIC",
        ],
    );
}

/// The process's one server, started on first use. A static is never
/// dropped, so nothing stops it at exit: the compute job's command stops
/// its servers itself (`pg_ctl stop` over `/tmp/k2rest-*`) when the tests
/// end.
pub fn shared() -> &'static Server {
    static S: OnceLock<Server> = OnceLock::new();
    S.get_or_init(Server::start)
}
