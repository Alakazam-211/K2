//! The App-facing route table (`prd-k2-app-runtime-v1` §5.1, AR8): the one
//! list of `/cli/*` routes and sockets an App may reach.
//!
//! Two doors read it:
//! - the browser helper (`k2-daemon --skin-gateway`), forwarding a guest's
//!   session pass from the App's origin;
//! - the App's **gateway link** (runtime S14, `apps::gateway` in
//!   k2-daemon), the per-App listener a walled App calls with its App pass.
//!
//! Default deny: a route not listed here is refused at both doors before
//! any handler runs. **S23 (0.46.1) generates this table from
//! `contract/catalog.json`** (rows with exposure `app`) and a CI test fails
//! an App-facing route that isn't catalogued; until then it is written by
//! hand and the helper's tests walk it.

/// Every App-facing `(method, path)`: what the browser helper forwards with
/// a guest's pass, and (minus [`HELPER_ONLY_HTTP`]) what an App's gateway
/// link serves to its App pass. The helper's cross-origin tests walk it,
/// so a new mutating route is covered the moment it is added here.
pub const HTTP_ALLOWLIST: &[(&str, &str)] = &[
    ("GET", "/cli/skin/agents"),
    ("HEAD", "/cli/skin/agents"),
    ("GET", "/cli/thread"),
    ("HEAD", "/cli/thread"),
    ("POST", "/cli/thread/post"),
    ("POST", "/cli/thread/answer"),
    ("POST", "/cli/thread/void"),
    ("GET", "/cli/fs/read-dir"),
    ("HEAD", "/cli/fs/read-dir"),
    ("GET", "/cli/fs/read-file"),
    ("HEAD", "/cli/fs/read-file"),
    ("GET", "/cli/fs/read-binary"),
    ("HEAD", "/cli/fs/read-binary"),
    ("GET", "/cli/fs/read-range"),
    ("HEAD", "/cli/fs/read-range"),
    ("POST", "/cli/fs/write-file"),
    ("POST", "/cli/fs/upload-binary"),
    // Resumable large upload: one chunk per request, body capped by
    // `request_body_cap`; the guest form is jailed and bound to the guest.
    ("POST", "/cli/fs/upload-chunk"),
    ("POST", "/cli/fs/create"),
    ("POST", "/cli/fs/copy"),
    ("POST", "/cli/fs/move"),
    ("POST", "/cli/fs/rename"),
    ("GET", "/cli/workspace/resources"),
    ("POST", "/cli/workspace/resources/add"),
    ("POST", "/cli/workspace/resources/remove"),
    ("POST", "/cli/workspace/ensure-pinned-chat"),
    ("GET", "/cli/feedback/list"),
    ("HEAD", "/cli/feedback/list"),
    ("GET", "/cli/feedback/show"),
    ("HEAD", "/cli/feedback/show"),
    ("POST", "/cli/feedback/create"),
    ("POST", "/cli/feedback/comment"),
    ("POST", "/cli/feedback/answer"),
    ("POST", "/cli/feedback/resolve"),
    ("POST", "/cli/feedback/assign"),
    ("GET", "/cli/wiki/index"),
    ("HEAD", "/cli/wiki/index"),
    ("GET", "/cli/wiki/note"),
    ("HEAD", "/cli/wiki/note"),
    ("GET", "/cli/store/list"),
    ("HEAD", "/cli/store/list"),
    ("GET", "/cli/store/get"),
    ("HEAD", "/cli/store/get"),
    ("GET", "/cli/store/query"),
    ("HEAD", "/cli/store/query"),
    ("GET", "/cli/db/tables"),
    ("HEAD", "/cli/db/tables"),
    ("GET", "/cli/db/rows"),
    ("HEAD", "/cli/db/rows"),
    ("POST", "/cli/db/rows"),
    ("POST", "/cli/db/rows/update"),
    ("POST", "/cli/db/rows/delete"),
    ("POST", "/cli/db/query"),
    ("POST", "/cli/skin/password/change"),
    // App heartbeats (prd-app-heartbeats-surface-v1 AH5/AH6).
    // Reads: heartbeats:read. Writes: POST only, heartbeats:write.
    ("GET", "/cli/heartbeat/list"),
    ("HEAD", "/cli/heartbeat/list"),
    ("GET", "/cli/heartbeat/show"),
    ("HEAD", "/cli/heartbeat/show"),
    ("GET", "/cli/heartbeat/status"),
    ("HEAD", "/cli/heartbeat/status"),
    ("GET", "/cli/heartbeat/fires-list"),
    ("HEAD", "/cli/heartbeat/fires-list"),
    ("POST", "/cli/heartbeat/add"),
    ("POST", "/cli/heartbeat/edit"),
    ("POST", "/cli/heartbeat/enable"),
    ("POST", "/cli/heartbeat/rename"),
    ("POST", "/cli/heartbeat/archive"),
    ("POST", "/cli/heartbeat/fire"),
    // Agent working (prd-daemon-activity-and-thread-working-v1 AP3): the
    // room's guest snapshot, activity:read. GET only.
    ("GET", "/cli/activity/snapshot"),
    // App identity surface (0.45.5): read needs only the room; writes are
    // POST only, agent:identity (name, color, picture) and agent:handle.
    ("GET", "/cli/agent/identity"),
    ("HEAD", "/cli/agent/identity"),
    ("POST", "/cli/agent/identity"),
    ("POST", "/cli/agent/handle"),
];

