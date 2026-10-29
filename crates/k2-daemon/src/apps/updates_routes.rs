//! `/cli/app/updates*`: an App's update feed (prd-tauri-core-0467-v1
//! TC-6, TC-8, TC-10, TC-11; vs-live TA-8, TA-10, TA-12 … TA-15;
//! `k2 app updates …`).
//!
//! ```text
//! GET  /cli/app/updates          ?app=[&channel=][&release=]  channels, releases, files, sizes, downloads, active users
//! GET  /cli/app/updates/key      ?app=                        the App's updater public key
//! POST /cli/app/updates/key/set  {app, pubkey}                set it; changing it on an App with releases is held for the owner
//! GET  /cli/app/updates/token    ?app=                        the download-host token: set or not, fingerprint (never the value)
//! POST /cli/app/updates/token/set {app, token}                keep the App's k2dl_ token in the vault (daemon-only)
//! POST /cli/app/updates/push     {app, channel, version, notes?, artifacts:[{platform, path, sig?}], dropPlatforms?}
//! POST /cli/app/updates/promote  {app, from, to, version?}    point `to` at a release already uploaded
//! POST /cli/app/updates/rollback {app, channel, to}           point a channel back (the answer warns: clients don't downgrade)
//! POST /cli/app/updates/pull     {app, version}               take a release out of every channel answer
//! GET  /cli/app/updates/stats    ?app=[&days=30]              downloads (download host) + check-ins (this box)
//! GET  /cli/app/updates/fleet                                 aggregate-only numbers for every App (owner/Admin)
//! ```
//!
//! **Who:** reads are anyone who may see the App (a passport: its own
//! workspace's Apps). Changes are the owner's, an Admin's, a K2 shell
//! tab's, or the App's agent with "Build and run Apps" (switch 1). Members
//! are read-only. **Changing** the updater key of an App that has releases
//! is a widening (every installed client would trust the new key): from an
//! agent or a K2 shell tab it is held for the owner (one `manage` card,
//! `apps.approve`); the owner and Admin logins apply it at once. Every
//! change is in the App's audit.
//!
//! **Keys:** K2 keeps only the public key; a secret key is refused by
//! name. **The token:** the download host's `k2dl_` token for this App,
//! delivered by K2 Web through the vault. The CLI reads it from a file (or
//! stdin), never argv; the daemon keeps it in the vault's system namespace
//! (delivery none: no route returns it) and sends it only to the download
//! host.
//!
//! **Push:** each file is read once from the App's own workspace folder
//! (the `k2 app data put` jail), its signature checked against the App's
//! key while it is copied to `~/.k2/apps/<app>/updates/staging/` and
//! hashed. Then a background upload sends each file once to the download
//! host; the channel moves only when every upload is confirmed, then the
//! static `latest.json` fallback is published there too, and staging is
//! removed. A failure leaves the release `failed` and no channel moved.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use k2_core::app_updates::{self as au, codes as uc, dl_client, feed, store};
use k2_core::deploy::{self, codes, store::AppRow};
use k2_vault::{Actor, Delivery, Kind, Namespace, PutMode, PutRequest, Requester, Scope, SecretBytes, SecretStore, Selector, Use};
use serde_json::{json, Value as J};

use super::routes::{actor, app_for, json_body, need_build, need_credential, no, s, ws_by_id, Refusal, Who, R};

pub const LIST: &str = "/cli/app/updates";
pub const KEY: &str = "/cli/app/updates/key";
pub const KEY_SET: &str = "/cli/app/updates/key/set";
pub const TOKEN: &str = "/cli/app/updates/token";
pub const TOKEN_SET: &str = "/cli/app/updates/token/set";
pub const PUSH: &str = "/cli/app/updates/push";
pub const PROMOTE: &str = "/cli/app/updates/promote";
pub const ROLLBACK: &str = "/cli/app/updates/rollback";
pub const PULL: &str = "/cli/app/updates/pull";
pub const STATS: &str = "/cli/app/updates/stats";
pub const FLEET: &str = "/cli/app/updates/fleet";

pub const ROUTES: &[&str] = &[LIST, KEY, KEY_SET, TOKEN, TOKEN_SET, PUSH, PROMOTE, ROLLBACK, PULL, STATS, FLEET];
/// POST-only (405 on GET).
pub const POST_ROUTES: &[&str] = &[KEY_SET, TOKEN_SET, PUSH, PROMOTE, ROLLBACK, PULL];

/// The vault name of the App's download-host token (system namespace,
/// scope `app/<service id>`).
pub const TOKEN_SECRET: &str = "K2DL_APP_TOKEN";
const SUBSYSTEM: &str = "app-updates";

/// Tauri installs only a newer version by default (TA-13): every rollback
/// answer says so.
pub const ROLLBACK_WARNING: &str = "Tauri's updater installs only a NEWER version by default, so people already on the bad version stay on it. \
     This rollback protects new installs and people still below it. To fix people on the bad version, ship a higher version (for example x.y.z+1) with the fix.";

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        LIST => list(who, params),
        KEY => key_show(who, params),
        KEY_SET => key_set(who, body),
        TOKEN => token_show(who, params),
        TOKEN_SET => token_set(who, body),
        PUSH => push(who, body),
        PROMOTE => promote(who, body),
        ROLLBACK => rollback(who, body),
        PULL => pull(who, body),
        STATS => stats(who, params),
        FLEET => fleet(who),
        _ => Err(no("not_found", 404, "no such App updates route")),
    }
}

// ── helpers ──────────────────────────────────────────────────────────

fn db<T>(f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>) -> R<T> {
    let h = k2_core::db::shared();
    let conn = h.lock();
    f(&conn).map_err(super::routes::db_err)
}

fn audit(app: &AppRow, event: &str, by: &str, payload: J) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    if let Err(e) = deploy::audit::append(&conn, deploy::audit::SUBJECT_APP, &app.app_id, event, by, None, &payload) {
        k2_core::log_debug!("[app-updates] audit {} {event}: {e}", app.app_id);
    }
}

fn the_app(who: &Who, raw: Option<&str>) -> R<AppRow> {
    need_credential(who)?;
    // Settings names an App by its service id; agents by its App id.
    let by_service = raw.and_then(|r| db(|c| deploy::store::get_app_by_service(c, r.trim())).ok().flatten()).map(|a| a.app_id);
    let app = app_for(who, by_service.as_deref().or(raw))?;
    if app.app_id == k2_core::skin::HOST_APP_ID {
        return Err(no("not_for_host", 400, "the update feed is for a published App, never the box login (host)"));
    }
    Ok(app)
}

