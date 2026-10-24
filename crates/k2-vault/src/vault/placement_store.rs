//! vault.db side of placements (`prd-vault-gaps-v1` item 4): where a
//! secret's value lives outside the vault.
//!
//! - `systemd_cred`: K2 wrote it through the root helper
//!   (`k2 vault systemd-cred`); a rotation re-puts it, and a row whose
//!   `version` is behind the secret's is stale (`placement_stale`).
//! - `manual`: a person or agent noted a place K2 doesn't write (another
//!   box, Vercel prod, a box without K2). After a rotation it is due
//!   (`placement_manual_due`) until `k2 vault placement done` marks it
//!   re-placed at the new version.
//!
//! No value is ever stored here, only names and places. Rows carry a MAC
//! under the row key; a row that fails it is skipped and noted (tamper),
//! and a row whose `target` this build doesn't know is skipped and counted
//! (`unknown`), never an error for the whole list (R2).

use super::*;

/// Longest `--where` text.
pub const WHERE_MAX: usize = 200;
/// Longest `--note` text.
pub const NOTE_MAX: usize = 500;
/// Most placement rows one secret may have.
pub const PER_SECRET_MAX: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementTarget {
    SystemdCred,
    Manual,
}

impl PlacementTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            PlacementTarget::SystemdCred => "systemd_cred",
            PlacementTarget::Manual => "manual",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "systemd_cred" => Some(PlacementTarget::SystemdCred),
            "manual" => Some(PlacementTarget::Manual),
            _ => None,
        }
    }
}

/// One placement row.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub id: String,
    pub secret_id: String,
    /// The secret version placed there (current when added; moved on by a
    /// re-put or `placement done`).
    pub version: u32,
    pub target: PlacementTarget,
    pub unit: Option<String>,
    pub cred_name: Option<String>,
    #[serde(rename = "where")]
    pub where_text: Option<String>,
    pub note: String,
    pub restart: bool,
    pub placed_by: String,
    pub placed_at: String,
    pub written_at: Option<String>,
}

/// What a caller asks to record.
#[derive(Debug, Clone)]
pub struct NewPlacement {
    pub secret_id: String,
    pub target: PlacementTarget,
    pub unit: Option<String>,
    pub cred_name: Option<String>,
    pub where_text: Option<String>,
    pub note: String,
    pub restart: bool,
}

/// A list read: the rows this build understands, and how many it skipped
/// because their `target` is newer than this build.
#[derive(Debug, Clone, Default)]
pub struct Placements {
    pub rows: Vec<Placement>,
    pub unknown: usize,
}

/// A place or note: bounded, no control characters (no newline can forge
/// a second line in the doctor or the audit).
fn text_ok(s: &str, max: usize) -> bool {
    s.chars().count() <= max && !s.chars().any(char::is_control)
}

fn mac_input(p: &Placement, target: &str) -> Vec<u8> {
    let mut e = Enc::new("placement");
    e.str(&p.id)
        .str(&p.secret_id)
        .i64(i64::from(p.version))
        .str(target)
        .opt_str(p.unit.as_deref())
        .opt_str(p.cred_name.as_deref())
        .opt_str(p.where_text.as_deref())
        .str(&p.note)
        .i64(i64::from(p.restart))
        .str(&p.placed_by)
        .str(&p.placed_at)
        .opt_str(p.written_at.as_deref());
    e.finish().to_vec()
}

/// (row or None for an unknown target, target text, mac).
type RawPlacement = (Option<Placement>, String, Vec<u8>, Placement);

fn read_raw(r: &Row<'_>) -> rusqlite::Result<RawPlacement> {
    let target: String = r.get("target")?;
    let p = Placement {
        id: r.get("id")?,
        secret_id: r.get("secret_id")?,
        version: r.get::<_, i64>("version")?.clamp(0, i64::from(u32::MAX)) as u32,
        target: PlacementTarget::Manual,
        unit: r.get("unit")?,
        cred_name: r.get("cred_name")?,
        where_text: r.get("where_text")?,
        note: r.get("note")?,
        restart: r.get::<_, i64>("restart")? != 0,
        placed_by: r.get("placed_by")?,
        placed_at: r.get("placed_at")?,
        written_at: r.get("written_at")?,
    };
    let mac: Vec<u8> = r.get("row_mac")?;
    let known = PlacementTarget::parse(&target).map(|t| Placement { target: t, ..p.clone() });
    Ok((known, target, mac, p))
}

