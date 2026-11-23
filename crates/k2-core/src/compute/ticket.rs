//! Controller side of the one-click enroll ticket (§7.2, CN15).
//!
//! The control plane signs tickets with an Ed25519 key that is **only**
//! for tickets (never an edge-attestation key: `edge_attest`'s keys are a
//! separate list, so an edge key can never mint a node). This server
//! trusts:
//!
//! - [`BAKED_KEYS`] — empty until k2-connect ships the route and its key
//!   (spec: `.k2/prds/compute-polish-v1-notes.md`); then the public half
//!   is baked here in a release;
//! - `~/.k2/compute-ticket-keys.json` (`{"keys":[{"kid","pub"}]}`, the
//!   owner's file) for rotation and for testing before the bake.
//!
//! No trusted key → [`supported`] is false and every ticket is refused
//! with `ticket_unsupported` (the capability check: enroll codes keep
//! working). A valid ticket only makes the node **pending**; the SAS
//! confirm still decides.
//!
//! A ticket works once. Used nonces are kept in
//! `~/.k2/compute-ticket-used.json` until their ticket expires, so a
//! restart doesn't make a used ticket good again. If that file can't be
//! read or written, tickets are refused (enroll codes keep working).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use aws_lc_rs::signature::{UnparsedPublicKey, ED25519};
use base64::Engine;

use super::proto::ticket::{Ticket, MAX_LIFETIME_SECS};

/// Baked first-party ticket keys `(kid, base64 raw 32-byte public key)`.
pub const BAKED_KEYS: &[(&str, &str)] = &[];

/// The kid the spec reserves for the first key.
pub const FIRST_KID: &str = "k2-compute-ticket-2026-10";

pub fn override_path() -> PathBuf {
    crate::paths::k2_home().join("compute-ticket-keys.json")
}

fn decode_pub(b64: &str) -> Option<[u8; 32]> {
    base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?.try_into().ok()
}

/// `kid → key`: baked, then the override file (a bad file is ignored).
pub fn trusted_keys() -> HashMap<String, [u8; 32]> {
    let mut keys = HashMap::new();
    for (kid, b64) in BAKED_KEYS {
        if let Some(k) = decode_pub(b64) {
            keys.insert((*kid).to_string(), k);
        }
    }
    let Ok(raw) = std::fs::read_to_string(override_path()) else { return keys };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        crate::log_debug!("[compute] ignoring malformed {}", override_path().display());
        return keys;
    };
    for e in v.get("keys").and_then(|k| k.as_array()).cloned().unwrap_or_default() {
        let kid = e.get("kid").and_then(|k| k.as_str()).unwrap_or("").trim().to_string();
        let key = e.get("pub").and_then(|k| k.as_str()).and_then(decode_pub);
        if let (false, Some(k)) = (kid.is_empty(), key) {
            keys.insert(kid, k);
        }
    }
    keys
}

/// True when this server can verify one-click tickets at all.
pub fn supported() -> bool {
    !trusted_keys().is_empty()
}

/// Why a ticket was refused (`code()` goes on the wire).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketRefusal {
    Unsupported,
    Malformed(String),
    UnknownKid,
    BadSignature,
    Expired,
    TooLong,
    WrongController,
    WrongNode,
    Replayed,
    /// The used-ticket file couldn't be read or written, so the ticket
    /// can't be marked used and is refused.
    StoreUnavailable,
}

