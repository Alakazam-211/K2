//! `k2-signer`: K2's release signer for clients (prd-ci-on-compute-for-clients-v1
//! §4.5, slice C4, with the vs-live amendments V5, V13, V14, V28–V30).
//!
//! It runs as the same `_k2sign` role user as `k2sign` (V28) and shares its
//! state: the one-sha-per-asset registry, the receipt replay registry, the
//! audit log and the lock, so the two signers can never sign different bytes
//! for one version. `k2sign.py` keeps K2's non-compute assets and the S3b
//! (receipt v1) path unchanged; `k2-signer` owns receipt v2 evidence, owner
//! node rosters, signer ↔ server pairing and signing for client projects.
//!
//! Layout (production; `K2_SIGNER_TEST_ROOT` moves all of it under a test
//! directory and is refused as root or as the signing user):
//!
//! | Path | Owner | What |
//! |---|---|---|
//! | `/usr/local/libexec/k2-signer/{k2-signer,k2-signer-admin,k2-signer-bin}` | root 0755 | wrappers (env -i) and the binary |
//! | `/usr/local/etc/k2-signer/projects/<project>.toml` | root 0644 | per-project policy ([`k2_node_proto::signer_policy`]) |
//! | `/usr/local/etc/k2-signer/trust.json` | root 0644 | paired controllers, pinned owner roster keys, accepted node keys |
//! | `/usr/local/etc/k2-signer/approver.pem` | root 0644 | the Secure Enclave approver's public key |
//! | `/usr/local/etc/k2-signer/pins.sha256` | root 0644 | hashes of every file above |
//! | `/var/k2sign/k2-signer/` | `_k2sign` 0700 | evidence registry, roster high-water marks, client sessions, keys, pairing |
//! | `/var/k2sign/state/` | `_k2sign` 0700 | shared with k2sign: `.lock`, `assets/`, `compute/receipts.tsv`, `sessions/` (project `k2`) |
//!
//! Modules:
//! - [`ctx`]: where things are, who may run, the lock, files, audit.
//! - [`minisign`]: minisign / Tauri updater keys and signatures in Rust.
//! - [`policy`]: loading a project's policy, the signer's extra rules.
//! - [`trust`]: controllers, owner roster keys, accepted keys, high-water marks.
//! - [`forge`]: what the forge (GitHub) says about a tag.
//! - [`check`]: every evidence rule (the heart; pure).
//! - [`registry`]: evidence never replaced, replays, one sha per asset.
//! - [`session`]: approval sessions (k2sign's for `k2`, its own for clients).
//! - [`pairing`]: signer ↔ server pairing with a 6-digit SAS.
//! - [`k2daemon`]: K2's own Linux daemon byte checks (`check-daemon-asset.py`).
//! - [`verbs`]: the command surface.

pub mod check;
pub mod ctx;
pub mod forge;
pub mod k2daemon;
pub mod minisign;
pub mod pairing;
pub mod policy;
pub mod registry;
pub mod session;
pub mod trust;
pub mod verbs;

/// Why a verb stopped. Exit codes match k2sign: 2 usage, 3 refused (one red
/// line saying why), 4 internal. Every check that can't run is a refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    Usage(String),
    Refused(String),
    Internal(String),
}

impl Fail {
    pub fn exit_code(&self) -> i32 {
        match self {
            Fail::Usage(_) => 2,
            Fail::Refused(_) => 3,
            Fail::Internal(_) => 4,
        }
    }
    pub fn message(&self) -> &str {
        match self {
            Fail::Usage(m) | Fail::Refused(m) | Fail::Internal(m) => m,
        }
    }
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Self {
        Fail::Internal(format!("io: {e}"))
    }
}

pub type R<T> = Result<T, Fail>;

/// A refusal.
pub fn refused<T>(msg: impl Into<String>) -> R<T> {
    Err(Fail::Refused(msg.into()))
}

/// A usage error.
pub fn usage<T>(msg: impl Into<String>) -> R<T> {
    Err(Fail::Usage(msg.into()))
}

/// Turn a `String` error (from k2-node-proto) into a refusal.
pub trait OrRefuse<T> {
    fn or_refuse(self) -> R<T>;
    fn or_refuse_with(self, what: &str) -> R<T>;
}

impl<T> OrRefuse<T> for Result<T, String> {
    fn or_refuse(self) -> R<T> {
        self.map_err(Fail::Refused)
    }
    fn or_refuse_with(self, what: &str) -> R<T> {
        self.map_err(|e| Fail::Refused(format!("{what}: {e}")))
    }
}