/// TA-10: K2 answers on an App's hosts only when it fronts the App.
fn need_front(app: &AppRow) -> R<()> {
    if let Err(why) = crate::app_pairing::front_rewrites(&app.service_id) {
        let hint = match why {
            crate::app_pairing::NotServed::Raw => {
                "this App's manifest says host.clientIp: false (a raw splice), so K2 can't answer /_k2/updates/ on its host; turn the client-IP front back on (host.clientIp: true) and deploy"
            }
            crate::app_pairing::NotServed::NotRuntime => "the update feed needs a runtime App (k2 app deploy)",
        };
        return Err(no(uc::NEEDS_K2_FRONT, 409, hint));
    }
    Ok(())
}

pub(crate) fn dl_id_of(app: &AppRow) -> R<String> {
    au::dl_id(&super::origins::box_subdomain(), &app.app_id).map_err(|e| no(uc::DL_ID_INVALID, 409, e))
}

fn the_key(app: &AppRow) -> R<au::PubKey> {
    let row = db(|c| store::key(c, &app.app_id))?.ok_or_else(|| {
        no(
            uc::NO_UPDATE_KEY,
            409,
            format!(
                "{} has no updater key yet: k2 app updates key {} --set \"<the .pub file's base64, as tauri.conf.json plugins.updater.pubkey>\"",
                app.app_id, app.app_id
            ),
        )
    })?;
    Ok(au::PubKey { wrapped: row.pubkey, key_id: row.key_id })
}

/// Where a push's files wait until their upload is confirmed.
pub fn staging_root(app_id: &str) -> PathBuf {
    deploy::app_dir(app_id).join("updates").join("staging")
}

/// The App's workspace folder: a push reads files only inside it.
fn workspace_root(app: &AppRow) -> R<PathBuf> {
    let ws = ws_by_id(&app.owner_workspace).ok_or_else(|| no(codes::FORBIDDEN, 404, "the App's workspace is not registered"))?;
    Ok(PathBuf::from(ws.path))
}

fn release_json(conn: &rusqlite::Connection, r: &store::ReleaseRow, channels: &[store::ChannelRow]) -> rusqlite::Result<J> {
    let arts = store::artifacts(conn, &r.id)?;
    let on: Vec<&str> = channels.iter().filter(|c| c.release_id == r.id).map(|c| c.channel.as_str()).collect();
    Ok(json!({
        "id": r.id,
        "version": r.version,
        "state": r.state,
        "error": r.error,
        "firstChannel": r.channel,
        "channels": on,
        "notes": r.notes,
        "pubDate": r.pub_date,
        "createdBy": r.created_by,
        "createdAt": r.created_at,
        "size": arts.iter().map(|a| a.size).sum::<i64>(),
        "files": arts.iter().map(|a| json!({
            "platform": a.platform,
            "file": a.file_name,
            "size": a.size,
            "sha256": a.sha256,
            "url": a.url,
            "upload": a.upload_state,
            "error": a.error,
        })).collect::<Vec<_>>(),
    }))
}

// ── the vault (the download-host token) ──────────────────────────────

fn vault() -> R<Arc<k2_vault::LocalVault>> {
    crate::vault::get().map_err(|e| no(super::egress_routes::codes::VAULT_CLOSED, 503, format!("the vault isn't open ({}): {}", e.code, e.hint)))
}

fn token_get(app: &AppRow) -> R<Option<String>> {
    let v = vault()?;
    match v.resolve(&Selector::system(Scope::app(&app.service_id), TOKEN_SECRET), Use::System, &Requester::System(SUBSYSTEM.into())) {
        Ok(r) => Ok(String::from_utf8(r.value.expose().to_vec()).ok()),
        Err(e) if e.status == 404 || e.code == "secret_not_active" => Ok(None),
        Err(e) => Err(no(super::egress_routes::codes::VAULT_CLOSED, 503, format!("the vault refused ({})", e.code))),
    }
}

fn token_put(app: &AppRow, token: &str) -> R<()> {
    let v = vault()?;
    let mut r = PutRequest::new(Namespace::System, Scope::app(&app.service_id), TOKEN_SECRET, Kind::Generic);
    r.default_delivery = Some(Delivery::None);
    r.delivery_ceiling = Some(Delivery::None);
    r.origin = format!("system:{SUBSYSTEM}");
    r.description = "An App's download-host (dl.k2.dev) upload token; daemon-only.".into();
    r.mode = PutMode::Upsert;
    r.idempotent = true;
    v.put(r, SecretBytes::from_slice(token.as_bytes()), &Actor::System(SUBSYSTEM.into()))
        .map(|_| ())
        .map_err(|e| no(super::egress_routes::codes::VAULT_CLOSED, 503, format!("the vault refused ({})", e.code)))
}

/// The App went away: its token goes too (best effort).
pub fn forget_app(app_id: &str, service_id: &str) {
    if let Ok(v) = crate::vault::get() {
        match v.destroy(&Selector::system(Scope::app(service_id), TOKEN_SECRET), &Actor::System(SUBSYSTEM.into())) {
            Ok(()) => {}
            Err(e) if e.status == 404 || e.code == "secret_destroyed" => {}
            Err(e) => k2_core::log_debug!("[app-updates] {app_id} removed: WARN destroying its token: {}", e.code),
        }
    }
    if let Err(e) = db(|c| store::forget_app(c, app_id)) {
        k2_core::log_debug!("[app-updates] {app_id} removed: WARN {}", e.hint);
    }
    let _ = std::fs::remove_dir_all(deploy::app_dir(app_id).join("updates"));
    super::updates_serve::forget(app_id);
}

/// The download-host client for this App (base from `K2_DL_BASE`).
fn client(app: &AppRow) -> R<dl_client::Client> {
    let token = token_get(app)?.ok_or_else(|| {
        no(
            uc::DL_TOKEN_MISSING,
            409,
            format!(
                "{} has no download-host token yet. K2 Web mints one per App id ({}); keep it with \
                 k2 app updates token {} --file <file> (or --stdin), never on the command line",
                app.app_id,
                dl_id_of(app).unwrap_or_else(|_| "dev.k2.<box>.<app>".into()),
                app.app_id
            ),
        )
    })?;
    #[allow(unused_mut)]
    let mut c = dl_client::Client::new(&au::dl_base(), &token).map_err(|e| no(uc::UPDATES_HOST_UNAVAILABLE, 503, e.line()))?;
    // Tests don't wait seconds between retries.
    #[cfg(test)]
    {
        c.backoff = std::time::Duration::from_millis(5);
        c.max_wait = std::time::Duration::from_millis(20);
    }
    Ok(c)
}

fn host_refusal(e: &dl_client::DlError) -> Refusal {
    match e {
        dl_client::DlError::Unavailable(_) => no(uc::UPDATES_HOST_UNAVAILABLE, 503, e.line()),
        dl_client::DlError::Refused { status: 401, .. } | dl_client::DlError::Refused { status: 403, .. } => Refusal {
            extra: Some(json!({ "host": e.line() })),
            ..no(uc::DL_TOKEN_INVALID, 409, "the download host refused this App's token: ask K2 Web for a new one, then k2 app updates token <app> --file <file>")
        },
        _ => no(uc::UPDATES_HOST_REFUSED, 502, e.line()),
    }
}

