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

/// The one backend each kind may have in production (the daemon builds
/// its own table only with these; a handed-over table is filtered to them).
pub fn fixed_backend(kind: Kind) -> Backend {
    let lo = |port| SocketAddr::from(([127, 0, 0, 1], port));
    match kind {
        Kind::Turn => Backend { kind, addr: lo(5349), proxy: true, refuse_loopback_clients: false },
        Kind::Stalwart => Backend { kind, addr: lo(8443), proxy: true, refuse_loopback_clients: true },
        // PROXY only when that Caddy reads it: the one free field.
        Kind::Caddy => Backend { kind, addr: lo(k2_core::front443::CADDY_FRONT_PORT), proxy: false, refuse_loopback_clients: false },
    }
}

impl RouteTable {
    /// The entries whose backend is exactly its kind's fixed backend
    /// ([`fixed_backend`]; a Caddy route may carry PROXY or not). Applied to
    /// the table a hand-off predecessor passes, never trusting more than
    /// the daemon itself would build. No file is ever read: a same-uid
    /// process (an agent) could plant one.
    pub fn fixed_backends_only(&self) -> RouteTable {
        let mut t = RouteTable::default();
        for (h, b) in &self.hosts {
            let f = fixed_backend(b.kind);
            let same = b.addr == f.addr
                && b.refuse_loopback_clients == f.refuse_loopback_clients
                && (b.kind == Kind::Caddy || b.proxy == f.proxy);
            if same {
                let _ = t.insert(h, b.clone());
            }
        }
        t
    }
}

/// The table a hand-off predecessor passed (taken once by `front443::start`).
fn handed() -> &'static std::sync::Mutex<Option<RouteTable>> {
    static H: OnceLock<std::sync::Mutex<Option<RouteTable>>> = OnceLock::new();
    H.get_or_init(|| std::sync::Mutex::new(None))
}

/// Successor: keep the predecessor's table (fixed backends only).
pub fn set_handed(t: RouteTable) {
    *handed().lock().unwrap_or_else(|p| p.into_inner()) = Some(t.fixed_backends_only());
}

pub fn take_handed() -> Option<RouteTable> {
    handed().lock().unwrap_or_else(|p| p.into_inner()).take()
}
