//! Agent templates T1/T2 (prd-agent-templates-v1): the loader refuses a bad
//! template, apply is idempotent and never overwrites the agent's files,
//! the old hire-type ids still work, and every built-in loads and applies
//! headless. Real shared test DB, a temp HOME, temp folders.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::hire::HireCaller;

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

/// A temp HOME plus the shared test DB.
fn setup() -> crate::test_env::TempHome {
    let home = crate::test_env::TempHome::new();
    crate::db::init_for_tests();
    home
}

fn ws(home: &crate::test_env::TempHome, tag: &str) -> String {
    home.path().join("agents").join(uniq(tag)).to_string_lossy().into_owned()
}

fn req(id: &str, path: &str) -> ApplyRequest {
    ApplyRequest { path: Some(path.to_string()), ..ApplyRequest::new(id) }
}

fn read(path: &str, rel: &str) -> String {
    fs::read_to_string(Path::new(path).join(rel)).unwrap_or_else(|e| panic!("{path}/{rel}: {e}"))
}

/// Every file under `dir` with its bytes (to prove a second run writes
/// nothing).
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).expect("read_dir") {
            let p = e.expect("entry").path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p.clone(), fs::read(&p).expect("read"));
            }
        }
    }
    out
}

fn statuses(r: &ApplyReport, step: &str) -> Vec<&'static str> {
    r.rows.iter().filter(|x| x.step == step).map(|x| x.status).collect()
}

fn library_template(home: &crate::test_env::TempHome, id: &str, v: u32, files: &[(&str, &str)]) -> PathBuf {
    let dir = home.path().join(".k2/agent-templates").join(id).join(format!("v{v}"));
    for (rel, body) in files {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
    }
    dir
}

fn manifest(id: &str, v: u32, extra: &str) -> String {
    format!("schema = 1\nid = \"{id}\"\nversion = {v}\nname = \"Example\"\nsummary = \"An example.\"\nauthor = \"user:sam\"\n{extra}")
}

// ── loading ──────────────────────────────────────────────────────────

#[test]
fn every_builtin_template_loads() {
    let ids: Vec<String> = builtins()
        .into_iter()
        .map(|t| t.unwrap_or_else(|e| panic!("a built-in failed to load: {e}")).meta.id)
        .collect();
    for want in ["general", "lead", "developer", "app-owner", "designer", "researcher", "reviewer", "writer", "support", "ops"] {
        assert!(ids.iter().any(|i| i == want), "built-in {want} missing from {ids:?}");
    }
    // Ids and aliases are unique across the built-ins.
    let mut seen = std::collections::BTreeSet::new();
    for t in builtins().into_iter().map(Result::unwrap) {
        for n in std::iter::once(t.meta.id.clone()).chain(t.meta.aliases.clone()) {
            assert!(seen.insert(n.clone()), "{n} is used twice");
        }
        assert_eq!(t.meta.author, "K2");
        assert!(t.hash.len() == 64, "sha256 hex: {}", t.hash);
        // Starter notes are headings only (the agent fills them).
        for (id, body) in t.wiki_notes() {
            let text: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
            let fm_end = text.iter().skip(1).position(|l| *l == "---").map(|i| i + 2).unwrap_or(0);
            for l in &text[fm_end..] {
                assert!(l.starts_with('#'), "{}: {id} has body text {l:?}", t.meta.id);
            }
        }
    }
}

#[test]
fn old_hire_type_ids_resolve_to_their_templates() {
    for (old, new) in [("worker", "general"), ("manager", "lead"), ("qa", "reviewer"), ("researcher", "researcher")] {
        assert_eq!(resolve(old).expect(old).meta.id, new, "{old}");
    }
    let e = resolve("boss").expect_err("unknown id");
    assert_eq!(e.code, "unknown_template");
    assert_eq!(e.status, 404);
}

