//! A′ (app-mobile-access-v1, 0.46.3; `research-app-mobile-surface-parity-v1`
//! §6 R1/R2): the catalog is the wall for App passes on the daemon's main
//! port too.
//!
//! Before 0.46.3 only the browser helper and an App's gateway link enforced
//! the App-facing route table ([`k2_core::app_surface`]); on the main port
//! (where native mobile Apps call with `Authorization: Bearer k2skn_…`)
//! each route arm checked the pass itself, so one careless arm was a guest
//! door. Now one check runs before the route match ([`fences`]): when a
//! request carries a live `k2skn_` pass and `(method, path)` is not a
//! catalog row exposed to `app` (any door, [`k2_core::app_surface::verb_for`])
//! or an App socket, the request is **fenced**: the dispatcher's one pass
//! resolver (`apps::gateway::resolve_pass`) answers "no pass" for the rest
//! of it, so no arm on that route can act on the pass, however it is
//! written.
//!
//! - The request still reaches its arm, so every answer keeps today's bytes
//!   (clients and Dannon's must-not-break tests pin them: the classic 403,
//!   the terminal/chatter/design-note teaching 403s, a 409 before auth): an
//!   arm that refused a pass refuses it the same way; only an arm that
//!   would have *accepted* one (the bug class) now sees none.
//! - Not on an App's gateway link: that door runs its own gate on the same
//!   table first (`apps::gateway::gate`), and also serves `/gw/me`.
//! - The never list (R2: terminals, grid, `/cli/sessions/events`,
//!   `/events`, `/v1`, admin, LLM settings, update, presence, homes) has no
//!   catalog rows, so it is fenced by construction; the tests pin it.
//! - The dispatcher walk test (below) fails for any pass-touching arm whose
//!   route has no catalog row (or that isn't a known refusing arm).
//! - Switch (Rosson's standing rule): `K2_APP_PASS_GATE=off` in the
//!   daemon's environment is today's behaviour (no fence). On by default.

use std::cell::{Cell, RefCell};

/// The daemon environment switch; `off` (or `0`/`false`) = no fence.
pub const SWITCH_ENV: &str = "K2_APP_PASS_GATE";

/// Is the fence on (default yes)?
pub fn enabled() -> bool {
    !matches!(
        std::env::var(SWITCH_ENV).ok().map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("off" | "0" | "false" | "no")
    )
}

/// May a request carrying an App pass reach `method path` at all? (Pure:
/// a catalog `app` row for that pair, or a GET to an App socket.)
pub fn admits(method: &str, path: &str) -> bool {
    let p = path.split('?').next().unwrap_or(path);
    k2_core::app_surface::verb_for(method, p).is_some()
        || (method.eq_ignore_ascii_case("GET") && k2_core::app_surface::ws_param(p).is_some())
}

/// Should this request be fenced? `query` is the effective auth query
/// (Bearer, `?token=` and cookie folded in). Only a **live** pass is fenced
/// (a dead one resolves to nothing anyway, so its answers stay today's).
pub fn fences(on_link: bool, method: &str, path: &str, query: &str) -> bool {
    note_request(method, path, query);
    if on_link || !enabled() {
        return false;
    }
    let Some(tok) = super::http::extract_token(query) else { return false };
    k2_core::skin::is_skin_token(tok)
        && !k2_core::skin::is_api_key_prefix(tok)
        && !admits(method, path)
        && k2_core::skin::resolve_skin_token(tok).is_some()
}

tokio::task_local! {
    static FENCE: Cell<bool>;
    static REQUEST: RefCell<Noted>;
}

/// Run one connection with its per-request fence (the dispatcher's loop)
/// and the request the permission-card refusal wrapper reads.
pub async fn scope<F: std::future::Future>(f: F) -> F::Output {
    FENCE.scope(Cell::new(false), REQUEST.scope(RefCell::new(Noted::default()), f)).await
}