// ── GET /cli/app/updates ─────────────────────────────────────────────

fn list(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let app = the_app(who, params.get("app").map(String::as_str))?;
    super::updates_serve::flush();
    let only_channel = params.get("channel").map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    let only_release = params.get("release").map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    let (key, channels, releases, checkins) = db(|c| {
        let key = store::key(c, &app.app_id)?;
        let chans = store::channels(c, &app.app_id)?;
        let mut rels = Vec::new();
        for r in store::releases(c, &app.app_id)? {
            if only_release.as_deref().is_some_and(|x| x != r.id && x != r.version) {
                continue;
            }
            if only_channel.as_deref().is_some_and(|ch| r.channel != ch && !chans.iter().any(|x| x.channel == ch && x.release_id == r.id)) {
                continue;
            }
            rels.push(release_json(c, &r, &chans)?);
        }
        let ci = store::checkins(c, &app.app_id, &au::day_of(deploy::now() - 29 * 86_400))?;
        Ok((key, chans, rels, ci))
    })?;
    let users = active_users(&checkins);
    let mut rels = releases;
    for r in rels.iter_mut() {
        let v = r["version"].as_str().unwrap_or("").to_string();
        r["activeUsers"] = json!(users.by_version.get(&v).copied().unwrap_or(0));
    }
    let chans: Vec<J> = channels
        .iter()
        .map(|c| {
            let v = rels.iter().find(|r| r["id"] == c.release_id.as_str()).map(|r| r["version"].clone()).unwrap_or(J::Null);
            json!({ "channel": c.channel, "version": v, "release": c.release_id, "updatedBy": c.updated_by, "updatedAt": c.updated_at })
        })
        .collect();
    let id = dl_id_of(&app).ok();
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "dlId": id,
        "key": key.map(|k| json!({ "keyId": k.key_id, "setBy": k.set_by, "setAt": k.set_at })),
        "channels": chans,
        "releases": rels,
        "activeUsers": { "last7Days": users.last7, "today": users.today, "checksLast30Days": users.checks30, "counting": users.counting },
        "endpoints": endpoints(&app),
    }))
}

/// The updater URLs to put in `tauri.conf.json` (on the App's first host).
fn endpoints(app: &AppRow) -> J {
    let host = crate::app_pairing::pairing_hosts(&app.service_id).into_iter().next();
    match host {
        Some(h) => json!({
            "dynamic": format!("https://{h}/_k2/updates/<channel>/{{{{target}}}}/{{{{arch}}}}/{{{{current_version}}}}"),
            "static": format!("https://{h}/_k2/updates/<channel>/latest.json"),
            "fallback": dl_id_of(app).ok().map(|id| format!("{}/{id}/<channel>/latest.json", au::dl_base())),
        }),
        None => J::Null,
    }
}

#[derive(Default)]
struct Users {
    by_version: BTreeMap<String, i64>,
    last7: i64,
    today: i64,
    checks30: i64,
    /// `installs` (an app sends an install id) or `checks` (counts only).
    counting: &'static str,
}

/// Active users from check-ins: distinct installs when the app sends an
/// install id, else the busiest day's checks (a ceiling: one install may
/// check more than once a day).
fn active_users(rows: &[store::CheckinRow]) -> Users {
    let today = au::day_of(deploy::now());
    let week = au::day_of(deploy::now() - 6 * 86_400);
    let installs: i64 = rows.iter().map(|r| r.installs).sum();
    let mut u = Users { counting: if installs > 0 { "installs" } else { "checks" }, ..Default::default() };
    let pick = |r: &store::CheckinRow| if installs > 0 { r.installs } else { r.checks };
    let mut per_day: BTreeMap<&str, i64> = BTreeMap::new();
    let mut per_version_day: BTreeMap<(&str, &str), i64> = BTreeMap::new();
    for r in rows {
        u.checks30 += r.checks;
        if r.version == "-" {
            continue;
        }
        if r.day >= week {
            *per_day.entry(r.day.as_str()).or_insert(0) += pick(r);
            *per_version_day.entry((r.version.as_str(), r.day.as_str())).or_insert(0) += pick(r);
        }
        if r.day == today {
            u.today += pick(r);
        }
    }
    u.last7 = per_day.values().copied().max().unwrap_or(0);
    for ((v, _), n) in per_version_day {
        let e = u.by_version.entry(v.to_string()).or_insert(0);
        *e = (*e).max(n);
    }
    u
}

// ── key ──────────────────────────────────────────────────────────────

fn key_show(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let app = the_app(who, params.get("app").map(String::as_str))?;
    let row = db(|c| store::key(c, &app.app_id))?;
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "pubkey": row.as_ref().map(|k| k.pubkey.clone()),
        "keyId": row.as_ref().map(|k| k.key_id.clone()),
        "setBy": row.as_ref().map(|k| k.set_by.clone()),
        "setAt": row.as_ref().map(|k| k.set_at),
    }))
}

fn key_set(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let app = the_app(who, s(&v, "app"))?;
    need_build(who)?;
    let raw = v.get("pubkey").and_then(J::as_str).unwrap_or("");
    let key = au::parse_pubkey(raw).map_err(|(code, hint)| no(code, 400, hint))?;
    need_front(&app)?;
    let by = actor(who);
    let (before, has_releases) = db(|c| Ok((store::key(c, &app.app_id)?, !store::releases(c, &app.app_id)?.is_empty())))?;
    if before.as_ref().is_some_and(|b| b.pubkey == key.wrapped) {
        return Ok(json!({ "ok": true, "app": app.app_id, "keyId": key.key_id, "unchanged": true }));
    }
    let change = before.is_some() && has_releases;
    // A change on an App with releases widens what every installed client
    // trusts: an agent or a K2 shell tab asks the owner (TC-6).
    let person = matches!(who, Who::Owner | Who::Login { .. });
    if change && !person && !super::approvals::manage_approved_now() {
        let ws = who.passport().map(|(w, _)| w.to_string()).unwrap_or_else(|| app.owner_workspace.clone());
        let old_id = before.as_ref().map(|b| b.key_id.clone()).unwrap_or_default();
        let r = super::approvals::file(&super::approvals::Filing {
            who,
            kind: k2_core::app_grace::requests::KIND_MANAGE,
            app: Some(&app),
            label: &app.app_id,
            workspace: &ws,
            body: json!({ "route": KEY_SET, "body": { "app": app.app_id, "pubkey": key.wrapped } }),
            diff: json!({ "lines": [
                format!("k2 app updates key {} --set <new key>", app.app_id),
                format!("updater key: {old_id} → {}", key.key_id),
                "every installed copy of the App will trust archives signed by the new key".to_string(),
            ], "route": KEY_SET }),
            summary: format!("k2 app updates key {} (change {old_id} → {})", app.app_id, key.key_id),
            reasons: vec!["changing an App's updater key after it has releases is the owner's call: every installed client trusts the new key".into()],
            reason: s(&v, "reason"),
        })?;
        audit(&app, "updates_key_held", &by, json!({ "request": r.id, "from": old_id, "to": key.key_id }));
        let mut out = super::approvals::held_answer(&r);
        out["code"] = json!(uc::KEY_CHANGE_HELD);
        return Ok(out);
    }
    db(|c| store::set_key(c, &app.app_id, &key.wrapped, &key.key_id, &by, deploy::now()))?;
    audit(&app, if before.is_some() { "updates_key_changed" } else { "updates_key_set" }, &by, json!({ "keyId": key.key_id, "from": before.map(|b| b.key_id) }));
    Ok(json!({ "ok": true, "app": app.app_id, "keyId": key.key_id, "changed": change }))
}

