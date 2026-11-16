//! Upgrades without clobbering (prd-agent-templates-v1 §4, T5): the
//! per-file checksum rule with `.proposed`, the K2:MANAGED block only,
//! additions, heartbeats and permissions only reported, the inbox note,
//! pack checksums (AT10) and the start pass. Real shared test DB, a temp
//! HOME, temp folders.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::hire::template::{apply, lock, ApplyRequest};
use crate::hire::HireCaller;

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

fn setup() -> crate::test_env::TempHome {
    let home = crate::test_env::TempHome::new();
    crate::db::init_for_tests();
    home
}

fn library(home: &crate::test_env::TempHome, id: &str, v: u32, extra: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = crate::paths::k2_home_in(home.path()).join("agent-templates").join(id).join(format!("v{v}"));
    let manifest = format!(
        "schema = 1\nid = \"{id}\"\nversion = {v}\nname = \"Example\"\nsummary = \"An example.\"\nauthor = \"user:sam\"\n{extra}"
    );
    for (rel, body) in std::iter::once(&("template.toml", manifest.as_str())).chain(files.iter()) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
    }
    dir
}

fn rd(path: &str, rel: &str) -> String {
    fs::read_to_string(Path::new(path).join(rel)).unwrap_or_else(|e| panic!("{path}/{rel}: {e}"))
}

fn status<'a>(r: &'a UpgradeReport, target: &str) -> Option<&'a str> {
    r.rows.iter().find(|x| x.target.ends_with(target)).map(|x| x.status)
}

const V1_TOML: &str = "\n[context]\ncatalog = [\"wiki:index\", \"manager:pack\"]\n";
const V2_TOML: &str = "\n[context]\ncatalog = [\"wiki:index\", \"manager:pack\", \"heartbeats:roster\"]\n\n[permissions]\nsuggest = [\"compute.use\"]\n\n[[heartbeats]]\nname = \"daily-check\"\nfrequency = \"daily\"\nspec = { time = \"09:00\" }\ninstructions = \"Check things.\"\n";

/// v1 hired, then v2 in the library: `(workspace, id)`.
fn hired(home: &crate::test_env::TempHome) -> (String, String) {
    let id = uniq("up");
    library(
        home,
        &id,
        1,
        V1_TOML,
        &[
            ("ROLE.md", "You are {{name}}. Version one.\n"),
            ("wiki/Notes.md", "# Notes\n"),
            ("wiki/Other.md", "# Other\n"),
            ("wiki/Gone.md", "# Gone\n"),
            ("context/style.md", "# Style v1\n"),
        ],
    );
    let path = home.path().join("agents").join(uniq("ag")).to_string_lossy().into_owned();
    let r = ApplyRequest { path: Some(path.clone()), name: Some("Max".into()), ..ApplyRequest::new(&id) };
    apply(&r, &HireCaller::Owner).expect("hire v1");
    library(
        home,
        &id,
        2,
        V2_TOML,
        &[
            ("ROLE.md", "You are {{name}}. Version two.\n"),
            ("wiki/Notes.md", "# Notes\n\n## New heading\n"),
            ("wiki/Other.md", "# Other v2\n"),
            ("wiki/Gone.md", "# Gone v2\n"),
            ("wiki/Added.md", "# Added\n"),
            ("context/style.md", "# Style v2\n"),
            ("skills/helper/SKILL.md", "# Helper\n"),
        ],
    );
    (path, id)
}