impl LocalVault {
    fn placement_write(&self, tx: &Transaction<'_>, keys: &Keys, p: &Placement, insert: bool) -> Result<()> {
        let mac = hmac(&keys.row, &mac_input(p, p.target.as_str()));
        if insert {
            tx.execute(
                "INSERT INTO placement (id, secret_id, version, target, unit, cred_name, where_text, note, restart,
                    placed_by, placed_at, written_at, row_mac)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![
                    p.id, p.secret_id, i64::from(p.version), p.target.as_str(), p.unit, p.cred_name, p.where_text,
                    p.note, i64::from(p.restart), p.placed_by, p.placed_at, p.written_at, mac.to_vec()
                ],
            )?;
        } else {
            let n = tx.execute(
                "UPDATE placement SET version = ?2, restart = ?3, written_at = ?4, row_mac = ?5 WHERE id = ?1",
                params![p.id, i64::from(p.version), i64::from(p.restart), p.written_at, mac.to_vec()],
            )?;
            if n != 1 {
                return Err(VaultError::not_found("no such placement"));
            }
        }
        Ok(())
    }

    fn placements_where(&self, conn: &Connection, keys: &Keys, secret_id: Option<&str>) -> Result<Placements> {
        let mut st = conn.prepare(
            "SELECT * FROM placement WHERE (?1 IS NULL OR secret_id = ?1) ORDER BY placed_at, id LIMIT 5000",
        )?;
        let it = st.query_map(params![secret_id], read_raw)?;
        let mut out = Placements::default();
        for row in it {
            let (known, target, mac, raw) = row?;
            if !hmac_verify(&keys.row, &mac_input(&raw, &target), &mac) {
                self.tamper_once(format!("placement {} failed its integrity check", raw.id));
                continue;
            }
            match known {
                Some(p) => out.rows.push(p),
                None => out.unknown += 1,
            }
        }
        Ok(out)
    }

    /// Rekey: verify every placement row under the old row key and re-MAC
    /// it under the new one; any failure stops the rekey.
    pub(super) fn rekey_placements(&self, tx: &Transaction<'_>, old: &Keys, new: &Keys) -> Result<()> {
        let rows: Vec<RawPlacement> = {
            let mut st = tx.prepare("SELECT * FROM placement")?;
            let it = st.query_map([], read_raw)?;
            it.collect::<std::result::Result<_, _>>()?
        };
        for (_, target, mac, raw) in rows {
            if !hmac_verify(&old.row, &mac_input(&raw, &target), &mac) {
                return Err(VaultError::tamper(format!("placement {} failed its integrity check; rekey stopped", raw.id)));
            }
            let m2 = hmac(&new.row, &mac_input(&raw, &target));
            tx.execute("UPDATE placement SET row_mac = ?2 WHERE id = ?1", params![raw.id, m2.to_vec()])?;
        }
        Ok(())
    }

    /// Every placement (or one secret's), oldest first.
    pub fn placements(&self, secret_id: Option<&str>) -> Result<Placements> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        self.placements_where(&conn, &keys, secret_id)
    }

    /// One placement by id.
    pub fn placement_get(&self, id: &str) -> Result<Placement> {
        self.placements(None)?
            .rows
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| VaultError::not_found("no such placement"))
    }

    /// Record a placement at the secret's current version. `actor` is the
    /// stamp of who placed it (policy is the caller's: the daemon checks
    /// who may add which target before calling).
    pub fn placement_add(&self, n: NewPlacement, actor: &str) -> Result<Placement> {
        if !text_ok(&n.note, NOTE_MAX) {
            return Err(VaultError::bad_request("bad_note", format!("--note is at most {NOTE_MAX} characters, on one line")));
        }
        match n.target {
            PlacementTarget::Manual => {
                let w = n.where_text.as_deref().map(str::trim).unwrap_or("");
                if w.is_empty() || !text_ok(w, WHERE_MAX) {
                    return Err(VaultError::bad_request(
                        "bad_where",
                        format!("--where <host>:<path> is required, at most {WHERE_MAX} characters, on one line"),
                    ));
                }
                if n.unit.is_some() || n.cred_name.is_some() || n.restart {
                    return Err(VaultError::bad_request("bad_placement", "a manual placement names a place, not a unit"));
                }
            }
            PlacementTarget::SystemdCred => {
                let ok = |s: &Option<String>| s.as_deref().is_some_and(|x| !x.is_empty() && text_ok(x, 128));
                if !ok(&n.unit) || !ok(&n.cred_name) {
                    return Err(VaultError::bad_request("bad_placement", "a systemd credential placement needs a unit and a cred name"));
                }
            }
        }
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let row = self.find_row(&conn, &keys, &Selector::Id(n.secret_id.clone()))?;
        let existing = self.placements_where(&conn, &keys, Some(&n.secret_id))?;
        drop(conn);
        drop(keys);
        if row.meta.status == SecretStatus::Destroyed {
            return Err(VaultError::forbidden("secret_not_active", "that secret is destroyed"));
        }
        if existing.rows.len() + existing.unknown >= PER_SECRET_MAX {
            return Err(VaultError::conflict("too_many_placements", format!("a secret has at most {PER_SECRET_MAX} placements")));
        }
        let at = ts(now());
        let p = Placement {
            id: uuid::Uuid::new_v4().to_string(),
            secret_id: n.secret_id.clone(),
            version: row.meta.latest_version,
            target: n.target,
            unit: n.unit.clone(),
            cred_name: n.cred_name.clone(),
            where_text: n.where_text.as_deref().map(|w| w.trim().to_string()),
            note: n.note.clone(),
            restart: n.restart,
            placed_by: actor.to_string(),
            placed_at: at.clone(),
            written_at: (n.target == PlacementTarget::SystemdCred).then(|| at.clone()),
        };
        let label = p.unit.clone().or_else(|| p.where_text.clone()).unwrap_or_default();
        let e = Entry::new("place", actor, &format!("ok:add:{}", p.target.as_str()))
            .secret(&p.id_secret(), Some(p.version))
            .consumer(label);
        let p2 = p.clone();
        self.mutate(false, |tx, keys| {
            self.placement_write(tx, keys, &p2, true)?;
            Ok(((), vec![e]))
        })?;
        Ok(p)
    }

    /// A systemd credential K2 just wrote at `version`: update its row
    /// (same secret, unit and cred name) or add one.
    pub fn placement_record_cred(
        &self,
        secret_id: &str,
        unit: &str,
        cred_name: &str,
        restart: bool,
        version: u32,
        actor: &str,
    ) -> Result<Placement> {
        let found = self.placements(Some(secret_id))?.rows.into_iter().find(|p| {
            p.target == PlacementTarget::SystemdCred && p.unit.as_deref() == Some(unit) && p.cred_name.as_deref() == Some(cred_name)
        });
        let Some(mut p) = found else {
            let mut p = self.placement_add(
                NewPlacement {
                    secret_id: secret_id.to_string(),
                    target: PlacementTarget::SystemdCred,
                    unit: Some(unit.to_string()),
                    cred_name: Some(cred_name.to_string()),
                    where_text: None,
                    note: String::new(),
                    restart,
                },
                actor,
            )?;
            if p.version != version {
                p = self.placement_mark(&p.id, version, restart, actor)?;
            }
            return Ok(p);
        };
        p.version = version;
        p.restart = restart;
        p.written_at = Some(ts(now()));
        let e = Entry::new("place", actor, "ok:write:systemd_cred").secret(&p.secret_id, Some(version)).consumer(unit.to_string());
        let p2 = p.clone();
        self.mutate(false, |tx, keys| {
            self.placement_write(tx, keys, &p2, false)?;
            Ok(((), vec![e]))
        })?;
        Ok(p)
    }

    /// Mark a placement as holding `version` now (a manual row re-placed by
    /// hand, or a systemd credential re-put after a rotation).
    pub fn placement_mark(&self, id: &str, version: u32, restart: bool, actor: &str) -> Result<Placement> {
        let mut p = self.placement_get(id)?;
        p.version = version;
        p.restart = restart;
        p.written_at = Some(ts(now()));
        let label = p.unit.clone().or_else(|| p.where_text.clone()).unwrap_or_default();
        let e = Entry::new("place", actor, &format!("ok:done:{}", p.target.as_str())).secret(&p.secret_id, Some(version)).consumer(label);
        let p2 = p.clone();
        self.mutate(false, |tx, keys| {
            self.placement_write(tx, keys, &p2, false)?;
            Ok(((), vec![e]))
        })?;
        Ok(p)
    }

    /// Drop a placement row (the place itself is the caller's business).
    pub fn placement_remove(&self, id: &str, actor: &str) -> Result<Placement> {
        let p = self.placement_get(id)?;
        let label = p.unit.clone().or_else(|| p.where_text.clone()).unwrap_or_default();
        let e = Entry::new("place", actor, &format!("ok:remove:{}", p.target.as_str())).secret(&p.secret_id, Some(p.version)).consumer(label);
        let id2 = p.id.clone();
        self.mutate(false, |tx, _keys| {
            let n = tx.execute("DELETE FROM placement WHERE id = ?1", params![id2])?;
            if n != 1 {
                return Err(VaultError::not_found("no such placement"));
            }
            Ok(((), vec![e]))
        })?;
        Ok(p)
    }
}

