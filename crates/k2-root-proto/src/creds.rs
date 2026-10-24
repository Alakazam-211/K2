//! The `creds` namespace (protocol 10, 0.46.4, `prd-vault-gaps-v1` item 4):
//! systemd encrypted credentials K2 writes for units it doesn't run
//! (`k2 vault systemd-cred`).
//!
//! Everything both sides must agree on: the cred-name and unit-name rules,
//! the allow-list file and its parser, the deny list of K2's own units, the
//! fixed paths, the canned drop-in bytes and the parsers of `systemctl
//! show`. The helper builds every path itself from a validated name;
//! nothing on argv is ever a path.
//!
//! **Which units (security review fixes, fail closed).**
//! 1. A unit must be on the box's allow list, [`ALLOW_LIST`]: a root-owned
//!    file the box's admin writes over SSH (or `install-root-helper.sh
//!    --creds-unit <unit>`). No file = no unit may get a credential.
//! 2. The deny list stays as a second guard: K2's own units by exact name,
//!    K2's generated families, core system units, and every unit
//!    `daemon.toml` names (read at call time by the helper).
//! 3. The name rule refuses templates and instances (`@`) and stems ending
//!    in `-` (systemd's prefix drop-in dirs, `k2-.service.d/`, reach every
//!    `k2-*` unit).
//! 4. The helper resolves the unit with `systemctl show -p
//!    LoadState,Id,Names`: it must be `loaded`, its `Id` must be the name
//!    asked for (no aliases), and every name it has passes 1–3.
//!
//! **Where.** K2's credentials live in [`CREDSTORE_DIR`], a K2-only root
//! directory outside systemd's credential search path
//! (`/etc/credstore.encrypted`, `/run/credstore.encrypted`, …), and the
//! drop-in names the file by absolute path. So no unit picks one up by bare
//! name or `ImportCredential=` glob. Cred names in systemd's reserved
//! credential namespaces ([`RESERVED_PREFIXES`]) are refused as well.

/// K2's own credential store: root 0700, outside systemd's search path.
pub const CREDSTORE_DIR: &str = "/etc/k2/credstore.encrypted";
/// Its parent (root, not group/other-writable; also holds the vault's own
/// KEK credential on boxes that use one).
pub const CREDSTORE_PARENT: &str = "/etc/k2";
/// `<cred>.cred` inside [`CREDSTORE_DIR`].
pub const CRED_SUFFIX: &str = ".cred";
/// The units this box's admin lets K2 put credentials on, one per line
/// (`#` comments). Root-owned, not group/other-writable, a regular file.
pub const ALLOW_LIST: &str = "/etc/k2-root-helper/systemd-cred-units";
/// Largest allow-list file read.
pub const ALLOW_LIST_MAX: usize = 16 * 1024;
/// What the helper wrote, by which daemon. Its own file, so an older
/// helper's strict `state.json` reader never sees fields it doesn't know.
pub const CREDS_STATE: &str = "/var/lib/k2-root-helper/creds.json";
/// The drop-in file is `k2-cred-<cred>.conf` inside `<unit>.d`.
pub const DROPIN_PREFIX: &str = "k2-cred-";
pub const DROPIN_SUFFIX: &str = ".conf";
/// systemd-creds itself (systemd ≥ 250).
pub const SYSTEMD_CREDS: &str = "/usr/bin/systemd-creds";
/// The oldest systemd whose `systemd-creds encrypt --with-key=auto` and
/// `LoadCredentialEncrypted=<id>:<path>` K2 relies on.
pub const MIN_SYSTEMD: u32 = 250;
/// `creds put` stdin cap (the plaintext value).
pub const CAP_CRED: usize = 64 * 1024;
/// Longest unit name accepted.
pub const UNIT_MAX: usize = 128;
pub const CRED_MAX: usize = 64;
/// `creds restart`: at most one per unit in this many seconds.
pub const RESTART_MIN_INTERVAL_SECS: u64 = 30;

/// systemd's reserved credential namespaces (systemd.system-credentials(7),
/// systemd 250–257, plus room): stock units import credentials under these
/// names and act on them as root (tmpfiles, sysusers, getty, sshd, network,
/// firstboot, …). Compared case-insensitively.
pub const RESERVED_PREFIXES: &[&str] = &[
    "agetty.",
    "boot.",
    "cryptenroll.",
    "cryptsetup.",
    "firstboot.",
    "fstab.",
    "getty.",
    "home.",
    "import.",
    "journal.",
    "kernel.",
    "login.",
    "machine.",
    "mkfs.",
    "network.",
    "nspawn.",
    "passwd.",
    "repart.",
    "resolve.",
    "resolved.",
    "shell.",
    "ssh.",
    "sysctl.",
    "sysext.",
    "system.",
    "systemd.",
    "sysupdate.",
    "sysusers.",
    "tmpfiles.",
    "udev.",
    "userdb.",
    "vconsole.",
    "vmm.",
];

