//! The verb catalog against the daemon (prd-zen-user-widgets-v2 UWA2,
//! UWA3, TUWA2, TUWA3, TUWA11).
//!
//! - Every `http` binding is a `ROUTES` row with a floor for that method;
//!   a `widget` row's binding sits at `Member` or below (a widget acts with
//!   your login, Z36). A seeded binding to `/cli/nope` fails.
//! - The renderer source walk: every `registerZenVerb('<verb>'` literal and
//!   every built-in bridge verb has a catalog row with the matching
//!   renderer binding (test files excluded: `zen-lib.test.ts` registers
//!   `zen.exit` and `homes.list` on purpose).
//! - The generated `ZEN_VERBS` holds exactly the builtin/registered rows.
//! - Per-exposure counts are pinned, so adding an exposure is reviewed.
//! - Every row's feature is a `/boot-status` key (or `zen-widgets-v1`,
//!   which this build adds).
//! - Headless (temp HOME, in-process dispatcher): GET on every write
//!   binding's route answers 405 (no GET side effects, 0.44.4).
//! - `k2 zen guide api` works with no daemon, is at most 80 lines, names
//!   every widget helper with its cap and reach, and `--json` parses.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use k2_core::contract::{catalog, gen, Catalog, Effect, Exposure, HttpMethod, RendererImpl};
use k2_daemon::routes::route_policy::{lookup, Floor};

static LOCK: Mutex<()> = Mutex::new(());

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Problems with the catalog's bindings against `ROUTES` (UWA2).
fn binding_problems(c: &Catalog) -> Vec<String> {
    let mut out = Vec::new();
    for v in &c.verbs {
        let Some(h) = &v.bindings.http else { continue };
        let Some(row) = lookup(&h.route) else {
            out.push(format!("{}: http route {} isn't a ROUTES row", v.verb, h.route));
            continue;
        };
        let floor = match h.method {
            HttpMethod::Get => row.get,
            HttpMethod::Post => row.post,
        };
        let Some(floor) = floor else {
            out.push(format!("{}: ROUTES has no {:?} floor for {}", v.verb, h.method, h.route));
            continue;
        };
        if v.exposed_to(Exposure::Widget) && !matches!(floor, Floor::Public | Floor::Member) {
            out.push(format!("{}: a widget verb's route {} must be Member or below, is {floor:?}", v.verb, h.route));
        }
    }
    out
}

#[test]
fn every_http_binding_is_a_routes_row_at_member() {
    let probs = binding_problems(catalog());
    assert!(probs.is_empty(), "{}", probs.join("\n"));
    let bound: Vec<&str> =
        catalog().verbs.iter().filter_map(|v| v.bindings.http.as_ref().map(|h| h.route.as_str())).collect();
    assert!(bound.contains(&"/cli/thread/post"), "thread.post binds /cli/thread/post: {bound:?}");
}

#[test]
fn a_binding_to_an_unknown_route_fails() {
    let mut c = catalog().clone();
    let v = c.verbs.iter_mut().find(|v| v.verb == "thread.post").expect("thread.post row");
    v.bindings.http.as_mut().expect("thread.post has http").route = "/cli/nope".into();
    let probs = binding_problems(&c);
    assert_eq!(probs, ["thread.post: http route /cli/nope isn't a ROUTES row"]);
}

#[test]
fn a_widget_binding_above_member_fails() {
    let mut c = catalog().clone();
    let v = c.verbs.iter_mut().find(|v| v.verb == "thread.post").expect("thread.post row");
    // `/cli/presence/kick` is an Admin POST row.
    v.bindings.http.as_mut().expect("http").route = "/cli/presence/kick".into();
    let probs = binding_problems(&c);
    assert_eq!(probs.len(), 1, "{probs:?}");
    assert!(probs[0].contains("must be Member or below"), "{probs:?}");
}

