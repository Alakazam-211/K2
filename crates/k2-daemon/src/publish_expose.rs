//! Where a `--cmd` service may listen (0.46.18).
//!
//! A published service is reached through the server's own front door
//! (:443, with K2's check and any App sign-in). A program that listens on
//! every address (`*:PORT`) is also reachable on its raw port, around all of
//! that. Before 0.46.18 an old `--cmd` service got no `HOST` (and, kept on the
//! inherited environment, no `PORT`), and nothing looked at where it listened
//! unless its site was private or behind a PIN.
//!
//! - **Its environment:** every `--cmd` service gets `HOST=127.0.0.1` and
//!   `PORT=<its port>` at start, plus the few spellings servers read for
//!   the same thing ([`loopback_env`]), unless the owner marked it public.
//!   A value the service's own `.env.k2` sets explicitly wins; one it would
//!   only inherit from the daemon doesn't.
//! - **The check:** `publish_runtime`'s bind check (after each start, then
//!   every 5 minutes) warns in `k2 publish ps`, the service's log and `k2
//!   vault doctor` when it listens beyond loopback. It never stops it.
//! - **`k2 publish expose <name> public`:** the owner says a service listens
//!   beyond the server on purpose. K2 records who and when, shows it, stops
//!   warning, and stops setting the loopback variables at its next start.
//!   `server` undoes it. An agent can't change it.
//!
//! Kept in `~/.k2/publish-expose.json` (0600; service ids, who, when), so
//! it needs no main-DB migration, like `publish-env.json`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// The value that marks a service public.
pub const PUBLIC: &str = "public";
/// The default: it listens on this server only.
pub const SERVER: &str = "server";

/// Who marked a service public, and when (unix seconds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Public {
    pub set_by: String,
    pub set_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    public: BTreeMap<String, Public>,
}

static LOCK: Mutex<()> = Mutex::new(());

fn path() -> PathBuf {
    k2_core::paths::k2_home().join("publish-expose.json")
}

fn load() -> File {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(f: &File) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(f).map_err(|e| e.to_string())?;
    k2_vault::fsutil::write_private_atomic(&path(), &body).map_err(|e| e.hint)
}

/// `Some` when the owner marked this service public.
pub fn public(service_id: &str) -> Option<Public> {
    load().public.get(service_id).cloned()
}

/// Mark a service public (`by` names who, for the record), or back to this
/// server only. Returns what is recorded now.
pub fn set(service_id: &str, public: bool, by: &str) -> Result<Option<Public>, String> {
    let _g = LOCK.lock();
    let mut f = load();
    if public {
        let e = f.public.entry(service_id.to_string()).or_insert_with(|| Public { set_by: by.to_string(), set_at: chrono::Utc::now().timestamp() });
        let out = e.clone();
        save(&f)?;
        Ok(Some(out))
    } else {
        if f.public.remove(service_id).is_some() {
            save(&f)?;
        }
        Ok(None)
    }
}

/// What a `--cmd` service gets so it listens on this server only: the
/// conventional `HOST` and `PORT` (Node servers, Create React App, Astro,
/// SvelteKit, Nuxt/Nitro, Remix; `PORT` also Next.js, Express, Puma,
/// Gunicorn), Rails' `BINDING`, and Flask's and Uvicorn's own names, which
/// their CLIs read and an explicit flag still overrides.
///
/// Never `HOSTNAME`: shells and many programs read it as the machine's own
/// name. Vite, Django's `runserver` and Python's `http.server` read none of
/// these; the bind check catches what they do.
pub fn loopback_env(port: u16) -> Vec<(&'static str, String)> {
    let host = "127.0.0.1".to_string();
    let port = port.to_string();
    vec![
        ("HOST", host.clone()),
        ("PORT", port.clone()),
        ("BINDING", host.clone()),
        ("FLASK_RUN_HOST", host.clone()),
        ("FLASK_RUN_PORT", port.clone()),
        ("UVICORN_HOST", host),
        ("UVICORN_PORT", port),
    ]
}