#[test]
fn template_upgrade_replaces_untouched_files_and_proposes_beside_edited_ones() {
    let home = setup();
    let (path, id) = hired(&home);
    let l1 = lock::read(&path).unwrap().unwrap();
    assert_eq!(l1.vars.get("name").map(String::as_str), Some("Max"), "the lock keeps what it rendered with");
    // The agent edits one note, deletes another, and writes in its own part of ROLE.md.
    fs::write(Path::new(&path).join(".k2/wiki/Other.md"), "# Other\n\nMine now.\n").unwrap();
    fs::remove_file(Path::new(&path).join(".k2/wiki/Gone.md")).unwrap();
    let role_path = Path::new(&path).join(".k2/agent/ROLE.md");
    fs::write(&role_path, format!("{}\nPrefers tabs.\n", rd(&path, ".k2/agent/ROLE.md"))).unwrap();
    let tool_before = crate::hire::steps::current_setting(&path, "default_agent");

    // diff = the dry run: every row, nothing written.
    let plan = upgrade(&path, None, "user:sam", true, true).expect("dry run");
    assert_eq!((plan.from, plan.to, plan.dry_run, plan.changed), (1, 2, true, true));
    assert_eq!(status(&plan, "ROLE.md"), Some("replace"), "{:?}", plan.rows);
    assert_eq!(status(&plan, "Notes.md"), Some("replace"));
    assert_eq!(status(&plan, "Other.md"), Some("keep"));
    assert_eq!(status(&plan, "Gone.md"), Some("deleted"));
    assert_eq!(status(&plan, "Added.md"), Some("add"));
    assert_eq!(status(&plan, "style.md"), Some("replace"));
    assert_eq!(status(&plan, "helper/SKILL.md"), Some("add"));
    assert_eq!(status(&plan, "heartbeats:roster"), Some("add"));
    assert_eq!(status(&plan, "daily-check"), Some("report"));
    assert_eq!(status(&plan, "compute.use"), Some("report"));
    assert!(plan.rows.iter().find(|r| r.target.ends_with("ROLE.md")).unwrap().diff.as_deref().is_some_and(|d| d.contains("+You are Max. Version two.")), "{:?}", plan.rows);
    assert_eq!(rd(&path, ".k2/wiki/Notes.md"), "# Notes\n", "a dry run writes nothing");
    assert!(!Path::new(&path).join(".k2/wiki/Other.md.proposed").exists());
    assert_eq!(lock::read(&path).unwrap().unwrap().version, 1);

    // The upgrade.
    let done = upgrade(&path, None, "user:sam", false, false).expect("upgrade");
    assert!(done.changed);
    let role = rd(&path, ".k2/agent/ROLE.md");
    assert!(role.contains("You are Max. Version two.") && !role.contains("Version one"), "{role}");
    assert!(role.contains("Prefers tabs."), "the agent's part is never touched: {role}");
    assert_eq!(rd(&path, ".k2/wiki/Notes.md"), "# Notes\n\n## New heading\n");
    assert_eq!(rd(&path, ".k2/wiki/Other.md"), "# Other\n\nMine now.\n", "an edited file is kept");
    assert_eq!(rd(&path, ".k2/wiki/Other.md.proposed"), "# Other v2\n");
    assert!(!Path::new(&path).join(".k2/wiki/Gone.md").exists(), "a deleted file stays deleted");
    assert_eq!(rd(&path, ".k2/wiki/Added.md"), "# Added\n");
    assert_eq!(rd(&path, ".k2/context/style.md"), "# Style v2\n");
    assert!(Path::new(&path).join(".k2/skills/helper/SKILL.md").is_file());
    let layers = crate::workspace::context_layers::list_layers(&path).unwrap();
    assert!(layers.iter().any(|l| l.source == "catalog:heartbeats-roster"), "{layers:?}");
    // Never: heartbeats, permissions, tool.
    let hbs = crate::heartbeats::k2_heartbeat_list(path.clone()).unwrap();
    assert!(!hbs.iter().any(|h| h.name == "daily-check"), "heartbeats are only reported");
    let l2 = lock::read(&path).unwrap().unwrap();
    assert_eq!(l2.version, 2);
    assert!(l2.suggested.is_empty() && l2.granted.is_empty(), "an upgrade never adds permissions: {l2:?}");
    assert_eq!(l2.kept, vec![".k2/wiki/Other.md".to_string()]);
    assert_eq!(crate::hire::steps::current_setting(&path, "default_agent"), tool_before);
    // One inbox note.
    let notes: Vec<_> = crate::inbox::list_root(Path::new(&path)).into_iter().filter(|i| i.title.contains("template went 1 → 2")).collect();
    assert_eq!(notes.len(), 1, "{notes:?}");
    let body = done.note.clone().unwrap();
    assert!(body.contains("Kept yours") && body.contains("Other.md.proposed") && body.contains("Replaced"), "{body}");

    // Again: up to date, nothing written, no second note.
    let again = upgrade(&path, None, "user:sam", false, false).expect("again");
    assert!(!again.changed && again.from == 2 && again.to == 2, "{again:?}");
    assert_eq!(crate::inbox::list_root(Path::new(&path)).iter().filter(|i| i.title.contains("went 1 → 2")).count(), 1);

    // Two library versions, diffed.
    let d = diff_versions(&id, 1, 2).expect("diff");
    let st = |p: &str| d["files"].as_array().unwrap().iter().find(|f| f["path"] == p).map(|f| f["status"].as_str().unwrap().to_string());
    assert_eq!(st("wiki/Added.md").as_deref(), Some("added"));
    assert_eq!(st("ROLE.md").as_deref(), Some("changed"));
    assert_eq!(diff_versions(&id, 1, 9).expect_err("no v9").code, "unknown_template");
}