impl TicketRefusal {
    pub fn code(&self) -> &'static str {
        match self {
            TicketRefusal::Unsupported => "ticket_unsupported",
            _ => "ticket_invalid",
        }
    }
    pub fn message(&self) -> String {
        match self {
            TicketRefusal::Unsupported => "this server doesn't take one-click tickets yet; use an enroll code".into(),
            TicketRefusal::Malformed(e) => format!("the ticket is malformed: {e}"),
            TicketRefusal::UnknownKid => "the ticket was signed with a key this server doesn't trust".into(),
            TicketRefusal::BadSignature => "the ticket's signature doesn't verify".into(),
            TicketRefusal::Expired => "the ticket expired; get a new one".into(),
            TicketRefusal::TooLong => "the ticket lives too long to be one the control plane minted".into(),
            TicketRefusal::WrongController => "the ticket is for another server".into(),
            TicketRefusal::WrongNode => "the ticket is for another node key or name".into(),
            TicketRefusal::Replayed => "this ticket was already used".into(),
            TicketRefusal::StoreUnavailable => {
                "this server couldn't record the ticket as used, so it won't take it; use an enroll code".into()
            }
        }
    }
}

/// The file of used ticket nonces: `{"used": {"<kid>:<nonce>": exp}}`.
pub fn used_path() -> PathBuf {
    crate::paths::k2_home().join("compute-ticket-used.json")
}

/// Used ticket nonces (`kid:nonce` → the ticket's exp), kept until exp.
/// A persistent set reads its file once, on first use, and writes the
/// whole set back (atomically, 0600, expired entries dropped) before a
/// new use counts. Any read or write failure refuses the ticket.
pub struct UsedNonces {
    /// `None`: memory only (tests of the pre-file behaviour).
    path: Option<PathBuf>,
    /// `None` until the file has been read.
    used: Option<HashMap<String, i64>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct UsedFile {
    used: HashMap<String, i64>,
}

impl UsedNonces {
    pub fn persistent(path: PathBuf) -> Self {
        Self { path: Some(path), used: None }
    }

    #[cfg(test)]
    fn memory_only() -> Self {
        Self { path: None, used: Some(HashMap::new()) }
    }

    fn load(path: &Path) -> Result<HashMap<String, i64>, String> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(e) => return Err(e.to_string()),
        };
        serde_json::from_str::<UsedFile>(&raw).map(|f| f.used).map_err(|e| format!("not a used-ticket file: {e}"))
    }

    fn save(path: &Path, used: &HashMap<String, i64>) -> Result<(), String> {
        let body = serde_json::to_vec(&UsedFile { used: used.clone() }).map_err(|e| e.to_string())?;
        crate::fs_atomic::atomic_write_private(path, &body).map_err(|e| e.to_string())
    }

    /// The owner's log line for a file that can't be read: where it is and
    /// how to clear it. Deleting it forgets the uses it holds, so it is
    /// safe only once every ticket it could name has expired.
    fn unreadable(path: &Path, e: &str) -> String {
        format!(
            "[compute] compute-ticket-used.json at {} can't be read ({e}); one-click tickets are refused until it's fixed or removed: \
             delete it once no ticket is pending (they expire within {} minutes)",
            path.display(),
            MAX_LIFETIME_SECS / 60
        )
    }

    fn unwritable(path: &Path, e: &str) -> String {
        format!(
            "[compute] compute-ticket-used.json at {} can't be written ({e}); one-click tickets are refused until it can be: \
             check the folder's free space and permissions",
            path.display()
        )
    }

    /// Mark `key` used until `exp`. Refuses a key that is already used,
    /// and any use this set can't load or record.
    pub fn consume(&mut self, key: &str, exp: i64, now: i64) -> Result<(), TicketRefusal> {
        let used = match self.used.take() {
            Some(u) => u,
            None => match &self.path {
                Some(p) => Self::load(p).map_err(|e| {
                    crate::log_debug!("{}", Self::unreadable(p, &e));
                    TicketRefusal::StoreUnavailable
                })?,
                None => HashMap::new(),
            },
        };
        let mut next: HashMap<String, i64> = used.into_iter().filter(|(_, e)| *e >= now).collect();
        if next.contains_key(key) {
            self.used = Some(next);
            return Err(TicketRefusal::Replayed);
        }
        next.insert(key.to_string(), exp);
        if let Some(p) = &self.path {
            if let Err(e) = Self::save(p, &next) {
                crate::log_debug!("{}", Self::unwritable(p, &e));
                // Memory keeps matching the file: this use never counted.
                next.remove(key);
                self.used = Some(next);
                return Err(TicketRefusal::StoreUnavailable);
            }
        }
        self.used = Some(next);
        Ok(())
    }
}

