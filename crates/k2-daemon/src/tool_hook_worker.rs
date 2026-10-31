//! Grant-hook delivery worker (Sew S27 §9, §16).
//!
//! Delivers `tool_outbox` rows (written by `k2_core::skin` in the same
//! transaction as each grant change) to Sew's mgmt socket, with retries,
//! backoff and the row's stable `Idempotency-Key`, and runs the reconcile
//! sweep (boot, every 10 min, on `kick_reconcile`). Spawned in
//! `daemon_main.rs` right after `federation_drain::spawn()`, i.e. after
//! `boot_status::set_ready()`, so its first pass is the boot pass and rows
//! queued before a restart deliver on the next boot.
//!
//! Before the FIRST send of a `provision` (A8: provision is declarative —
//! an absent field clears it), the worker looks the member up
//! (`GET /mgmt/v1/principals?external_id=…`) and copies back the profile
//! fields K2 does not own (`title`, `pronouns`; `email` for agents and the
//! owner). The filled body is persisted with `filled_at` BEFORE the send,
//! so every retry sends the same bytes under the same key.
//!
//! Outcome rules (§9.3 with S27-A6/A7/A11):
//!
//! | Response | Row |
//! |---|---|
//! | 2xx | `done` (target healthy) |
//! | 404 on deprovision | `done` (already gone) |
//! | 404 on update | `done` + reconcile |
//! | 404 on `sew_session_revoke` | `done` (no such member: nothing to end) |
//! | 409 `username_taken`, first time | rewrite to `<base>-<hhhh>`, key `<old>_u`, now |
//! | 409 `username_taken` again | `blocked` |
//! | 422 `assistant_person_unknown` | `pending` + backoff, `blocked` after 1 h |
//! | 422 `assistant_person_inactive` and any other 4xx | `blocked` |
//! | 429 | `pending`, `max(Retry-After, backoff)` |
//! | 401 | `pending` + backoff; target unhealthy "mgmt token rejected" |
//! | 5xx / transport | `pending` + backoff; target unhealthy; stop this target's pass |
//!
//! There is no max attempts for transient failures: a Sew outage of any
//! length leaves rows `pending`. A target with `enabled = 0` is paused.
//! S28's `sew_session_revoke` rows (`POST /mgmt/v1/sessions/revoke`) are
//! delivered like the rest, each on its own `revoke:…` subject. Rows of an
//! op this build doesn't know are never selected, sent or blocked.
//!
//! K31 App Space rows (`space_member_add` / `space_member_remove`, subject
//! `space:<space>:<ext>`) ride the same rules, plus ([`refine_space`]):
//!
//! | Answer to `space_member_add` | Row |
//! |---|---|
//! | 2xx, the person in `added`/`already` | `done` (ledger `added`) |
//! | 2xx, `not_added: not_provisioned` | `pending` + backoff (provision still queued) |
//! | 2xx, `skipped` (banned) or another `not_added` reason | `held` (HC2) |
//! | 2xx naming neither | `blocked` |
//! | 404 on `space_member_remove` | `done` |
//!
//! Sew 0.5.0 rows: `policy_set` (`POST /mgmt/v1/policies`, the
//! managed-fields policy the sweep found not K2's; `k2_core::tool_hook::policy`)
//! and `space_unbind` (`POST /mgmt/v1/spaces/unbind` for a removed App's
//! Space; a 404, no such Space, is `done`). Both follow the table above; a
//! 400 `validation_failed` blocks loudly. Provisions still carry
//! `title`/`pronouns` (A8's copy-back): Sew 0.5.0's provision clears an
//! absent one, so K2 keeps them managed until Sew 0.5.1.
//!
//! Sew 0.6.0 rows (`app_governed_spaces`; `k2_core::tool_hook::governed`):
//!
//! | Row | Sent as | Answer |
//! |---|---|---|
//! | `space_settings` (subject `space:<id>`) | `POST /mgmt/v1/spaces/{id}/settings {governance}` | 409 `box_users_present` → K31 removes of exactly the listed box users, the switch goes again after; 409 `no_governing_app` → blocked (bind again) |
//! | `channels_bulk` (subject `chans:<id>`) | `POST /mgmt/v1/channels/bulk` | per op, slug → id; 409 `not_app_governed` → retry |
//! | `space_member_channels` (subject `chan:<id>:<ext>`) | one due row: `POST …/members/{ext}/channels`; more in one Space: `POST …/members/channels` (≤ 500, key from the rows' keys) | per item ([`send_sync_group`]; bulk items are `{external_id, status, answer}` or `{…, error}`): `skipped` (`banned`, `deprovisioned`, `quota_exceeded`) / `unknown_channel_ids` → held; `skipped: owner` → done; `queued` → done (read back next sweep); 422 `member_not_provisioned` → provisioned again, then synced again |
//! Sew 0.6.0 `home_client` rows (A23, flag `home_client_owner_approval`):
//! each send carries the install owner's approval, a fresh `k2:owner` chat
//! pass under its own `Idempotency-Key` ([`home_client_send`]), and only
//! for a setting the owner made; anyone else's value blocks unsent.
//! `403 owner_approval_required` blocks with a clear error (or retries
//! after a flag re-read when the send carried no approval), and `409
//! home_client_cannot_govern` blocks with a clear error ([`refine_home`]).
//!
//! K26: a pass that finished, blocked or held a row, or saw a row fail for
//! the first time, emits ONE `sew:changed {reason: grant-hook}` (the
//! app-level `sew_changed` event) so the People and Sew pages refetch.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::task::JoinHandle;

use k2_core::log_debug;
use k2_core::tool_hook::outbox::OutboxRow;
use k2_core::tool_hook::sew_wire::{self, codes, reasons};
use k2_core::tool_hook::{self as hook, Op};

use crate::sew_mgmt::{MgmtClient, MgmtResponse, TransportError};

/// Worker knobs (read once at spawn).
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub tick: Duration,
    pub backoff_base: Duration,
    pub backoff_cap: Duration,
    pub reconcile_every: Duration,
    pub reconcile_debounce: Duration,
    pub dep_wait: Duration,
}

fn env_ms(name: &str, default: u64) -> Duration {
    Duration::from_millis(
        std::env::var(name)
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(default),
    )
}

impl WorkerConfig {
    pub fn from_env() -> Self {
        WorkerConfig {
            tick: env_ms("K2_GRANT_HOOK_TICK_MS", 5_000),
            backoff_base: env_ms("K2_GRANT_HOOK_BACKOFF_BASE_MS", 2_000),
            backoff_cap: env_ms("K2_GRANT_HOOK_BACKOFF_CAP_MS", 300_000),
            reconcile_every: env_ms("K2_GRANT_HOOK_RECONCILE_MS", 600_000),
            reconcile_debounce: env_ms("K2_GRANT_HOOK_RECONCILE_DEBOUNCE_MS", 5_000),
            dep_wait: env_ms("K2_GRANT_HOOK_DEP_WAIT_MS", 3_600_000),
        }
    }
}

/// `base × 2^(attempt-1)`, capped, ±20% jitter.
pub fn backoff(cfg: &WorkerConfig, attempt: i64) -> Duration {
    let exp = (attempt.max(1) - 1).min(30) as u32;
    let raw = cfg
        .backoff_base
        .saturating_mul(2u32.saturating_pow(exp))
        .min(cfg.backoff_cap);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let permille = (nanos % 401) as i64 - 200; // -200..=200 → ±20%
    let ms = raw.as_millis() as i64;
    Duration::from_millis((ms + ms * permille / 1000).max(0) as u64)
}

fn secs_ceil(d: Duration) -> i64 {
    let ms = d.as_millis() as i64;
    (ms + 999) / 1000
}

/// What to do with a row after one attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Done {
        status: u16,
        body: String,
        reconcile: bool,
    },
    UsernameRetry {
        status: u16,
    },
    Retry {
        status: Option<u16>,
        error: String,
        delay: Duration,
        /// Target-level failure: mark unhealthy with this error.
        target_error: Option<String>,
        /// Stop this target's pass (transport/5xx/401).
        stop_target: bool,
    },
    Blocked {
        status: Option<u16>,
        error: String,
    },
    /// HC2 (K31): a Space add answered 2xx without adding the person, for
    /// a reason that holds until the grant changes.
    Held {
        status: u16,
        body: String,
        why: String,
    },
    /// Sew 0.6.0 I3: the switch to `app` answered `409 box_users_present`;
    /// K2 removes exactly these box users, then switches again.
    BoxUsersPresent {
        status: u16,
        body: String,
        ids: Vec<String>,
    },
    /// Sew 0.6.0 I4: `422 member_not_provisioned` for a sync item; K2
    /// provisions the person again, then syncs again.
    Reprovision {
        status: u16,
        why: String,
    },
}

