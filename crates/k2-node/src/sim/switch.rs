//! `/etc/k2-node/sim.toml`: the server slots the node owner turned iPhones
//! on for (prd-ios-on-mac-node-0467-v1 §3.8, Q5). Root writes it
//! (`sudo k2-node sim enable --server <slot>`, or `setup --sim`); the node
//! only reads it. A slot that isn't listed gets no `sim-ios`, no
//! `Offer.sim`, and every `sim_open` from it is refused.
//!
//! Who can be turned on:
//! - only a server the node owner trusts as their own (`own`): a Simulator
//!   app runs outside the job wall and all of a node's sim sessions share
//!   one user, so another company's server can't be kept apart (§3.7);
//! - only a slot whose network is `on`: the Simulator has the Mac's full
//!   network, so a slot limited to `none` or `no-lan` can't be honoured.

use std::path::{Path, PathBuf};

use k2_node_proto::share::{Network, Slot, Trust, MAX_SLOTS, OWNER_SLOT};
use serde::{Deserialize, Serialize};

pub const FILE: &str = "sim.toml";

/// The licence line the owner sees when turning a slot on (§3.8).
pub const LICENCE_LINE: &str = "Turn this on only for servers of the company that owns this Mac, or that leases all of it for at least 24 hours (Apple's macOS licence).";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimSwitch {
    /// Server slots that may run iPhones here.
    #[serde(default)]
    pub slots: Vec<Slot>,
}

impl SimSwitch {
    pub fn on(&self, slot: Slot) -> bool {
        self.slots.contains(&slot)
    }
}

pub fn path(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE)
}

/// No file = nothing turned on. A file that doesn't parse is an error, and
/// callers treat it as nothing turned on (fail closed).
pub fn read(config_dir: &Path) -> Result<SimSwitch, String> {
    let p = path(config_dir);
    let text = match crate::util::read_regular(&p) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SimSwitch::default()),
        Err(e) => return Err(format!("read {}: {e}", p.display())),
    };
    let s: SimSwitch = toml::from_str(&text).map_err(|e| format!("{} doesn't parse: {e}", p.display()))?;
    if let Some(bad) = s.slots.iter().find(|x| **x >= MAX_SLOTS) {
        return Err(format!("{}: slot {bad} doesn't exist (0-{})", p.display(), MAX_SLOTS - 1));
    }
    Ok(s)
}

pub fn write(config_dir: &Path, s: &SimSwitch) -> Result<(), String> {
    let mut s = s.clone();
    s.slots.sort_unstable();
    s.slots.dedup();
    let body = toml::to_string(&s).map_err(|e| format!("encode {FILE}: {e}"))?;
    let text = format!("# Server slots whose iPhones run on this Mac (k2-node sim enable|disable --server <slot>).\n{body}");
    crate::util::atomic_write(&path(config_dir), text.as_bytes(), 0o644)
}

/// Why `slot` can't run iPhones here; `None` = it may.
pub fn refusal(macos: bool, trust: Trust, network: Network) -> Option<String> {
    if !macos {
        return Some("iPhones run only on a Mac node (the iOS Simulator needs macOS and Xcode)".into());
    }
    if trust == Trust::Team {
        return Some(format!(
            "this server isn't one of yours (--trust team): all iPhones on a Mac share one user, so another company's server can't be kept apart. {LICENCE_LINE}"
        ));
    }
    match network {
        Network::On => None,
        n => Some(format!(
            "this server's jobs here have network {} and the iPhone would have the Mac's whole network; give it network on first (sudo k2-node servers limits <server> --network on)",
            n.as_str()
        )),
    }
}

/// One server line as `sim enable` sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotLine {
    pub slot: Slot,
    pub label: String,
    pub trust: Trust,
    pub network: Network,
}

/// The server lines on this node: the owner (slot 0, `own`, the policy's
/// network) and every line of `servers.toml`.
pub fn lines(config_dir: &Path) -> Result<Vec<SlotLine>, String> {
    let policy = crate::config::read_policy_file(&config_dir.join("policy.toml"))?;
    let owner_net = Network::of_policy(policy.network.as_deref().unwrap_or("on"));
    let mut out = vec![SlotLine { slot: OWNER_SLOT, label: "the owner server".into(), trust: Trust::Own, network: owner_net }];
    if let Some(f) = crate::admin::servers::read_file(config_dir)? {
        for e in f.servers.iter().filter(|e| e.slot != OWNER_SLOT) {
            out.push(SlotLine { slot: e.slot, label: e.label.clone(), trust: e.trust, network: e.network });
        }
    }
    Ok(out)
}

