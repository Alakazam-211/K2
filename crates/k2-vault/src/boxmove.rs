//! Box moves carry the vault (`.k2/prds/prd-box-move-carry-v1.md`).
//!
//! The data key (VDK) moves, never the box key (KEK):
//!
//! 1. The NEW box makes a one-time age key (the daemon holds it; this
//!    crate only uses it).
//! 2. The OLD box exports: it adds the age holder [`MOVE_HOLDER`] (the VDK
//!    wrapped to the move key), **freezes** itself (reads work, writes are
//!    refused with `vault_moved_out`), snapshots vault.db, and writes one
//!    `.k2mv` file: a signed manifest plus the snapshot, age-encrypted as a
//!    whole to the move key (BM-A15: names, hosts and audit rows are plain
//!    columns in vault.db).
//! 3. The NEW box checks the file (dry run: a throwaway adopt in a scratch
//!    folder), then adopts it for real: re-wraps the VDK to its own KEK,
//!    sets `gen = max(snapshot, anchor) + 1` (BM-A6) and drops the move
//!    holder.
//!
//! Nothing on the old box is wiped: the export only adds the holder and the
//! freeze marker, and `unfreeze` takes both away again.
//!
//! The manifest carries a `needs` list from its first version: the
//! features an importer must support. A K2 that meets a need it doesn't
//! know refuses the file with `move_file_needs_newer` instead of guessing.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::agefmt::{self, Identity, Recipient};
use crate::confirm::KdfCost;
use crate::error::{Result, VaultError};
use crate::keystore::{Anchor, KeyHome, KeyStore};
use crate::model::Scope;
use crate::policy::{Actor, HumanProof};
use crate::secret::Key32;
use crate::store::{Requester, SecretStore, Selector, Use};
use crate::vault::{LocalVault, OpenOptions, PlacementTarget};

/// The age holder the export adds (reserved: `add_wrap` refuses it).
pub const MOVE_HOLDER: &str = "move";
/// The outer format of a `.k2mv` file.
pub const FILE_FORMAT: &str = "k2-vault-move/1";
/// The freeze marker beside vault.db (survives a restart).
pub const FROZEN_MARKER: &str = "moved-out.json";
/// What a v1 file needs from its importer. Every name here is one this
/// build supports; a later format adds names, and an older K2 refuses
/// them by name (`move_file_needs_newer`).
pub const NEEDS_V1: &[&str] = &["vault-format:k2-vault/1", "holder:age-x25519", "snapshot:sqlite-vacuum"];
/// The needs this build can import.
pub const SUPPORTED_NEEDS: &[&str] = NEEDS_V1;
/// A move file is a whole vault.db (ciphertext values) plus a manifest.
pub const MAX_FILE_BYTES: usize = 512 * 1024 * 1024;

/// 423: the vault was exported for a box move.
pub fn moved_out() -> VaultError {
    VaultError::new(
        "vault_moved_out",
        423,
        "this vault was exported for a box move: it reads, but writes wait. The owner lifts it with k2 migrate export-secrets --unfreeze if the move is called off",
    )
}

/// `ab12-cd34-ef56-7890`: the short fingerprint both boxes print for the
/// move key, so the owner can see it's the same key.
pub fn fingerprint(r: &Recipient) -> String {
    let h = r.fingerprint();
    h.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap_or("")).collect::<Vec<_>>().join("-")
}

/// Where the source box was (copied into the manifest; informational).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MoveSource {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_subdomain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel_device_id: Option<String>,
}

/// The signed manifest (§3.2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MoveManifest {
    pub format: String,
    /// Features the importer must support (see [`NEEDS_V1`]).
    pub needs: Vec<String>,
    pub source_vault_id: String,
    /// The snapshot header's `gen`, `audit_seq` and `audit_mac` (hex).
    pub gen: u64,
    pub audit_seq: u64,
    pub audit_mac: String,
    /// sha256 of the snapshot bytes (hex).
    pub snapshot_sha256: String,
    pub snapshot_bytes: u64,
    /// Active `user` and `system` secrets in the snapshot.
    pub user_secrets: u64,
    pub system_secrets: u64,
    pub source: MoveSource,
    /// The source vault's key home (`keeper`, `keychain`, …).
    pub source_key_home: String,
    /// The move key's fingerprint ([`fingerprint`]).
    pub move_key: String,
    /// Hex Ed25519 public key the manifest is signed with: the source
    /// vault's carry-signing key, which also rides inside the snapshot (the
    /// import checks the two are the same key).
    pub source_public_key: String,
    pub created_at: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    format: String,
    /// The manifest JSON exactly as signed.
    manifest: String,
    sig: String,
    snapshot_b64: String,
}