// ── token ────────────────────────────────────────────────────────────

fn token_show(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let app = the_app(who, params.get("app").map(String::as_str))?;
    let t = token_get(&app)?;
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "dlId": dl_id_of(&app).ok(),
        "set": t.is_some(),
        "fingerprint": t.as_deref().map(au::token_fingerprint),
        "host": au::dl_base(),
    }))
}

fn token_set(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let app = the_app(who, s(&v, "app"))?;
    need_build(who)?;
    let id = dl_id_of(&app)?;
    let t = v.get("token").and_then(J::as_str).map(str::trim).unwrap_or("");
    if !au::valid_dl_token(t) {
        return Err(no(
            uc::DL_TOKEN_INVALID,
            400,
            "that isn't a download-host App token (k2dl_ followed by 32 lowercase hex). A k2dlr_ stats token is K2 Web's, never an App's",
        ));
    }
    token_put(&app, t)?;
    let fp = au::token_fingerprint(t);
    audit(&app, "updates_token_set", &actor(who), json!({ "fingerprint": fp, "dlId": id }));
    Ok(json!({ "ok": true, "app": app.app_id, "dlId": id, "fingerprint": fp }))
}

// ── push ─────────────────────────────────────────────────────────────

/// A reader that writes what it reads to `out` (the staging copy).
struct Tee<R: Read, W: Write> {
    r: R,
    w: W,
}

impl<R: Read, W: Write> Read for Tee<R, W> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.r.read(buf)?;
        self.w.write_all(&buf[..n])?;
        Ok(n)
    }
}

struct Staged {
    platform: String,
    host_name: String,
    content_type: &'static str,
    path: PathBuf,
    sha256: String,
    size: u64,
    sig_b64: String,
}

fn in_workspace(root: &Path, raw: &str) -> R<PathBuf> {
    super::data_put::resolve_from(root, raw).map_err(|e| no(uc::FILE_OUTSIDE_WORKSPACE, 403, e))
}