/// `[A-Za-z0-9_.-]{1,64}`, no leading `.` or `-`, not `.`/`..`, no `/`, not
/// in a reserved namespace ([`RESERVED_PREFIXES`]).
pub fn cred_name_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= CRED_MAX
        && b[0] != b'.'
        && b[0] != b'-'
        && s != "."
        && s != ".."
        && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
        && reserved_prefix(s).is_none()
}

/// The reserved namespace `s` falls in, if any.
pub fn reserved_prefix(s: &str) -> Option<&'static str> {
    let l = s.to_ascii_lowercase();
    RESERVED_PREFIXES.iter().copied().find(|p| l.starts_with(p))
}

/// `[A-Za-z0-9_.-]{1,128}` ending `.service`; no leading `.` or `-` (an
/// option to systemctl); no `@` (templates and instances); the stem doesn't
/// end in `-` (a prefix drop-in dir reaches every unit starting with it).
pub fn unit_name_ok(s: &str) -> bool {
    let Some(stem) = s.strip_suffix(".service") else {
        return false;
    };
    let b = s.as_bytes();
    !stem.is_empty()
        && b.len() <= UNIT_MAX
        && !matches!(b[0], b'.' | b'-')
        && !stem.ends_with('-')
        && !stem.ends_with('.')
        && b.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// K2's own units, by exact name. Each line says where K2 installs it.
pub const K2_OWN_UNITS: &[&str] = &[
    // The daemon: the default unit (provision, root-helper installer), the
    // home-dir package unit (scripts/package-daemon-deb.sh), the second
    // daemon of a two-daemon box, and the old names a box may still run.
    "k2-daemon.service",
    "k2-daemon-home.service",
    "k2-daemon-std.service",
    "k2.service",
    "k2so-daemon.service",
    // The App engine (crates/k2-root-proto/src/engine.rs).
    "k2-app-engine.service",
    // k2-node and its admin/packages units (crates/k2-node, scripts/node).
    "k2-node.service",
    "k2-node-admin.service",
    "k2-node-packages.service",
    // front443's transient restart (crates/k2-root-proto/src/paths.rs).
    "k2-front-restart.service",
    // Hosted mail (crates/k2-root-proto/src/paths.rs).
    "stalwart.service",
    // Sew (crates/k2-root-proto/src/sew.rs, k2-root-helper/src/sew_teardown.rs).
    "sew.service",
    "sew-db.service",
    "sew-migrate.service",
    "sew-media.service",
    "sew-recorder.service",
    "sew-transcriber.service",
    // The distro's Postgres wrapper K2 drives (pg namespace).
    "postgresql.service",
];

/// K2's generated families, as (prefix, middle) patterns: the stem starts
/// with `prefix` and, when `middle` is set, contains it after the prefix.
/// `k2app-process@<app>`, App deployments `k2app-<app>--<deployment>`,
/// compute jobs `k2job-<id>`, Postgres clusters `postgresql@<n>-<name>`,
/// Sew's timers' services `sew-backup@…`, `sew-drill@…`, and `k2test-*`
/// (test fixtures: always denied, so a fixture name never becomes a real
/// allowed one).
pub const K2_UNIT_FAMILIES: &[(&str, Option<&str>)] = &[
    ("k2app-process@", None),
    ("k2app-", Some("--")),
    ("k2job-", None),
    ("postgresql@", None),
    ("sew-backup@", None),
    ("sew-drill@", None),
    ("k2test-", None),
    // K2 daemon units are k2-daemon-<box> on hosted boxes (k2-daemon-nsi,
    // a second daemon's k2-daemon-b, …); daemon.toml names them at call
    // time, and this family keeps a box's own name out even when it isn't.
    ("k2-daemon-", None),
];

/// Core system units, by exact name or `prefix*`. The allow list is the
/// wall; this is the belt.
pub const SYSTEM_UNITS: &[&str] = &[
    "ssh.service",
    "sshd.service",
    "console-getty.service",
    "emergency.service",
    "rescue.service",
    "debug-shell.service",
    "networking.service",
    "NetworkManager.service",
    "cron.service",
    "crond.service",
    "polkit.service",
    "docker.service",
    "containerd.service",
    "firewalld.service",
    "nftables.service",
    "ufw.service",
    "tailscaled.service",
    "udev.service",
];
pub const SYSTEM_PREFIXES: &[&str] = &[
    "systemd-",
    "dbus",
    "getty@",
    "serial-getty@",
    "container-getty@",
    "autovt@",
    "user@",
    "user-runtime-dir@",
    "wg-quick@",
    "cloud-",
];

/// Why `unit` may not get a K2 credential by the deny list, or `None`.
/// `daemon_units` are the units `daemon.toml` names (read at call time).
pub fn deny_reason(unit: &str, daemon_units: &[String]) -> Option<&'static str> {
    if !unit_name_ok(unit) {
        return Some("not a plain unit name (no templates, instances or prefix names)");
    }
    if daemon_units.iter().any(|d| d == unit) {
        return Some("a K2 daemon unit (daemon.toml)");
    }
    if K2_OWN_UNITS.contains(&unit) {
        return Some("one of K2's own units");
    }
    let stem = unit.strip_suffix(".service").unwrap_or(unit);
    for (prefix, middle) in K2_UNIT_FAMILIES {
        if let Some(rest) = stem.strip_prefix(prefix) {
            if middle.is_none_or(|m| rest.contains(m)) {
                return Some("a unit K2 generates");
            }
        }
    }
    if SYSTEM_UNITS.contains(&unit) || SYSTEM_PREFIXES.iter().any(|p| unit.starts_with(p)) {
        return Some("a core system unit");
    }
    None
}