/// The one table of App-facing sockets (helper and link), and the query
/// parameter each one must carry. A socket cannot be allowlisted without naming its
/// parameter (GA1). `/cli/activity/events` was once allowlisted without it,
/// so the helper asked it for `conversation=` and refused every app.
pub const WS_ALLOWLIST: &[(&str, &str)] = &[
    ("/cli/overlay/events", "conversation"),
    ("/cli/fs/events", "workspace"),
    ("/cli/activity/events", "workspace"),
];

/// Routes only the browser helper forwards: they act on a guest's own
/// login session (`session=1`), which an App pass never is.
pub const HELPER_ONLY_HTTP: &[&str] = &["/cli/skin/password/change"];

fn path_only(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

/// May an App's gateway link serve `method path` (exact match)?
pub fn link_allows_http(method: &str, path: &str) -> bool {
    let p = path_only(path);
    let m = method.to_ascii_uppercase();
    !HELPER_ONLY_HTTP.contains(&p) && HTTP_ALLOWLIST.iter().any(|(am, ap)| *am == m.as_str() && *ap == p)
}

/// The query parameter an App-facing socket must carry, if `path` is one.
pub fn ws_param(path: &str) -> Option<&'static str> {
    let p = path_only(path);
    WS_ALLOWLIST.iter().find(|(wp, _)| *wp == p).map(|(_, param)| *param)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_serves_the_shared_list_minus_helper_only_rows() {
        assert!(link_allows_http("GET", "/cli/thread"));
        assert!(link_allows_http("get", "/cli/thread?conversation=x"));
        assert!(link_allows_http("POST", "/cli/thread/post"));
        assert!(!link_allows_http("GET", "/cli/thread/post"), "exact method");
        assert!(!link_allows_http("POST", "/cli/skin/password/change"), "a guest's own session only");
        assert!(HTTP_ALLOWLIST.iter().any(|(_, p)| *p == "/cli/skin/password/change"), "still the helper's");
        for never in ["/cli/auth/login", "/cli/sessions/events", "/cli/app/deploy", "/cli/skin/users/rooms", "/v1/sandboxes", "/cli/vault/get"] {
            assert!(!link_allows_http("GET", never) && !link_allows_http("POST", never), "{never}");
        }
        assert_eq!(ws_param("/cli/overlay/events?conversation=a"), Some("conversation"));
        assert_eq!(ws_param("/cli/sessions/events"), None);
        for (m, p) in HTTP_ALLOWLIST {
            assert!(p.starts_with("/cli/"), "{m} {p}");
            assert!(matches!(*m, "GET" | "HEAD" | "POST"), "{m} {p}");
        }
    }
}
