//! Canned bytes. The helper renders these itself (RH4): the daemon never
//! sends a unit file, only (for the Stalwart unit) the 64-hex recovery
//! password. A daemon and a helper of different releases therefore never
//! byte-compare each other's templates.

use crate::paths::{STALWART_BIN, STALWART_CONFIG, STALWART_USER};

/// The service unit K2 writes (upstream ships none for our layout).
/// `recovery_admin_password`: present during install (deterministic
/// bootstrap credential, ✔ live-verified `STALWART_RECOVERY_ADMIN`
/// env form); the `recovery-off` step rewrites the unit WITHOUT it
/// once the provisioned admin + ApiKey are vaulted, and rotates the
/// bootstrap principal `admin` (the env credential AND that principal
/// stay valid even in normal mode — verified — so leaving either would
/// be a standing backdoor). The unit is written 0600 for exactly this
/// reason. Hardening lives in the drop-in so the split mirrors PRD §10
/// verbatim and a future unit rewrite can't silently drop it.
pub fn systemd_unit(recovery_admin_password: Option<&str>) -> String {
    let env_line = match recovery_admin_password {
        Some(pw) => format!("Environment=STALWART_RECOVERY_ADMIN=admin:{pw}\n"),
        None => String::new(),
    };
    format!(
        "[Unit]\n\
         Description=Stalwart mail server (managed by the K2 Mail supervisor)\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={STALWART_BIN} --config {STALWART_CONFIG}\n\
         User={STALWART_USER}\n\
         Group={STALWART_USER}\n\
         {env_line}\
         StandardOutput=journal\n\
         StandardError=journal\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

/// PRD §10, verbatim — plus `/etc/stalwart` in ReadWritePaths:
/// ✔ LIVE-VERIFIED the Bootstrap/set persists the DataStore config to
/// that path AS THE stalwart USER (the first live attempt failed with
/// `Permission denied (os error 13)` until the dir was writable).
///
/// No `UMask=` yet. The root-helper PRD lists `UMask=0027` here, but the
/// backup B1 churn observer (shipped, `mail/backup.rs`) reads the store as
/// the daemon user and relies on RocksDB's 0644/0755 under the default
/// umask. `UMask=0027` lands with backup B2, together with the helper's
/// `backup store-inventory` verb that replaces that read.
pub fn hardening_dropin() -> &'static str {
    "[Service]\n\
     ProtectSystem=strict\n\
     ProtectHome=yes\n\
     ReadWritePaths=/var/lib/stalwart /var/log/stalwart /etc/stalwart\n\
     NoNewPrivileges=yes\n\
     PrivateTmp=yes\n\
     CapabilityBoundingSet=CAP_NET_BIND_SERVICE\n\
     AmbientCapabilities=CAP_NET_BIND_SERVICE\n\
     Restart=on-failure\n"
}

/// The daemon unit's hand-off drop-in (runtime stage H, vs-live AR3/K37):
///
/// - `KillMode=process`: a stop, restart or crash of the daemon kills the
///   daemon only. frpc, published services and skin gateway helpers live
///   in the same cgroup and survive to be re-adopted (stage H keeps the
///   tunnel up; `mixed` would still SIGKILL them once the main process
///   exits — checked on systemd 255).
/// - `NotifyAccess=main`: the hand-off's `MAINPID=<successor>` comes from
///   the main process itself. Never `all`: then any process in the unit
///   (an agent) could name itself the main pid and pass this helper's
///   daemon-caller check.
/// - `ExecReload=`: `systemctl reload` = a no-downtime hand-off (SIGUSR2).
///
/// Shared with the provisioning templates and the user-unit writer, so a
/// new box, a migrated box and a user-scope daemon all carry the same lines.
pub fn handoff_dropin() -> &'static str {
    "# K2 runtime stage H (front-door hand-off). Written by k2-root-helper\n\
     # `daemon handoff-dropin install`; remove with `... remove`.\n\
     [Service]\n\
     KillMode=process\n\
     NotifyAccess=main\n\
     ExecReload=/bin/kill -USR2 $MAINPID\n"
}

/// The recovery admin password the daemon may send to `mail render-unit`:
/// exactly 64 lowercase hex characters.
pub fn recovery_pw_ok(pw: &str) -> bool {
    pw.len() == 64 && pw.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// systemd drop-in for `postgresql.service` and `postgresql@.service`
/// (D29). `OOMPolicy=kill` = this cgroup only. Byte-identical to the
/// legacy `scripts/k2-pg-helper` body.
pub fn pg_ram_dropin() -> &'static str {
    "[Service]\nMemoryAccounting=yes\nMemoryHigh=25%\nMemoryMax=40%\nCPUWeight=50\nOOMPolicy=kill\n"
}

/// `/etc/postgresql/<n>/main/conf.d/99-k2-ram.conf` (D29 engine fence).
/// Byte-identical to the legacy `scripts/k2-pg-helper` body.
pub fn pg_guc_conf() -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_has_the_password_line_only_when_given() {
        let pw = "a".repeat(64);
        let with = systemd_unit(Some(&pw));
        assert!(with.contains(&format!("Environment=STALWART_RECOVERY_ADMIN=admin:{pw}\n")));
        assert!(!systemd_unit(None).contains("STALWART_RECOVERY_ADMIN"));
        assert!(with.starts_with("[Unit]\n"));
        assert!(
            with.contains("ExecStart=/usr/local/bin/stalwart --config /etc/stalwart/config.json\n")
        );
    }

    #[test]
    fn recovery_password_is_64_lower_hex() {
        assert!(recovery_pw_ok(&"0123456789abcdef".repeat(4)));
        for bad in [
            "".to_string(),
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            format!("{}\n", "a".repeat(63)),
            format!("{} ", "a".repeat(63)),
            "g".repeat(64),
        ] {
            assert!(!recovery_pw_ok(&bad), "{bad:?}");
        }
    }

    #[test]
    fn pg_templates_match_the_legacy_script_bytes() {
        // The legacy script heredocs, exactly (scripts/k2-pg-helper).
        let script = include_str!("../../../scripts/k2-pg-helper");
        let dropin_block = "[Service]\nMemoryAccounting=yes\nMemoryHigh=25%\nMemoryMax=40%\nCPUWeight=50\nOOMPolicy=kill\nEOF";
        assert!(script.contains(dropin_block));
        assert_eq!(format!("{}EOF", pg_ram_dropin()), dropin_block);
        let guc = format!("{}EOF", pg_guc_conf());
        assert!(
            script.contains(&guc),
            "guc conf drifted from the legacy script"
        );
    }
}