/// What `export` hands back: the file bytes and the manifest in it.
pub struct MoveExport {
    pub file: Vec<u8>,
    pub manifest: MoveManifest,
}

/// The freeze marker's content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FrozenMarker {
    pub at: String,
    pub move_key: String,
    pub by: String,
}

/// A checked move file (decrypted, signature and hash good).
pub struct MoveFile {
    pub manifest: MoveManifest,
    pub snapshot: Vec<u8>,
}

impl Drop for MoveFile {
    fn drop(&mut self) {
        self.snapshot.zeroize();
    }
}

/// The import's dry-run answer: everything checked on a throwaway copy.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MoveCheck {
    pub vault_id: String,
    pub gen: u64,
    pub user_secrets: u64,
    pub system_secrets: u64,
    pub source: MoveSource,
    pub source_key_home: String,
    pub move_key: String,
    pub created_at: String,
    /// Per asked system secret (`engine:<id>/<name>`): present?
    pub system_present: Vec<(String, bool)>,
    /// Secrets placed as root units' systemd credentials on the old box
    /// (host-bound; re-place them here, BM-A11): `(secret name, unit)`.
    pub systemd_creds: Vec<(String, String)>,
}

impl LocalVault {
    /// Export this vault for a box move (§3.2): the owner, with a person's
    /// confirm (`proof`; it changes who can open the vault, VU-B1). Order:
    /// move holder → freeze → snapshot, so every write before the freeze is
    /// in the snapshot and none after.
    pub fn move_export(&self, to: &Recipient, source: MoveSource, actor: &Actor, proof: &HumanProof) -> Result<MoveExport> {
        if !actor.is_admin_human() {
            return Err(VaultError::forbidden("vault_admin_required", "a box move export is for the owner"));
        }
        if !matches!(proof, HumanProof::Confirmed { .. }) {
            return Err(VaultError::forbidden("vault_confirm_required", "a box move export needs the owner's confirm"));
        }
        if self.in_rollback() {
            return Err(VaultError::conflict(
                "vault_rollback",
                "this vault looks older than its key expects; the owner confirms that first (Settings → Vault), then export",
            ));
        }
        if self.key_store().rotation_pending()? {
            return Err(VaultError::conflict(
                "kek_rotation_pending",
                "a box key rotation is half done here (the root helper holds a staged next key); finish or abort it first (k2 vault doctor)",
            ));
        }
        let stamp = actor.stamp();
        // The signing key must exist before the freeze (making it is a write).
        let public_key = self.carry_public_key()?;
        let was_frozen = self.is_frozen();
        self.put_move_wrap(to, actor, proof, was_frozen)?;
        let marker = FrozenMarker { at: crate::vault::now_ts(), move_key: fingerprint(to), by: stamp.clone() };
        let marker_json = serde_json::to_vec_pretty(&marker).map_err(|_| VaultError::internal("marker json"))?;
        crate::fsutil::write_private_atomic(&self.dir().join(FROZEN_MARKER), &marker_json)?;
        self.set_frozen(true);
        match self.export_frozen(to, source, &public_key) {
            Ok(x) => {
                self.audit_carry("move-export", actor, &format!("move:{}", fingerprint(to)), &[]);
                Ok(x)
            }
            Err(e) => {
                if !was_frozen {
                    let _ = self.move_unfreeze_inner(&stamp);
                }
                Err(e)
            }
        }
    }