/// K31: read a 2xx `space_member_add` answer for the one person the row
/// sent; every other outcome passes through.
pub fn refine_space(cfg: &WorkerConfig, row: &OutboxRow, outcome: Outcome) -> Outcome {
    use k2_core::tool_hook::app_space::{add_verdict, AddVerdict};
    if Op::parse(&row.op) != Some(Op::SpaceMemberAdd) {
        return outcome;
    }
    let Outcome::Done { status, body, .. } = &outcome else {
        return outcome;
    };
    match add_verdict(&row.payload_raw, body) {
        AddVerdict::Added => outcome,
        AddVerdict::Held(why) => Outcome::Held { status: *status, body: body.clone(), why },
        AddVerdict::NotProvisioned => Outcome::Retry {
            status: Some(*status),
            error: "not added yet: the person has no chat account (their provision is still queued)".into(),
            delay: backoff(cfg, row.attempts + 1),
            target_error: None,
            stop_target: false,
        },
        AddVerdict::Unknown(why) => Outcome::Blocked { status: Some(*status), error: why },
    }
}

/// K26: does this outcome change what the People / Sew pages show?
fn visible_change(row: &OutboxRow, outcome: &Outcome) -> bool {
    match outcome {
        Outcome::Done { .. }
        | Outcome::Blocked { .. }
        | Outcome::Held { .. }
        | Outcome::BoxUsersPresent { .. }
        | Outcome::Reprovision { .. } => true,
        // Pending after its FIRST failure (later retries change nothing new).
        Outcome::Retry { .. } => row.attempts == 0,
        Outcome::UsernameRetry { .. } => false,
    }
}

// HC2 note: `principals/provision` has no `skipped` field (Sew,
// 2026-10-15). `skipped` lives on the batch Space/channel member-add
// responses (`sew_wire::AddMembersOutcome`), which K5/K31's Space-add op
// reads; S27's ops never record `held`.

/// Pure outcome rules. `now` and `first_attempt_at` are Unix seconds.
pub fn classify(
    cfg: &WorkerConfig,
    row: &OutboxRow,
    res: Result<&MgmtResponse, &TransportError>,
    now: i64,
) -> Outcome {
    let op = Op::parse(&row.op).unwrap_or(Op::Provision);
    let attempt = row.attempts + 1;
    let resp = match res {
        Ok(r) => r,
        Err(e) => {
            return Outcome::Retry {
                status: None,
                error: e.0.clone(),
                delay: backoff(cfg, attempt),
                target_error: Some(e.0.clone()),
                stop_target: true,
            }
        }
    };
    let status = resp.status;
    if resp.is_success() {
        let body = resp.body_text();
        return Outcome::Done {
            status,
            body,
            reconcile: false,
        };
    }
    let err = resp.error();
    let code = err.as_ref().map(|e| e.error.code.as_str()).unwrap_or("");
    let reason = err
        .as_ref()
        .and_then(|e| e.error.detail_str("reason"))
        .unwrap_or("");
    let field = err
        .as_ref()
        .and_then(|e| e.error.detail_str("field"))
        .unwrap_or("");
    match status {
        404 if op == Op::Deprovision => Outcome::Done {
            status,
            body: resp.body_text(),
            reconcile: false,
        },
        // A3: no such member (yet, or any more): the sweep re-plans.
        404 if matches!(op, Op::Update | Op::AvatarSet | Op::AvatarClear) => Outcome::Done {
            status,
            body: resp.body_text(),
            reconcile: true,
        },
        // S28 §12 / G11: an unknown external id (never provisioned, or
        // already gone) has no session to end. Confirmed by Sew
        // 2026-10-17 (main febfe626, `provisioning::revoke_sessions`): a
        // 404 for an unknown `external_id` means done; a `k2_sid`-only
        // revoke always answers 200 `{revoked: N}`, N ≥ 0.
        404 if op == Op::SewSessionRevoke => Outcome::Done {
            status,
            body: resp.body_text(),
            reconcile: false,
        },
        // K31: no such member or Space: nothing left to remove. Sew 0.5.0:
        // no such Space: nothing left to unbind.
        404 if matches!(op, Op::SpaceMemberRemove | Op::SpaceUnbind) => Outcome::Done {
            status,
            body: resp.body_text(),
            reconcile: false,
        },
        // Sew 0.6.0 I3: box users other than the owner are in the Space;
        // K2 removes exactly those, then switches again.
        409 if op == Op::SpaceSettings && reason == reasons::BOX_USERS_PRESENT => {
            let ids = err
                .as_ref()
                .and_then(|e| e.error.details.get("external_ids"))
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>())
                .unwrap_or_default();
            if ids.is_empty() {
                Outcome::Blocked { status: Some(status), error: format!("{}: Sew named no external_ids", resp.error_line()) }
            } else {
                Outcome::BoxUsersPresent { status, body: resp.body_text(), ids }
            }
        }
        409 if op == Op::SpaceSettings && reason == reasons::NO_GOVERNING_APP => Outcome::Blocked {
            status: Some(status),
            error: format!(
                "{}: Sew holds no governing link for this Space; bind it again (k2 hostchat app-space bind <app> <space>), then retry this row",
                resp.error_line()
            ),
        },
        // Sew 0.6.0: a channel op in a Space Sew doesn't hold as App-governed
        // yet (the switch may still be on its way): retry.
        409 if op == Op::ChannelsBulk && reason == reasons::NOT_APP_GOVERNED => Outcome::Retry {
            status: Some(status),
            error: resp.error_line(),
            delay: backoff(cfg, attempt),
            target_error: None,
            stop_target: false,
        },
        409 if code == codes::CONFLICT
            && matches!(op, Op::Provision | Op::Update)
            && (reason == reasons::USERNAME_TAKEN || field.ends_with("username")) =>
        {
            if row.idem_key.ends_with("_u") {
                let name = serde_json::from_str::<Value>(&row.payload_raw)
                    .ok()
                    .and_then(|v| {
                        v.pointer("/profile/username")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                Outcome::Blocked {
                    status: Some(status),
                    error: format!(
                        "username '{name}' is taken in chat (the fallback after the first choice was taken too); rename the other member in Sew, then retry"
                    ),
                }
            } else {
                Outcome::UsernameRetry { status }
            }
        }
        422 if reason == reasons::ASSISTANT_PERSON_UNKNOWN => {
            let first = row.first_attempt_at.unwrap_or(now);
            if (now - first) as u128 * 1000 > cfg.dep_wait.as_millis() {
                Outcome::Blocked {
                    status: Some(status),
                    error: format!(
                        "{}: the assistant's person never arrived in chat",
                        resp.error_line()
                    ),
                }
            } else {
                Outcome::Retry {
                    status: Some(status),
                    error: resp.error_line(),
                    delay: backoff(cfg, attempt),
                    target_error: None,
                    stop_target: false,
                }
            }
        }
        429 => {
            let b = backoff(cfg, attempt);
            Outcome::Retry {
                status: Some(status),
                error: resp.error_line(),
                delay: resp.retry_after().map_or(b, |ra| ra.max(b)),
                target_error: None,
                stop_target: false,
            }
        }
        401 => Outcome::Retry {
            status: Some(status),
            error: "mgmt token rejected".into(),
            delay: backoff(cfg, attempt),
            target_error: Some("mgmt token rejected".into()),
            stop_target: true,
        },
        s if s >= 500 => Outcome::Retry {
            status: Some(status),
            error: resp.error_line(),
            delay: backoff(cfg, attempt),
            target_error: Some(resp.error_line()),
            stop_target: true,
        },
        _ => Outcome::Blocked {
            status: Some(status),
            error: resp.error_line(),
        },
    }
}

fn path_for(op: Op) -> &'static str {
    match op {
        Op::Provision => sew_wire::MGMT_PROVISION,
        Op::Update => sew_wire::MGMT_UPDATE,
        Op::Deprovision => sew_wire::MGMT_DEPROVISION,
        Op::OwnerSet => sew_wire::MGMT_OWNER_SET,
        // S28 §12: `{k2_sid}` or `{external_id}`, as queued.
        Op::SewSessionRevoke => sew_wire::MGMT_SESSIONS_REVOKE,
        // K31 (SW13): App Space membership.
        Op::SpaceMemberAdd => sew_wire::MGMT_SPACE_MEMBERS_ADD,
        Op::SpaceMemberRemove => sew_wire::MGMT_SPACE_MEMBERS_REMOVE,
        // A3: the set path carries `external_id` in its query ([`send_row`]).
        Op::AvatarSet => hook::avatar::MGMT_AVATAR,
        Op::AvatarClear => hook::avatar::MGMT_AVATAR_CLEAR,
        // Decision 13: the App id is in the path ([`hook::home::path_for`]).
        Op::HomeClient => hook::home::MGMT_HOME_CLIENT,
        Op::AppSettings => "/mgmt/v1/apps/{app_id}/settings",
        // Sew 0.5.0: the managed-fields policy; a removed App's Space.
        Op::PolicySet => hook::policy::MGMT_POLICIES,
        Op::SpaceUnbind => sew_wire::MGMT_SPACES_UNBIND,
        // Sew 0.6.0: the Space (and person) are in the path
        // ([`send_sync_group`], [`hook::home::path_for`]).
        Op::SpaceMemberChannels => "/mgmt/v1/spaces/{space_id}/members/{external_id}/channels",
        Op::SpaceSettings => "/mgmt/v1/spaces/{space_id}/settings",
        Op::ChannelsBulk => sew_wire::MGMT_CHANNELS_BULK,
    }
}