impl LocalVault {
    /// Who wrote one version of a secret and when (`created_by` stamp,
    /// RFC 3339), MAC-checked. For the systemd-cred confirm: a person sees
    /// who wrote the value they are about to put into a root unit (review
    /// A2-N2 / B2-N1).
    pub fn version_author(&self, secret_id: &str, version: u32) -> Result<(String, String)> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        type Row8 = (Vec<u8>, Vec<u8>, String, String, String, String, Option<String>, Vec<u8>);
        let row: Option<Row8> = conn
            .query_row(
                "SELECT nonce, ciphertext, fingerprint, created_at, created_by, state, revoked_at, row_mac FROM secret_version WHERE secret_id = ?1 AND version = ?2",
                params![secret_id, version],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
            )
            .optional()?;
        let (nonce, ct, fp, cat, cby, state, rev, mac) = row.ok_or_else(|| VaultError::not_found("that version doesn't exist"))?;
        if !hmac_verify(&keys.row, &version_mac_input(secret_id, version, &nonce, &ct, &fp, &cat, &cby, &state, rev.as_deref()), &mac) {
            self.tamper_once(format!("secret {secret_id} v{version} failed its integrity check"));
            return Err(VaultError::tamper("this version failed its integrity check"));
        }
        Ok((cby, cat))
    }
}