    fn export_frozen(&self, to: &Recipient, source: MoveSource, public_key: &str) -> Result<MoveExport> {
        let tmp = self.dir().join(format!("move-snapshot-{}.db", uuid::Uuid::new_v4().simple()));
        self.snapshot_to(&tmp)?;
        let read = (|| -> Result<(Vec<u8>, (String, u64, u64, String), (u64, u64))> {
            let head = LocalVault::peek_header(&tmp)?.ok_or_else(|| VaultError::internal("snapshot has no header"))?;
            let counts = count_active(&tmp)?;
            let bytes = std::fs::read(&tmp)?;
            Ok((bytes, head, counts))
        })();
        let _ = std::fs::remove_file(&tmp);
        let (mut snapshot, (vault_id, gen, audit_seq, audit_mac), (user, system)) = read?;
        let manifest = MoveManifest {
            format: FILE_FORMAT.into(),
            needs: NEEDS_V1.iter().map(|s| s.to_string()).collect(),
            source_vault_id: vault_id,
            gen,
            audit_seq,
            audit_mac,
            snapshot_sha256: crate::crypto::hex(&crate::crypto::sha256(&snapshot)),
            snapshot_bytes: snapshot.len() as u64,
            user_secrets: user,
            system_secrets: system,
            source,
            source_key_home: self.key_store().kind().as_str().into(),
            move_key: fingerprint(to),
            source_public_key: public_key.to_string(),
            created_at: crate::vault::now_ts(),
        };
        let m = serde_json::to_string(&manifest).map_err(|_| VaultError::internal("manifest json"))?;
        let sig = self.engine_sign(crate::carry::ENGINE, crate::carry::SIGNING, m.as_bytes())?;
        let env = Envelope { format: FILE_FORMAT.into(), manifest: m, sig: crate::crypto::hex(&sig), snapshot_b64: STANDARD.encode(&snapshot) };
        snapshot.zeroize();
        let mut body = serde_json::to_vec(&env).map_err(|_| VaultError::internal("move file json"))?;
        let file = agefmt::encrypt(&[*to], &body);
        body.zeroize();
        Ok(MoveExport { file, manifest })
    }

    /// Call the move off on the old box: lift the freeze and drop the move
    /// holder (§7). `Ok(false)` when the vault wasn't frozen.
    pub fn move_unfreeze(&self, actor: &Actor) -> Result<bool> {
        if !actor.is_admin_human() {
            return Err(VaultError::forbidden("vault_admin_required", "lifting a box move freeze is for the owner"));
        }
        let was = self.is_frozen();
        self.move_unfreeze_inner(&actor.stamp())?;
        if was {
            self.audit_carry("move-unfreeze", actor, "move", &[]);
        }
        Ok(was)
    }

    fn move_unfreeze_inner(&self, stamp: &str) -> Result<()> {
        let marker = self.dir().join(FROZEN_MARKER);
        if marker.exists() {
            std::fs::remove_file(&marker)?;
        }
        self.set_frozen(false);
        self.drop_move_wrap(stamp)?;
        Ok(())
    }

    /// The freeze marker, when this vault is frozen.
    pub fn frozen_marker(&self) -> Option<FrozenMarker> {
        if !self.is_frozen() {
            return None;
        }
        std::fs::read(self.dir().join(FROZEN_MARKER)).ok().and_then(|b| serde_json::from_slice(&b).ok())
    }

    /// After an import: drop the move holder (the one-time key is gone).
    pub fn move_drop_holder(&self) -> Result<bool> {
        self.drop_move_wrap("system:move")
    }

    /// Is the `system` secret `name` in scope `engine:<engine>` active?
    pub fn has_system_secret(&self, engine: &str, name: &str) -> bool {
        let sel = Selector::system(Scope::engine(engine), name);
        self.resolve(&sel, Use::System, &Requester::System("move".into())).is_ok()
    }

    /// Secrets placed as systemd credentials: `(secret name, unit)`.
    pub fn systemd_cred_placements(&self) -> Result<Vec<(String, String)>> {
        let rows = self.placements(None)?.rows;
        let mut out = vec![];
        for p in rows.into_iter().filter(|p| p.target == PlacementTarget::SystemdCred) {
            let name = self
                .meta(&Selector::Id(p.secret_id.clone()), &Actor::System("move".into()))
                .map(|m| m.name)
                .unwrap_or_else(|_| p.secret_id.clone());
            out.push((name, p.unit.clone().unwrap_or_default()));
        }
        Ok(out)
    }

