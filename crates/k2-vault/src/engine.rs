//! Engine credentials (§10.7, V13, KV41): each engine (Stalwart,
//! Postgres, frpc, Sew, a future runtime) gets its own root credentials,
//! issued and kept by the vault as `system` secrets in scope
//! `engine:<id>`.
//!
//! - [`LocalVault::engine_ensure`] mints a credential if it's missing.
//! - [`LocalVault::engine_rotate`] makes a new value, lets the engine
//!   accept it (`apply`), and only then stores it.
//! - Asymmetric engine tokens: an Ed25519 key whose public half engines
//!   (or other boxes) check against ([`LocalVault::engine_public_key`],
//!   [`LocalVault::engine_sign`]).

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use zeroize::Zeroize;

use crate::error::{Result, VaultError};
use crate::model::{Delivery, Kind, Namespace, Scope};
use crate::policy::Actor;
use crate::secret::SecretBytes;
use crate::store::{PutMode, PutRequest, Requester, SecretMeta, SecretStore, Selector, Use};
use crate::vault::LocalVault;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gen {
    /// 32 random bytes as 64 lowercase hex characters (text-safe password).
    Random32,
    /// An Ed25519 signing key (32-byte seed).
    Ed25519,
}

fn generate(g: Gen) -> SecretBytes {
    let mut b = crate::crypto::random_bytes::<32>();
    let out = match g {
        Gen::Random32 => SecretBytes::from_string(crate::crypto::hex(&b)),
        Gen::Ed25519 => SecretBytes::from_slice(&b),
    };
    b.zeroize();
    out
}

fn kind_for(g: Gen) -> Kind {
    match g {
        Gen::Random32 => Kind::EngineCred,
        Gen::Ed25519 => Kind::SigningKey,
    }
}

fn actor(engine: &str) -> Actor {
    Actor::System(format!("engine:{engine}"))
}

impl LocalVault {
    fn engine_sel(engine: &str, name: &str) -> Selector {
        Selector::system(Scope::engine(engine), name)
    }

    fn engine_put(&self, engine: &str, name: &str, g: Gen, value: SecretBytes, mode: PutMode) -> Result<SecretMeta> {
        let mut r = PutRequest::new(Namespace::System, Scope::engine(engine), name, kind_for(g));
        r.default_delivery = Some(Delivery::None);
        r.delivery_ceiling = Some(Delivery::Env);
        r.origin = format!("system:engine:{engine}");
        r.mode = mode;
        Ok(self.put(r, value, &actor(engine))?.meta)
    }

    /// The credential, minted when missing.
    pub fn engine_ensure(&self, engine: &str, name: &str, g: Gen) -> Result<SecretMeta> {
        match self.meta(&Self::engine_sel(engine, name), &actor(engine)) {
            Ok(m) if m.status == crate::model::SecretStatus::Active => Ok(m),
            Ok(_) | Err(VaultError { status: 404, .. }) => self.engine_put(engine, name, g, generate(g), PutMode::Upsert),
            Err(e) => Err(e),
        }
    }

    /// The current value (daemon use only; never returned by a route).
    pub fn engine_value(&self, engine: &str, name: &str) -> Result<SecretBytes> {
        Ok(self.resolve(&Self::engine_sel(engine, name), Use::System, &Requester::System(format!("engine:{engine}")))?.value)
    }

    /// Rotate: a new value goes to `apply` first; only when the engine
    /// accepted it does the vault store it as the new version.
    pub fn engine_rotate(&self, engine: &str, name: &str, g: Gen, apply: impl FnOnce(&SecretBytes) -> std::result::Result<(), String>) -> Result<SecretMeta> {
        let new = generate(g);
        apply(&new).map_err(|e| VaultError::new("engine_rejected", 502, format!("{engine} didn't accept the new credential: {e}")))?;
        self.engine_put(engine, name, g, new, PutMode::Upsert)
    }

    fn engine_signing_key(&self, engine: &str, name: &str) -> Result<SigningKey> {
        self.engine_ensure(engine, name, Gen::Ed25519)?;
        let v = self.engine_value(engine, name)?;
        let seed: [u8; 32] = v
            .expose()
            .try_into()
            .map_err(|_| VaultError::tamper("engine signing key has the wrong length"))?;
        Ok(SigningKey::from_bytes(&seed))
    }

    pub fn engine_public_key(&self, engine: &str, name: &str) -> Result<[u8; 32]> {
        Ok(self.engine_signing_key(engine, name)?.verifying_key().to_bytes())
    }

    pub fn engine_sign(&self, engine: &str, name: &str, msg: &[u8]) -> Result<[u8; 64]> {
        Ok(self.engine_signing_key(engine, name)?.sign(msg).to_bytes())
    }
}

/// Check an engine signature against a published public key.
pub fn verify(public_key: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
    match VerifyingKey::from_bytes(public_key) {
        Ok(k) => k.verify(msg, &ed25519_dalek::Signature::from_bytes(sig)).is_ok(),
        Err(_) => false,
    }
}
