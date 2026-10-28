//! Blob route tests (prd-garden-board-needs-v1 §14 "k2-daemon"): through
//! `zen_data_routes::handle`, so every gate runs. Temp HOME + scoped DB.

use std::collections::HashMap;

use base64::Engine as _;
use serde_json::{json, Value};

use k2_core::garden_data::blobs::{ref_of, sha256_hex};
use k2_core::garden_data::MadeBy;

use crate::garden_data::agent_routes::tests::{agent, env, err, ok, person_grant};
use crate::garden_data::call::door_tests::{builder, door, set_caps, Door, APP};
use crate::zen_data_routes::{handle, is_agent_route, DataWho};

/// A real PNG of `w`×`h` (for the thumbnail path).
fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 40, 40]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("encode png");
    out
}

#[test]
fn thumbnails_are_sniffed_cached_capped_and_cost_more() {
    let d = door(&["store:read", "store:write"]);
    d.stamp(MadeBy::Owner);
    // Review #2: bytes K2 can't tell, declared as HEIC, are stored as
    // octet-stream and never reach sips / ImageIO.
    let tiff = d.upload(b"II*\0\x08\0\0\0not a heic at all");
    assert_eq!(tiff["mime"], "application/octet-stream");
    let (code, v) = d.blob_get(tiff["ref"].as_str().unwrap(), &[("px", "256")]);
    assert_eq!((code, v["error"]["code"].as_str()), (400, Some("bad_request")), "{v}");
    // Review #3: at most MAX_THUMB_DECODES at once; more answer busy.
    let pic = d.upload(&png(600, 400));
    assert_eq!(pic["mime"], "image/png");
    let r = pic["ref"].as_str().unwrap().to_string();
    {
        let _a = super::DecodeSlot::take().expect("slot 1");
        let _b = super::DecodeSlot::take().expect("slot 2");
        let (code, v) = d.blob_get(&r, &[("px", "256")]);
        assert_eq!((code, v["error"]["code"].as_str()), (429, Some("busy")), "{v}");
    }
    let (code, t) = d.blob_get(&r, &[("px", "256")]);
    assert_eq!(code, 200, "{t}");
    assert_eq!((t["px"].as_u64(), t["eof"].as_bool()), (Some(256), Some(true)));
    // Cached by (hash, px): served even while every decode slot is taken.
    {
        let _a = super::DecodeSlot::take().expect("slot 1");
        let _b = super::DecodeSlot::take().expect("slot 2");
        let (code, again) = d.blob_get(&r, &[("px", "256")]);
        assert_eq!(code, 200, "{again}");
        assert_eq!(again["base64"], t["base64"]);
    }
    // A thumbnail read costs THUMB_READ_COST reads of the 120 burst.
    let mut b = super::budget::Buckets::default();
    let key = super::budget::BudgetKey::Placement { garden: "g".into(), placement: "p".into() };
    for _ in 0..12 {
        b.spend(&key, super::budget::Spend::Reads(k2_core::garden_data::blobs::THUMB_READ_COST), 0.0).expect("within the burst");
    }
    assert_eq!(b.spend(&key, super::budget::Spend::Reads(10), 0.0).unwrap_err().code(), "rate_limited");
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn unb64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).expect("base64")
}

fn status(r: &crate::cli_response::CliResponse) -> u16 {
    r.status.split(' ').next().unwrap().parse().unwrap()
}

impl Door {
    fn blob_put(&self, extra: Value, head: &str) -> (u16, Value) {
        let mut body = json!({ "garden": self.garden, "placement": "b1", "hash": self.hash() });
        for (k, v) in extra.as_object().expect("object") {
            body[k] = v.clone();
        }
        let r = handle("/cli/zen/blob/put", DataWho::Owner, body.to_string().as_bytes(), &HashMap::new(), head);
        (status(&r), serde_json::from_str(&r.body).expect("json"))
    }

    fn blob_get(&self, reference: &str, extra: &[(&str, &str)]) -> (u16, Value) {
        let mut p: HashMap<String, String> = [("garden", self.garden.as_str()), ("placement", "b1"), ("ref", reference)]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        p.insert("hash".into(), self.hash());
        for (k, v) in extra {
            p.insert(k.to_string(), v.to_string());
        }
        let r = handle("/cli/zen/blob/get", DataWho::Owner, b"", &p, APP);
        (status(&r), serde_json::from_str(&r.body).expect("json"))
    }

