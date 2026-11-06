//! New "Untitled" files over the real dispatcher (Cmd+N, the tab + menu, the
//! Board tab): the name checks the disk, a create never overwrites, racing
//! creates lose nothing, and both routes are POST-only and closed to App passes.

use crate::support::app_fixture;

use app_fixture::*;
use serde_json::json;

const ROUTES: [&str; 2] = ["/cli/fs/untitled-name", "/cli/fs/create-untitled"];

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn untitled_routes_check_the_disk_and_never_overwrite() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let dir = env.path().join("room");
    std::fs::create_dir_all(&dir).unwrap();
    let dir_s = dir.to_string_lossy().to_string();

    // A GET is a 405, whoever asks, and writes nothing.
    for path in ROUTES {
        let r = get(port, path, OWNER, &format!("dir={}", enc(&dir_s))).await;
        assert_eq!(r.status, 405, "GET {path}: {}", r.body);
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);

    // Only the owner's door: no token, a bad one, or an App pass gets nothing.
    for token in ["wrong-token", "k2skn_not_a_real_pass"] {
        for path in ROUTES {
            let r = post(port, path, token, json!({ "dir": dir_s, "content": "x" })).await;
            assert!(r.status == 401 || r.status == 403, "{path} with {token}: {} {}", r.status, r.body);
        }
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);

    // The name skips what's on disk and the tabs named in `taken`, and creates nothing.
    std::fs::write(dir.join("Untitled"), "the real file").unwrap();
    let r = post(port, ROUTES[0], OWNER, json!({ "dir": dir_s })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["name"], "Untitled-2");
    let r = post(port, ROUTES[0], OWNER, json!({ "dir": dir_s, "taken": ["Untitled-2"] })).await;
    assert_eq!(r.json()["name"], "Untitled-3");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

    // A first save under a name someone took moves on and overwrites nothing.
    let r = post(port, ROUTES[1], OWNER, json!({ "dir": dir_s, "prefer": "Untitled", "content": "typed" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["name"], "Untitled-2");
    assert_eq!(std::fs::read_to_string(dir.join("Untitled")).unwrap(), "the real file");
    assert_eq!(std::fs::read_to_string(dir.join("Untitled-2")).unwrap(), "typed");

    // The Board tab's call: any extension.
    let r = post(port, ROUTES[1], OWNER, json!({ "dir": dir_s, "ext": "k5board", "content": "{}" })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["name"], "Untitled.k5board");
    assert_eq!(r.json()["path"], dir.join("Untitled.k5board").to_string_lossy().as_ref());

    // Racing creates: every writer gets its own file, with its own text.
    let mut racers = Vec::new();
    for i in 0..16 {
        let dir_s = dir_s.clone();
        racers.push(tokio::spawn(async move {
            let body = format!("writer {i}");
            let r = post(port, ROUTES[1], OWNER, json!({ "dir": dir_s, "ext": "md", "content": body })).await;
            assert_eq!(r.status, 200, "{}", r.body);
            (r.json()["name"].as_str().unwrap().to_string(), body)
        }));
    }
    let mut names = std::collections::HashSet::new();
    for racer in racers {
        let (name, body) = racer.await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(&name)).unwrap(), body, "{name}");
        assert!(names.insert(name.clone()), "{name} handed out twice");
    }
    assert_eq!(names.len(), 16);
}