impl Placement {
    fn id_secret(&self) -> String {
        self.secret_id.clone()
    }

    /// Behind the secret's latest version: stale (systemd_cred) or due
    /// (manual).
    pub fn behind(&self, latest: u32) -> bool {
        self.version < latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Kind, Namespace, Scope};
    use crate::policy::Actor;
    use crate::secret::SecretBytes;

    fn vault() -> (crate::fsutil::TestDir, LocalVault) {
        let d = crate::fsutil::TestDir::new("placement");
        let ks: Arc<dyn KeyStore> = Arc::new(crate::keystore::fake::FakeKeyStore::new());
        let v = LocalVault::open(d.path().join("vault"), ks, OpenOptions { kdf: KdfCost::Floor }).unwrap();
        (d, v)
    }

    fn secret(v: &LocalVault, name: &str) -> String {
        let req = PutRequest::new(Namespace::User, Scope::workspace("ws1"), name, Kind::ApiKey);
        v.put(req, SecretBytes::new(b"k2test-one".to_vec()), &Actor::Owner).unwrap().meta.id
    }

    #[test]
    fn manual_rows_go_due_after_a_rotate_and_done_clears_them() {
        let (_d, v) = vault();
        let sid = secret(&v, "INGEST_KEY");
        let p = v
            .placement_add(
                NewPlacement {
                    secret_id: sid.clone(),
                    target: PlacementTarget::Manual,
                    unit: None,
                    cred_name: None,
                    where_text: Some("vercel:prod/k2-web".into()),
                    note: "Vercel prod env".into(),
                    restart: false,
                },
                "owner",
            )
            .unwrap();
        assert_eq!(p.version, 1);
        assert!(!p.behind(1));
        v.rotate(&Selector::Id(sid.clone()), SecretBytes::new(b"k2test-two".to_vec()), &Actor::Owner).unwrap();
        let rows = v.placements(Some(&sid)).unwrap();
        assert_eq!(rows.rows.len(), 1);
        assert!(rows.rows[0].behind(2), "due after the rotate");
        let done = v.placement_mark(&p.id, 2, false, "owner").unwrap();
        assert_eq!(done.version, 2);
        assert!(!v.placement_get(&p.id).unwrap().behind(2));
        v.placement_remove(&p.id, "owner").unwrap();
        assert!(v.placements(Some(&sid)).unwrap().rows.is_empty());
        let audit = v.audit(&AuditQuery { secret_id: Some(sid.clone()), ..Default::default() }, &Actor::Owner).unwrap();
        let outcomes: Vec<&str> = audit.iter().filter(|a| a.action == "place").map(|a| a.outcome.as_str()).collect();
        assert!(outcomes.contains(&"ok:add:manual") && outcomes.contains(&"ok:done:manual") && outcomes.contains(&"ok:remove:manual"), "{outcomes:?}");
    }

