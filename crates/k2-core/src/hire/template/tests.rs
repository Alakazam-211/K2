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
    assert_eq!((l.id.as_str(), l.version), ("developer", 2));
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

// ── first-conversation questions: the pieces ─────────────────────────

#[test]
fn template_fill_note_puts_answers_under_their_headings() {
    let note = "---\ntitle: N\n---\n\n# N\n\n## A\n\n## B\n";
    let a = Some("A".to_string());
    let b = Some("B".to_string());
    assert_eq!(apply::fill_note(note, &[]), note);
    assert_eq!(apply::fill_note(note, &[(a.clone(), "one")]), "---\ntitle: N\n---\n\n# N\n\n## A\n\none\n\n## B\n");
    // Two under one heading keep their order; the last heading takes text too.
    assert_eq!(
        apply::fill_note(note, &[(a.clone(), "one"), (a, "two\nlines"), (b, "end")]),
        "---\ntitle: N\n---\n\n# N\n\n## A\n\none\n\ntwo\nlines\n\n## B\n\nend\n"
    );
    // No heading (or one that isn't there): at the end.
    assert_eq!(apply::fill_note(note, &[(None, "tail")]), format!("{note}\ntail\n"));
    assert!(format::is_heading("## How they sound", "How they sound"));
    assert!(!format::is_heading("##How", "How") && !format::is_heading("text How", "How"));
    assert_eq!(format::Fills::parse("role").unwrap(), format::Fills::Role);
    assert_eq!(
        format::Fills::parse("wiki:About my person#What they do").unwrap(),
        format::Fills::Wiki("About my person.md".into(), Some("What they do".into()))
    );
    assert!(format::Fills::parse("wiki:#x").is_err() && format::Fills::parse("wiki:N#").is_err());
}

#[test]
fn template_answers_for_a_note_not_written_go_to_role() {
    let home = setup();
    // --no-wiki: the note isn't written, so its answer lands in ROLE.md.
    let path = ws(&home, "nowiki");
    let mut r = req("writer", &path);
    r.no_wiki = true;
    r.answers.insert("samples".into(), "example.k2.dev/blog".into());
    let out = apply(&r, &HireCaller::Owner).expect("apply");
    let role = read(&path, ".k2/agent/ROLE.md");
    assert!(role.contains("- **Where can I see how you write: a few samples or links?** example.k2.dev/blog"), "{role}");
    assert!(!role.contains("Answered at hire, in your wiki"), "{role}");
    assert!(out.questions.iter().any(|q| q.id == "samples" && q.answered));
    // A dry run previews the answers in the charter and writes nothing.
    let dry = ws(&home, "dry-answers");
    let mut r = req("support", &dry);
    r.dry_run = true;
    r.answers.insert("tone".into(), "Warm and short.".into());
    let plan = apply(&r, &HireCaller::Owner).expect("dry run");
    assert!(plan.charter.contains("- **How should I sound with customers?** Warm and short."), "{}", plan.charter);
    assert!(!Path::new(&dry).exists());
    // An unknown id through apply too, naming the valid ids.
    let mut r = req("ops", &ws(&home, "ops"));
    r.answers.insert("budget".into(), "x".into());
    let e = apply(&r, &HireCaller::Owner).expect_err("unknown");
    assert!(e.hint.contains("its question ids: systems, limits, alerts"), "{}", e.hint);
    // Too long.
    let mut r = req("ops", &ws(&home, "ops2"));
    r.answers.insert("limits".into(), "x".repeat(format::MAX_ANSWER_CHARS + 1));
    assert!(apply(&r, &HireCaller::Owner).expect_err("long").hint.contains("at most 1000"));
}

// ── first-conversation questions ─────────────────────────────────────
//
// These go through the hire body's JSON (`answers`, as /cli/agents/hire
// and /v1/w take it), so they also build against a K2 without questions
// and fail there.

/// The picker types: each asks its person a few questions.
const PICKERS: &[&str] = &["general", "writer", "researcher", "developer", "designer", "reviewer", "support", "ops", "lead", "app-owner"];

/// Hire through converge with a JSON body (the hire routes' shape).
fn hire_json(body: serde_json::Value) -> Result<crate::hire::converge::ConvergeOutcome, crate::hire::HireError> {
    let req: crate::hire::converge::ConvergeRequest = serde_json::from_value(body).expect("a converge body");
    crate::hire::converge::converge(&req, &HireCaller::Owner, &crate::hire::converge::ConvergeAuthority::unrestricted())
}

fn questions_of(id: &str) -> Vec<serde_json::Value> {
    let t = resolve(id).unwrap_or_else(|e| panic!("{id}: {e}"));
    t.show_json(false)["questions"].as_array().unwrap_or_else(|| panic!("{id}: show lists no questions")).clone()
}