/// Set the current request's fence (the dispatcher, before the route match
/// and at the top of every request). Also makes the request noted by
/// [`fences`] (if any) the current one: at the top of a request nothing is
/// pending, so the previous request's context is cleared.
pub fn set_fenced(on: bool) {
    let _ = FENCE.try_with(|c| c.set(on));
    let _ = REQUEST.try_with(|r| {
        let mut r = r.borrow_mut();
        r.current = r.pending.take();
    });
}

// ── Permission cards (PRD rev 3.1 §3.1, vs-live C2; lane C) ──────────────
//
// `handle_one_request` answers through hundreds of direct `send_response`
// calls, so the refusal wrapper (`permission::walls::teach_tcp`) sits in
// `http::send_response*` and reads the request from this task-local. The
// dispatcher already calls [`fences`] once per request with the method,
// path and effective auth query, right before [`set_fenced`]; `fences`
// notes them (pending) and `set_fenced` makes them current. No dispatcher
// edit, no second wrapper. Nothing is noted while the global "Permission
// cards" switch is off, or for a request with no token.

/// One request as the refusal wrapper sees it.
#[derive(Debug, Clone, Default)]
pub(crate) struct NotedRequest {
    pub method: String,
    pub path: String,
    /// The effective auth query (Bearer / `?token=` / cookie folded in).
    pub query: String,
    /// The request's JSON-object body, once an arm has read it.
    pub body: Option<serde_json::Value>,
    /// The request's form body params, once an arm has read them.
    pub form: std::collections::HashMap<String, String>,
}

#[derive(Debug, Default)]
struct Noted {
    pending: Option<NotedRequest>,
    current: Option<NotedRequest>,
}

fn note_request(method: &str, path: &str, query: &str) {
    if !k2_core::permission::gate::server_on() {
        return;
    }
    if super::http::extract_token(query).is_none_or(str::is_empty) {
        return;
    }
    let _ = REQUEST.try_with(|r| {
        r.borrow_mut().pending = Some(NotedRequest {
            method: method.to_string(),
            path: path.split('?').next().unwrap_or(path).to_string(),
            query: query.to_string(),
            body: None,
            form: Default::default(),
        });
    });
}

/// Record the current request's body (`http::read_post_body*`). A no-op
/// unless a request is noted: a JSON object is kept as is, anything else
/// is read as a form.
pub(crate) fn note_body(raw: &[u8]) {
    let _ = REQUEST.try_with(|r| {
        let mut r = r.borrow_mut();
        let Some(cur) = r.current.as_mut() else { return };
        let Ok(s) = std::str::from_utf8(raw) else { return };
        if s.trim_start().starts_with('{') {
            if let Ok(v @ serde_json::Value::Object(_)) = serde_json::from_str::<serde_json::Value>(s) {
                cur.body = Some(v);
            }
        } else if !s.trim().is_empty() {
            cur.form = super::http::parse_form_body(raw);
        }
    });
}

/// Run `f` on the current noted request, if there is one.
pub(crate) fn with_request<T>(f: impl FnOnce(&NotedRequest) -> T) -> Option<T> {
    REQUEST.try_with(|r| r.borrow().current.as_ref().map(f)).ok().flatten()
}

