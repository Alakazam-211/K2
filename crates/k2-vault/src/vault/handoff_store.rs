//! vault.db side of the agent-to-agent secret handoff
//! (`prd-agent-secret-handoff-v1` §6, §7.3, §9, §12).
//!
//! - `handoff`: one row per delivery and direction (names, never values).
//! - `handoff_value`: a pending incoming copy's value, encrypted under the
//!   data key with the delivery id in the AAD; deleted on decide/expire.
//! - `handoff_accept`: the per-workspace accept-from list (H19: a vault
//!   row MAC, so an agent that can edit `k2so.db` still can't add itself).
//! - `peer_vault`: peers' vault recipients from signed statements (H3).
//!
//! The land-or-pend rule (§6.1, Rosson's rulings 2026-10-17): pending by
//! default; straight in only when the daemon says `auto_land` (accept-from
//! or a reply to the receiver's own ask), never over another secret, and
//! never for a person-given secret, a wildcard/body-inject item (H11) or a
//! cross-server send with the receiver's remote-instruct opt-in off (R4).
//! A same-sender re-send of the same secret is a new version.

use super::*;
use crate::handoff::{self as h, AcceptEntry, Decision, HandoffItem, HandoffRecord, Incoming, PeerVault, Received};

fn record_mac_input(r: &HandoffRecord, item_json: &str) -> Vec<u8> {
    let mut e = Enc::new("handoff");
    e.str(&r.delivery_id)
        .str(&r.direction)
        .str(&r.status)
        .str(&r.local_workspace_id)
        .str(&r.peer_addr)
        .opt_str(r.peer_fp.as_deref())
        .str(&r.sender_actor)
        .opt_str(r.source_vault_id.as_deref())
        .str(&r.name)
        .opt_str(r.landed_name.as_deref())
        .str(&r.kind)
        .str(&r.note)
        .opt_str(r.reply_to.as_deref())
        .str(item_json)
        .opt_str(r.secret_id.as_deref())
        .opt_i64(r.version.map(i64::from))
        .opt_str(r.reason.as_deref())
        .opt_str(r.card.as_deref())
        .str(&r.created_at)
        .opt_str(r.decided_at.as_deref())
        .opt_str(r.decided_by.as_deref())
        .opt_str(r.expires_at.as_deref());
    e.finish().to_vec()
}

fn accept_mac_input(a: &AcceptEntry) -> Vec<u8> {
    let mut e = Enc::new("handoff-accept");
    e.str(&a.workspace_id).str(&a.sender).str(&a.added_by).str(&a.added_at);
    e.finish().to_vec()
}

fn peer_vault_mac_input(p: &PeerVault, features_json: &str) -> Vec<u8> {
    let mut e = Enc::new("peer-vault");
    e.str(&p.peer_fp).str(&p.vault_id).str(&p.recipient).str(features_json).str(&p.fetched_at);
    e.finish().to_vec()
}

fn value_row_mac_input(id: &str, nonce: &[u8], ct: &[u8]) -> Vec<u8> {
    let mut e = Enc::new("handoff-value-row");
    e.str(id).bytes(nonce).bytes(ct);
    e.finish().to_vec()
}

fn item_json(i: &HandoffItem) -> String {
    serde_json::to_string(i).expect("handoff item json")
}

fn read_record_raw(r: &Row<'_>) -> Result<(HandoffRecord, String, Vec<u8>)> {
    let item_s: String = r.get("item_json")?;
    let item: HandoffItem = serde_json::from_str(&item_s).map_err(|_| VaultError::tamper("bad handoff item json"))?;
    let rec = HandoffRecord {
        delivery_id: r.get("delivery_id")?,
        direction: r.get("direction")?,
        status: r.get("status")?,
        local_workspace_id: r.get("local_workspace_id")?,
        peer_addr: r.get("peer_addr")?,
        peer_fp: r.get("peer_fp")?,
        sender_actor: r.get("sender_actor")?,
        source_vault_id: r.get("source_vault_id")?,
        name: r.get("name")?,
        landed_name: r.get("landed_name")?,
        kind: r.get("kind")?,
        note: r.get("note")?,
        reply_to: r.get("reply_to")?,
        item,
        secret_id: r.get("secret_id")?,
        version: r.get::<_, Option<i64>>("version")?.map(|v| v as u32),
        reason: r.get("reason")?,
        card: r.get("card")?,
        created_at: r.get("created_at")?,
        decided_at: r.get("decided_at")?,
        decided_by: r.get("decided_by")?,
        expires_at: r.get("expires_at")?,
    };
    let mac: Vec<u8> = r.get("row_mac")?;
    Ok((rec, item_s, mac))
}

fn is_pk_conflict(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation)
}

impl LocalVault {
    // ── handoff rows ────────────────────────────────────────────────