    /// Active secrets per namespace (`user`, `system`), as status counts
    /// them (and as the export counts its snapshot).
    pub fn active_counts(&self) -> (u64, u64) {
        let c = self.status_inner().counts;
        (c.user, c.system)
    }
}

/// `(user, system)` active secrets in a vault.db, read without a key.
fn count_active(path: &Path) -> Result<(u64, u64)> {
    let conn = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let n = |ns: &str| -> Result<u64> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM secret WHERE namespace = ?1 AND status = 'active'",
            rusqlite::params![ns],
            |r| r.get::<_, i64>(0),
        )? as u64)
    };
    Ok((n("user")?, n("system")?))
}

fn bad(hint: impl Into<String>) -> VaultError {
    VaultError::bad_request("move_file_bad", hint)
}

/// Decrypt and check a `.k2mv` file: the move key opens it, its format and
/// `needs` are known, the manifest's signature verifies, and the snapshot's
/// sha256 is the manifest's. Nothing is written.
pub fn open_move_file(identity: &Identity, file: &[u8]) -> Result<MoveFile> {
    if file.len() > MAX_FILE_BYTES {
        return Err(bad("that file is larger than any move file"));
    }
    let raw = agefmt::decrypt(identity, file).map_err(|e| match e.code {
        "age_no_matching_identity" => VaultError::forbidden(
            "move_key_wrong",
            "this box's move key doesn't open that file: it was made for another move key (compare the fingerprints k2 migrate import-secrets begin and the export printed)",
        ),
        "vault_tamper" => bad("the file was changed after the export (it fails its encryption check); copy it again"),
        _ => bad("that isn't a K2 vault move file (.k2mv)"),
    })?;
    let env: Envelope = serde_json::from_slice(raw.expose()).map_err(|_| bad("not a K2 vault move file"))?;
    if env.format != FILE_FORMAT {
        return Err(VaultError::conflict(
            "move_file_needs_newer",
            format!("this move file is {}; this K2 reads {FILE_FORMAT}. Update K2 on this box, then import again", env.format),
        ));
    }
    // `needs` first, from the raw JSON: a newer file may carry fields this
    // build's typed manifest doesn't know.
    let loose: serde_json::Value = serde_json::from_str(&env.manifest).map_err(|_| bad("the manifest isn't JSON"))?;
    let needs: Vec<String> = loose
        .get("needs")
        .and_then(|v| v.as_array())
        .ok_or_else(|| bad("the manifest has no needs list"))?
        .iter()
        .map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad("a manifest need isn't text")))
        .collect::<Result<_>>()?;
    let unknown: Vec<&str> = needs.iter().map(String::as_str).filter(|n| !SUPPORTED_NEEDS.contains(n)).collect();
    if !unknown.is_empty() {
        return Err(VaultError::conflict(
            "move_file_needs_newer",
            format!("this move file needs features this K2 doesn't have ({}). Update K2 on this box, then import again", unknown.join(", ")),
        ));
    }
    let manifest: MoveManifest = serde_json::from_value(loose).map_err(|_| bad("the manifest is missing fields"))?;
    let pk: [u8; 32] = crate::crypto::unhex(&manifest.source_public_key)
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| bad("bad source key in the manifest"))?;
    let sig: [u8; 64] = crate::crypto::unhex(&env.sig).and_then(|v| v.try_into().ok()).ok_or_else(|| bad("bad signature"))?;
    if !crate::engine::verify(&pk, env.manifest.as_bytes(), &sig) {
        return Err(bad("the manifest's signature doesn't verify (the file was changed)"));
    }
    let snapshot = STANDARD.decode(&env.snapshot_b64).map_err(|_| bad("the snapshot isn't base64"))?;
    if snapshot.len() as u64 != manifest.snapshot_bytes
        || crate::crypto::hex(&crate::crypto::sha256(&snapshot)) != manifest.snapshot_sha256
    {
        return Err(bad("the snapshot isn't the one the manifest names (sha256 differs)"));
    }
    Ok(MoveFile { manifest, snapshot })
}