fn push(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let app = the_app(who, s(&v, "app"))?;
    need_build(who)?;
    let ch = s(&v, "channel").unwrap_or("").to_string();
    if !au::valid_channel(&ch) {
        return Err(no(uc::CHANNEL_INVALID, 400, format!("channel '{ch}': use stable, beta, or [a-z][a-z0-9-] up to 20 characters")));
    }
    let version = s(&v, "version").unwrap_or("").to_string();
    au::parse_version(&version).map_err(|e| no(uc::VERSION_INVALID, 400, e))?;
    let notes = v.get("notes").and_then(J::as_str).map(str::to_string).filter(|n| !n.trim().is_empty());
    if notes.as_ref().is_some_and(|n| n.len() > au::MAX_NOTES_BYTES) {
        return Err(no(codes::BAD_REQUEST, 413, format!("notes are at most {} KiB", au::MAX_NOTES_BYTES / 1024)));
    }
    let drops: BTreeSet<String> =
        v.get("dropPlatforms").and_then(J::as_array).into_iter().flatten().filter_map(J::as_str).map(|p| p.trim().to_string()).collect();
    let arts = v.get("artifacts").and_then(J::as_array).cloned().unwrap_or_default();
    if arts.is_empty() {
        return Err(no(codes::BAD_REQUEST, 400, "name at least one --artifact <platform>=<file>"));
    }
    if arts.len() > au::MAX_PLATFORMS {
        return Err(no(codes::BAD_REQUEST, 400, format!("at most {} platforms in one release", au::MAX_PLATFORMS)));
    }
    let key = the_key(&app)?;
    need_front(&app)?;
    let root = workspace_root(&app)?;
    // Every check that needs no reading first.
    let mut plan: Vec<(au::Platform, PathBuf, PathBuf, &'static str, String)> = Vec::new();
    for a in &arts {
        let p = a.get("platform").and_then(J::as_str).unwrap_or("");
        let platform = au::parse_platform(p).map_err(|e| no(uc::PLATFORM_INVALID, 400, e))?;
        if plan.iter().any(|x| x.0 == platform) {
            return Err(no(uc::PLATFORM_INVALID, 400, format!("{} is named twice", platform.key())));
        }
        let file = in_workspace(&root, a.get("path").and_then(J::as_str).unwrap_or(""))?;
        let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let (_, ct) = au::artifact_type(&name).ok_or_else(|| {
            no(
                uc::ARTIFACT_TYPE_REFUSED,
                415,
                format!(
                    "{name} ({}): the feed takes {}",
                    platform.key(),
                    au::ARTIFACT_TYPES.iter().map(|(s, _)| *s).collect::<Vec<_>>().join(" ")
                ),
            )
        })?;
        let meta = std::fs::metadata(&file).map_err(|e| no(codes::BAD_REQUEST, 400, format!("{}: {e}", file.display())))?;
        if !meta.is_file() {
            return Err(no(codes::BAD_REQUEST, 400, format!("{} is not a file", file.display())));
        }
        if meta.len() > au::MAX_ARTIFACT_BYTES {
            return Err(no(uc::ARTIFACT_TOO_LARGE, 413, format!("{name} is {} MiB; the cap is {} MiB", meta.len() >> 20, au::MAX_ARTIFACT_BYTES >> 20)));
        }
        let sig_raw = a.get("sig").and_then(J::as_str).map(str::to_string).unwrap_or_else(|| format!("{}.sig", file.display()));
        let sig = in_workspace(&root, &sig_raw).map_err(|e| {
            if e.code == uc::FILE_OUTSIDE_WORKSPACE && !Path::new(&sig_raw).exists() {
                no(uc::SIGNATURE_MISSING, 400, format!("no signature for {} at {sig_raw}: sign it (tauri signer sign) or pass --sig {}=<file.sig>", platform.key(), platform.key()))
            } else {
                e
            }
        })?;
        let host_name = au::host_file_name(&name).ok_or_else(|| no(uc::ARTIFACT_TYPE_REFUSED, 415, format!("{name}: no usable name")))?;
        plan.push((platform, file, sig, ct, host_name));
    }
    // The version and the platform guard against the channel now.
    let (existing, current) = db(|c| {
        let e = store::release_by_version(c, &app.app_id, &version)?;
        let cur = match store::channel(c, &app.app_id, &ch)? {
            Some(row) => match store::release(c, &row.release_id)? {
                Some(r) => {
                    let arts = store::artifacts(c, &r.id)?;
                    Some((r, arts))
                }
                None => None,
            },
            None => None,
        };
        Ok((e, cur))
    })?;
    if let Some(e) = existing.filter(|e| e.state != store::STATE_FAILED) {
        return Err(no(
            uc::VERSION_EXISTS,
            409,
            format!("{} {version} already exists ({}). A version is never replaced: ship a higher one, or point this channel at it with promote", app.app_id, e.state),
        ));
    }
    if let Some((cur, cur_arts)) = &current {
        if !au::newer(&version, &cur.version) {
            return Err(no(
                uc::VERSION_NOT_NEWER,
                409,
                format!("{ch} is at {}; a push must be newer. To point {ch} back at an older release, use k2 app updates rollback", cur.version),
            ));
        }
        let new_keys: BTreeSet<String> = plan.iter().map(|p| p.0.key()).collect();
        let missing: Vec<String> = cur_arts.iter().map(|a| a.platform.clone()).filter(|k| !new_keys.contains(k) && !drops.contains(k)).collect();
        if !missing.is_empty() {
            return Err(Refusal {
                extra: Some(json!({ "missing": missing })),
                ..no(
                    uc::PLATFORM_DROPPED,
                    409,
                    format!(
                        "{ch} {} serves {} and this push doesn't: those users would stop getting updates. Add the files, or drop them on purpose with --drop-platform {}",
                        cur.version,
                        missing.join(", "),
                        missing.join(" --drop-platform ")
                    ),
                )
            });
        }
    }
    let id = dl_id_of(&app)?;
    let c = client(&app)?;
    {
        // One quick look before reading hundreds of MiB.
        let mut probe = dl_client::Client::new(c.base(), "unused").map_err(|e| host_refusal(&e))?;
        probe.attempts = 1;
        probe.health().map_err(|e| host_refusal(&e))?;
    }
    // Read each file once: verify against the App's key while copying to
    // staging and hashing.
    let rel_tag = format!("p-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let stage_dir = staging_root(&app.app_id).join(&rel_tag);
    std::fs::create_dir_all(&stage_dir).map_err(|e| no("staging_failed", 500, format!("{}: {e}", stage_dir.display())))?;
    let cleanup = |e: Refusal| {
        let _ = std::fs::remove_dir_all(&stage_dir);
        e
    };
    let mut staged: Vec<Staged> = Vec::new();
    for (platform, file, sig, ct, host_name) in &plan {
        let sig_text = std::fs::read_to_string(sig).map_err(|e| cleanup(no(uc::SIGNATURE_MISSING, 400, format!("{}: {e}", sig.display()))))?;
        let (raw, b64) = au::normalize_signature(&sig_text).map_err(|e| cleanup(no(uc::SIGNATURE_INVALID, 400, format!("{}: {e}", platform.key()))))?;
        let dest = stage_dir.join(host_name);
        let src = std::fs::File::open(file).map_err(|e| cleanup(no(codes::BAD_REQUEST, 400, format!("{}: {e}", file.display()))))?;
        let out = std::fs::File::create(&dest).map_err(|e| cleanup(no("staging_failed", 500, e.to_string())))?;
        let checked = au::verify_and_hash(&key, &raw, Tee { r: src, w: std::io::BufWriter::new(out) }, au::MAX_ARTIFACT_BYTES)
            .map_err(|(code, hint)| cleanup(no(code, if code == uc::ARTIFACT_TOO_LARGE { 413 } else { 400 }, format!("{}: {hint}", platform.key()))))?;
        staged.push(Staged { platform: platform.key(), host_name: host_name.clone(), content_type: ct, path: dest, sha256: checked.sha256, size: checked.size, sig_b64: b64 });
    }
    let by = actor(who);
    let now = deploy::now();
    let rel = db(|c| {
        let r = store::insert_release(
            c,
            &store::NewRelease { app_id: &app.app_id, version: &version, channel: &ch, notes: notes.as_deref(), pub_date: &au::rfc3339(now), created_by: &by },
            now,
        )?;
        for st in &staged {
            store::insert_artifact(
                c,
                &store::ArtifactRow {
                    release_id: r.id.clone(),
                    platform: st.platform.clone(),
                    file_name: st.host_name.clone(),
                    content_type: st.content_type.to_string(),
                    sha256: st.sha256.clone(),
                    size: st.size as i64,
                    signature: st.sig_b64.clone(),
                    url: None,
                    upload_state: store::UP_PENDING.into(),
                    error: None,
                },
            )?;
        }
        Ok(r)
    })
    .map_err(cleanup)?;
    // The staging folder takes the release id (clean finds it by name).
    let final_dir = staging_root(&app.app_id).join(&rel.id);
    let stage_dir = match std::fs::rename(&stage_dir, &final_dir) {
        Ok(()) => final_dir,
        Err(_) => stage_dir.clone(),
    };
    let staged: Vec<Staged> = staged.into_iter().map(|s| Staged { path: stage_dir.join(&s.host_name), ..s }).collect();
    audit(
        &app,
        "updates_push",
        &by,
        json!({ "release": rel.id, "version": version, "channel": ch, "platforms": staged.iter().map(|s| &s.platform).collect::<Vec<_>>(), "dropped": drops }),
    );
    let files: Vec<J> = staged.iter().map(|s| json!({ "platform": s.platform, "file": s.host_name, "size": s.size, "sha256": s.sha256 })).collect();
    let job = Upload { app: app.clone(), dl_id: id, release: rel.clone(), channel: ch.clone(), staged, stage_dir, by };
    let handle = k2_core::db::shared();
    std::thread::Builder::new()
        .name(format!("app-updates-{}", rel.id))
        .spawn(move || job.run(handle, c))
        .map_err(|e| no("upload_spawn_failed", 500, e.to_string()))?;
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "release": rel.id,
        "version": version,
        "channel": ch,
        "state": store::STATE_UPLOADING,
        "files": files,
        "next": format!("k2 app updates ls {} --release {}", app.app_id, rel.id),
    }))
}

