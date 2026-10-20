//! Every fixed path the root helper touches or the daemon probes.
//!
//! Production paths are compiled in. The helper resolves each of them
//! beneath a root directory that is `/` in the binary and a temp dir in
//! tests (`k2_root_helper::Ctx`); nothing on argv, stdin or in the
//! environment can choose one.

// ── The helper itself ───────────────────────────────────────────────────

/// The installed helper. The sudoers line names exactly this path.
pub const HELPER_PATH: &str = "/usr/local/libexec/k2-root-helper";
/// Directory of [`HELPER_PATH`]; self-update stages and renames here.
pub const HELPER_DIR: &str = "/usr/local/libexec";
pub const HELPER_FILE: &str = "k2-root-helper";
/// One step back for `helper self-rollback`.
pub const HELPER_PREV_FILE: &str = "k2-root-helper.prev";
/// Where a rolled-back build is parked.
pub const HELPER_BAD_FILE: &str = "k2-root-helper.bad";
/// Self-update stage (a dot name in the same directory; never on PATH).
pub const HELPER_STAGE_FILE: &str = ".k2-root-helper.new";
/// Reserved: a future Arch/deb/rpm package installs the helper here and
/// owns its updates (§9.6). The helper refuses `self-update` from it.
pub const HELPER_PACKAGE_PATH: &str = "/usr/lib/k2/k2-root-helper";

/// `/etc/sudoers.d/k2-root-helper` — the one sudoers line.
pub const SUDOERS_PATH: &str = "/etc/sudoers.d/k2-root-helper";
/// Root-owned config written by the installer (who the daemon is).
pub const CONFIG_DIR: &str = "/etc/k2-root-helper";
pub const DAEMON_TOML: &str = "/etc/k2-root-helper/daemon.toml";
/// Root-owned state: the refuse list, the keep store.
pub const STATE_DIR: &str = "/var/lib/k2-root-helper";
pub const STATE_JSON: &str = "/var/lib/k2-root-helper/state.json";
/// `keep/<daemon user>/` (dirs 0700, files 0600). The user comes from the
/// caller check, never from argv.
pub const KEEP_DIR: &str = "/var/lib/k2-root-helper/keep";
/// Mutating verbs take an exclusive `flock` on
/// `/run/k2-root-helper.<namespace>.lock` (30 s timeout): one lock per
/// namespace, so a long mail snapshot never blocks a Postgres restart.
pub const LOCK_DIR: &str = "/run";

/// The daemon reaches the helper only through this sudo.
pub const SUDO_PATH: &str = "/usr/bin/sudo";
/// Executables the caller walk steps over (root-owned only). `sudo-rs`
/// ships as `/usr/bin/sudo-rs`, and Ubuntu 25.10+ resolves `/usr/bin/sudo`
/// to `/usr/lib/cargo/bin/sudo` through alternatives.
pub const SUDO_EXES: &[&str] = &[
    "/usr/bin/sudo",
    "/usr/bin/sudo-rs",
    "/usr/lib/cargo/bin/sudo",
];

/// The `k2` CLI on PATH (`provision-k2-server.sh`'s install). `helper
/// cli-refresh` replaces it when it is a root-owned K2 CLI older than the
/// helper's own copy; the daemon refreshes a copy it owns itself.
pub const CLI_PATH: &str = "/usr/local/bin/k2";
pub const CLI_DIR: &str = "/usr/local/bin";
pub const CLI_FILE: &str = "k2";

// ── Programs the helper spawns (absolute, env cleared) ──────────────────

pub const SYSTEMCTL: &str = "/usr/bin/systemctl";
/// `front install` schedules the daemon unit's one restart through it.
pub const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";
pub const USERADD: &str = "/usr/sbin/useradd";
pub const ID: &str = "/usr/bin/id";
pub const RUNUSER: &str = "/usr/sbin/runuser";
pub const PSQL: &str = "/usr/bin/psql";
pub const VISUDO: &str = "/usr/sbin/visudo";
pub const LOGGER: &str = "/usr/bin/logger";
pub const JOURNALCTL: &str = "/usr/bin/journalctl";
/// `PATH` every child gets.
pub const CHILD_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";

// ── Legacy helpers (one release train of coexistence) ───────────────────

pub const LEGACY_MAIL_HELPER: &str = "/usr/local/libexec/k2-mail-helper";
pub const LEGACY_MAIL_SUDOERS: &str = "/etc/sudoers.d/k2-mail-helper";
pub const LEGACY_PG_HELPER: &str = "/usr/local/libexec/k2-pg-helper";
pub const LEGACY_PG_SUDOERS: &str = "/etc/sudoers.d/k2-pg-helper";

// ── Stalwart (hosted mail) ──────────────────────────────────────────────

