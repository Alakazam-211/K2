//! Phase-5 roster — discoverable cross-server addressing.
//!
//! `prd-cross-server-agent-comms.md` Phase 5. Two halves, both fail-closed:
//!
//!   - **Projection** ([`build_local_roster`]) — THIS daemon's exposed agents:
//!     for every registered workspace that has a CONFIGURED agent (per the
//!     #70 DB-canonical [`resolve_agent_name`](crate::workspace::agent_identity::resolve_agent_name)),
//!     one [`RosterAgent`] carrying the workspace's authoritative `projects.id`
//!     UUID (Risk H1 — exact id, never the fuzzy name), its display name, the
//!     agent name, and the `<workspace-uuid>::<agent>` address fragment a peer
//!     prefixes with our selector to form `<peer>::<workspace>::<agent>`.
//!     Workspaces with no configured agent are NOT exposed (opt-in by
//!     configuration).
//!
//!   - **Peer authentication of the GET** ([`sign_roster_request`] /
//!     [`verify_roster_request`]) — the roster route is a GET, so it carries no
//!     signed envelope. Instead the calling peer proves possession of its
//!     pinned key by signing a short, timestamped challenge; the serving daemon
//!     verifies the signature against the pinned key, bounds the timestamp by a
//!     skew window, and gates with [`PeerStore::require_peer`]`(fp, "roster")`.
//!     A bare fingerprint is public, so the SIGNATURE (not the fingerprint
//!     param) is the credential — DECISION-2: NEVER a `token_ok`/owner check.
//!
//! Read-only by construction: the projection enumerates DB rows + resolves
//! names; it spawns nothing and touches no PTY (inbox-safe).

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{
    EcdsaKeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_ASN1_SIGNING,
};
use base64::Engine;
use chrono::Utc;
use rcgen::KeyPair;
use serde::{Deserialize, Serialize};

use super::peers::{
    fingerprint_of_spki_der, raw_point_from_spki_der, spki_pem_to_der, FederationPeer, PeerStore,
    RequirePeerError,
};

/// Capability a peer must hold to read this daemon's roster.
pub const CAP_ROSTER: &str = "roster";

/// Accepted clock-skew window (seconds) for the roster-request challenge
/// timestamp. A request whose `ts` is older/newer than this is rejected
/// (bounds the replay opportunity on an otherwise-unauthenticated GET).
pub const DEFAULT_ROSTER_SKEW_SECS: i64 = 300;

/// Domain-separation prefix for the roster-request challenge — keeps a roster
/// signature from ever being reusable as any other kind of signature.
const ROSTER_CHALLENGE_PREFIX: &str = "k2-federation-roster";

/// One exposed agent in this daemon's roster projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterAgent {
    /// Authoritative `projects.id` v4 UUID (Risk H1 — exact, not the name).
    pub workspace_id: String,
    /// Workspace display name (`projects.name`) — convenience only.
    pub workspace_name: String,
    /// The workspace agent's resolved name (#70 DB-canonical resolver).
    pub agent: String,
    /// `<workspace-uuid>::<agent>` — the peer prefixes its selector to address
    /// us as `<peer>::<workspace>::<agent>`.
    pub address: String,
    /// Pre-slug / basename / previous-handle tokens (D8). Empty on old peers.
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// This daemon's roster projection (the body the GET returns to a peer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LocalRoster {
    pub agents: Vec<RosterAgent>,
}

/// Build the local roster projection: every registered workspace that has a
/// CONFIGURED agent **and** is willing to accept federated contact.
///
/// Exposure rules (fail-closed):
/// 1. Workspace must have a resolvable agent name (configuration opt-in).
/// 2. Contact permission:
///    - App master **"Let remote users message agents"** ON → all configured
///      agents are listed; OR
///    - Master OFF → only workspaces with **per-workspace Remote Access**
///      (`allow_remote_instruct`) OR **Allow agents to create connections**
///      for this workspace (`agents_can_create_connections`) ON.
///
/// Read-only (DB + name resolution); spawns nothing.
pub fn build_local_roster() -> LocalRoster {
    let mut agents = Vec::new();
    let master_remote = crate::app_settings::load().allow_remote_instruct;
    let projects = crate::projects_ops::projects_list().unwrap_or_default();
    for p in projects {
        if let Some(agent) = visible_agent_name(&p.path, master_remote) {
            // Roster `agent` is the handle (D8). Prefer projects.handle;
            // fall back to resolve_agent_name (AGENT.md `name:` after
            // migrate, or basename for unmigrated test rows).
            let handle = p.handle.trim();
            let agent = if !handle.is_empty() {
                handle.to_ascii_lowercase()
            } else {
                agent.to_ascii_lowercase()
            };
            let aliases = {
                let db = crate::db::shared();
                let conn = db.lock();
                crate::workspace::handle::aliases_for(&conn, &p.id)
            };
            agents.push(RosterAgent {
                address: format!("{}::{}", p.id, agent),
                workspace_id: p.id,
                workspace_name: p.name,
                agent,
                aliases,
            });
        }
    }
    LocalRoster { agents }
}