/// Write the snapshot of `mf` as `dir/vault.db` (0600), and check that its
/// header is the manifest's (`gen`, audit head, vault id).
pub fn place_snapshot(mf: &MoveFile, dir: &Path) -> Result<()> {
    crate::fsutil::ensure_private_dir(dir)?;
    let path = dir.join("vault.db");
    crate::fsutil::write_private_atomic(&path, &mf.snapshot)?;
    let m = &mf.manifest;
    match LocalVault::peek_header(&path)? {
        Some((id, gen, seq, mac)) if id == m.source_vault_id && gen == m.gen && seq == m.audit_seq && mac == m.audit_mac => Ok(()),
        Some(_) => Err(bad("the snapshot's header isn't the one the manifest names")),
        None => Err(bad("the snapshot has no vault header")),
    }
}

/// The dry run (§3.2): place the snapshot in `scratch` and adopt it there
/// with a throwaway key, exactly as the real import would; check the move
/// holder opens, the audit chain and every row verify, the counts match
/// the manifest, and the snapshot holds the signing key the manifest was
/// signed with. `system` lists `(engine, name)` secrets to report on.
pub fn check_move(mf: &MoveFile, identity: &Identity, scratch: &Path, kdf: KdfCost, system: &[(&str, &str)]) -> Result<MoveCheck> {
    place_snapshot(mf, scratch)?;
    LocalVault::check_identity(scratch, identity, MOVE_HOLDER).map_err(|e| match e.code {
        "no_holder" => bad("the snapshot has no move holder"),
        "age_no_matching_identity" => {
            VaultError::forbidden("move_key_wrong", "the move holder in the snapshot doesn't open with this box's move key")
        }
        _ => e,
    })?;
    let ks: Arc<dyn KeyStore> = Arc::new(ScratchKeyStore::default());
    let v = LocalVault::adopt_with_identity(scratch, ks, identity, MOVE_HOLDER, OpenOptions { kdf })?;
    let r = verify_adopted(&v, &mf.manifest);
    let report = r.map(|()| {
        let (user, sys) = v.active_counts();
        MoveCheck {
            vault_id: v.vault_id().to_string(),
            gen: mf.manifest.gen,
            user_secrets: user,
            system_secrets: sys,
            source: mf.manifest.source.clone(),
            source_key_home: mf.manifest.source_key_home.clone(),
            move_key: mf.manifest.move_key.clone(),
            created_at: mf.manifest.created_at.clone(),
            system_present: system.iter().map(|(e, n)| (format!("engine:{e}/{n}"), v.has_system_secret(e, n))).collect(),
            systemd_creds: v.systemd_cred_placements().unwrap_or_default(),
        }
    });
    drop(v);
    r_cleanup(scratch);
    report
}

fn r_cleanup(scratch: &Path) {
    for f in ["vault.db", "vault.db-wal", "vault.db-shm"] {
        let _ = std::fs::remove_file(scratch.join(f));
    }
}

/// What an adopted move must look like: no tamper, no rollback, the
/// counts of the manifest, and the manifest's signing key inside.
pub fn verify_adopted(v: &LocalVault, m: &MoveManifest) -> Result<()> {
    let tamper = v.tamper_notes();
    if !tamper.is_empty() {
        return Err(VaultError::tamper(format!("the moved vault failed a check: {}", tamper.join("; "))));
    }
    if v.in_rollback() {
        return Err(VaultError::conflict("move_check_failed", "the moved vault opened in rollback"));
    }
    if v.vault_id() != m.source_vault_id {
        return Err(bad("the moved vault isn't the one the manifest names"));
    }
    let (user, system) = v.active_counts();
    if (user, system) != (m.user_secrets, m.system_secrets) {
        return Err(VaultError::conflict(
            "move_check_failed",
            format!(
                "the moved vault holds {user} yours / {system} K2 system secrets; the manifest says {} / {}",
                m.user_secrets, m.system_secrets
            ),
        ));
    }
    if v.carry_public_key()? != m.source_public_key {
        return Err(bad("the manifest was signed by a key that isn't this vault's"));
    }
    Ok(())
}

