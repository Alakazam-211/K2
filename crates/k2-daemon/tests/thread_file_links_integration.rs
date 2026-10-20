//! Thread file links (Rosson 2026-10-18), headless: `POST
//! /cli/fs/resolve-links` on the real dispatcher in-process (temp `$HOME`).
//! The daemon decides which printed paths are files or folders inside the
//! Thread's workspace; inside answers kind + workspace-relative path + the
//! path to open, everything else (outside, missing, a `..` walk out, a
//! symlink that escapes) answers the same bare `linkable: false`.

#![cfg(unix)]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::path::Path;

use app_fixture::*;
use serde_json::{json, Value};

const PW: &str = "s3cret-horse-staple";

fn write(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn refused(input: &str) -> Value {
    json!({ "input": input, "linkable": false })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resolve_links_answers_only_paths_inside_the_threads_workspace() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let tag = &uuid::Uuid::new_v4().to_string()[..6];
    let ws = seed_ws(&env, &format!("links{tag}"));
    write(&ws.path, "src/a.ts", b"export {}\n");
    std::fs::create_dir_all(ws.path.join("docs")).unwrap();
    let outside = env.path().join(format!("outside-{tag}"));
    write(&outside, "secret.txt", b"not yours");
    std::os::unix::fs::symlink(outside.join("secret.txt"), ws.path.join("leak.txt")).unwrap();
    std::os::unix::fs::symlink(&outside, ws.path.join("leakdir")).unwrap();
    std::os::unix::fs::symlink(ws.path.join("src/a.ts"), ws.path.join("alias.ts")).unwrap();

    let abs_inside = ws.path.join("src/a.ts").to_string_lossy().into_owned();
    let abs_outside = outside.join("secret.txt").to_string_lossy().into_owned();
    let ws_dir = ws.path.file_name().unwrap().to_string_lossy().into_owned();
    let tilde_inside = format!("~/{ws_dir}/src/a.ts");
    let outside_rel = format!("../outside-{tag}/secret.txt");
    let paths = vec![
        "src/a.ts".to_string(),
        abs_inside.clone(),
        "docs/".to_string(),
        "docs/../src/a.ts".to_string(),
        "alias.ts".to_string(),
        tilde_inside.clone(),
        // Refused, all alike:
        "src/missing.ts".to_string(),
        abs_outside.clone(),
        outside_rel.clone(),
        "leak.txt".to_string(),
        "leakdir/secret.txt".to_string(),
        "leakdir".to_string(),
        "/etc/hosts".to_string(),
    ];

    let r = post(port, "/cli/fs/resolve-links", OWNER, json!({ "workspace": ws.handle, "paths": paths })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    let root = ws.path.to_string_lossy().into_owned();
    assert_eq!(v["root"], json!(root));
    assert!(v["prefixes"].as_array().unwrap().contains(&json!(root)), "{v}");
    let links = v["links"].as_array().expect("links");
    assert_eq!(links.len(), paths.len(), "one answer per candidate, in order");

    let file = |input: &str, rel: &str| {
        json!({ "input": input, "linkable": true, "kind": "file", "rel": rel, "path": format!("{root}/{rel}") })
    };
    assert_eq!(links[0], file("src/a.ts", "src/a.ts"));
    assert_eq!(links[1], file(&abs_inside, "src/a.ts"));
    assert_eq!(
        links[2],
        json!({ "input": "docs/", "linkable": true, "kind": "directory", "rel": "docs", "path": format!("{root}/docs") })
    );
    assert_eq!(links[3], file("docs/../src/a.ts", "src/a.ts"));
    // A symlink inside the workspace links to its target.
    assert_eq!(links[4], file("alias.ts", "src/a.ts"));
    // `~` is the daemon's home ($HOME is the test's temp home).
    assert_eq!(links[5], file(&tilde_inside, "src/a.ts"));
    // Missing, outside, `..` out, symlink escapes: the same bare answer.
    for (i, input) in paths.iter().enumerate().skip(6) {
        assert_eq!(links[i], refused(input), "candidate {i} ({input})");
    }
    assert!(!r.body.contains("not yours"));

    // A sidecar address resolves to its workspace too.
    let r = post(
        port,
        "/cli/fs/resolve-links",
        OWNER,
        json!({ "workspace": format!("{}/reviewer", ws.handle), "paths": ["src/a.ts"] }),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["links"][0], file("src/a.ts", "src/a.ts"));

    // The workspace path itself works as the token; no paths = root only.
    let r = post(port, "/cli/fs/resolve-links", OWNER, json!({ "workspace": root })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["links"], json!([]));

    // An unknown workspace (an imported Thread, a typo) is a 404, and a
    // root the caller names that is not a registered workspace is too.
    for token in ["nope-not-a-workspace", "import:abc", "/", outside.to_str().unwrap()] {
        let r = post(port, "/cli/fs/resolve-links", OWNER, json!({ "workspace": token, "paths": ["secret.txt"] })).await;
        assert_eq!(r.status, 404, "{token}: {}", r.body);
        assert!(!r.body.contains("secret"), "{}", r.body);
    }
    // Bad bodies are 400.
    let r = post(port, "/cli/fs/resolve-links", OWNER, json!({ "paths": [] })).await;
    assert_eq!(r.status, 400, "{}", r.body);
    let r = post(port, "/cli/fs/resolve-links", OWNER, json!({ "workspace": "  ", "paths": [] })).await;
    assert_eq!(r.status, 400, "{}", r.body);

    // POST only.
    let r = get(port, "/cli/fs/resolve-links", OWNER, "workspace=x").await;
    assert_eq!(r.status, 405, "{}", r.body);

    // No token, no answer.
    let r = post(port, "/cli/fs/resolve-links", "wrong-token", json!({ "workspace": ws.handle, "paths": ["src/a.ts"] })).await;
    assert_ne!(r.status, 200, "{}", r.body);

    // 0.46.3 (Thread parity, Rosson 2026-10-20): an App verb now
    // (`files.resolveLinks`), on both doors, but a guest's answer is cut to
    // its OWN file rights and never carries a host path (root, prefixes,
    // path). Nothing outside the workspace ever resolves, for anyone.
    assert!(k2_daemon::skin_gateway::allowlisted_http("POST", "/cli/fs/resolve-links"));
    let post_ok = |path: &'static str, body: Value| async move {
        let r = post(port, path, OWNER, body).await;
        assert_eq!(r.status, 200, "{path}: {}", r.body);
    };
    let role = format!("links{tag}");
    post_ok("/cli/skin/roles", json!({ "name": role })).await;
    post_ok("/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["files:read"] })).await;
    let user = format!("guest{tag}");
    post_ok("/cli/skin/users", json!({ "username": user })).await;
    post_ok("/cli/skin/users/password", json!({ "username": user, "password": PW })).await;
    post_ok("/cli/skin/roles/assign", json!({ "username": user, "role": role })).await;
    let body = json!({ "username": user, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body)))
        .await
        .unwrap();
    assert_eq!(r.status, 200, "login: {}", r.body);
    let pass = r.json()["token"].as_str().expect("token").to_string();
    let r = post(port, "/cli/fs/resolve-links", &pass, json!({ "workspace": ws.handle, "paths": paths })).await;
    assert_eq!(r.status, 200, "a guest with files:read on the whole room: {}", r.body);
    let g = r.json();
    assert!(g.get("root").is_none() && g.get("prefixes").is_none(), "no host path for a guest: {g}");
    let gl = g["links"].as_array().expect("links");
    assert!(gl.iter().all(|l| l.get("path").is_none()), "no absolute path field: {}", r.body);
    assert_eq!(gl.len(), paths.len());
    for (i, l) in gl.iter().enumerate() {
        // The guest never expands `~` (no home); the rest match the owner's
        // answer, as rel only.
        let want = links[i]["linkable"].as_bool().unwrap() && i != 5;
        assert_eq!(l["linkable"].as_bool(), Some(want), "{}: {l}", paths[i]);
        if want {
            assert_eq!(l["rel"], links[i]["rel"], "{}", paths[i]);
        } else {
            assert_eq!(l.as_object().unwrap().len(), 2, "a refusal is bare: {l}");
        }
    }
    for i in 6..paths.len() {
        assert_eq!(gl[i]["linkable"], false, "outside the workspace never resolves: {}", paths[i]);
    }
    // A room it doesn't hold: the uniform skin_room, nothing resolved.
    let other = seed_ws(&env, &format!("lo{tag}"));
    let r = post(port, "/cli/fs/resolve-links", &pass, json!({ "workspace": other.handle, "paths": ["src/a.ts"] })).await;
    assert_ne!(r.status, 200, "another room is refused: {}", r.body);
    assert!(!r.body.contains("src/a.ts"), "{}", r.body);
}