#[test]
fn template_loader_rejects_a_bad_template() {
    let home = setup();
    let role = ("ROLE.md", "You are {{name}}.\n");
    let cases: Vec<(&str, Vec<(String, String)>, &str)> = vec![
        ("unknown-key", vec![("template.toml".into(), manifest("unknown-key", 1, "mailSend = true\n"))], "unknown field"),
        ("bad-path", vec![("template.toml".into(), manifest("bad-path", 1, "")), ("notes/x.md".into(), "x".into())], "not a template path"),
        ("bad-ph", vec![("template.toml".into(), manifest("bad-ph", 1, "")), ("ROLE.md".into(), "Hi {{owner}}\n".into())], "unknown placeholder"),
        ("no-role", vec![("template.toml".into(), manifest("no-role", 1, ""))], "ROLE.md is missing"),
        ("k2-author", vec![("template.toml".into(), manifest("k2-author", 1, "").replace("user:sam", "K2"))], "only K2's built-ins"),
        ("version-off", vec![("template.toml".into(), manifest("version-off", 2, ""))], "doesn't match its folder v1"),
        ("bad-catalog", vec![("template.toml".into(), manifest("bad-catalog", 1, "[context]\ncatalog = [\"nope:pack\"]\n"))], "catalog"),
        ("bad-set", vec![("template.toml".into(), manifest("bad-set", 1, "[permissions]\nsuggest = [\"set:nope\"]\n"))], "unknown permission set"),
        ("system-lib", vec![("template.toml".into(), manifest("system-lib", 1, "kind = \"system\"\n"))], "only built-ins are kind"),
    ];
    for (id, files, want) in cases {
        let mut files = files;
        if !files.iter().any(|(p, _)| p == "ROLE.md") && id != "no-role" {
            files.push((role.0.into(), role.1.into()));
        }
        let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        library_template(&home, id, 1, &refs);
        let e = resolve(id).expect_err(id);
        assert_eq!(e.code, "bad_template", "{id}: {e}");
        assert!(e.hint.contains(want), "{id}: want {want:?} in {:?}", e.hint);
    }
    // A symlink inside a template folder (AT4).
    #[cfg(unix)]
    {
        let dir = library_template(&home, "linked", 1, &[("template.toml", &manifest("linked", 1, "")), role]);
        std::os::unix::fs::symlink("/etc/hosts", dir.join("wiki.md")).unwrap();
        let e = resolve("linked").expect_err("symlink");
        assert!(e.hint.contains("symlink"), "{}", e.hint);
    }
    // And a good one loads, with the hash over its files.
    library_template(&home, "fine", 1, &[("template.toml", &manifest("fine", 1, "")), role]);
    let t = resolve("fine").expect("a good library template");
    assert_eq!(t.origin, Origin::Library);
    assert_eq!(t.hash, format::hash_files(&t.files));
    // A library template can't take a built-in's id or alias: `writer`
    // still resolves to the built-in, and the folder is listed as a problem.
    library_template(&home, "writer", 1, &[("template.toml", &manifest("writer", 1, "")), role]);
    library_template(&home, "qa", 1, &[("template.toml", &manifest("qa", 1, "")), role]);
    assert_eq!(resolve("writer").unwrap().origin, Origin::Builtin);
    // `list` keeps going past the bad ones and names them.
    let l = list_json(false).expect("list");
    assert!(l["templates"].as_array().unwrap().iter().any(|x| x["id"] == "fine"));
    let problems: Vec<String> = l["problems"].as_array().unwrap().iter().map(|p| p["error"].as_str().unwrap().to_string()).collect();
    assert!(problems.iter().any(|p| p.contains("unknown-key")), "{problems:?}");
    assert!(problems.iter().any(|p| p.contains("'writer' is a built-in template id")), "{problems:?}");
    assert!(problems.iter().any(|p| p.contains("'qa' is a built-in template id")), "{problems:?}");
}