/// This server's used-ticket set, under K2's home.
fn used_store() -> MutexGuard<'static, UsedNonces> {
    static STORE: OnceLock<Mutex<UsedNonces>> = OnceLock::new();
    crate::guard_test_store_home("compute-ticket-used.json");
    let path = used_path();
    let mut store = STORE.get_or_init(|| Mutex::new(UsedNonces::persistent(path.clone()))).lock().unwrap_or_else(|e| e.into_inner());
    if store.path.as_deref() != Some(path.as_path()) {
        *store = UsedNonces::persistent(path);
    }
    store
}

/// Verify a ticket for this server (`own_sub`: the bare tunnel subdomain,
/// or a full host for custom-domain controllers) and the node that
/// presented it. Consumes the nonce on success.
pub fn verify(ticket: &str, own_sub: &str, node_fp: &str, node_name: &str, now: i64) -> Result<Ticket, TicketRefusal> {
    verify_with(&trusted_keys(), ticket, own_sub, node_fp, node_name, now)
}

/// [`verify`] with these keys, against this server's used-ticket set.
pub fn verify_with(
    keys: &HashMap<String, [u8; 32]>,
    ticket: &str,
    own_sub: &str,
    node_fp: &str,
    node_name: &str,
    now: i64,
) -> Result<Ticket, TicketRefusal> {
    verify_with_store(keys, &mut used_store(), ticket, own_sub, node_fp, node_name, now)
}

/// [`verify_with`] against the used-ticket set `used`.
pub fn verify_with_store(
    keys: &HashMap<String, [u8; 32]>,
    used: &mut UsedNonces,
    ticket: &str,
    own_sub: &str,
    node_fp: &str,
    node_name: &str,
    now: i64,
) -> Result<Ticket, TicketRefusal> {
    if keys.is_empty() {
        return Err(TicketRefusal::Unsupported);
    }
    let t = Ticket::decode(ticket).map_err(TicketRefusal::Malformed)?;
    let key = keys.get(&t.kid).ok_or(TicketRefusal::UnknownKid)?;
    let sig = base64::engine::general_purpose::STANDARD
        .decode(t.sig.trim())
        .map_err(|_| TicketRefusal::Malformed("signature isn't base64".into()))?;
    let msg = t.message().map_err(TicketRefusal::Malformed)?;
    UnparsedPublicKey::new(&ED25519, key).verify(&msg, &sig).map_err(|_| TicketRefusal::BadSignature)?;
    if t.exp < now {
        return Err(TicketRefusal::Expired);
    }
    if t.exp > now + MAX_LIFETIME_SECS {
        return Err(TicketRefusal::TooLong);
    }
    let own = own_sub.trim().to_ascii_lowercase();
    let theirs = t.controller_sub.trim().to_ascii_lowercase();
    if own.is_empty() || (theirs != own && crate::hosts::box_host(&theirs) != own && theirs != crate::hosts::box_host(&own)) {
        return Err(TicketRefusal::WrongController);
    }
    if t.node_fp != node_fp || t.node_name != node_name {
        return Err(TicketRefusal::WrongNode);
    }
    used.consume(&format!("{}:{}", t.kid, t.nonce), t.exp, now)?;
    Ok(t)
}

