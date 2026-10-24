//! vault.db side of `agent` delivery (prd-vault-gaps-v1 item 3): which
//! ssh keys a requester's agent socket lists, signing with one, and the
//! per-secret agent settings (rate limits, forwarded signing).
//!
//! - [`LocalVault::agent_identities`]: the requester's `agent` items (the
//!   same grant rules as spawn delivery), each as a public key. No value
//!   leaves the crate and nothing is audited (a listing isn't a use).
//! - [`LocalVault::agent_sign`]: resolve the secret for `Use::Agent` (grant,
//!   status, expiry checked again on every signature), check that its
//!   current key is the one asked for, sign, and roll up one `sign` audit
//!   entry per signature (with the bind's host-key fingerprint when the
//!   client sent one). The private key never leaves this crate.
//! - `agent_policy` (R4): an additive, MAC'd table (H22: `user_version`
//!   stays 1). No row = the defaults. Raising a limit or allowing
//!   forwarded signing is loosening (`can_loosen`, as `set_policy`).

use super::*;
use crate::ssh_agent::AgentKey;

/// Default burst: signatures per second per (session, secret).
pub const DEFAULT_BURST: u32 = 60;
/// Default signatures per minute per (session, secret).
pub const DEFAULT_PER_MINUTE: u32 = 240;
pub const BURST_MAX: u32 = 600;
pub const PER_MINUTE_MAX: u32 = 6000;

/// A secret's agent settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPolicy {
    pub burst: u32,
    pub per_minute: u32,
    /// Sign over a forwarded agent (a session bind with `is_forwarding`).
    pub allow_forwarded: bool,
    /// `None` = no row: the defaults.
    pub updated_by: Option<String>,
    pub updated_at: Option<String>,
}

impl Default for AgentPolicy {
    fn default() -> Self {
        AgentPolicy { burst: DEFAULT_BURST, per_minute: DEFAULT_PER_MINUTE, allow_forwarded: false, updated_by: None, updated_at: None }
    }
}

/// What `k2 vault agent-limit` changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentPolicyPatch {
    pub burst: Option<u32>,
    pub per_minute: Option<u32>,
    pub allow_forwarded: Option<bool>,
}

/// One key on a requester's agent socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentIdentity {
    pub secret_id: String,
    pub name: String,
    /// The public key blob (wire form).
    pub public_blob: Vec<u8>,
}

/// `agent` delivery is for ssh keys, and the value must be a key the agent
/// can serve (`delivery_not_for_kind`, `bad_value`, `ssh_key_encrypted`).
/// Only checks when one of `deliveries` is `agent`.
pub(super) fn check_agent_delivery(kind: Kind, deliveries: &[Delivery], value: Option<&[u8]>) -> Result<()> {
    if !deliveries.contains(&Delivery::Agent) {
        return Ok(());
    }
    if kind != Kind::SshKey {
        return Err(VaultError::bad_request("delivery_not_for_kind", "agent delivery is for ssh keys (--kind ssh_key)"));
    }
    match value {
        Some(v) => crate::ssh_agent::check_value(v),
        None => Ok(()),
    }
}

fn policy_mac_input(secret_id: &str, p: &AgentPolicy) -> Vec<u8> {
    let mut e = Enc::new("agent_policy");
    e.str(secret_id)
        .i64(i64::from(p.burst))
        .i64(i64::from(p.per_minute))
        .i64(i64::from(p.allow_forwarded))
        .opt_str(p.updated_by.as_deref())
        .opt_str(p.updated_at.as_deref());
    e.finish().to_vec()
}

fn read_policy(r: &Row<'_>) -> rusqlite::Result<(AgentPolicy, Vec<u8>)> {
    let p = AgentPolicy {
        burst: r.get::<_, i64>("burst")?.clamp(1, i64::from(BURST_MAX)) as u32,
        per_minute: r.get::<_, i64>("per_minute")?.clamp(1, i64::from(PER_MINUTE_MAX)) as u32,
        allow_forwarded: r.get::<_, i64>("allow_forwarded")? != 0,
        updated_by: Some(r.get("updated_by")?),
        updated_at: Some(r.get("updated_at")?),
    };
    Ok((p, r.get("row_mac")?))
}

impl LocalVault {
    fn agent_policy_in(&self, conn: &Connection, keys: &Keys, secret_id: &str) -> Result<AgentPolicy> {
        let row = conn
            .query_row("SELECT * FROM agent_policy WHERE secret_id = ?1", params![secret_id], read_policy)
            .optional()?;
        let Some((p, mac)) = row else { return Ok(AgentPolicy::default()) };
        if !hmac_verify(&keys.row, &policy_mac_input(secret_id, &p), &mac) {
            // Fail closed: the strictest settings, and status says tamper.
            self.tamper.lock().push(format!("agent policy of {secret_id} failed its integrity check"));
            return Ok(AgentPolicy { burst: 1, per_minute: 1, allow_forwarded: false, updated_by: None, updated_at: None });
        }
        Ok(p)
    }