/// `0`, `slot:3` or a server's label.
pub fn find<'a>(lines: &'a [SlotLine], spec: &str) -> Result<&'a SlotLine, String> {
    let s = spec.trim();
    let slot = s.strip_prefix("slot:").unwrap_or(s).parse::<Slot>().ok();
    lines
        .iter()
        .find(|l| Some(l.slot) == slot || l.label == s)
        .ok_or_else(|| format!("no server {s:?} on this node (see k2-node servers list)"))
}

/// `sim enable` / `sim disable`: returns the line to print.
pub fn set(config_dir: &Path, macos: bool, spec: &str, on: bool) -> Result<String, String> {
    let all = lines(config_dir)?;
    let line = find(&all, spec)?;
    let mut s = read(config_dir).unwrap_or_default();
    if on {
        if let Some(why) = refusal(macos, line.trust, line.network) {
            return Err(format!("can't run iPhones for {} (slot {}): {why}", line.label, line.slot));
        }
        if !s.slots.contains(&line.slot) {
            s.slots.push(line.slot);
        }
        write(config_dir, &s)?;
        Ok(format!("iPhones on for {} (slot {}). {LICENCE_LINE}", line.label, line.slot))
    } else {
        s.slots.retain(|x| *x != line.slot);
        write(config_dir, &s)?;
        Ok(format!("iPhones off for {} (slot {}): new ones are refused; one already running ends with its lease.", line.label, line.slot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_own_servers_with_full_network_on_a_mac() {
        assert!(refusal(true, Trust::Own, Network::On).is_none());
        let team = refusal(true, Trust::Team, Network::On).unwrap();
        assert!(team.contains("isn't one of yours") && team.contains("24 hours"), "{team}");
        assert!(refusal(true, Trust::Own, Network::NoLan).unwrap().contains("network no-lan"));
        assert!(refusal(true, Trust::Own, Network::None).unwrap().contains("network none"));
        assert!(refusal(false, Trust::Own, Network::On).unwrap().contains("only on a Mac"));
    }

    #[test]
    fn the_switch_round_trips_and_a_missing_file_is_off() {
        let d = crate::util::temp_dir("simswitch");
        assert_eq!(read(&d).unwrap(), SimSwitch::default());
        write(&d, &SimSwitch { slots: vec![3, 0, 3] }).unwrap();
        let s = read(&d).unwrap();
        assert_eq!(s.slots, vec![0, 3]);
        assert!(s.on(0) && s.on(3) && !s.on(1));
        std::fs::write(path(&d), "slots = [9]\n").unwrap();
        assert!(read(&d).unwrap_err().contains("doesn't exist"));
        std::fs::write(path(&d), "slots = [0]\nextra = 1\n").unwrap();
        assert!(read(&d).is_err(), "unknown keys are refused");
    }

    #[test]
    fn enable_refuses_a_no_lan_owner_and_turns_on_a_full_one() {
        let d = crate::util::temp_dir("simset");
        std::fs::write(d.join("policy.toml"), "network = \"no-lan\"\n").unwrap();
        let e = set(&d, true, "0", true).unwrap_err();
        assert!(e.contains("network no-lan"), "{e}");
        assert_eq!(read(&d).unwrap(), SimSwitch::default(), "nothing written");
        std::fs::write(d.join("policy.toml"), "network = \"on\"\n").unwrap();
        assert!(set(&d, true, "slot:0", true).unwrap().contains("24 hours"));
        assert!(read(&d).unwrap().on(0));
        assert!(set(&d, true, "7", true).unwrap_err().contains("no server"));
        assert!(set(&d, false, "0", true).unwrap_err().contains("only on a Mac"));
        set(&d, true, "0", false).unwrap();
        assert!(!read(&d).unwrap().on(0));
    }
}