/// Set [`loopback_env`] on a `--cmd` service's command, after its own
/// environment is prepared: a name the service's `.env.k2` set explicitly
/// keeps its value; a name it would inherit from the daemon is replaced.
/// Nothing at all for a service the owner marked public.
pub fn apply(cmd: &mut Command, service_id: &str, port: u16) {
    if public(service_id).is_some() {
        return;
    }
    let set: Vec<String> = cmd.get_envs().filter(|(_, v)| v.is_some()).map(|(k, _)| k.to_string_lossy().into_owned()).collect();
    for (k, v) in loopback_env(port) {
        if !set.iter().any(|s| s == k) {
            cmd.env(k, v);
        }
    }
}

/// The warning for a `--cmd` service that listens beyond this server, with
/// the exact fix.
pub fn warning(name: &str, port: i64, addrs: &[String]) -> String {
    format!(
        "{name} listens on {}, beyond this server: anyone who can reach this machine on port {port} gets it without the server's front door (:443) or any sign-in. \
         Fix: make it listen on 127.0.0.1 ({product} gives it HOST=127.0.0.1 and PORT={port}; a server that ignores them needs its own flag, such as --host 127.0.0.1), \
         then restart it: {cli} publish stop {name} && {cli} publish start {name}. \
         If it is public on purpose, the owner runs: {cli} publish expose {name} public",
        addrs.join(", "),
        cli = k2_core::brand::CLI_NAME,
        product = k2_core::brand::PRODUCT_NAME,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loopback_env_names_host_and_port_and_never_hostname() {
        let e = loopback_env(4321);
        let get = |k: &str| e.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("HOST"), Some("127.0.0.1"));
        assert_eq!(get("PORT"), Some("4321"));
        assert_eq!(get("BINDING"), Some("127.0.0.1"));
        assert_eq!(get("FLASK_RUN_PORT"), Some("4321"));
        assert_eq!(get("UVICORN_HOST"), Some("127.0.0.1"));
        assert!(get("HOSTNAME").is_none(), "HOSTNAME is the machine's name to most programs");
        for (_, v) in &e {
            assert!(v == "127.0.0.1" || v == "4321", "{e:?}");
        }
    }

    #[test]
    fn an_explicit_value_wins_an_inherited_one_does_not_and_public_sets_nothing() {
        crate::test_support::with_temp_home(|| {
            let mut c = Command::new("true");
            c.env("HOST", "0.0.0.0");
            apply(&mut c, "svc-a", 5000);
            let envs: BTreeMap<String, String> =
                c.get_envs().filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned()))).collect();
            assert_eq!(envs.get("HOST").map(String::as_str), Some("0.0.0.0"), "the service's own .env.k2 value stays");
            assert_eq!(envs.get("PORT").map(String::as_str), Some("5000"));
            assert_eq!(envs.get("BINDING").map(String::as_str), Some("127.0.0.1"));

            let rec = set("svc-b", true, "owner").expect("set").expect("recorded");
            assert_eq!(rec.set_by, "owner");
            assert_eq!(public("svc-b"), Some(rec.clone()));
            // Marking it again keeps the first record.
            assert_eq!(set("svc-b", true, "someone").expect("set"), Some(rec));
            let mut c = Command::new("true");
            apply(&mut c, "svc-b", 5001);
            assert_eq!(c.get_envs().count(), 0, "public: K2 sets nothing");
            assert_eq!(set("svc-b", false, "owner").expect("unset"), None);
            assert!(public("svc-b").is_none());
        });
    }

    #[test]
    fn the_warning_names_the_address_the_fix_and_the_owners_way_out() {
        let w = warning("shop", 3000, &["0.0.0.0:3000".to_string()]);
        assert!(w.contains("shop listens on 0.0.0.0:3000, beyond this server"), "{w}");
        assert!(w.contains("HOST=127.0.0.1 and PORT=3000"), "{w}");
        assert!(w.contains(&format!("{} publish stop shop && {} publish start shop", k2_core::brand::CLI_NAME, k2_core::brand::CLI_NAME)), "{w}");
        assert!(w.contains(&format!("{} publish expose shop public", k2_core::brand::CLI_NAME)), "{w}");
    }
}