#[test]
fn exposure_counts_are_pinned() {
    let c = catalog();
    let widget = c.verbs_exposed_to(Exposure::Widget).len();
    let app = c.verbs_exposed_to(Exposure::App).len();
    // Cut A: widget rows only (UW16 + UWB10). Changing a count is a
    // reviewed change: update this pin in the same commit.
    // 0.45.3: + canvas.setHitRegions and window.startDrag (frame rows).
    // Garden v3 (contract-sources-v1 §6): + the six sources.* frame rows.
    // Garden data (contract-garden-data-v1 §7.2, catalog v8): + the 15
    // garden-local store.*, save.* and viewer.get rows (never widget/app).
    // Runtime S23 (catalog v10): + one app row per live App-facing pair
    // (56: the 53 the hand-written table had, plus thread/latest R5,
    // skin/me R6, the link's skin/login and Sew's sew/pass) and socket (3).
    // RS-A15 (0.46.1): + guests.rooms / guests.setRooms on both transports
    // (app + garden-local). Room-scope 0.46.1: + room.members, app.self and
    // app.usage on both transports and the Garden's widget.self (garden-local).
    // Room-scope R3: + files.stat (GET /cli/fs/stat).
    // Thread parity (0.46.3, catalog v16): widget + thread.attachment,
    // thread.open, thread.links, thread.pickFiles and thread.markRead (was
    // none, UW51); app + thread.attachmentGet, thread.catchup,
    // thread.setRead, thread.unread and files.resolveLinks.
    // Guest read state + Garden person rows (0.46.3, catalog v17): app +
    // thread.mute (app-read-state-v1, on both transports); garden-local +
    // thread.mute and 11 App rows that joined the Garden door
    // (garden-person-rows-v1): thread.catchup, thread.unread,
    // tickets.list/show/create/comment/resolve, activity.snapshot,
    // files.list/read/readBinary. No project chat row on the Garden.
    // App project chat (0.46.3, catalog v18): app + project.list,
    // project.read, project.post, project.markRead and project.events
    // (App only, never garden-local).
    // App mobile access (0.46.3, folded into catalog v19): app + 7 rows,
    // guests.logout, guests.refresh, guests.devices, guests.revokeDevice,
    // app.info, password.forgot and password.reset (its thread.catchup and
    // thread.attachmentGet are Thread parity's rows above).
    // app-room-stream (0.46.3, catalog v19): + the thread.stream socket (app)
    // and the Garden's thread.inbox (widget, a registered bridge row).
    // App users' wallets (0.46.3, catalog v19): + the nine wallet.* app rows
    // (helper door): wallet.list, status, add, login, loginStatus,
    // loginInput, loginCancel, use and remove.
    // The counts-only ear (0.46.3, thread-activity-ear-v1): + thread.activity
    // (app only).
    // Garden columns and the agent list (catalog v20, 0.46.3 r10): + six
    // widget rows, layout.get/set/toggle/subscribe, agents.show and
    // conversation.setContext.
    // Box usage's Garden card (0.46.4, disk guard Slice B): + app.resources
    // (garden-local only).
    let garden_local = c.verbs_exposed_to(Exposure::GardenLocal).len();
    // Catalog v21 (0.46.4): + store.patch (garden-local) and links.open (widget).
    // Catalog v22 (0.46.4, prd-app-caps-0464-v1): + app.rooms and guests.list
    // (app + garden-local) and files.downloads / files.download (app).
    // Catalog v23 (0.46.4, custom-app-push-v1): + 7 app rows, push.register,
    // unregister, device, prefs (a guest's own) and push.send, status,
    // devices (the App's link).
    // Catalog v24 (0.46.5, app-phone-pairing-v1): + 5 app rows,
    // pairing.start, status, cancel (a guest's own web pass), check and
    // redeem (Public, the code).
    // Catalog v25 (0.46.5, app-user-profile-v1): + 7 app rows, profile.get,
    // update, setImage, clearImage, sessions, revokeSession, signOutOthers
    // (a person's own).
    // Catalog v26 (0.46.5, email-notifications-v1): + email.send, status,
    // log (the App's link).
    // Catalog v27 (0.46.5, thread-read-all-v1): + thread.markAllRead (app).
    // Catalog v28 (0.46.5, store batch patch ops): no new rows (patch ops
    // in store.batch and transactions; a conflict names its op).
    // Catalog v29 (0.46.5, the activity SDK): + thread.onActivity (widget)
    // and thread.activityRooms (app).
    // Catalog v30 (0.46.5, Garden blobs, prd-garden-board-needs-v1 slice 1):
    // + blobs.put/get and downloads.begin/save (widget), blobs.info/list
    // (garden-local).
    // Catalog v31 (0.46.6, ticket attachments): + tickets.attachment (app,
    // garden-local).
    // Catalog v32 (0.46.6, Garden Board needs slices 2-3): + uploads.pick and
    // uploads.dropped (widget), store.history/restore/snapshot (garden-local).
    // Catalog v34 (0.46.6, app-chat-v1, Sew for Apps): + guests.chat,
    // guests.setChat, chat.member, chat.whois (app).
    // Catalog v35 (0.46.6, Sew for Apps A9): + chat.createChannel (app).
    // Catalog v40 (workbench-v1, the Workbench Garden): + agents.openTerminal
    // (widget) and 10 garden-local rows, work.list, git.snapshot/diffFile/
    // log/land/note, compute.jobs, agents.start/stop/settle. None is an App
    // row: app stays 129.
    assert_eq!((widget, app, garden_local), (45, 129, 53), "per-exposure verb counts (widget, app, garden-local)");
    let both: Vec<&str> = c
        .verbs
        .iter()
        .filter(|v| v.exposed_to(Exposure::App) && v.exposed_to(Exposure::GardenLocal))
        .map(|v| v.verb.as_str())
        .collect();
    assert_eq!(both.len(), 20, "5 room-scope rows + the 13 person rows (catalog v31 adds tickets.attachment) + catalog v22's app.rooms and guests.list: {both:?}");
    for verb in [
        "thread.catchup",
        "thread.unread",
        "thread.mute",
        "tickets.list",
        "tickets.show",
        "tickets.create",
        "tickets.comment",
        "tickets.resolve",
        "tickets.attachment",
        "activity.snapshot",
        "files.list",
        "files.read",
        "files.readBinary",
    ] {
        assert!(both.contains(&verb), "{verb} is on both transports");
    }
    assert!(!both.iter().any(|v| v.starts_with("projects.") || v.starts_with("project.")), "no project chat on the Garden bridge");
    for verb in ["project.list", "project.read", "project.post", "project.markRead", "project.events"] {
        let row = c.verb(verb).unwrap_or_else(|| panic!("{verb} row"));
        assert!(row.exposed_to(Exposure::App) && !row.exposed_to(Exposure::GardenLocal), "{verb}: App only");
    }
    // Catalog v33 (0.46.6, Gardens on the phone): the phone column, no new rows.
    // Catalog v34 (0.46.6, Sew for Apps): + guests.chat, guests.setChat,
    // chat.member, chat.whois (app).
    // Catalog v35 (0.46.6, Sew for Apps A9): + chat.createChannel (app).
    // Catalog v36 (0.46.6, email notifications part 2): email.status
    // recipientStatus, email.log bounced/complained, errors email_paused and
    // all_suppressed; no new rows.
    // Catalog v37 (0.46.6, secret cards that say how a secret is sent): the
    // secret card's inject/envName/replaces and thread.answerCard's inject
    // echo, no new rows.
    // Catalog v38 (0.46.6, must-change at next sign-in): errors
    // password_change_required (guests.login) and password_unchanged
    // (password.reset); no new rows.
    assert_eq!(c.catalog_version, 38);
    assert_eq!(
        c.verbs.len(),
        226,
        "today's 38 bridge verbs (thread.inbox joined in 0.46.3) plus the frame rows theme.changed, canvas.setHitRegions, window.startDrag, sources.*, the 15 Garden data rows, the 66 app rows (5 on both transports) and widget.self, plus Thread parity 0.46.3's 4 widget and 5 app rows, plus thread.mute, plus App project chat's 5 app rows, plus App mobile access's 7 app rows, plus app-room-stream's thread.stream app row, plus the nine wallet.* app rows (0.46.3), plus the counts-only ear's thread.activity app row (0.46.3), plus permission cards' 9 unexposed permission.* rows (7 + A2's note and undo person routes; 0.46.3, no catalog bump), plus catalog v20's Garden columns and agent list rows (layout.get/set/toggle/subscribe, agents.show, conversation.setContext; 0.46.3 r10), plus catalog v21's store.patch and links.open (0.46.4), plus box usage's garden-local app.resources (0.46.4, no catalog bump), plus catalog v22's app.rooms, guests.list, files.downloads and files.download (0.46.4), plus catalog v23's App push rows push.register/unregister/device/prefs and the link's push.send/status/devices (0.46.4), plus catalog v24's App phone pairing rows pairing.start/status/cancel/check/redeem (0.46.5), plus catalog v25's App user profile rows profile.get/update/setImage/clearImage/sessions/revokeSession/signOutOthers (0.46.5), plus catalog v26's email.send/status/log (0.46.5), plus catalog v27's thread.markAllRead (0.46.5), plus catalog v29's thread.onActivity (widget) and thread.activityRooms (app) (0.46.5), plus catalog v30's blobs.put/get/info/list and downloads.begin/save (0.46.5), plus catalog v31's tickets.attachment (0.46.6), plus catalog v32's uploads.pick, uploads.dropped and store.history/restore/snapshot (0.46.6), plus catalog v33's guests.chat, guests.setChat, chat.member and chat.whois (0.46.6, Sew for Apps), plus catalog v35's chat.createChannel (0.46.6, Sew for Apps A9)"
    );
    // Permission cards (0.46.3 Beta): K2's own verbs, never a widget's or
    // an App's (App guests never see a permission card).
    let perm: Vec<&str> = c.verbs.iter().filter(|v| v.verb.starts_with("permission.")).map(|v| v.verb.as_str()).collect();
    assert_eq!(
        perm,
        ["permission.request", "permission.update", "permission.cancel", "permission.status", "permission.list", "permission.enabled", "permission.pending", "permission.note", "permission.undo"]
    );
    for verb in perm {
        let row = c.verb(verb).expect("row");
        assert!(row.exposure.is_empty(), "{verb} is exposed to nobody outside K2");
        assert_eq!(row.feature, k2_core::permission::FEATURE_KEY, "{verb}");
    }
}