#[test]
fn template_suggest_refuses_one_time_actions_when_the_catalog_is_installed() {
    fn fake(id: &str) -> ActionKind {
        match id {
            "test.once" => ActionKind::OneTime,
            "test.nope" => ActionKind::Unknown,
            _ => ActionKind::Standing,
        }
    }
    install_action_check(fake);
    if action_kind("test.once") != ActionKind::OneTime {
        // Another test in this binary installed the real one first; this
        // test's fake can't be checked here.
        panic!("action check already installed by someone else");
    }
    let home = setup();
    for (id, s, want) in [("once", "test.once", "one-time"), ("nope", "test.nope", "not a K2 permission action")] {
        library_template(&home, id, 1, &[
            ("template.toml", &manifest(id, 1, &format!("[permissions]\nsuggest = [\"{s}\"]\n"))),
            ("ROLE.md", "x\n"),
        ]);
        let e = resolve(id).expect_err(id);
        assert!(e.hint.contains(want), "{id}: {}", e.hint);
    }
}

// ── apply ────────────────────────────────────────────────────────────

#[test]
fn template_apply_writes_the_agent_and_a_second_run_changes_nothing() {
    let home = setup();
    let path = ws(&home, "dev");
    let mut r = req("developer", &path);
    r.name = Some("Max".into());
    r.params.insert("about".into(), "the billing service".into());
    let first = apply(&r, &HireCaller::Owner).expect("first apply");
    assert!(first.created && first.changed);
    assert_eq!(first.template.id, "developer");
    let role = read(&path, ".k2/agent/ROLE.md");
    assert!(role.contains(crate::skills::version::SKILL_BEGIN_MARKER) && role.contains(crate::skills::version::SKILL_END_MARKER), "{role}");
    assert!(role.contains("You are Max. You build and fix software"), "{role}");
    assert!(role.contains("focus on: the billing service"), "{role}");
    assert!(role.contains("## Working with your person and the team"), "shared block: {role}");
    assert!(!role.contains("{{"), "no placeholder left: {role}");
    assert!(role.contains("## Your notes"), "{role}");
    assert!(Path::new(&path).join(".k2/wiki/About my person.md").is_file());
    assert!(Path::new(&path).join(".k2/wiki/Codebase map.md").is_file());
    assert!(read(&path, ".k2/wiki/_Index.md").contains("[[About my person]] · [[Codebase map]]"));
    let layers = crate::workspace::context_layers::list_layers(&path).expect("layers");
    for src in ["catalog:wiki-index", "catalog:wiki-hygiene", "catalog:connections-roster", "catalog:skills-roster"] {
        assert!(layers.iter().any(|l| l.source == src), "{src} stacked: {layers:?}");
    }
    let l = lock::read(&path).expect("lock").expect("lock written");
    assert_eq!((l.id.as_str(), l.version), ("developer", 1));
    assert_eq!(l.hash, first.template.hash);
    assert_eq!(l.by, "owner-token");
    assert_eq!(l.suggested, vec!["compute.use".to_string()]);
    assert!(l.granted.is_empty() && l.carded.is_empty(), "T2 grants nothing");
    assert!(l.files.contains_key(".k2/agent/ROLE.md") && l.files.contains_key(".k2/wiki/Codebase map.md"));
    assert_eq!(first.permissions.suggested, vec!["compute.use".to_string()]);
    assert!(first.hello.as_deref().is_some_and(|h| h.starts_with("Say hello to ") && !h.contains("{{")), "{:?}", first.hello);

    let before = snapshot(Path::new(&path));
    let second = apply(&r, &HireCaller::Owner).expect("second apply");
    assert!(!second.changed, "a second run changes nothing: {:?}", second.rows);
    assert!(!second.created);
    assert!(second.hello.is_none(), "the hello goes out once");
    assert!(second.rows.iter().all(|x| x.status != "done" && x.status != "write"), "{:?}", second.rows);
    assert_eq!(snapshot(Path::new(&path)), before, "no file changed on the second run");
}