pub const STALWART_BIN: &str = "/usr/local/bin/stalwart";
pub const STALWART_CONFIG_DIR: &str = "/etc/stalwart";
pub const STALWART_CONFIG: &str = "/etc/stalwart/config.json";
pub const STALWART_DATA_DIR: &str = "/var/lib/stalwart";
pub const STALWART_LOG_DIR: &str = "/var/log/stalwart";
pub const STALWART_USER: &str = "stalwart";
pub const STALWART_UNIT: &str = "stalwart";
pub const STALWART_UNIT_PATH: &str = "/etc/systemd/system/stalwart.service";
pub const STALWART_DROPIN_DIR: &str = "/etc/systemd/system/stalwart.service.d";
pub const STALWART_DROPIN_PATH: &str = "/etc/systemd/system/stalwart.service.d/k2-hardening.conf";
/// `k2 hostmail upgrade`'s pre-upgrade snapshot (helper `snapshot-data`):
/// `data/` = a copy of [`STALWART_DATA_DIR`], `config/` = a copy of
/// [`STALWART_CONFIG_DIR`], plus [`STALWART_SNAPSHOT_MARKER`] written
/// last. Fixed path, root 0700; kept after a successful upgrade until the
/// next upgrade replaces it.
pub const STALWART_SNAPSHOT_DIR: &str = "/var/lib/stalwart.k2-snap";
/// Written LAST inside the snapshot; no marker = incomplete, and restore
/// refuses it.
pub const STALWART_SNAPSHOT_MARKER: &str = "K2-SNAPSHOT.json";
/// Where `install-bin` stages the binary before the atomic rename onto
/// [`STALWART_BIN`] (never a truncate-in-place of a live binary).
pub const STALWART_BIN_STAGING: &str = "/usr/local/bin/.stalwart.k2-new";
/// RocksDB's lock file inside the store (`{data}/data/LOCK`); a process
/// that still has it locked still has the store open.
pub const STALWART_STORE_LOCK: &str = "/var/lib/stalwart/data/LOCK";

// ── The daemon's own unit (runtime stage H) ─────────────────────────────

/// System unit directory. The daemon's drop-in dir is
/// `<this>/<unit from daemon.toml>.d` (the unit never comes from argv).
pub const SYSTEMD_SYSTEM_DIR: &str = "/etc/systemd/system";
/// The hand-off drop-in's file name, inside that unit's `.d` dir.
pub const HANDOFF_DROPIN_FILE: &str = "k2-handoff.conf";

// ── front443: the box :443 socket held for the daemon (protocol 8) ──────

/// The socket unit `front install` writes (fixed name, fixed template).
pub const FRONT_SOCKET_UNIT: &str = "k2-front-443.socket";
/// Its file name inside [`SYSTEMD_SYSTEM_DIR`].
pub const FRONT_SOCKET_FILE: &str = "k2-front-443.socket";
/// The daemon drop-in (`Sockets=`) inside `<unit>.d`.
pub const FRONT_DROPIN_FILE: &str = "k2-front.conf";
/// `FileDescriptorName=`: how the daemon finds the fd in `LISTEN_FDNAMES`.
pub const FRONT_FD_NAME: &str = "k2-front-443";
/// The transient unit `front install` uses to restart the daemon's unit a
/// few seconds later (`k2-front-restart.timer` + `.service`).
pub const FRONT_RESTART_UNIT: &str = "k2-front-restart";
/// How long after `front install` the daemon's unit restarts (its HTTP
/// answer goes out first).
pub const FRONT_RESTART_DELAY_SECS: u32 = 3;

// ── Postgres sidecar ────────────────────────────────────────────────────

/// Distro-owned unit — K2 drops in, never rewrites postgresql.service.
pub const PG_RAM_DROPIN_DIR: &str = "/etc/systemd/system/postgresql.service.d";
pub const PG_RAM_DROPIN_PATH: &str = "/etc/systemd/system/postgresql.service.d/k2-ram.conf";
/// `postgresql.service` is a oneshot wrapper on Debian/Ubuntu; the
/// postmaster is `postgresql@.service`, so the fence goes on both.
pub const PG_RAM_TEMPLATE_DROPIN_DIR: &str = "/etc/systemd/system/postgresql@.service.d";
pub const PG_RAM_TEMPLATE_DROPIN_PATH: &str =
    "/etc/systemd/system/postgresql@.service.d/k2-ram.conf";
/// `/etc/postgresql/<n>/main/conf.d/99-k2-ram.conf` for each cluster.
pub const PG_ETC: &str = "/etc/postgresql";
pub const PG_CONFD_FILE: &str = "99-k2-ram.conf";
pub const PG_OS_USER: &str = "postgres";
