use super::*;

fn untitled_tmp(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "k2-fs-routes-untitled-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn untitled_name_answers_from_the_disk_and_writes_nothing() {
    let dir = untitled_tmp("name");
    std::fs::write(dir.join("Untitled.k5board"), "x").unwrap();
    let body = serde_json::json!({
        "dir": dir.to_string_lossy(), "ext": "k5board", "taken": ["Untitled-2.k5board"],
    })
    .to_string();
    let r = handle_untitled_name(body.as_bytes());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["name"], "Untitled-3.k5board");
    assert_eq!(v["path"], dir.join("Untitled-3.k5board").to_string_lossy().as_ref());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn create_untitled_never_overwrites_and_says_what_it_made() {
    let dir = untitled_tmp("create");
    std::fs::write(dir.join("Untitled"), "the real file").unwrap();
    let body = serde_json::json!({
        "dir": dir.to_string_lossy(), "prefer": "Untitled", "content": "typed",
    })
    .to_string();
    let r = handle_create_untitled(body.as_bytes());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["name"], "Untitled-2");
    assert_eq!(std::fs::read_to_string(dir.join("Untitled-2")).unwrap(), "typed");
    assert_eq!(std::fs::read_to_string(dir.join("Untitled")).unwrap(), "the real file");
}

#[test]
fn untitled_routes_refuse_a_bad_request() {
    let dir = untitled_tmp("refuse");
    let d = dir.to_string_lossy().to_string();
    for body in [
        "not json".to_string(),
        serde_json::json!({ "ext": "md" }).to_string(),
        serde_json::json!({ "dir": d, "ext": "../x" }).to_string(),
        serde_json::json!({ "dir": dir.join("missing/deeper").to_string_lossy() }).to_string(),
    ] {
        assert!(handle_untitled_name(body.as_bytes()).status.starts_with("400"), "{body}");
        assert!(handle_create_untitled(body.as_bytes()).status.starts_with("400"), "{body}");
    }
    let body = serde_json::json!({ "dir": d, "prefer": "notes.md" }).to_string();
    assert!(handle_create_untitled(body.as_bytes()).status.starts_with("400"));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
}