    /// The agent settings of one secret (the defaults when none are set).
    pub fn agent_policy(&self, secret_id: &str) -> Result<AgentPolicy> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        self.agent_policy_in(&conn, &keys, secret_id)
    }

    /// `k2 vault agent-limit`: change a secret's burst, per-minute limit or
    /// forwarded signing. Raising either limit, or allowing forwarded
    /// signing, is loosening (an agent can't loosen a person's secret).
    pub fn set_agent_policy(&self, sel: &Selector, patch: AgentPolicyPatch, actor: &Actor) -> Result<(SecretMeta, AgentPolicy)> {
        let stamp = actor.stamp();
        if let Some(b) = patch.burst {
            if !(1..=BURST_MAX).contains(&b) {
                return Err(VaultError::bad_request("bad_limit", format!("--burst is 1 to {BURST_MAX} signatures a second")));
            }
        }
        if let Some(m) = patch.per_minute {
            if !(1..=PER_MINUTE_MAX).contains(&m) {
                return Err(VaultError::bad_request("bad_limit", format!("--per-minute is 1 to {PER_MINUTE_MAX} signatures a minute")));
            }
        }
        let (row, facts_ok, old) = {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            let row = self.find_row(&conn, &keys, sel)?;
            let f = self.facts_for(&conn, &keys, &row.meta, actor)?;
            let granted = self.granted_to_actor(&conn, &keys, &row.meta, actor)?;
            if !policy::can_see(actor, &f, granted) {
                return Err(VaultError::not_found("no such secret"));
            }
            let old = self.agent_policy_in(&conn, &keys, &row.meta.id)?;
            let mut new = old.clone();
            if let Some(b) = patch.burst {
                new.burst = b;
            }
            if let Some(m) = patch.per_minute {
                new.per_minute = m;
            }
            if let Some(a) = patch.allow_forwarded {
                new.allow_forwarded = a;
            }
            let loosens = new.burst > old.burst || new.per_minute > old.per_minute || (new.allow_forwarded && !old.allow_forwarded);
            let check = if loosens { policy::can_loosen(actor, &f) } else { policy::can_manage(actor, &f) };
            (row.clone(), check.map(|_| (new, loosens)), old)
        };
        let m = row.meta;
        if m.kind != Kind::SshKey {
            return Err(VaultError::bad_request("agent_policy_not_for_kind", "agent limits are for ssh keys"));
        }
        if m.status == SecretStatus::Destroyed {
            return Err(VaultError::conflict("secret_destroyed", "that secret was destroyed"));
        }
        let (mut new, loosens) = match facts_ok {
            Ok(x) => x,
            Err(e) => return Err(self.deny(Entry::new("policy", &stamp, "deny").secret(&m.id, None), e)),
        };
        if new.burst == old.burst && new.per_minute == old.per_minute && new.allow_forwarded == old.allow_forwarded {
            return Ok((m, old));
        }
        new.updated_by = Some(stamp.clone());
        new.updated_at = Some(ts(now()));
        let outcome = format!(
            "ok:agent burst={} per_minute={} forwarded={}{}",
            new.burst,
            new.per_minute,
            if new.allow_forwarded { "allow" } else { "refuse" },
            if loosens { ":widened" } else { "" }
        );
        let (id, p2) = (m.id.clone(), new.clone());
        self.mutate(true, |tx, keys| {
            let mac = hmac(&keys.row, &policy_mac_input(&id, &p2));
            tx.execute(
                "INSERT INTO agent_policy (secret_id, burst, per_minute, allow_forwarded, updated_by, updated_at, row_mac)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(secret_id) DO UPDATE SET burst = ?2, per_minute = ?3, allow_forwarded = ?4,
                    updated_by = ?5, updated_at = ?6, row_mac = ?7",
                params![
                    id,
                    i64::from(p2.burst),
                    i64::from(p2.per_minute),
                    i64::from(p2.allow_forwarded),
                    p2.updated_by,
                    p2.updated_at,
                    mac.to_vec()
                ],
            )?;
            Ok(((), vec![Entry::new("policy", &stamp, &outcome).secret(&id, None)]))
        })?;
        Ok((m, new))
    }

    /// Rekey: re-MAC every agent policy row under the new row key.
    pub(super) fn rekey_agent_policies(&self, tx: &Transaction<'_>, old: &Keys, new: &Keys) -> Result<()> {
        let rows: Vec<(String, AgentPolicy, Vec<u8>)> = {
            let mut st = tx.prepare("SELECT * FROM agent_policy")?;
            let it = st.query_map([], |r| {
                let id: String = r.get("secret_id")?;
                let (p, mac) = read_policy(r)?;
                Ok((id, p, mac))
            })?;
            it.collect::<std::result::Result<_, _>>()?
        };
        for (id, p, mac) in rows {
            if !hmac_verify(&old.row, &policy_mac_input(&id, &p), &mac) {
                return Err(VaultError::tamper(format!("agent policy of {id} failed its integrity check; rekey stopped")));
            }
            let m2 = hmac(&new.row, &policy_mac_input(&id, &p));
            tx.execute("UPDATE agent_policy SET row_mac = ?2 WHERE secret_id = ?1", params![id, m2.to_vec()])?;
        }
        Ok(())
    }

    /// The keys `who`'s agent socket lists: its `agent` items (highest
    /// delivery wins, as at spawn), each parsed to its public key. A value
    /// that no longer parses (rotated to something the agent can't serve)
    /// is left out.
    pub fn agent_identities(&self, who: &Requester) -> Result<Vec<AgentIdentity>> {
        let items: Vec<SpawnItem> = self.spawn_items(who)?.into_iter().filter(|i| i.delivery == Delivery::Agent).collect();
        if items.is_empty() {
            return Ok(vec![]);
        }
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let mut out = vec![];
        for it in items {
            let Ok(row) = self.find_row(&conn, &keys, &Selector::Id(it.secret_id.clone())) else { continue };
            if row.meta.kind != Kind::SshKey {
                continue;
            }
            let Ok(v) = self.decrypt_version(&conn, &keys, &row.meta, row.meta.latest_version) else { continue };
            let Ok(k) = AgentKey::parse(v.expose()) else { continue };
            out.push(AgentIdentity { secret_id: it.secret_id, name: it.name, public_blob: k.public_blob() });
        }
        Ok(out)
    }

    /// Sign `data` with secret `secret_id` for `who`. The grant, status and
    /// expiry are checked again (`Use::Agent`), and the current key must be
    /// the one asked for (`key_changed` after a rotation). One `sign`
    /// audit entry per signature, rolled up, with `ssh:<host-key fp>` when
    /// the connection sent a session bind.
    pub fn agent_sign(&self, secret_id: &str, who: &Requester, key_blob: &[u8], data: &[u8], flags: u32, host_fp: Option<&str>) -> Result<Vec<u8>> {
        let r = self.resolve_inner(&Selector::Id(secret_id.to_string()), Use::Agent(host_fp), who)?;
        let v = Some(r.version);
        if r.meta.kind != Kind::SshKey {
            return Err(self.agent_refused(secret_id, v, who, "delivery_not_for_kind", host_fp));
        }
        let parsed = AgentKey::parse(r.value.expose());
        drop(r);
        let key = match parsed {
            Ok(k) => k,
            Err(e) => {
                self.agent_note(Some(secret_id), v, who, &format!("deny:{}", e.code), host_fp);
                return Err(e);
            }
        };
        if key.public_blob() != key_blob {
            return Err(self.agent_refused(secret_id, v, who, "key_changed", host_fp));
        }
        match key.sign(data, flags) {
            Ok(sig) => {
                self.agent_note(Some(secret_id), v, who, "ok", host_fp);
                Ok(sig)
            }
            Err(e) => {
                self.agent_note(Some(secret_id), v, who, &format!("deny:{}", e.code), host_fp);
                Err(e)
            }
        }
    }

    fn agent_refused(&self, secret_id: &str, version: Option<u32>, who: &Requester, code: &'static str, host_fp: Option<&str>) -> VaultError {
        self.agent_note(Some(secret_id), version, who, &format!("deny:{code}"), host_fp);
        VaultError::forbidden(code, "the agent won't sign with that key")
    }

    /// One agent audit entry, rolled up (so a client hammering a refusal
    /// can't flood the audit chain): action `sign`, outcome `ok` or
    /// `deny:<code>` (`forwarded`, `rate_limited`, `not_granted`, …), the
    /// session as consumer, `ssh:<host-key fp>` as host when known.
    pub fn agent_note(&self, secret_id: Option<&str>, version: Option<u32>, who: &Requester, outcome: &str, host_fp: Option<&str>) {
        let label = who.label();
        let mut e = Entry::new("sign", &label, outcome).consumer(label.clone());
        if let Some(id) = secret_id {
            e = e.secret(id, version);
        }
        if let Some(fp) = host_fp {
            e = e.host(format!("ssh:{fp}"));
        }
        self.rollups.lock().add(&e);
    }

    /// `k2 vault public-key NAME`: the `.pub` line of an ssh key `actor`
    /// can see, and its `SHA256:` fingerprint. Works for any OpenSSH key
    /// (a passphrase key's public half is stored in the clear) and RSA PEM.
    pub fn agent_public_key(&self, sel: &Selector, actor: &Actor) -> Result<(SecretMeta, String, String)> {
        let m = self.meta_inner(sel, actor)?;
        if m.kind != Kind::SshKey {
            return Err(VaultError::bad_request("not_an_ssh_key", "public-key is for ssh keys (--kind ssh_key)"));
        }
        if m.status == SecretStatus::Destroyed {
            return Err(VaultError::conflict("secret_destroyed", "that secret was destroyed"));
        }
        let v = {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            self.decrypt_version(&conn, &keys, &m, m.latest_version)?
        };
        let (line, fp) = crate::ssh_agent::public_of(v.expose(), &m.name)?;
        drop(v);
        self.rollups.lock().add(&Entry::new("public_key", &actor.stamp(), "ok").secret(&m.id, Some(m.latest_version)));
        Ok((m, line, fp))
    }
}