#[test]
fn template_upgrade_refreshes_packs_by_checksum() {
    let home = setup();
    let (path, _id) = hired(&home);
    let pack = ".k2/context/catalog/manager.md";
    let body = crate::workspace::context_layers::static_pack_body("catalog:manager").unwrap();
    let mut l = lock::read(&path).unwrap().unwrap();
    assert_eq!(l.files.get(pack), Some(&sha256_text(body)), "apply records the pack it stacked");

    // An older pack text K2 wrote: replaced.
    fs::write(Path::new(&path).join(pack), "# old pack\n").unwrap();
    l.files.insert(pack.into(), sha256_text("# old pack\n"));
    lock::write(&path, &l).unwrap();
    let r = upgrade_at_start(&path).expect("start pass").expect("has a template");
    assert_eq!(status(&r, "manager.md"), Some("replace"), "{:?}", r.rows);
    assert_eq!(rd(&path, pack), body);
    assert!(!Path::new(&path).join(".k2/wiki/Notes.md.proposed").exists(), "the start pass never upgrades a library template");
    assert_eq!(lock::read(&path).unwrap().unwrap().version, 1);

    // The agent edited it: kept, proposed.
    fs::write(Path::new(&path).join(pack), "# mine\n").unwrap();
    let mut l = lock::read(&path).unwrap().unwrap();
    l.files.insert(pack.into(), sha256_text("# older K2 text\n"));
    lock::write(&path, &l).unwrap();
    let r = upgrade_at_start(&path).unwrap().unwrap();
    assert_eq!(status(&r, "manager.md"), Some("keep"));
    assert_eq!(rd(&path, pack), "# mine\n");
    assert_eq!(rd(&path, &format!("{pack}.proposed")), body);
    // A second start says nothing new.
    let r = upgrade_at_start(&path).unwrap().unwrap();
    assert!(!r.changed, "{:?}", r.rows);

    // A lock from before pack checksums: proposed, never replaced.
    let mut l = lock::read(&path).unwrap().unwrap();
    l.files.remove(pack);
    l.kept.clear();
    lock::write(&path, &l).unwrap();
    fs::remove_file(Path::new(&path).join(format!("{pack}.proposed"))).unwrap();
    let r = upgrade_at_start(&path).unwrap().unwrap();
    assert_eq!(status(&r, "manager.md"), Some("keep"));
    assert_eq!(rd(&path, pack), "# mine\n");
}