/// The roster exposure rule for one workspace (see [`build_local_roster`]):
/// contact permission (app master, per-workspace Remote Access, or Allow
/// agents to create connections) AND a configured agent. Returns the
/// trimmed resolved agent name when visible.
fn visible_agent_name(project_path: &str, master_remote: bool) -> Option<String> {
    if !master_remote {
        let ws_remote = crate::workspace::settings::get_allow_remote_instruct(project_path);
        let ws_conn =
            crate::workspace::settings::agents_can_create_connections_for_path(project_path);
        if !ws_remote && !ws_conn {
            return None;
        }
    }
    crate::workspace::agent_identity::resolve_agent_name(project_path)
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
}

/// True when the workspace at `project_path` is listed in this daemon's
/// roster — the same rule [`build_local_roster`] applies.
pub fn is_visible(project_path: &str) -> bool {
    visible_agent_name(project_path, crate::app_settings::load().allow_remote_instruct).is_some()
}

/// Receiver gate for a verified federated signal addressed to
/// `(workspace_id, to_name)` (CA1). Accepts only when the workspace is
/// visible under this daemon's roster rules AND `to_name` is one of its
/// names (handle, alias, `projects.name`, resolved agent name) under any
/// sender fold ([`names_loosely_match`](crate::workspace::handle::names_loosely_match)).
/// Fail-closed: an unknown id, a hidden workspace or a foreign name → false.
pub fn accepts_inbound(workspace_id: &str, project_path: &str, to_name: &str) -> bool {
    if to_name.trim().is_empty() {
        return false;
    }
    let master_remote = crate::app_settings::load().allow_remote_instruct;
    let Some(agent) = visible_agent_name(project_path, master_remote) else {
        return false;
    };
    let mut names = {
        let db = crate::db::shared();
        let conn = db.lock();
        crate::workspace::handle::workspace_names(&conn, workspace_id)
    };
    if names.is_empty() {
        return false;
    }
    names.push(agent);
    names
        .iter()
        .any(|n| crate::workspace::handle::names_loosely_match(to_name, n))
}

/// The exact challenge bytes signed/verified for a roster request. Binds the
/// signer's claimed fingerprint and a timestamp under a domain-separation
/// prefix.
pub fn roster_challenge(fingerprint: &str, ts: i64) -> String {
    format!("{ROSTER_CHALLENGE_PREFIX}\n{fingerprint}\n{ts}")
}

/// Sign a roster request with `my_key` for timestamp `ts`. Returns
/// `(my_fingerprint, base64-sig)` — the caller sends both (plus `ts`) as the
/// `fp` / `sig` query params the serving daemon verifies. Pure (CSPRNG only).
pub fn sign_roster_request(my_key: &KeyPair, ts: i64) -> Result<(String, String), String> {
    let fp = fingerprint_of_spki_der(&my_key.public_key_der());
    let challenge = roster_challenge(&fp, ts);
    let pkcs8 = my_key.serialize_der();
    let signing = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &pkcs8)
        .map_err(|e| format!("load signing key: {e}"))?;
    let rng = SystemRandom::new();
    let sig = signing
        .sign(&rng, challenge.as_bytes())
        .map_err(|e| format!("sign roster challenge: {e}"))?;
    Ok((
        fp,
        base64::engine::general_purpose::STANDARD.encode(sig.as_ref()),
    ))
}

/// Why a roster request was denied. Every variant is a DENY — fail-closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RosterAuthError {
    /// No peer pinned with the claimed fingerprint.
    UnknownPeer,
    /// Peer pinned but not `Trusted`.
    NotTrusted,
    /// Peer `Trusted` but lacks the `roster` capability.
    CapabilityDenied,
    /// The pinned key was malformed.
    Key(String),
    /// Signature did not verify against the pinned key (no key possession).
    BadSignature,
    /// The request `ts` is outside the accepted skew window.
    SkewTooLarge { skew_secs: i64 },
}