/// `fs/read-file`'s refusal carries the old `error` message AND a
/// `code` the file viewer branches on: `not_found` opens a new,
/// editable file; `binary` / `not_utf8` / `too_large` lock the viewer
/// so an edit can't overwrite the real file.
#[test]
fn read_file_refusal_body_carries_the_code() {
    let dir = std::env::temp_dir().join(format!(
        "k2-fs-routes-read-code-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let bin = dir.join("deck.pptx");
    std::fs::write(&bin, [b'P', b'K', 0x03, 0x04, 0x00]).expect("write binary");
    let latin = dir.join("latin1.txt");
    std::fs::write(&latin, [b'c', b'a', b'f', 0xE9]).expect("write latin-1");
    let text = dir.join("ok.txt");
    std::fs::write(&text, "hello").expect("write text");

    let read = |p: &Path| {
        let mut params = HashMap::new();
        params.insert("path".to_string(), p.to_string_lossy().to_string());
        handle_read_file(&params)
    };
    let body = |r: &CliResponse| -> serde_json::Value {
        serde_json::from_str(&r.body).expect("JSON body")
    };

    let cases: [(&Path, &str, &str); 3] = [
        (&dir.join("new.md"), "not_found", "File not found"),
        (&bin, "binary", "Cannot read binary file"),
        (&latin, "not_utf8", "Cannot read binary file"),
    ];
    for (p, code, msg) in cases {
        let r = read(p);
        assert_eq!(r.status, "400 Bad Request", "{}", p.display());
        let v = body(&r);
        assert_eq!(v["code"], code, "{}", p.display());
        assert_eq!(v["error"], msg, "{}", p.display());
    }

    let ok = read(&text);
    assert_eq!(ok.status, "200 OK");
    let v = body(&ok);
    assert_eq!(v["content"], "hello");
    assert!(v.get("code").is_none(), "a good read has no code: {v}");

    std::fs::remove_dir_all(&dir).expect("clean temp dir");
}

/// 0.46.6 (App guest login lockout report, fix 3a): `fs/read-dir` on a
/// folder that isn't there is a 404 `not_found` (it was a 400 with the
/// OS text), its parent missing too; a file where a folder was asked
/// for stays a 400 (`read_failed`). Both keep `error` as text.
#[test]
fn read_dir_on_a_missing_folder_is_a_404_not_found() {
    let dir = std::env::temp_dir().join(format!("k2-fs-routes-read-dir-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("here")).expect("temp dir");
    std::fs::write(dir.join("file.txt"), "x").expect("write file");
    let read = |p: &Path| {
        let mut params = HashMap::new();
        params.insert("path".to_string(), p.to_string_lossy().to_string());
        handle_read_dir(&params)
    };
    let body = |r: &CliResponse| -> serde_json::Value { serde_json::from_str(&r.body).expect("JSON body") };
    for missing in [dir.join("gone"), dir.join("gone/deeper")] {
        let r = read(&missing);
        assert_eq!(r.status, "404 Not Found", "{}: {}", missing.display(), r.body);
        assert_eq!(body(&r), serde_json::json!({ "error": "No such folder", "code": "not_found" }), "{}", missing.display());
    }
    let r = read(&dir.join("file.txt"));
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    let v = body(&r);
    assert_eq!(v["code"], "read_failed", "{v}");
    assert!(v["error"].as_str().is_some_and(|e| !e.is_empty()), "{v}");
    let ok = read(&dir);
    assert_eq!(ok.status, "200 OK", "{}", ok.body);
    let names: Vec<String> = body(&ok).as_array().expect("entries").iter().map(|e| e["name"].as_str().expect("name").to_string()).collect();
    assert!(names.contains(&"here".to_string()) && names.contains(&"file.txt".to_string()), "{names:?}");
    std::fs::remove_dir_all(&dir).expect("clean temp dir");
}

/// 3a on an App's (guest's) path: a missing folder in the room is a
/// 404 that names no host path; a missing name behind a link that
/// leads out of the room stays the jail's 400.
#[test]
fn skin_read_dir_on_a_missing_folder_is_a_404_not_found() {
    let dir = std::env::temp_dir().join(format!("k2-skin-read-dir-{}", uuid::Uuid::new_v4()));
    let outside = std::env::temp_dir().join(format!("k2-skin-read-dir-out-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("docs")).expect("mkdir");
    std::fs::create_dir_all(&outside).expect("mkdir outside");
    k2_core::test_env::symlink(&outside, dir.join("docs/out")).expect("symlink out");
    let canon = dunce::canonicalize(&dir).expect("canon");
    let handle = format!("rd{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let id = insert_handled_project(&canon.to_string_lossy(), &handle);
    let pass = session_pass(&id, &[k2_core::skin::CAP_FILES_READ]);
    let read = |rel: &str| {
        let mut params = HashMap::new();
        params.insert("workspace".to_string(), handle.clone());
        params.insert("path".to_string(), rel.to_string());
        handle_skin_read_dir(&params, &pass)
    };
    for rel in ["docs/gone", "gone/deeper"] {
        let r = read(rel);
        assert_eq!(r.status, "404 Not Found", "{rel}: {}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("JSON body");
        assert_eq!(v, serde_json::json!({ "error": "No such folder", "code": "not_found" }), "{rel}");
    }
    assert_eq!(read("docs").status, "200 OK");
    let r = read("docs/out/gone");
    assert_eq!(r.status, "400 Bad Request", "out of the room stays the jail's 400: {}", r.body);
    let r = read("docs/../gone");
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    std::fs::remove_dir_all(&dir).expect("clean");
    std::fs::remove_dir_all(&outside).expect("clean outside");
}

/// The route refuses every non-URL target for every caller (the
/// handler opens nothing but allow-listed URLs, so an agent's token
/// can't make this machine open a path, an app or a UNC share), and
/// no `open_url` event goes out for one either.
#[test]
fn open_external_refuses_paths_and_other_schemes_for_any_caller() {
    for target in [
        "/Applications/Calculator.app",
        "/bin/sh",
        r"C:\Windows\System32\calc.exe",
        r"\\host\share\x.exe",
        "file:///etc/passwd",
        "ssh://-oProxyCommand=calc/x",
        "search-ms:query=x",
        "ms-msdt:/id x",
        "shell:startup",
        "javascript:alert(1)",
    ] {
        let body = serde_json::json!({ "target": target }).to_string();
        let resp = handle_open_external(body.as_bytes());
        assert_eq!(resp.status, "400 Bad Request", "{target}: {}", resp.body);
        assert!(resp.body.contains("only http, https and mailto"), "{target}: {}", resp.body);
    }
}

/// 0.40.34 viewer-aware open-external: with ≥1 live events-bus
/// subscriber, an http(s) target is BROADCAST (`open_url`, source
/// "terminal-link") and the daemon-local open/xdg-open shell-out is
/// SKIPPED — proven by the early-return "Routed to …" message (the
/// local path returns "Opened: …" / an error instead).
#[test]
fn open_external_routes_http_to_connected_subscriber_instead_of_local_open() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let probe = format!(
            "https://probe.example/open-external-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        // A live subscriber = "an app is connected".
        let mut rx = crate::session_events::subscribe();

        let body = serde_json::json!({ "target": probe }).to_string();
        let resp = handle_open_external(body.as_bytes());
        assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
        assert!(
            resp.body.contains("Routed to"),
            "must skip the local opener when a subscriber is connected: {}",
            resp.body
        );

        // The subscriber received the event (drain past contamination
        // from other tests on the global bus).
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                panic!("did not receive probe open_url in time");
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(crate::session_events::SessionEvent::OpenUrl { url, source }))
                    if url == probe =>
                {
                    assert_eq!(source, "terminal-link");
                    break;
                }
                Ok(Ok(_)) => continue,
                Ok(Err(_)) => panic!("receiver closed"),
                Err(_) => panic!("timed out waiting for probe"),
            }
        }
    });
}

#[test]
fn compress_rejects_missing_path_before_creating_a_job() {
    let resp = handle_compress(br#"{"path":"/nonexistent/definitely-not-here-k2"}"#);
    assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
}

#[test]
fn compress_status_rejects_unknown_job_id() {
    let resp = handle_compress_status(&HashMap::from([(
        "job_id".to_string(),
        "no-such-job".to_string(),
    )]));
    assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
}

#[test]
fn compress_cancel_rejects_unknown_job_id() {
    let resp = handle_compress_cancel(br#"{"job_id":"no-such-job"}"#);
    assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
}

#[test]
fn compress_job_runs_to_done_and_status_reports_zip_path() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let src = std::env::temp_dir().join(format!("k2-fs-routes-compress-{nanos}"));
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("f.txt"), b"data").unwrap();

    let body = serde_json::json!({ "path": src.to_string_lossy() }).to_string();
    let resp = handle_compress(body.as_bytes());
    assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    let job_id = v["job_id"].as_str().expect("job_id").to_string();

    // Poll to a terminal phase (tiny tree → fast; 5s is generous).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let params = HashMap::from([("job_id".to_string(), job_id)]);
    let final_status = loop {
        let s = handle_compress_status(&params);
        assert_eq!(s.status, "200 OK", "body: {}", s.body);
        let j: serde_json::Value = serde_json::from_str(&s.body).unwrap();
        match j["phase"].as_str() {
            Some("running") => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "compress job never finished"
                );
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => break j,
        }
    };
    assert_eq!(
        final_status["phase"].as_str(),
        Some("done"),
        "{final_status}"
    );
    let zip_path = final_status["zip_path"].as_str().expect("zip_path");
    assert!(
        std::path::Path::new(zip_path).exists(),
        "zip missing: {zip_path}"
    );
    assert!(zip_path.ends_with(".zip"), "got: {zip_path}");

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_file(zip_path);
}

#[test]
fn extract_rejects_missing_path_before_creating_a_job() {
    let resp = handle_extract(br#"{"path":"/nonexistent/definitely-not-here-k2"}"#);
    assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
}

#[test]
fn extract_rejects_non_zip_before_creating_a_job() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("k2-fs-routes-extract-notzip-{nanos}.txt"));
    std::fs::write(&path, b"not a zip").unwrap();
    let body = serde_json::json!({ "path": path.to_string_lossy() }).to_string();
    let resp = handle_extract(body.as_bytes());
    assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
    assert!(
        resp.body.contains(".zip") || resp.body.contains("zip"),
        "body: {}",
        resp.body
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn extract_status_rejects_unknown_job_id() {
    let resp = handle_extract_status(&HashMap::from([(
        "job_id".to_string(),
        "no-such-job".to_string(),
    )]));
    assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
}

#[test]
fn extract_cancel_rejects_unknown_job_id() {
    let resp = handle_extract_cancel(br#"{"job_id":"no-such-job"}"#);
    assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
}

#[test]
fn extract_job_runs_to_done_and_status_reports_dest_path() {
    use std::sync::atomic::AtomicBool;

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("k2-fs-routes-extract-{nanos}"));
    let src = dir.join("bundle");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("hello.txt"), b"world").unwrap();
    // Build the fixture via compress (daemon has no direct zip dep).
    let zip_path = k2_core::fs_compress::compress_to_zip(
        src.to_str().unwrap(),
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .expect("compress fixture");

    let body = serde_json::json!({ "path": zip_path.to_string_lossy() }).to_string();
    let resp = handle_extract(body.as_bytes());
    assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    let job_id = v["job_id"].as_str().expect("job_id").to_string();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let params = HashMap::from([("job_id".to_string(), job_id)]);
    let final_status = loop {
        let s = handle_extract_status(&params);
        assert_eq!(s.status, "200 OK", "body: {}", s.body);
        let j: serde_json::Value = serde_json::from_str(&s.body).unwrap();
        match j["phase"].as_str() {
            Some("running") => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "extract job never finished"
                );
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => break j,
        }
    };
    assert_eq!(
        final_status["phase"].as_str(),
        Some("done"),
        "{final_status}"
    );
    let dest_path = final_status["dest_path"].as_str().expect("dest_path");
    assert!(
        std::path::Path::new(dest_path).is_dir(),
        "dest missing: {dest_path}"
    );
    // compress stores parent-relative entries (`bundle/hello.txt`), so
    // extract reproduces that layout under the sibling dest.
    let content =
        std::fs::read_to_string(std::path::Path::new(dest_path).join("bundle/hello.txt"))
            .expect("bundle/hello.txt");
    assert_eq!(content, "world");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn zip_list_returns_entries_for_valid_zip() {
    use std::sync::atomic::AtomicBool;

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("k2-fs-routes-zip-list-{nanos}"));
    let src = dir.join("bundle");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("hello.txt"), b"world").unwrap();
    let zip_path = k2_core::fs_compress::compress_to_zip(
        src.to_str().unwrap(),
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .expect("compress fixture");

    let params = HashMap::from([("path".to_string(), zip_path.to_string_lossy().to_string())]);
    let resp = handle_zip_list(&params);
    assert_eq!(resp.status, "200 OK", "body: {}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(v["truncated"].as_bool(), Some(false));
    let entries = v["entries"].as_array().expect("entries array");
    assert!(!entries.is_empty(), "expected at least one entry");
    assert!(
        entries.iter().any(|e| {
            e["name"]
                .as_str()
                .map(|n| n.ends_with("hello.txt"))
                .unwrap_or(false)
        }),
        "hello.txt missing from {:?}",
        entries
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn zip_list_rejects_missing_path_and_non_zip() {
    let resp = handle_zip_list(&HashMap::new());
    assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("k2-fs-routes-zip-list-txt-{nanos}.txt"));
    std::fs::write(&path, b"not a zip").unwrap();
    let params = HashMap::from([("path".to_string(), path.to_string_lossy().to_string())]);
    let resp = handle_zip_list(&params);
    assert_eq!(resp.status, "400 Bad Request", "body: {}", resp.body);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn info_returns_home_separator_and_os() {
    let resp = handle_info(&HashMap::new());
    assert_eq!(resp.status, "200 OK", "fs/info must succeed");

    let v: serde_json::Value =
        serde_json::from_str(&resp.body).expect("fs/info body must be JSON");

    // separator is exactly the host's MAIN_SEPARATOR.
    assert_eq!(
        v["separator"].as_str().expect("separator must be a string"),
        std::path::MAIN_SEPARATOR.to_string(),
    );

    // os is the compile-target OS string.
    assert_eq!(
        v["os"].as_str().expect("os must be a string"),
        std::env::consts::OS,
    );

    // home is present as a string (may be empty if unavailable, but the
    // key must exist and be string-typed — not null/missing).
    assert!(
        v["home"].is_string(),
        "home must be a string, got {:?}",
        v["home"],
    );
}

fn jail_temp_root() -> (PathBuf, String) {
    let dir = std::env::temp_dir().join(format!(
        "k2-skin-fs-jail-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("README.md"), b"hi").expect("readme");
    let s = dunce::canonicalize(&dir).unwrap().to_string_lossy().into_owned();
    (dir, s)
}

#[test]
fn skin_jail_rejects_dotdot_abs_and_empty() {
    let (keep, root_s) = jail_temp_root();
    let ok = jail_rel_path(&root_s, ".", false).expect("dot");
    assert!(ok.starts_with(&root_s), "{ok:?}");
    let file = jail_rel_path(&root_s, "README.md", false).expect("file");
    assert!(file.ends_with("README.md"), "{file:?}");
    assert!(jail_rel_path(&root_s, "", false).is_err());
    assert!(jail_rel_path(&root_s, "/etc/passwd", false).is_err());
    assert!(jail_rel_path(&root_s, "../", false).is_err());
    assert!(jail_rel_path(&root_s, "../outside", false).is_err());
    assert!(jail_rel_path(&root_s, "a/../../outside", false).is_err());
    assert!(jail_rel_path(&root_s, "notes.md", true).is_ok());
    let _ = std::fs::remove_dir_all(keep);
}

#[cfg(unix)]
#[test]
fn skin_jail_write_refuses_existing_leaf_symlink() {
    let (keep, root_s) = jail_temp_root();
    let outside = keep.parent().unwrap().join(format!(
        "k2-skin-fs-outside-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&outside, b"secret-outside").expect("outside");
    std::os::unix::fs::symlink(&outside, keep.join("notes.md")).expect("symlink");
    let err = jail_rel_path(&root_s, "notes.md", true).expect_err("symlink leaf");
    assert!(
        err.contains("escapes the workspace"),
        "symlink write must jail, got {err}"
    );
    assert_eq!(
        std::fs::read_to_string(&outside).expect("outside intact"),
        "secret-outside"
    );
    assert!(
        jail_rel_path(&root_s, "fresh.md", true).is_ok(),
        "create-new-leaf must still pass parent jail"
    );
    assert!(
        jail_rel_path(&root_s, "README.md", true).is_ok(),
        "regular existing leaf must still be writable"
    );
    let _ = std::fs::remove_file(&outside);
    let _ = std::fs::remove_dir_all(keep);
}
fn insert_handled_project(path: &str, handle: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![id, handle, path, handle],
    )
    .expect("insert project");
    id
}

fn session_pass(project_id: &str, caps: &[&str]) -> SkinPass {
    let mut room_policy = k2_core::skin::RoomPolicy::new();
    room_policy.insert(
        project_id.to_string(),
        caps.iter().map(|c| (*c).to_string()).collect(),
    );
    SkinPass {
        id: uuid::Uuid::new_v4().to_string(),
        principal_id: Some(uuid::Uuid::new_v4().to_string()),
        username: "guest".to_string(),
        caps: caps.iter().map(|c| (*c).to_string()).collect(),
        rooms: vec![project_id.to_string()],
        session: true,
        room_policy,
        app: "host".to_string(),
        room_paths: Default::default(),
        expires_at: None,
        chat: false,
    }
}

#[test]
fn skin_rename_rejects_non_segment_names_and_keeps_the_file() {
    let dir = std::env::temp_dir().join(format!(
        "k2-skin-rename-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let notes = dir.join("notes");
    std::fs::create_dir_all(&notes).expect("mkdir");
    let file = notes.join("draft.md");
    std::fs::write(&file, b"keep-me").expect("write");
    let canon = dunce::canonicalize(&dir).expect("canon");
    let handle = format!("rn{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let id = insert_handled_project(&canon.to_string_lossy(), &handle);
    let pass = session_pass(
        &id,
        &[
            k2_core::skin::CAP_FILES_WRITE,
            k2_core::skin::CAP_THREAD_READ,
            k2_core::skin::CAP_THREAD_POST,
        ],
    );
    for name in ["../x", "a/b", ".", ".."] {
        let body = serde_json::json!({
            "workspace": handle,
            "path": "notes/draft.md",
            "new_name": name,
        })
        .to_string();
        let resp = handle_rename_gated(body.as_bytes(), Some(pass.clone()));
        assert_eq!(resp.status, "400 Bad Request", "name {name}: {}", resp.body);
        assert!(
            resp.body.contains("new_name must be a single path segment"),
            "name {name}: {}",
            resp.body
        );
        assert_eq!(
            std::fs::read(&file).expect("rejected rename must leave the file"),
            b"keep-me"
        );
        assert!(
            !dir.join("x").exists(),
            "rejected ../x must not create a sibling"
        );
    }
    let body = serde_json::json!({
        "workspace": handle,
        "path": "notes/draft.md",
        "new_name": "brief.md",
    })
    .to_string();
    let resp = handle_rename_gated(body.as_bytes(), Some(pass));
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
    assert_eq!(v["path"], "notes/brief.md");
    let rel = v["path"].as_str().expect("path string");
    assert!(
        !rel.starts_with('/'),
        "guest path must stay relative: {rel}"
    );
    assert!(
        !resp.body.contains(&canon.to_string_lossy().to_string()),
        "response leaked the room root: {}",
        resp.body
    );
    assert_eq!(
        std::fs::read(notes.join("brief.md")).expect("renamed file"),
        b"keep-me"
    );
    assert!(!file.exists(), "old name must be gone");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn skin_rename_thread_only_missing_write_keeps_the_file() {
    let dir = std::env::temp_dir().join(format!(
        "k2-skin-rename-cap-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let file = dir.join("draft.md");
    std::fs::write(&file, b"keep-me").expect("write");
    let canon = dunce::canonicalize(&dir).expect("canon");
    let handle = format!("rc{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let id = insert_handled_project(&canon.to_string_lossy(), &handle);
    let pass = session_pass(
        &id,
        &[
            k2_core::skin::CAP_THREAD_READ,
            k2_core::skin::CAP_THREAD_POST,
        ],
    );
    let body = serde_json::json!({
        "workspace": handle,
        "path": "draft.md",
        "new_name": "..",
    })
    .to_string();
    let resp = handle_rename_gated(body.as_bytes(), Some(pass));
    assert_eq!(resp.status, "403 Forbidden", "{}", resp.body);
    assert!(
        resp.body.contains("missing capability files:write"),
        "{}",
        resp.body
    );
    assert!(!resp.body.contains("skin_room"), "{}", resp.body);
    assert!(
        !resp.body.contains("single path segment"),
        "cap miss is checked before the segment rule: {}",
        resp.body
    );
    assert_eq!(std::fs::read(&file).expect("file stayed"), b"keep-me");
    let _ = std::fs::remove_dir_all(&dir);
}

// ── GH#72: guests can't write the agent's config folder by default ──

struct ConfigRoom {
    dir: PathBuf,
    handle: String,
    id: String,
}

impl Drop for ConfigRoom {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl ConfigRoom {
    fn role_md(&self) -> PathBuf {
        self.dir.join(".k2/agent/ROLE.md")
    }
    fn read_role(&self) -> String {
        std::fs::read_to_string(self.role_md()).expect("ROLE.md must exist")
    }
}

/// A room with `.k2/agent/ROLE.md`, `.k2/inbox/`, `.k2/heartbeats/`,
/// `notes/a.md`, and `cfg -> .k2` / `agentdir -> .k2/agent` symlinks.
fn config_room(tag: &str) -> ConfigRoom {
    let dir = std::env::temp_dir().join(format!("k2-cfg-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join(".k2/agent")).expect("agent dir");
    std::fs::create_dir_all(dir.join(".k2/inbox")).expect("inbox dir");
    std::fs::create_dir_all(dir.join(".k2/heartbeats/digest")).expect("hb dir");
    std::fs::create_dir_all(dir.join("notes")).expect("notes dir");
    std::fs::write(dir.join(".k2/agent/ROLE.md"), b"original role").expect("role");
    std::fs::write(dir.join("notes/a.md"), b"note a").expect("note");
    // Directory symlinks on every OS (Windows: real symlinks, which a
    // guest's write must not be able to follow into .k2 either).
    k2_core::test_env::symlink(dir.join(".k2"), dir.join("cfg")).expect("cfg link");
    k2_core::test_env::symlink(dir.join(".k2/agent"), dir.join("agentdir")).expect("agentdir link");
    let dir = dunce::canonicalize(&dir).expect("canon");
    let handle = format!("cf{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let id = insert_handled_project(&dir.to_string_lossy(), &handle);
    ConfigRoom { dir, handle, id }
}

fn writer(room: &ConfigRoom, opt_in: bool) -> SkinPass {
    let mut caps = vec![
        k2_core::skin::CAP_THREAD_READ,
        k2_core::skin::CAP_FILES_READ,
        k2_core::skin::CAP_FILES_WRITE,
    ];
    if opt_in {
        caps.push(k2_core::skin::CAP_FILES_AGENT_CONFIG);
    }
    session_pass(&room.id, &caps)
}

fn assert_protected(resp: &CliResponse, label: &str) {
    assert_eq!(resp.status, "403 Forbidden", "{label}: {}", resp.body);
    let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json body");
    assert_eq!(v["error"], "agent_config_protected", "{label}: {}", resp.body);
}

fn write_body(room: &ConfigRoom, path: &str, content: &str) -> Vec<u8> {
    serde_json::json!({"workspace": room.handle, "path": path, "content": content})
        .to_string()
        .into_bytes()
}

fn upload_body(room: &ConfigRoom, dir: &str, filename: &str) -> Vec<u8> {
    serde_json::json!({"workspace": room.handle, "dir": dir, "filename": filename, "base64": "aGk="})
        .to_string()
        .into_bytes()
}

fn chunk_body(room: &ConfigRoom, dir: &str, filename: &str) -> Vec<u8> {
    serde_json::json!({
        "workspace": room.handle,
        "dir": dir,
        "filename": filename,
        "upload_id": uuid::Uuid::new_v4().to_string(),
        "offset": 0,
        "base64": "aGk=",
        "is_last": true,
        "total_bytes": 2,
    })
    .to_string()
    .into_bytes()
}

fn json_bytes(v: serde_json::Value) -> Vec<u8> {
    v.to_string().into_bytes()
}

#[test]
fn guest_writes_into_agent_config_are_refused_by_default() {
    let room = config_room("deny");
    let pass = writer(&room, false);
    let h = room.handle.clone();

    // write-file: the role file, a new inbox item, a new agent file.
    for path in [".k2/agent/ROLE.md", ".k2/inbox/task.md", ".k2/agent/new.txt", ".k2"] {
        let r = handle_write_file_gated(&write_body(&room, path, "guest text"), Some(pass.clone()));
        assert_protected(&r, &format!("write-file {path}"));
    }
    // Case, separator and trailing-dot variants of the folder name.
    for path in [
        ".K2/agent/ROLE.md",
        ".K2/inbox/x.md",
        ".k2\\agent\\ROLE.md",
        ".k2./agent/x.md",
        "./.k2/inbox/x.md",
    ] {
        let r = handle_write_file_gated(&write_body(&room, path, "guest text"), Some(pass.clone()));
        assert_protected(&r, &format!("write-file variant {path}"));
    }
    // Symlinks inside the room that resolve into .k2, including a
    // not-yet-created folder behind the link.
    for path in ["cfg/agent/ROLE.md", "agentdir/ROLE.md", "cfg/inbox/x.md", "cfg/newdir/x.md"] {
        let r = handle_write_file_gated(&write_body(&room, path, "guest text"), Some(pass.clone()));
        assert_protected(&r, &format!("write-file via symlink {path}"));
    }
    // `..` never reaches the check: the jail refuses it first.
    for path in ["notes/../.k2/agent/ROLE.md", "../x/.k2/agent/ROLE.md"] {
        let r = handle_write_file_gated(&write_body(&room, path, "guest text"), Some(pass.clone()));
        assert_eq!(r.status, "400 Bad Request", "{path}: {}", r.body);
        assert!(r.body.contains("'..'"), "{path}: {}", r.body);
    }

    // upload-binary: into the folder, and a file named .k2 at the root.
    for (dir, name) in [
        (".k2/inbox", "x.bin"),
        (".k2", "x.bin"),
        (".", ".k2"),
        ("cfg/inbox", "x.bin"),
        ("cfg/brandnew", "x.bin"),
    ] {
        let r = handle_upload_binary_gated(&upload_body(&room, dir, name), Some(pass.clone()));
        assert_protected(&r, &format!("upload-binary {dir}/{name}"));
    }
    // upload-chunk: a chunk and a status probe.
    for dir in [".k2/inbox", "cfg/inbox", ".K2/agent", "cfg/brandnew"] {
        let r = handle_upload_chunk_gated(&chunk_body(&room, dir, "big.bin"), Some(pass.clone()));
        assert_protected(&r, &format!("upload-chunk {dir}"));
    }
    let status = json_bytes(serde_json::json!({
        "workspace": h, "dir": ".k2/inbox", "filename": "big.bin", "upload_id": "u1", "status": true,
    }));
    assert_protected(&handle_upload_chunk_gated(&status, Some(pass.clone())), "upload-chunk status");

    // create (mkdir and file).
    for (path, is_dir) in [(".k2/skills", true), (".k2/inbox/new.md", false), ("cfg/brandnew", true)] {
        let body = json_bytes(serde_json::json!({"workspace": h, "path": path, "is_directory": is_dir}));
        assert_protected(&handle_create_gated(&body, Some(pass.clone())), &format!("create {path}"));
    }
    // copy / move INTO the folder.
    for dest in [".k2/inbox", ".k2/agent", "cfg/inbox"] {
        let body = json_bytes(serde_json::json!({"workspace": h, "sources": ["notes/a.md"], "destination": dest}));
        assert_protected(&handle_copy_gated(&body, Some(pass.clone())), &format!("copy into {dest}"));
        assert_protected(&handle_move_gated(&body, Some(pass.clone())), &format!("move into {dest}"));
    }
    // move OUT of the folder.
    for src in [".k2/agent/ROLE.md", ".k2/inbox", "agentdir/ROLE.md"] {
        let body = json_bytes(serde_json::json!({"workspace": h, "sources": [src], "destination": "notes"}));
        assert_protected(&handle_move_gated(&body, Some(pass.clone())), &format!("move out {src}"));
    }
    // `.K2` spelling, on the room's own filesystem. Case-insensitive
    // (macOS default APFS): `.K2` IS `.k2`. Case-sensitive (Linux):
    // `.K2` is a separate folder, which the check refuses anyway (the
    // name is folded on every platform: harmless there, and a Linux
    // room can still sit on a case-folding mount). A move source must
    // exist, so give `.K2/agent` its own folder when it is not `.k2`.
    let case_folds = room.dir.join(".K2").exists();
    if !case_folds {
        std::fs::create_dir_all(room.dir.join(".K2/agent")).expect("separate .K2/agent");
    }
    let body = json_bytes(serde_json::json!({"workspace": h, "sources": [".K2/agent"], "destination": "notes"}));
    assert_protected(
        &handle_move_gated(&body, Some(pass.clone())),
        &format!("move out .K2/agent (case-folding fs: {case_folds})"),
    );
    assert!(room.dir.join(".K2/agent").is_dir(), "refused move left .K2/agent in place");
    assert!(!room.dir.join("notes/agent").exists(), "nothing moved into notes");
    // rename OUT of (source in .k2) and INTO (new name is .k2).
    for (path, new_name) in [(".k2/agent/ROLE.md", "ROLE.old"), (".k2/inbox", "box"), ("notes", ".K2")] {
        let body = json_bytes(serde_json::json!({"workspace": h, "path": path, "new_name": new_name}));
        assert_protected(&handle_rename_gated(&body, Some(pass.clone())), &format!("rename {path} -> {new_name}"));
    }
    // `.k2` itself (and the root) is also R2 heartbeat ground: still 403.
    for src in [".k2", "."] {
        let body = json_bytes(serde_json::json!({"workspace": h, "sources": [src], "destination": "notes"}));
        let r = handle_move_gated(&body, Some(pass.clone()));
        assert_eq!(r.status, "403 Forbidden", "move {src}: {}", r.body);
    }

    // Nothing changed on disk.
    assert_eq!(room.read_role(), "original role");
    assert_eq!(std::fs::read_dir(room.dir.join(".k2/inbox")).expect("inbox").count(), 0);
    assert!(!room.dir.join(".k2/agent/new.txt").exists());
    assert!(!room.dir.join(".k2/skills").exists());
    assert!(!room.dir.join(".k2/brandnew").exists());
    assert!(!room.dir.join(".k2/newdir").exists());
    assert!(room.dir.join("notes/a.md").exists(), "a refused move must leave the source");
    assert!(room.dir.join("notes").is_dir());

    // Reads under .k2 still work (2026-09-29 ruling).
    let mut q = HashMap::new();
    q.insert("workspace".to_string(), h.clone());
    q.insert("path".to_string(), ".k2/agent/ROLE.md".to_string());
    let r = handle_read_file_gated(&q, Some(pass.clone()));
    assert_eq!(r.status, "200 OK", "read ROLE.md: {}", r.body);
    assert!(r.body.contains("original role"), "{}", r.body);
    // Copying OUT of .k2 is a read: allowed.
    let body = json_bytes(serde_json::json!({"workspace": h, "sources": [".k2/agent/ROLE.md"], "destination": "notes"}));
    let r = handle_copy_gated(&body, Some(pass.clone()));
    assert_eq!(r.status, "200 OK", "copy out of .k2: {}", r.body);
    assert_eq!(std::fs::read_to_string(room.dir.join("notes/ROLE.md")).expect("copy"), "original role");

    // Ordinary room files still work, every door.
    let r = handle_write_file_gated(&write_body(&room, "notes/today.md", "ok"), Some(pass.clone()));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let r = handle_write_file_gated(&write_body(&room, "k2-notes.md", "ok"), Some(pass.clone()));
    assert_eq!(r.status, "200 OK", "a name that merely contains k2: {}", r.body);
    let body = json_bytes(serde_json::json!({"workspace": h, "path": "notes/.k2", "is_directory": true}));
    let r = handle_create_gated(&body, Some(pass.clone()));
    assert_eq!(r.status, "200 OK", "a nested .k2 folder isn't the agent's: {}", r.body);
    let r = handle_write_file_gated(&write_body(&room, "notes/.k2/x.md", "ok"), Some(pass.clone()));
    assert_eq!(r.status, "200 OK", "a .k2 folder that isn't the room's: {}", r.body);
    let r = handle_upload_binary_gated(&upload_body(&room, "notes", "u.bin"), Some(pass.clone()));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let r = handle_upload_chunk_gated(&chunk_body(&room, "notes/big", "big.bin"), Some(pass.clone()));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let body = json_bytes(serde_json::json!({"workspace": h, "path": "notes/sub", "is_directory": true}));
    assert_eq!(handle_create_gated(&body, Some(pass.clone())).status, "200 OK");
    let body = json_bytes(serde_json::json!({"workspace": h, "path": "notes/today.md", "new_name": "t.md"}));
    assert_eq!(handle_rename_gated(&body, Some(pass.clone())).status, "200 OK");
    let body = json_bytes(serde_json::json!({"workspace": h, "sources": ["notes/t.md"], "destination": "notes/sub"}));
    assert_eq!(handle_move_gated(&body, Some(pass.clone())).status, "200 OK");
    assert!(room.dir.join("notes/sub/t.md").exists());
}

/// Regression (0.45.6 gate): upload-chunk never writes through an entry
/// already at the final name (it publishes `name (1).ext` beside it),
/// so a leaf symlink there, pointing out of the room or into `.k2`,
/// must not turn an ordinary upload into `agent_config_protected`.
/// upload-binary DOES write through a dangling leaf (`fs::write` after
/// an `exists()` probe), so it keeps following the leaf: refused.
#[cfg(unix)]
#[test]
fn upload_chunk_publishes_beside_a_leaf_symlink_and_still_guards_k2() {
    let room = config_room("leaf");
    let pass = writer(&room, false);
    let outside = std::env::temp_dir().join(format!("k2-cfg-out-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&outside).expect("outside dir");
    let victim = outside.join("victim.txt");
    std::fs::write(&victim, b"untouched").expect("victim");
    std::os::unix::fs::symlink(&victim, room.dir.join("notes/out.bin")).expect("leaf out");
    std::os::unix::fs::symlink(room.dir.join(".k2/agent/ROLE.md"), room.dir.join("notes/role.md"))
        .expect("leaf into .k2");

    for (name, beside) in [("out.bin", "notes/out (1).bin"), ("role.md", "notes/role (1).md")] {
        let r = handle_upload_chunk_gated(&chunk_body(&room, "notes", name), Some(pass.clone()));
        assert_eq!(r.status, "200 OK", "upload-chunk notes/{name}: {}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json body");
        assert_eq!(v["path"], beside, "published beside the link: {}", r.body);
        assert_eq!(std::fs::read(room.dir.join(beside)).expect("published"), b"hi");
    }
    assert_eq!(std::fs::read(&victim).expect("victim"), b"untouched");
    assert_eq!(room.read_role(), "original role");

    // Still guarded: `.k2` as the name at the root, and a folder that
    // resolves into `.k2`.
    assert_protected(
        &handle_upload_chunk_gated(&chunk_body(&room, ".", ".k2"), Some(pass.clone())),
        "upload-chunk ./.k2",
    );
    assert_protected(
        &handle_upload_chunk_gated(&chunk_body(&room, "cfg/inbox", "x.bin"), Some(pass.clone())),
        "upload-chunk cfg/inbox",
    );
    // upload-binary follows the leaf: a link into `.k2` is refused.
    assert_protected(
        &handle_upload_binary_gated(&upload_body(&room, "notes", "role.md"), Some(pass.clone())),
        "upload-binary through a leaf link into .k2",
    );
    assert_eq!(room.read_role(), "original role");
    assert_eq!(std::fs::read_dir(room.dir.join(".k2/inbox")).expect("inbox").count(), 0);
    std::fs::remove_dir_all(&outside).expect("cleanup outside");
}

/// The agent's folder has had three names: `.k2`, `.k2so` before it, and
/// `.k5` next. A room can still hold a leftover `.k2so/` beside its
/// `.k2/`, with the root links (`CLAUDE.md`, `.claude/skills/…`) pointing
/// into it: the migration only re-pointed them when it renamed the folder.
/// A guest's `files:write` stops at every one of the three, under any
/// spelling, and at the target of such a link.
#[test]
fn guest_writes_into_the_agent_folder_are_refused_under_each_of_its_names() {
    let room = config_room("names");
    let h = room.handle.clone();
    // The leftover folder with a skill in it, and the root link that
    // still points there; and a folder under the coming name.
    std::fs::create_dir_all(room.dir.join(".k2so/skills/k2-cli")).expect("old skills dir");
    std::fs::create_dir_all(room.dir.join(".k2so/agent")).expect("old agent dir");
    std::fs::write(room.dir.join(".k2so/skills/k2-cli/SKILL.md"), b"old skill").expect("old skill");
    std::fs::write(room.dir.join(".k2so/agent/ROLE.md"), b"old role").expect("old role");
    k2_core::test_env::symlink(room.dir.join(".k2so/skills/k2-cli/SKILL.md"), room.dir.join("SKILL.md")).expect("root link");
    k2_core::test_env::symlink(room.dir.join(".k2so/skills"), room.dir.join("oldskills")).expect("dir link");
    std::fs::create_dir_all(room.dir.join(".k5/agent")).expect("new agent dir");
    std::fs::write(room.dir.join(".k5/agent/ROLE.md"), b"new role").expect("new role");
    let pass = writer(&room, false);

    // write-file: each name, an upper-case spelling of each, a trailing
    // dot, a backslash, and the folder itself.
    for path in [
        ".k2so/skills/k2-cli/SKILL.md",
        ".k2so/agent/ROLE.md",
        ".k2so/agent/new.txt",
        ".k2so",
        ".K2SO/skills/k2-cli/SKILL.md",
        ".K2so/agent/ROLE.md",
        ".k2so./agent/x.md",
        ".k2so\\agent\\ROLE.md",
        "./.k2so/agent/x.md",
        ".k5/agent/ROLE.md",
        ".k5/agent/new.txt",
        ".k5",
        ".K5/agent/ROLE.md",
        ".k5 /agent/x.md",
        ".k2/agent/ROLE.md",
        ".K2/agent/ROLE.md",
    ] {
        let r = handle_write_file_gated(&write_body(&room, path, "guest text"), Some(pass.clone()));
        assert_protected(&r, &format!("write-file {path}"));
    }
    // The target of a root link into the leftover folder, by the link's
    // own name and through a linked folder.
    for path in ["SKILL.md", "oldskills/k2-cli/SKILL.md", "oldskills/new/SKILL.md"] {
        let r = handle_write_file_gated(&write_body(&room, path, "guest text"), Some(pass.clone()));
        assert_protected(&r, &format!("write-file through a link: {path}"));
    }
    // upload-binary and upload-chunk.
    for (dir, name) in [(".k2so/skills/k2-cli", "x.bin"), (".k2so", "x.bin"), (".", ".k2so"), (".", ".k5"), (".k5/agent", "x.bin"), ("oldskills", "x.bin")] {
        let r = handle_upload_binary_gated(&upload_body(&room, dir, name), Some(pass.clone()));
        assert_protected(&r, &format!("upload-binary {dir}/{name}"));
    }
    for dir in [".k2so/agent", ".K2SO/agent", ".k5/agent", "oldskills/k2-cli"] {
        let r = handle_upload_chunk_gated(&chunk_body(&room, dir, "big.bin"), Some(pass.clone()));
        assert_protected(&r, &format!("upload-chunk {dir}"));
    }
    // create, copy and move INTO, move and rename OUT OF, rename INTO.
    for (path, is_dir) in [(".k2so/skills/new", true), (".k2so/agent/new.md", false), (".k5/skills", true), ("oldskills/new", true)] {
        let body = json_bytes(serde_json::json!({"workspace": h, "path": path, "is_directory": is_dir}));
        assert_protected(&handle_create_gated(&body, Some(pass.clone())), &format!("create {path}"));
    }
    for dest in [".k2so/agent", ".k2so/skills/k2-cli", ".k5/agent", "oldskills"] {
        let body = json_bytes(serde_json::json!({"workspace": h, "sources": ["notes/a.md"], "destination": dest}));
        assert_protected(&handle_copy_gated(&body, Some(pass.clone())), &format!("copy into {dest}"));
        assert_protected(&handle_move_gated(&body, Some(pass.clone())), &format!("move into {dest}"));
    }
    for src in [".k2so/agent/ROLE.md", ".k2so/skills", ".k5/agent/ROLE.md", "oldskills/k2-cli/SKILL.md"] {
        let body = json_bytes(serde_json::json!({"workspace": h, "sources": [src], "destination": "notes"}));
        assert_protected(&handle_move_gated(&body, Some(pass.clone())), &format!("move out {src}"));
    }
    for (path, new_name) in [(".k2so/agent/ROLE.md", "ROLE.old"), (".k2so/skills", "box"), (".k5/agent", "box"), ("notes", ".k2so"), ("notes", ".K5")] {
        let body = json_bytes(serde_json::json!({"workspace": h, "path": path, "new_name": new_name}));
        assert_protected(&handle_rename_gated(&body, Some(pass.clone())), &format!("rename {path} -> {new_name}"));
    }

    // Nothing changed on disk.
    assert_eq!(std::fs::read_to_string(room.dir.join(".k2so/skills/k2-cli/SKILL.md")).expect("old skill"), "old skill");
    assert_eq!(std::fs::read_to_string(room.dir.join(".k2so/agent/ROLE.md")).expect("old role"), "old role");
    assert_eq!(std::fs::read_to_string(room.dir.join(".k5/agent/ROLE.md")).expect("new role"), "new role");
    assert_eq!(room.read_role(), "original role");
    assert!(!room.dir.join(".k2so/agent/new.txt").exists());
    assert!(!room.dir.join(".k2so/skills/new").exists());
    assert!(!room.dir.join(".k5/skills").exists());
    assert!(room.dir.join("notes/a.md").exists(), "a refused move must leave the source");
    assert!(room.dir.join("notes").is_dir(), "a refused rename must leave the folder");

    // Names that only look like it are ordinary room files.
    for dir in [".k2so-old", "docs/.k2so", ".k5x"] {
        std::fs::create_dir_all(room.dir.join(dir)).expect("look-alike dir");
    }
    for path in ["k2so-notes.md", ".k2so-old/x.md", "docs/.k2so/x.md", ".k5x/x.md"] {
        let r = handle_write_file_gated(&write_body(&room, path, "ok"), Some(pass.clone()));
        assert_eq!(r.status, "200 OK", "{path}: {}", r.body);
    }
    // With the opt-in a guest writes the leftover folder as it writes `.k2`.
    let opted = writer(&room, true);
    let r = handle_write_file_gated(&write_body(&room, ".k2so/agent/note.md", "ok"), Some(opted));
    assert_eq!(r.status, "200 OK", "opt-in write under .k2so: {}", r.body);
}

/// `files:write` alone doesn't reach the files the agent's CLI loads as standing
/// orders; `files:agentconfig` does. Only at the room's root, and only writes.
#[test]
fn guest_writes_to_root_instruction_files_need_the_opt_in() {
    let room = config_room("instr");
    let pass = writer(&room, false);
    for path in ["CLAUDE.md", "AGENTS.md", "GEMINI.md", "claude.md", "./AGENTS.md", "GEMINI.md.", "CLAUDE.md "] {
        let r = handle_write_file_gated(&write_body(&room, path, "guest text"), Some(pass.clone()));
        assert_protected(&r, &format!("write-file {path}"));
    }
    for path in ["notes/CLAUDE.md", "README.md", "notes/AGENTS.md"] {
        let r = handle_write_file_gated(&write_body(&room, path, "guest text"), Some(pass.clone()));
        assert_eq!(r.status, "200 OK", "{path}: {}", r.body);
    }
    let opted = writer(&room, true);
    let r = handle_write_file_gated(&write_body(&room, "CLAUDE.md", "guest text"), Some(opted));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    // What a guest may read or download doesn't change.
    assert!(!lexically_in_agent_config("CLAUDE.md", false));
}

/// Heartbeat ground is the `heartbeats` folder under each name the agent's
/// folder has had, and it holds even with the `files:agentconfig` opt-in.
/// The one file a guest may write there is under the live `.k2` only.
#[test]
fn heartbeat_ground_holds_under_each_name_of_the_agent_folder() {
    let room = config_room("hbnames");
    let h = room.handle.clone();
    std::fs::create_dir_all(room.dir.join(".k2so/heartbeats/digest")).expect("old hb dir");
    std::fs::write(room.dir.join(".k2so/heartbeats/digest/WAKEUP.md"), b"old wake").expect("old wake");
    std::fs::create_dir_all(room.dir.join(".k5/heartbeats/digest")).expect("new hb dir");
    k2_core::test_env::symlink(room.dir.join(".k2so"), room.dir.join("oldcfg")).expect("oldcfg link");
    let heartbeats_area = |r: &CliResponse, label: &str| {
        assert_eq!(r.status, "403 Forbidden", "{label}: {}", r.body);
        assert!(r.body.contains("heartbeats_area"), "{label}: {}", r.body);
    };
    for opt_in in [true, false] {
        let pass = writer(&room, opt_in);
        for path in [
            ".k2so/heartbeats/digest/WAKEUP.md",
            ".k2so/heartbeats/digest/notes.md",
            ".k2so/heartbeats/newhb/WAKEUP.md",
            ".K2SO/Heartbeats/digest/x.md",
            ".k2so./heartbeats/digest/x.md",
            ".k5/heartbeats/digest/WAKEUP.md",
            ".K5/heartbeats/digest/x.md",
        ] {
            let r = handle_write_file_gated(&write_body(&room, path, "guest wake"), Some(pass.clone()));
            heartbeats_area(&r, &format!("write-file {path} (opt-in {opt_in})"));
        }
        #[cfg(unix)]
        {
            let r = handle_write_file_gated(&write_body(&room, "oldcfg/heartbeats/digest/WAKEUP.md", "x"), Some(pass.clone()));
            heartbeats_area(&r, &format!("through a link into .k2so (opt-in {opt_in})"));
            let body = json_bytes(serde_json::json!({"workspace": h, "path": "oldcfg/heartbeats/newhb", "is_directory": true}));
            heartbeats_area(&handle_create_gated(&body, Some(pass.clone())), &format!("create through a link (opt-in {opt_in})"));
        }
        let body = json_bytes(serde_json::json!({"workspace": h, "sources": ["notes/a.md"], "destination": ".k2so/heartbeats/digest"}));
        heartbeats_area(&handle_copy_gated(&body, Some(pass.clone())), &format!("copy into .k2so/heartbeats (opt-in {opt_in})"));
        let body = json_bytes(serde_json::json!({"workspace": h, "sources": [".k2so/heartbeats/digest"], "destination": "notes"}));
        heartbeats_area(&handle_move_gated(&body, Some(pass.clone())), &format!("move out of .k2so/heartbeats (opt-in {opt_in})"));
    }
    assert_eq!(std::fs::read_to_string(room.dir.join(".k2so/heartbeats/digest/WAKEUP.md")).expect("old wake"), "old wake");
    assert!(!room.dir.join(".k2so/heartbeats/newhb").exists());
    assert!(!room.dir.join(".k2so/heartbeats/digest/notes.md").exists());
    assert!(!room.dir.join(".k5/heartbeats/digest/WAKEUP.md").exists());
    assert!(room.dir.join("notes/a.md").exists());
}

#[test]
fn opt_in_lets_a_guest_write_agent_config_but_not_heartbeats() {
    let room = config_room("allow");
    let pass = writer(&room, true);
    let h = room.handle.clone();

    let r = handle_write_file_gated(&write_body(&room, ".k2/agent/ROLE.md", "owner allowed"), Some(pass.clone()));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(room.read_role(), "owner allowed");
    let r = handle_write_file_gated(&write_body(&room, ".k2/inbox/task.md", "t"), Some(pass.clone()));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let r = handle_upload_binary_gated(&upload_body(&room, ".k2/inbox", "x.bin"), Some(pass.clone()));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let r = handle_upload_chunk_gated(&chunk_body(&room, ".k2/inbox", "big.bin"), Some(pass.clone()));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let body = json_bytes(serde_json::json!({"workspace": h, "path": ".k2/skills", "is_directory": true}));
    assert_eq!(handle_create_gated(&body, Some(pass.clone())).status, "200 OK");
    let body = json_bytes(serde_json::json!({"workspace": h, "sources": ["notes/a.md"], "destination": ".k2/inbox"}));
    assert_eq!(handle_copy_gated(&body, Some(pass.clone())).status, "200 OK");
    let body = json_bytes(serde_json::json!({"workspace": h, "path": ".k2/inbox/a.md", "new_name": "b.md"}));
    assert_eq!(handle_rename_gated(&body, Some(pass.clone())).status, "200 OK");
    let body = json_bytes(serde_json::json!({"workspace": h, "sources": [".k2/inbox/b.md"], "destination": "notes"}));
    assert_eq!(handle_move_gated(&body, Some(pass.clone())).status, "200 OK");
    assert!(room.dir.join("notes/b.md").exists());
    assert!(room.dir.join(".k2/inbox/task.md").exists());
    assert!(room.dir.join(".k2/inbox/x.bin").exists());
    assert!(room.dir.join(".k2/inbox/big.bin").exists());
    assert!(room.dir.join(".k2/skills").is_dir());

    // R2 holds even with the opt-in: heartbeat ground stays managed.
    let r = handle_write_file_gated(&write_body(&room, ".k2/heartbeats/digest/notes.md", "x"), Some(pass.clone()));
    assert_eq!(r.status, "403 Forbidden", "{}", r.body);
    assert!(r.body.contains("heartbeats_area"), "{}", r.body);
    #[cfg(unix)]
    {
        let r = handle_write_file_gated(&write_body(&room, "cfg/heartbeats/newhb/x.md", "x"), Some(pass.clone()));
        assert_eq!(r.status, "403 Forbidden", "symlink + new folder into heartbeats: {}", r.body);
        assert!(r.body.contains("heartbeats_area"), "{}", r.body);
        let body = json_bytes(serde_json::json!({"workspace": h, "path": "cfg/heartbeats/newhb", "is_directory": true}));
        let r = handle_create_gated(&body, Some(pass.clone()));
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        assert!(r.body.contains("heartbeats_area"), "{}", r.body);
        assert!(!room.dir.join(".k2/heartbeats/newhb").exists());
    }

    // The opt-in is per room: it does not carry to a room without it.
    let other = config_room("other");
    let mut policy = pass.room_policy.clone();
    policy.insert(other.id.clone(), vec![k2_core::skin::CAP_FILES_WRITE.to_string()]);
    let two_rooms = SkinPass {
        rooms: vec![room.id.clone(), other.id.clone()],
        room_policy: policy,
        ..pass.clone()
    };
    let r = handle_write_file_gated(&write_body(&other, ".k2/agent/ROLE.md", "x"), Some(two_rooms.clone()));
    assert_protected(&r, "opt-in is per room");
    assert_eq!(other.read_role(), "original role");
    let r = handle_write_file_gated(&write_body(&room, ".k2/agent/ROLE.md", "again"), Some(two_rooms));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(room.read_role(), "again");
}

#[test]
fn owner_writes_into_agent_config_are_unaffected() {
    let room = config_room("owner");
    let body = serde_json::json!({
        "path": room.role_md().to_string_lossy(),
        "content": "owner text",
    })
    .to_string();
    let r = handle_write_file_gated(body.as_bytes(), None);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(room.read_role(), "owner text");
    let body = serde_json::json!({
        "path": room.dir.join(".k2/inbox/owner.md").to_string_lossy(),
        "is_directory": false,
    })
    .to_string();
    let r = handle_create_gated(body.as_bytes(), None);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(room.dir.join(".k2/inbox/owner.md").exists());
}

#[test]
fn agent_config_path_checks() {
    for (rel, want) in [
        (".k2", true),
        (".k2/agent/ROLE.md", true),
        ("./.k2/inbox", true),
        (".K2/agent", true),
        (".k2\\agent", true),
        (".k2./x", true),
        (".k2 /x", true),
        ("notes/.k2/x", false),
        ("k2/x", false),
        (".k2x/y", false),
        ("notes", false),
    ] {
        assert_eq!(lexically_in_agent_config(rel, false), want, "{rel}");
    }
    assert!(lexically_in_agent_config(".", true), "the root carries .k2 along");
    assert!(!lexically_in_agent_config(".", false));

    let room = config_room("abs");
    let root = room.dir.to_string_lossy().into_owned();
    assert!(abs_in_agent_config(&root, &room.dir.join(".k2/agent/ROLE.md"), false));
    assert!(abs_in_agent_config(&root, &room.dir.join(".k2/new/deeper/x"), false));
    assert!(!abs_in_agent_config(&root, &room.dir.join("notes/a.md"), false));
    assert!(!abs_in_agent_config(&root, &room.dir.join("notes/new/deeper"), false));
    assert!(!abs_in_agent_config(&root, &room.dir, false));
    assert!(abs_in_agent_config(&root, &room.dir, true));
    #[cfg(unix)]
    {
        assert!(abs_in_agent_config(&root, &room.dir.join("cfg/agent/ROLE.md"), false));
        assert!(abs_in_agent_config(&root, &room.dir.join("cfg/new/deeper/x"), false));
        assert!(abs_in_agent_config(&root, &room.dir.join("agentdir"), false));
        // A dangling link fails closed.
        std::os::unix::fs::symlink(room.dir.join("nowhere"), room.dir.join("dangle"))
            .expect("dangling link");
        assert!(abs_in_agent_config(&root, &room.dir.join("dangle/x"), false));
    }
}

#[test]
fn owner_rename_dot_and_dotdot_are_not_the_skin_segment_rule() {
    let dir = std::env::temp_dir().join(format!(
        "k2-owner-rename-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let notes = dir.join("notes");
    std::fs::create_dir_all(&notes).expect("mkdir");
    let file = notes.join("draft.md");
    std::fs::write(&file, b"keep-me").expect("write");
    for name in [".", ".."] {
        let body = serde_json::json!({
            "old_path": file.to_string_lossy(),
            "new_name": name,
        })
        .to_string();
        let resp = handle_rename(body.as_bytes());
        assert!(
            !resp.body.contains("single path segment"),
            "owner rename must keep accepting {name}: {}",
            resp.body
        );
        assert_eq!(
            std::fs::read(&file).expect("owner rename must leave the file in place"),
            b"keep-me"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ── The drop-folder race (0.46.7): one re-resolve, then the full check ──

/// A room with `.k2/agent` and `Inbox/`, canonical.
fn race_room(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("k2-race-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join(".k2").join("agent")).expect("agent dir");
    std::fs::create_dir_all(dir.join("Inbox")).expect("inbox dir");
    dunce::canonicalize(&dir).expect("canon room")
}

/// The protected-path verdict the upload doors take on a resolved
/// target: `true` = refused as the agent's config (or unresolvable).
fn refused(root: &Path, resolved: Option<&Path>) -> bool {
    resolved_in_agent_config(&root.to_string_lossy(), resolved, false)
}

/// (1) A real dangling link is still refused: nothing to resolve, even
/// with the one retry.
#[test]
fn a_dangling_link_still_fails_closed() {
    let root = race_room("dangle");
    let link = root.join("Inbox").join("gone");
    k2_core::test_env::symlink(root.join("no-such-folder"), &link).expect("dangling link");
    let got = resolve_existing_prefix(&link.join("f.bin"));
    assert_eq!(got, None, "a dangling link resolves to nothing");
    assert!(refused(&root, got.as_deref()), "and is refused");
    let _ = std::fs::remove_dir_all(&root);
}

/// (2) A link loop is still refused.
#[test]
fn a_link_loop_still_fails_closed() {
    let root = race_room("loop");
    let (a, b) = (root.join("Inbox").join("a"), root.join("Inbox").join("b"));
    k2_core::test_env::symlink(&b, &a).expect("a -> b");
    k2_core::test_env::symlink(&a, &b).expect("b -> a");
    let got = resolve_existing_prefix(&a.join("f.bin"));
    assert_eq!(got, None, "a loop resolves to nothing");
    assert!(refused(&root, got.as_deref()), "and is refused");
    let _ = std::fs::remove_dir_all(&root);
}

/// (3) A link into `.k2` made between the failed canonicalize and the
/// existence check: the retry resolves it into `.k2`, and the decision
/// on that path refuses it.
#[test]
fn a_link_into_k2_made_in_the_window_is_refused() {
    let root = race_room("k2link");
    let target = root.join("Inbox").join("guest");
    let mut made = 0;
    let got = resolve_existing_prefix_seam(&target, |cur| {
        if cur == target.as_path() && made == 0 {
            made += 1;
            k2_core::test_env::symlink(root.join(".k2").join("agent"), &target).expect("racing link into .k2");
        }
    });
    assert_eq!(made, 1, "the seam ran in the window");
    let got = got.expect("the new link resolves");
    assert!(got.starts_with(root.join(".k2")), "resolved into .k2: {}", got.display());
    assert!(refused(&root, Some(&got)), "a link into .k2 is refused");
    let _ = std::fs::remove_dir_all(&root);
}

/// (4) The guest's drop folder made by a parallel request in the same
/// window: the retry resolves it, and it is allowed (it was refused as
/// "agent_config_protected" before 0.46.7).
#[test]
fn a_folder_made_in_the_window_is_resolved_and_allowed() {
    let root = race_room("mkdir");
    let target = root.join("Inbox").join("guest");
    let mut made = 0;
    let got = resolve_existing_prefix_seam(&target.join("f.bin"), |cur| {
        if cur == target.as_path() && made == 0 {
            made += 1;
            std::fs::create_dir(&target).expect("racing mkdir");
        }
    });
    assert_eq!(made, 1, "the seam ran in the window");
    let got = got.expect("the new folder resolves");
    assert_eq!(got, root.join("Inbox").join("guest").join("f.bin"));
    assert!(!refused(&root, Some(&got)), "an ordinary drop folder is allowed");
    let _ = std::fs::remove_dir_all(&root);
}

/// The editors' autosave: `expect` (the SHA-256 of the file as read) makes
/// `fs/write-file` refuse to write over another program's save, 409 with
/// what is there now; `absent` guards a new file; no `expect` is as before.
#[test]
fn write_file_with_expect_never_writes_over_another_save() {
    let dir = untitled_tmp("expect");
    let f = dir.join("notes.md");
    let path = f.to_string_lossy().to_string();
    std::fs::write(&f, "# mine\n").unwrap();
    let read_hash = content_sha256(b"# mine\n");
    assert_eq!(read_hash.len(), 64);
    let write = |content: &str, expect: Option<&str>| {
        let mut b = serde_json::json!({ "path": path, "content": content });
        if let Some(e) = expect {
            b["expect"] = serde_json::json!(e);
        }
        handle_write_file(b.to_string().as_bytes())
    };
    // Unchanged since the read: written.
    let r = write("# mine, edited\n", Some(&read_hash));
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "# mine, edited\n");
    // Another program saves; the editor's next autosave carries its old base.
    let mine = content_sha256(b"# mine, edited\n");
    std::fs::write(&f, "# theirs\n").unwrap();
    let r = write("# mine, edited more\n", Some(&mine));
    assert_eq!(r.status, "409 Conflict", "{}", r.body);
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(v["code"], "file_changed", "{v}");
    assert_eq!(v["current"]["sha256"], content_sha256(b"# theirs\n").as_str(), "{v}");
    assert_eq!(v["current"]["content"], "# theirs\n", "the text, to merge without another read: {v}");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "# theirs\n", "their save stays");
    // A new file: absent while nothing is there, file_exists after.
    let new = dir.join("new.md");
    let new_path = new.to_string_lossy().to_string();
    let w_new = |expect: &str| handle_write_file(serde_json::json!({ "path": new_path, "content": "hi", "expect": expect }).to_string().as_bytes());
    assert_eq!(w_new(EXPECT_ABSENT).status, "200 OK");
    let r = w_new(EXPECT_ABSENT);
    assert_eq!(r.status, "409 Conflict", "{}", r.body);
    assert!(r.body.contains("file_exists"), "{}", r.body);
    // The file goes: file_missing.
    std::fs::remove_file(&new).unwrap();
    let r = w_new(&content_sha256(b"hi"));
    assert_eq!(r.status, "409 Conflict", "{}", r.body);
    assert!(r.body.contains("file_missing"), "{}", r.body);
    assert!(!new.exists(), "nothing written");
    // A malformed expect is a 400; none at all writes as before.
    assert_eq!(write("x", Some("ABC")).status, "400 Bad Request");
    assert_eq!(write("# no check\n", None).status, "200 OK");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "# no check\n");
    // An App guest's write gets the code and the hash, never the text.
    let skin_like = expect_unchanged(&path, Some(&mine), false).expect_err("changed");
    assert!(!skin_like.body.contains("no check") && skin_like.body.contains("file_changed"), "{}", skin_like.body);
    let _ = std::fs::remove_dir_all(&dir);
}