#[test]
fn template_upgrade_at_start_moves_builtins_only_up() {
    let home = setup();
    let path = home.path().join("agents").join(uniq("dev")).to_string_lossy().into_owned();
    let r = ApplyRequest { path: Some(path.clone()), ..ApplyRequest::new("writer") };
    apply(&r, &HireCaller::Owner).expect("hire");
    let before = rd(&path, ".k2/agent/ROLE.md");
    // Current built-in: nothing to do.
    let out = upgrade_at_start(&path).unwrap().unwrap();
    assert!(!out.changed && out.from == out.to, "{out:?}");
    // A lock one version behind: the built-in upgrades.
    let mut l = lock::read(&path).unwrap().unwrap();
    let current = l.version;
    l.version = current - 1;
    lock::write(&path, &l).unwrap();
    let out = upgrade_at_start(&path).unwrap().unwrap();
    assert_eq!((out.from, out.to), (current - 1, current));
    assert_eq!(rd(&path, ".k2/agent/ROLE.md"), before, "the same text: unchanged, nothing clobbered");
    assert_eq!(lock::read(&path).unwrap().unwrap().version, current);
    // No template: nothing.
    let plain = home.path().join(uniq("plain"));
    fs::create_dir_all(&plain).unwrap();
    assert!(upgrade_at_start(&plain.to_string_lossy()).unwrap().is_none());
    assert_eq!(upgrade(&plain.to_string_lossy(), None, "user:sam", true, false).expect_err("no lock").code, "no_template");
}

#[test]
fn template_upgrade_with_an_old_lock_only_proposes_what_it_cannot_render() {
    let home = setup();
    let id = uniq("legacy");
    library(&home, &id, 1, "\n[params]\nabout = { label = \"Focus?\" }\n", &[("ROLE.md", "You are {{name}}.\nFocus: {{about}}\n")]);
    let path = home.path().join("agents").join(uniq("ag")).to_string_lossy().into_owned();
    let mut r = ApplyRequest { path: Some(path.clone()), ..ApplyRequest::new(&id) };
    r.params.insert("about".into(), "billing".into());
    apply(&r, &HireCaller::Owner).unwrap();
    library(&home, &id, 2, "\n[params]\nabout = { label = \"Focus?\" }\n", &[("ROLE.md", "You are {{name}}, v2.\nFocus: {{about}}\n")]);
    // With the recorded values the param survives the upgrade.
    upgrade(&path, None, "user:sam", false, false).unwrap();
    assert!(rd(&path, ".k2/agent/ROLE.md").contains("v2.\nFocus: billing"), "{}", rd(&path, ".k2/agent/ROLE.md"));
    // An old lock (no values): the charter is only proposed.
    let path2 = home.path().join("agents").join(uniq("ag")).to_string_lossy().into_owned();
    library(&home, &id, 3, "\n[params]\nabout = { label = \"Focus?\" }\n", &[("ROLE.md", "You are {{name}}, v3.\nFocus: {{about}}\n")]);
    let mut r2 = ApplyRequest { path: Some(path2.clone()), template: id.clone(), ..Default::default() };
    r2.params.insert("about".into(), "support".into());
    apply(&r2, &HireCaller::Owner).unwrap();
    let mut old = lock::read(&path2).unwrap().unwrap();
    old.vars.clear();
    old.version = 2;
    lock::write(&path2, &old).unwrap();
    let out = upgrade(&path2, None, "user:sam", false, false).unwrap();
    assert_eq!(status(&out, "ROLE.md"), Some("keep"), "{:?}", out.rows);
    assert!(rd(&path2, ".k2/agent/ROLE.md").contains("Focus: support"));
    assert!(Path::new(&path2).join(".k2/agent/ROLE.md.proposed").is_file());
}

#[test]
fn template_line_diff_marks_changes() {
    assert_eq!(line_diff("a\nb\nc\n", "a\nB\nc\n"), "@@ line 2\n+B\n-b");
    assert_eq!(line_diff("same\n", "same\n"), "");
}