/// The allow-list file: one unit per line, blank lines and `#` comments
/// ignored. Any line that isn't a plain unit name fails the whole file
/// (fail closed).
pub fn parse_allow_list(text: &str) -> Result<Vec<String>, &'static str> {
    let mut out = vec![];
    for line in text.lines() {
        let l = line.split('#').next().unwrap_or("").trim();
        if l.is_empty() {
            continue;
        }
        if !unit_name_ok(l) {
            return Err("the systemd-cred allow list has a line that isn't a plain <name>.service");
        }
        if !out.iter().any(|u| u == l) {
            out.push(l.to_string());
        }
    }
    Ok(out)
}

/// Why a unit (one of its names) can't get a credential: off the allow
/// list, or on the deny list. `None` = allowed.
pub fn unit_refusal(name: &str, allow: &[String], daemon_units: &[String]) -> Option<&'static str> {
    if let Some(why) = deny_reason(name, daemon_units) {
        return Some(why);
    }
    if !allow.iter().any(|a| a == name) {
        return Some("not on this box's systemd-cred allow list (/etc/k2-root-helper/systemd-cred-units)");
    }
    None
}

/// `systemctl show -p LoadState,Id,Names <unit>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitShow {
    pub load_state: String,
    pub id: String,
    pub names: Vec<String>,
}

pub fn parse_unit_show(out: &str) -> UnitShow {
    let mut s = UnitShow::default();
    for line in out.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "LoadState" => s.load_state = v.trim().to_string(),
            "Id" => s.id = v.trim().to_string(),
            "Names" => s.names = v.split_whitespace().map(str::to_string).collect(),
            _ => {}
        }
    }
    s
}

/// busctl (systemd's own D-Bus tool, absolute, env cleared). `systemctl
/// show` can't be used for credentials: it prints `a(ss)`/`a(say)`
/// properties as `[unprintable]` or not at all (review A2-N1).
pub const BUSCTL: &str = "/usr/bin/busctl";

/// The `org.freedesktop.systemd1.Service` properties that name a unit's
/// credentials, with the D-Bus signature each must have.
pub const CRED_PROPS: &[(&str, &str)] = &[
    ("LoadCredential", "a(ss)"),
    ("LoadCredentialEncrypted", "a(ss)"),
    ("SetCredential", "a(say)"),
    ("SetCredentialEncrypted", "a(say)"),
    ("ImportCredential", "as"),
    // systemd 257: `ImportCredential=GLOB:RENAME` (review A3-P2).
    ("ImportCredentialEx", "a(ss)"),
];
/// Properties an older systemd doesn't have (ImportCredential < 254,
/// ImportCredentialEx < 257): absent = empty.
pub const OPTIONAL_CRED_PROPS: &[&str] = &["ImportCredential", "ImportCredentialEx"];

/// A unit's credentials as systemd holds them after loading every file
/// (the fragment and all drop-ins): what `busctl get-property` answers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitCreds {
    /// `LoadCredential=` (id, path).
    pub load: Vec<(String, String)>,
    /// `LoadCredentialEncrypted=` (id, path).
    pub load_encrypted: Vec<(String, String)>,
    /// `SetCredential*=` ids.
    pub set_ids: Vec<String>,
    /// `ImportCredential=` globs.
    pub import_globs: Vec<String>,
    /// `ImportCredential=GLOB:RENAME` (glob, rename prefix), systemd 257.
    pub import_renames: Vec<(String, String)>,
}

