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
/// - `OOMPolicy=continue`: agents run inside the daemon's cgroup. With the
///   default `stop`, the kernel OOM killer taking one agent (or frpc, or an
///   App helper) makes systemd stop the whole unit and `Restart=always`
///   bring it back: every session and tunnel drops for one runaway agent.
///   `continue` lets only the killed process die (verified on the build
///   box). If the daemon itself is OOM-killed, its main process exits and
///   `Restart=always` still restarts it. Applies on `daemon-reload`, no
///   restart needed.
/// - `RuntimeDirectory=k2` + `RuntimeDirectoryMode=0700` +
///   `RuntimeDirectoryPreserve=yes` (0.46.4, akzm 2026-10-24): the
///   daemon's private runtime dir (frpc.toml, agents' secret files) is the
///   unit's own `/run/k2` (`$RUNTIME_DIRECTORY`), not `/run/user/<uid>` or
///   `/dev/shm`, which systemd-logind deletes when the user's last login
///   session ends (no linger, `RemoveIPC=yes`). `Preserve=yes`: frpc (and
///   k2-ptyd agents) outlive a daemon stop under `KillMode=process`, so
///   the dir must too. Takes effect at the unit's next start.
///
/// Shared with the provisioning templates and the user-unit writer, so a
/// new box, a migrated box and a user-scope daemon all carry the same lines.
/// The daemon re-asserts it every boot ([`handoff_dropin`] bytes compare),
/// so a box on an older drop-in picks up a new line on update.
pub fn handoff_dropin() -> &'static str {
    "# K2 runtime stage H (front-door hand-off). Written by k2-root-helper\n\
     # `daemon handoff-dropin install`; remove with `... remove`.\n\
     [Service]\n\
     KillMode=process\n\
     NotifyAccess=main\n\
     ExecReload=/bin/kill -USR2 $MAINPID\n\
     OOMPolicy=continue\n\
     RuntimeDirectory=k2\n\
     RuntimeDirectoryMode=0700\n\
     RuntimeDirectoryPreserve=yes\n"
}

/// front443's socket unit (`/etc/systemd/system/k2-front-443.socket`,
/// helper `front install`, protocol 8). systemd owns the box's :443 and
/// hands the listening socket to the daemon (`LISTEN_FDS`, named
/// [`crate::paths::FRONT_FD_NAME`]); the daemon holds no capability, and
/// agent PTYs never inherit one (ambient capabilities were rejected for
/// exactly that).
///
/// - `ListenStream=[::]:443` + `BindIPv6Only=both`: one dual-stack socket.
/// - `Service=`: the daemon's own unit, from `daemon.toml` (never argv).
///   Required: without it systemd pairs the socket with a
///   `k2-front-443.service` that does not exist and refuses to start it.
///   A connection while the daemon is down starts the daemon (`kill -9`
///   leaves systemd holding :443 and the queue).
/// - No `[Install]`: the daemon drop-in's `Wants=` pulls the socket in at
///   boot, so `front remove` leaves no enablement symlink behind.
///
/// `None` when `service_unit` is not a valid unit name
/// ([`crate::caller::unit_ok`]): the template never carries anything else.
pub fn front_socket_unit(service_unit: &str) -> Option<String> {
    if !crate::caller::unit_ok(service_unit) {
        return None;
    }
    Some(format!(
        "# K2 front443: the box :443 SNI router's socket. Written by k2-root-helper\n\
         # `front install`; remove with `front remove`.\n\
         [Unit]\n\
         Description=K2 box :443 front (SNI pass-through router in the K2 daemon)\n\
         \n\
         [Socket]\n\
         ListenStream=[::]:443\n\
         BindIPv6Only=both\n\
         FileDescriptorName={fd}\n\
         Service={service_unit}\n\
         NoDelay=true\n\
         Backlog=4096\n",
        fd = crate::paths::FRONT_FD_NAME,
    ))
}

