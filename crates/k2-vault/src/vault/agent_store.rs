//! vault.db side of `agent` delivery (prd-vault-gaps-v1 item 3): which
//! ssh keys a requester's agent socket lists, signing with one, the
//! per-secret agent settings (rate limits, forwarded signing), and the
//! daemon's own record of each agent socket.
//!
//! - [`LocalVault::agent_identities`]: the requester's `agent` items (the
//!   same grant rules as spawn delivery), each as a public key. Public
//!   blobs are cached by (secret, version) and parsed outside the vault
//!   locks. No value leaves the crate and nothing is audited (a listing
//!   isn't a use).
//! - [`LocalVault::agent_sign`]: resolve the secret for `Use::Agent` (grant,
//!   status, expiry checked again on every signature), check that its
//!   current key is the one asked for, sign, and roll up one `sign` audit
//!   entry per signature. The private key never leaves this crate.
//! - `agent_policy` (R4): an additive, MAC'd table (H22: `user_version`
//!   stays 1). No row = the defaults. Raising a limit or allowing
//!   forwarded signing is loosening (`can_loosen`, as `set_policy`). A row
//!   is bound to the audit entry that wrote it (its `audit_seq`), and that
//!   entry must be the latest agent-policy entry for the secret: deleting
//!   or replaying a row (to undo a tightening) fails closed.
//! - `agent_socket`: one MAC'd row per live agent socket, written by the
//!   daemon when it starts one (tag, workspace, secrets dir, socket path,
//!   short-path root) and bound, once k2-ptyd has spawned the session, to
//!   that ptyd instance and process (ptyd boot id, pid, start time). A
//!   restarted daemon re-serves a socket, and the boot sweep removes one,
//!   only from this record; nothing in the (same-user writable) secrets
//!   dir or the ptyd meta is trusted for that, and a replayed record can't
//!   match a different process.
//! - Policy freshness also checks the audit log's tail against the MAC'd
//!   header (no gap, nothing missing), so deleting a newer audit entry
//!   can't revive an older row.

use std::collections::HashMap;

use super::*;
use crate::ssh_agent::AgentKey;

/// Default burst: signatures per second per (session, secret).
pub const DEFAULT_BURST: u32 = 60;
/// Default signatures per minute per (session, secret).
pub const DEFAULT_PER_MINUTE: u32 = 240;
pub const BURST_MAX: u32 = 600;
pub const PER_MINUTE_MAX: u32 = 6000;

/// The settings a secret falls back to when its row can't be trusted.
fn strictest() -> AgentPolicy {
    AgentPolicy { burst: 1, per_minute: 1, allow_forwarded: false, updated_by: None, updated_at: None }
}

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

/// The daemon's record of one agent socket.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentSocket {
    /// The session id (or `k2 job` run id) the socket serves.
    pub tag: String,
    /// `session` or `job`.
    pub kind: String,
    pub workspace_id: String,
    pub secrets_dir: String,
    pub sock_path: String,
    /// The short-path root the socket was made under (empty: the socket is
    /// in `secrets_dir`). Validation uses this, never the current env.
    pub short_root: String,
    pub created_at: String,
    /// The k2-ptyd instance and process the session runs in (empty / 0:
    /// not spawned under ptyd, never re-served).
    pub ptyd_boot: String,
    pub pid: i64,
    pub started_at: i64,
}

/// What would break if this vault were opened by 0.46.3 (`k2 vault
/// doctor`): rows whose enum text that daemon doesn't know (and so fails
/// every list and spawn on), or whose meaning it ignores.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DowngradeBlockers {
    /// Secrets or grants with `agent` delivery.
    pub agent: u64,
    /// Grants to a `k2 job`.
    pub job_grants: u64,
    /// Signed-fetch secrets (0.46.3 would send the value as Bearer).
    pub signed: u64,
}