    fn handoff_insert(&self, tx: &Transaction<'_>, keys: &Keys, r: &HandoffRecord) -> Result<()> {
        let ij = item_json(&r.item);
        let mac = hmac(&keys.row, &record_mac_input(r, &ij));
        let res = tx.execute(
            "INSERT INTO handoff (delivery_id, direction, status, local_workspace_id, peer_addr, peer_fp, sender_actor,
                source_vault_id, name, landed_name, kind, note, reply_to, item_json, secret_id, version, reason, card,
                created_at, decided_at, decided_by, expires_at, row_mac)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)",
            params![
                r.delivery_id, r.direction, r.status, r.local_workspace_id, r.peer_addr, r.peer_fp, r.sender_actor,
                r.source_vault_id, r.name, r.landed_name, r.kind, r.note, r.reply_to, ij, r.secret_id,
                r.version.map(i64::from), r.reason, r.card, r.created_at, r.decided_at, r.decided_by, r.expires_at,
                mac.to_vec()
            ],
        );
        match res {
            Ok(_) => Ok(()),
            Err(e) if is_pk_conflict(&e) => Err(VaultError::conflict("replay", "this delivery id was already recorded")),
            Err(e) => Err(e.into()),
        }
    }

    fn handoff_write(&self, tx: &Transaction<'_>, keys: &Keys, r: &HandoffRecord) -> Result<()> {
        let ij = item_json(&r.item);
        let mac = hmac(&keys.row, &record_mac_input(r, &ij));
        let n = tx.execute(
            "UPDATE handoff SET status = ?3, landed_name = ?4, secret_id = ?5, version = ?6, reason = ?7, card = ?8,
                decided_at = ?9, decided_by = ?10, expires_at = ?11, row_mac = ?12
             WHERE delivery_id = ?1 AND direction = ?2",
            params![
                r.delivery_id, r.direction, r.status, r.landed_name, r.secret_id, r.version.map(i64::from), r.reason,
                r.card, r.decided_at, r.decided_by, r.expires_at, mac.to_vec()
            ],
        )?;
        if n != 1 {
            return Err(VaultError::not_found("no such delivery"));
        }
        Ok(())
    }

    fn handoff_load(&self, conn: &Connection, keys: &Keys, id: &str, dir: &str) -> Result<Option<HandoffRecord>> {
        let row = conn
            .query_row(
                "SELECT * FROM handoff WHERE delivery_id = ?1 AND direction = ?2",
                params![id, dir],
                |r| Ok(read_record_raw(r)),
            )
            .optional()?;
        let Some(row) = row else { return Ok(None) };
        let (rec, ij, mac) = row?;
        if !hmac_verify(&keys.row, &record_mac_input(&rec, &ij), &mac) {
            self.tamper.lock().push(format!("handoff {id} failed its integrity check"));
            return Err(VaultError::tamper("this handoff's record failed its integrity check"));
        }
        Ok(Some(rec))
    }

    /// One delivery, by id and direction (`in` | `out`).
    pub fn handoff_get(&self, id: &str, dir: &str) -> Result<HandoffRecord> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        self.handoff_load(&conn, &keys, id, dir)?.ok_or_else(|| VaultError::not_found("no such delivery"))
    }

    /// Deliveries, newest first, optionally for one workspace and one
    /// direction. Rows that fail their MAC are skipped (and noted).
    pub fn handoffs(&self, workspace: Option<&str>, direction: Option<&str>) -> Result<Vec<HandoffRecord>> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT * FROM handoff ORDER BY created_at DESC, delivery_id LIMIT 2000")?;
        let it = st.query_map([], |r| Ok(read_record_raw(r)))?;
        let mut out = vec![];
        for row in it {
            let (rec, ij, mac) = match row? {
                Ok(x) => x,
                Err(_) => continue,
            };
            if !hmac_verify(&keys.row, &record_mac_input(&rec, &ij), &mac) {
                self.tamper.lock().push(format!("handoff {} failed its integrity check", rec.delivery_id));
                continue;
            }
            if workspace.is_some_and(|w| w != rec.local_workspace_id) || direction.is_some_and(|d| d != rec.direction) {
                continue;
            }
            out.push(rec);
        }
        Ok(out)
    }

    /// The sender's `out` row (§5.1 step 6, §5.2 "the sender writes its out
    /// row only after an answer"), with a vault audit row naming the source
    /// secret and version.
    pub fn handoff_record_out(&self, r: HandoffRecord, source: Option<(&str, u32)>) -> Result<HandoffRecord> {
        if r.direction != "out" {
            return Err(VaultError::internal("an out row"));
        }
        let mut e = Entry::new("send", &r.sender_actor, &format!("ok:{}", r.status)).consumer(r.peer_addr.clone());
        if let Some((id, v)) = source {
            e = e.secret(id, Some(v));
        }
        let r2 = r.clone();
        self.mutate(false, |tx, keys| {
            self.handoff_insert(tx, keys, &r2)?;
            Ok(((), vec![e]))
        })?;
        Ok(r)
    }

    /// Change an `out` row after the fact (a retry answered, unsend).
    pub fn handoff_update_out(&self, id: &str, status: &str, reason: Option<&str>) -> Result<HandoffRecord> {
        let mut r = self.handoff_get(id, "out")?;
        r.status = status.to_string();
        r.reason = reason.map(str::to_string);
        let r2 = r.clone();
        self.mutate(false, |tx, keys| {
            self.handoff_write(tx, keys, &r2)?;
            Ok(((), vec![]))
        })?;
        Ok(r)
    }

    // ── asks (H5, §4.3) ─────────────────────────────────────────────

    /// Record an ask (`ask_out`: this workspace asked; `ask_in`: a local
    /// agent asked this one). Open for [`h::PENDING_DAYS`].
    pub fn handoff_record_ask(&self, mut r: HandoffRecord) -> Result<HandoffRecord> {
        if r.direction != "ask_out" && r.direction != "ask_in" {
            return Err(VaultError::internal("an ask row"));
        }
        validate_name(&r.name, true)?;
        r.status = "open".into();
        r.created_at = ts(now());
        r.expires_at = Some(ts(now() + chrono::Duration::days(h::PENDING_DAYS)));
        let r2 = r.clone();
        let e = Entry::new("ask", &r.sender_actor, &format!("ok:{}", r.direction)).consumer(r.peer_addr.clone());
        self.mutate(false, |tx, keys| {
            self.handoff_insert(tx, keys, &r2)?;
            Ok(((), vec![e]))
        })?;
        Ok(r)
    }

    /// Close an open ask (`answered` / `expired`); false when it wasn't open.
    pub fn handoff_close_ask(&self, id: &str, dir: &str, status: &str) -> Result<bool> {
        let mut r = match self.handoff_get(id, dir) {
            Ok(r) => r,
            Err(e) if e.status == 404 => return Ok(false),
            Err(e) => return Err(e),
        };
        if r.status != "open" {
            return Ok(false);
        }
        r.status = status.to_string();
        r.decided_at = Some(ts(now()));
        self.mutate(false, |tx, keys| {
            self.handoff_write(tx, keys, &r)?;
            Ok(((), vec![]))
        })?;
        Ok(true)
    }

    /// Does `workspace` have an open ask `id` to `peer_addr` for `name`
    /// (still within its 7 days)? A reply that matches lands straight in
    /// (R1).
    pub fn handoff_ask_matches(&self, id: &str, workspace: &str, peer_addr: &str, name: &str) -> bool {
        self.handoff_get(id, "ask_out")
            .map(|a| {
                a.local_workspace_id == workspace
                    && a.status == "open"
                    && a.peer_addr.eq_ignore_ascii_case(peer_addr)
                    && a.name == name
                    && a.expires_at.as_deref().and_then(parse_ts).is_none_or(|x| x >= now())
            })
            .unwrap_or(false)
    }

    // ── unsend (H5, §9) ─────────────────────────────────────────────

    /// The sender takes back a delivery (§9). On this vault's `in` row: a
    /// pending copy is dropped; a landed copy is disabled only if it is
    /// still the version that delivery wrote (a person's replacement is
    /// left alone). `peer_fp`: cross server, the row must come from that
    /// peer. Honest limit: unsend is not a revoke; the receiver may already
    /// have used the key. Returns the row and whether a copy was disabled.
    pub fn handoff_unsend(&self, id: &str, peer_fp: Option<&str>, actor_label: &str) -> Result<(HandoffRecord, bool)> {
        let mut r = self.handoff_get(id, "in")?;
        if let Some(fp) = peer_fp {
            if r.peer_fp.as_deref() != Some(fp) {
                return Err(VaultError::forbidden("not_yours", "only the server that sent a delivery can take it back"));
            }
        }
        let consumer = format!("ws:{}", r.local_workspace_id);
        let mut disabled = false;
        match r.status.as_str() {
            "pending" => {
                r.status = "unsent".into();
                r.reason = Some("unsent".into());
            }
            "landed" => {
                let current = match (&r.secret_id, r.version) {
                    (Some(sid), Some(v)) => {
                        let keys = self.keys.read();
                        let conn = self.conn.lock();
                        self.find_row(&conn, &keys, &Selector::Id(sid.clone()))
                            .ok()
                            .filter(|row| row.meta.latest_version == v && row.meta.status == SecretStatus::Active)
                            .map(|row| row.meta.id)
                    }
                    _ => None,
                };
                if let Some(sid) = current {
                    self.set_status(&Selector::Id(sid), &Actor::System(h::SYSTEM_ACTOR.into()), SecretStatus::Disabled)?;
                    disabled = true;
                    r.reason = Some("unsent".into());
                } else {
                    r.reason = Some("unsent_kept_replaced".into());
                }
                r.status = "unsent".into();
            }
            other => return Err(VaultError::conflict("not_unsendable", format!("that delivery is {other}"))),
        }
        r.decided_at = Some(ts(now()));
        r.decided_by = Some(actor_label.to_string());
        let r2 = r.clone();
        let outcome = if disabled { "ok:disabled" } else { "ok" };
        self.mutate(false, |tx, keys| {
            tx.execute("DELETE FROM handoff_value WHERE delivery_id = ?1", params![r2.delivery_id])?;
            self.handoff_write(tx, keys, &r2)?;
            Ok(((), vec![Entry::new("unsend", actor_label, outcome).consumer(consumer)]))
        })?;
        Ok((r, disabled))
    }

    /// Remember the Thread card a pending copy posted.
    pub fn handoff_set_card(&self, id: &str, card: &str) -> Result<()> {
        let mut r = self.handoff_get(id, "in")?;
        r.card = Some(card.to_string());
        self.mutate(false, |tx, keys| {
            self.handoff_write(tx, keys, &r)?;
            Ok(((), vec![]))
        })
    }

    /// A free-text vault audit row for a handoff event the daemon decides
    /// (refusals before any row exists). Names only.
    pub fn handoff_audit(&self, action: &str, actor: &str, consumer: &str, outcome: &str) {
        self.audit_only(vec![Entry::new(action, actor, outcome).consumer(consumer)]);
    }

    // ── pending values ──────────────────────────────────────────────

    fn handoff_value_aad(&self, id: &str) -> Vec<u8> {
        let mut e = Enc::new("handoff-value");
        e.bytes(&self.vid).str(id);
        e.finish().to_vec()
    }

    fn handoff_value_put(&self, tx: &Transaction<'_>, keys: &Keys, id: &str, value: &SecretBytes) -> Result<()> {
        let (nonce, ct) = xseal(&keys.vdk, &self.handoff_value_aad(id), value.expose());
        let mac = hmac(&keys.row, &value_row_mac_input(id, &nonce, &ct));
        tx.execute(
            "INSERT INTO handoff_value (delivery_id, nonce, ciphertext, row_mac) VALUES (?1, ?2, ?3, ?4)",
            params![id, nonce.to_vec(), ct, mac.to_vec()],
        )?;
        Ok(())
    }

    fn handoff_value_read(&self, conn: &Connection, keys: &Keys, id: &str) -> Result<SecretBytes> {
        let row: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = conn
            .query_row(
                "SELECT nonce, ciphertext, row_mac FROM handoff_value WHERE delivery_id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (nonce, ct, mac) = row.ok_or_else(|| VaultError::not_found("the pending value is gone"))?;
        if !hmac_verify(&keys.row, &value_row_mac_input(id, &nonce, &ct), &mac) {
            self.tamper.lock().push(format!("handoff value {id} failed its integrity check"));
            return Err(VaultError::tamper("the pending value failed its integrity check"));
        }
        Ok(SecretBytes::new(xopen(&keys.vdk, &nonce, &self.handoff_value_aad(id), &ct)?))
    }

    /// Does a pending value row exist (tests and `doctor`)?
    pub fn handoff_has_value(&self, id: &str) -> bool {
        let conn = self.conn.lock();
        conn.query_row("SELECT 1 FROM handoff_value WHERE delivery_id = ?1", params![id], |_| Ok(()))
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    // ── receive (land or pend) ──────────────────────────────────────

    fn landing_request(&self, inc_item: &HandoffItem, ws: &str, name: &str, delivery_id: &str, label: &str) -> Result<PutRequest> {
        let kind = inc_item.kind()?;
        let (d, c) = inc_item.landing_delivery();
        let mut req = PutRequest::new(Namespace::User, Scope::workspace(ws), name, kind);
        req.hosts = inc_item.hosts.clone();
        req.inject = inc_item.inject.clone();
        req.default_delivery = Some(d);
        req.delivery_ceiling = Some(c);
        req.origin = format!("{}{delivery_id}", h::ORIGIN_PREFIX);
        let desc = if inc_item.description.trim().is_empty() {
            format!("From {label}")
        } else {
            format!("From {label}: {}", inc_item.description.trim())
        };
        req.description = desc.chars().take(500).collect();
        req.expires_at = inc_item.ttl_secs.map(|t| ts(now() + chrono::Duration::seconds(t)));
        req.mode = PutMode::Create;
        req.human_given_override = Some(false);
        Ok(req)
    }

    /// Is `row` an earlier landed copy of this same secret from this same
    /// sender (§6.1 re-send = rotation)?
    fn same_sender_copy(&self, row: &SecretRow, peer_addr: &str, sender_name: &str) -> bool {
        if row.meta.status != SecretStatus::Active {
            return false;
        }
        let Some(prior) = row.meta.origin.strip_prefix(h::ORIGIN_PREFIX) else { return false };
        match self.handoff_get(prior, "in") {
            Ok(p) => p.peer_addr == peer_addr && p.name == sender_name,
            Err(_) => false,
        }
    }

    /// Land or pend an incoming secret (§6.1). The daemon has already run
    /// every transport and sender gate; this applies the receiving rules,
    /// writes the `in` row and the vault audit. Durable replay (H9): a
    /// delivery id already received answers 409 `replay`.
    pub fn handoff_receive(&self, inc: Incoming, value: SecretBytes) -> Result<Received> {
        if !h::valid_delivery_id(&inc.delivery_id) {
            return Err(VaultError::bad_request("bad_handoff", "delivery ids are hd_<uuid>"));
        }
        inc.item.validate()?;
        h::check_value(value.expose())?;
        let scope = Scope::workspace(inc.workspace_id.clone());
        scope.validate()?;
        if *self.rollback.lock() {
            return Err(VaultError::new("vault_rollback", 409, "the vault looks rolled back; the owner must confirm in Settings → Vault"));
        }
        {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            if self.handoff_load(&conn, &keys, &inc.delivery_id, "in")?.is_some() {
                return Err(VaultError::conflict("replay", "this delivery was already received"));
            }
        }
        let landed_name = inc.item.landed_name().to_string();
        let existing = {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            match self.find_row(&conn, &keys, &Selector::user(scope.clone(), landed_name.clone())) {
                Ok(r) => Some(r),
                Err(e) if e.status == 404 => None,
                Err(e) => return Err(e),
            }
        };
        enum Plan {
            Create,
            Rotate,
            Pend(&'static str),
        }
        let plan = if inc.remote_off {
            Plan::Pend("remote_instruction_off")
        } else if inc.item.human_given {
            Plan::Pend("person_given")
        } else if inc.item.needs_person() {
            Plan::Pend("needs_person")
        } else {
            match &existing {
                None if inc.auto_land => Plan::Create,
                None => Plan::Pend("not_allowed"),
                Some(row) if self.same_sender_copy(row, &inc.peer_addr, &inc.item.name) => {
                    if inc.auto_land {
                        Plan::Rotate
                    } else {
                        Plan::Pend("resend")
                    }
                }
                Some(_) => Plan::Pend("clash"),
            }
        };
        let consumer = format!("ws:{}", inc.workspace_id);
        let actor_label = format!("handoff:{}", inc.peer_addr);
        let mut rec = HandoffRecord {
            delivery_id: inc.delivery_id.clone(),
            direction: "in".into(),
            status: String::new(),
            local_workspace_id: inc.workspace_id.clone(),
            peer_addr: inc.peer_addr.clone(),
            peer_fp: inc.peer_fp.clone(),
            sender_actor: inc.sender_actor.clone(),
            source_vault_id: inc.source_vault_id.clone(),
            name: inc.item.name.clone(),
            landed_name: Some(landed_name.clone()),
            kind: inc.item.kind.clone(),
            note: inc.note.clone(),
            reply_to: inc.reply_to.clone(),
            item: inc.item.clone(),
            secret_id: None,
            version: None,
            reason: None,
            card: None,
            created_at: ts(now()),
            decided_at: None,
            decided_by: None,
            expires_at: None,
        };
        match plan {
            Plan::Pend(reason) => {
                let pending: Vec<HandoffRecord> =
                    self.handoffs(Some(&inc.workspace_id), Some("in"))?.into_iter().filter(|r| r.status == "pending").collect();
                if pending.len() >= h::PENDING_PER_WORKSPACE {
                    return Err(VaultError::new(
                        "too_many_pending",
                        429,
                        "that agent already has 25 secrets waiting for its owner; ask them to review k2 vault incoming",
                    ));
                }
                if pending.iter().filter(|r| r.peer_addr == inc.peer_addr).count() >= h::PENDING_PER_SENDER {
                    return Err(VaultError::new(
                        "too_many_pending",
                        429,
                        "5 of your secrets already wait for that agent's owner; wait for them to decide",
                    ));
                }
                rec.status = "pending".into();
                rec.reason = Some(reason.into());
                rec.expires_at = Some(ts(now() + chrono::Duration::days(h::PENDING_DAYS)));
                let r2 = rec.clone();
                self.mutate(false, |tx, keys| {
                    self.handoff_insert(tx, keys, &r2)?;
                    self.handoff_value_put(tx, keys, &r2.delivery_id, &value)?;
                    Ok(((), vec![Entry::new("receive", &actor_label, &format!("ok:pending:{reason}")).consumer(consumer.clone())]))
                })?;
                Ok(Received { record: rec, meta: None })
            }
            Plan::Create | Plan::Rotate => {
                let rotate = matches!(plan, Plan::Rotate);
                let mut req = self.landing_request(&inc.item, &inc.workspace_id, &landed_name, &inc.delivery_id, &inc.sender_label)?;
                if rotate {
                    req.mode = PutMode::Rotate;
                }
                let out = self.put_inner(req, value, &Actor::System(h::SYSTEM_ACTOR.into()))?;
                rec.status = "landed".into();
                rec.secret_id = Some(out.meta.id.clone());
                rec.version = Some(out.meta.latest_version);
                rec.expires_at = out.meta.expires_at.clone();
                rec.reason = Some(if rotate { "new_version".into() } else { "allowed".into() });
                let r2 = rec.clone();
                let sid = out.meta.id.clone();
                let ver = out.meta.latest_version;
                let res = self.mutate(false, |tx, keys| {
                    self.handoff_insert(tx, keys, &r2)?;
                    Ok(((), vec![Entry::new("receive", &actor_label, "ok:landed").consumer(consumer.clone()).secret(&sid, Some(ver))]))
                });
                if let Err(e) = res {
                    // Never leave a copy without its provenance row.
                    if !rotate {
                        let _ = self.set_status(&Selector::Id(sid), &Actor::System(h::SYSTEM_ACTOR.into()), SecretStatus::Destroyed);
                    }
                    return Err(e);
                }
                Ok(Received { record: rec, meta: Some(out.meta) })
            }
        }
    }

    // ── a person decides ────────────────────────────────────────────

    /// Accept or decline a pending copy (§4.5). People only: the owner
    /// token, an Owner/Admin login, or a Member (who may keep secrets for
    /// a workspace). Agents, guests and K2 shell tabs can't (KV8 parity).
    pub fn handoff_decide(&self, id: &str, d: &Decision, actor: &Actor) -> Result<Received> {
        if !matches!(actor, Actor::Owner | Actor::Login { .. }) {
            return Err(VaultError::forbidden(
                "handoff_person_only",
                "a person on this server accepts or declines secrets from other agents (Settings → Vault → Incoming, or the Thread card)",
            ));
        }
        let mut r = self.handoff_get(id, "in")?;
        if r.status != "pending" {
            return Err(VaultError::conflict("not_pending", format!("that delivery is {}", r.status)));
        }
        if r.expires_at.as_deref().and_then(parse_ts).is_some_and(|x| x < now()) {
            self.handoff_expire(r)?;
            return Err(VaultError::conflict("not_pending", "that delivery expired (pending copies wait 7 days)"));
        }
        policy::can_create(actor, Namespace::User, &Scope::workspace(r.local_workspace_id.clone()))?;
        let stamp = actor.stamp();
        let consumer = format!("ws:{}", r.local_workspace_id);
        match d.action.as_str() {
            "decline" => {
                r.status = "declined".into();
                r.decided_at = Some(ts(now()));
                r.decided_by = Some(stamp.clone());
                let r2 = r.clone();
                self.mutate(false, |tx, keys| {
                    tx.execute("DELETE FROM handoff_value WHERE delivery_id = ?1", params![r2.delivery_id])?;
                    self.handoff_write(tx, keys, &r2)?;
                    Ok(((), vec![Entry::new("decline", &stamp, "ok").consumer(consumer.clone())]))
                })?;
                self.checkpoint();
                Ok(Received { record: r, meta: None })
            }
            "accept" => {
                let name = match d.as_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    Some(n) => n.to_string(),
                    None => r.item.landed_name().to_string(),
                };
                validate_name(&name, true)?;
                let value = {
                    let keys = self.keys.read();
                    let conn = self.conn.lock();
                    self.handoff_value_read(&conn, &keys, &r.delivery_id)?
                };
                let scope = Scope::workspace(r.local_workspace_id.clone());
                let exists = {
                    let keys = self.keys.read();
                    let conn = self.conn.lock();
                    match self.find_row(&conn, &keys, &Selector::user(scope.clone(), name.clone())) {
                        Ok(_) => true,
                        Err(e) if e.status == 404 => false,
                        Err(e) => return Err(e),
                    }
                };
                let label = r.peer_addr.clone();
                let mut req = self.landing_request(&r.item, &r.local_workspace_id, &name, &r.delivery_id, &label)?;
                let put_actor = if exists {
                    if !d.replace {
                        return Err(VaultError::conflict(
                            "name_taken",
                            format!("this workspace already has {name}; accept it as another name, or replace the current one"),
                        ));
                    }
                    req.mode = PutMode::Rotate;
                    Actor::System(h::SYSTEM_ACTOR.into())
                } else if r.item.needs_person() {
                    // H11: wildcard sites or body injection land as the
                    // person who accepted them.
                    actor.clone()
                } else {
                    Actor::System(h::SYSTEM_ACTOR.into())
                };
                let out = self.put_inner(req, value, &put_actor)?;
                r.status = "landed".into();
                r.landed_name = Some(name);
                r.secret_id = Some(out.meta.id.clone());
                r.version = Some(out.meta.latest_version);
                r.decided_at = Some(ts(now()));
                r.decided_by = Some(stamp.clone());
                r.expires_at = out.meta.expires_at.clone();
                let r2 = r.clone();
                let (sid, ver) = (out.meta.id.clone(), out.meta.latest_version);
                self.mutate(false, |tx, keys| {
                    tx.execute("DELETE FROM handoff_value WHERE delivery_id = ?1", params![r2.delivery_id])?;
                    self.handoff_write(tx, keys, &r2)?;
                    Ok(((), vec![Entry::new("accept", &stamp, "ok").consumer(consumer.clone()).secret(&sid, Some(ver))]))
                })?;
                self.checkpoint();
                Ok(Received { record: r, meta: Some(out.meta) })
            }
            _ => Err(VaultError::bad_request("bad_action", "accept or decline")),
        }
    }

    fn handoff_expire(&self, mut r: HandoffRecord) -> Result<HandoffRecord> {
        r.status = "expired".into();
        r.decided_at = Some(ts(now()));
        r.decided_by = Some("system:expiry".into());
        let r2 = r.clone();
        let consumer = format!("ws:{}", r.local_workspace_id);
        self.mutate(false, |tx, keys| {
            tx.execute("DELETE FROM handoff_value WHERE delivery_id = ?1", params![r2.delivery_id])?;
            self.handoff_write(tx, keys, &r2)?;
            Ok(((), vec![Entry::new("expire", "system:expiry", "ok").consumer(consumer)]))
        })?;
        Ok(r)
    }

    /// Periodic work (§9, H13): pending copies past their 7 days expire
    /// (value rows deleted); landed copies past their `--ttl` are destroyed.
    /// Returns the deliveries it expired, so the daemon closes their cards
    /// and writes the audit lines.
    pub fn handoff_tick(&self) -> Result<Vec<HandoffRecord>> {
        let nowt = now();
        let due: Vec<HandoffRecord> = self
            .handoffs(None, Some("in"))?
            .into_iter()
            .filter(|r| r.status == "pending" && r.expires_at.as_deref().and_then(parse_ts).is_some_and(|x| x < nowt))
            .collect();
        let mut expired = Vec::with_capacity(due.len());
        for r in due {
            expired.push(self.handoff_expire(r)?);
        }
        if !expired.is_empty() {
            self.checkpoint();
        }
        // Asks close after their 7 days (a late reply then pends).
        for dir in ["ask_out", "ask_in"] {
            let open: Vec<HandoffRecord> = self
                .handoffs(None, Some(dir))?
                .into_iter()
                .filter(|r| r.status == "open" && r.expires_at.as_deref().and_then(parse_ts).is_some_and(|x| x < nowt))
                .collect();
            for r in open {
                self.handoff_close_ask(&r.delivery_id, dir, "expired")?;
            }
        }
        let stale: Vec<String> = {
            let conn = self.conn.lock();
            let mut st = conn.prepare(
                "SELECT id FROM secret WHERE namespace = 'user' AND origin LIKE 'handoff:%' AND status != 'destroyed'
                   AND expires_at IS NOT NULL AND expires_at < ?1",
            )?;
            let it = st.query_map(params![ts(nowt)], |r| r.get::<_, String>(0))?;
            it.collect::<std::result::Result<_, _>>()?
        };
        for id in stale {
            self.set_status(&Selector::Id(id), &Actor::System("expiry".into()), SecretStatus::Destroyed)?;
        }
        Ok(expired)
    }

    /// Tests: move a delivery's expiry into the past (re-MACed).
    #[cfg(any(test, feature = "test-util"))]
    pub fn handoff_backdate_for_tests(&self, id: &str, dir: &str) -> Result<()> {
        let mut r = self.handoff_get(id, dir)?;
        r.expires_at = Some(ts(now() - chrono::Duration::minutes(1)));
        self.mutate(false, |tx, keys| {
            self.handoff_write(tx, keys, &r)?;
            Ok(((), vec![]))
        })
    }

    /// Tests: move a secret's own expiry into the past (re-MACed), so the
    /// H13 sweep runs without waiting an hour.
    #[cfg(any(test, feature = "test-util"))]
    pub fn expire_secret_for_tests(&self, secret_id: &str) -> Result<()> {
        let row = {
            let keys = self.keys.read();
            let conn = self.conn.lock();
            self.find_row(&conn, &keys, &Selector::Id(secret_id.to_string()))?
        };
        let mut m = row.meta.clone();
        m.expires_at = Some(ts(now() - chrono::Duration::minutes(1)));
        self.mutate(false, |tx, keys| {
            self.write_secret(tx, keys, &m, &row.external_ref, false)?;
            Ok(((), vec![]))
        })
    }

    // ── accept-from (§7.3, H19) ─────────────────────────────────────

    /// The accept-from list for a workspace (or every workspace).
    pub fn accept_from(&self, workspace: Option<&str>) -> Result<Vec<AcceptEntry>> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let mut st = conn.prepare("SELECT workspace_id, sender, added_by, added_at, row_mac FROM handoff_accept ORDER BY workspace_id, sender")?;
        let it = st.query_map([], |r| {
            Ok((
                AcceptEntry { workspace_id: r.get(0)?, sender: r.get(1)?, added_by: r.get(2)?, added_at: r.get(3)? },
                r.get::<_, Vec<u8>>(4)?,
            ))
        })?;
        let mut out = vec![];
        for row in it {
            let (a, mac) = row?;
            if !hmac_verify(&keys.row, &accept_mac_input(&a), &mac) {
                self.tamper.lock().push(format!("accept-from {}/{} failed its integrity check", a.workspace_id, a.sender));
                continue;
            }
            if workspace.is_some_and(|w| w != a.workspace_id) {
                continue;
            }
            out.push(a);
        }
        Ok(out)
    }

    /// Does `workspace` accept from any of `senders` (already normalised)?
    pub fn accepts_from(&self, workspace: &str, senders: &[String]) -> bool {
        self.accept_from(Some(workspace))
            .map(|l| l.iter().any(|a| senders.iter().any(|s| s == &a.sender)))
            .unwrap_or(false)
    }

    /// Add a sender (people only; the route checks the caller).
    pub fn accept_from_add(&self, workspace: &str, sender: &str, actor: &Actor) -> Result<AcceptEntry> {
        if !matches!(actor, Actor::Owner | Actor::Login { .. }) {
            return Err(VaultError::forbidden("handoff_person_only", "only a person on this server edits who an agent accepts secrets from"));
        }
        Scope::workspace(workspace).validate()?;
        let sender = h::normalise_sender(sender)?;
        if sender == format!("ws:{workspace}") {
            return Err(VaultError::bad_request("bad_sender", "an agent doesn't send to itself"));
        }
        let a = AcceptEntry { workspace_id: workspace.to_string(), sender, added_by: actor.stamp(), added_at: ts(now()) };
        let a2 = a.clone();
        let stamp = actor.stamp();
        self.mutate(true, |tx, keys| {
            let mac = hmac(&keys.row, &accept_mac_input(&a2));
            tx.execute(
                "INSERT INTO handoff_accept (workspace_id, sender, added_by, added_at, row_mac) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (workspace_id, sender) DO UPDATE SET added_by = excluded.added_by, added_at = excluded.added_at, row_mac = excluded.row_mac",
                params![a2.workspace_id, a2.sender, a2.added_by, a2.added_at, mac.to_vec()],
            )?;
            Ok(((), vec![Entry::new("accept-from-add", &stamp, "ok").consumer(format!("ws:{}<-{}", a2.workspace_id, a2.sender))]))
        })?;
        Ok(a)
    }

    /// Remove a sender; `false` when it wasn't listed.
    pub fn accept_from_remove(&self, workspace: &str, sender: &str, actor: &Actor) -> Result<bool> {
        if !matches!(actor, Actor::Owner | Actor::Login { .. }) {
            return Err(VaultError::forbidden("handoff_person_only", "only a person on this server edits who an agent accepts secrets from"));
        }
        let sender = h::normalise_sender(sender)?;
        let stamp = actor.stamp();
        let ws = workspace.to_string();
        self.mutate(true, |tx, _| {
            let n = tx.execute("DELETE FROM handoff_accept WHERE workspace_id = ?1 AND sender = ?2", params![ws, sender])?;
            let e = Entry::new("accept-from-remove", &stamp, if n > 0 { "ok" } else { "ok:absent" }).consumer(format!("ws:{ws}<-{sender}"));
            Ok((n > 0, vec![e]))
        })
    }

    // ── peer vaults (H3) ────────────────────────────────────────────

    pub fn peer_vault(&self, fp: &str) -> Option<PeerVault> {
        let keys = self.keys.read();
        let conn = self.conn.lock();
        let row: Option<(String, String, String, String, Vec<u8>)> = conn
            .query_row(
                "SELECT vault_id, recipient, features, fetched_at, row_mac FROM peer_vault WHERE peer_fp = ?1",
                params![fp],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
            .ok()
            .flatten();
        let (vault_id, recipient, features_json, fetched_at, mac) = row?;
        let features: Vec<String> = serde_json::from_str(&features_json).ok()?;
        let p = PeerVault { peer_fp: fp.to_string(), vault_id, recipient, features, fetched_at };
        if !hmac_verify(&keys.row, &peer_vault_mac_input(&p, &features_json), &mac) {
            self.tamper.lock().push(format!("peer vault {fp} failed its integrity check"));
            return None;
        }
        Some(p)
    }

    /// Cache a verified statement. Returns the previous entry when the
    /// vault id or recipient changed (the daemon audits `peer-vault-changed`).
    pub fn peer_vault_put(&self, p: &PeerVault) -> Result<Option<PeerVault>> {
        agefmt::Recipient::parse(&p.recipient)?;
        let prev = self.peer_vault(&p.peer_fp);
        let changed = prev.as_ref().filter(|o| o.vault_id != p.vault_id || o.recipient != p.recipient).cloned();
        let fj = serde_json::to_string(&p.features).expect("features json");
        let p2 = p.clone();
        let entries = match &changed {
            Some(o) => vec![Entry::new("peer-vault-changed", "system:handoff", &format!("ok:{}->{}", o.vault_id, p.vault_id)).consumer(p.peer_fp.clone())],
            None => vec![],
        };
        self.mutate(false, |tx, keys| {
            let mac = hmac(&keys.row, &peer_vault_mac_input(&p2, &fj));
            tx.execute(
                "INSERT INTO peer_vault (peer_fp, vault_id, recipient, features, fetched_at, row_mac) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (peer_fp) DO UPDATE SET vault_id = excluded.vault_id, recipient = excluded.recipient,
                    features = excluded.features, fetched_at = excluded.fetched_at, row_mac = excluded.row_mac",
                params![p2.peer_fp, p2.vault_id, p2.recipient, fj, p2.fetched_at, mac.to_vec()],
            )?;
            Ok(((), entries))
        })?;
        Ok(changed)
    }

    /// Forget a peer's cached vault (after `carry_not_for_this_vault`).
    pub fn peer_vault_forget(&self, fp: &str) -> Result<()> {
        let fp = fp.to_string();
        self.mutate(false, |tx, _| {
            tx.execute("DELETE FROM peer_vault WHERE peer_fp = ?1", params![fp])?;
            Ok(((), vec![]))
        })
    }
}