#[test]
fn template_apply_never_overwrites_the_agents_files() {
    let home = setup();
    let path = ws(&home, "writer");
    apply(&req("writer", &path), &HireCaller::Owner).expect("first apply");
    let mine = "# About my person\n\nSam writes short, plain posts.\n";
    fs::write(Path::new(&path).join(".k2/wiki/About my person.md"), mine).unwrap();
    let role_path = Path::new(&path).join(".k2/agent/ROLE.md");
    let role_mine = format!("{}\nMy own line.\n", read(&path, ".k2/agent/ROLE.md"));
    fs::write(&role_path, &role_mine).unwrap();
    // A note the agent removed comes back (re-apply fills what's missing).
    fs::remove_file(Path::new(&path).join(".k2/wiki/Voice and style.md")).unwrap();

    let again = apply(&req("writer", &path), &HireCaller::Owner).expect("re-apply");
    assert_eq!(read(&path, ".k2/wiki/About my person.md"), mine, "the agent's note is never overwritten");
    assert_eq!(fs::read_to_string(&role_path).unwrap(), role_mine, "ROLE.md is never replaced");
    assert!(Path::new(&path).join(".k2/wiki/Voice and style.md").is_file());
    let wiki: Vec<(&str, &str)> = again.rows.iter().filter(|r| r.step == "wiki").map(|r| (r.target.as_str(), r.status)).collect();
    assert!(wiki.contains(&(".k2/wiki/About my person.md", "skip")), "{wiki:?}");
    assert!(wiki.contains(&(".k2/wiki/Voice and style.md", "done")), "{wiki:?}");

    // A workspace with its own ROLE.md (no template yet): kept, "yours".
    let own = ws(&home, "own");
    fs::create_dir_all(Path::new(&own).join(".k2/agent")).unwrap();
    let theirs = "---\nname: own\n---\n\nMy own charter.\n";
    fs::write(Path::new(&own).join(".k2/agent/ROLE.md"), theirs).unwrap();
    let r = apply(&req("general", &own), &HireCaller::Owner).expect("apply on an existing agent");
    assert_eq!(statuses(&r, "role"), vec!["yours"]);
    assert_eq!(read(&own, ".k2/agent/ROLE.md"), theirs);
}

#[test]
fn template_apply_one_template_per_agent() {
    let home = setup();
    let path = ws(&home, "one");
    apply(&req("writer", &path), &HireCaller::Owner).expect("first");
    let e = apply(&req("ops", &path), &HireCaller::Owner).expect_err("a second template");
    assert_eq!((e.status, e.code), (409, "template_conflict"), "{e}");
    // An alias of the same template is the same template.
    let gen = ws(&home, "gen");
    apply(&req("general", &gen), &HireCaller::Owner).expect("general");
    let again = apply(&req("worker", &gen), &HireCaller::Owner).expect("worker is general");
    assert!(!again.changed);
}

#[test]
fn template_dry_run_writes_nothing_and_matches_the_apply() {
    let home = setup();
    let path = ws(&home, "dry");
    let mut r = req("lead", &path);
    r.dry_run = true;
    let plan = apply(&r, &HireCaller::Owner).expect("dry run");
    assert!(plan.dry_run && plan.created && !plan.changed);
    assert!(!Path::new(&path).exists(), "a dry run writes nothing");
    assert_eq!(statuses(&plan, "folder"), vec!["write"]);
    assert_eq!(statuses(&plan, "role"), vec!["write"]);
    assert_eq!(statuses(&plan, "permission"), vec!["suggest"; 4], "cards + three switches: {:?}", plan.rows);
    assert!(plan.charter.contains("You lead a small team of agents"));
    r.dry_run = false;
    let done = apply(&r, &HireCaller::Owner).expect("apply");
    // Every row the plan said it would write, the apply wrote.
    let want: Vec<(&str, &str)> = plan.rows.iter().filter(|x| x.status == "write").map(|x| (x.step, x.target.as_str())).collect();
    let got: Vec<(&str, &str)> = done.rows.iter().filter(|x| x.status == "done").map(|x| (x.step, x.target.as_str())).collect();
    assert_eq!(want, got);
}

