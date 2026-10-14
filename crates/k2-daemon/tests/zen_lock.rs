//! Garden Lock (prd-garden-lock-and-sync-v1, with Rosson's rulings of
//! 2026-10-13).
//!
//! Drives `k2_core::zen` on temp folders, the REAL `k2-daemon` binary
//! under a temp HOME, and the in-process dispatcher for session passports.
//! Nothing here touches the real `~/.k2`; no test spawns an agent CLI.
//!
//! Fail loudly: every assertion names what it saw.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use k2_core::zen::defaults::{Defaults, DefaultsSet};
use k2_core::zen::lock::{LOCKED_GARDEN_FILE, REFUSED_DIR};
use k2_core::zen::sync::{Mode, Part, Parts};
use k2_core::zen::{self as zen, ZenError, ZenFile, ZenFiles};
use serde_json::{json, Value as J};

// ── helpers ────────────────────────────────────────────────────────────

struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(tag: &str) -> (TempRoot, PathBuf) {
    let dir = std::env::temp_dir().join(format!("k2-zen-lock-{tag}-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let zen = dir.join("zen");
    (TempRoot(dir), zen)
}

fn set_up(tag: &str) -> (TempRoot, ZenFiles) {
    let (t, z) = temp_dir(tag);
    let f = ZenFiles::new(&z);
    f.setup().expect("setup");
    (t, f)
}

fn gid(f: &ZenFiles, name: &str) -> String {
    f.find_garden(name).unwrap_or_else(|| panic!("no Garden {name}: {:?}", f.gardens())).1.id
}

fn garden_path(f: &ZenFiles, id: &str) -> PathBuf {
    f.path_of(&ZenFile::Garden(id.to_string()))
}

fn write_garden(f: &ZenFiles, id: &str, text: &str) {
    let p = garden_path(f, id);
    std::fs::write(&p, text).unwrap_or_else(|e| panic!("write {}: {e}", p.display()));
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn resolve(f: &ZenFiles, id: &str) -> J {
    f.resolve(Some(id)).unwrap_or_else(|e| panic!("resolve {id}: {e}"))
}

/// A theme table that restyles one Garden: its accent.
fn accented(accent: &str) -> String {
    format!("schema = 1\ntemplate = \"k2.texting@1\"\n\n[colors.light]\naccent = \"{accent}\"\n")
}

fn accent(v: &J) -> String {
    v["theme"]["tokens"]["colors"]["light"]["accent"].as_str().unwrap_or_else(|| panic!("no light accent: {v}")).to_string()
}

/// One custom widget in a blank Garden.
fn custom_page(widget: &str) -> String {
    format!(
        "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n\
         [[widget]]\nid = \"w\"\nkind = \"custom\"\nwidget = \"{widget}\"\ncolumn = 0\n"
    )
}

fn custom_hash(v: &J) -> String {
    let w = v["page"]["widgets"].as_array().expect("widgets").iter().find(|w| w["kind"] == "custom").cloned();
    w.unwrap_or_else(|| panic!("no custom widget: {v}"))["hash"].as_str().expect("hash").to_string()
}

fn edit_widget_js(f: &ZenFiles, name: &str, line: &str) {
    let p = f.widget_dir(name).join("arcade.js");
    let mut text = read(&p);
    text.push_str(line);
    std::fs::write(&p, text).expect("edit widget js");
}

/// A fake set: this binary's, edited, with its fingerprint recomputed.
fn fake_set(edit: impl FnOnce(&mut DefaultsSet)) -> Defaults {
    let mut s = DefaultsSet::from_binary();
    edit(&mut s);
    s.fingerprint = s.compute_fingerprint().expect("fake set fingerprint");
    Defaults::from_set(s).unwrap_or_else(|e| panic!("fake set must load: {e}"))
}

/// "The next release": texting's columns move.
fn next_release() -> Defaults {
    fake_set(|s| {
        let t = s.templates.get_mut(zen::TEMPLATE_ID).expect("texting in the set");
        assert!(t.contains("size = 34") && t.contains("size = 66"), "fixture: texting columns");
        *t = t.replacen("size = 34", "size = 30", 1).replacen("size = 66", "size = 70", 1);
    })
}

// ── core: state, upgrade, freeze ────────────────────────────────────────

/// "Upgrade leaves all Gardens unlocked": a `sync.json` from before Lock
/// (no `locked` key) reads as unlocked, and so does a computer with no
/// sync state at all (the upgrade pass).
#[test]
fn upgrade_leaves_every_garden_unlocked() {
    let (_t, f) = set_up("upgrade");
    for g in f.gardens() {
        assert!(!f.is_locked(&g.id), "{} starts unlocked", g.name);
    }
    // 0.45.4's sync.json shape, no `locked`.
    let g1 = gid(&f, "Garden 1");
    let old = json!({
        "version": 1, "migratedAt": "2026-10-08T00:00:00Z", "liveDefaults": Defaults::live().fingerprint(),
        "gardens": { g1.clone(): { "page": { "mode": "synced" }, "theme": { "mode": "synced" } } }
    });
    std::fs::write(f.sync_path(), serde_json::to_string_pretty(&old).expect("json")).expect("write old sync.json");
    f.forget_boot();
    assert!(!f.is_locked(&g1), "an old entry has no lock");
    let list = f.sync_list().expect("sync list");
    for row in list["gardens"].as_array().expect("rows") {
        assert_eq!(row["locked"], false, "every row unlocked: {row}");
    }
    for g in f.gardens_json() {
        assert_eq!(g["locked"], false, "gardens_json: {g}");
    }
    // Missing sync state (the upgrade pass) too.
    std::fs::remove_file(f.sync_path()).expect("rm sync.json");
    f.forget_boot();
    f.sync_boot();
    for g in f.gardens() {
        assert!(!f.is_locked(&g.id), "after the upgrade pass {} is unlocked", g.name);
    }
    assert!(!f.locks_dir().exists(), "no .locks/ until something is locked");
}

#[test]
fn lock_freezes_the_live_version_and_shows_everywhere() {
    let (_t, f) = set_up("freeze");
    let g = gid(&f, "Garden 1");
    write_garden(&f, &g, &accented("#111111"));
    f.refresh().expect("refresh");
    let out = f.set_garden_lock(&g, true).expect("lock");
    assert!(out.locked && out.changed, "{out:?}");
    assert_eq!(read(&f.lock_dir(&g).join(LOCKED_GARDEN_FILE)), accented("#111111"), "the frozen copy is the live text");
    let sf = f.sync_state();
    assert!(sf.garden(&g).locked && sf.garden(&g).locked_at.is_some(), "sync.json: {:?}", sf.garden(&g));
    let mirror: J = serde_json::from_str(&read(&f.mirror_path(&g))).expect("mirror json");
    assert_eq!(mirror["locked"], true, "the mirror carries the lock: {mirror}");
    let row = f.sync_list().expect("list")["gardens"].as_array().expect("rows").iter().find(|r| r["id"] == g.as_str()).cloned().expect("row");
    assert_eq!(row["locked"], true, "{row}");
    assert!(f.gardens_json().iter().any(|x| x["id"] == g.as_str() && x["locked"] == true));
    // Locking takes the LIVE version: a broken file on disk freezes its
    // last good snapshot, and the next refresh puts that back.
    let g2 = gid(&f, "Garden 2");
    write_garden(&f, &g2, &accented("#222222").replace("template = \"k2.texting@1\"\n", ""));
    f.refresh().expect("refresh");
    write_garden(&f, &g2, "schema = 1\n[colors.light]\nacent = \"#fff\"\n");
    f.set_garden_lock(&g2, true).expect("lock g2");
    let frozen = read(&f.lock_dir(&g2).join(LOCKED_GARDEN_FILE));
    assert!(frozen.contains("#222222"), "froze the last good, not the broken edit: {frozen}");
    let r = f.enforce_locks();
    assert_eq!(r.iter().map(|x| x.garden.as_str()).collect::<Vec<_>>(), vec![g2.as_str()], "{r:?}");
    assert_eq!(read(&garden_path(&f, &g2)), frozen);
}

/// §5.4: an edit to a locked Garden's file never goes live; the next
/// refresh moves it into history and writes the locked text back.
#[test]
fn a_raw_edit_to_a_locked_garden_is_put_back_and_kept() {
    let (_t, f) = set_up("raw-edit");
    let g = gid(&f, "Garden 1");
    write_garden(&f, &g, &accented("#111111"));
    f.refresh().expect("refresh");
    f.set_garden_lock(&g, true).expect("lock");
    let before = resolve(&f, &g);
    let edit = accented("#444444");
    write_garden(&f, &g, &edit);
    // Before the watcher fires: the live page is still the locked one.
    assert_eq!(accent(&resolve(&f, &g)), "#111111", "an edit never shows, even before refresh");
    let v = f.validate(Some(&ZenFile::Garden(g.clone()))).expect("validate");
    assert_eq!(v["locked"]["garden"], g.as_str(), "validate reports the lock: {v}");
    assert!(v["locked"]["message"].as_str().expect("message").contains("is locked. Agents can't change it. Ask the person"), "{v}");
    let (_, refused) = f.refresh_with_report(&zen::widget_access::WidgetPauses::empty()).expect("refresh");
    assert_eq!(refused.len(), 1, "one refusal: {refused:?}");
    assert_eq!(refused[0].garden, g);
    let kept = refused[0].kept.clone().expect("the edit was kept");
    assert!(kept.starts_with(&format!(".history/gardens/{g}.toml/{REFUSED_DIR}/")), "{kept}");
    assert_eq!(read(&f.root().join(&kept)), edit, "nothing is lost: the edit is in history");
    assert_eq!(read(&garden_path(&f, &g)), accented("#111111"), "the locked text is back on disk");
    assert_eq!(accent(&resolve(&f, &g)), "#111111");
    assert_eq!(resolve(&f, &g)["page"], before["page"]);
    assert_eq!(f.refused_edits(&g).len(), 1);
    assert!(f.enforce_locks().is_empty(), "a second refresh has nothing to put back");
    // A deleted file comes back too.
    std::fs::remove_file(garden_path(&f, &g)).expect("rm");
    let r = f.enforce_locks();
    assert_eq!((r.len(), r[0].kept.clone()), (1, None), "{r:?}");
    assert_eq!(read(&garden_path(&f, &g)), accented("#111111"));
    // The refused edit never became a snapshot (snapshots are the live
    // version's fallbacks).
    let gf = ZenFile::Garden(g.clone());
    for s in f.snapshots(&gf) {
        assert!(!s.name.contains("refused"), "{s:?}");
    }
    // Unlocked, an edit goes live as before.
    f.set_garden_lock(&g, false).expect("unlock");
    assert!(!f.lock_dir(&g).exists(), "unlock removes the frozen copy");
    write_garden(&f, &g, &accented("#444444"));
    f.refresh().expect("refresh");
    let v = resolve(&f, &g);
    assert_eq!(v["errors"], json!([]), "the edit is clean, so its refusal above meant the lock: {v}");
    assert_eq!(accent(&v), "#444444", "unlocked edits go live");
    assert_eq!(read(&garden_path(&f, &g)), accented("#444444"));
}

/// Ruling 1: the person (the app, owner token) still renames, rethemes,
/// resets, starts over and deletes a locked Garden; the lock then holds
/// the person's new text.
#[test]
fn the_person_still_changes_a_locked_garden() {
    let (_t, f) = set_up("owner");
    let g = gid(&f, "Garden 1");
    write_garden(&f, &g, &accented("#111111"));
    f.refresh().expect("refresh");
    f.set_garden_lock(&g, true).expect("lock");
    let (e, renamed) = f.rename_garden(&g, "Morning").expect("rename");
    assert!(renamed && e.name == "Morning");
    f.set_theme(Some("paper"), Some(&g)).expect("retheme");
    assert_eq!(resolve(&f, &g)["theme"]["name"], "paper");
    let out = f.reset(&ZenFile::Garden(g.clone()), None).expect("reset");
    assert_eq!(out.restored, "default");
    let stub = read(&garden_path(&f, &g));
    assert!(!stub.contains("#111111"), "reset to the stub: {stub}");
    assert_eq!(read(&f.lock_dir(&g).join(LOCKED_GARDEN_FILE)), stub, "the lock now holds the reset text");
    assert!(f.enforce_locks().is_empty(), "the person's reset is not put back");
    write_garden(&f, &g, &accented("#3b3b3b"));
    f.enforce_locks();
    assert_eq!(read(&garden_path(&f, &g)), stub, "still locked: a later edit goes back to the reset text");
    let t = f.set_garden_template(&g, "blank", true).expect("template");
    assert!(t.changed);
    let blank = read(&garden_path(&f, &g));
    assert_eq!(read(&f.lock_dir(&g).join(LOCKED_GARDEN_FILE)), blank, "Start over is the person's too");
    assert!(f.is_locked(&g), "still locked after all of it");
    let d = f.delete_garden(&g).expect("delete");
    assert_eq!(d.deleted.id, g);
    assert!(!f.lock_dir(&g).exists(), "the lock folder left .locks/");
    let hist = f.history_root().join("gardens").join(format!("{g}.toml"));
    assert!(hist.join("lock").join(LOCKED_GARDEN_FILE).is_file(), "it travels into history");
    let rec: J = serde_json::from_str(&read(&hist.join("deleted.json"))).expect("deleted.json");
    assert_eq!(rec["sync"]["locked"], true, "the delete record keeps the lock: {rec}");
}

/// Ruling 4: themes are not frozen. A global theme switch and an edit to
/// the theme a locked Garden uses both reach it.
#[test]
fn a_theme_change_still_reaches_a_locked_garden() {
    let (_t, f) = set_up("theme");
    let g = gid(&f, "Garden 1");
    f.new_theme("hearth", Some("basic")).expect("theme new");
    f.set_theme(Some("hearth"), None).expect("global hearth");
    f.refresh().expect("refresh");
    f.set_garden_lock(&g, true).expect("lock");
    let before = accent(&resolve(&f, &g));
    let p = f.path_of(&ZenFile::Theme("hearth".into()));
    let text = read(&p);
    let line = text.lines().find(|l| l.trim_start().starts_with("accent = ")).expect("an accent line in hearth").to_string();
    std::fs::write(&p, text.replacen(&line, "accent = \"#7c2d12\"", 1)).expect("edit theme");
    f.refresh().expect("refresh");
    let after = accent(&resolve(&f, &g));
    assert_ne!(after, before, "the theme edit reached the locked Garden");
    assert_eq!(after, "#7c2d12");
    f.set_theme(Some("midnight"), None).expect("global midnight");
    assert_eq!(resolve(&f, &g)["theme"]["name"], "midnight", "a global switch reaches it too");
    assert!(f.enforce_locks().is_empty(), "theme changes are never refused");
}

/// Ruling 2: locked + in sync gets K2's template updates; locked + its
/// own copy doesn't move.
#[test]
fn locked_and_synced_follows_k2_locked_and_copy_does_not() {
    let (_t, z) = temp_dir("sync");
    let f = ZenFiles::new(&z);
    f.setup().expect("setup");
    let synced = gid(&f, "Garden 1");
    let copy = f.new_garden("Copy", Some("texting"), None, None).expect("new").id;
    f.set_garden_sync(&copy, Parts::Both, false).expect("copy");
    f.refresh().expect("refresh");
    for id in [&synced, &copy] {
        f.set_garden_lock(id, true).expect("lock");
    }
    assert_eq!(resolve(&f, &synced)["page"]["layout"]["split"], json!([34, 66]));
    assert_eq!(resolve(&f, &copy)["page"]["layout"]["split"], json!([34, 66]));
    let f = ZenFiles::with_defaults(&z, next_release());
    f.sync_boot();
    assert!(f.is_locked(&synced) && f.is_locked(&copy), "a K2 update keeps the locks");
    assert_eq!(f.sync_state().garden(&synced).part(Part::Page).mode, Mode::Synced);
    assert_eq!(resolve(&f, &synced)["page"]["layout"]["split"], json!([30, 70]), "locked + synced: K2's update applies");
    assert_eq!(resolve(&f, &copy)["page"]["layout"]["split"], json!([34, 66]), "locked + own copy: nothing changes");
    assert!(f.enforce_locks().is_empty(), "a template update rewrites no file");
}

/// The one "Keep in sync" checkbox: both parts on, both off (`part: both`),
/// and a lock never changes it.
#[test]
fn keep_in_sync_sets_both_parts_and_lock_is_separate() {
    let (_t, f) = set_up("both");
    let g = gid(&f, "Garden 1");
    f.set_garden_sync(&g, Parts::Both, false).expect("off");
    let st = f.sync_state().garden(&g);
    assert_eq!((st.part(Part::Page).mode, st.part(Part::Theme).mode), (Mode::Copy, Mode::Copy));
    f.set_garden_lock(&g, true).expect("lock");
    let st = f.sync_state().garden(&g);
    assert_eq!((st.part(Part::Page).mode, st.part(Part::Theme).mode), (Mode::Copy, Mode::Copy), "lock keeps sync");
    f.set_garden_sync(&g, Parts::Both, true).expect("on");
    let st = f.sync_state().garden(&g);
    assert_eq!((st.part(Part::Page).mode, st.part(Part::Theme).mode), (Mode::Synced, Mode::Synced));
    assert!(st.locked, "sync keeps the lock");
    f.undo_garden_sync(&g).expect("undo");
    assert!(f.sync_state().garden(&g).locked, "undo keeps the lock");
}

/// Freeze scope: widget code. An edit to a shared widget folder reaches an
/// unlocked Garden but not the locked one, which keeps running its frozen
/// bundle even when the folder is gone.
#[test]
fn a_locked_garden_keeps_its_widget_code() {
    let (_t, f) = set_up("widgets");
    f.new_widget("agent-arcade", Some("arcade")).expect("widget new");
    let locked = gid(&f, "Garden 2");
    let open = f.new_garden("Open", None, None, None).expect("new").id;
    for id in [&locked, &open] {
        write_garden(&f, id, &custom_page("agent-arcade"));
    }
    f.refresh().expect("refresh");
    let h0 = custom_hash(&resolve(&f, &locked));
    assert_eq!(h0, custom_hash(&resolve(&f, &open)));
    let out = f.set_garden_lock(&locked, true).expect("lock");
    assert_eq!(out.widgets, vec!["agent-arcade".to_string()], "{out:?}");
    edit_widget_js(&f, "agent-arcade", "\n// a change an agent made\n");
    f.refresh().expect("refresh");
    let h_open = custom_hash(&resolve(&f, &open));
    assert_ne!(h_open, h0, "the unlocked Garden follows the edit");
    assert_eq!(custom_hash(&resolve(&f, &locked)), h0, "the locked Garden keeps its frozen code");
    let st = f.widget_bundle_in(Some(&locked), "agent-arcade").expect("bundle locked");
    assert_eq!(st.hash(), h0, "the bundle route serves the frozen copy");
    let st = f.widget_bundle_in(Some(&open), "agent-arcade").expect("bundle open");
    assert_eq!(st.hash(), h_open);
    assert_eq!(f.widget_bundle_in(None, "agent-arcade").expect("bundle").hash(), h_open, "no garden: the live folder");
    std::fs::remove_dir_all(f.widget_dir("agent-arcade")).expect("rm widget folder");
    f.refresh().expect("refresh");
    let v = resolve(&f, &locked);
    assert_eq!(v["errors"], json!([]), "the frozen copy keeps it whole: {v}");
    assert_eq!(custom_hash(&v), h0);
    assert!(!resolve(&f, &open)["errors"].as_array().expect("errors").is_empty(), "the unlocked one shows the missing folder");
    // Unlocked, it runs the live folder again (gone → broken).
    f.set_garden_lock(&locked, false).expect("unlock");
    assert!(f.frozen_widget(&locked, "agent-arcade").is_none());
}

#[test]
fn refuse_if_locked_names_the_garden() {
    let (_t, f) = set_up("refuse");
    let g = gid(&f, "Garden 1");
    f.refuse_if_locked(&g).expect("unlocked passes");
    f.set_garden_lock(&g, true).expect("lock");
    match f.refuse_if_locked("garden 1") {
        Err(ZenError::GardenLocked { garden, name }) => assert_eq!((garden.as_str(), name.as_str()), (g.as_str(), "Garden 1")),
        other => panic!("by name, case aside: {other:?}"),
    }
    let msg = ZenError::GardenLocked { garden: g.clone(), name: "Diary".into() }.to_string();
    assert_eq!(msg, "\u{201c}Diary\u{201d} is locked. Agents can't change it. Ask the person to unlock it in Settings \u{2192} Gardens.");
    f.refuse_if_locked("no-such-garden").expect("an unknown Garden is the route's own 404");
}

// ── headless: the real binary under a temp HOME ────────────────────────

struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Daemon {
    child: std::process::Child,
    port: u16,
    owner: String,
    /// Own TMPDIR / XDG_RUNTIME_DIR under the temp HOME (never the real
    /// runtime dir with the production frpc.toml); reaps this daemon's
    /// frpc after the child is killed (fields drop after `Drop::drop`).
    _rt: k2_core::test_env::DaemonRuntime,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?.trim().to_string();
    (!t.is_empty()).then_some(t)
}

fn spawn_daemon(home: &Path) -> Daemon {
    let port_file = home.join(".k2").join("daemon.port");
    let rt = k2_core::test_env::DaemonRuntime::new(home);
    let child = Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
        .env("HOME", home)
        .envs(rt.envs())
        .env("K2_TEST_AGENT_SHIM_DIR", home.join("agent-shim-empty"))
        .env("K2SO_WATCHDOG_DISABLED", "1")
        .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
        .env("K2_SUBSCRIPTION_PROBE", "deny")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn k2-daemon");
    let token_file = home.join(".k2").join("daemon.token");
    let deadline = Instant::now() + Duration::from_secs(20);
    let (port, owner) = loop {
        let port = read_trimmed(&port_file).and_then(|p| p.parse::<u16>().ok());
        if let (Some(port), Some(owner)) = (port, read_trimmed(&token_file)) {
            break (port, owner);
        }
        assert!(Instant::now() < deadline, "daemon never published daemon.port + daemon.token");
        std::thread::sleep(Duration::from_millis(50));
    };
    let d = Daemon { child, port, owner, _rt: rt };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(mut c) = Conn::try_open(d.port) {
            let (s, b) = c.request("GET", "/boot-status", None);
            if s == 200 && json(&b, "boot-status")["phase"] == "ready" {
                break;
            }
        }
        assert!(Instant::now() < deadline, "daemon never reached ready");
        std::thread::sleep(Duration::from_millis(100));
    }
    d
}

struct Conn {
    reader: BufReader<TcpStream>,
}

impl Conn {
    fn try_open(port: u16) -> std::io::Result<Conn> {
        let s = TcpStream::connect(("127.0.0.1", port))?;
        s.set_read_timeout(Some(Duration::from_secs(15)))?;
        Ok(Conn { reader: BufReader::new(s) })
    }

    fn request(&mut self, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        let body = body.unwrap_or("");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        self.reader.get_mut().write_all(head.as_bytes()).expect("write request");
        let mut status_line = String::new();
        self.reader.read_line(&mut status_line).expect("status line");
        let status: u16 = status_line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("{path}: {status_line:?}"));
        let mut len = None;
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("header");
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                if k.eq_ignore_ascii_case("content-length") {
                    len = Some(v.trim().parse::<usize>().expect("content-length"));
                }
            }
        }
        let mut buf = vec![0u8; len.unwrap_or_else(|| panic!("{path}: no Content-Length"))];
        self.reader.read_exact(&mut buf).expect("body");
        (status, String::from_utf8(buf).expect("utf-8"))
    }
}

