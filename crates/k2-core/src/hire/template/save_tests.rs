//! Save as template, add and remove (prd-agent-templates-v1 §7, T6): the
//! allowlist, the secret check, versions and authors, and a saved template
//! hires. Real shared test DB, a temp HOME, temp folders. Secrets here are
//! fake, secret-shaped strings.

use std::fs;
use std::path::Path;

use super::*;
use crate::hire::template::{apply, resolve, ApplyRequest};
use crate::hire::HireCaller;

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

fn setup() -> crate::test_env::TempHome {
    let home = crate::test_env::TempHome::new();
    crate::db::init_for_tests();
    home
}

/// A developer agent with its own skill, a K2-managed skill, an inline
/// layer, a heartbeat, a wiki note and an `.env` file.
fn agent(home: &crate::test_env::TempHome) -> String {
    let path = home.path().join("agents").join(uniq("dev")).to_string_lossy().into_owned();
    let mut r = ApplyRequest { path: Some(path.clone()), name: Some("Max".into()), ..ApplyRequest::new("developer") };
    r.params.insert("about".into(), "the billing service".into());
    apply(&r, &HireCaller::Owner).expect("hire");
    let p = Path::new(&path);
    fs::create_dir_all(p.join(".k2/skills/release-notes")).unwrap();
    fs::write(p.join(".k2/skills/release-notes/SKILL.md"), "---\nname: release-notes\n---\n\n# Release notes\n\nWrite them short.\n").unwrap();
    fs::create_dir_all(p.join(".k2/skills/k2-cli")).unwrap();
    fs::write(
        p.join(".k2/skills/k2-cli/SKILL.md"),
        format!("---\nk2_skill: k2-cli\nskill_version: 3\n---\n\n{}\nK2's own.\n{}\n", SKILL_BEGIN_MARKER, SKILL_END_MARKER),
    )
    .unwrap();
    crate::hire::steps::add_inline_layer(&path, "house-style", "# House style\n\nShort sentences.\n").expect("layer");
    crate::hire::steps::add_heartbeats(
        &path,
        &[recipe::HeartbeatIn {
            name: "daily-digest".into(),
            frequency: "daily".into(),
            spec: Some(serde_json::json!({ "time": "09:00" })),
            instructions: Some("Read the inbox and post a digest.".into()),
        }],
    )
    .expect("heartbeat");
    fs::write(p.join(".env"), "SECRET=sk-ant-api03-FAKEfake1234567890abcdEF\n").unwrap();
    fs::write(p.join(".k2/wiki/Team.md"), "# Team\n\nSam and Lee.\n").unwrap();
    path
}

fn input(path: &str, id: &str) -> SaveInput {
    SaveInput {
        workspace: path.to_string(),
        id: id.to_string(),
        author: "user:sam".into(),
        suggest: vec!["compute.use".into()],
        cards: true,
        ..Default::default()
    }
}