/// The daemon unit's front443 drop-in (`<unit>.d/k2-front.conf`): the
/// daemon inherits the socket's fd on every start, and the socket is
/// listening before the daemon starts.
pub fn front_dropin() -> &'static str {
    "# K2 front443. Written by k2-root-helper `front install`; remove with\n\
     # `front remove`.\n\
     [Unit]\n\
     Wants=k2-front-443.socket\n\
     After=k2-front-443.socket\n\
     \n\
     [Service]\n\
     Sockets=k2-front-443.socket\n"
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
    fn front_templates_are_fixed_and_carry_only_a_valid_unit() {
        let u = front_socket_unit("k2-daemon.service").unwrap();
        assert!(u.contains("\n[Socket]\nListenStream=[::]:443\nBindIPv6Only=both\n"), "{u}");
        assert!(u.contains("\nFileDescriptorName=k2-front-443\n"), "{u}");
        assert!(u.contains("\nService=k2-daemon.service\n"), "{u}");
        assert!(!u.contains("[Install]"), "no enablement symlink: {u}");
        assert_eq!(u.matches("ListenStream=").count(), 1);
        // Exactly the canned lines: every line is a comment, a section or
        // one of the fixed keys.
        for line in u.lines() {
            let fixed = line.is_empty()
                || line.starts_with('#')
                || matches!(line, "[Unit]" | "[Socket]")
                || ["Description=", "ListenStream=", "BindIPv6Only=", "FileDescriptorName=", "Service=", "NoDelay=", "Backlog="]
                    .iter()
                    .any(|k| line.starts_with(k));
            assert!(fixed, "unexpected line {line:?}");
        }
        assert_eq!(
            front_socket_unit("k2-daemon-std.service").unwrap().replace("k2-daemon-std.service", "k2-daemon.service"),
            u
        );
        for bad in [
            "",
            "k2-daemon",
            "k2-daemon.service\nExecStartPre=/bin/sh",
            "../x.service",
            "x.service\n[Socket]\nListenStream=0.0.0.0:22",
            "a b.service",
            "x.socket",
        ] {
            assert!(front_socket_unit(bad).is_none(), "{bad:?}");
        }
        let d = front_dropin();
        assert!(d.contains("\n[Service]\nSockets=k2-front-443.socket\n"), "{d}");
        assert!(d.contains("\nWants=k2-front-443.socket\nAfter=k2-front-443.socket\n"), "{d}");
        assert!(!d.contains("Ambient") && !d.contains("Capabilit"), "no capability, ever: {d}");
    }

    #[test]
    fn handoff_dropin_keeps_the_daemon_up_when_an_agent_is_oom_killed() {
        let d = handoff_dropin();
        let service: Vec<&str> = d.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(
            service,
            [
                "[Service]",
                "KillMode=process",
                "NotifyAccess=main",
                "ExecReload=/bin/kill -USR2 $MAINPID",
                "OOMPolicy=continue",
                "RuntimeDirectory=k2",
                "RuntimeDirectoryMode=0700",
                "RuntimeDirectoryPreserve=yes",
            ]
        );
        assert!(d.ends_with('\n'));
    }

    /// The runtime dir systemd-logind can't delete (akzm 2026-10-24): the
    /// unit's own `/run/k2`, private, kept across restarts (frpc and
    /// k2-ptyd agents outlive a daemon stop under `KillMode=process`).
    #[test]
    fn handoff_dropin_gives_the_daemon_its_own_runtime_dir() {
        let d = handoff_dropin();
        let lines: Vec<&str> = d.lines().collect();
        let at = |k: &str| lines.iter().position(|l| *l == k).unwrap_or_else(|| panic!("missing {k:?} in {d}"));
        let service = at("[Service]");
        for want in ["RuntimeDirectory=k2", "RuntimeDirectoryMode=0700", "RuntimeDirectoryPreserve=yes"] {
            assert!(at(want) > service, "{want} must sit in [Service]: {d}");
        }
        assert_eq!(d.matches("RuntimeDirectory=").count(), 1, "{d}");
        assert!(!d.contains("RuntimeDirectoryPreserve=no") && !d.contains("RuntimeDirectoryPreserve=restart"));
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