/// The `Idempotency-Key` of one I4 bulk call: derived from its rows' own
/// keys in order, so a retry of the same rows sends the same key and
/// bytes, and any other set gets another key.
pub fn bulk_key(rows: &[OutboxRow]) -> String {
    use sha2::{Digest, Sha256};
    let joined = rows.iter().map(|r| r.idem_key.as_str()).collect::<Vec<_>>().join(",");
    let hex: String = Sha256::digest(joined.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    format!("k2mcb_{}", &hex[..40])
}

/// A sync verdict as the row's outcome. `body` is that person's answer.
fn sync_outcome(cfg: &WorkerConfig, row: &OutboxRow, status: u16, body: String, v: hook::governed::Verdict) -> Outcome {
    use hook::governed::Verdict as V;
    match v {
        V::Synced | V::Queued => Outcome::Done { status, body, reconcile: false },
        V::Held(why) => Outcome::Held { status, body, why },
        V::Reprovision(why) => Outcome::Reprovision { status, why },
        V::Retry(error) => Outcome::Retry {
            status: Some(status),
            error,
            delay: backoff(cfg, row.attempts + 1),
            target_error: None,
            stop_target: false,
        },
        V::Blocked(error) => Outcome::Blocked { status: Some(status), error },
    }
}

/// One sync row's outcome from Sew's whole answer to its call: transport,
/// 401, 429 and 5xx follow [`classify`]; a 2xx is that person's answer; a
/// 4xx is read per [`hook::governed::verdict_err`].
fn sync_call_outcome(cfg: &WorkerConfig, row: &OutboxRow, res: Result<&MgmtResponse, &TransportError>, now: i64) -> Outcome {
    let Ok(resp) = res else { return classify(cfg, row, res, now) };
    if resp.is_success() || resp.status == 401 || resp.status == 429 || resp.status >= 500 {
        return classify(cfg, row, res, now);
    }
    let err = resp.error();
    let code = err.as_ref().map(|e| e.error.code.clone()).unwrap_or_default();
    let reason = err.as_ref().and_then(|e| e.error.detail_str("reason").map(str::to_string)).unwrap_or_default();
    sync_outcome(cfg, row, resp.status, resp.body_text(), hook::governed::verdict_err(resp.status, &code, &reason, &resp.error_line()))
}

/// Send one Space's due sync rows: one row → I4 single; more → I4 bulk
/// (≤ 500 a call), each item read on its own. Returns whether to stop this
/// target's pass.
async fn send_sync_group(
    cfg: &WorkerConfig,
    client: &MgmtClient,
    kind: &str,
    space: &str,
    rows: &[OutboxRow],
    report: &mut PassReport,
) -> Result<bool, String> {
    use hook::governed::{verdict_ok, Verdict};
    use sew_wire::{MemberChannelsBulkItem, MemberChannelsBulkRequest, MemberChannelsBulkResponse, MemberChannelsItem, MemberChannelsRequest, MemberChannelsResponse};
    let parsed: Vec<(OutboxRow, sew_wire::ExternalId, MemberChannelsRequest)> = rows
        .iter()
        .filter_map(|r| {
            let (_, ext) = hook::governed::parse_chan_subject(&r.subject_key)?;
            let body = serde_json::from_str::<MemberChannelsRequest>(&r.payload_raw).ok()?;
            Some((r.clone(), ext, body))
        })
        .collect();
    // A row this worker can't read is a bug: block it loudly.
    for r in rows.iter().filter(|r| !parsed.iter().any(|(p, _, _)| p.id == r.id)) {
        report.attempted += 1;
        apply(kind, r, Outcome::Blocked { status: None, error: "not a channel sync row this K2 can read".into() }, report).await?;
    }
    for chunk in parsed.chunks(sew_wire::MEMBER_CHANNELS_BULK_MAX) {
        let now = k2_core::tool_hook::outbox::now_secs();
        report.attempted += chunk.len();
        if chunk.len() == 1 {
            let (row, ext, _) = &chunk[0];
            let path = sew_wire::member_channels_path(space, ext);
            let res = client.post(&path, row.payload_raw.as_bytes(), &row.idem_key, &row.actor).await;
            let outcome = match res.as_ref() {
                Ok(r) if r.is_success() => {
                    let body = r.body_text();
                    match serde_json::from_str::<MemberChannelsResponse>(&body) {
                        Ok(a) => sync_outcome(cfg, row, r.status, body, verdict_ok(&a)),
                        Err(e) => Outcome::Blocked { status: Some(r.status), error: format!("Sew's answer is not a channel sync answer ({e})") },
                    }
                }
                _ => sync_call_outcome(cfg, row, res.as_ref(), now),
            };
            if visible_change(row, &outcome) {
                report.changed += 1;
            }
            let (stop, _) = apply(kind, row, outcome, report).await?;
            if stop {
                return Ok(true);
            }
            continue;
        }
        let rows_only: Vec<OutboxRow> = chunk.iter().map(|(r, _, _)| r.clone()).collect();
        let body = MemberChannelsBulkRequest {
            app_id: chunk[0].2.app_id.clone(),
            items: chunk
                .iter()
                .map(|(_, ext, b)| MemberChannelsItem { external_id: ext.clone(), channel_ids: b.channel_ids.clone(), level: b.level.clone() })
                .collect(),
        };
        let bytes = serde_json::to_vec(&body).map_err(|e| format!("bulk body: {e}"))?;
        let key = bulk_key(&rows_only);
        let res = client.post(&sew_wire::member_channels_bulk_path(space), &bytes, &key, &chunk[0].0.actor).await;
        let answers: Option<MemberChannelsBulkResponse> = match res.as_ref() {
            Ok(r) if r.is_success() => serde_json::from_slice(&r.body).ok(),
            _ => None,
        };
        let mut stop = false;
        for (row, ext, _) in chunk {
            let outcome = match (res.as_ref(), answers.as_ref()) {
                (Ok(r), Some(a)) if r.is_success() => match a.items.iter().find(|i: &&MemberChannelsBulkItem| i.external_id == *ext) {
                    // Sew 0.6.0: `{external_id, status, answer}` or
                    // `{external_id, status, error}`. The row's result is
                    // the person's own answer (what a single sync answers).
                    Some(item) => match (&item.answer, &item.error) {
                        (_, Some(e)) => {
                            let item_body = serde_json::to_string(item).unwrap_or_default();
                            sync_outcome(cfg, row, item.status, item_body, hook::governed::verdict_err(item.status, &e.code, e.reason(), &format!("{} {}", e.code, e.hint)))
                        }
                        (Some(answer), None) => {
                            let answer_body = serde_json::to_string(answer).unwrap_or_default();
                            sync_outcome(cfg, row, item.status, answer_body, verdict_ok(answer))
                        }
                        (None, None) => Outcome::Blocked {
                            status: Some(item.status),
                            error: format!("Sew's bulk item for {ext} has neither an answer nor an error (status {})", item.status),
                        },
                    },
                    None => Outcome::Blocked { status: Some(r.status), error: format!("Sew's bulk answer named no item for {ext}") },
                },
                (Ok(r), None) if r.is_success() => Outcome::Blocked { status: Some(r.status), error: "Sew's answer is not a bulk channel sync answer".into() },
                _ => sync_call_outcome(cfg, row, res.as_ref(), now),
            };
            if let Outcome::Done { body, .. } = &outcome {
                if serde_json::from_str::<MemberChannelsResponse>(body).is_ok_and(|a| verdict_ok(&a) == Verdict::Queued) {
                    log_debug!("[grant-hook] {ext} in Space {space}: Sew runs the change as a job (queued)");
                }
            }
            if visible_change(row, &outcome) {
                report.changed += 1;
            }
            let (s, _) = apply(kind, row, outcome, report).await?;
            stop |= s;
        }
        if stop {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A retry that keeps this target's pass going (the row waits, nothing was
/// sent or Sew answered for this row alone).
fn row_retry(cfg: &WorkerConfig, row: &OutboxRow, status: Option<u16>, error: String) -> Outcome {
    Outcome::Retry { status, error, delay: backoff(cfg, row.attempts + 1), target_error: None, stop_target: false }
}

/// Sew 0.6.0 A23 (`home_client_owner_approval`): send one home-client row.
/// With the flag, the body carries the install owner's approval, a fresh
/// `k2:owner` chat pass minted here for this one send under its own
/// `Idempotency-Key` ([`crate::sew::home`]), and only for a setting the
/// owner made ([`crate::sew::home::owner_actor`]). Without the flag the
/// row goes as before. `Ok((answer, approved))`; `Err` = nothing was sent.
async fn home_client_send(
    cfg: &WorkerConfig,
    client: &MgmtClient,
    row: &OutboxRow,
) -> Result<(Result<MgmtResponse, TransportError>, bool), Outcome> {
    let path = hook::home::MGMT_HOME_CLIENT;
    let flags = blocking(hook::sew_features).await.map_err(|e| row_retry(cfg, row, None, e))?;
    if !flags.iter().any(|f| f == hook::features::HOME_CLIENT_OWNER_APPROVAL) {
        return Ok((client.post(path, row.payload_raw.as_bytes(), &row.idem_key, &row.actor).await, false));
    }
    let actor = row.actor.clone();
    let owner = blocking(move || Ok(crate::sew::home::owner_actor(&actor, &k2_core::connect_users::role_for_user)))
        .await
        .map_err(|e| row_retry(cfg, row, None, e))?;
    if !owner {
        crate::vault::auth_audit(crate::sew::home::AUDIT_HOME_CLIENT, &row.actor, "approval refused: not set by the owner", "worker");
        return Err(Outcome::Blocked {
            status: None,
            error: format!(
                "the home client was set by {}, not this server's owner, so K2 won't send the owner's approval for it; \
                 the owner sets it again (k2 hostchat home client <app|none>)",
                row.actor
            ),
        });
    }
    // The install id every time (a reinstall changes it; the pass names it).
    let status = match client.get(hook::sew_wire::MGMT_STATUS).await {
        Ok(r) if r.is_success() => r,
        Ok(r) if r.status == 401 => {
            return Err(Outcome::Retry {
                status: Some(401),
                error: "mgmt token rejected".into(),
                delay: backoff(cfg, row.attempts + 1),
                target_error: Some("mgmt token rejected".into()),
                stop_target: true,
            })
        }
        Ok(r) => return Err(row_retry(cfg, row, Some(r.status), format!("Sew's status (for the owner's approval): {}", r.error_line()))),
        Err(e) => {
            return Err(Outcome::Retry {
                status: None,
                error: e.0.clone(),
                delay: backoff(cfg, row.attempts + 1),
                target_error: Some(e.0),
                stop_target: true,
            })
        }
    };
    let install_id = serde_json::from_slice::<sew_wire::Status>(&status.body)
        .map(|s| s.install_id)
        .map_err(|e| row_retry(cfg, row, Some(status.status), format!("Sew's status does not parse ({e})")))?;
    let now = k2_core::tool_hook::outbox::now_secs();
    let claims = crate::sew::home::approval_claims(&install_id, now).map_err(|e| row_retry(cfg, row, None, format!("owner approval: {e}")))?;
    let jti = claims.jti.clone();
    let pass = blocking(move || crate::chat_pass::sign_chat_pass(&claims).map_err(|e| e.to_string()))
        .await
        .map_err(|e| row_retry(cfg, row, None, format!("the owner's approval could not be signed (chat-pass keys: {e})")))?;
    let body = crate::sew::home::with_approval(&row.payload_raw, &pass).map_err(|e| Outcome::Blocked { status: None, error: e })?;
    crate::vault::auth_audit(crate::sew::home::AUDIT_HOME_CLIENT, &row.actor, &format!("approval minted jti:{jti}"), "worker");
    Ok((client.post(path, &body, &crate::sew::home::approval_key(&jti), &row.actor).await, true))
}

/// Sew 0.6.0: a home-client row's refusals, said clearly
/// ([`crate::sew::home::sew_refusal`]). Returns the outcome and whether to
/// read Sew's flags again (a `403 owner_approval_required` to a send
/// without the approval: Sew gained the flag since the last sweep). An
/// approved send Sew still refuses retries for `dep_wait` (1 h; after a
/// reinstall the owner's own `owner/set` may not have landed yet, and Sew
/// counts "no owner" as no approval), then blocks with the clear error.
pub fn refine_home(
    cfg: &WorkerConfig,
    row: &OutboxRow,
    res: Result<&MgmtResponse, &TransportError>,
    outcome: Outcome,
    approved: bool,
    now: i64,
) -> (Outcome, bool) {
    if Op::parse(&row.op) != Some(Op::HomeClient) {
        return (outcome, false);
    }
    let Ok(r) = res else { return (outcome, false) };
    if r.is_success() {
        return (outcome, false);
    }
    let reason = r.error().and_then(|e| e.error.detail_str("reason").map(str::to_string)).unwrap_or_default();
    let app_id = serde_json::from_str::<Value>(&row.payload_raw)
        .ok()
        .and_then(|v| v.get("app_id").and_then(Value::as_str).map(str::to_string));
    match crate::sew::home::sew_refusal(r.status, &reason, app_id.as_deref(), approved) {
        Some((true, error)) if r.status == 403 => {
            let first = row.first_attempt_at.unwrap_or(now);
            if (now - first).max(0) as u128 * 1000 > cfg.dep_wait.as_millis() {
                (Outcome::Blocked { status: Some(r.status), error }, false)
            } else {
                (row_retry(cfg, row, Some(r.status), error), false)
            }
        }
        Some((true, error)) => (Outcome::Blocked { status: Some(r.status), error }, false),
        Some((false, error)) => (row_retry(cfg, row, Some(r.status), error), true),
        None => (outcome, false),
    }
}

/// A8 (b): the filled body for a provision row, `Ok(None)` when nothing
/// changes (a new member, or Sew's list carries none of the fields), or the
/// retry outcome when the lookup itself failed.
async fn fill_provision(
    cfg: &WorkerConfig,
    client: &MgmtClient,
    row: &OutboxRow,
) -> Result<Option<Value>, Outcome> {
    let retry =
        |error: String, status: Option<u16>, target: bool, delay: Duration| Outcome::Retry {
            status,
            error: format!("member lookup: {error}"),
            delay,
            target_error: target.then(|| format!("member lookup: {error}")),
            stop_target: target,
        };
    // A bug-class answer (a 4xx, a body that is not a principals page):
    // fail loudly, never send a body that could clear what K2 can't see.
    let blocked = |error: String, status: Option<u16>| Outcome::Blocked {
        status,
        error: format!("member lookup: {error}"),
    };
    let b = backoff(cfg, row.attempts + 1);
    let mut req: sew_wire::ProvisionRequest = match serde_json::from_str(&row.payload_raw) {
        Ok(r) => r,
        // Not a provision body this worker built: send it unchanged and
        // let Sew's answer decide (a 400 blocks it loudly).
        Err(_) => return Ok(None),
    };
    let path = sew_wire::principal_lookup_path(&req.external_id);
    let resp = client
        .get(&path)
        .await
        .map_err(|e| retry(e.0, None, true, b))?;
    match resp.status {
        s if (200..300).contains(&s) => {}
        401 => return Err(retry("mgmt token rejected".into(), Some(401), true, b)),
        429 => {
            let d = resp.retry_after().map_or(b, |ra| ra.max(b));
            return Err(retry(resp.error_line(), Some(429), false, d));
        }
        s if s >= 500 => return Err(retry(resp.error_line(), Some(s), true, b)),
        s => return Err(blocked(resp.error_line(), Some(s))),
    }
    let page: sew_wire::MgmtPrincipalPage = serde_json::from_slice(&resp.body)
        .map_err(|e| blocked(format!("unexpected body ({e})"), Some(resp.status)))?;
    let Some(m) = page.items.iter().find(|m| m.external_id == req.external_id) else {
        return Ok(None);
    };
    if hook::chat::fill_provision_from_member(&mut req, m) {
        let v = serde_json::to_value(&req).map_err(|e| blocked(format!("encode ({e})"), None))?;
        Ok(Some(v))
    } else {
        Ok(None)
    }
}

/// The username-conflict rewrite: `<base>-<hhhh>` in `profile.username`.
fn suffixed_payload(row: &OutboxRow) -> Option<Value> {
    let mut v: Value = serde_json::from_str(&row.payload_raw).ok()?;
    let base = v.pointer("/profile/username")?.as_str()?.to_string();
    let new = hook::chat::suffixed_username(&base, &row.subject_key);
    *v.pointer_mut("/profile/username")? = Value::String(new);
    Some(v)
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("grant hook task: {e}"))?
}

/// Counts from one delivery pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PassReport {
    pub attempted: usize,
    pub done: usize,
    pub retried: usize,
    /// A8: provision bodies filled from the member list before a first send.
    pub filled: usize,
    pub blocked: usize,
    /// K31: Space adds held (HC2).
    pub held: usize,
    /// K26: rows whose state changed in a way a page shows.
    pub changed: usize,
}

/// One delivery pass over every enabled target. Sequential per target,
/// never holding the skin.db mutex across an `.await`.
pub async fn pass_once(cfg: &WorkerConfig) -> Result<PassReport, String> {
    let mut report = PassReport::default();
    let targets = blocking(hook::list_targets).await?;
    for t in targets.into_iter().filter(|t| t.enabled) {
        let client = MgmtClient::new(&t.socket, &t.token_file);
        let kind = t.kind.clone();
        let rows = {
            let kind = kind.clone();
            blocking(move || hook::due(&kind, sew_wire::MEMBER_CHANNELS_BULK_MAX)).await?
        };
        // Sew 0.6.0: a Space's due channel syncs go together (I4 bulk);
        // everything else row by row, as before (at most 100 a pass).
        let (sync, rows): (Vec<OutboxRow>, Vec<OutboxRow>) =
            rows.into_iter().partition(|r| Op::parse(&r.op) == Some(Op::SpaceMemberChannels));
        let rows: Vec<OutboxRow> = rows.into_iter().take(100).collect();
        let mut groups: Vec<(String, Vec<OutboxRow>)> = Vec::new();
        for r in sync {
            let space = hook::governed::parse_chan_subject(&r.subject_key).map(|(s, _)| s).unwrap_or_default();
            match groups.iter_mut().find(|(s, _)| *s == space) {
                Some((_, g)) => g.push(r),
                None => groups.push((space, vec![r])),
            }
        }
        let mut stopped = false;
        for (space, g) in &groups {
            if send_sync_group(cfg, &client, &kind, space, g, &mut report).await? {
                stopped = true;
                break;
            }
        }
        if stopped {
            continue;
        }
        for row in rows {
            let Some(op) = Op::parse(&row.op).filter(|o| o.delivered_by_s27()) else {
                continue;
            };
            report.attempted += 1;
            let mut row = row;
            if op == Op::Provision && row.filled_at.is_none() {
                match fill_provision(cfg, &client, &row).await {
                    Ok(Some(filled)) => {
                        report.filled += 1;
                        let (id, body) = (row.id.clone(), filled.clone());
                        blocking(move || hook::record_filled(&id, Some(&body))).await?;
                        row.payload_raw = filled.to_string();
                    }
                    Ok(None) => {
                        let id = row.id.clone();
                        blocking(move || hook::record_filled(&id, None)).await?;
                    }
                    Err(outcome) => {
                        // The lookup failed: nothing was sent. Same retry
                        // rules as a failed send (transport/5xx/401 stop
                        // this target's pass).
                        let (stop, _) = apply(&kind, &row, outcome, &mut report).await?;
                        if stop {
                            break;
                        }
                        continue;
                    }
                }
            }
            // Sew 0.6.0 A23: the home client goes with the owner's approval.
            let mut approved = false;
            let res = if op == Op::HomeClient {
                match home_client_send(cfg, &client, &row).await {
                    Ok((r, a)) => {
                        approved = a;
                        r
                    }
                    Err(outcome) => {
                        if visible_change(&row, &outcome) {
                            report.changed += 1;
                        }
                        let (stop, _) = apply(&kind, &row, outcome, &mut report).await?;
                        if stop {
                            break;
                        }
                        continue;
                    }
                }
            } else if op == Op::AvatarSet {
                // A3: the raw picture, read now; a superseded row (the
                // picture changed or went) finishes unsent.
                let raw = row.payload_raw.clone();
                match blocking(move || hook::avatar_bytes(&raw)).await {
                    Ok(Some(bytes)) => {
                        let ext = row.subject_key.clone();
                        client
                            .post_typed(&hook::avatar::set_path(&ext, hook::avatar::row_force(&row.payload_raw)), &bytes, hook::avatar::CONTENT_TYPE, &row.idem_key, &row.actor)
                            .await
                    }
                    Ok(None) => {
                        report.done += 1;
                        let id = row.id.clone();
                        blocking(move || hook::record_superseded(&id)).await?;
                        continue;
                    }
                    Err(e) => {
                        report.blocked += 1;
                        let id = row.id.clone();
                        blocking(move || hook::record_blocked(&id, None, &e)).await?;
                        continue;
                    }
                }
            } else {
                let path = hook::home::path_for(op, &row.subject_key).unwrap_or_else(|| path_for(op).to_string());
                client.post(&path, row.payload_raw.as_bytes(), &row.idem_key, &row.actor).await
            };
            let now = k2_core::tool_hook::outbox::now_secs();
            let outcome = refine_space(cfg, &row, classify(cfg, &row, res.as_ref(), now));
            let (outcome, reread_flags) = refine_home(cfg, &row, res.as_ref(), outcome, approved, now);
            if reread_flags {
                hook::kick_reconcile();
            }
            if visible_change(&row, &outcome) {
                report.changed += 1;
            }
            let (stop, reconcile) = apply(&kind, &row, outcome, &mut report).await?;
            if reconcile {
                hook::kick_reconcile();
            }
            if stop {
                break;
            }
        }
    }
    // K26: one refetch signal per pass, never per row.
    if report.changed > 0 {
        crate::sew::s32::emit("grant-hook");
    }
    Ok(report)
}

async fn apply(
    kind: &str,
    row: &OutboxRow,
    outcome: Outcome,
    report: &mut PassReport,
) -> Result<(bool, bool), String> {
    let id = row.id.clone();
    let kind = kind.to_string();
    log_debug!(
        "[grant-hook] {} op={} subject={} attempt={} → {:?}",
        row.tool,
        row.op,
        row.subject_key,
        row.attempts + 1,
        match &outcome {
            Outcome::Done { status, .. } => format!("done {status}"),
            Outcome::UsernameRetry { status } => format!("username retry {status}"),
            Outcome::Retry { status, error, .. } => format!("retry {status:?} {error}"),
            Outcome::Blocked { status, error } => format!("blocked {status:?} {error}"),
            Outcome::Held { status, why, .. } => format!("held {status} {why}"),
            Outcome::BoxUsersPresent { status, ids, .. } => format!("box users present {status} {ids:?}"),
            Outcome::Reprovision { status, why } => format!("reprovision {status} {why}"),
        }
    );
    match outcome {
        Outcome::Done {
            status,
            body,
            reconcile,
        } => {
            report.done += 1;
            blocking(move || {
                hook::record_done(&id, status, &body)?;
                hook::set_health(&kind, true, None)
            })
            .await?;
            Ok((false, reconcile))
        }
        Outcome::UsernameRetry { status } => {
            let Some(payload) = suffixed_payload(row) else {
                report.blocked += 1;
                blocking(move || {
                    hook::record_blocked(
                        &id,
                        Some(status),
                        "username taken; no username in the body",
                    )
                })
                .await?;
                return Ok((false, false));
            };
            report.retried += 1;
            let key = format!("{}_u", row.idem_key);
            blocking(move || hook::record_username_retry(&id, &payload, &key, status)).await?;
            Ok((false, false))
        }
        Outcome::Retry {
            status,
            error,
            delay,
            target_error,
            stop_target,
        } => {
            report.retried += 1;
            let delay_secs = secs_ceil(delay);
            blocking(move || {
                hook::record_retry(&id, status, &error, delay_secs)?;
                if let Some(te) = target_error {
                    hook::set_health(&kind, false, Some(&te))?;
                }
                Ok(())
            })
            .await?;
            Ok((stop_target, false))
        }
        Outcome::Blocked { status, error } => {
            report.blocked += 1;
            blocking(move || hook::record_blocked(&id, status, &error)).await?;
            Ok((false, false))
        }
        Outcome::Held { status, body, why } => {
            report.held += 1;
            blocking(move || hook::record_held(&id, status, &body, &why)).await?;
            Ok((false, false))
        }
        Outcome::BoxUsersPresent { status, body, ids } => {
            report.done += 1;
            blocking(move || hook::record_box_users_present(&id, status, &body, &ids).map(|_| ())).await?;
            Ok((false, false))
        }
        Outcome::Reprovision { status, why } => {
            report.retried += 1;
            blocking(move || hook::record_reprovision(&id, status, &why)).await?;
            Ok((false, false))
        }
    }
}

/// Counts from one reconcile sweep, per target.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub swept: usize,
    pub failed: Vec<String>,
}

/// One reconcile sweep over every enabled target (§16.2).
pub async fn reconcile_once() -> Result<SweepReport, String> {
    let mut report = SweepReport::default();
    let targets = blocking(hook::list_targets).await?;
    for t in targets.into_iter().filter(|t| t.enabled) {
        let client = MgmtClient::new(&t.socket, &t.token_file);
        let kind = t.kind.clone();
        match sweep_target(&client, &kind).await {
            Ok(()) => report.swept += 1,
            Err(e) => {
                log_debug!("[grant-hook] reconcile {kind} failed: {e}");
                let k = kind.clone();
                let e2 = e.clone();
                blocking(move || {
                    hook::set_health(&k, false, Some(&e2))?;
                    hook::set_reconcile_error(&k, &e2)
                })
                .await?;
                report.failed.push(format!("{kind}: {e}"));
            }
        }
    }
    Ok(report)
}

async fn get_json<T: serde::de::DeserializeOwned>(
    client: &MgmtClient,
    path: &str,
) -> Result<T, String> {
    let r = client.get(path).await.map_err(|e| e.0)?;
    if r.status == 401 {
        return Err("mgmt token rejected".into());
    }
    if !r.is_success() {
        return Err(r.error_line());
    }
    serde_json::from_slice(&r.body).map_err(|e| format!("{path}: unexpected body ({e})"))
}

async fn sweep_target(client: &MgmtClient, kind: &str) -> Result<(), String> {
    let status: sew_wire::Status = get_json(client, sew_wire::MGMT_STATUS).await?;
    if status.provider != "k2" {
        return Err("Sew is in standalone mode".into());
    }
    {
        let k = kind.to_string();
        let id = status.install_id.clone();
        blocking(move || hook::note_install_id(&k, &id)).await?;
    }
    let mut actual = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut path = format!(
            "{}?limit={}",
            sew_wire::MGMT_PRINCIPALS,
            sew_wire::PRINCIPALS_LIMIT_MAX
        );
        if let Some(a) = &after {
            path.push_str(&format!("&after={a}"));
        }
        let page: sew_wire::MgmtPrincipalPage = get_json(client, &path).await?;
        let last = page.items.last().map(|m| m.member_id.clone());
        actual.extend(page.items);
        if !page.has_more {
            break;
        }
        match last {
            Some(l) if Some(&l) != after.as_ref() => after = Some(l),
            _ => return Err("principals paging did not advance".into()),
        }
    }
    // Sew 0.5.0 (`k2_managed_avatar`): read the managed-fields policy so
    // the settings step can make it K2's. A failed read is not the sweep's
    // failure: K2 keeps what it last saw and reads again next sweep.
    let policy_read = if status.features.iter().any(|f| f == hook::features::K2_MANAGED_AVATAR) {
        match policy_read(client).await {
            Ok(v) => hook::PolicyRead::Seen(v),
            Err(e) => {
                log_debug!("[grant-hook] {kind}: policy read failed: {e}");
                hook::PolicyRead::NotRead
            }
        }
    } else {
        hook::PolicyRead::NotRead
    };
    let k = kind.to_string();
    blocking(move || {
        hook::set_health(&k, true, None)?;
        // Sew 0.6.0: each bound App's manifest chat block, then members
        // first (a keyed App's users get their chat grant, which provisions
        // them before any channel sync).
        crate::sew::governed::refresh_facts();
        match hook::governed_auto_grants() {
            Ok((0, 0)) => {}
            Ok((a, r)) => log_debug!("[grant-hook] chat-key members: {a} granted, {r} removed"),
            Err(e) => log_debug!("[grant-hook] chat-key members: {e}"),
        }
        hook::apply_reconcile(&k, &actual)?;
        // Decision 13: each chat App's signalling origin, then the settings
        // Sew's feature flags say it takes (never by version).
        crate::sew::home::refresh_origins();
        hook::apply_settings(&k, &status.features, &policy_read).map(|_| ())
    })
    .await
}