    fn upload(&self, bytes: &[u8]) -> Value {
        let (code, v) = self.blob_put(json!({ "base64": b64(bytes), "last": true, "name": "shot.png" }), APP);
        assert_eq!(code, 200, "{v}");
        v
    }
}

#[test]
fn the_widget_door_uploads_reads_and_names_blobs() {
    let d = door(&["store:read", "store:write"]);
    d.stamp(MadeBy::Owner);
    let png = b"\x89PNG\r\n\x1a\n-pretend-pixels-".to_vec();
    let info = d.upload(&png);
    let r = ref_of(&sha256_hex(&png));
    assert_eq!(info, json!({ "ref": r, "size": png.len(), "mime": "image/png", "name": "shot.png", "new": true }));
    // Ranged read back, exactly.
    let (code, v) = d.blob_get(&r, &[("offset", "8"), ("len", "7")]);
    assert_eq!(code, 200, "{v}");
    assert_eq!((v["offset"].as_u64(), v["len"].as_u64(), v["eof"].as_bool()), (Some(8), Some(7), Some(false)));
    assert_eq!(unb64(v["base64"].as_str().unwrap()), png[8..15].to_vec());
    let (_, all) = d.blob_get(&r, &[]);
    assert_eq!(unb64(all["base64"].as_str().unwrap()), png);
    assert_eq!(all["eof"], true);
    // A key names it; blobs.info and blobs.list see the hold.
    d.call("store.put", json!(["board/b1/obj/1", { "src": r }]));
    let i = d.call("blobs.info", json!([r]));
    assert_eq!((i["refs"].as_i64(), i["pending"].as_bool(), i["addedBy"].as_str()), (Some(1), Some(false), Some("you via board")), "{i}");
    let l = d.call("blobs.list", json!([{}]));
    assert_eq!(l["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(l["bytes"].as_u64(), Some(png.len() as u64));
    // A ref to bytes this space never received: blob_missing, nothing written.
    let stranger = ref_of(&sha256_hex(b"never uploaded"));
    assert_eq!(d.refuse("store.put", json!(["board/b1/obj/2", { "src": stranger }])), "blob_missing");
    assert_eq!(d.call("store.get", json!(["board/b1/obj/2"])), Value::Null);
    // px is for images only, and only 256 or 512.
    let (code, v) = d.blob_get(&r, &[("px", "300")]);
    assert_eq!((code, v["error"]["code"].as_str()), (400, Some("bad_request")));
    let text = d.upload(b"plain words");
    let (code, v) = d.blob_get(text["ref"].as_str().unwrap(), &[("px", "256")]);
    assert_eq!((code, v["error"]["code"].as_str()), (400, Some("bad_request")), "{v}");
}

#[test]
fn chunked_uploads_must_follow_on_exactly() {
    let d = door(&["store:read", "store:write"]);
    d.stamp(MadeBy::Owner);
    // Security review #8: every chunk but the last carries at least 1 MiB.
    let (code, v) = d.blob_put(json!({ "base64": b64(b"tiny"), "offset": 0 }), APP);
    assert_eq!((code, v["error"]["code"].as_str()), (400, Some("bad_request")), "a 1-byte trickle can't hold a slot");
    let mib = 1024 * 1024;
    let head = vec![b'h'; mib];
    let (code, first) = d.blob_put(json!({ "base64": b64(&head), "offset": 0 }), APP);
    assert_eq!(code, 200, "{first}");
    let up = first["upload"].as_str().expect("an upload id").to_string();
    assert_eq!(first["received"], mib);
    // A gap or an overlap is refused and adds nothing.
    let (code, v) = d.blob_put(json!({ "upload": up, "offset": mib - 1, "base64": b64(b"world"), "last": true }), APP);
    assert_eq!((code, v["error"]["code"].as_str()), (400, Some("bad_request")));
    let (code, done) = d.blob_put(json!({ "upload": up, "offset": mib, "base64": b64(b"world"), "last": true, "mime": "image/heic" }), APP);
    assert_eq!(code, 200, "{done}");
    let mut all = head.clone();
    all.extend_from_slice(b"world");
    assert_eq!(done["ref"], json!(ref_of(&sha256_hex(&all))));
    assert_eq!(done["mime"], "text/plain", "a declared mime is ignored (review #2)");
    // The upload is closed now.
    let (code, _) = d.blob_put(json!({ "upload": up, "offset": mib + 5, "base64": b64(b"!"), "last": true }), APP);
    assert_eq!(code, 400);
    // A new upload must start at 0.
    let (code, _) = d.blob_put(json!({ "offset": 3, "base64": b64(b"x"), "last": true }), APP);
    assert_eq!(code, 400);
}

#[test]
fn the_widget_door_runs_the_call_gates() {
    let d = door(&["store:read"]);
    d.stamp(MadeBy::Owner);
    // Not the owner token.
    let body = json!({ "garden": d.garden, "placement": "b1", "hash": d.hash(), "base64": b64(b"x"), "last": true });
    let r = handle("/cli/zen/blob/put", agent("w-builder"), body.to_string().as_bytes(), &HashMap::new(), APP);
    assert_eq!(status(&r), 403);
    assert!(r.body.contains("owner_only"), "{}", r.body);
    // A web page's Origin.
    let evil = "POST /cli/zen/blob/put HTTP/1.1\r\nOrigin: https://evil.example\r\n\r\n";
    let (code, v) = d.blob_put(json!({ "base64": b64(b"x"), "last": true }), evil);
    assert_eq!((code, v["error"]["code"].as_str()), (403, Some("origin_refused")));
    // The manifest has no store:write.
    let (code, v) = d.blob_put(json!({ "base64": b64(b"x"), "last": true }), APP);
    assert_eq!((code, v["error"]["code"].as_str()), (403, Some("cap_not_granted")), "{v}");
    // A stale frame (the widget changed since it loaded).
    set_caps(&d.f, &["store:read", "store:write"]);
    d.stamp(MadeBy::Owner);
    let body = json!({ "garden": d.garden, "placement": "b1", "hash": "0".repeat(64), "base64": b64(b"x"), "last": true });
    let r = handle("/cli/zen/blob/put", DataWho::Owner, body.to_string().as_bytes(), &HashMap::new(), APP);
    assert_eq!(status(&r), 409);
    assert!(r.body.contains("stale_widget"), "{}", r.body);
    // Unknown fields are refused (bodies are strict).
    let (code, _) = d.blob_put(json!({ "base64": b64(b"x"), "last": true, "path": "/etc/passwd" }), APP);
    assert_eq!(code, 400);
    // Not base64.
    let (code, _) = d.blob_put(json!({ "base64": "%%%", "last": true }), APP);
    assert_eq!(code, 400);
    // The widget door never reaches the cell socket.
    assert!(!is_agent_route("/cli/zen/blob/put") && !is_agent_route("/cli/zen/blob/get"));
    assert!(is_agent_route("/cli/zen/data/blob/put"));
}

#[test]
fn an_agent_made_widget_reads_only_blobs_its_makers_grants_reach() {
    let d = door(&["store:read", "store:write"]);
    d.stamp(builder());
    person_grant("board", "w-builder", "write", "board/");
    let open = d.upload(b"under board");
    d.call("store.put", json!(["board/b1/img", open["ref"]]));
    // The person's own image under secret/, uploaded outside the widget.
    let secret = ok(
        &DataWho::Owner,
        "POST",
        "/cli/zen/data/blob/put",
        json!({ "widget": "board", "base64": b64(b"the person's"), "last": true }),
        &[],
    );
    ok(&DataWho::Owner, "POST", "/cli/zen/data/put", json!({ "widget": "board", "key": "secret/x", "value": secret["ref"] }), &[]);
    let (code, _) = d.blob_get(open["ref"].as_str().unwrap(), &[]);
    assert_eq!(code, 200);
    let (code, v) = d.blob_get(secret["ref"].as_str().unwrap(), &[]);
    assert_eq!((code, v["error"]["code"].as_str()), (403, Some("not_granted")), "{v}");
    assert!(v["error"].get("key").is_none(), "review #6: a refusal never names a key outside the grant: {v}");
    let l = d.call("blobs.list", json!([]));
    assert_eq!(l["items"].as_array().map(Vec::len), Some(1), "the list is filtered to the maker's prefixes: {l}");
    assert_eq!(d.refuse("blobs.info", json!([secret["ref"]])), "not_granted");
    // Its own pending upload (no key yet) it may read back.
    let pending = d.upload(b"just pasted");
    let (code, _) = d.blob_get(pending["ref"].as_str().unwrap(), &[]);
    assert_eq!(code, 200);
}

#[test]
fn the_agent_door_round_trips_bytes_under_grants() {
    let _e = env();
    let o = DataWho::Owner;
    let cut = 1024 * 1024;
    let bytes: Vec<u8> = (0..=255u8).cycle().take(cut + 70_000).collect();
    // Owner: chunked put, ranged get, exact bytes back.
    let first = ok(&o, "POST", "/cli/zen/data/blob/put", json!({ "widget": "board", "base64": b64(&bytes[..cut]) }), &[]);
    let up = first["upload"].as_str().expect("upload").to_string();
    let done = ok(
        &o,
        "POST",
        "/cli/zen/data/blob/put",
        json!({ "widget": "board", "upload": up, "offset": cut, "base64": b64(&bytes[cut..]), "last": true, "name": "data.bin" }),
        &[],
    );
    let r = done["ref"].as_str().expect("ref").to_string();
    assert_eq!(r, ref_of(&sha256_hex(&bytes)));
    let mut back = Vec::new();
    let mut offset = 0usize;
    loop {
        let off = offset.to_string();
        let v = ok(&o, "GET", "/cli/zen/data/blob/get", json!({}), &[("widget", "board"), ("ref", &r), ("offset", &off), ("len", "300000")]);
        let chunk = unb64(v["base64"].as_str().unwrap());
        offset += chunk.len();
        back.extend(chunk);
        if v["eof"] == true {
            break;
        }
    }
    assert_eq!(sha256_hex(&back), sha256_hex(&bytes), "sha256 of the bytes read back");
    ok(&o, "POST", "/cli/zen/data/put", json!({ "widget": "board", "key": "board/b1/obj", "value": { "src": r } }), &[]);
    // An agent with no grant can't upload, read or list.
    let a = agent("w-keeper");
    let (code, c) = err(&a, "POST", "/cli/zen/data/blob/put", json!({ "widget": "board", "base64": b64(b"x"), "last": true }), &[]);
    assert_eq!((code, c.as_str()), (403, "not_granted"));
    let (code, c) = err(&a, "GET", "/cli/zen/data/blob/get", json!({}), &[("widget", "board"), ("ref", &r)]);
    assert_eq!((code, c.as_str()), (403, "not_granted"));
    let (code, _) = err(&a, "GET", "/cli/zen/data/blob/list", json!({}), &[("widget", "board")]);
    assert_eq!(code, 403);
    // A read grant on another prefix still can't read it; one on board/ can.
    person_grant("board", "w-keeper", "read", "notes/");
    let (code, _) = err(&a, "GET", "/cli/zen/data/blob/get", json!({}), &[("widget", "board"), ("ref", &r)]);
    assert_eq!(code, 403);
    let l = ok(&a, "GET", "/cli/zen/data/blob/list", json!({}), &[("widget", "board")]);
    assert_eq!(l["items"].as_array().map(Vec::len), Some(0));
    person_grant("board", "w-keeper", "read", "board/");
    let v = ok(&a, "GET", "/cli/zen/data/blob/get", json!({}), &[("widget", "board"), ("ref", &r), ("len", "10")]);
    assert_eq!(unb64(v["base64"].as_str().unwrap()), bytes[..10].to_vec());
    // A write grant lets it upload; its own pending upload is readable.
    person_grant("board", "w-keeper", "write", "board/b2/");
    let mine = ok(&a, "POST", "/cli/zen/data/blob/put", json!({ "widget": "board", "base64": b64(b"agent shot"), "last": true }), &[]);
    let v = ok(&a, "GET", "/cli/zen/data/blob/get", json!({}), &[("widget", "board"), ("ref", mine["ref"].as_str().unwrap())]);
    assert_eq!(unb64(v["base64"].as_str().unwrap()), b"agent shot".to_vec());
    // A bad ref is a bad request, never a path.
    let (code, _) = err(&o, "GET", "/cli/zen/data/blob/get", json!({}), &[("widget", "board"), ("ref", "k2blob:sha256-../../x")]);
    assert_eq!(code, 400);
    // A missing blob.
    let ghost = ref_of(&sha256_hex(b"ghost"));
    let (code, c) = err(&o, "GET", "/cli/zen/data/blob/get", json!({}), &[("widget", "board"), ("ref", &ghost)]);
    assert_eq!((code, c.as_str()), (409, "blob_missing"));
}

// ── ingest (0.46.6, prd-garden-board-needs-v1 GP3/GP4) ─────────────────

impl Door {
    fn ingest(&self, paths: &[String], accept: &[&str], head: &str) -> (u16, Value) {
        let body = json!({ "garden": self.garden, "placement": "b1", "hash": self.hash(), "paths": paths, "accept": accept });
        let r = handle("/cli/zen/blob/ingest", DataWho::Owner, body.to_string().as_bytes(), &HashMap::new(), head);
        (status(&r), serde_json::from_str(&r.body).expect("json"))
    }
}

#[test]
fn picked_and_dropped_files_become_the_widgets_blobs() {
    let d = door(&["store:read", "store:write", "uploads:pick"]);
    d.stamp(MadeBy::Owner);
    let dir = k2_core::paths::k2_home().join("picked");
    std::fs::create_dir_all(&dir).unwrap();
    let pic = dir.join("Photo One.png");
    let png = png(8, 8);
    std::fs::write(&pic, &png).unwrap();
    let doc = dir.join("notes.json");
    std::fs::write(&doc, b"{\"a\":1}").unwrap();
    let paths = |ps: &[&std::path::Path]| -> Vec<String> { ps.iter().map(|p| p.to_string_lossy().to_string()).collect() };
    // Images only: a JSON file in the pick refuses the whole pick.
    let (code, v) = d.ingest(&paths(&[&pic, &doc]), &["image/*"], APP);
    assert_eq!((code, v["error"]["code"].as_str()), (400, Some("bad_request")), "{v}");
    assert_eq!(d.call("blobs.list", json!([{}]))["items"].as_array().map(Vec::len), Some(0), "nothing kept");
    // Each file by its .ext or its sniffed type; the answer names no path.
    let (code, v) = d.ingest(&paths(&[&pic, &doc]), &["image/*", ".json"], APP);
    assert_eq!(code, 200, "{v}");
    let files = v["files"].as_array().expect("files");
    assert_eq!(files[0], json!({ "ref": ref_of(&sha256_hex(&png)), "name": "Photo One.png", "size": png.len(), "mime": "image/png" }));
    assert_eq!((files[1]["name"].as_str(), files[1]["mime"].as_str()), (Some("notes.json"), Some("application/json")));
    assert!(!v.to_string().contains(dir.to_string_lossy().as_ref()), "never a path back: {v}");
    // Pending until a key names them (GB10).
    let i = d.call("blobs.info", json!([files[0]["ref"]]));
    assert_eq!(i["pending"], true);
    // A drop takes any kind (no accept).
    let (code, _) = d.ingest(&paths(&[&doc]), &[], APP);
    assert_eq!(code, 200);
    // Gates: a web page's Origin, a folder, a relative path, a bad accept.
    let evil = "POST /cli/zen/blob/ingest HTTP/1.1\r\nOrigin: https://evil.example\r\n\r\n";
    let (code, v) = d.ingest(&paths(&[&pic]), &[], evil);
    assert_eq!((code, v["error"]["code"].as_str()), (403, Some("origin_refused")));
    let (code, _) = d.ingest(&paths(&[&dir]), &[], APP);
    assert_eq!(code, 400, "a folder isn't a file");
    let (code, _) = d.ingest(&["notes.json".to_string()], &[], APP);
    assert_eq!(code, 400, "only absolute paths");
    let (code, _) = d.ingest(&paths(&[&pic]), &["image"], APP);
    assert_eq!(code, 400, "accept is .ext or a mime pattern");
    let many: Vec<String> = (0..21).map(|_| pic.to_string_lossy().to_string()).collect();
    let (code, _) = d.ingest(&many, &[], APP);
    assert_eq!(code, 400, "at most 20 files");
    // Without uploads:pick in the manifest.
    crate::garden_data::call::door_tests::set_caps(&d.f, &["store:read", "store:write"]);
    d.stamp(MadeBy::Owner);
    let (code, v) = d.ingest(&paths(&[&pic]), &[], APP);
    assert_eq!((code, v["error"]["code"].as_str()), (403, Some("cap_not_granted")), "{v}");
}

#[test]
fn accept_entries_match_by_extension_or_sniffed_type() {
    let a = |list: &[&str]| -> Vec<String> { list.iter().map(|s| s.to_string()).collect() };
    assert!(super::accepts(&a(&[]), "x.bin", "application/octet-stream"));
    assert!(super::accepts(&a(&["image/*"]), "x", "image/webp"));
    assert!(super::accepts(&a(&[".PDF"]), "Report.pdf", "application/pdf"));
    assert!(super::accepts(&a(&["application/json"]), "a.txt", "application/json"));
    assert!(!super::accepts(&a(&["image/*"]), "evil.png", "text/plain"), "the sniffed type decides for a mime pattern");
    for ok in [".png", "image/*", "image/svg+xml", "text/plain"] {
        assert!(super::valid_accept(ok), "{ok}");
    }
    for bad in ["png", ".", "image", "*/*x", ".p-g", "image/<script>"] {
        assert!(!super::valid_accept(bad), "{bad}");
    }
}
