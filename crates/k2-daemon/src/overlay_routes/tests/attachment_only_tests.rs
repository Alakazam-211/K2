//! GH#87: Return in the Thread box with a file and no words posts the file
//! alone. The rule lives here: a person's compose post with attachments
//! and no text (empty, or no `text` field at all) is stored with an empty
//! body and its files; with neither text nor files it is refused as usage
//! and nothing is stored.

use super::*;

#[test]
fn a_compose_post_with_files_and_no_text_is_stored_and_one_with_neither_is_refused() {
    let handle = format!("ovlonly{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    let conv = uuid::Uuid::new_v4().to_string();
    pin(&project_id, &conv);
    let root = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let p: String = conn
            .query_row("SELECT path FROM projects WHERE id = ?1", params![project_id], |r| r.get(0))
            .expect("seeded path");
        std::path::PathBuf::from(p)
    };
    std::fs::create_dir_all(root.join("docs")).expect("workspace dir");
    std::fs::write(root.join("docs/report.pdf"), b"%PDF-1.7\n").expect("file");
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let post = |body: serde_json::Value| dispatch_post("/cli/thread/post", &HashMap::new(), body.to_string().as_bytes());
    let stored = || -> Vec<serde_json::Value> {
        let get = dispatch("/cli/thread", &params_of(&[("addr", handle.as_str())])).expect("GET thread");
        assert_eq!(get.status, "200 OK", "{}", get.body);
        json_body(&get)["items"].as_array().expect("items").clone()
    };

    // The box's send: no `text` field at all, one file.
    let ok = post(serde_json::json!({
        "addr": handle, "via": "compose",
        "attachments": [{"path": "docs/report.pdf"}],
    }));
    assert_eq!(ok.status, "200 OK", "{}", ok.body);
    let posted = json_body(&ok);
    let atts = posted["attachments"].as_array().unwrap_or_else(|| panic!("attachments in the reply: {posted}"));
    assert_eq!(atts.len(), 1, "{posted}");
    assert_eq!(atts[0]["path"], "docs/report.pdf", "{posted}");
    let items = stored();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["doc"]["body"], "", "the body is empty, not invented: {items:?}");
    assert_eq!(items[0]["doc"]["attachments"].as_array().map(Vec::len), Some(1), "{items:?}");

    // An empty `text` with the file: the same.
    let ok = post(serde_json::json!({
        "addr": handle, "text": "", "via": "compose",
        "attachments": [{"path": "docs/report.pdf"}],
    }));
    assert_eq!(ok.status, "200 OK", "{}", ok.body);
    assert_eq!(stored().len(), 2);

    // Neither words nor files (an empty list counts as none): usage 400,
    // nothing stored.
    for body in [
        serde_json::json!({"addr": handle, "text": "", "via": "compose"}),
        serde_json::json!({"addr": handle, "via": "compose"}),
        serde_json::json!({"addr": handle, "text": "", "via": "compose", "attachments": []}),
    ] {
        let r = post(body.clone());
        assert_eq!(r.status, "400 Bad Request", "{body}: {}", r.body);
        let v = json_body(&r);
        assert_eq!(v["error"]["code"], "usage", "{body}: {v}");
    }
    assert_eq!(stored().len(), 2, "a refused post stores nothing");
}