/// `GET /mgmt/v1/policies` → the `identity.managed_fields` value (`None` =
/// absent, Sew's default).
async fn policy_read(client: &MgmtClient) -> Result<Option<Value>, String> {
    let r = client.get(hook::policy::MGMT_POLICIES).await.map_err(|e| e.0)?;
    if !r.is_success() {
        return Err(r.error_line());
    }
    hook::policy::managed_fields_in(&r.body)
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Spawn the worker with knobs from the environment. Single instance per
/// process (a second spawn is a no-op task).
pub fn spawn() -> JoinHandle<()> {
    spawn_with(WorkerConfig::from_env())
}

pub fn spawn_with(cfg: WorkerConfig) -> JoinHandle<()> {
    tokio::spawn(async move {
        if RUNNING.swap(true, Ordering::SeqCst) {
            log_debug!("[grant-hook] worker already running; not starting a second one");
            return;
        }
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                RUNNING.store(false, Ordering::SeqCst);
            }
        }
        let _reset = Reset;
        run(cfg).await
    })
}

async fn run(cfg: WorkerConfig) {
    log_debug!(
        "[grant-hook] worker started — tick={}ms reconcile={}ms",
        cfg.tick.as_millis(),
        cfg.reconcile_every.as_millis()
    );
    // Boot: a sweep first, then deliver (rows queued before a restart).
    let mut last_reconcile: Option<Instant> = None;
    let mut want_reconcile = true;
    let mut last_prune: Option<Instant> = None;
    loop {
        if hook::take_reconcile_request() {
            want_reconcile = true;
        }
        let due_by_clock = last_reconcile.is_none_or(|t| t.elapsed() >= cfg.reconcile_every);
        let debounced = last_reconcile.is_none_or(|t| t.elapsed() >= cfg.reconcile_debounce);
        if (want_reconcile && debounced) || due_by_clock {
            if let Err(e) = reconcile_once().await {
                log_debug!("[grant-hook] reconcile error: {e}");
            }
            last_reconcile = Some(Instant::now());
            want_reconcile = false;
        }
        if let Err(e) = pass_once(&cfg).await {
            log_debug!("[grant-hook] pass error: {e}");
        }
        if last_prune.is_none_or(|t| t.elapsed() >= Duration::from_secs(24 * 3600)) {
            match blocking(hook::prune).await {
                Ok(n) if n > 0 => log_debug!("[grant-hook] pruned {n} finished rows"),
                Ok(_) => {}
                Err(e) => log_debug!("[grant-hook] prune error: {e}"),
            }
            last_prune = Some(Instant::now());
        }
        tokio::select! {
            _ = hook::wait_for_kick() => {}
            _ = tokio::time::sleep(cfg.tick) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> WorkerConfig {
        WorkerConfig {
            tick: Duration::from_millis(50),
            backoff_base: Duration::from_millis(2_000),
            backoff_cap: Duration::from_millis(300_000),
            reconcile_every: Duration::from_secs(600),
            reconcile_debounce: Duration::from_secs(5),
            dep_wait: Duration::from_secs(3600),
        }
    }

    fn row(op: &str, attempts: i64, idem: &str) -> OutboxRow {
        OutboxRow {
            id: "r1".into(),
            seq: 1,
            tool: "chat".into(),
            op: op.into(),
            subject_key: "k2:principal:0f8fad5b-d9cb-469f-a165-70867728950e".into(),
            payload: None,
            payload_raw: r#"{"external_id":"k2:principal:0f8fad5b-d9cb-469f-a165-70867728950e","kind":"person","guest":false,"profile":{"display_name":"Ann","username":"ann"}}"#.into(),
            idem_key: idem.into(),
            grant_id: None,
            grant_rev: None,
            cause: "grant_create".into(),
            actor: "owner-token".into(),
            after_subject: None,
            state: "pending".into(),
            attempts,
            first_attempt_at: if attempts > 0 { Some(1_000) } else { None },
            next_attempt_at: 0,
            last_status: None,
            last_error: None,
            result: None,
            created_at: 0,
            updated_at: 0,
            done_at: None,
            filled_at: None,
        }
    }

    fn resp(status: u16, body: &str, extra: &str) -> MgmtResponse {
        crate::sew_mgmt::parse_response(
            format!(
                "HTTP/1.1 {status} X\r\n{extra}Content-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .unwrap()
    }

    fn err(code: &str, reason: &str, field: &str) -> String {
        serde_json::json!({"ok":false,"error":{"code":code,"hint":"h","details":{"reason":reason,"field":field}}}).to_string()
    }

    #[test]
    fn backoff_doubles_caps_and_jitters_within_20_percent() {
        let c = cfg();
        for (attempt, base_ms) in [(1, 2_000u64), (2, 4_000), (3, 8_000), (20, 300_000)] {
            let d = backoff(&c, attempt).as_millis() as u64;
            assert!(
                d >= base_ms * 8 / 10 && d <= base_ms * 12 / 10,
                "attempt {attempt}: {d}"
            );
        }
    }

    #[test]
    fn outcome_table() {
        let c = cfg();
        let r = row("provision", 0, "k2ob_x");
        // 2xx → done
        assert!(matches!(
            classify(
                &c,
                &r,
                Ok(&resp(200, r#"{"member_id":"7","created":true}"#, "")),
                0
            ),
            Outcome::Done {
                reconcile: false,
                ..
            }
        ));
        // HC2 lives on member-add responses, not provision: a stray
        // `skipped` on a provision body changes nothing.
        let stray = resp(200, r#"{"member_id":"7","skipped":["7"]}"#, "");
        assert!(matches!(
            classify(&c, &r, Ok(&stray), 0),
            Outcome::Done { .. }
        ));
        // 404 deprovision → done; 404 update → done + reconcile; 404 provision → blocked
        assert!(matches!(
            classify(
                &c,
                &row("deprovision", 0, "k"),
                Ok(&resp(404, &err("not_found", "", ""), "")),
                0
            ),
            Outcome::Done {
                reconcile: false,
                ..
            }
        ));
        assert!(matches!(
            classify(
                &c,
                &row("update", 0, "k"),
                Ok(&resp(404, &err("not_found", "", ""), "")),
                0
            ),
            Outcome::Done {
                reconcile: true,
                ..
            }
        ));
        // S28 §12 / G11: a session revoke for an unknown member is done;
        // its 200 with any count too; a 5xx retries like every op.
        let rv = row("sew_session_revoke", 0, "k");
        assert!(matches!(
            classify(&c, &rv, Ok(&resp(404, &err("not_found", "", ""), "")), 0),
            Outcome::Done {
                reconcile: false,
                ..
            }
        ));
        assert!(matches!(
            classify(&c, &rv, Ok(&resp(200, r#"{"revoked":0}"#, "")), 0),
            Outcome::Done { .. }
        ));
        assert!(matches!(
            classify(&c, &rv, Ok(&resp(503, &err("unavailable", "", ""), "")), 0),
            Outcome::Retry {
                stop_target: true,
                ..
            }
        ));
        assert!(matches!(
            classify(&c, &r, Ok(&resp(404, &err("not_found", "", ""), "")), 0),
            Outcome::Blocked { .. }
        ));
        // 409 username (S27-A6 field profile.username) → suffix once, then blocked
        let taken = resp(
            409,
            &err("conflict", "username_taken", "profile.username"),
            "",
        );
        assert_eq!(
            classify(&c, &r, Ok(&taken), 0),
            Outcome::UsernameRetry { status: 409 }
        );
        assert!(
            matches!(classify(&c, &row("provision", 1, "k2ob_x_u"), Ok(&taken), 0), Outcome::Blocked { ref error, .. } if error.starts_with("username 'ann' is taken in chat (the fallback"))
        );
        // 409 idempotency_conflict → blocked
        assert!(matches!(
            classify(
                &c,
                &r,
                Ok(&resp(409, &err("idempotency_conflict", "", ""), "")),
                0
            ),
            Outcome::Blocked { .. }
        ));
        // 422 person unknown → retry within 1 h, blocked after (S27-A7)
        let unknown = resp(
            422,
            &err("invalid_state", "assistant_person_unknown", "assistant_of"),
            "",
        );
        assert!(matches!(
            classify(&c, &row("provision", 1, "k"), Ok(&unknown), 1_000 + 60),
            Outcome::Retry {
                target_error: None,
                ..
            }
        ));
        assert!(matches!(
            classify(&c, &row("provision", 9, "k"), Ok(&unknown), 1_000 + 3_601),
            Outcome::Blocked { .. }
        ));
        // 422 person inactive → blocked at once
        let inactive = resp(
            422,
            &err("invalid_state", "assistant_person_inactive", "assistant_of"),
            "",
        );
        assert!(matches!(
            classify(&c, &r, Ok(&inactive), 0),
            Outcome::Blocked { .. }
        ));
        // 429 honours Retry-After when larger than backoff
        match classify(
            &c,
            &r,
            Ok(&resp(
                429,
                &err("rate_limited", "", ""),
                "Retry-After: 30\r\n",
            )),
            0,
        ) {
            Outcome::Retry {
                delay,
                stop_target: false,
                target_error: None,
                ..
            } => assert_eq!(delay, Duration::from_secs(30)),
            o => panic!("429 → {o:?}"),
        }
        // 401 → retry, target unhealthy, stop
        assert!(
            matches!(classify(&c, &r, Ok(&resp(401, &err("unauthenticated", "", ""), "")), 0), Outcome::Retry { stop_target: true, target_error: Some(ref e), .. } if e == "mgmt token rejected")
        );
        // 5xx → retry, unhealthy, stop
        assert!(matches!(
            classify(&c, &r, Ok(&resp(503, &err("unavailable", "", ""), "")), 0),
            Outcome::Retry {
                stop_target: true,
                target_error: Some(_),
                ..
            }
        ));
        // transport → retry, unhealthy, stop
        assert!(matches!(
            classify(&c, &r, Err(&TransportError("connect: refused".into())), 0),
            Outcome::Retry {
                status: None,
                stop_target: true,
                ..
            }
        ));
        // other 4xx → blocked with code: hint
        assert!(
            matches!(classify(&c, &r, Ok(&resp(400, &err("validation_failed", "", "profile"), "")), 0), Outcome::Blocked { ref error, .. } if error == "validation_failed: h")
        );
    }

    #[test]
    fn username_suffix_payload_rewrites_only_the_username() {
        let r = row("provision", 0, "k2ob_x");
        let v = suffixed_payload(&r).expect("rewrites");
        let u = v.pointer("/profile/username").unwrap().as_str().unwrap();
        assert!(u.starts_with("ann-") && u.len() == 8, "{u}");
        assert_eq!(v["external_id"], serde_json::json!(r.subject_key));
        assert_eq!(v.pointer("/profile/display_name").unwrap(), "Ann");
    }

    fn space_row(op: &str, attempts: i64) -> OutboxRow {
        let ext = "k2:principal:0f8fad5b-d9cb-469f-a165-70867728950e";
        let mut r = row(op, attempts, "k2ob_s");
        r.subject_key = format!("space:spc_1:{ext}");
        r.payload_raw = if op == "space_member_add" {
            format!(r#"{{"space_id":"spc_1","app_id":"hearth","external_ids":["{ext}"]}}"#)
        } else {
            format!(r#"{{"space_id":"spc_1","app_id":"hearth","external_id":"{ext}"}}"#)
        };
        r
    }

    /// K31: how a 2xx `spaces/members/add` answer maps to the row.
    #[test]
    fn space_add_answers_map_to_done_retry_held_or_blocked() {
        let c = cfg();
        let ext = "k2:principal:0f8fad5b-d9cb-469f-a165-70867728950e";
        let r = space_row("space_member_add", 0);
        let run = |body: &str| refine_space(&c, &r, classify(&c, &r, Ok(&resp(200, body, "")), 0));
        let added = format!(r#"{{"added":[{{"external_id":"{ext}","member_id":"7"}}],"already":[],"skipped":[],"not_added":[]}}"#);
        assert!(matches!(run(&added), Outcome::Done { status: 200, .. }));
        let already = format!(r#"{{"added":[],"already":[{{"external_id":"{ext}","member_id":"7"}}],"skipped":[],"not_added":[]}}"#);
        assert!(matches!(run(&already), Outcome::Done { .. }));
        let np = format!(r#"{{"added":[],"already":[],"skipped":[],"not_added":[{{"external_id":"{ext}","reason":"not_provisioned"}}]}}"#);
        match run(&np) {
            Outcome::Retry { status: Some(200), stop_target: false, target_error: None, delay, .. } => {
                assert!(delay >= Duration::from_millis(1_600), "backoff, not a hot loop: {delay:?}")
            }
            o => panic!("not_provisioned → {o:?}"),
        }
        let guest = format!(r#"{{"added":[],"already":[],"skipped":[],"not_added":[{{"external_id":"{ext}","reason":"guest"}}]}}"#);
        assert!(matches!(run(&guest), Outcome::Held { status: 200, ref why, .. } if why == "not added: guest"));
        let banned = r#"{"added":[],"already":[],"skipped":["7"],"not_added":[]}"#;
        assert!(matches!(run(banned), Outcome::Held { ref why, .. } if why.contains("banned")));
        assert!(matches!(run(r#"{"added":[],"already":[],"skipped":[],"not_added":[]}"#), Outcome::Blocked { status: Some(200), .. }));
        // Non-2xx and other ops pass through untouched.
        let app_mismatch = resp(409, &err("conflict", "app_mismatch", ""), "");
        assert!(matches!(refine_space(&c, &r, classify(&c, &r, Ok(&app_mismatch), 0)), Outcome::Blocked { status: Some(409), .. }));
        let p = row("provision", 0, "k2ob_p");
        assert!(matches!(refine_space(&c, &p, classify(&c, &p, Ok(&resp(200, "{}", "")), 0)), Outcome::Done { .. }));
        // Remove: 404 (no such Space or member) is done.
        let rm = space_row("space_member_remove", 0);
        assert!(matches!(
            classify(&c, &rm, Ok(&resp(404, &err("not_found", "", ""), "")), 0),
            Outcome::Done { status: 404, reconcile: false, .. }
        ));
        assert_eq!(path_for(Op::SpaceMemberAdd), "/mgmt/v1/spaces/members/add");
        assert_eq!(path_for(Op::SpaceMemberRemove), "/mgmt/v1/spaces/members/remove");
    }

    /// Sew 0.5.0 rows: where they go, and how Sew's answers land.
    #[test]
    fn policy_and_unbind_rows_go_to_their_routes_and_block_loudly() {
        let c = cfg();
        assert_eq!(path_for(Op::PolicySet), "/mgmt/v1/policies");
        assert_eq!(path_for(Op::SpaceUnbind), "/mgmt/v1/spaces/unbind");
        let un = space_row("space_unbind", 0);
        assert!(matches!(
            classify(&c, &un, Ok(&resp(404, &err("not_found", "", ""), "")), 0),
            Outcome::Done { status: 404, reconcile: false, .. }
        ), "no such Space: nothing to unbind");
        assert!(matches!(
            classify(&c, &un, Ok(&resp(409, &err("conflict", "app_mismatch", ""), "")), 0),
            Outcome::Blocked { status: Some(409), .. }
        ), "bound to another App: K2's record is wrong, say so");
        let pol = row("policy_set", 0, "k2ob_pol");
        assert!(matches!(
            classify(&c, &pol, Ok(&resp(400, &err("validation_failed", "", ""), "")), 0),
            Outcome::Blocked { status: Some(400), .. }
        ));
        assert!(matches!(classify(&c, &pol, Ok(&resp(200, r#"{"policies":{}}"#, "")), 0), Outcome::Done { status: 200, .. }));
        assert!(Op::DELIVERED.contains(&Op::PolicySet) && Op::DELIVERED.contains(&Op::SpaceUnbind));
    }

    fn home_row(attempts: i64, app: Option<&str>) -> OutboxRow {
        let mut r = row("home_client", attempts, "k2ob_home");
        r.subject_key = "home-client".into();
        r.payload_raw = serde_json::json!({ "app_id": app }).to_string();
        r.actor = "owner".into();
        r
    }

    /// Sew 0.6.0: a home-client row's refusals land as clear errors, never
    /// a bare error line; other rows and other answers keep their rules.
    #[test]
    fn home_client_refusals_are_clear_and_approval_waits_are_bounded() {
        let c = cfg();
        let denied = resp(403, &err("missing_permission", "owner_approval_required", ""), "");
        let govern = resp(409, &err("conflict", "home_client_cannot_govern", "app_id"), "");
        let run = |r: &OutboxRow, a: &MgmtResponse, approved: bool, now: i64| {
            refine_home(&c, r, Ok(a), classify(&c, r, Ok(a), now), approved, now)
        };

        // Approved and still refused: retry within the wait, then block, the
        // message naming the reason and what to check.
        let fresh = home_row(0, Some("hearth"));
        match run(&fresh, &denied, true, 5_000) {
            (Outcome::Retry { status: Some(403), error, stop_target: false, .. }, false) => {
                assert!(error.contains("owner_approval_required"), "{error}")
            }
            other => panic!("approved 403 within the wait: {other:?}"),
        }
        let old = home_row(3, Some("hearth")); // first attempt at 1_000
        match run(&old, &denied, true, 1_000 + 3_601) {
            (Outcome::Blocked { status: Some(403), error }, false) => {
                assert!(error.contains("owner_approval_required") && error.contains("k2:owner"), "{error}")
            }
            other => panic!("approved 403 after the wait: {other:?}"),
        }
        // Sent without the approval (flags not read since Sew gained it):
        // retry and read Sew's flags again.
        match run(&fresh, &denied, false, 5_000) {
            (Outcome::Retry { status: Some(403), .. }, true) => {}
            other => panic!("unapproved 403: {other:?}"),
        }
        // The home client never governs: blocked, naming the App.
        match run(&fresh, &govern, true, 5_000) {
            (Outcome::Blocked { status: Some(409), error }, false) => {
                assert!(error.contains("home_client_cannot_govern") && error.contains("hearth"), "{error}")
            }
            other => panic!("409 cannot govern: {other:?}"),
        }
        // Success, other answers and other ops: unchanged.
        let ok = resp(200, r#"{"app_id":"hearth"}"#, "");
        assert!(matches!(run(&fresh, &ok, true, 5_000), (Outcome::Done { status: 200, .. }, false)));
        let mismatch = resp(409, &err("conflict", "app_mismatch", ""), "");
        assert!(matches!(run(&fresh, &mismatch, true, 5_000), (Outcome::Blocked { status: Some(409), ref error }, false) if error == &mismatch.error_line()));
        let prov = row("provision", 0, "k2ob_p");
        let before = classify(&c, &prov, Ok(&govern), 5_000);
        assert_eq!(refine_home(&c, &prov, Ok(&govern), before.clone(), true, 5_000), (before, false));
    }

    /// K26: which outcomes are worth one `sew_changed` per pass.
    #[test]
    fn visible_changes_are_finished_blocked_held_or_a_first_failure() {
        let r0 = row("provision", 0, "k2ob_x");
        let r3 = row("provision", 3, "k2ob_x");
        let retry = Outcome::Retry { status: None, error: "x".into(), delay: Duration::from_secs(1), target_error: None, stop_target: true };
        assert!(visible_change(&r0, &Outcome::Done { status: 200, body: String::new(), reconcile: false }));
        assert!(visible_change(&r3, &Outcome::Blocked { status: Some(400), error: "x".into() }));
        assert!(visible_change(&r3, &Outcome::Held { status: 200, body: String::new(), why: "x".into() }));
        assert!(visible_change(&r0, &retry), "pending after its first failure");
        assert!(!visible_change(&r3, &retry), "later retries change nothing a page shows");
        assert!(!visible_change(&r0, &Outcome::UsernameRetry { status: 409 }));
    }
}