/// A systemd unit object path (`/org/freedesktop/systemd1/unit/<escaped>`).
pub fn object_path_ok(p: &str) -> bool {
    p.strip_prefix("/org/freedesktop/systemd1/unit/")
        .is_some_and(|rest| !rest.is_empty() && rest.len() <= 400 && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
}

/// `busctl --json=short call org.freedesktop.systemd1
/// /org/freedesktop/systemd1 org.freedesktop.systemd1.Manager GetUnit s
/// <unit>` → `{"type":"o","data":["/org/freedesktop/systemd1/unit/…"]}`.
pub fn parse_unit_path(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
    if v["type"] != "o" {
        return None;
    }
    let p = v["data"].as_array()?.first()?.as_str()?.to_string();
    object_path_ok(&p).then_some(p)
}

/// One `busctl --json=short get-property … <prop>` answer
/// (`{"type":"a(ss)","data":[["id","path"],…]}`) into `c`. A wrong type or
/// shape is an error (fail closed).
pub fn apply_prop(c: &mut UnitCreds, prop: &str, json: &str) -> Result<(), &'static str> {
    let (_, sig) = CRED_PROPS.iter().find(|(p, _)| *p == prop).ok_or("unknown credential property")?;
    let v: serde_json::Value = serde_json::from_str(json.trim()).map_err(|_| "busctl answered something that isn't JSON")?;
    if v["type"].as_str() != Some(sig) {
        return Err("busctl answered an unexpected type for a credential property");
    }
    let data = v["data"].as_array().ok_or("busctl answered no data")?;
    for item in data {
        match *sig {
            "a(ss)" => {
                let pair = item.as_array().filter(|a| a.len() == 2).ok_or("bad credential entry")?;
                let id = pair[0].as_str().ok_or("bad credential id")?.to_string();
                let path = pair[1].as_str().ok_or("bad credential path")?.to_string();
                match prop {
                    "LoadCredential" => c.load.push((id, path)),
                    "ImportCredentialEx" => c.import_renames.push((id, path)),
                    _ => c.load_encrypted.push((id, path)),
                }
            }
            "a(say)" => {
                let pair = item.as_array().filter(|a| a.len() == 2).ok_or("bad credential entry")?;
                c.set_ids.push(pair[0].as_str().ok_or("bad credential id")?.to_string());
            }
            _ => c.import_globs.push(item.as_str().ok_or("bad import glob")?.to_string()),
        }
    }
    Ok(())
}

/// Does the unit already get a credential called `id` from somewhere other
/// than `ours` (K2's absolute path)? Any `LoadCredential=` or
/// `SetCredential*=` with that ID, a `LoadCredentialEncrypted=` with that
/// ID and another path, or a matching `ImportCredential=` glob.
pub fn credential_conflict(c: &UnitCreds, id: &str, ours: &str) -> Option<String> {
    if c.load.iter().any(|(i, _)| i == id) {
        return Some(format!("LoadCredential={id}"));
    }
    if let Some((_, p)) = c.load_encrypted.iter().find(|(i, p)| i == id && p != ours) {
        return Some(format!("LoadCredentialEncrypted={id}:{p}"));
    }
    if c.set_ids.iter().any(|i| i == id) {
        return Some(format!("SetCredential={id}"));
    }
    if let Some(g) = c.import_globs.iter().find(|g| glob_match(g, id)) {
        return Some(format!("ImportCredential={g}"));
    }
    // A renaming import: credentials matching GLOB arrive with the glob's
    // literal prefix replaced by RENAME, so `foo.*:tls.` gives `tls.key`.
    for (g, rename) in &c.import_renames {
        if rename.is_empty() {
            if glob_match(g, id) {
                return Some(format!("ImportCredential={g}"));
            }
            continue;
        }
        let lit: String = g.chars().take_while(|ch| !matches!(ch, '*' | '?' | '[')).collect();
        if let Some(rest) = id.strip_prefix(rename.as_str()) {
            if glob_match(g, &format!("{lit}{rest}")) {
                return Some(format!("ImportCredential={g}:{rename}"));
            }
        }
    }
    None
}

/// After K2's drop-in: systemd loads exactly K2's path for `id`, and
/// nothing else gives that ID (a positive match; fails closed).
pub fn credential_is_ours(c: &UnitCreds, id: &str, ours: &str) -> bool {
    credential_conflict(c, id, ours).is_none() && c.load_encrypted.iter().any(|(i, p)| i == id && p == ours)
}

/// `*` and `?` globs (ImportCredential= patterns).
fn glob_match(pat: &str, s: &str) -> bool {
    fn go(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => go(&p[1..], s) || (!s.is_empty() && go(p, &s[1..])),
            (Some(b'?'), Some(_)) => go(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &s[1..]),
            _ => false,
        }
    }
    go(pat.as_bytes(), s.as_bytes())
}

/// systemd's object-path escaping of a unit name (`k2-watch.service` →
/// `k2_2dwatch_2eservice`): tests and the live check.
pub fn bus_escape(unit: &str) -> String {
    unit.bytes()
        .map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("_{b:02x}") })
        .collect()
}

/// `/etc/k2/credstore.encrypted/<cred>.cred` (`cred` validated).
pub fn cred_path(cred: &str) -> String {
    format!("{CREDSTORE_DIR}/{cred}{CRED_SUFFIX}")
}

/// `k2-cred-<cred>.conf` (`cred` validated).
pub fn dropin_file(cred: &str) -> String {
    format!("{DROPIN_PREFIX}{cred}{DROPIN_SUFFIX}")
}

/// The canned drop-in. Only the validated cred name varies: no `Exec*`,
/// `Environment` or `User` line can come from a caller, and the name's
/// alphabet has no `%`, whitespace or newline. The credential is named by
/// absolute path (K2's own store), never looked up by bare name.
pub fn cred_dropin(cred: &str) -> Option<String> {
    cred_name_ok(cred).then(|| {
        format!(
            "# Written by K2 (k2 vault systemd-cred). K2 rewrites or removes this file; don't edit it.\n\
             [Service]\n\
             LoadCredentialEncrypted={cred}:{}\n",
            cred_path(cred)
        )
    })
}