impl DowngradeBlockers {
    pub fn any(&self) -> bool {
        self.agent + self.job_grants + self.signed > 0
    }
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

fn policy_mac_input(secret_id: &str, p: &AgentPolicy, audit_seq: i64) -> Vec<u8> {
    let mut e = Enc::new("agent_policy");
    e.str(secret_id)
        .i64(i64::from(p.burst))
        .i64(i64::from(p.per_minute))
        .i64(i64::from(p.allow_forwarded))
        .opt_str(p.updated_by.as_deref())
        .opt_str(p.updated_at.as_deref())
        .i64(audit_seq);
    e.finish().to_vec()
}

/// The audit outcome a policy write records (without `:widened`).
fn policy_outcome(p: &AgentPolicy) -> String {
    format!("ok:agent burst={} per_minute={} forwarded={}", p.burst, p.per_minute, if p.allow_forwarded { "allow" } else { "refuse" })
}

fn read_policy(r: &Row<'_>) -> rusqlite::Result<(AgentPolicy, i64, Vec<u8>)> {
    let p = AgentPolicy {
        burst: r.get::<_, i64>("burst")?.clamp(1, i64::from(BURST_MAX)) as u32,
        per_minute: r.get::<_, i64>("per_minute")?.clamp(1, i64::from(PER_MINUTE_MAX)) as u32,
        allow_forwarded: r.get::<_, i64>("allow_forwarded")? != 0,
        updated_by: Some(r.get("updated_by")?),
        updated_at: Some(r.get("updated_at")?),
    };
    Ok((p, r.get("audit_seq")?, r.get("row_mac")?))
}

fn socket_mac_input(s: &AgentSocket) -> Vec<u8> {
    let mut e = Enc::new("agent_socket");
    e.str(&s.tag)
        .str(&s.kind)
        .str(&s.workspace_id)
        .str(&s.secrets_dir)
        .str(&s.sock_path)
        .str(&s.short_root)
        .str(&s.created_at)
        .str(&s.ptyd_boot)
        .i64(s.pid)
        .i64(s.started_at);
    e.finish().to_vec()
}

fn read_socket(r: &Row<'_>) -> rusqlite::Result<(AgentSocket, Vec<u8>)> {
    Ok((
        AgentSocket {
            tag: r.get("tag")?,
            kind: r.get("kind")?,
            workspace_id: r.get("workspace_id")?,
            secrets_dir: r.get("secrets_dir")?,
            sock_path: r.get("sock_path")?,
            short_root: r.get("short_root")?,
            created_at: r.get("created_at")?,
            ptyd_boot: r.get("ptyd_boot")?,
            pid: r.get("pid")?,
            started_at: r.get("started_at")?,
        },
        r.get("row_mac")?,
    ))
}

fn socket_params(s: &AgentSocket, mac: &[u8]) -> [Box<dyn rusqlite::ToSql>; 11] {
    [
        Box::new(s.tag.clone()),
        Box::new(s.kind.clone()),
        Box::new(s.workspace_id.clone()),
        Box::new(s.secrets_dir.clone()),
        Box::new(s.sock_path.clone()),
        Box::new(s.short_root.clone()),
        Box::new(s.created_at.clone()),
        Box::new(s.ptyd_boot.clone()),
        Box::new(s.pid),
        Box::new(s.started_at),
        Box::new(mac.to_vec()),
    ]
}

const SOCKET_UPSERT: &str = "INSERT OR REPLACE INTO agent_socket
    (tag, kind, workspace_id, secrets_dir, sock_path, short_root, created_at, ptyd_boot, pid, started_at, row_mac)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)";


/// Parsed public blobs by (secret id, version); `None` = not a key the
/// agent serves. A version's value never changes, so entries never go
/// stale. Public data only.
type BlobCache = parking_lot::Mutex<HashMap<(String, u32), Option<Vec<u8>>>>;
static BLOBS: std::sync::OnceLock<BlobCache> = std::sync::OnceLock::new();
const BLOB_CACHE_MAX: usize = 1024;

fn blobs() -> &'static BlobCache {
    BLOBS.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