struct Upload {
    app: AppRow,
    dl_id: String,
    release: store::ReleaseRow,
    channel: String,
    staged: Vec<Staged>,
    stage_dir: PathBuf,
    by: String,
}

type DbHandle = Arc<parking_lot::ReentrantMutex<rusqlite::Connection>>;

impl Upload {
    /// The background half: each file once, then the channel, then the
    /// static fallback; staging goes either way.
    fn run(self, db: DbHandle, c: dl_client::Client) {
        let rid = self.release.id.clone();
        let mut failed: Option<String> = None;
        for st in &self.staged {
            let r = c.upload(&self.dl_id, &self.release.version, &st.host_name, &st.path, &st.sha256, st.size, st.content_type);
            let conn = db.lock();
            match r {
                Ok(u) => {
                    let _ = store::set_artifact_upload(&conn, &rid, &st.platform, store::UP_UPLOADED, Some(&u.url), None);
                }
                Err(e) => {
                    let line = e.line();
                    let _ = store::set_artifact_upload(&conn, &rid, &st.platform, store::UP_FAILED, None, Some(&line));
                    failed = Some(format!("{}: {line}", st.platform));
                    break;
                }
            }
        }
        let now = deploy::now();
        let outcome: Result<(), String> = match failed {
            Some(f) => Err(f),
            None => {
                let conn = db.lock();
                // The channel may have moved while this uploaded.
                let cur = store::channel(&conn, &self.app.app_id, &self.channel).ok().flatten().and_then(|row| store::release(&conn, &row.release_id).ok().flatten());
                match cur {
                    Some(c) if c.state == store::STATE_LIVE && !au::newer(&self.release.version, &c.version) => {
                        Err(format!("{} reached {} {} while this uploaded; push a higher version", c.version, self.app.app_id, self.channel))
                    }
                    _ => store::set_release_state(&conn, &rid, store::STATE_LIVE, None, now)
                        .and_then(|_| store::set_channel(&conn, &self.app.app_id, &self.channel, Some(&rid), store::ACT_PUSH, &self.by, now))
                        .map_err(|e| e.to_string()),
                }
            }
        };
        match &outcome {
            Ok(()) => {
                refresh_with(&db, &self.app.app_id);
                let fallback = publish_fallback(&db, &c, &self.app.app_id, &self.dl_id, &self.channel);
                let conn = db.lock();
                let _ = deploy::audit::append(
                    &conn,
                    deploy::audit::SUBJECT_APP,
                    &self.app.app_id,
                    "updates_live",
                    &self.by,
                    None,
                    &json!({ "release": rid, "version": self.release.version, "channel": self.channel, "fallback": fallback }),
                );
            }
            Err(why) => {
                let conn = db.lock();
                let _ = store::set_release_state(&conn, &rid, store::STATE_FAILED, Some(why), now);
                let _ = deploy::audit::append(
                    &conn,
                    deploy::audit::SUBJECT_APP,
                    &self.app.app_id,
                    "updates_failed",
                    &self.by,
                    None,
                    &json!({ "release": rid, "version": self.release.version, "channel": self.channel, "why": why }),
                );
            }
        }
        let _ = std::fs::remove_dir_all(&self.stage_dir);
    }
}

/// Re-read `app`'s feed into the serve snapshot from this DB handle.
fn refresh_with(db: &DbHandle, app: &str) {
    super::updates_serve::refresh_from(db, app);
}

/// Put the channel's static `latest.json` on the download host (the
/// fallback the box also publishes). `"ok"`, `"skipped"` or the reason.
fn publish_fallback(db: &DbHandle, c: &dl_client::Client, app: &str, dl_id: &str, ch: &str) -> String {
    let f = {
        let conn = db.lock();
        feed::load(&conn, app)
    };
    match f {
        Ok(f) => match f.channels.get(ch) {
            Some(fc) => match c.put_manifest(dl_id, ch, &feed::static_manifest(fc.current())) {
                Ok(_) => "ok".into(),
                Err(e) => e.line(),
            },
            None => "skipped: the channel is empty".into(),
        },
        Err(e) => e.to_string(),
    }
}

/// Boot: remove the staging of every release that isn't uploading.
pub fn clean_all_staging_of_failed() {
    let apps = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        deploy::store::list_apps(&conn, None).unwrap_or_default()
    };
    for a in apps {
        for p in stale_staging(&a.app_id, i64::MAX) {
            let _ = std::fs::remove_dir_all(&p);
        }
    }
}

/// Staging folders of `app_id` whose release isn't uploading and that are
/// older than a day at `now` (`k2 app resources clean`).
pub fn stale_staging(app_id: &str, now: i64) -> Vec<PathBuf> {
    let root = staging_root(app_id);
    let Ok(rd) = std::fs::read_dir(&root) else { return Vec::new() };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        let uploading = {
            let h = k2_core::db::shared();
            let conn = h.lock();
            store::release(&conn, &name).ok().flatten().is_some_and(|r| r.state == store::STATE_UPLOADING)
        };
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|d| now.saturating_sub(d.as_secs() as i64) >= 86_400 || now == i64::MAX);
        if !uploading && old {
            out.push(p);
        }
    }
    out.sort();
    out
}

// ── promote, rollback, pull ──────────────────────────────────────────

fn need_channel(raw: Option<&str>, what: &str) -> R<String> {
    let c = raw.unwrap_or("").to_string();
    if !au::valid_channel(&c) {
        return Err(no(uc::CHANNEL_INVALID, 400, format!("{what}: '{c}' isn't a channel name (stable, beta, or [a-z][a-z0-9-] up to 20)")));
    }
    Ok(c)
}

fn live_release(app: &AppRow, version: &str) -> R<store::ReleaseRow> {
    let r = db(|c| store::release_by_version(c, &app.app_id, version))?
        .ok_or_else(|| no(uc::RELEASE_NOT_FOUND, 404, format!("{} has no release {version} (k2 app updates ls {})", app.app_id, app.app_id)))?;
    if r.state == store::STATE_UPLOADING {
        return Err(no(uc::UPLOAD_PENDING, 409, format!("{version} is still uploading: wait for it (k2 app updates ls {} --release {})", app.app_id, r.id)));
    }
    if r.state != store::STATE_LIVE {
        return Err(no(uc::RELEASE_NOT_LIVE, 409, format!("{version} is {}: only an uploaded, live release can be pointed at", r.state)));
    }
    Ok(r)
}

fn current_of(app: &AppRow, ch: &str) -> R<Option<store::ReleaseRow>> {
    db(|c| match store::channel(c, &app.app_id, ch)? {
        Some(row) => store::release(c, &row.release_id),
        None => Ok(None),
    })
}

