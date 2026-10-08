//! K2 vault (`prd-k2-vault-v1.md`): one encrypted store for passwords,
//! tokens and keys, used in-process by the daemon behind [`SecretStore`].
//!
//! - One encrypted file (`~/.k2/vault/vault.db`), one key chain, one audit
//!   log.
//! - Two namespaces: `system` (K2's own secrets; no route ever returns a
//!   value) and `user` (owner, agent and app secrets, used through grants).
//! - Agents use secrets without seeing them by default: fetch/proxy add
//!   the value to calls, only to the hosts the secret is bound to, only
//!   where its creator said (sealed injection). Env/file are opt-in.
//!
//! This crate does no network I/O and has no dependency on k2-core or
//! k2-daemon.

pub mod agefmt;
pub mod audit;
pub mod carry;
pub mod confirm;
pub mod crypto;
pub mod db;
pub mod delivery;
pub mod engine;
pub mod error;
pub mod fetch;
pub mod fsutil;
pub mod hosts;
pub mod inject;
pub mod keystore;
pub mod model;
pub mod policy;
pub mod provider;
pub mod recovery;
pub mod redact;
pub mod refs;
pub mod secret;
pub mod store;
pub mod vault;

pub use error::{Result, VaultError};
pub use keystore::{Anchor, KeyHome, KeyStore};
pub use model::{Consumer, ConsumerKind, Delivery, Kind, Namespace, Scope, ScopeKind, SecretStatus};
pub use policy::{Actor, AgentActor, HumanProof, Role};
pub use secret::{Key32, SecretBytes};
pub use store::{
    AuditQuery, AuditRow, Grant, GrantRequest, ListFilter, Placeholder, PolicyPatch, ProxyItem, PutMode, PutOutcome, PutRequest,
    Requester, Resolved, SecretMeta, SecretStore, Selector, SpawnItem, Use,
};
pub use vault::{LocalVault, OpenOptions, VaultStatus};