fn json(body: &str, what: &str) -> J {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{what}: not JSON ({e}): {body}"))
}

fn call(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, J) {
    let (s, b) = Conn::try_open(port).expect("connect").request(method, path, body);
    (s, json(&b, path))
}

/// Headless (K2's daemon-first rule): the lock route, the listings, the
/// file restore with its audit line, and person-only refusals, all over
/// HTTP with no client attached.
#[test]
fn headless_lock_route_restore_and_audit() {
    let home = Home(std::env::temp_dir().join(format!("k2-zen-lock-headless-{}-{}", std::process::id(), uuid::Uuid::new_v4())));
    std::fs::create_dir_all(home.0.join(".k2")).expect("HOME/.k2");
    std::fs::create_dir_all(home.0.join("agent-shim-empty")).expect("shim");
    let d = spawn_daemon(&home.0);
    let (port, tok) = (d.port, d.owner.clone());

    let (s, b) = call(port, "GET", "/boot-status", None);
    assert_eq!(s, 200);
    assert!(b["features"].as_array().expect("features").iter().any(|x| x == "zen-garden-lock-v1"), "feature key: {b}");

    let (s, v) = call(port, "POST", &format!("/cli/zen/garden/lock?token={tok}"), Some(r#"{"garden":"g-x","locked":true}"#));
    assert_eq!((s, v["error"].as_str()), (404, Some("zen_not_set_up")), "before setup: {v}");
    let (s, v) = call(port, "POST", &format!("/cli/zen/setup?token={tok}"), Some("{}"));
    assert_eq!(s, 200, "setup: {v}");
    let g1 = v["gardens"][0]["id"].as_str().expect("Garden 1").to_string();
    assert_eq!(v["gardens"][0]["locked"], false, "a new Garden starts unlocked: {v}");

    // GET on the POST row → 405.
    let (s, b) = Conn::try_open(port).expect("connect").request("GET", &format!("/cli/zen/garden/lock?token={tok}"), None);
    assert_eq!(s, 405, "GET garden/lock: {b}");
    let (s, v) = call(port, "POST", &format!("/cli/zen/garden/lock?token={tok}"), Some(&format!(r#"{{"garden":"{g1}"}}"#)));
    assert_eq!(s, 400, "locked is required: {v}");

    let (s, v) = call(port, "POST", &format!("/cli/zen/garden/lock?token={tok}"), Some(&format!(r#"{{"garden":"{g1}","locked":true}}"#)));
    assert_eq!(s, 200, "lock: {v}");
    assert_eq!((v["locked"].as_bool(), v["changed"].as_bool()), (Some(true), Some(true)), "{v}");
    assert_eq!(v["garden"]["locked"], true, "the answer is the sync row: {v}");
    let (_, v) = call(port, "GET", &format!("/cli/zen/sync?token={tok}"), None);
    assert!(v["gardens"].as_array().expect("rows").iter().any(|r| r["id"] == g1.as_str() && r["locked"] == true), "{v}");
    let (_, v) = call(port, "GET", &format!("/cli/zen/gardens?token={tok}"), None);
    assert!(v["gardens"].as_array().expect("gardens").iter().any(|r| r["id"] == g1.as_str() && r["locked"] == true), "{v}");

    // A raw edit: GET put it back, kept it, and audited it.
    let zen_dir = home.0.join(".k2").join("zen");
    let file = zen_dir.join("gardens").join(format!("{g1}.toml"));
    let locked_text = read(&file);
    std::fs::write(&file, "schema = 1\n[colors.light]\naccent = \"#444444\"\n").expect("edit");
    let (s, v) = call(port, "GET", &format!("/cli/zen/get?garden={g1}&token={tok}"), None);
    assert_eq!(s, 200, "{v}");
    assert_ne!(v["theme"]["tokens"]["colors"]["light"]["accent"], "#444444", "the edit never shows: {v}");
    assert_eq!(read(&file), locked_text, "the locked text is back on disk");
    let refused = zen_dir.join(".history").join("gardens").join(format!("{g1}.toml")).join("refused");
    let kept: Vec<_> = std::fs::read_dir(&refused).expect("refused dir").flatten().collect();
    assert_eq!(kept.len(), 1, "one kept edit");
    let audit = read(&home.0.join(".k2").join("auth-audit.jsonl"));
    assert!(
        audit.lines().any(|l| l.contains("\"zen.garden.lock_refused\"") && l.contains(&g1) && l.contains("file_edit")),
        "one audit line for the put-back edit: {audit}"
    );
    assert!(audit.lines().any(|l| l.contains("\"zen.garden.lock\"") && l.contains("owner_token")), "the lock is audited: {audit}");
    let (s, v) = call(port, "GET", &format!("/cli/zen/validate?garden={g1}&token={tok}"), None);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["locked"]["garden"], g1.as_str(), "validate names the lock: {v}");

    // The person (owner token) still renames a locked Garden.
    let (s, v) = call(port, "POST", &format!("/cli/zen/garden/rename?token={tok}"), Some(&format!(r#"{{"garden":"{g1}","name":"Morning"}}"#)));
    assert_eq!(s, 200, "the person renames a locked Garden: {v}");

    // A Connect login (any role) can't flip it.
    let (s, v) = call(port, "POST", &format!("/cli/users/add?token={tok}"), Some(r#"{"username":"zlock","password":"correct-horse-battery-9"}"#));
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(port, "POST", &format!("/cli/users/set-role?token={tok}"), Some(r#"{"username":"zlock","role":"owner"}"#));
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(port, "POST", "/cli/auth/login", Some(r#"{"username":"zlock","password":"correct-horse-battery-9"}"#));
    assert_eq!(s, 200, "{v}");
    let login = v["token"].as_str().expect("login token").to_string();
    let (s, v) = call(port, "POST", &format!("/cli/zen/garden/lock?token={login}"), Some(&format!(r#"{{"garden":"{g1}","locked":false}}"#)));
    assert_eq!((s, v["error"].as_str()), (403, Some("owner_only")), "Connect owner: {v}");
    let (s, v) = call(port, "POST", &format!("/cli/zen/garden/rename?token={login}"), Some(&format!(r#"{{"garden":"{g1}","name":"x"}}"#)));
    assert_eq!((s, v["error"].as_str()), (403, Some("zen_local_only")), "Connect login on a Garden write: {v}");

    let (s, v) = call(port, "POST", &format!("/cli/zen/garden/lock?token={tok}"), Some(&format!(r#"{{"garden":"{g1}","locked":false}}"#)));
    assert_eq!((s, v["locked"].as_bool(), v["changed"].as_bool()), (200, Some(false), Some(true)), "unlock: {v}");
    assert!(!zen_dir.join(".locks").join(&g1).exists(), "unlock removes the frozen copy");
}

/// §5.2, §5.3: a session passport (an agent) is refused with 423 on every
/// Garden write to a locked Garden, still makes them on an unlocked one,
/// can neither lock nor unlock (403), and still gets 403 zen_local_only
/// on every other Zen route. The owner token still changes the locked one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_passport_is_refused_on_a_locked_garden() {
    use k2_core::session::SessionId;
    use k2_daemon::session_token::{self, CredMode, HookPrincipal, Provider};
    let prev = std::env::var_os("HOME");
    let tmp = std::env::temp_dir().join(format!("k2-zen-lock-passport-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).expect("temp HOME");
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();
    let d = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(k2_daemon::test_harness::start("owner-token-zen-lock")));
    let owner = d.owner_token.clone();
    let passport = session_token::mint_session_token(
        &SessionId::new(),
        "pane-zen-lock",
        HookPrincipal { workspace_uuid: "ws-zen-lock".into(), agent_address: "agent-zen-lock".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    );
    assert!(session_token::validate_hook(&passport).is_some(), "passport validates");
    let port = d.port;
    let post = |path: &str, tok: &str, body: String| -> (u16, J) {
        tokio::task::block_in_place(|| call(port, "POST", &format!("{path}?token={tok}"), Some(&body)))
    };
    let get = |path: &str, tok: &str| -> (u16, J) {
        tokio::task::block_in_place(|| {
            let sep = if path.contains('?') { '&' } else { '?' };
            call(port, "GET", &format!("{path}{sep}token={tok}"), None)
        })
    };

    let (s, v) = post("/cli/zen/setup", &owner, "{}".into());
    assert_eq!(s, 200, "setup: {v}");
    let ids: Vec<String> = v["gardens"].as_array().expect("gardens").iter().map(|g| g["id"].as_str().expect("id").to_string()).collect();
    let (locked, open) = (ids[0].clone(), ids[1].clone());
    let (s, v) = post("/cli/zen/garden/lock", &owner, format!(r#"{{"garden":"{locked}","locked":true}}"#));
    assert_eq!(s, 200, "owner locks: {v}");

    // Agents can neither lock nor unlock.
    for want in [false, true] {
        let (s, v) = post("/cli/zen/garden/lock", &passport, format!(r#"{{"garden":"{locked}","locked":{want}}}"#));
        assert_eq!((s, v["error"].as_str()), (403, Some("owner_only")), "passport lock={want}: {v}");
        assert!(v["message"].as_str().expect("message").contains("Only the person can lock or unlock"), "{v}");
        let (s, v) = post("/cli/zen/garden/lock", &passport, format!(r#"{{"garden":"{open}","locked":{want}}}"#));
        assert_eq!((s, v["error"].as_str()), (403, Some("owner_only")), "passport on an unlocked Garden, lock={want}: {v}");
    }

    let writes = |g: &str| -> Vec<(&'static str, String)> {
        vec![
            ("/cli/zen/garden/rename", format!(r#"{{"garden":"{g}","name":"Renamed {g}"}}"#)),
            ("/cli/zen/garden/template", format!(r#"{{"garden":"{g}","template":"blank","force":true}}"#)),
            ("/cli/zen/garden/reorder", format!(r#"{{"garden":"{g}","to":2}}"#)),
            ("/cli/zen/reset", format!(r#"{{"garden":"{g}"}}"#)),
            ("/cli/zen/reset", format!(r#"{{"file":"gardens/{g}.toml"}}"#)),
            ("/cli/zen/theme/set", format!(r#"{{"name":"paper","garden":"{g}"}}"#)),
            ("/cli/zen/theme/next", format!(r#"{{"garden":"{g}"}}"#)),
            ("/cli/zen/theme/prev", format!(r#"{{"garden":"{g}"}}"#)),
            ("/cli/zen/garden/delete", format!(r#"{{"garden":"{g}"}}"#)),
        ]
    };
    let zen_dir = tmp.join(".k2").join("zen");
    let locked_file = zen_dir.join("gardens").join(format!("{locked}.toml"));
    let before = read(&locked_file);
    for (path, body) in writes(&locked) {
        let (s, v) = post(path, &passport, body.clone());
        assert_eq!((s, v["error"].as_str()), (423, Some("garden_locked")), "passport {path} {body}: {v}");
        assert_eq!(v["garden"], locked.as_str(), "{v}");
        assert!(v["message"].as_str().expect("message").contains("is locked. Agents can't change it. Ask the person to unlock it in Settings"), "{v}");
    }
    assert_eq!(read(&locked_file), before, "refused writes changed nothing");
    let (_, v) = get("/cli/zen/gardens", &owner);
    let row = v["gardens"].as_array().expect("gardens").iter().find(|g| g["id"] == locked.as_str()).cloned().expect("still there");
    assert_eq!((row["index"].as_u64(), row["theme"].clone()), (Some(1), J::Null), "no rename, move or theme pick: {row}");
    assert_ne!(row["name"].as_str(), Some(format!("Renamed {locked}").as_str()));

    // On an unlocked Garden the same passport writes go through (agents
    // could always make them), except delete, made last.
    for (path, body) in writes(&open) {
        let (s, v) = post(path, &passport, body.clone());
        assert_eq!(s, 200, "passport {path} on an unlocked Garden {body}: {v}");
    }
    // The global theme isn't a Garden: an agent may still switch it, and
    // it reaches the locked Garden (ruling 4).
    let (s, v) = post("/cli/zen/theme/set", &passport, r#"{"name":"midnight"}"#.into());
    assert_eq!(s, 200, "global theme from a passport: {v}");
    let (s, v) = get(&format!("/cli/zen/get?garden={locked}"), &owner);
    assert_eq!((s, v["theme"]["name"].as_str()), (200, Some("midnight")), "the theme reached the locked Garden: {v}");

    // Every other Zen route still refuses a passport.
    for (m, p, body) in [
        ("GET", "/cli/zen/sync", None),
        ("GET", "/cli/zen/gardens", None),
        ("GET", "/cli/zen/get", None),
        ("POST", "/cli/zen/garden/sync", Some(format!(r#"{{"garden":"{locked}","part":"both","sync":false}}"#))),
        ("POST", "/cli/zen/garden/new", Some(r#"{"name":"Agent made"}"#.to_string())),
        ("POST", "/cli/zen/setup", Some("{}".to_string())),
    ] {
        let (s, v) = match body {
            Some(b) => post(p, &passport, b),
            None => get(p, &passport),
        };
        assert_eq!((s, v["error"].as_str()), (403, Some("zen_local_only")), "passport {m} {p}: {v}");
    }

    // The person (owner token, no passport) still changes the locked one.
    let (s, v) = post("/cli/zen/garden/rename", &owner, format!(r#"{{"garden":"{locked}","name":"Mine"}}"#));
    assert_eq!(s, 200, "owner rename: {v}");
    let (s, v) = post("/cli/zen/theme/set", &owner, format!(r#"{{"name":"paper","garden":"{locked}"}}"#));
    assert_eq!(s, 200, "owner retheme: {v}");
    let (s, v) = post("/cli/zen/reset", &owner, format!(r#"{{"garden":"{locked}"}}"#));
    assert_eq!(s, 200, "owner reset: {v}");
    let (s, v) = post("/cli/zen/garden/delete", &owner, format!(r#"{{"garden":"{locked}"}}"#));
    assert_eq!(s, 200, "owner delete: {v}");

    let audit = read(&tmp.join(".k2").join("auth-audit.jsonl"));
    assert!(
        audit.lines().any(|l| l.contains("\"zen.garden.lock_refused\"") && l.contains("passport_agent") && l.contains("is locked")),
        "a refused agent write is audited: {audit}"
    );
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    std::fs::remove_dir_all(&tmp).expect("remove temp HOME");
}