    #[test]
    fn version_author_names_who_wrote_each_version() {
        let (_d, v) = vault();
        let sid = secret(&v, "K");
        assert_eq!(v.version_author(&sid, 1).unwrap().0, "owner");
        let agent = Actor::Agent(crate::policy::AgentActor { workspace_id: "ws1".into(), session_id: Some("s9".into()), shell: false, vault_manage: false });
        v.rotate(&Selector::Id(sid.clone()), SecretBytes::new(b"k2test-two".to_vec()), &agent).unwrap();
        assert_eq!(v.version_author(&sid, 2).unwrap().0, "agent:ws1/s9");
        assert_eq!(v.version_author(&sid, 3).unwrap_err().code, "not_found");
        {
            let conn = v.conn.lock();
            conn.execute("UPDATE secret_version SET created_by = 'owner' WHERE secret_id = ?1 AND version = 2", params![sid]).unwrap();
        }
        assert_eq!(v.version_author(&sid, 2).unwrap_err().code, "vault_tamper", "a forged author fails its MAC");
    }

    /// Review A3-P3: a rollback to an agent's version keeps that author.
    #[test]
    fn a_rollback_to_an_agent_version_keeps_its_author() {
        let (_d, v) = vault();
        let sid = secret(&v, "K");
        let agent = Actor::Agent(crate::policy::AgentActor { workspace_id: "ws1".into(), session_id: Some("s9".into()), shell: false, vault_manage: false });
        v.rotate(&Selector::Id(sid.clone()), SecretBytes::new(b"k2test-agent".to_vec()), &agent).unwrap();
        v.rotate(&Selector::Id(sid.clone()), SecretBytes::new(b"k2test-owner".to_vec()), &Actor::Owner).unwrap();
        v.rollback_to(&Selector::Id(sid.clone()), 2, &Actor::Owner).unwrap();
        assert_eq!(v.version_author(&sid, 4).unwrap().0, "owner (rollback of v2 by agent:ws1/s9)");
        v.rollback_to(&Selector::Id(sid.clone()), 1, &Actor::Owner).unwrap();
        assert_eq!(v.version_author(&sid, 5).unwrap().0, "owner", "a person's version stays the person's");
    }

