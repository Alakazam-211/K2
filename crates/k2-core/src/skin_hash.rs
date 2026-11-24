//! An App user's password hash: Argon2id behind the hash slot, a legacy
//! hash an import brought (`import::verify_legacy`), and the dummy target
//! a missing user's sign-in still verifies against. Moved out of skin.rs
//! as it was (0.46.18 queue, ratchet).

use super::*;

pub(super) fn hash_password(password: &str) -> Result<String, String> {
    let _slot = guard::hash_slot();
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("password hashing failed: {e}"))
}

pub(super) fn verify_hash(password: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return import::verify_legacy(password, hash),
    };
    let _slot = guard::hash_slot();
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub(super) fn build_dummy_hash() -> String {
    let _slot = guard::hash_slot();
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(b"k2-skin-dummy-verify-target", &salt)
        .map(|h| h.to_string())
        .unwrap_or_else(|_| {
            String::from(
                "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            )
        })
}