/// Point `ch` at `rel`, refresh the answers, re-publish the fallback.
fn move_channel(app: &AppRow, ch: &str, rel: Option<&store::ReleaseRow>, action: &str, by: &str) -> R<String> {
    db(|c| store::set_channel(c, &app.app_id, ch, rel.map(|r| r.id.as_str()), action, by, deploy::now()))?;
    let handle = k2_core::db::shared();
    refresh_with(&handle, &app.app_id);
    let fallback = match (rel, dl_id_of(app), client(app)) {
        (None, _, _) => "skipped: the channel is empty (ask K2 Web to unset it on the download host)".to_string(),
        (Some(_), Ok(id), Ok(c)) => publish_fallback(&handle, &c, &app.app_id, &id, ch),
        (_, Err(e), _) | (_, _, Err(e)) => format!("skipped: {}", e.hint),
    };
    Ok(fallback)
}

fn promote(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let app = the_app(who, s(&v, "app"))?;
    need_build(who)?;
    let to = need_channel(s(&v, "to"), "--to")?;
    // --from names where the release is; with --version it may be left out.
    let from = match (s(&v, "from"), s(&v, "version")) {
        (None, Some(_)) => None,
        (f, _) => Some(need_channel(f, "--from")?),
    };
    if from.as_deref() == Some(to.as_str()) {
        return Err(no(codes::BAD_REQUEST, 400, "--from and --to are the same channel"));
    }
    let version = match (s(&v, "version"), &from) {
        (Some(x), _) => x.to_string(),
        (None, Some(f)) => current_of(&app, f)?.map(|r| r.version).ok_or_else(|| no(uc::CHANNEL_EMPTY, 409, format!("{f} has no release to promote")))?,
        (None, None) => return Err(no(codes::BAD_REQUEST, 400, "name --from <channel> or --version <v>")),
    };
    let rel = live_release(&app, &version)?;
    if let Some(cur) = current_of(&app, &to)? {
        if cur.id == rel.id {
            return Ok(json!({ "ok": true, "app": app.app_id, "channel": to, "version": version, "unchanged": true }));
        }
        if !au::newer(&rel.version, &cur.version) {
            return Err(no(
                uc::VERSION_NOT_NEWER,
                409,
                format!("{to} is at {}; promote moves a channel forward only. To go back, k2 app updates rollback {} --channel {to} --to {version}", cur.version, app.app_id),
            ));
        }
        // The platform guard holds for a promote too.
        let (cur_p, new_p) = db(|c| Ok((store::artifacts(c, &cur.id)?, store::artifacts(c, &rel.id)?)))?;
        let missing: Vec<String> = cur_p.iter().map(|a| a.platform.clone()).filter(|k| !new_p.iter().any(|n| &n.platform == k)).collect();
        if !missing.is_empty() && !super::routes::b(&v, "dropPlatforms") {
            return Err(Refusal {
                extra: Some(json!({ "missing": missing })),
                ..no(uc::PLATFORM_DROPPED, 409, format!("{to} {} serves {} and {version} doesn't; add --drop-platform to promote anyway", cur.version, missing.join(", ")))
            });
        }
    }
    let by = actor(who);
    let fallback = move_channel(&app, &to, Some(&rel), store::ACT_PROMOTE, &by)?;
    audit(&app, "updates_promote", &by, json!({ "from": from, "to": to, "version": version, "release": rel.id }));
    Ok(json!({ "ok": true, "app": app.app_id, "channel": to, "version": version, "release": rel.id, "fallback": fallback }))
}

fn rollback(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let app = the_app(who, s(&v, "app"))?;
    need_build(who)?;
    let ch = need_channel(s(&v, "channel"), "--channel")?;
    let to = s(&v, "to").ok_or_else(|| no(codes::BAD_REQUEST, 400, "--to <version> is required"))?.to_string();
    let rel = live_release(&app, &to)?;
    let cur = current_of(&app, &ch)?.ok_or_else(|| no(uc::CHANNEL_EMPTY, 409, format!("{ch} has no release")))?;
    if !au::newer(&cur.version, &rel.version) {
        return Err(no(
            uc::VERSION_NOT_NEWER,
            409,
            format!("{ch} is at {}; a rollback points it at an OLDER release. To move forward, push or promote", cur.version),
        ));
    }
    let carried = db(|c| store::channel_history(c, &app.app_id, &ch))?;
    if !carried.contains(&rel.id) {
        return Err(no(
            uc::RELEASE_NOT_LIVE,
            409,
            format!("{to} was never on {ch}; a rollback goes back to a release the channel served (k2 app updates ls {} --channel {ch})", app.app_id),
        ));
    }
    let by = actor(who);
    let fallback = move_channel(&app, &ch, Some(&rel), store::ACT_ROLLBACK, &by)?;
    audit(&app, "updates_rollback", &by, json!({ "channel": ch, "from": cur.version, "to": to }));
    Ok(json!({ "ok": true, "app": app.app_id, "channel": ch, "from": cur.version, "version": to, "fallback": fallback, "warning": ROLLBACK_WARNING }))
}

fn pull(who: &Who, body: &[u8]) -> R<J> {
    let v = json_body(body)?;
    let app = the_app(who, s(&v, "app"))?;
    need_build(who)?;
    let version = s(&v, "version").ok_or_else(|| no(codes::BAD_REQUEST, 400, "--version <version> is required"))?.to_string();
    let rel = db(|c| store::release_by_version(c, &app.app_id, &version))?
        .ok_or_else(|| no(uc::RELEASE_NOT_FOUND, 404, format!("{} has no release {version}", app.app_id)))?;
    if rel.state == store::STATE_YANKED {
        return Ok(json!({ "ok": true, "app": app.app_id, "version": version, "unchanged": true }));
    }
    if rel.state == store::STATE_UPLOADING {
        return Err(no(uc::UPLOAD_PENDING, 409, format!("{version} is still uploading; pull it after it ends")));
    }
    let by = actor(who);
    let now = deploy::now();
    // Each channel on it falls back to the newest other live release it
    // carried that is below it, else empties.
    let moved = db(|c| {
        store::set_release_state(c, &rel.id, store::STATE_YANKED, Some(&format!("pulled by {by}")), now)?;
        let mut moved = Vec::new();
        for ch in store::channels(c, &app.app_id)?.into_iter().filter(|x| x.release_id == rel.id) {
            let mut best: Option<store::ReleaseRow> = None;
            for id in store::channel_history(c, &app.app_id, &ch.channel)? {
                let Some(r) = store::release(c, &id)? else { continue };
                if r.state != store::STATE_LIVE || !au::newer(&rel.version, &r.version) {
                    continue;
                }
                if best.as_ref().is_none_or(|b| au::newer(&r.version, &b.version)) {
                    best = Some(r);
                }
            }
            store::set_channel(c, &app.app_id, &ch.channel, best.as_ref().map(|b| b.id.as_str()), store::ACT_PULL, &by, now)?;
            moved.push((ch.channel.clone(), best));
        }
        Ok(moved)
    })?;
    let handle = k2_core::db::shared();
    refresh_with(&handle, &app.app_id);
    let mut out_ch = Vec::new();
    for (ch, best) in &moved {
        let fallback = match (best, dl_id_of(&app), client(&app)) {
            (None, _, _) => "skipped: the channel is empty (ask K2 Web to unset it on the download host)".to_string(),
            (Some(_), Ok(id), Ok(c)) => publish_fallback(&handle, &c, &app.app_id, &id, ch),
            (_, Err(e), _) | (_, _, Err(e)) => format!("skipped: {}", e.hint),
        };
        out_ch.push(json!({ "channel": ch, "now": best.as_ref().map(|b| b.version.clone()), "fallback": fallback }));
    }
    audit(&app, "updates_pull", &by, json!({ "version": version, "release": rel.id, "channels": out_ch }));
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "version": version,
        "channels": out_ch,
        "note": "K2's answers no longer offer it. Its files stay at their download-host URLs (an installer someone saved keeps working); ask K2 Web to quarantine them if they must stop.",
    }))
}