#[test]
fn every_feature_is_a_boot_status_key() {
    // The DB-gated keys (thread-read-v1, inbox-catchup-v1,
    // app-thread-parity-v1, app-read-state-v1, garden-person-rows-v1) are
    // on once the DB is up.
    let _ = k2_core::db::shared();
    let mut known: BTreeSet<&str> = k2_daemon::boot_status::features().into_iter().collect();
    for key in ["app-read-state-v1", "garden-person-rows-v1"] {
        assert!(known.contains(key), "{key} is a /boot-status key once the DB is up");
    }
    assert!(k2_core::zen::widgets::KNOWN_FEATURES.contains(&"garden-person-rows-v1"), "a widget may require the Garden rows");
    assert_eq!(catalog().verb("thread.mute").map(|v| v.feature.as_str()), Some("app-read-state-v1"));
    // Added by this build (UW36); B2 puts it in /boot-status.
    known.insert("zen-widgets-v1");
    // DB-gated keys (reported once the daemon DB is up): this test opens no DB.
    known.insert(k2_daemon::boot_status::FEATURE_INBOX_CATCHUP);
    // Listed only while the "Permission cards" switch is on (lane B's
    // dynamic key); this test leaves the switch off.
    known.insert(k2_core::permission::FEATURE_KEY);
    for v in &catalog().verbs {
        assert!(known.contains(v.feature.as_str()), "{}: feature {} isn't a /boot-status key", v.verb, v.feature);
    }
}