/// `systemctl --version` first line → the major (`systemd 255 (255.4-1)`).
pub fn parse_systemd_version(out: &str) -> Option<u32> {
    let first = out.lines().next()?.trim();
    let n = first.strip_prefix("systemd ")?.split(|c: char| !c.is_ascii_digit()).next()?;
    n.parse().ok()
}

/// The multi-variable env-file wrapper (docs and the live test): seal the
/// whole env file as one credential and source it from `%d` (systemd's
/// credentials directory). Values in the file must be single-quoted. The
/// file is shell code run by the unit: only people may change it (a new
/// version is never re-placed without a person's confirm).
pub fn env_wrapper_dropin(cred: &str, exec: &str) -> Option<String> {
    if !cred_name_ok(cred) || !exec.starts_with('/') || exec.bytes().any(|b| b.is_ascii_whitespace() || b == b'\'' || b == b'%') {
        return None;
    }
    Some(format!(
        "[Service]\n\
         LoadCredentialEncrypted={cred}:{}\n\
         ExecStart=\n\
         ExecStart=/bin/sh -c 'set -a; . \"%d/{cred}\"; set +a; exec {exec}'\n",
        cred_path(cred)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cred_names() {
        for ok in ["ingest_key", "k2c.env", "A-b_9", "x", "INGEST_KEY", "k2.ops.ingest", &"a".repeat(64)] {
            assert!(cred_name_ok(ok), "{ok}");
        }
        for bad in ["", ".", "..", ".hidden", "-x", "a/b", "a b", "a\nb", "a%b", "a:b", "ä", &"a".repeat(65), "a\0b"] {
            assert!(!cred_name_ok(bad), "{bad:?}");
        }
    }

    /// Review B-H2 / A-L1: systemd's reserved credential namespaces.
    #[test]
    fn reserved_credential_namespaces_are_refused() {
        for bad in [
            "tmpfiles.extra",
            "sysusers.extra",
            "sysctl.extra",
            "login.motd",
            "agetty.autologin",
            "getty.ttys.serial",
            "ssh.authorized_keys.root",
            "passwd.hashed-password.root",
            "firstboot.locale",
            "network.network.50-k2",
            "system.hostname",
            "systemd.extra-unit.x.service",
            "vmm.notify_socket",
            "journal.forward_to_socket",
            "TMPFILES.extra",
            "Systemd.unit-dropin.x",
        ] {
            assert!(!cred_name_ok(bad), "{bad}");
            assert!(reserved_prefix(bad).is_some(), "{bad}");
        }
        // Words that only start like a namespace, without its dot, are fine.
        for ok in ["systemdx", "login_key", "network-key", "sshkey"] {
            assert!(cred_name_ok(ok), "{ok}");
        }
    }

    /// Review A-H2 / B-M1: no templates, instances, prefix names.
    #[test]
    fn unit_names() {
        for ok in ["k2-watch.service", "a.b.service", "x.service", "NetworkThing.service", &format!("{}.service", "a".repeat(120))] {
            assert!(unit_name_ok(ok), "{ok}");
        }
        for bad in [
            "",
            ".service",
            "-x.service",
            ".x.service",
            "@x.service",
            "foo@.service",
            "foo@bar.service",
            "k2-.service",
            "sew-.service",
            "a..service",
            "x.socket",
            "x",
            "a/b.service",
            "../x.service",
            "a b.service",
            "a\n.service",
            "a%i.service",
            &format!("{}.service", "a".repeat(121)),
        ] {
            assert!(!unit_name_ok(bad), "{bad:?}");
        }
    }

    /// PRD rev 2: the real targets start with `k2-` and pass the deny list;
    /// every K2 unit and family is refused; so is a daemon.toml unit.
    #[test]
    fn deny_list_is_exact_not_a_prefix() {
        for ok in ["k2-watch.service", "k2-ops-push.service", "k2-control-plane.service", "nginx.service"] {
            assert_eq!(deny_reason(ok, &[]), None, "{ok}");
        }
        for u in K2_OWN_UNITS.iter().chain(SYSTEM_UNITS) {
            assert!(deny_reason(u, &[]).is_some(), "{u}");
        }
        for u in [
            "k2app-x--d-1.service",
            "k2app-shop--d-0123456789ab.service",
            "k2job-j1-g1.service",
            "k2test-anything.service",
            "systemd-journald.service",
            "dbus.service",
            "dbus-broker.service",
            "cloud-init.service",
            // `@` names are refused by the name rule before any list.
            "k2app-process@jobs.service",
            "postgresql@16-main.service",
            "getty@tty1.service",
            "wg-quick@wg0.service",
        ] {
            assert!(deny_reason(u, &[]).is_some(), "{u}");
        }
        assert_eq!(deny_reason("k2app-notes.service", &[]), None, "k2app-<name> without -- is not a deployment unit");
        let daemons = vec!["ops-daemon-two.service".to_string()];
        assert_eq!(deny_reason("ops-daemon-two.service", &daemons), Some("a K2 daemon unit (daemon.toml)"));
        assert_eq!(deny_reason("ops-daemon-two.service", &[]), None);
        assert!(deny_reason("k2-daemon-nsi.service", &[]).is_some(), "a hosted box's daemon unit");
    }

    /// Review A-H2 / B-M1: fail closed. No allow list, or a unit off it, is
    /// refused even when the deny list passes it.
    #[test]
    fn allow_list_is_the_wall() {
        let allow = parse_allow_list("# ops units\nk2-watch.service\n\n  k2-control-plane.service  # control plane\nk2-watch.service\n").unwrap();
        assert_eq!(allow, vec!["k2-watch.service", "k2-control-plane.service"]);
        assert_eq!(unit_refusal("k2-watch.service", &allow, &[]), None);
        assert!(unit_refusal("k2-ops-push.service", &allow, &[]).unwrap().contains("allow list"));
        assert!(unit_refusal("k2-watch.service", &[], &[]).is_some(), "no list = nothing allowed");
        // The deny list still wins over an admin's line.
        let allow2 = parse_allow_list("sshd.service\nk2-daemon.service\n").unwrap();
        assert!(unit_refusal("sshd.service", &allow2, &[]).is_some());
        assert!(unit_refusal("k2-daemon.service", &allow2, &[]).is_some());
        for bad in ["foo@.service\n", "k2-.service\n", "x\n", "a b.service\n", "/etc/x.service\n"] {
            assert!(parse_allow_list(bad).is_err(), "{bad:?}");
        }
        assert_eq!(parse_allow_list("").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn unit_show_parses() {
        let s = parse_unit_show("LoadState=loaded\nId=k2-watch.service\nNames=k2-watch.service watch.service\n");
        assert_eq!(s.load_state, "loaded");
        assert_eq!(s.id, "k2-watch.service");
        assert_eq!(s.names, vec!["k2-watch.service", "watch.service"]);
        assert_eq!(parse_unit_show("LoadState=not-found\nId=x.service\nNames=x.service\n").load_state, "not-found");
    }

    /// Review A-M1, as systemd really answers (A2-N1): `busctl --json=short
    /// get-property` JSON, never `systemctl show` text.
    #[test]
    fn credential_conflicts_from_real_busctl_json() {
        let ours = cred_path("tls.key");
        let creds = |props: &[(&str, &str)]| {
            let mut c = UnitCreds::default();
            for (p, j) in props {
                apply_prop(&mut c, p, j).unwrap();
            }
            c
        };
        let empty = creds(&[
            ("LoadCredential", r#"{"type":"a(ss)","data":[]}"#),
            ("LoadCredentialEncrypted", r#"{"type":"a(ss)","data":[]}"#),
            ("SetCredential", r#"{"type":"a(say)","data":[]}"#),
            ("SetCredentialEncrypted", r#"{"type":"a(say)","data":[]}"#),
            ("ImportCredential", r#"{"type":"as","data":[]}"#),
        ]);
        assert_eq!(credential_conflict(&empty, "tls.key", &ours), None);
        assert!(!credential_is_ours(&empty, "tls.key", &ours), "nothing loaded: not ours");
        let plain = creds(&[("LoadCredential", r#"{"type":"a(ss)","data":[["tls.key","/etc/ssl/private/svc.key"]]}"#)]);
        assert!(credential_conflict(&plain, "tls.key", &ours).is_some());
        let bare = creds(&[("LoadCredentialEncrypted", r#"{"type":"a(ss)","data":[["tls.key","tls.key"]]}"#)]);
        assert!(credential_conflict(&bare, "tls.key", &ours).is_some(), "a bare id from systemd's credstore");
        let set = creds(&[("SetCredential", r#"{"type":"a(say)","data":[["tls.key",[97,98,99]]]}"#)]);
        assert!(credential_conflict(&set, "tls.key", &ours).is_some());
        for g in ["tls.*", "*"] {
            let imp = creds(&[("ImportCredential", &format!(r#"{{"type":"as","data":["{g}"]}}"#))]);
            assert!(credential_conflict(&imp, "tls.key", &ours).is_some(), "{g}");
        }
        let other = creds(&[
            ("ImportCredential", r#"{"type":"as","data":["db.*"]}"#),
            ("LoadCredential", r#"{"type":"a(ss)","data":[["other","/x"]]}"#),
        ]);
        assert_eq!(credential_conflict(&other, "tls.key", &ours), None);
        let mine = creds(&[("LoadCredentialEncrypted", &format!(r#"{{"type":"a(ss)","data":[["tls.key","{ours}"]]}}"#))]);
        assert_eq!(credential_conflict(&mine, "tls.key", &ours), None, "our own line (a re-bind)");
        assert!(credential_is_ours(&mine, "tls.key", &ours));
        let theirs = creds(&[("LoadCredentialEncrypted", r#"{"type":"a(ss)","data":[["tls.key","/etc/other.cred"]]}"#)]);
        assert!(!credential_is_ours(&theirs, "tls.key", &ours), "an admin drop-in after ours wins");
        // Wrong types and shapes fail closed.
        let mut c = UnitCreds::default();
        for (p, j) in [
            ("LoadCredential", r#"{"type":"as","data":["x"]}"#),
            ("LoadCredential", r#"{"type":"a(ss)","data":[["only-id"]]}"#),
            ("LoadCredentialEncrypted", "[unprintable]"),
            ("Nope", r#"{"type":"as","data":[]}"#),
        ] {
            assert!(apply_prop(&mut c, p, j).is_err(), "{p} {j}");
        }
        assert!(glob_match("a*c", "abbc") && glob_match("a?c", "abc") && !glob_match("a?c", "ac"));
        // systemd 257 renaming imports (A3-P2).
        let ren = creds(&[("ImportCredentialEx", r#"{"type":"a(ss)","data":[["foo.*","tls."]]}"#)]);
        assert!(credential_conflict(&ren, "tls.key", &ours).is_some(), "foo.key arrives as tls.key");
        assert_eq!(credential_conflict(&ren, "db.key", &ours), None);
        let plain_ex = creds(&[("ImportCredentialEx", r#"{"type":"a(ss)","data":[["tls.*",""]]}"#)]);
        assert!(credential_conflict(&plain_ex, "tls.key", &ours).is_some());
    }

    #[test]
    fn unit_object_paths() {
        assert_eq!(bus_escape("k2-watch.service"), "k2_2dwatch_2eservice");
        let j = r#"{"type":"o","data":["/org/freedesktop/systemd1/unit/k2_2dwatch_2eservice"]}"#;
        assert_eq!(parse_unit_path(j).as_deref(), Some("/org/freedesktop/systemd1/unit/k2_2dwatch_2eservice"));
        for bad in [
            r#"{"type":"s","data":["/org/freedesktop/systemd1/unit/x"]}"#,
            r#"{"type":"o","data":["/etc/passwd"]}"#,
            r#"{"type":"o","data":["/org/freedesktop/systemd1/unit/a/../b"]}"#,
            "LoadState=loaded",
        ] {
            assert_eq!(parse_unit_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn dropin_bytes_are_canned_and_name_k2s_own_store() {
        assert_eq!(
            cred_dropin("ingest_key").unwrap(),
            "# Written by K2 (k2 vault systemd-cred). K2 rewrites or removes this file; don't edit it.\n[Service]\nLoadCredentialEncrypted=ingest_key:/etc/k2/credstore.encrypted/ingest_key.cred\n"
        );
        assert!(cred_dropin("a\nExecStart=/bin/sh").is_none());
        assert!(cred_dropin("a b").is_none());
        assert!(cred_dropin("tmpfiles.extra").is_none());
        assert_eq!(dropin_file("k2c.env"), "k2-cred-k2c.env.conf");
        assert_eq!(cred_path("k2c.env"), "/etc/k2/credstore.encrypted/k2c.env.cred");
        assert!(!CREDSTORE_DIR.starts_with("/etc/credstore"), "never systemd's own search path");
    }

    #[test]
    fn systemd_version_parses() {
        assert_eq!(parse_systemd_version("systemd 255 (255.4-1ubuntu8.4)\n+PAM +AUDIT"), Some(255));
        assert_eq!(parse_systemd_version("systemd 249\n"), Some(249));
        assert_eq!(parse_systemd_version("garbage"), None);
        assert_eq!(parse_systemd_version(""), None);
    }

    #[test]
    fn env_wrapper_uses_percent_d() {
        let d = env_wrapper_dropin("k2c.env", "/usr/local/bin/k2-control-plane").unwrap();
        assert_eq!(
            d,
            "[Service]\nLoadCredentialEncrypted=k2c.env:/etc/k2/credstore.encrypted/k2c.env.cred\nExecStart=\nExecStart=/bin/sh -c 'set -a; . \"%d/k2c.env\"; set +a; exec /usr/local/bin/k2-control-plane'\n"
        );
        assert!(env_wrapper_dropin("k2c.env", "relative").is_none());
        assert!(env_wrapper_dropin("k2c.env", "/bin/x y").is_none());
    }

    /// Names a repo scan finds that look like K2's but are not units K2
    /// installs: ops units on boxes (allowed targets when an admin lists
    /// them), and test or validator names written before the `k2test-`
    /// fixture family. New fixtures use `k2test-` (always denied).
    const NOT_K2_UNITS: &[(&str, &str)] = &[
        ("k2-watch.service", "ops unit on boxes; an allowed target (PRD rev 2)"),
        ("k2-ops-push.service", "ops unit on boxes; an allowed target"),
        ("k2-control-plane.service", "K2 Connect control plane; an allowed target"),
        ("k2a-daemon.service", "test fixture: a daemon.toml unit"),
        ("k2b-daemon.service", "test fixture: a daemon.toml unit"),
        ("k2-user.service", "test fixture: a user-scope daemon.toml unit"),
        ("k2-gone.service", "test fixture: a removed daemon"),
        ("k2-front-443.service", "doc: the service systemd would look for, never written"),
        ("k2-sew.service", "test fixture: not a Sew unit"),
        ("k2-sew-test-no-such-unit.service", "test fixture: a missing unit"),
        ("k2app-notes.service", "test fixture: not a deployment unit"),
        ("sewing.service", "validator case: not a Sew unit"),
        ("sewer.service", "validator case: not a Sew unit"),
        ("sew-Db.service", "validator case"),
    ];

    /// Names refused by the name rule that the scan sees in validator tests
    /// and docs (templates, prefix names): denied anyway.
    fn denied_or_known(n: &str) -> bool {
        deny_reason(n, &[]).is_some() || NOT_K2_UNITS.iter().any(|(u, _)| *u == n)
    }

    fn k2_looking(stem: &str) -> bool {
        ["k2", "sew", "stalwart", "postgresql"].iter().any(|p| stem.starts_with(p))
    }

    /// Every `<name>.service` in `text`: plain names, and templated names
    /// (`format!("k2job-{id}.service")`) as their literal head before the
    /// first placeholder, marked `true`.
    fn service_names(text: &str) -> Vec<(String, bool)> {
        let b = text.as_bytes();
        let unit_char = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'@' | b'-');
        let mut out = vec![];
        let mut i = 0;
        while let Some(off) = text[i..].find(".service") {
            let end = i + off;
            let after = end + ".service".len();
            i = after;
            if b.get(after).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-') {
                continue;
            }
            let mut start = end;
            let mut templated = false;
            while start > 0 {
                let c = b[start - 1];
                if unit_char(c) {
                    start -= 1;
                } else if matches!(c, b'{' | b'}' | b'$' | b'%') || (templated && c.is_ascii_alphanumeric()) {
                    templated = true;
                    start -= 1;
                } else {
                    break;
                }
            }
            if start == end {
                continue;
            }
            let full = &text[start..after];
            if templated {
                // The literal head before the first placeholder.
                let head: String = full.chars().take_while(|c| !matches!(c, '{' | '$' | '%')).collect();
                out.push((head, true));
            } else {
                out.push((full.to_string(), false));
            }
        }
        out
    }

    /// Review blocker (PRD rev 2 item 4, security #8), widened after review
    /// B-L5: scan the repo for the `*.service` names K2 writes (templates,
    /// installers, daemon.toml fixtures, tests, docs, Quadlet files) and
    /// fail on any K2-looking name that is neither on the deny list nor
    /// named in [`NOT_K2_UNITS`]. Templated names must start with a K2
    /// family prefix. The allow list is the wall; this keeps the belt
    /// current.
    #[test]
    fn deny_list_covers_every_k2_unit_in_the_repo() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut stack: Vec<std::path::PathBuf> = ["crates", "scripts", "cli", "src-tauri", "tests", "docs", "packaging", "src"]
            .iter()
            .map(|d| repo.join(d))
            .filter(|p| p.is_dir())
            .collect();
        assert!(stack.len() >= 4, "repo layout changed: {stack:?}");
        let mut files = 0;
        let mut seen = std::collections::BTreeSet::new();
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).expect("read dir") {
                let e = e.expect("entry");
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                let ft = e.file_type().expect("file type");
                if ft.is_symlink() {
                    continue;
                }
                if ft.is_dir() {
                    if !name.starts_with('.') && name != "target" && name != "node_modules" && name != "gen" {
                        stack.push(p);
                    }
                    continue;
                }
                let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
                let scanned = matches!(
                    ext,
                    "rs" | "sh" | "toml" | "service" | "socket" | "timer" | "conf" | "in" | "py" | "txt" | "tmpl" | "md" | "json"
                        | "yaml" | "yml" | "container" | "ts" | "tsx"
                ) || name == "k2";
                if !scanned {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&p) else { continue };
                files += 1;
                for n in service_names(&text) {
                    seen.insert(n);
                }
            }
        }
        assert!(files > 500, "scanned only {files} files");
        for must in ["k2-daemon.service", "stalwart.service", "sew.service", "k2-node.service"] {
            assert!(seen.contains(&(must.to_string(), false)), "the scan no longer finds {must}: the scanner is broken");
        }
        let mut missing = vec![];
        for (n, templated) in &seen {
            if *templated {
                // `k2job-{}.service` → head `k2job-`: must be a family.
                if k2_looking(n)
                    && !K2_UNIT_FAMILIES.iter().any(|(p, _)| n.starts_with(p) || p.starts_with(n.as_str()))
                    && !["k2app-", "k2-", "sew-", "k2"].contains(&n.as_str())
                {
                    missing.push(format!("{n}{{…}}.service"));
                }
                continue;
            }
            let stem = n.strip_suffix(".service").unwrap_or(n);
            if !k2_looking(stem) || denied_or_known(n) {
                continue;
            }
            missing.push(n.clone());
        }
        assert!(
            missing.is_empty(),
            "K2-looking unit names that are neither on the creds deny list (k2_root_proto::creds) \
             nor in NOT_K2_UNITS with a reason: {missing:?}"
        );
        for (u, _) in NOT_K2_UNITS {
            assert_eq!(deny_reason(u, &[]), None, "{u} is in NOT_K2_UNITS but denied");
        }
    }
}