// ── stats ────────────────────────────────────────────────────────────

fn stats(who: &Who, params: &HashMap<String, String>) -> R<J> {
    let app = the_app(who, params.get("app").map(String::as_str))?;
    let days: i64 = params.get("days").and_then(|d| d.trim().parse().ok()).unwrap_or(30).clamp(1, 366);
    super::updates_serve::flush();
    let now = deploy::now();
    let (from, to) = (au::day_of(now - (days - 1) * 86_400), au::day_of(now));
    let rows = db(|c| store::checkins(c, &app.app_id, &from))?;
    let users = active_users(&rows);
    let downloads = match (dl_id_of(&app), client(&app)) {
        (Ok(id), Ok(c)) => match c.stats(&id, &from, &to) {
            Ok(s) => {
                let (by, total) = dl_client::downloads_by_version(&s);
                json!({
                    "available": true,
                    "total": total,
                    "byVersion": by.iter().map(|(v, (n, b))| json!({ "version": v, "downloads": n, "bytes": b })).collect::<Vec<_>>(),
                    "raw": s,
                })
            }
            Err(e) => json!({ "available": false, "why": e.line() }),
        },
        (Err(e), _) | (_, Err(e)) => json!({ "available": false, "why": e.hint }),
    };
    Ok(json!({
        "ok": true,
        "app": app.app_id,
        "from": from,
        "to": to,
        "downloads": downloads,
        "checkins": rows,
        "activeUsers": { "last7Days": users.last7, "today": users.today, "checks": users.checks30, "counting": users.counting, "byVersion": users.by_version },
    }))
}

/// The `updates` part of `k2 app status` (Lane C composes it): channels
/// and versions, pending uploads, active users. Downloads this month are
/// the download host's (a network call), so status asks `stats` only when
/// it wants them.
pub fn status_summary(app_id: &str) -> J {
    super::updates_serve::flush();
    let r = db(|c| {
        let chans = store::channels(c, app_id)?;
        let rels = store::releases(c, app_id)?;
        let rows = store::checkins(c, app_id, &au::day_of(deploy::now() - 6 * 86_400))?;
        let key = store::key(c, app_id)?;
        let mut ch = Vec::new();
        for x in &chans {
            let v = rels.iter().find(|r| r.id == x.release_id).map(|r| r.version.clone());
            ch.push(json!({ "channel": x.channel, "version": v }));
        }
        let pending: Vec<String> = rels.iter().filter(|r| r.state == store::STATE_UPLOADING).map(|r| r.version.clone()).collect();
        let failed: Vec<String> = rels.iter().filter(|r| r.state == store::STATE_FAILED).map(|r| r.version.clone()).collect();
        Ok(json!({
            "key": key.is_some(),
            "channels": ch,
            "pendingUploads": pending,
            "failed": failed,
            "activeUsers7d": active_users(&rows).last7,
        }))
    });
    r.unwrap_or_else(|e| json!({ "error": e.hint }))
}

// ── the fleet view (K2 Web reads it later) ───────────────────────────

/// GET `/cli/app/updates/fleet`: per App, aggregate numbers only (no
/// person, no install id, no address): channels and versions, release and
/// file counts and bytes, check-ins per day by platform and version for 30
/// days. The owner and Admin logins (and the owner token); never a
/// passport. Documented in `docs/app-updates-fleet-api.md`.
fn fleet(who: &Who) -> R<J> {
    need_credential(who)?;
    if !who.is_admin() {
        return Err(no(codes::FORBIDDEN, 403, "the fleet view is the owner's and Admins' (an agent reads its own App with k2 app updates ls)"));
    }
    super::updates_serve::flush();
    let from = au::day_of(deploy::now() - 29 * 86_400);
    let apps = db(|c| {
        let mut out = Vec::new();
        for a in store::apps_with_feed(c)? {
            let chans = store::channels(c, &a)?;
            let rels = store::releases(c, &a)?;
            let mut files = 0i64;
            let mut bytes = 0i64;
            for r in &rels {
                for f in store::artifacts(c, &r.id)? {
                    files += 1;
                    bytes += f.size;
                }
            }
            let rows = store::checkins(c, &a, &from)?;
            let users = active_users(&rows);
            out.push(json!({
                "app": a,
                "dlId": au::dl_id(&super::origins::box_subdomain(), &a).ok(),
                "channels": chans.iter().map(|x| json!({
                    "channel": x.channel,
                    "version": rels.iter().find(|r| r.id == x.release_id).map(|r| r.version.clone()),
                })).collect::<Vec<_>>(),
                "releases": rels.len(),
                "releasesByState": {
                    "live": rels.iter().filter(|r| r.state == store::STATE_LIVE).count(),
                    "uploading": rels.iter().filter(|r| r.state == store::STATE_UPLOADING).count(),
                    "failed": rels.iter().filter(|r| r.state == store::STATE_FAILED).count(),
                    "pulled": rels.iter().filter(|r| r.state == store::STATE_YANKED).count(),
                },
                "files": files,
                "bytes": bytes,
                "activeUsers": { "last7Days": users.last7, "today": users.today, "counting": users.counting },
                "checkins": rows.iter().map(|r| json!({ "day": r.day, "channel": r.channel, "platform": r.platform, "version": r.version, "checks": r.checks, "installs": r.installs })).collect::<Vec<_>>(),
            }));
        }
        Ok(out)
    })?;
    Ok(json!({ "ok": true, "schema": 1, "box": super::origins::box_subdomain(), "from": from, "to": au::day_of(deploy::now()), "apps": apps }))
}

#[cfg(test)]
#[path = "updates_routes_tests.rs"]
mod tests;
