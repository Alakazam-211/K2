//! The front's route table: exact server name → loopback backend. Built
//! by the daemon from its own state (voice, Skin Direct, hosted mail; see
//! `front443::rebuild`) and swapped in atomically, so a connection always
//! routes against one whole table. No wildcard, no default route: an
//! unknown name is closed.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// sew-media's `passthrough` TURN listener (127.0.0.1:5349).
    Turn,
    /// Skin Direct's Caddy on its loopback HTTPS port.
    Caddy,
    /// Stalwart's HTTPS listener on 127.0.0.1:8443 (opt-in).
    Stalwart,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Turn => "turn",
            Kind::Caddy => "caddy",
            Kind::Stalwart => "stalwart",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backend {
    pub kind: Kind,
    /// Always a loopback address.
    pub addr: SocketAddr,
    /// Write a PROXY v2 header first.
    pub proxy: bool,
    /// Close clients whose own address is loopback (Stalwart: a ban of the
    /// loopback address would lock K2 out of Stalwart's management
    /// listener, so loopback never authenticates through the front).
    pub refuse_loopback_clients: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteTable {
    hosts: BTreeMap<String, Backend>,
}

impl RouteTable {
    /// Add `host` (lowercased; must be a valid SNI host name). Refuses a
    /// non-loopback backend and a host already routed elsewhere (the first
    /// route wins and the refusal says so).
    pub fn insert(&mut self, host: &str, b: Backend) -> Result<(), String> {
        let h = super::client_hello::host_name(host.trim().as_bytes())
            .ok_or_else(|| format!("not a host name: {host:?}"))?;
        if !b.addr.ip().is_loopback() {
            return Err(format!("{h}: backend {} is not loopback", b.addr));
        }
        if let Some(cur) = self.hosts.get(&h) {
            if *cur != b {
                return Err(format!("{h} already routes to {} ({})", cur.addr, cur.kind.as_str()));
            }
            return Ok(());
        }
        self.hosts.insert(h, b);
        Ok(())
    }

    pub fn lookup(&self, sni: &str) -> Option<&Backend> {
        self.hosts.get(sni)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&String, &Backend)> {
        self.hosts.iter()
    }

    pub fn len(&self) -> usize {
        self.hosts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.hosts
                .iter()
                .map(|(h, b)| {
                    serde_json::json!({
                        "host": h,
                        "kind": b.kind,
                        "backend": b.addr.to_string(),
                        "proxyV2": b.proxy,
                        "refuseLoopbackClients": b.refuse_loopback_clients,
                    })
                })
                .collect(),
        )
    }
}

fn table() -> &'static ArcSwap<RouteTable> {
    static T: OnceLock<ArcSwap<RouteTable>> = OnceLock::new();
    T.get_or_init(|| ArcSwap::from_pointee(RouteTable::default()))
}

/// The live table (one load per connection).
pub fn current() -> Arc<RouteTable> {
    table().load_full()
}

/// Swap in a whole new table.
pub fn install(t: RouteTable) {
    table().store(Arc::new(t));
}

/// `~/.k2/front443-routes.json`: the last table built, so a daemon that
/// boots (or takes over in a hand-off) routes from its first connection,
/// before the DB and the rest of its state are up.
pub fn persisted_path() -> std::path::PathBuf {
    k2_core::paths::k2_home().join("front443-routes.json")
}

pub fn save_persisted(t: &RouteTable) -> Result<(), String> {
    let text = serde_json::to_string(t).map_err(|e| e.to_string())?;
    k2_core::fs_atomic::atomic_write_private(&persisted_path(), text.as_bytes()).map_err(|e| e.to_string())
}

/// The persisted table, re-validated entry by entry (a hand-edited file
/// can't point the front anywhere but loopback).
pub fn load_persisted() -> Option<RouteTable> {
    parse_persisted(&std::fs::read_to_string(persisted_path()).ok()?)
}

/// [`load_persisted`]'s parse + re-validation (pure).
pub fn parse_persisted(text: &str) -> Option<RouteTable> {
    let raw: RouteTable = serde_json::from_str(text).ok()?;
    let mut t = RouteTable::default();
    for (h, b) in raw.hosts {
        let _ = t.insert(&h, b);
    }
    Some(t)
}