impl std::fmt::Display for RosterAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPeer => write!(f, "unknown peer (not pinned)"),
            Self::NotTrusted => write!(f, "peer not trusted"),
            Self::CapabilityDenied => write!(f, "peer lacks the roster capability"),
            Self::Key(e) => write!(f, "pinned key: {e}"),
            Self::BadSignature => write!(f, "roster signature verification failed"),
            Self::SkewTooLarge { skew_secs } => {
                write!(f, "clock skew {skew_secs}s exceeds the accepted window")
            }
        }
    }
}

impl std::error::Error for RosterAuthError {}

/// Verify a peer's roster request, fail-closed. Returns the verified peer ONLY
/// when: the fingerprint is pinned, the `sig` over [`roster_challenge`]
/// validates against the pinned key (key possession), `ts` is within
/// `skew_secs`, AND [`PeerStore::require_peer`]`(fp, "roster")` passes
/// (`Trusted` + holds the capability). Every other path is a DENY.
pub fn verify_roster_request<'a>(
    store: &'a PeerStore,
    fingerprint: &str,
    ts: i64,
    sig_b64: &str,
    skew_secs: i64,
) -> Result<&'a FederationPeer, RosterAuthError> {
    // 1. Look up the claimed peer ONLY to fetch its pinned key (authz is #4).
    let peer = store.get(fingerprint).ok_or(RosterAuthError::UnknownPeer)?;

    // 2. AUTHENTICITY — verify the signature against the pinned key. Proves the
    //    caller possesses the private key for `fingerprint`; a bare (public)
    //    fingerprint param is NOT sufficient.
    let der = spki_pem_to_der(&peer.public_key_pem).map_err(RosterAuthError::Key)?;
    let raw_point = raw_point_from_spki_der(&der).map_err(RosterAuthError::Key)?;
    let sig = base64::engine::general_purpose::STANDARD
        .decode(sig_b64.as_bytes())
        .map_err(|_| RosterAuthError::BadSignature)?;
    let challenge = roster_challenge(fingerprint, ts);
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, raw_point)
        .verify(challenge.as_bytes(), &sig)
        .map_err(|_| RosterAuthError::BadSignature)?;

    // 3. SKEW — bound the replay window on the GET.
    let skew = (Utc::now().timestamp() - ts).abs();
    if skew > skew_secs {
        return Err(RosterAuthError::SkewTooLarge { skew_secs: skew });
    }

    // 4. AUTHORIZATION — Trusted + holds `roster` (DECISION-2; never a token).
    store
        .require_peer(fingerprint, CAP_ROSTER)
        .map_err(|e| match e {
            RequirePeerError::UnknownPeer => RosterAuthError::UnknownPeer,
            RequirePeerError::NotTrusted(_) => RosterAuthError::NotTrusted,
            RequirePeerError::CapabilityDenied { .. } => RosterAuthError::CapabilityDenied,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::federation::peers::PeerTrust;
    use crate::tunnel::test_support::with_temp_home;

    fn peer_key() -> KeyPair {
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("gen peer key")
    }

    /// Pin `key` as a Trusted peer holding `roster`. Returns (store, fingerprint).
    fn trusted_roster_store(key: &KeyPair) -> (PeerStore, String) {
        let mut store = PeerStore::default();
        let fp = store.upsert(FederationPeer::pin("peer", "peer", key.public_key_pem()).unwrap());
        store.set_trust(&fp, PeerTrust::Trusted);
        store.grant(&fp, CAP_ROSTER);
        (store, fp)
    }

    // ── projection ────────────────────────────────────────────────────

    fn unique_ws_path(label: &str) -> String {
        std::env::temp_dir()
            .join(format!(
                "k2-roster-{label}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ))
            .to_string_lossy()
            .into_owned()
    }

    fn insert_project(name: &str, path: &str, agent_enabled: i64) -> String {
        let db = crate::db::shared();
        let conn = db.lock();
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path, agent_enabled) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, name, path, agent_enabled],
        )
        .expect("insert project row");
        id
    }

    #[test]
    fn build_local_roster_lists_configured_agents_only() {
        crate::db::init_for_tests();
        // A configured workspace (agent_enabled=1) and an unconfigured one.
        // Per-workspace Remote Access opts the configured one into the roster
        // (master allow_remote_instruct may be off in tests).
        let conf_path = unique_ws_path("conf");
        let conf_id = insert_project("roster-conf", &conf_path, 1);
        crate::workspace::settings::update_project_setting(
            &conf_path,
            "allow_remote_instruct",
            "1",
        )
        .expect("opt conf into remote contact");
        let bare_path = unique_ws_path("bare");
        let bare_id = insert_project("roster-bare", &bare_path, 0);

        let roster = build_local_roster();
        // The configured workspace appears, addressed by its UUID + basename.
        let conf = roster
            .agents
            .iter()
            .find(|a| a.workspace_id == conf_id)
            .expect("configured workspace must be exposed");
        let expected_basename = std::path::Path::new(&conf_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_ascii_lowercase();
        // D8: roster agent is the handle when minted, else resolve_agent_name
        // (folder basename for unmigrated test rows).
        assert!(
            conf.agent == expected_basename || conf.agent == "roster-conf",
            "roster agent should be handle or basename, got {}",
            conf.agent
        );
        assert_eq!(conf.address, format!("{}::{}", conf_id, conf.agent));
        assert_eq!(conf.workspace_name, "roster-conf");
        assert!(
            conf.agent.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "roster agent must be a handle token: {}",
            conf.agent
        );
        // The unconfigured workspace is NOT exposed (opt-in by configuration).
        assert!(
            roster.agents.iter().all(|a| a.workspace_id != bare_id),
            "an unconfigured workspace must not appear in the roster"
        );
    }

    #[test]
    fn build_local_roster_hides_agents_without_contact_permission() {
        // Isolate HOME so a developer's real ~/.k2/settings.json
        // (which may have allow_remote_instruct on) can't leak into the filter.
        with_temp_home(|| {
            crate::db::init_for_tests();
            let conf_path = unique_ws_path("noremote");
            let conf_id = insert_project("roster-noremote", &conf_path, 1);
            // Master default off; no per-workspace flags → must be hidden.
            let roster = build_local_roster();
            assert!(
                roster.agents.iter().all(|a| a.workspace_id != conf_id),
                "workspace without remote contact permission must be hidden"
            );
        });
    }

    /// `is_visible` is the roster's own rule: a workspace is visible exactly
    /// when `build_local_roster` lists it.
    #[test]
    fn is_visible_agrees_with_the_roster_projection() {
        with_temp_home(|| {
            crate::db::init_for_tests();
            let listed = unique_ws_path("vis-listed");
            insert_project("vis-listed", &listed, 1);
            crate::workspace::settings::update_project_setting(&listed, "allow_remote_instruct", "1")
                .expect("opt in");
            let no_contact = unique_ws_path("vis-nocontact");
            insert_project("vis-nocontact", &no_contact, 1);
            let no_agent = unique_ws_path("vis-noagent");
            insert_project("vis-noagent", &no_agent, 0);
            crate::workspace::settings::update_project_setting(&no_agent, "allow_remote_instruct", "1")
                .expect("opt in");

            let roster = build_local_roster();
            let projects = crate::projects_ops::projects_list().expect("list projects");
            for path in [&listed, &no_contact, &no_agent] {
                let id = &projects.iter().find(|p| &p.path == path).expect("registered").id;
                let in_roster = roster.agents.iter().any(|a| &a.workspace_id == id);
                assert_eq!(is_visible(path), in_roster, "is_visible vs roster for {path}");
            }
            assert!(is_visible(&listed));
            assert!(!is_visible(&no_contact));
            assert!(!is_visible(&no_agent));
        });
    }

    /// `accepts_inbound`: the workspace's own names pass; another
    /// workspace's name, an empty name and a hidden workspace do not.
    #[test]
    fn accepts_inbound_needs_a_visible_workspace_and_one_of_its_names() {
        with_temp_home(|| {
            crate::db::init_for_tests();
            let a_path = unique_ws_path("acc-a");
            let a_id = insert_project("Alpha Agent", &a_path, 1);
            crate::workspace::settings::update_project_setting(&a_path, "allow_remote_instruct", "1")
                .expect("opt in");
            let b_path = unique_ws_path("acc-b");
            let b_id = insert_project("Bravo Agent", &b_path, 1);
            crate::workspace::settings::update_project_setting(&b_path, "allow_remote_instruct", "1")
                .expect("opt in");
            let a_base = std::path::Path::new(&a_path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();

            assert!(accepts_inbound(&a_id, &a_path, "Alpha Agent"));
            assert!(accepts_inbound(&a_id, &a_path, "alpha-agent"));
            assert!(accepts_inbound(&a_id, &a_path, &a_base), "resolved agent name (basename)");
            assert!(!accepts_inbound(&a_id, &a_path, "Bravo Agent"), "B's name at A's id");
            assert!(!accepts_inbound(&b_id, &b_path, "Alpha Agent"), "A's name at B's id");
            assert!(!accepts_inbound(&a_id, &a_path, "  "), "empty name");

            crate::workspace::settings::update_project_setting(&a_path, "allow_remote_instruct", "0")
                .expect("opt out");
            assert!(!accepts_inbound(&a_id, &a_path, "Alpha Agent"), "hidden workspace");
        });
    }

    // ── peer-authenticated GET ────────────────────────────────────────

    #[test]
    fn roster_request_sign_verify_round_trip() {
        with_temp_home(|| {
            let key = peer_key();
            let (store, fp) = trusted_roster_store(&key);
            let ts = Utc::now().timestamp();
            let (signed_fp, sig) = sign_roster_request(&key, ts).expect("sign");
            assert_eq!(signed_fp, fp, "signer fingerprint must match the pinned id");
            verify_roster_request(&store, &fp, ts, &sig, DEFAULT_ROSTER_SKEW_SECS)
                .expect("a trusted peer with a valid signature must be allowed");
        });
    }

    #[test]
    fn verify_denies_unknown_peer() {
        let key = peer_key();
        let ts = Utc::now().timestamp();
        let (fp, sig) = sign_roster_request(&key, ts).unwrap();
        let store = PeerStore::default(); // nobody pinned
        let err = verify_roster_request(&store, &fp, ts, &sig, 300)
            .expect_err("unknown peer must be denied");
        assert_eq!(err, RosterAuthError::UnknownPeer);
    }

    #[test]
    fn verify_denies_pending_peer() {
        let key = peer_key();
        let mut store = PeerStore::default();
        // Pinned + granted but left Pending → fail-closed.
        let fp = store.upsert(FederationPeer::pin("p", "p", key.public_key_pem()).unwrap());
        store.grant(&fp, CAP_ROSTER);
        let ts = Utc::now().timestamp();
        let (_fp, sig) = sign_roster_request(&key, ts).unwrap();
        let err =
            verify_roster_request(&store, &fp, ts, &sig, 300).expect_err("pending must be denied");
        assert_eq!(err, RosterAuthError::NotTrusted);
    }

    #[test]
    fn verify_denies_trusted_peer_without_roster_capability() {
        let key = peer_key();
        let mut store = PeerStore::default();
        let fp = store.upsert(FederationPeer::pin("p", "p", key.public_key_pem()).unwrap());
        store.set_trust(&fp, PeerTrust::Trusted); // Trusted but NO roster cap
        let ts = Utc::now().timestamp();
        let (_fp, sig) = sign_roster_request(&key, ts).unwrap();
        let err = verify_roster_request(&store, &fp, ts, &sig, 300)
            .expect_err("missing cap must be denied");
        assert_eq!(err, RosterAuthError::CapabilityDenied);
    }

    #[test]
    fn verify_rejects_bad_signature() {
        let key = peer_key();
        let (store, fp) = trusted_roster_store(&key);
        let ts = Utc::now().timestamp();
        // A signature made by a DIFFERENT key (attacker who knows the public fp).
        let attacker = peer_key();
        let (_afp, sig) = sign_roster_request(&attacker, ts).unwrap();
        let err = verify_roster_request(&store, &fp, ts, &sig, 300)
            .expect_err("a signature not made by the pinned key must be rejected");
        assert_eq!(err, RosterAuthError::BadSignature);
    }

    #[test]
    fn verify_rejects_stale_timestamp() {
        let key = peer_key();
        let (store, fp) = trusted_roster_store(&key);
        let ts = Utc::now().timestamp() - 10_000; // far outside the window
        let (_fp, sig) = sign_roster_request(&key, ts).unwrap();
        let err = verify_roster_request(&store, &fp, ts, &sig, 300)
            .expect_err("a stale timestamp must be rejected");
        assert!(matches!(err, RosterAuthError::SkewTooLarge { .. }), "got {err:?}");
    }
}