/// Is the current request fenced? (`apps::gateway::resolve_pass`.)
pub fn fenced() -> bool {
    FENCE.try_with(|c| c.get()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// R2: owner-level surfaces a pass must never reach on the main port.
    pub(crate) const NEVER: &[(&str, &str)] = &[
        ("GET", "/cli/sessions/grid"),
        ("GET", "/cli/sessions/bytes"),
        ("GET", "/cli/sessions/events"),
        ("GET", "/cli/sessions/subscribe"),
        ("GET", "/events"),
        ("POST", "/cli/sessions/v2/spawn"),
        ("POST", "/cli/terminal/write"),
        ("GET", "/cli/terminal/read"),
        ("POST", "/cli/terminal/send-message"),
        ("GET", "/cli/chat/transcript"),
        ("GET", "/v1/sandboxes"),
        ("POST", "/v1/w"),
        ("GET", "/cli/presence/roster"),
        ("GET", "/cli/presence/summary"),
        ("POST", "/cli/daemon/update/start"),
        ("GET", "/cli/daemon/update/check"),
        ("GET", "/cli/llm/accounts/list"),
        ("GET", "/cli/me/homes"),
        ("GET", "/cli/settings/get"),
        ("POST", "/cli/settings/update"),
        ("POST", "/cli/users/set-role"),
        ("GET", "/cli/skin/users"),
        ("POST", "/cli/skin/users/rooms"),
        ("POST", "/cli/skin/roles"),
        ("GET", "/cli/skin/access"),
        ("POST", "/cli/skin/access/settings"),
        ("GET", "/cli/chatter"),
        ("POST", "/cli/browser/notes"),
        ("GET", "/cli/projects/list"),
        ("GET", "/cli/feedback/list-all"),
        ("POST", "/cli/vault/get"),
        ("GET", "/cli/auth/whoami"),
    ];

    fn pass_query() -> String {
        "token=k2skn_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string()
    }

    #[test]
    fn every_catalog_app_row_and_socket_is_admitted() {
        for (m, p, verb, _) in k2_core::app_surface::VERBS {
            let m = if *m == "WS" { "GET" } else { m };
            assert!(admits(m, p), "{verb} ({m} {p}) is an app row: the gate must admit it");
            assert!(!fences(false, m, p, &pass_query()), "{verb}");
        }
        // The two arms that took passes before they had rows (catch-up,
        // attachments) now have them.
        assert!(admits("POST", "/cli/thread/catchup"));
        assert!(admits("GET", "/cli/thread/attachment"));
        assert!(admits("POST", "/cli/skin/logout"));
        // 0.46.3: the ear's catch-up is an App row (its handler gates the
        // room, thread:read and activity:read, and answers counts only).
        assert!(admits("GET", "/cli/thread/activity"));
        assert!(!admits("POST", "/cli/thread/activity"));
    }

    /// A live legacy App's routes (in production on a customer box): every
    /// one must stay admitted for a `k2skn_` pass on the main port, so a
    /// later catalog edit can't silently close one. A cap change (such as
    /// 0.46.3's `activity:room`) may change what a route answers, never
    /// whether a pass reaches it. `WS` = a GET upgrade.
    const LIVE_LEGACY_APP_ROUTES: &[(&str, &str)] = &[
        ("POST", "/cli/skin/logout"),
        ("GET", "/cli/skin/agents"),
        ("POST", "/cli/skin/password/change"),
        ("GET", "/cli/thread"),
        ("POST", "/cli/thread/post"),
        ("POST", "/cli/thread/answer"),
        ("POST", "/cli/workspace/ensure-pinned-chat"),
        ("WS", "/cli/overlay/events"),
        ("GET", "/cli/activity/snapshot"),
        ("WS", "/cli/activity/events"),
        ("GET", "/cli/heartbeat/list"),
        ("GET", "/cli/heartbeat/show"),
        ("GET", "/cli/feedback/list"),
        ("GET", "/cli/feedback/show"),
        ("POST", "/cli/feedback/comment"),
        ("GET", "/cli/fs/read-dir"),
        ("GET", "/cli/fs/read-file"),
        ("GET", "/cli/fs/read-binary"),
        ("GET", "/cli/fs/read-range"),
        ("POST", "/cli/fs/upload-binary"),
        ("POST", "/cli/fs/create"),
        ("POST", "/cli/fs/copy"),
        ("POST", "/cli/fs/move"),
        ("POST", "/cli/fs/rename"),
        ("GET", "/cli/workspace/resources"),
        ("POST", "/cli/workspace/resources/add"),
        ("POST", "/cli/workspace/resources/remove"),
    ];

    #[test]
    fn a_live_legacy_apps_routes_stay_admitted_for_a_pass() {
        for (m, p) in LIVE_LEGACY_APP_ROUTES {
            let (method, is_ws) = if *m == "WS" { ("GET", true) } else { (*m, false) };
            assert!(admits(method, p), "{m} {p}: a live legacy App calls it with a pass; it must stay admitted");
            assert!(!fences(false, method, p, &pass_query()), "{m} {p} must not be fenced");
            if is_ws {
                assert!(k2_core::app_surface::ws_param(p).is_some(), "{p} must stay an App socket");
            } else {
                assert!(k2_core::app_surface::verb_for(method, p).is_some(), "{m} {p} must stay a catalog app row");
            }
            // The query string never closes a route.
            assert!(admits(method, &format!("{p}?workspace=x")), "{m} {p}?…");
        }
    }

    #[test]
    fn the_never_list_is_closed_to_a_pass() {
        for (m, p) in NEVER {
            assert!(!admits(m, p), "{m} {p} must never be open to an App pass (R2)");
        }
    }

    #[tokio::test]
    async fn the_fence_closes_the_one_resolver_for_its_request_only() {
        scope(async {
            assert!(!fenced());
            set_fenced(true);
            assert!(fenced());
            // Answers before any lookup: a fenced request never has a pass.
            assert!(crate::apps::gateway::resolve_pass(&pass_query()).is_none());
            set_fenced(false);
            assert!(!fenced(), "the next request starts unfenced");
        })
        .await;
        set_fenced(true);
        assert!(!fenced(), "outside a connection scope there is no fence to set");
    }

    #[test]
    fn other_credentials_and_the_link_pass_through() {
        assert!(!fences(false, "GET", "/cli/sessions/grid", "token=owner-token"));
        assert!(!fences(false, "GET", "/cli/sessions/grid", ""));
        assert!(!fences(false, "GET", "/cli/sessions/grid", "token=k2sk_api_key"), "API keys are not passes");
        assert!(!fences(true, "GET", "/gw/me", &pass_query()), "the link has its own gate");
        assert!(!fenced(), "outside a request nothing is fenced");
    }

    /// Every `/cli` route the daemon classifies: a pass reaches it iff it
    /// is a catalog row (so no route arm can take a pass without one).
    #[test]
    fn every_policy_route_is_closed_to_a_pass_unless_it_is_a_catalog_row() {
        let mut open = BTreeSet::new();
        for r in super::super::route_policy::ROUTES {
            for (m, has) in [("GET", r.get.is_some()), ("POST", r.post.is_some())] {
                if !has {
                    continue;
                }
                let catalogued = k2_core::app_surface::verb_for(m, r.path).is_some()
                    || (m == "GET" && k2_core::app_surface::ws_param(r.path).is_some());
                assert_eq!(admits(m, r.path), catalogued, "{m} {}", r.path);
                if catalogued {
                    open.insert(format!("{m} {}", r.path));
                }
            }
        }
        assert!(open.len() >= 60, "too few open pairs: {open:?}");
    }

    // ── the dispatcher walk ──────────────────────────────────────────

    /// Constant route names the dispatcher matches on, by expression.
    fn route_consts() -> Vec<(&'static str, Vec<&'static str>)> {
        vec![
            ("crate::overlay_ws::OVERLAY_WS_PATH", vec![crate::overlay_ws::OVERLAY_WS_PATH]),
            ("crate::fs_events_ws::FS_EVENTS_WS_PATH", vec![crate::fs_events_ws::FS_EVENTS_WS_PATH]),
            ("crate::activity_events_ws::ACTIVITY_EVENTS_WS_PATH", vec![crate::activity_events_ws::ACTIVITY_EVENTS_WS_PATH]),
            ("crate::chat_overlay_ws::CHAT_TRANSCRIPT_WS_PATH", vec![crate::chat_overlay_ws::CHAT_TRANSCRIPT_WS_PATH]),
            ("crate::thread_attachments::ROUTE", vec![crate::thread_attachments::ROUTE]),
            ("crate::skin_me_routes::ROUTE", vec![crate::skin_me_routes::ROUTE]),
            ("crate::guest_rooms_routes::ROUTE", vec![crate::guest_rooms_routes::ROUTE]),
            ("crate::overlay_routes::catchup::ROUTE", vec![crate::overlay_routes::catchup::ROUTE]),
            ("crate::apps::surfaces::PASS_ROUTES", crate::apps::surfaces::PASS_ROUTES.to_vec()),
            (
                "crate::heartbeat_app_routes::is_app_door",
                crate::heartbeat_app_routes::READ_PATHS.iter().chain(crate::heartbeat_app_routes::WRITE_PATHS.iter()).copied().collect(),
            ),
            ("crate::app_mobile_routes::PASS_ROUTES", crate::app_mobile_routes::PASS_ROUTES.to_vec()),
            // 0.46.3 lanes folded in at r4: App project chat's socket, the
            // App room stream and the read-state arm.
            ("crate::apps::project_chat::EVENTS_WS_PATH", vec![crate::apps::project_chat::EVENTS_WS_PATH]),
            ("crate::room_stream_ws::ROOM_STREAM_WS_PATH", vec![crate::room_stream_ws::ROOM_STREAM_WS_PATH]),
            (
                "crate::read_routes::is_read_route",
                vec![
                    crate::read_routes::UNREAD_PATH,
                    crate::read_routes::READ_PATH,
                    crate::read_routes::MUTE_PATH,
                    crate::read_routes::PROJECT_READ_PATH,
                ],
            ),
        ]
    }

    /// Arms that look at a pass only to refuse it (403 before anything
    /// runs): they need no row, and the gate must keep refusing them.
    const REFUSING_ARMS: &[&str] = &[
        "/cli/sessions/bytes",
        "/cli/sessions/grid",
        "/cli/chat/transcript",
        "/cli/browser/notes",
        "/cli/browser/notes/redeliver",
        "/cli/workspace/set",
        "/cli/chatter",
        "/cli/chatterlog",
        "/cli/workspace/msg",
        "/cli/terminal/read",
        "/cli/terminal/write",
        "/cli/browser/open-url",
        // A pass on the read-state arm's project route only gets
        // not_a_person (a guest's project read state is App project chat's).
        "/cli/project-group/read",
    ];

    /// Prefix arms (a route family matched with `starts_with`) that touch a
    /// pass inside: the family's exact routes are covered by the gate and
    /// [`every_policy_route_is_closed_to_a_pass_unless_it_is_a_catalog_row`].
    const PREFIX_ARMS: &[&str] = &["/cli/thread/", "/cli/skin", "/cli/db/"];

    fn indent(l: &str) -> usize {
        l.len() - l.trim_start().len()
    }

    /// The match-arm heads enclosing `line`, innermost first: each is the
    /// pattern text from its first line up to `=>`.
    fn arm_heads(lines: &[&str], line: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut limit = indent(lines[line]);
        let mut j = line;
        while j > 0 {
            j -= 1;
            let l = lines[j];
            let t = l.trim();
            if t.is_empty() || t.starts_with("//") || indent(l) >= limit {
                continue;
            }
            limit = indent(l);
            let (base, end) = if t == "{" && j > 0 && lines[j - 1].trim().ends_with("=>") {
                (indent(l), j - 1)
            } else if t.ends_with("=> {") || t.ends_with("=>") {
                (indent(l), j)
            } else {
                continue;
            };
            let mut k = end;
            while k > 0 && !(indent(lines[k]) == base && !lines[k].trim().starts_with('|')) {
                k -= 1;
            }
            out.push(lines[k..=end].iter().map(|s| s.trim()).collect::<Vec<_>>().join(" "));
            j = k;
            limit = base;
        }
        out
    }

    /// The arm head `line` itself sits in (a guard that spans lines, such
    /// as `p if … && is_skin_token(…) =>`), if any.
    fn head_at(lines: &[&str], line: usize) -> Option<String> {
        let mut end = None;
        for (k, l) in lines.iter().enumerate().skip(line).take(6) {
            let t = l.trim();
            if t.ends_with("=>") || t.ends_with("=> {") {
                end = Some(k);
                break;
            }
            if t.ends_with('{') || t.ends_with(';') || t.ends_with('}') {
                return None;
            }
        }
        let end = end?;
        let base = if lines[end].trim().ends_with("=>") && lines.get(end + 1).is_some_and(|l| l.trim() == "{") {
            indent(lines[end + 1])
        } else {
            indent(lines[end])
        };
        let mut s = line;
        while s > 0 && !(indent(lines[s]) == base && !lines[s].trim().starts_with('|')) {
            s -= 1;
        }
        Some(lines[s..=end].iter().map(|x| x.trim()).collect::<Vec<_>>().join(" "))
    }

    /// Blank `#[cfg(test)] mod … { … }` blocks (rustfmt shape), keeping
    /// line numbers.
    fn strip_test_modules(src: &str) -> Vec<String> {
        let lines: Vec<&str> = src.lines().collect();
        let mut out = Vec::with_capacity(lines.len());
        let mut i = 0;
        while i < lines.len() {
            let l = lines[i];
            let next_is_mod = lines.get(i + 1).is_some_and(|n| {
                let t = n.trim_start();
                t.starts_with("mod ") && t.trim_end().ends_with('{')
            });
            if l.trim() == "#[cfg(test)]" && next_is_mod {
                let close = format!("{}}}", &l[..indent(l)]);
                let mut j = i + 2;
                while j < lines.len() && lines[j] != close {
                    j += 1;
                }
                for _ in i..=j.min(lines.len() - 1) {
                    out.push(String::new());
                }
                i = j + 1;
                continue;
            }
            out.push(l.to_string());
            i += 1;
        }
        out
    }

    /// Exact route literals in an arm head (`starts_with("…")` families
    /// are not routes).
    fn literals(head: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = head;
        while let Some(i) = rest.find("\"/") {
            let s = &rest[i + 1..];
            let Some(e) = s.find('"') else { break };
            if !rest[..i].trim_end().ends_with("starts_with(") {
                out.push(s[..e].to_string());
            }
            rest = &s[e + 1..];
        }
        out
    }

    fn prefixes(head: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = head;
        while let Some(i) = rest.find("starts_with(\"") {
            let s = &rest[i + "starts_with(\"".len()..];
            let Some(e) = s.find('"') else { break };
            out.push(s[..e].to_string());
            rest = &s[e..];
        }
        out
    }

    /// Every route arm in the dispatcher that touches a pass names only
    /// routes that are catalog rows, or is on [`REFUSING_ARMS`] (the gate
    /// keeps those closed), or is a [`PREFIX_ARMS`] family. A new arm that
    /// takes `k2skn_` without a row fails here (and the gate refuses it at
    /// runtime anyway).
    #[test]
    fn every_pass_touching_arm_is_a_catalog_row_or_refuses() {
        let src = include_str!("dispatcher.rs");
        let owned = strip_test_modules(src);
        let lines: Vec<&str> = owned.iter().map(String::as_str).collect();
        let markers = ["is_skin_token", "resolve_pass(", "resolve_skin_token(", "with_request_skin("];
        let consts = route_consts();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut families: BTreeSet<String> = BTreeSet::new();
        let mut problems = Vec::new();
        let mut markers_seen = 0;
        for (n, l) in lines.iter().enumerate() {
            if l.trim().starts_with("//") || !markers.iter().any(|m| l.contains(m)) {
                continue;
            }
            markers_seen += 1;
            // The innermost enclosing arm that names a route (or a family).
            let mut routes: Vec<String> = Vec::new();
            let mut family = None;
            let mut heads = arm_heads(&lines, n);
            if let Some(own) = head_at(&lines, n) {
                heads.insert(0, own);
            }
            for head in heads {
                routes = literals(&head);
                for (expr, vals) in &consts {
                    if head.contains(expr) {
                        routes.extend(vals.iter().map(|s| s.to_string()));
                    }
                }
                routes.retain(|r| r.starts_with("/cli/") || r == "/events");
                if !routes.is_empty() {
                    break;
                }
                if let Some(p) = prefixes(&head).into_iter().find(|p| p.starts_with("/cli/")) {
                    family = Some(p);
                    break;
                }
            }
            if routes.is_empty() {
                match family {
                    Some(f) if PREFIX_ARMS.contains(&f.as_str()) => {
                        families.insert(f);
                    }
                    Some(f) => problems.push(format!(
                        "dispatcher.rs:{}: a pass is handled in the {f}* family arm: list it in PREFIX_ARMS once its routes have rows",
                        n + 1
                    )),
                    None => problems.push(format!("dispatcher.rs:{}: a pass is handled in an arm the walk can't name", n + 1)),
                }
                continue;
            }
            for r in routes {
                seen.insert(r.clone());
                let open = admits("GET", &r) || admits("POST", &r);
                let refusing = REFUSING_ARMS.contains(&r.as_str());
                if refusing && open {
                    problems.push(format!("{r} (dispatcher.rs:{}) is on REFUSING_ARMS but the catalog opens it", n + 1));
                }
                if !open && !refusing {
                    problems.push(format!(
                        "{r} (dispatcher.rs:{}) touches an App pass but has no catalog app row: add the row \
                         (crates/k2-core/src/contract/catalog.json) or, when the arm only refuses passes, list it in REFUSING_ARMS",
                        n + 1
                    ));
                }
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
        assert!(markers_seen >= 30, "only {markers_seen} pass markers: extractor broken?");
        for must in ["/cli/thread/attachment", "/cli/skin/me", "/cli/fs/upload-chunk", "/cli/sessions/grid", "/cli/skin/agents"] {
            assert!(seen.contains(must), "the walk lost {must}: extractor broken? saw {seen:?}");
        }
        assert!(families.contains("/cli/thread/"), "the Thread POST family (catch-up, post) is a pass arm: {families:?}");
        assert!(seen.len() >= 30, "the walk saw only {} routes: {seen:?}", seen.len());
        for r in REFUSING_ARMS {
            assert!(!admits("GET", r) && !admits("POST", r), "{r} must stay closed");
        }
    }

    #[test]
    fn the_walk_names_single_multi_line_and_nested_arms() {
        let src = r#"
    match p {
        "/cli/a" | "/cli/b" => {
            if x.is_skin_token() {}
        }
        p if is_post
            && (p == "/cli/c" || p == "/cli/d") =>
        {
            match pass {
                Some(pass) if ok => {
                    is_skin_token(t);
                }
            }
        }
        p if p.starts_with("/cli/fam/") => {
            is_skin_token(t);
        }
    }
"#;
        let lines: Vec<&str> = src.lines().collect();
        let at = |needle: &str, nth: usize| lines.iter().enumerate().filter(|(_, l)| l.contains(needle)).nth(nth).unwrap().0;
        let h = arm_heads(&lines, at("is_skin_token", 0));
        assert_eq!(literals(&h[0]), ["/cli/a", "/cli/b"]);
        let h = arm_heads(&lines, at("is_skin_token", 1));
        assert!(literals(&h[0]).is_empty(), "{h:?}");
        assert_eq!(literals(&h[1]), ["/cli/c", "/cli/d"], "{h:?}");
        let h = arm_heads(&lines, at("is_skin_token", 2));
        assert!(literals(&h[0]).is_empty());
        assert_eq!(prefixes(&h[0]), ["/cli/fam/"]);
    }
}