#[test]
fn template_save_takes_the_allowlist_and_versions_by_author() {
    let home = setup();
    let path = agent(&home);
    let id = uniq("mine");

    // A dry run: the review, nothing written.
    let mut inp = input(&path, &id);
    inp.dry_run = true;
    let plan = save(&inp).expect("dry run");
    assert!(plan.dry_run && plan.version == 1, "{plan:?}");
    assert!(!super::super::library_dir().join(&id).exists(), "a dry run writes nothing");
    let files: Vec<&str> = plan.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(&files[..2], &["template.toml", "ROLE.md"], "{files:?}");
    assert!(files.contains(&"context/house-style.md") && files.contains(&"skills/release-notes/SKILL.md"), "{files:?}");
    assert!(!files.iter().any(|f| f.starts_with("wiki/")), "no wiki note unless picked: {files:?}");
    assert!(!files.iter().any(|f| f.contains("k2-cli") || f.contains("env")), "{files:?}");
    assert!(plan.skipped.iter().any(|s| s.what == "skill" && s.name == "k2-cli" && s.why == "k2_managed"), "{:?}", plan.skipped);
    assert!(plan.catalog.contains(&"wiki:index".to_string()) && plan.catalog.contains(&"skills:roster".to_string()), "{:?}", plan.catalog);
    assert_eq!(plan.suggest, vec!["compute.use".to_string()]);
    assert_eq!(plan.heartbeats, vec!["daily-digest".to_string()]);
    assert_eq!(plan.tool.as_deref(), Some("default"));
    assert_eq!(plan.author, "user:sam");
    assert!(plan.hash.starts_with("sha256:") && plan.path.ends_with(&format!("agent-templates/{id}/v1")), "{plan:?}");

    // The save, with one picked note.
    inp.dry_run = false;
    inp.wiki = vec!["Team".into()];
    inp.summary = Some("Builds the billing service.".into());
    let v1 = save(&inp).expect("save");
    assert_eq!(v1.version, 1);
    assert!(v1.files.iter().any(|f| f.path == "wiki/Team.md"), "{:?}", v1.files);
    let t = resolve(&id).expect("the saved template loads");
    assert_eq!((t.meta.version, t.meta.author.as_str(), t.meta.summary.as_str()), (1, "user:sam", "Builds the billing service."));
    assert!(t.meta.permissions.cards && !t.meta.shared_block);
    assert_eq!(t.meta.heartbeats[0].instructions.as_deref(), Some("Read the inbox and post a digest."));
    let role = t.role_md();
    assert!(role.contains("You build and fix software") && role.contains("billing service"), "{role}");
    assert!(!role.contains("K2:MANAGED") && !role.contains("## Your notes") && !role.starts_with("# Persona"), "{role}");

    // Same author again: v2, v1 kept.
    let v2 = save(&inp).expect("save again");
    assert_eq!(v2.version, 2);
    assert!(super::super::library_dir().join(&id).join("v1").is_dir());
    // Another author: id_taken. Built-ins and aliases: id_reserved. A bad slug: bad_id.
    let e = save(&SaveInput { author: "agent:other".into(), ..inp.clone() }).expect_err("another author");
    assert_eq!((e.status, e.code), (409, "id_taken"), "{e}");
    for reserved in ["developer", "qa", "kessel"] {
        let e = save(&input(&path, reserved)).expect_err(reserved);
        assert_eq!((e.status, e.code), (409, "id_reserved"), "{reserved}: {e}");
    }
    let e = save(&input(&path, "Bad Id")).expect_err("bad id");
    assert_eq!((e.status, e.code), (400, "bad_id"));
    // A missing picked note.
    let e = save(&SaveInput { wiki: vec!["Nope".into()], ..input(&path, &uniq("x")) }).expect_err("no note");
    assert_eq!(e.status, 404, "{e}");

    // The saved template hires a new agent.
    let fresh = home.path().join("agents").join(uniq("copy")).to_string_lossy().into_owned();
    let r = ApplyRequest { path: Some(fresh.clone()), ..ApplyRequest::new(&id) };
    apply(&r, &HireCaller::Owner).expect("hire from the saved template");
    let role = fs::read_to_string(Path::new(&fresh).join(".k2/agent/ROLE.md")).unwrap();
    assert_eq!(role.matches("## Your notes").count(), 1, "{role}");
    assert!(Path::new(&fresh).join(".k2/skills/release-notes/SKILL.md").is_file());
    assert!(Path::new(&fresh).join(".k2/wiki/Team.md").is_file());
    assert!(!Path::new(&fresh).join(".env").exists());
}

#[test]
fn template_save_refuses_a_secret_naming_file_and_line() {
    let home = setup();
    let path = agent(&home);
    let fake = format!("k2skn_{}", "a1B2c3D4e5".repeat(4) + "xyz");
    fs::write(
        Path::new(&path).join(".k2/skills/release-notes/SKILL.md"),
        format!("# Release notes\n\nok\nthe pass is {fake}\n"),
    )
    .unwrap();
    let e = save(&input(&path, &uniq("leak"))).expect_err("secret");
    assert_eq!((e.status, e.code), (422, "secret_found"), "{e}");
    assert_eq!(e.file.as_deref(), Some(".k2/skills/release-notes/SKILL.md"));
    assert_eq!(e.line, Some(4));
    assert!(!e.hint.contains(&fake) && !e.body().to_string().contains(&fake), "never echoes the secret");
    assert_eq!(e.body()["error"]["line"], 4);
    // A picked note with a secret is refused the same way.
    fs::write(Path::new(&path).join(".k2/skills/release-notes/SKILL.md"), "# Release notes\n").unwrap();
    fs::write(Path::new(&path).join(".k2/wiki/Keys.md"), "# Keys\n\n-----BEGIN RSA PRIVATE KEY-----\n").unwrap();
    let e = save(&SaveInput { wiki: vec!["Keys.md".into()], ..input(&path, &uniq("leak")) }).expect_err("secret note");
    assert_eq!((e.code, e.file.as_deref(), e.line), ("secret_found", Some(".k2/wiki/Keys.md"), Some(3)));
}