    #[test]
    fn text_rules_and_target_shapes() {
        let (_d, v) = vault();
        let sid = secret(&v, "K");
        let m = |w: Option<&str>, note: &str| NewPlacement {
            secret_id: sid.clone(),
            target: PlacementTarget::Manual,
            unit: None,
            cred_name: None,
            where_text: w.map(String::from),
            note: note.into(),
            restart: false,
        };
        assert_eq!(v.placement_add(m(None, ""), "owner").unwrap_err().code, "bad_where");
        assert_eq!(v.placement_add(m(Some("  "), ""), "owner").unwrap_err().code, "bad_where");
        assert_eq!(v.placement_add(m(Some("a\nb"), ""), "owner").unwrap_err().code, "bad_where");
        assert_eq!(v.placement_add(m(Some(&"x".repeat(201)), ""), "owner").unwrap_err().code, "bad_where");
        assert_eq!(v.placement_add(m(Some("box:/etc/x"), &"n".repeat(501)), "owner").unwrap_err().code, "bad_note");
        assert_eq!(v.placement_add(m(Some("box:/etc/x"), "a\u{7}"), "owner").unwrap_err().code, "bad_note");
        assert!(v.placement_add(m(Some(&"x".repeat(200)), &"n".repeat(500)), "owner").is_ok());
        let mut bad = m(Some("box:/x"), "");
        bad.unit = Some("k2-watch.service".into());
        assert_eq!(v.placement_add(bad, "owner").unwrap_err().code, "bad_placement");
        let cred = NewPlacement {
            secret_id: sid.clone(),
            target: PlacementTarget::SystemdCred,
            unit: None,
            cred_name: Some("k".into()),
            where_text: None,
            note: String::new(),
            restart: false,
        };
        assert_eq!(v.placement_add(cred, "owner").unwrap_err().code, "bad_placement");
        let r = v.placement_record_cred(&sid, "k2-watch.service", "ingest_key", true, 1, "owner").unwrap();
        assert_eq!((r.target, r.version, r.restart), (PlacementTarget::SystemdCred, 1, true));
        assert!(r.written_at.is_some());
        let again = v.placement_record_cred(&sid, "k2-watch.service", "ingest_key", false, 1, "system:systemd-cred").unwrap();
        assert_eq!(again.id, r.id, "same unit + cred: one row");
        assert!(!again.restart);
        assert_eq!(v.placements(Some(&sid)).unwrap().rows.len(), 2);
    }

    #[test]
    fn a_tampered_row_is_skipped_and_an_unknown_target_is_counted() {
        let (_d, v) = vault();
        let sid = secret(&v, "K");
        let p = v.placement_record_cred(&sid, "k2-watch.service", "k", false, 1, "owner").unwrap();
        {
            let conn = v.conn.lock();
            conn.execute("UPDATE placement SET unit = 'sshd.service' WHERE id = ?1", params![p.id]).unwrap();
        }
        let l = v.placements(None).unwrap();
        assert!(l.rows.is_empty(), "a row that fails its MAC is never used");
        assert!(v.tamper_notes().iter().any(|n| n.contains("placement")));
        // A newer K2's target: counted, not an error.
        let q = Placement {
            id: "p-future".into(),
            secret_id: sid.clone(),
            version: 1,
            target: PlacementTarget::Manual,
            unit: None,
            cred_name: None,
            where_text: Some("x".into()),
            note: String::new(),
            restart: false,
            placed_by: "owner".into(),
            placed_at: "2026-10-24T00:00:00Z".into(),
            written_at: None,
        };
        {
            let keys = v.keys.read();
            let conn = v.conn.lock();
            let mac = hmac(&keys.row, &mac_input(&q, "zz_newer_target"));
            conn.execute(
                "INSERT INTO placement (id, secret_id, version, target, where_text, note, restart, placed_by, placed_at, row_mac)
                 VALUES ('p-future', ?1, 1, 'zz_newer_target', 'x', '', 0, 'owner', '2026-10-24T00:00:00Z', ?2)",
                params![sid, mac.to_vec()],
            )
            .unwrap();
        }
        let l = v.placements(None).unwrap();
        assert_eq!(l.unknown, 1);
        assert!(l.rows.is_empty());
    }
}