// ── the renderer source walk (UWA2) ─────────────────────────────────────

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display())) {
        let p = e.expect("dir entry").path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "ts" || x == "tsx") {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !(name.contains(".test.") || name.contains(".spec.")) {
                out.push(p);
            }
        }
    }
}

/// `registerZenVerb('<verb>'` literals in non-test renderer sources.
fn registered_verbs() -> BTreeSet<String> {
    let mut files = Vec::new();
    walk(&root().join("src/renderer"), &mut files);
    let mut out = BTreeSet::new();
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("read {}: {e}", f.display()));
        for (i, _) in src.match_indices("registerZenVerb(") {
            let rest = &src[i + "registerZenVerb(".len()..];
            let Some(q) = rest.chars().next().filter(|c| *c == '\'' || *c == '"') else { continue };
            let lit: String = rest[1..].chars().take_while(|c| *c != q).collect();
            out.insert(lit);
        }
    }
    out
}

/// The built-in bridge verbs: `BUILTIN_VERBS` in `zen-bridge.ts`.
fn builtin_verbs() -> BTreeSet<String> {
    let src = std::fs::read_to_string(root().join("src/renderer/lib/zen/zen-bridge.ts")).expect("read zen-bridge.ts");
    let at = src.find("BUILTIN_VERBS = new Set").expect("zen-bridge.ts keeps a BUILTIN_VERBS set (this walk reads it)");
    let open = src[at..].find("([").expect("BUILTIN_VERBS = new Set<…>([ … ])") + at + 2;
    let close = src[open..].find("])").expect("end of BUILTIN_VERBS") + open;
    src[open..close]
        .split(',')
        .map(|s| s.trim().trim_matches(|c| c == '\'' || c == '"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[test]
fn every_renderer_verb_has_a_catalog_row() {
    let c = catalog();
    let registered = registered_verbs();
    assert!(registered.len() >= 24, "the walk found too few registerZenVerb literals: {registered:?}");
    for v in &registered {
        let row = c.verb(v).unwrap_or_else(|| panic!("registerZenVerb('{v}') has no catalog row"));
        let imp = row.bindings.renderer.as_ref().map(|r| r.implementation);
        assert_eq!(imp, Some(RendererImpl::Registered), "{v} is registered, so its renderer impl is `registered`");
    }
    let builtins = builtin_verbs();
    assert_eq!(builtins.len(), 12, "today's 12 built-in bridge verbs: {builtins:?}");
    for v in &builtins {
        let row = c.verb(v).unwrap_or_else(|| panic!("built-in bridge verb {v} has no catalog row"));
        let imp = row.bindings.renderer.as_ref().map(|r| r.implementation);
        assert_eq!(imp, Some(RendererImpl::Builtin), "{v} is built into zen-bridge.ts");
    }
}

/// The generated `ZEN_VERBS` keys.
fn generated_zen_verbs() -> BTreeSet<String> {
    let src = std::fs::read_to_string(root().join(gen::ZEN_VERBS_TS)).expect("read zen-verbs.generated.ts");
    let open = src.find("export const ZEN_VERBS = {").expect("ZEN_VERBS in the generated file");
    let close = src[open..].find("} as const").expect("end of ZEN_VERBS") + open;
    src[open..close]
        .lines()
        .skip(1)
        .filter_map(|l| l.trim().strip_prefix('\'').and_then(|r| r.split_once('\'')).map(|(k, _)| k.to_string()))
        .collect()
}

#[test]
fn generated_zen_verbs_holds_exactly_the_bridge_rows() {
    let c = catalog();
    let generated = generated_zen_verbs();
    for v in &c.verbs {
        let imp = v.bindings.renderer.as_ref().map(|r| r.implementation);
        match imp {
            Some(RendererImpl::Builtin | RendererImpl::Registered) => {
                assert!(generated.contains(&v.verb), "{} is missing from the generated ZEN_VERBS", v.verb)
            }
            Some(RendererImpl::Frame) => {
                assert!(!generated.contains(&v.verb), "frame row {} must not be in ZEN_VERBS", v.verb)
            }
            None => {}
        }
    }
    // Today's 37 plus Thread parity 0.46.3's thread.attachment, thread.open,
    // thread.links and thread.pickFiles, plus app-room-stream's thread.inbox,
    // plus catalog v20's agents.show and conversation.setContext.
    assert_eq!(generated.len(), 45, "+ the 0.46.5 activity SDK's thread.onActivity");
}

// ── TUWA3: no GET side effects, headless ────────────────────────────────

const OWNER_TOKEN: &str = "owner-token-contract-routes-b1";

fn get_status(port: u16, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    s.set_read_timeout(Some(Duration::from_secs(15))).expect("read timeout");
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).expect("write request");
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).expect("read response");
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or_else(|| panic!("no status in {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_on_every_write_binding_is_405() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var_os("HOME");
    let home = std::env::temp_dir().join(format!("k2-contract-routes-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).expect("temp HOME");
    std::env::set_var("HOME", &home);
    let d = k2_daemon::test_harness::start(OWNER_TOKEN).await;
    // A route that is also a read row's GET binding (`/cli/db/rows`,
    // `/cli/agent/identity`) answers its GET as that read: it is covered
    // by the read row, not here.
    let read_gets: BTreeSet<String> = catalog()
        .verbs
        .iter()
        .filter(|v| v.effect == Effect::Read)
        .filter_map(|v| v.bindings.http.as_ref().filter(|h| h.method == HttpMethod::Get).map(|h| h.route.clone()))
        .collect();
    let writes: Vec<(String, String)> = catalog()
        .verbs
        .iter()
        .filter(|v| v.effect == Effect::Write)
        .filter_map(|v| v.bindings.http.as_ref().map(|h| (v.verb.clone(), h.route.clone())))
        .filter(|(_, route)| !read_gets.contains(route))
        .collect();
    assert!(read_gets.contains("/cli/db/rows") && read_gets.contains("/cli/agent/identity"), "{read_gets:?}");
    assert!(writes.len() >= 3, "thread.post, thread.answer and thread.void bind routes: {writes:?}");
    // The permission-cards family is absent while its switch is off (PRD
    // rev 3.1 §2.4, C1: today's answer, byte for byte); this daemon leaves it
    // off, so its write rows must NOT answer 405 here (the family's own
    // coded 405 is lane A1's, tested with the switch on).
    assert!(!k2_core::permission::gate::server_on(), "this binary leaves the Permission cards switch off");
    for (verb, route) in &writes {
        let path = format!("{route}?token={OWNER_TOKEN}&addr=x&text=x&id=x&answer=x");
        let port = d.port;
        let (status, body) = tokio::task::spawn_blocking(move || get_status(port, &path)).await.expect("join");
        if k2_core::permission::gate::route_gated(route) {
            assert_ne!(status, 200, "GET {route} ({verb}) is not served while permission cards are off; body={body}");
            assert!(!body.contains("method_not_allowed"), "GET {route} ({verb}) answers as an unknown route while off; body={body}");
            continue;
        }
        assert_eq!(status, 405, "GET {route} ({verb}) must be 405; body={body}");
    }
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    k2_core::test_env::remove_test_dir(&home);
}

// ── TUWA11: k2 zen guide api ────────────────────────────────────────────

/// Run `cli/k2 zen guide api <args>` the way an agent with no daemon would.
fn guide_api(args: &[&str]) -> String {
    let home = std::env::temp_dir().join(format!("k2-guide-api-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).expect("empty HOME");
    let out = std::process::Command::new(k2_core::test_env::bash_program())
        .arg(root().join("cli/k2"))
        .args(["zen", "guide", "api"])
        .args(args)
        .env_clear().envs(k2_core::test_env::cli_base_env())
        .env("HOME", &home)
        .env("PATH", k2_core::test_env::cli_path("/usr/bin:/bin:/usr/sbin:/sbin"))
        .output()
        .expect("run cli/k2 zen guide api");
    std::fs::remove_dir_all(&home).expect("cleanup");
    assert!(
        out.status.success(),
        "k2 zen guide api {args:?} exited {:?}: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

#[test]
fn guide_api_works_without_a_daemon() {
    let page = guide_api(&[]);
    assert_eq!(page.trim_end(), gen::guide_api_page(catalog()).trim_end(), "cli/k2 prints the generated page");
    assert!(page.lines().count() <= gen::GUIDE_MAX_LINES, "guide api is {} lines", page.lines().count());
    let c = catalog();
    for (cap, rows) in gen::widget_groups(c) {
        let head = cap.unwrap_or("no cap");
        let at = page.find(&format!("\n{head}")).unwrap_or_else(|| panic!("no group {head}"));
        for v in rows {
            let line = page
                .lines()
                .find(|l| l.starts_with(&format!("  k2.{} ", gen::signature(v))))
                .unwrap_or_else(|| panic!("no line for {}", v.verb));
            let reach = if matches!(v.reach, k2_core::contract::Reach::Portable) { "portable" } else { "local" };
            assert!(line.contains(reach), "{}: {line}", v.verb);
            assert!(page.find(line).expect("line in page") > at, "{} sits under {head}", v.verb);
        }
    }
    let json: serde_json::Value = serde_json::from_str(guide_api(&["--json"]).trim()).expect("--json parses");
    assert_eq!(json["id"], "api");
    assert_eq!(json["title"], "The k2 object in a custom widget");
    assert!(json["body"].as_str().is_some_and(|b| b.starts_with("k2 zen guide api —")));
    let index = std::process::Command::new(k2_core::test_env::bash_program())
        .arg(root().join("cli/k2"))
        .args(["zen", "guide"])
        .env_clear().envs(k2_core::test_env::cli_base_env())
        .env("HOME", std::env::temp_dir())
        .env("PATH", k2_core::test_env::cli_path("/usr/bin:/bin"))
        .output()
        .expect("index");
    assert!(String::from_utf8_lossy(&index.stdout).contains("  api "), "the index lists api");
}