/// The import's stale check (§4): is the vault.db already at `path` the
/// same vault, NEWER than the snapshot (the old box wrote after the
/// export)? Then the export is stale: `move_stale`.
pub fn refuse_stale(path: &Path, m: &MoveManifest) -> Result<()> {
    if let Some((id, gen, _, _)) = LocalVault::peek_header(path)? {
        if id == m.source_vault_id && gen > m.gen {
            return Err(VaultError::conflict(
                "move_stale",
                format!(
                    "the vault.db on this box is newer than the move file (gen {gen} > {}): the old box wrote after the export. Export again on the old box, then import the new file",
                    m.gen
                ),
            ));
        }
    }
    Ok(())
}

/// The files an import moves aside (and an undo puts back).
const VAULT_FILES: &[&str] = &["vault.db", "vault.db-wal", "vault.db-shm", FROZEN_MARKER];
/// What the last import on this box replaced (for `--undo`).
pub const LAST_IMPORT: &str = "last-import.json";

/// The record `--undo` reads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportRecord {
    /// `replaced-<ts>` beside vault.db.
    pub aside: String,
    pub at: String,
    pub vault_id: String,
    /// `gen` after the import (the snapshot's, moved past this box's anchor).
    pub gen: u64,
    /// This box's anchor before the import, put back by an undo.
    #[serde(default)]
    pub previous_anchor: Option<Anchor>,
    pub move_key: String,
}

/// A finished import.
pub struct Imported {
    pub vault: LocalVault,
    pub record: ImportRecord,
}

fn move_files(from: &Path, to: &Path) -> Result<()> {
    crate::fsutil::ensure_private_dir(to)?;
    for f in VAULT_FILES {
        let p = from.join(f);
        if p.exists() {
            std::fs::rename(&p, to.join(f))?;
        }
    }
    Ok(())
}

fn remove_files(dir: &Path) {
    for f in VAULT_FILES {
        let _ = std::fs::remove_file(dir.join(f));
    }
}

fn stamp_now() -> String {
    chrono::Utc::now().format("%Y%m%d%H%M%S%3f").to_string()
}

/// The import with `--apply` (§3.2), on the vault folder `dir` whether
/// this box's vault is open, closed or missing its key (BM-A5): stale
/// check, move the current files aside to `replaced-<ts>/`, adopt the
/// snapshot with the move key (re-wrapped to `ks`, `gen = max(snapshot,
/// anchor) + 1`), drop the move holder, and check the result. Any failure
/// after the move-aside puts the files (and the anchor) back by itself.
/// The caller has closed its own handle on the old vault (or never had
/// one).
pub fn apply_move(dir: &Path, ks: Arc<dyn KeyStore>, identity: &Identity, mf: &MoveFile, opts: OpenOptions) -> Result<Imported> {
    refuse_stale(&dir.join("vault.db"), &mf.manifest)?;
    let previous_anchor = ks.anchor().ok().flatten();
    let aside_name = format!("replaced-{}", stamp_now());
    let aside = dir.join(&aside_name);
    move_files(dir, &aside)?;
    let undo = |e: VaultError| -> VaultError {
        remove_files(dir);
        let _ = move_files(&aside, dir);
        let _ = std::fs::remove_dir(&aside);
        if let Some(a) = &previous_anchor {
            let _ = ks.set_anchor(a);
        }
        e
    };
    if let Err(e) = place_snapshot(mf, dir) {
        return Err(undo(e));
    }
    let v = match LocalVault::adopt_with_identity(dir, ks.clone(), identity, MOVE_HOLDER, opts) {
        Ok(v) => v,
        Err(e) => return Err(undo(e)),
    };
    let checked = v.move_drop_holder().and_then(|_| verify_adopted(&v, &mf.manifest)).and_then(|_| v.head());
    let gen = match checked {
        Ok((gen, _, _)) => gen,
        Err(e) => {
            drop(v);
            return Err(undo(e));
        }
    };
    let record = ImportRecord {
        aside: aside_name,
        at: crate::vault::now_ts(),
        vault_id: v.vault_id().to_string(),
        gen,
        previous_anchor,
        move_key: mf.manifest.move_key.clone(),
    };
    let rec = serde_json::to_vec_pretty(&record).map_err(|_| VaultError::internal("import record json"))?;
    let move_dir = dir.join("move");
    crate::fsutil::ensure_private_dir(&move_dir)?;
    crate::fsutil::write_private_atomic(&move_dir.join(LAST_IMPORT), &rec)?;
    Ok(Imported { vault: v, record })
}

