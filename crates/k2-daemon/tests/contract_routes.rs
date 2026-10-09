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

#![cfg(unix)]

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
    assert_eq!((widget, app), (18, 0), "per-exposure verb counts (widget, app)");
    assert_eq!(c.verbs.len(), 40, "today's 37 bridge verbs plus the frame rows theme.changed, canvas.setHitRegions, window.startDrag");
}

#[test]
fn every_feature_is_a_boot_status_key() {
    let mut known: BTreeSet<&str> = k2_daemon::boot_status::features().into_iter().collect();
    // Added by this build (UW36); B2 puts it in /boot-status.
    known.insert("zen-widgets-v1");
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
    assert_eq!(generated.len(), 37);
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
    let writes: Vec<(String, String)> = catalog()
        .verbs
        .iter()
        .filter(|v| v.effect == Effect::Write)
        .filter_map(|v| v.bindings.http.as_ref().map(|h| (v.verb.clone(), h.route.clone())))
        .collect();
    assert!(writes.len() >= 3, "thread.post, thread.answer and thread.void bind routes: {writes:?}");
    for (verb, route) in &writes {
        let path = format!("{route}?token={OWNER_TOKEN}&addr=x&text=x&id=x&answer=x");
        let port = d.port;
        let (status, body) = tokio::task::spawn_blocking(move || get_status(port, &path)).await.expect("join");
        assert_eq!(status, 405, "GET {route} ({verb}) must be 405; body={body}");
    }
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    std::fs::remove_dir_all(&home).expect("remove temp HOME");
}

// ── TUWA11: k2 zen guide api ────────────────────────────────────────────

/// Run `cli/k2 zen guide api <args>` the way an agent with no daemon would.
fn guide_api(args: &[&str]) -> String {
    let home = std::env::temp_dir().join(format!("k2-guide-api-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).expect("empty HOME");
    let out = std::process::Command::new("bash")
        .arg(root().join("cli/k2"))
        .args(["zen", "guide", "api"])
        .args(args)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
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
    assert!(page.lines().count() <= 80, "guide api is {} lines", page.lines().count());
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
    let index = std::process::Command::new("bash")
        .arg(root().join("cli/k2"))
        .args(["zen", "guide"])
        .env_clear()
        .env("HOME", std::env::temp_dir())
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("index");
    assert!(String::from_utf8_lossy(&index.stdout).contains("  api "), "the index lists api");
}