#[test]
fn template_params_are_checked() {
    let _home = setup();
    let t = resolve("general").unwrap();
    let mut p = BTreeMap::new();
    p.insert("budget".to_string(), "x".to_string());
    assert_eq!(bind_params(&t, &p, &HireCaller::Owner).unwrap_err().code, "usage");
    let mut p = BTreeMap::new();
    p.insert("about".to_string(), "x".repeat(501));
    assert!(bind_params(&t, &p, &HireCaller::Owner).unwrap_err().hint.contains("at most 500"));
    let mut p = BTreeMap::new();
    p.insert("person".to_string(), "Sam Lee".to_string());
    let v = bind_params(&t, &p, &HireCaller::Owner).unwrap();
    assert_eq!(v["person"], "Sam Lee");
    assert_eq!(v["about"], "", "an optional param not given is empty (its line is dropped)");
}

#[test]
fn template_hire_with_no_dir_goes_under_the_hire_root() {
    let home = setup();
    let root = home.path().join("hire-root");
    fs::create_dir_all(&root).unwrap();
    let auth = crate::hire::converge::ConvergeAuthority { new_workspace_root: Some(root.clone()), ..crate::hire::converge::ConvergeAuthority::unrestricted() };
    let name = uniq("Max");
    let mut r = ApplyRequest::new("writer");
    r.name = Some(name.clone());
    let out = apply_with(&r, &HireCaller::Owner, &auth).expect("no-dir hire");
    let slug = crate::workspace_session_handles::slugify_address_token(&name).unwrap();
    assert_eq!(Path::new(&out.path).file_name().unwrap().to_string_lossy(), slug);
    assert!(crate::hire::root::is_under(Path::new(&out.path), &root));
    // The same hire again is a re-run, not a second folder.
    let again = apply_with(&r, &HireCaller::Owner, &auth).expect("re-run");
    assert_eq!(again.path, out.path);
    assert!(!again.changed);
    // Someone else's folder there is refused.
    let taken = root.join("taken");
    fs::create_dir_all(taken.join("src")).unwrap();
    let mut r = ApplyRequest::new("writer");
    r.name = Some("taken".into());
    assert_eq!(apply_with(&r, &HireCaller::Owner, &auth).unwrap_err().code, "folder_exists");
}

#[test]
fn template_system_kind_is_refused_for_api_keys() {
    let _home = setup();
    let mut t = resolve("general").unwrap();
    t.meta.kind = Kind::System;
    let key = HireCaller::ApiKey { id: "k".into(), label: None };
    assert_eq!(check_caller(&t, &key).unwrap_err().code, "system_template");
    assert!(check_caller(&t, &HireCaller::Owner).is_ok());
}

#[test]
fn every_builtin_template_applies_headless() {
    let home = setup();
    for t in builtins().into_iter().map(|t| t.expect("loads")) {
        let path = ws(&home, &t.meta.id);
        let out = apply(&req(&t.meta.id, &path), &HireCaller::Owner).unwrap_or_else(|e| panic!("{}: {e}", t.meta.id));
        assert!(out.created, "{}", t.meta.id);
        let role = read(&path, ".k2/agent/ROLE.md");
        assert!(role.contains(crate::skills::version::SKILL_BEGIN_MARKER), "{}: {role}", t.meta.id);
        assert!(!role.contains("{{"), "{}: placeholder left: {role}", t.meta.id);
        assert!(!role.contains("focus on:"), "{}: the empty focus line is dropped: {role}", t.meta.id);
        for (note, _) in t.wiki_notes() {
            assert!(Path::new(&path).join(".k2/wiki").join(&note).is_file(), "{}: {note}", t.meta.id);
        }
        let l = lock::read(&path).unwrap().expect("lock");
        assert_eq!(l.id, t.meta.id);
        let expect_type = if t.meta.id == "lead" { "type: manager" } else { "type: agent-template" };
        assert!(role.contains(expect_type), "{}: {role}", t.meta.id);
        let again = apply(&req(&t.meta.id, &path), &HireCaller::Owner).expect("again");
        assert!(!again.changed, "{}: {:?}", t.meta.id, again.rows);
    }
}