/// The last import's record, if this box has one to undo.
pub fn last_import(dir: &Path) -> Option<ImportRecord> {
    std::fs::read(dir.join("move").join(LAST_IMPORT)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// `--undo` on the new box (§7): the imported vault goes to
/// `undone-<ts>/` (kept, never deleted), the replaced files come back, and
/// so does the anchor (stores that only count up keep the higher `gen`; the
/// caller acknowledges that rollback with the undo's own confirm). Returns
/// the record it undid. The caller has closed its handle on the vault and
/// opens the folder again.
pub fn undo_import(dir: &Path, ks: &Arc<dyn KeyStore>) -> Result<ImportRecord> {
    let rec = last_import(dir)
        .ok_or_else(|| VaultError::conflict("no_import", "no import to undo on this box (k2 migrate status)"))?;
    let aside = dir.join(&rec.aside);
    if !aside.is_dir() {
        return Err(VaultError::conflict("no_import", format!("{} is gone; nothing to put back", rec.aside)));
    }
    if let Some((id, _, _, _)) = LocalVault::peek_header(&dir.join("vault.db"))? {
        if id != rec.vault_id {
            return Err(VaultError::conflict(
                "import_replaced",
                "the vault here isn't the one the last import made (it was replaced since); undo refused",
            ));
        }
    }
    move_files(dir, &dir.join(format!("undone-{}", stamp_now())))?;
    move_files(&aside, dir)?;
    let _ = std::fs::remove_dir(&aside);
    if let Some(a) = &rec.previous_anchor {
        ks.set_anchor(a)?;
    }
    std::fs::remove_file(dir.join("move").join(LAST_IMPORT))?;
    Ok(rec)
}

/// The scratch folder for a dry run, beside vault.db.
pub fn scratch_dir(vault_dir: &Path) -> PathBuf {
    vault_dir.join("move").join(format!("check-{}", uuid::Uuid::new_v4().simple()))
}

/// An in-memory key store for the dry run's throwaway adopt. Never a box
/// key: it lives as long as the check.
#[derive(Default)]
pub struct ScratchKeyStore {
    kek: Mutex<Option<[u8; 32]>>,
    anchor: Mutex<Option<Anchor>>,
}

impl KeyStore for ScratchKeyStore {
    fn kind(&self) -> KeyHome {
        KeyHome::Fake
    }
    fn load(&self) -> Result<Option<Key32>> {
        Ok(self.kek.lock().as_ref().map(Key32::from_bytes))
    }
    fn init(&self) -> Result<Key32> {
        let mut g = self.kek.lock();
        if g.is_some() {
            return Err(VaultError::conflict("kek_exists", "a box key already exists"));
        }
        let k = Key32::random();
        *g = Some(*k.as_bytes());
        Ok(k)
    }
    fn supports_anchor(&self) -> bool {
        true
    }
    fn anchor(&self) -> Result<Option<Anchor>> {
        Ok(self.anchor.lock().clone())
    }
    fn set_anchor(&self, a: &Anchor) -> Result<()> {
        *self.anchor.lock() = Some(a.clone());
        Ok(())
    }
    fn stage_next(&self) -> Result<Key32> {
        Err(VaultError::forbidden("scratch_key", "a dry-run key store never rotates"))
    }
    fn commit_next(&self) -> Result<()> {
        Err(VaultError::forbidden("scratch_key", "a dry-run key store never rotates"))
    }
    fn abort_next(&self) -> Result<()> {
        Ok(())
    }
    fn remove(&self) -> Result<()> {
        *self.kek.lock() = None;
        Ok(())
    }
}

#[cfg(test)]
#[path = "boxmove_tests.rs"]
mod tests;