fn folder(home: &crate::test_env::TempHome, id: &str, author: &str, role: &str) -> std::path::PathBuf {
    let dir = home.path().join("incoming").join(uniq("t"));
    fs::create_dir_all(dir.join("wiki")).unwrap();
    fs::write(
        dir.join("template.toml"),
        format!("schema = 1\nid = \"{id}\"\nversion = 7\nname = \"Example\"\nsummary = \"An example.\"\nauthor = \"{author}\"\n\n[agent]\ntool = \"default\"\n"),
    )
    .unwrap();
    fs::write(dir.join("ROLE.md"), role).unwrap();
    fs::write(dir.join("wiki/Notes.md"), "# Notes\n").unwrap();
    dir
}

#[test]
fn template_add_and_remove_follow_the_id_rules() {
    let home = setup();
    let id = uniq("added");
    // The installer is the author; the folder's own claim (even K2) is replaced.
    let dir = folder(&home, &id, "K2", "You are {{name}}.\n");
    let a = add(&dir, "agent:max", false).expect("add");
    assert_eq!((a.version, a.author.as_str()), (1, "agent:max"));
    let t = resolve(&id).expect("loads");
    assert_eq!((t.meta.version, t.meta.author.as_str()), (1, "agent:max"));
    assert_eq!(add(&dir, "agent:max", false).expect("again").version, 2);
    let e = add(&dir, "user:sam", false).expect_err("another author");
    assert_eq!(e.code, "id_taken");
    // Built-in ids, bad paths and secrets.
    let e = add(&folder(&home, "writer", "user:sam", "x\n"), "user:sam", false).expect_err("built-in");
    assert_eq!(e.code, "id_reserved");
    let bad = folder(&home, &uniq("bad"), "user:sam", "x\n");
    fs::write(bad.join("notes.txt"), "x").unwrap();
    assert_eq!(add(&bad, "user:sam", false).expect_err("bad path").code, "bad_template");
    let leak = folder(&home, &uniq("leak"), "user:sam", "Use ghp_FAKEfake1234567890abcdefGHIJ\n");
    let e = add(&leak, "user:sam", false).expect_err("secret");
    assert_eq!((e.code, e.file.as_deref(), e.line), ("secret_found", Some("ROLE.md"), Some(1)));
    #[cfg(unix)]
    {
        let sym = folder(&home, &uniq("sym"), "user:sam", "x\n");
        std::os::unix::fs::symlink("/etc/hosts", sym.join("wiki/Hosts.md")).unwrap();
        assert_eq!(add(&sym, "user:sam", false).expect_err("symlink").code, "bad_template");
    }

    // Remove: an agent only its own; one version or all; built-ins never.
    let e = remove(&id, None, Some("agent:other")).expect_err("not the author");
    assert_eq!((e.status, e.code), (403, "not_author"));
    let r = remove(&id, Some(1), Some("agent:max")).expect("remove v1");
    assert_eq!(r["remaining"], serde_json::json!([2]));
    assert_eq!(remove(&id, Some(1), None).expect_err("gone").code, "unknown_template");
    remove(&id, None, None).expect("a person removes the rest");
    assert!(!super::super::library_dir().join(&id).exists());
    assert_eq!(remove("writer", None, None).expect_err("built-in").code, "id_reserved");
    assert_eq!(remove("qa", None, None).expect_err("alias").code, "id_reserved");
}

#[test]
fn template_charter_from_role_takes_the_managed_inside_and_the_notes() {
    let role = format!(
        "---\nname: max\nrole: developer\n---\n\n# Persona — Max\n\n{SKILL_BEGIN_MARKER}\nYou are Max.\n\n## Working with your person\n- Ask.\n{SKILL_END_MARKER}\n\n## Your notes\n\n_Yours: K2 never changes anything below this line._\n\nPrefers tabs.\n"
    );
    assert_eq!(charter_from_role(&role), "You are Max.\n\n## Working with your person\n- Ask.\n\nPrefers tabs.\n");
    assert_eq!(charter_from_role("---\nname: x\n---\n\n# Persona — X\n\nMy own charter.\n"), "My own charter.\n");
    assert_eq!(charter_from_role("Just text.\n"), "Just text.\n");
}

/// A ROLE.md written today has the "yours" line that names no product, and
/// it is read as the line too; the old line above (in files written before)
/// still is. Neither ends up in the charter.
#[test]
fn template_charter_from_role_drops_the_new_yours_line_too() {
    use crate::hire::template::apply::{role_body, ROLE_YOURS_LINE, ROLE_YOURS_LINE_OLD};
    let body = role_body("Max", "You are Max.\n");
    assert!(body.contains(ROLE_YOURS_LINE), "{body}");
    assert!(!body.contains(ROLE_YOURS_LINE_OLD), "{body}");
    let role = format!("---\nname: max\nrole: developer\n---\n\n{body}\nPrefers tabs.\n");
    assert_eq!(charter_from_role(&role), "You are Max.\n\nPrefers tabs.\n");
}