impl LocalVault {
    /// The latest agent-policy audit entry for `secret_id`, MAC-checked:
    /// `Some((seq, outcome))`, `None` when there is none. An entry whose
    /// MAC fails is `Err` (tamper).
    fn latest_policy_audit(&self, conn: &Connection, keys: &Keys, secret_id: &str) -> Result<Option<(i64, String)>> {
        let row = conn
            .query_row(
                "SELECT seq, ts, secret_id, version, action, actor, consumer, target_host, outcome, count, prev_mac, row_mac
                 FROM audit WHERE secret_id = ?1 AND action = 'policy' AND outcome LIKE 'ok:agent %' ORDER BY seq DESC LIMIT 1",
                params![secret_id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        Entry {
                            secret_id: r.get(2)?,
                            version: r.get(3)?,
                            action: r.get(4)?,
                            actor: r.get(5)?,
                            consumer: r.get(6)?,
                            target_host: r.get(7)?,
                            outcome: r.get(8)?,
                            count: r.get::<_, i64>(9)? as u64,
                        },
                        r.get::<_, Vec<u8>>(10)?,
                        r.get::<_, Vec<u8>>(11)?,
                    ))
                },
            )
            .optional()?;
        let Some((seq, t, e, prev, mac)) = row else { return Ok(None) };
        if audit::row_mac(&keys.audit, seq as u64, &t, &e, &prev).as_slice() != mac.as_slice() {
            return Err(VaultError::tamper("agent policy audit entry"));
        }
        Ok(Some((seq, e.outcome)))
    }

    /// Is the audit log whole: rows 1..=header.audit_seq, no gap, the
    /// header's MAC good? Cached per (header seq, `PRAGMA data_version`):
    /// K2's own appends move the header, any other write to the file moves
    /// the data version, so an entry deleted behind K2's back is seen on
    /// the next check.
    fn audit_log_whole(&self, conn: &Connection, keys: &Keys) -> bool {
        let Ok(Some((h, mac))) = load_header(conn) else { return false };
        if !hmac_verify(&keys.header, &h.mac_input(), &mac) {
            return false;
        }
        let Ok(dv) = conn.query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0)) else { return false };
        if let Some((seq, v, ok)) = *self.audit_whole.lock() {
            if seq == h.audit_seq && v == dv {
                return ok;
            }
        }
        let counted: rusqlite::Result<(i64, i64, i64)> =
            conn.query_row("SELECT COUNT(*), COALESCE(MIN(seq), 1), COALESCE(MAX(seq), 0) FROM audit", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)));
        let ok = counted.is_ok_and(|(n, lo, hi)| lo == 1 && hi == h.audit_seq as i64 && n == hi);
        *self.audit_whole.lock() = Some((h.audit_seq, dv, ok));
        ok
    }

    fn agent_policy_in(&self, conn: &Connection, keys: &Keys, secret_id: &str) -> Result<AgentPolicy> {
        if !self.audit_log_whole(conn, keys) {
            // An audit entry is missing (or added) outside K2: no row's
            // freshness can be shown, so no row (and no "no row") is trusted.
            self.tamper_once("the audit log has a gap or doesn't match its header; agent policies fall back to the strictest".into());
            return Ok(strictest());
        }
        let row = conn
            .query_row("SELECT * FROM agent_policy WHERE secret_id = ?1", params![secret_id], read_policy)
            .optional()?;
        let latest = match self.latest_policy_audit(conn, keys, secret_id) {
            Ok(l) => l,
            Err(_) => {
                self.tamper_once(format!("the audit entry for the agent policy of {secret_id} failed its integrity check"));
                return Ok(strictest());
            }
        };
        match (row, latest) {
            (None, None) => Ok(AgentPolicy::default()),
            (None, Some(_)) => {
                // The audit log says a policy was set; the row is gone
                // (removed outside K2, or reset by an upgrade from a
                // pre-release build). A person sets it again.
                self.tamper_once(format!(
                    "the agent policy of {secret_id} is missing (removed outside K2, or reset by an upgrade); set it again with k2 vault agent-limit"
                ));
                Ok(strictest())
            }
            (Some((p, seq, mac)), latest) => {
                if !hmac_verify(&keys.row, &policy_mac_input(secret_id, &p, seq), &mac) {
                    self.tamper_once(format!("agent policy of {secret_id} failed its integrity check"));
                    return Ok(strictest());
                }
                let want = policy_outcome(&p);
                let fresh = latest.is_some_and(|(s, out)| s == seq && (out == want || out == format!("{want}:widened")));
                if !fresh {
                    // An older row put back, or one with no audit entry.
                    self.tamper_once(format!("agent policy of {secret_id} isn't the latest one K2 wrote"));
                    return Ok(strictest());
                }
                Ok(p)
            }
        }
    }

    /// The agent settings of one secret (the defaults when none are set;
    /// the strictest when its row can't be trusted).
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
        if new.burst == old.burst && new.per_minute == old.per_minute && new.allow_forwarded == old.allow_forwarded && old.updated_by.is_some() {
            return Ok((m, old));
        }
        new.updated_by = Some(stamp.clone());
        new.updated_at = Some(ts(now()));
        let outcome = format!("{}{}", policy_outcome(&new), if loosens { ":widened" } else { "" });
        let (id, p2) = (m.id.clone(), new.clone());
        self.mutate(true, |tx, keys| {
            // The one entry this mutation appends gets the next sequence
            // number: the row is bound to it.
            let seq = load_header(tx)?.ok_or_else(|| VaultError::internal("header missing"))?.0.audit_seq as i64 + 1;
            let mac = hmac(&keys.row, &policy_mac_input(&id, &p2, seq));
            tx.execute(
                "INSERT INTO agent_policy (secret_id, burst, per_minute, allow_forwarded, updated_by, updated_at, audit_seq, row_mac)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(secret_id) DO UPDATE SET burst = ?2, per_minute = ?3, allow_forwarded = ?4,
                    updated_by = ?5, updated_at = ?6, audit_seq = ?7, row_mac = ?8",
                params![
                    id,
                    i64::from(p2.burst),
                    i64::from(p2.per_minute),
                    i64::from(p2.allow_forwarded),
                    p2.updated_by,
                    p2.updated_at,
                    seq,
                    mac.to_vec()
                ],
            )?;
            Ok(((), vec![Entry::new("policy", &stamp, &outcome).secret(&id, None)]))
        })?;
        Ok((m, new))
    }

    /// Rekey: verify, then re-MAC, every agent policy and agent socket row
    /// under the new row key. Any row that fails stops the rekey.
    pub(super) fn rekey_agent_tables(&self, tx: &Transaction<'_>, old: &Keys, new: &Keys) -> Result<()> {
        let rows: Vec<(String, AgentPolicy, i64, Vec<u8>)> = {
            let mut st = tx.prepare("SELECT * FROM agent_policy")?;
            let it = st.query_map([], |r| {
                let id: String = r.get("secret_id")?;
                let (p, seq, mac) = read_policy(r)?;
                Ok((id, p, seq, mac))
            })?;
            it.collect::<std::result::Result<_, _>>()?
        };
        for (id, p, seq, mac) in rows {
            if !hmac_verify(&old.row, &policy_mac_input(&id, &p, seq), &mac) {
                return Err(VaultError::tamper(format!("agent policy of {id} failed its integrity check; rekey stopped")));
            }
            let m2 = hmac(&new.row, &policy_mac_input(&id, &p, seq));
            tx.execute("UPDATE agent_policy SET row_mac = ?2 WHERE secret_id = ?1", params![id, m2.to_vec()])?;
        }
        let socks: Vec<(AgentSocket, Vec<u8>)> = {
            let mut st = tx.prepare("SELECT * FROM agent_socket")?;
            let it = st.query_map([], read_socket)?;
            it.collect::<std::result::Result<_, _>>()?
        };
        for (s, mac) in socks {
            if !hmac_verify(&old.row, &socket_mac_input(&s), &mac) {
                return Err(VaultError::tamper(format!("agent socket record {} failed its integrity check; rekey stopped", s.tag)));
            }
            let m2 = hmac(&new.row, &socket_mac_input(&s));
            tx.execute("UPDATE agent_socket SET row_mac = ?2 WHERE tag = ?1", params![s.tag, m2.to_vec()])?;
        }
        Ok(())
    }

    // ── the daemon's agent socket records ──────────────────────────────

    /// Record (or replace) the socket the daemon just started for `s.tag`.
    pub fn agent_socket_put(&self, s: &AgentSocket) -> Result<()> {
        if !matches!(s.kind.as_str(), "session" | "job") || s.tag.is_empty() || s.secrets_dir.is_empty() || s.sock_path.is_empty() {
            return Err(VaultError::bad_request("bad_agent_socket", "agent socket record"));
        }
        // A new socket is never bound yet: the binding comes from ptyd.
        let s2 = AgentSocket { ptyd_boot: String::new(), pid: 0, started_at: 0, ..s.clone() };
        self.mutate(false, |tx, keys| {
            let mac = hmac(&keys.row, &socket_mac_input(&s2));
            tx.execute(SOCKET_UPSERT, rusqlite::params_from_iter(socket_params(&s2, &mac).iter().map(|b| b.as_ref())))?;
            Ok(((), vec![]))
        })
    }

    /// k2-ptyd spawned session `tag`: bind its record (if it has one, and
    /// it isn't bound yet) to that ptyd instance and process. A restarted
    /// daemon re-serves the socket only to that same process.
    pub fn agent_socket_bind(&self, tag: &str, ptyd_boot: &str, pid: i64, started_at: i64) -> Result<bool> {
        if ptyd_boot.is_empty() || pid <= 0 || started_at <= 0 {
            return Err(VaultError::bad_request("bad_agent_socket", "ptyd binding"));
        }
        let Some(rec) = self.agent_socket_get(tag)? else { return Ok(false) };
        if !rec.ptyd_boot.is_empty() {
            // Bound once, for the life of the session.
            return Ok(rec.ptyd_boot == ptyd_boot && rec.pid == pid && rec.started_at == started_at);
        }
        let s2 = AgentSocket { ptyd_boot: ptyd_boot.to_string(), pid, started_at, ..rec };
        self.mutate(false, |tx, keys| {
            let mac = hmac(&keys.row, &socket_mac_input(&s2));
            tx.execute(SOCKET_UPSERT, rusqlite::params_from_iter(socket_params(&s2, &mac).iter().map(|b| b.as_ref())))?;
            Ok(((), vec![]))
        })?;
        Ok(true)
    }

    /// Every agent socket record whose MAC holds. A row that fails is left
    /// out (and noted once): it is never re-served or followed.
    pub fn agent_sockets(&self) -> Result<Vec<AgentSocket>> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT * FROM agent_socket ORDER BY tag LIMIT 10000")?;
        let it = st.query_map([], read_socket)?;
        let mut out = vec![];
        for r in it {
            let (s, mac) = r?;
            if hmac_verify(&keys.row, &socket_mac_input(&s), &mac) {
                out.push(s);
            } else {
                self.tamper_once(format!("agent socket record {} failed its integrity check", s.tag));
            }
        }
        Ok(out)
    }

    /// One record by tag (MAC-checked).
    pub fn agent_socket_get(&self, tag: &str) -> Result<Option<AgentSocket>> {
        Ok(self.agent_sockets()?.into_iter().find(|s| s.tag == tag))
    }

    /// Drop a record (the socket is gone).
    pub fn agent_socket_remove(&self, tag: &str) -> Result<()> {
        let t = tag.to_string();
        self.mutate(false, |tx, _| {
            tx.execute("DELETE FROM agent_socket WHERE tag = ?1", params![t])?;
            Ok(((), vec![]))
        })
    }

    /// Tests: write a record row as-is (forged or inconsistent), MAC'd.
    #[cfg(any(test, feature = "test-util"))]
    pub fn agent_socket_put_unchecked_for_tests(&self, s: &AgentSocket) -> Result<()> {
        let s2 = s.clone();
        self.mutate(false, |tx, keys| {
            let mac = hmac(&keys.row, &socket_mac_input(&s2));
            tx.execute(SOCKET_UPSERT, rusqlite::params_from_iter(socket_params(&s2, &mac).iter().map(|b| b.as_ref())))?;
            Ok(((), vec![]))
        })
    }

    /// `k2 vault doctor`: rows a 0.46.3 daemon would choke on or misread.
    pub fn downgrade_blockers(&self) -> Result<DowngradeBlockers> {
        let conn = self.conn.lock();
        let count = |sql: &str| -> Result<u64> { Ok(conn.query_row(sql, [], |r| r.get::<_, i64>(0))?.max(0) as u64) };
        Ok(DowngradeBlockers {
            // Destroyed rows still carry the text 0.46.3 can't parse.
            agent: count("SELECT COUNT(*) FROM secret WHERE default_delivery = 'agent' OR delivery_ceiling = 'agent'")?
                + count("SELECT COUNT(*) FROM grants WHERE delivery = 'agent'")?,
            job_grants: count("SELECT COUNT(*) FROM grants WHERE consumer_kind = 'job'")?,
            signed: count("SELECT COUNT(*) FROM secret WHERE status != 'destroyed' AND inject LIKE '%\"sign\"%'")?,
        })
    }

    /// The keys `who`'s agent socket lists: its `agent` items (highest
    /// delivery wins, as at spawn), each as its public key. Values are
    /// decrypted only for (secret, version) pairs not seen before, and
    /// parsed after the vault locks are released. A value that isn't a key
    /// the agent serves is left out.
    pub fn agent_identities(&self, who: &Requester) -> Result<Vec<AgentIdentity>> {
        let items: Vec<SpawnItem> = self.spawn_items(who)?.into_iter().filter(|i| i.delivery == Delivery::Agent).collect();
        if items.is_empty() {
            return Ok(vec![]);
        }
        let mut known: Vec<(SpawnItem, u32, Option<Option<Vec<u8>>>)> = vec![];
        let mut fresh: Vec<(usize, SecretBytes)> = vec![];
        {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            for it in items {
                let Ok(row) = self.find_row(&conn, &keys, &Selector::Id(it.secret_id.clone())) else { continue };
                if row.meta.kind != Kind::SshKey {
                    continue;
                }
                let version = row.meta.latest_version;
                let cached = blobs().lock().get(&(it.secret_id.clone(), version)).cloned();
                if cached.is_none() {
                    let Ok(v) = self.decrypt_version(&conn, &keys, &row.meta, version) else { continue };
                    fresh.push((known.len(), v));
                }
                known.push((it, version, cached));
            }
        }
        for (i, v) in fresh {
            let blob = AgentKey::parse(v.expose()).ok().map(|k| k.public_blob());
            drop(v);
            let (it, version, slot) = &mut known[i];
            let mut c = blobs().lock();
            if c.len() >= BLOB_CACHE_MAX {
                c.clear();
            }
            c.insert((it.secret_id.clone(), *version), blob.clone());
            *slot = Some(blob);
        }
        Ok(known
            .into_iter()
            .filter_map(|(it, _, b)| b.flatten().map(|public_blob| AgentIdentity { secret_id: it.secret_id, name: it.name, public_blob }))
            .collect())
    }

    /// Sign `data` with secret `secret_id` for `who`. The grant, status and
    /// expiry are checked again (`Use::Agent`), and the current key must be
    /// the one asked for (`key_changed` after a rotation). One `sign`
    /// audit entry per signature, rolled up, with `ssh:<host-key fp>` when
    /// the connection sent a session bind (advisory: the fingerprint is
    /// what the client says it bound to).
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
    /// can't flood the audit chain; the roll-up itself caps its distinct
    /// keys): action `sign`, outcome `ok` or `deny:<code>` (`forwarded`,
    /// `rate_limited`, `not_granted`, …), the session as consumer,
    /// `ssh:<host-key fp>` as host when known.
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