/// Reference minting (what k2-connect's route does), from a raw 32-byte
/// Ed25519 seed. Used by tests and to print the spec's test vector; a
/// server never holds a ticket seed. Returns `(public key b64, ticket)`.
#[allow(clippy::too_many_arguments)]
pub fn mint_with_seed(
    seed: &[u8; 32],
    kid: &str,
    controller_sub: &str,
    node_fp: &str,
    node_name: &str,
    exp: i64,
    nonce: &str,
) -> Result<(String, String), String> {
    use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
    let kp = Ed25519KeyPair::from_seed_unchecked(seed).map_err(|_| "bad seed".to_string())?;
    let mut t = Ticket {
        v: 1,
        kid: kid.into(),
        controller_sub: controller_sub.into(),
        node_fp: node_fp.into(),
        node_name: node_name.into(),
        exp,
        nonce: nonce.into(),
        sig: String::new(),
    };
    t.sig = base64::engine::general_purpose::STANDARD.encode(kp.sign(&t.message()?).as_ref());
    Ok((base64::engine::general_purpose::STANDARD.encode(kp.public_key().as_ref()), t.encode()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};

    fn kp() -> Ed25519KeyPair {
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    fn mint(k: &Ed25519KeyPair, kid: &str, sub: &str, fp: &str, name: &str, exp: i64, nonce: &str) -> String {
        let mut t = Ticket {
            v: 1,
            kid: kid.into(),
            controller_sub: sub.into(),
            node_fp: fp.into(),
            node_name: name.into(),
            exp,
            nonce: nonce.into(),
            sig: String::new(),
        };
        let sig = k.sign(&t.message().unwrap());
        t.sig = base64::engine::general_purpose::STANDARD.encode(sig.as_ref());
        t.encode().unwrap()
    }

    /// A used-ticket set in its own temp K2 home.
    struct Home(PathBuf);

    impl Home {
        fn new() -> Self {
            let home = crate::test_env::unique_temp_path("ticket-home");
            std::fs::create_dir_all(&home).unwrap();
            Self(home)
        }
        fn used_file(&self) -> PathBuf {
            self.0.join("compute-ticket-used.json")
        }
        /// What a fresh daemon on this home starts with.
        fn store(&self) -> UsedNonces {
            UsedNonces::persistent(self.used_file())
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn one_key(k: &Ed25519KeyPair) -> HashMap<String, [u8; 32]> {
        HashMap::from([(FIRST_KID.to_string(), <[u8; 32]>::try_from(k.public_key().as_ref()).unwrap())])
    }

    #[test]
    fn a_good_ticket_verifies_once_and_everything_else_is_refused() {
        let k = kp();
        let other = kp();
        let keys = one_key(&k);
        let home = Home::new();
        let mut used = home.store();
        let fp = "ab".repeat(32);
        let now = 1_800_000_000;
        let good = mint(&k, FIRST_KID, "alice", &fp, "mini-1", now + 600, "nonce-good-1");
        let t = verify_with_store(&keys, &mut used, &good, "alice", &fp, "mini-1", now).unwrap();
        assert_eq!(t.node_name, "mini-1");
        assert_eq!(verify_with_store(&keys, &mut used, &good, "alice", &fp, "mini-1", now), Err(TicketRefusal::Replayed));
        let full_host = mint(&k, FIRST_KID, "alice", &fp, "mini-1", now + 600, "nonce-host");
        assert!(verify_with_store(&keys, &mut used, &full_host, "alice.k2.dev", &fp, "mini-1", now).is_ok(), "bare sub vs full host");

        let cases = [
            (mint(&other, FIRST_KID, "alice", &fp, "mini-1", now + 600, "n1"), TicketRefusal::BadSignature),
            (mint(&k, "k2-edge-app-2026-09", "alice", &fp, "mini-1", now + 600, "n2"), TicketRefusal::UnknownKid),
            (mint(&k, FIRST_KID, "alice", &fp, "mini-1", now - 1, "n3"), TicketRefusal::Expired),
            (mint(&k, FIRST_KID, "alice", &fp, "mini-1", now + 3600, "n4"), TicketRefusal::TooLong),
            (mint(&k, FIRST_KID, "akzm", &fp, "mini-1", now + 600, "n5"), TicketRefusal::WrongController),
            (mint(&k, FIRST_KID, "alice", &"cd".repeat(32), "mini-1", now + 600, "n6"), TicketRefusal::WrongNode),
            (mint(&k, FIRST_KID, "alice", &fp, "mini-2", now + 600, "n7"), TicketRefusal::WrongNode),
        ];
        for (tk, want) in cases {
            assert_eq!(verify_with_store(&keys, &mut used, &tk, "alice", &fp, "mini-1", now), Err(want));
        }
        assert!(matches!(verify_with_store(&keys, &mut used, "garbage", "alice", &fp, "mini-1", now), Err(TicketRefusal::Malformed(_))));
        assert_eq!(verify_with_store(&HashMap::new(), &mut used, &good, "alice", &fp, "mini-1", now), Err(TicketRefusal::Unsupported));
        assert_eq!(TicketRefusal::Unsupported.code(), "ticket_unsupported");
        assert_eq!(TicketRefusal::Expired.code(), "ticket_invalid");
        assert_eq!(TicketRefusal::StoreUnavailable.code(), "ticket_invalid");
    }

    /// The restart case: a fresh used-ticket set on the same home (what a
    /// restarted daemon builds) still refuses a ticket used before it.
    fn use_then_restart_then_replay(first: &mut UsedNonces, after_restart: &mut UsedNonces) -> Result<Ticket, TicketRefusal> {
        let k = kp();
        let keys = one_key(&k);
        let fp = "ab".repeat(32);
        let now = 1_800_000_000;
        let tk = mint(&k, FIRST_KID, "alice", &fp, "mini-1", now + 600, "nonce-restart");
        verify_with_store(&keys, first, &tk, "alice", &fp, "mini-1", now).expect("first use");
        verify_with_store(&keys, after_restart, &tk, "alice", &fp, "mini-1", now + 60)
    }

    #[test]
    fn a_ticket_used_before_a_restart_is_refused_after_it() {
        let home = Home::new();
        let got = use_then_restart_then_replay(&mut home.store(), &mut home.store());
        assert_eq!(got, Err(TicketRefusal::Replayed));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(home.used_file()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the used-ticket file is owner-only");
        }
    }

    /// The contrast: a set kept in memory only starts empty after a
    /// restart, so the file is what carries uses across one.
    #[test]
    fn a_memory_only_set_forgets_a_used_ticket_on_restart() {
        let got = use_then_restart_then_replay(&mut UsedNonces::memory_only(), &mut UsedNonces::memory_only());
        assert!(got.is_ok(), "{got:?}");
    }

    #[test]
    fn expired_entries_are_pruned_from_the_file() {
        let home = Home::new();
        let now = 1_800_000_000;
        std::fs::write(home.used_file(), json_used(&[("k:old", now - 1), ("k:live", now + 300)])).unwrap();
        let mut used = home.store();
        used.consume("k:new", now + 600, now).unwrap();
        let file: UsedFile = serde_json::from_str(&std::fs::read_to_string(home.used_file()).unwrap()).unwrap();
        let mut keys: Vec<&str> = file.used.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["k:live", "k:new"]);
        assert_eq!(home.store().consume("k:live", now + 300, now), Err(TicketRefusal::Replayed), "a live entry from the file still counts");
    }

    fn json_used(entries: &[(&str, i64)]) -> String {
        let used: HashMap<String, i64> = entries.iter().map(|(k, e)| (k.to_string(), *e)).collect();
        serde_json::to_string(&UsedFile { used }).unwrap()
    }

    #[test]
    fn a_used_file_that_cant_be_read_refuses_every_ticket() {
        let k = kp();
        let keys = one_key(&k);
        let fp = "ab".repeat(32);
        let now = 1_800_000_000;
        let tk = mint(&k, FIRST_KID, "alice", &fp, "mini-1", now + 600, "nonce-closed");
        let home = Home::new();
        // Not JSON, then not a file at all.
        std::fs::write(home.used_file(), "not json").unwrap();
        assert_eq!(verify_with_store(&keys, &mut home.store(), &tk, "alice", &fp, "mini-1", now), Err(TicketRefusal::StoreUnavailable));
        std::fs::remove_file(home.used_file()).unwrap();
        std::fs::create_dir(home.used_file()).unwrap();
        assert_eq!(verify_with_store(&keys, &mut home.store(), &tk, "alice", &fp, "mini-1", now), Err(TicketRefusal::StoreUnavailable));
        // Once the file is good again the same ticket is taken: the refusals
        // didn't count as uses.
        std::fs::remove_dir(home.used_file()).unwrap();
        assert!(verify_with_store(&keys, &mut home.store(), &tk, "alice", &fp, "mini-1", now).is_ok());
    }

    #[test]
    fn the_log_line_names_the_file_and_how_to_clear_it() {
        let p = crate::paths::k2_home_in("/home/acme").join("compute-ticket-used.json");
        let line = UsedNonces::unreadable(&p, "expected value at line 1 column 1");
        assert!(line.contains(&format!("at {} can't be read (expected value", p.display())), "{line}");
        assert!(line.contains("delete it once no ticket is pending (they expire within 15 minutes)"), "{line}");
        let line = UsedNonces::unwritable(&p, "No space left on device");
        assert!(line.contains(&format!("at {} can't be written (No space left on device)", p.display())), "{line}");
    }

    #[test]
    fn a_use_that_cant_be_written_is_refused_and_not_counted() {
        let home = Home::new();
        let now = 1_800_000_000;
        let mut used = home.store();
        used.consume("k:first", now + 600, now).unwrap();
        // The file's place is taken by a folder with something in it: the
        // atomic rename onto it fails.
        std::fs::remove_file(home.used_file()).unwrap();
        std::fs::create_dir(home.used_file()).unwrap();
        std::fs::write(home.used_file().join("x"), "x").unwrap();
        assert_eq!(used.consume("k:second", now + 600, now), Err(TicketRefusal::StoreUnavailable));
        std::fs::remove_dir_all(home.used_file()).unwrap();
        assert_eq!(used.consume("k:second", now + 600, now), Ok(()), "the failed write didn't mark it used");
        assert_eq!(used.consume("k:first", now + 600, now), Err(TicketRefusal::Replayed));
    }

    /// The spec's reference vector (`.k2/prds/compute-polish-v1-notes.md`):
    /// its fields are the spec's own, byte for byte (the subject included).
    /// seed 32×0x07, these fields → this exact public key and signature.
    #[test]
    fn reference_vector_for_the_control_plane_spec() {
        let (pubk, tk) = mint_with_seed(&[7u8; 32], FIRST_KID, "rosson", &"ab".repeat(32), "mini-1", 1_800_000_600, "AAAAAAAAAAAAAAAAAAAAAA").unwrap();
        let t = Ticket::decode(&tk).unwrap();
        assert_eq!(pubk, "6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw=");
        // Cross-checked with Python `cryptography` (Ed25519, same message).
        assert_eq!(t.sig, "elSKSpTjMPZA4A0XR07r7117gxxi/kwgn1k0GiIIFpCn0DTznfltg/UbqhKp3zzGRyxD6cUZ8kIOAmzY+ueQBQ==");
        let mut keys = HashMap::new();
        keys.insert(FIRST_KID.to_string(), decode_pub(&pubk).unwrap());
        let home = Home::new();
        assert!(verify_with_store(&keys, &mut home.store(), &tk, "rosson", &"ab".repeat(32), "mini-1", 1_800_000_000).is_ok());
    }

    #[test]
    fn ticket_keys_are_separate_from_edge_keys() {
        for (kid, _) in crate::edge_attest::BAKED_KEYS {
            assert!(!BAKED_KEYS.iter().any(|(k, _)| k == kid), "{kid} must not mint tickets");
        }
        assert_ne!(override_path().file_name(), crate::edge_attest::override_path().file_name());
        assert_ne!(used_path().file_name(), override_path().file_name());
    }
}