#[test]
fn every_picker_template_loads_with_its_questions() {
    let _home = setup();
    for id in PICKERS {
        let qs = questions_of(id);
        assert!((3..=5).contains(&qs.len()), "{id}: 3–5 questions, got {}", qs.len());
        for q in &qs {
            for k in ["id", "ask", "fills"] {
                assert!(q[k].as_str().is_some_and(|s| !s.trim().is_empty()), "{id}: question without {k}: {q}");
            }
            let fills = q["fills"].as_str().unwrap();
            assert!(fills == "role" || fills.starts_with("wiki:"), "{id}: {fills}");
        }
        let t = resolve(id).unwrap();
        let hello = t.meta.hello.as_ref().expect("hello").brief.clone();
        assert!(hello.contains("{{open_questions}}"), "{id}: the hello names the open questions: {hello}");
    }
    // The system types are K2 Onboarding's: no questions here.
    for id in ["kessel", "chief-of-staff"] {
        assert_eq!(questions_of(id).len(), 0, "{id}");
    }
}

#[test]
fn template_loader_rejects_a_bad_question() {
    let home = setup();
    let note = "---\ntitle: Notes\n---\n\n# Notes\n\n## Audience\n";
    let q = |id: &str, ask: &str, fills: &str| format!("\n[[questions]]\nid = \"{id}\"\nask = \"{ask}\"\nfills = \"{fills}\"\n");
    let cases: Vec<(&str, String, &str)> = vec![
        ("q-dup", format!("{}{}", q("who", "Who reads it?", "role"), q("who", "Who else?", "role")), "used twice"),
        ("q-target", q("who", "Who reads it?", "notes:Notes"), "must be role or wiki:"),
        ("q-note", q("who", "Who reads it?", "wiki:Nope#Audience"), "doesn't ship"),
        ("q-heading", q("who", "Who reads it?", "wiki:Notes#Nope"), "no heading 'Nope'"),
        ("q-ask", q("who", " ", "role"), "ask is 1–200 characters"),
        ("q-id", q("Who", "Who reads it?", "role"), "must be a-z"),
        ("q-unknown", "\n[[questions]]\nid = \"who\"\nask = \"Who?\"\nfills = \"role\"\nhint = \"x\"\n".to_string(), "unknown field"),
    ];
    for (id, extra, want) in cases {
        library_template(&home, id, 1, &[
            ("template.toml", &manifest(id, 1, &extra)),
            ("ROLE.md", "You are {{name}}.\n"),
            ("wiki/Notes.md", note),
        ]);
        let e = resolve(id).expect_err(id);
        assert_eq!(e.code, "bad_template", "{id}: {e}");
        assert!(e.hint.contains(want), "{id}: want {want:?} in {:?}", e.hint);
    }
    // A good one loads.
    let good = format!("{}{}", q("who", "Who reads it?", "wiki:Notes#Audience"), q("where", "Where do drafts go?", "role"));
    library_template(&home, "q-good", 1, &[("template.toml", &manifest("q-good", 1, &good)), ("ROLE.md", "You are {{name}}.\n"), ("wiki/Notes.md", note)]);
    let ids: Vec<String> = questions_of("q-good").iter().map(|q| q["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, vec!["who", "where"]);
}

#[test]
fn template_hire_answers_render_into_role_and_wiki() {
    let home = setup();
    let path = ws(&home, "ada-writer");
    let out = hire_json(serde_json::json!({
        "path": path,
        "name": "Ada",
        "template": "writer",
        "params": { "person": "Sam Lee" },
        "answers": {
            "audience": "Small-business owners who read on their phones.",
            "samples": "https://example.k2.dev/blog\nThe two posts pinned at the top.",
        },
    }))
    .expect("hire with answers");
    let role = read(&path, ".k2/agent/ROLE.md");
    assert!(role.contains("## What I know about my work"), "{role}");
    assert!(role.contains("- **Who do you write for?** Small-business owners who read on their phones."), "{role}");
    assert!(!role.contains("example.k2.dev"), "a wiki answer stays in its note: {role}");
    assert!(role.contains("## Your first conversation"), "{role}");
    assert!(role.contains("Answered at hire, in your wiki: [[Voice and style]]."), "{role}");
    let open = role.split("Still open from your hire:").nth(1).expect("an open list");
    assert!(open.contains("- Any words, tone or topics to avoid?") && open.contains("- Where should my drafts go when they're ready?"), "{open}");
    assert!(!open.contains("Who do you write for?") && !open.contains("samples or links"), "only the open ones: {open}");
    assert!(!role.contains("{{"), "{role}");
    // The note: the answer under its heading, the other headings kept.
    let voice = read(&path, ".k2/wiki/Voice and style.md");
    assert!(
        voice.contains("## Examples\n\nhttps://example.k2.dev/blog\nThe two posts pinned at the top.\n"),
        "under Examples: {voice}"
    );
    assert!(voice.contains("## How they sound\n\n## Words they use and avoid\n\n## Examples"), "{voice}");
    // The hello names only the open questions.
    let t = out.template.expect("the template ran");
    let hello = t.hello.clone().expect("hello on the first create");
    assert!(hello.contains("\"Any words, tone or topics to avoid?\"; \"Where should my drafts go when they're ready?\""), "{hello}");
    assert!(!hello.contains("Who do you write for?") && !hello.contains("samples or links"), "{hello}");
    assert!(hello.contains("Sam Lee"), "{hello}");
    let states = serde_json::to_value(&t).unwrap()["questions"].clone();
    let answered: Vec<(&str, bool)> =
        states.as_array().expect("questions").iter().map(|q| (q["id"].as_str().unwrap(), q["answered"].as_bool().unwrap())).collect();
    assert_eq!(answered, vec![("audience", true), ("samples", true), ("avoid", false), ("drafts", false)]);
}

#[test]
fn template_hire_with_no_answers_leaves_every_question_open() {
    let home = setup();
    let path = ws(&home, "plain-writer");
    let out = hire_json(serde_json::json!({ "path": path, "template": "writer" })).expect("hire");
    let role = read(&path, ".k2/agent/ROLE.md");
    assert!(!role.contains("## What I know about my work"), "nothing answered: {role}");
    assert!(role.contains("## Your first conversation"), "{role}");
    for ask in ["Who do you write for?", "samples or links?", "topics to avoid?", "when they're ready?"] {
        assert!(role.contains(ask), "{ask}: {role}");
    }
    let hello = out.template.expect("template").hello.expect("hello");
    assert!(hello.contains("Still open from your hire: \"Who do you write for?\"; "), "{hello}");
    let starter = resolve("writer").unwrap().files["wiki/Voice and style.md"].clone();
    assert_eq!(read(&path, ".k2/wiki/Voice and style.md"), starter, "an unanswered note is the bare starter");

    // Everything answered: nothing open, and the hello's open line drops.
    let all = ws(&home, "all-writer");
    let out = hire_json(serde_json::json!({ "path": all, "template": "writer", "answers": {
        "audience": "Founders", "samples": "example.k2.dev/notes", "avoid": "Jargon", "drafts": "The drafts folder",
    } }))
    .expect("hire");
    let role = read(&all, ".k2/agent/ROLE.md");
    assert!(role.contains("Still open from your hire: nothing."), "{role}");
    let hello = out.template.expect("template").hello.expect("hello");
    assert!(!hello.contains("Still open"), "{hello}");
}

#[test]
fn template_hire_refuses_an_unknown_answer_id() {
    let home = setup();
    let path = ws(&home, "bad-answer");
    let e = hire_json(serde_json::json!({ "path": path, "template": "writer", "answers": { "tone": "warm" } })).expect_err("unknown id");
    assert_eq!(e.code, "usage", "{e}");
    assert!(e.hint.contains("no question 'tone'") && e.hint.contains("audience, samples, avoid, drafts"), "{}", e.hint);
    assert!(!Path::new(&path).exists(), "nothing written");
    let e = hire_json(serde_json::json!({ "path": path, "template": "writer", "answers": { "audience": "  " } })).expect_err("empty");
    assert!(e.hint.contains("answer audience is empty"), "{}", e.hint);
    let e = hire_json(serde_json::json!({ "path": path, "answers": { "audience": "x" } })).expect_err("no template");
    assert!(e.hint.contains("answers go with a template"), "{}", e.hint);
    assert!(!Path::new(&path).exists(), "nothing written");
}

#[test]
fn template_reapply_with_answers_is_idempotent_and_keeps_edits() {
    let home = setup();
    let path = ws(&home, "again");
    let body = serde_json::json!({ "path": path, "template": "developer", "answers": {
        "repo": "~/code/shop", "done": "Tests pass and a PR is open.",
    } });
    hire_json(body.clone()).expect("first");
    assert!(read(&path, ".k2/wiki/Codebase map.md").contains("## Layout\n\n~/code/shop\n"));
    assert!(read(&path, ".k2/agent/ROLE.md").contains("- **What does done mean for a change"));
    let before = snapshot(Path::new(&path));
    let again = hire_json(body.clone()).expect("again");
    assert!(!again.changed, "{:?}", again.template.map(|t| t.rows));
    assert_eq!(snapshot(Path::new(&path)), before, "no file changed");
    // The agent edits its notes and ROLE.md; new answers never overwrite.
    let map = Path::new(&path).join(".k2/wiki/Codebase map.md");
    fs::write(&map, "# Codebase map\n\nMine now.\n").unwrap();
    let role_path = Path::new(&path).join(".k2/agent/ROLE.md");
    let mine = format!("{}\nWe ship on Fridays.\n", read(&path, ".k2/agent/ROLE.md"));
    fs::write(&role_path, &mine).unwrap();
    let other = serde_json::json!({ "path": path, "template": "developer", "answers": { "repo": "~/code/other", "tests": "make test" } });
    let out = hire_json(other).expect("re-apply with other answers");
    assert_eq!(fs::read_to_string(&map).unwrap(), "# Codebase map\n\nMine now.\n");
    assert_eq!(fs::read_to_string(&role_path).unwrap(), mine);
    assert!(out.template.expect("template").hello.is_none(), "the hello went out once");
}
