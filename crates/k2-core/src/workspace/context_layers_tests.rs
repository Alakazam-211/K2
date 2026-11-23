use super::*;
use std::fs;
use uuid::Uuid;

fn unique_root(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "k2-ctx-layers-{}-{}-{}",
        tag,
        std::process::id(),
        Uuid::new_v4()
    ));
    fs::create_dir_all(p.join(".k2/agent")).unwrap();
    fs::create_dir_all(p.join(".k2/wiki")).unwrap();
    p
}

fn register_project(path: &str) -> String {
    let db = crate::db::shared();
    let conn = db.lock();
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
        params![id, "ctx-test", path],
    )
    .expect("insert project");
    id
}

fn cleanup_project(path: &str, project_id: &str) {
    let db = crate::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "DELETE FROM project_context_layers WHERE project_id = ?1",
        params![project_id],
    );
    let _ = conn.execute("DELETE FROM projects WHERE id = ?1", params![project_id]);
    let _ = fs::remove_dir_all(path);
}

#[test]
fn list_empty_stack_matches_pinned_only() {
    let root = unique_root("empty");
    let path = root.to_str().unwrap();
    let pid = register_project(path);

    let stack = list_stack(path).expect("list_stack");
    assert!(stack.layers.is_empty(), "no optional layers yet");
    assert_eq!(stack.pinned.len(), 3);
    assert_eq!(stack.pinned[0].label, "Role (persona)");
    assert!(
        stack.pinned[0].path.ends_with("ROLE.md"),
        "default pinned path is ROLE.md, got {}",
        stack.pinned[0].path
    );
    assert!(stack.pinned[1].label.contains("Project"));
    assert!(stack.pinned[2].label.contains("Tooling"));
    assert!(stack.pinned[2].generated);
    let tooling = stack.pinned[2]
        .preview
        .as_deref()
        .expect("tooling preview");
    assert!(
        tooling.starts_with("## Tooling"),
        "preview must be the AGENTS.md section, got {tooling:?}"
    );
    assert!(
        tooling.contains("k2-cli") && tooling.contains(".k2/skills/k2-cli/SKILL.md"),
        "preview must show the k2-cli pointer body"
    );
    assert_eq!(
        tooling,
        crate::workspace::skill_regen::AGENTS_MD_TOOLING_SECTION.trim_end()
    );
    assert!(stack.pinned[0].enabled);
    assert!(stack.pinned[1].enabled);
    assert!(stack.pinned[2].enabled);
    assert!(!stack.soft_warn);

    cleanup_project(path, &pid);
}

#[test]
fn add_path_layer_roundtrip_and_duplicate() {
    let root = unique_root("add");
    let path = root.to_str().unwrap();
    let pid = register_project(path);

    let layer_file = root.join("docs/arch.md");
    fs::create_dir_all(layer_file.parent().unwrap()).unwrap();
    fs::write(&layer_file, "# Architecture\n\nDiagrams live here.\n").unwrap();

    let layer = add_layer(path, Some("docs/arch.md"), None, Some("Arch")).expect("add layer");
    assert_eq!(layer.path, "docs/arch.md");
    assert!(layer.enabled);
    assert_eq!(layer.position, 0);
    assert_eq!(layer.source, "user");
    assert_eq!(layer.label.as_deref(), Some("Arch"));
    assert!(layer.exists);
    assert!(layer.bytes > 0);

    let err =
        add_layer(path, Some("docs/arch.md"), None, None).expect_err("duplicate must fail");
    assert_eq!(err.code(), "duplicate_layer");

    let stack = list_stack(path).unwrap();
    assert_eq!(stack.layers.len(), 1);

    remove_layer(path, &layer.id).expect("remove");
    assert!(list_layers(path).unwrap().is_empty());

    cleanup_project(path, &pid);
}

#[test]
fn path_escape_rejected() {
    let root = unique_root("escape");
    let path = root.to_str().unwrap();
    let pid = register_project(path);

    let err =
        add_layer(path, Some("../../etc/passwd"), None, None).expect_err("escape must fail");
    assert_eq!(err.code(), "path_escape", "got {err}");

    cleanup_project(path, &pid);
}

#[test]
fn catalog_wiki_index_adds_and_rewrites_source() {
    let root = unique_root("catalog");
    let path = root.to_str().unwrap();
    let pid = register_project(path);

    fs::write(
        root.join(".k2/wiki/_Index.md"),
        "# Wiki Index\n\n- note a\n",
    )
    .unwrap();

    let layer = add_layer(path, None, Some("wiki:index"), None).expect("catalog add");
    assert_eq!(layer.source, "catalog:wiki-index");
    assert_eq!(layer.path, ".k2/wiki/_Index.md");
    assert_eq!(layer.label.as_deref(), Some("Wiki index"));
    assert!(layer.exists);

    let err = add_layer(path, None, Some("wiki:nope"), None).expect_err("unknown");
    assert_eq!(err.code(), "catalog_unknown");

    cleanup_project(path, &pid);
}

#[test]
fn set_enabled_and_move_reorder() {
    let root = unique_root("move");
    let path = root.to_str().unwrap();
    let pid = register_project(path);

    for (name, body) in [("a.md", "# A\n"), ("b.md", "# B\n"), ("c.md", "# C\n")] {
        fs::write(root.join(name), body).unwrap();
        add_layer(path, Some(name), None, None).unwrap();
    }

    let layers = list_layers(path).unwrap();
    assert_eq!(layers.len(), 3);
    let id_a = layers[0].id.clone();
    let id_c = layers[2].id.clone();

    set_enabled(path, &id_a, false).unwrap();
    let a = get_layer(path, &id_a).unwrap();
    assert!(!a.enabled);

    // Move C to top.
    move_layer(path, &id_c, Some(0), None).unwrap();
    let layers = list_layers(path).unwrap();
    assert_eq!(layers[0].path, "c.md");
    assert_eq!(layers[0].position, 0);

    // Direction down.
    move_layer(path, &id_c, None, Some("down")).unwrap();
    let layers = list_layers(path).unwrap();
    assert_eq!(layers[1].path, "c.md");

    cleanup_project(path, &pid);
}

#[test]
fn unregistered_project_fails_loud() {
    let err = list_stack("/tmp/k2-ctx-definitely-not-registered-xyz").expect_err("must fail");
    assert_eq!(err.code(), "not_found");
}

#[test]
fn list_catalog_has_wiki_entries() {
    crate::tunnel::test_support::with_temp_home(|| {
    let catalog = list_catalog();
    assert_eq!(
        catalog.len(),
        13,
        "thirteen builtins only in an empty host catalog; got {:?}",
        catalog.iter().map(|p| p.id.as_str()).collect::<Vec<_>>()
    );
    assert!(catalog.iter().any(|p| p.id == "wiki:index"));
    assert!(catalog.iter().any(|p| p.id == "wiki:home"));
    assert!(catalog.iter().any(|p| p.id == WIKI_HYGIENE_ID));
    assert!(catalog.iter().any(|p| p.id == SUBAGENTS_PACK_ID));
    assert!(catalog.iter().any(|p| p.id == "manager:pack"));
    assert!(catalog.iter().any(|p| p.id == KESSEL_PACK_ID));
    assert!(!catalog.iter().any(|p| p.id == "k2:pack"), "k2:pack is an alias of kessel:pack since 0.46.12");
    assert!(catalog.iter().any(|p| p.id == CONNECTIONS_ROSTER_ID));
    assert!(catalog.iter().any(|p| p.id == HEARTBEATS_ROSTER_ID));
    assert!(catalog.iter().any(|p| p.id == SKILLS_ROSTER_ID));
    let users = catalog
        .iter()
        .find(|p| p.id == USERS_ROSTER_ID)
        .expect("users:roster catalog entry");
    assert_eq!(users.kind, "live");
    assert!(!users.recommended, "users:roster is opt-in");
    assert_eq!(users.label, "User roster");
    assert_eq!(users.source, USERS_ROSTER_SOURCE);
    assert_eq!(users.path, USERS_ROSTER_PATH);
    let skin = catalog
        .iter()
        .find(|p| p.id == APPS_ROSTER_ID)
        .expect("apps:roster catalog entry");
    assert_eq!(skin.kind, "live");
    assert!(!skin.recommended, "apps:roster is opt-in");
    assert_eq!(skin.label, "App guest roster");
    assert_eq!(skin.source, SKIN_ROSTER_SOURCE);
    assert_eq!(skin.path, SKIN_ROSTER_PATH);
    assert!(
        catalog.iter().all(|p| p.id != SKIN_ROSTER_ID),
        "leftover skin:roster is an alias, not a second catalog row"
    );
    for p in &catalog {
        assert!(
            matches!(p.kind.as_str(), "live" | "static" | "path"),
            "catalog entry {} bad kind {}",
            p.id,
            p.kind
        );
        assert!(
            p.description
                .as_ref()
                .map(|d| !d.is_empty())
                .unwrap_or(false),
            "catalog entry {} missing description",
            p.id
        );
        assert_eq!(p.author.as_deref(), Some("K2"));
        if p.kind == "static" || p.kind == "path" {
            assert_eq!(
                p.version.as_deref(),
                Some("1.0.0"),
                "static/path need version"
            );
        }
        if p.kind == "live" {
            assert!(p.version.is_none(), "live packs have no version pin");
        }
        assert!(
            !p.tags.is_empty(),
            "catalog entry {} should have tags",
            p.id
        );
        assert!(
            !p.tags.iter().any(|t| t.eq_ignore_ascii_case("recommended")),
            "recommended must be a boolean field, not a free-form tag ({})",
            p.id
        );
    }
    let recommended: Vec<_> = catalog
        .iter()
        .filter(|p| p.recommended)
        .map(|p| p.id.as_str())
        .collect();
    assert!(recommended.contains(&"wiki:index"));
    assert!(recommended.contains(&"wiki:hygiene"));
    assert!(recommended.contains(&SUBAGENTS_PACK_ID));
    assert!(recommended.contains(&CONNECTIONS_ROSTER_ID));
    assert!(recommended.contains(&HEARTBEATS_ROSTER_ID));
    assert!(!recommended.contains(&"skills:roster"));
    assert!(!recommended.contains(&USERS_ROSTER_ID));
    assert!(!recommended.contains(&"users:roster"));
    assert!(!recommended.contains(&APPS_ROSTER_ID));
    assert!(!recommended.contains(&SKIN_ROSTER_ID));
    assert!(!recommended.contains(&"apps:roster"));
    assert!(!recommended.contains(&"skin:roster"));
    assert!(!recommended.contains(&"manager:pack"));
    });
}

#[test]
fn connections_roster_body_notes_humans_and_users_flag() {
    let root = unique_root("roster-users");
    let path = root.to_str().unwrap();
    let pid = register_project(path);

    let body = render_connections_roster_body(path);
    assert!(
        body.contains("k2 connections list --users"),
        "roster must teach --users; body:\n{body}"
    );
    assert!(
        body.contains("Do **not** `k2 msg` those names")
            || body.contains("do not `k2 msg` those names"),
        "roster must say do not k2 msg humans; body:\n{body}"
    );
    assert!(
        body.contains("not** listed") || body.contains("not listed"),
        "roster must say humans are not listed; body:\n{body}"
    );
    assert!(
        body.contains("k2 msg <workspace-name>"),
        "roster must still teach k2 msg for peers; body:\n{body}"
    );
    assert!(
        body.contains("users:roster"),
        "roster may point at User roster catalog id; body:\n{body}"
    );
    assert!(
        !body.contains("| USERNAME |"),
        "connections roster must not grow a human username table; body:\n{body}"
    );

    cleanup_project(path, &pid);
}

#[test]
fn connections_roster_catalog_materializes_and_composes_live() {
    let root = unique_root("roster");
    let path = root.to_str().unwrap();
    let _pid = register_project(path);

    let layer = add_layer(path, None, Some("connections:roster"), None).expect("roster");
    assert_eq!(layer.source, CONNECTIONS_ROSTER_SOURCE);
    assert_eq!(layer.path, CONNECTIONS_ROSTER_PATH);
    assert!(layer.exists);
    let abs = root.join(".k2/context/catalog/connections-roster.md");
    assert!(abs.is_file(), "roster file must materialize");
    let body = fs::read_to_string(&abs).unwrap();
    assert!(
        body.contains("No connected agents yet") || body.contains("Local connections"),
        "roster should describe connections; first 300:\n{}",
        &body[..body.len().min(300)]
    );

    // Compose must include the roster section even if the file is deleted
    // (live generation path).
    fs::remove_file(&abs).ok();
    let composed = show_composed(path).expect("compose");
    assert!(
        composed.contains("Connected agents roster")
            || composed.contains("No connected agents yet"),
        "composed AGENTS.md must inline live roster; first 500:\n{}",
        &composed[..composed.len().min(500)]
    );

    cleanup_project(path, &_pid);
}

#[test]
fn users_roster_catalog_is_live_not_recommended_and_roster_alias_stays_connections() {
    crate::tunnel::test_support::with_temp_home(|| {
    let catalog = list_catalog();
    let users = catalog
        .iter()
        .find(|p| p.id == USERS_ROSTER_ID)
        .expect("users:roster");
    assert_eq!(users.kind, "live");
    assert!(!users.recommended);
    assert_eq!(users.label, "User roster");

    let root = unique_root("roster-alias");
    let path = root.to_str().unwrap();
    let pid = register_project(path);
    let layer = add_layer(path, None, Some("roster"), None).expect("bare roster");
    assert_eq!(
        layer.source, CONNECTIONS_ROSTER_SOURCE,
        "bare roster must stay connections:roster"
    );
    assert_eq!(layer.path, CONNECTIONS_ROSTER_PATH);
    let people = add_layer(path, None, Some("people"), None).expect("people alias");
    assert_eq!(people.source, USERS_ROSTER_SOURCE);
    cleanup_project(path, &pid);
    });
}

#[test]
fn users_roster_catalog_materializes_and_composes_live() {
    crate::tunnel::test_support::with_temp_home(|| {
        let root = unique_root("users-roster");
        let path = root.to_str().unwrap();
        let pid = register_project(path);

        let layer = add_layer(path, None, Some("users:roster"), None).expect("users roster");
        assert_eq!(layer.source, USERS_ROSTER_SOURCE);
        assert_eq!(layer.path, USERS_ROSTER_PATH);
        assert!(layer.exists);
        let abs = root.join(".k2/context/catalog/users-roster.md");
        assert!(abs.is_file(), "users roster file must materialize");
        let file_body = fs::read_to_string(&abs).unwrap();
        assert!(
            file_body.contains("USERNAME") && file_body.contains("ROLE"),
            "roster file should be a people table; first 300:\n{}",
            &file_body[..file_body.len().min(300)]
        );

        fs::remove_file(&abs).ok();
        let composed = show_composed(path).expect("compose");
        assert!(
            composed.contains("User roster") && composed.contains("USERNAME"),
            "composed AGENTS.md must inline live user roster; first 500:\n{}",
            &composed[..composed.len().min(500)]
        );

        cleanup_project(path, &pid);
    });
}

#[test]
fn skin_roster_catalog_is_live_not_recommended_and_alias() {
    crate::tunnel::test_support::with_temp_home(|| {
        let catalog = list_catalog();
        let skin = catalog
            .iter()
            .find(|p| p.id == APPS_ROSTER_ID)
            .expect("apps:roster");
        assert_eq!(skin.kind, "live");
        assert!(!skin.recommended);
        assert_eq!(skin.label, "App guest roster");

        let root = unique_root("skin-roster-alias");
        let path = root.to_str().unwrap();
        let pid = register_project(path);
        let layer = add_layer(path, None, Some("skin-users"), None).expect("skin-users alias");
        assert_eq!(layer.source, SKIN_ROSTER_SOURCE);
        assert_eq!(layer.path, SKIN_ROSTER_PATH);
        cleanup_project(path, &pid);

        let root_left = unique_root("skin-roster-leftover");
        let path_left = root_left.to_str().unwrap();
        let pid_left = register_project(path_left);
        let leftover =
            add_layer(path_left, None, Some("skin:roster"), None).expect("skin:roster leftover");
        assert_eq!(leftover.source, SKIN_ROSTER_SOURCE);
        assert_eq!(leftover.path, SKIN_ROSTER_PATH);
        cleanup_project(path_left, &pid_left);

        let root_canon = unique_root("apps-roster-canonical");
        let path_canon = root_canon.to_str().unwrap();
        let pid_canon = register_project(path_canon);
        let canonical =
            add_layer(path_canon, None, Some("apps:roster"), None).expect("apps:roster");
        assert_eq!(canonical.source, SKIN_ROSTER_SOURCE);
        assert_eq!(canonical.path, SKIN_ROSTER_PATH);
        cleanup_project(path_canon, &pid_canon);
    });
}

#[test]
fn skin_roster_catalog_materializes_and_composes_live() {
    crate::tunnel::test_support::with_temp_home(|| {
        let root = unique_root("skin-roster");
        let path = root.to_str().unwrap();
        let pid = register_project(path);

        let layer = add_layer(path, None, Some("skin:roster"), None).expect("skin roster leftover");
        assert_eq!(layer.source, SKIN_ROSTER_SOURCE);
        assert_eq!(layer.path, SKIN_ROSTER_PATH);
        assert!(layer.exists);
        let abs = root.join(".k2/context/catalog/skin-roster.md");
        assert!(abs.is_file(), "skin roster file must materialize");
        let file_body = fs::read_to_string(&abs).unwrap();
        assert!(
            file_body.contains("No app users yet") && file_body.contains("k2 app user add"),
            "empty roster should teach add; first 400:\n{}",
            &file_body[..file_body.len().min(400)]
        );

        crate::skin::add_principal("ghostbird").expect("add guest");
        let room = uuid::Uuid::new_v4().to_string();
        let (_meta, raw) =
            crate::skin::create_token("ghostbird", None, std::slice::from_ref(&room))
                .expect("mint");
        assert!(raw.starts_with("k2skn_"), "secret prefix");

        fs::remove_file(&abs).ok();
        let composed = show_composed(path).expect("compose");
        assert!(
            composed.contains("App guest roster")
                && composed.contains("ghostbird")
                && composed.contains("USERNAME")
                && composed.contains("ROLE")
                && composed.contains("PASSWORD")
                && !composed.contains("LIVE KEYS"),
            "composed AGENTS.md must inline guest roster (not live keys); first 600:\n{}",
            &composed[..composed.len().min(600)]
        );
        assert!(
            !composed.contains(&raw),
            "must not leak the raw secret; first 600:\n{}",
            &composed[..composed.len().min(600)]
        );
        let roster_start = composed
            .find("## App guest roster")
            .expect("roster heading");
        let roster_rest = &composed[roster_start..];
        let roster_end = roster_rest[2..]
            .find("\n## ")
            .map(|i| i + 2)
            .unwrap_or(roster_rest.len());
        let roster = &roster_rest[..roster_end];
        assert!(
            !roster.contains("k2skn_"),
            "roster table must not show key prefix; roster:\n{roster}"
        );

        cleanup_project(path, &pid);
    });
}

#[test]
fn render_skin_roster_body_teaches_cli_and_no_msg() {
    crate::tunnel::test_support::with_temp_home(|| {
        let body = render_skin_roster_body("/unused");
        assert!(
            body.contains("Do **not** `k2 msg`") || body.contains("do not `k2 msg`"),
            "must say do not k2 msg skin guests; body:\n{body}"
        );
        assert!(
            body.contains("k2 app user list"),
            "must teach k2 app user list; body:\n{body}"
        );
        assert!(
            body.contains("k2 skin user list"),
            "must keep leftover k2 skin user list; body:\n{body}"
        );
        assert!(
            body.contains("k2 app token create --name"),
            "must teach platform mint --name, not <username>; body:\n{body}"
        );
        assert!(
            body.contains("k2 skin-token create --name"),
            "must keep leftover k2 skin-token create --name; body:\n{body}"
        );
        assert!(
            !body.contains("k2 skin-token create <username>"),
            "must not teach mint-for-user; body:\n{body}"
        );
        assert!(
            body.contains("not Connect"),
            "must distinguish from Connect users; body:\n{body}"
        );
    });
}

#[test]
fn skin_roster_not_in_default_empty_stack() {
    let root = unique_root("skin-empty-stack");
    let path = root.to_str().unwrap();
    let pid = register_project(path);

    let stack = list_stack(path).expect("list_stack");
    assert!(
        !stack.layers.iter().any(|l| l.source == SKIN_ROSTER_SOURCE
            || l.path.ends_with("skin-roster.md")),
        "skin:roster must not be seeded"
    );
    let composed = show_composed(path).expect("compose");
    assert!(
        !composed.contains("## App guest roster"),
        "default compose must not inline App guest roster; first 400:\n{}",
        &composed[..composed.len().min(400)]
    );

    cleanup_project(path, &pid);
}

fn set_test_owner_display(name: &str) {
    let mut s = crate::app_settings::load();
    s.owner_display_name = Some(name.to_string());
    crate::app_settings::save(&s).expect("save owner display");
}

#[test]
fn render_users_roster_body_owner_member_viewer_disabled_no_secrets() {
    crate::tunnel::test_support::with_temp_home(|| {
        set_test_owner_display("Rosson");
        let empty = render_users_roster_body("/unused");
        assert!(
            empty.contains("| owner | owner | no |"),
            "empty store must still list host owner; body:\n{empty}"
        );
        assert!(
            !empty.contains("password")
                && !empty.contains("hash")
                && !empty.contains("token_epoch"),
            "empty roster leaked secrets; body:\n{empty}"
        );

        crate::connect_users::add_user("julie", "password1").expect("add julie");
        crate::connect_users::set_role("julie", crate::connect_users::Role::Admin)
            .expect("promote");
        crate::connect_users::add_user("member1", "password1").expect("add member");
        crate::connect_users::add_user("viewy", "password1").expect("add second member");
        crate::connect_users::add_user("ghost", "password1").expect("add ghost");
        crate::connect_users::set_disabled("ghost", true).expect("disable");

        let body = render_users_roster_body("/unused");
        assert!(body.contains("| owner | owner | no |"), "missing owner (ownerDisplayName is not read); body:\n{body}");
        assert!(
            body.contains("julie") && body.contains("admin"),
            "missing admin; body:\n{body}"
        );
        assert!(
            body.contains("member1") && body.contains("member"),
            "missing member; body:\n{body}"
        );
        assert!(
            body.contains("viewy") && !body.contains("viewer"),
            "second member must be listed with no viewer role; body:\n{body}"
        );
        assert!(
            body.contains("ghost") && body.contains("yes"),
            "disabled user must appear; body:\n{body}"
        );
        let lower = body.to_ascii_lowercase();
        assert!(
            !lower.contains("password")
                && !body.contains("hash")
                && !body.contains("token_epoch")
                && !body.contains("tokenEpoch"),
            "people table must not leak secrets; body:\n{body}"
        );
    });
}

/// 0.46.0: a person's real name follows the username; nothing changes
/// for people without one.
#[test]
fn render_users_roster_body_shows_real_names_beside_usernames() {
    crate::tunnel::test_support::with_temp_home(|| {
        set_test_owner_display("Rosson");
        crate::connect_users::add_user("julie", "password1").expect("add julie");
        crate::connect_users::add_user("member1", "password1").expect("add member");
        let before = render_users_roster_body("/unused");
        assert!(before.contains("| julie | member | no |"), "body:\n{before}");
        assert!(before.contains("| owner | owner | no |"), "ownerDisplayName is not read; body:\n{before}");
        assert!(!before.contains("| julie —"), "no names → unchanged; body:\n{before}");
        assert!(!before.contains("use the username"), "no names → no extra note; body:\n{before}");

        crate::connect_users::set_names("julie", Some("Julie"), Some("Park")).expect("names");
        let body = render_users_roster_body("/unused");
        assert!(body.contains("| julie — Julie Park | member | no |"), "body:\n{body}");
        assert!(body.contains("| owner | owner | no |"), "nothing linked: the owner row; body:\n{body}");
        // Linked: the owner is that login, with its name.
        crate::connect_users::add_user("z3thon", "password1").expect("add z3thon");
        crate::connect_users::set_role("z3thon", crate::connect_users::Role::Owner).expect("owner");
        crate::connect_users::set_names("z3thon", Some("Rosson"), Some("Long")).expect("names");
        crate::app_settings::update(serde_json::json!({ "ownerAccounts": ["z3thon"] })).expect("link");
        let body = render_users_roster_body("/unused");
        assert!(body.contains("| z3thon — Rosson Long | owner | no |"), "body:\n{body}");
        assert!(!body.contains("Rosson |"), "ownerDisplayName is not read; body:\n{body}");
        assert!(body.contains("| member1 | member | no |"), "nameless rows unchanged; body:\n{body}");
        assert!(body.contains("use the username"), "teaches that names are context only; body:\n{body}");
    });
}

#[test]
fn render_users_roster_body_teaches_from_stamp_and_users_flag() {
    crate::tunnel::test_support::with_temp_home(|| {
        let body = render_users_roster_body("/unused");
        assert!(
            body.contains("Do **not** `k2 msg`") || body.contains("do not `k2 msg`"),
            "must say do not k2 msg humans; body:\n{body}"
        );
        assert!(
            body.contains("[from <name>]"),
            "must mention [from <name>] stamps; body:\n{body}"
        );
        assert!(
            body.contains("k2 connections list --users") || body.contains("--users"),
            "must teach --users lookup; body:\n{body}"
        );
    });
}

#[test]
fn users_roster_not_in_default_empty_stack() {
    let root = unique_root("users-empty-stack");
    let path = root.to_str().unwrap();
    let pid = register_project(path);

    let stack = list_stack(path).expect("list_stack");
    assert!(
        stack.layers.is_empty(),
        "default empty stack must have no optional layers"
    );
    assert!(
        !stack.layers.iter().any(|l| l.source == USERS_ROSTER_SOURCE
            || l.path.ends_with("users-roster.md")),
        "users:roster must not be seeded"
    );
    let composed = show_composed(path).expect("compose");
    assert!(
        !composed.contains("## User roster"),
        "default compose must not inline User roster; first 400:\n{}",
        &composed[..composed.len().min(400)]
    );

    cleanup_project(path, &pid);
}

#[test]
fn wiki_hygiene_and_live_rosters_materialize() {
    let root = unique_root("hygiene-rosters");
    let path = root.to_str().unwrap();
    let _pid = register_project(path);

    let hygiene = add_layer(path, None, Some("wiki:hygiene"), None).expect("hygiene");
    assert_eq!(hygiene.source, WIKI_HYGIENE_SOURCE);
    let hygiene_body =
        fs::read_to_string(root.join(".k2/context/catalog/wiki-hygiene.md")).unwrap();
    assert!(
        hygiene_body.contains("Wiki hygiene") && hygiene_body.contains(".k2/wiki/"),
        "hygiene pack body unexpected: {}",
        &hygiene_body[..hygiene_body.len().min(200)]
    );

    let hb = add_layer(path, None, Some("heartbeats:roster"), None).expect("hb roster");
    assert_eq!(hb.source, HEARTBEATS_ROSTER_SOURCE);
    assert!(root
        .join(".k2/context/catalog/heartbeats-roster.md")
        .is_file());

    let sk = add_layer(path, None, Some("skills:roster"), None).expect("skills roster");
    assert_eq!(sk.source, SKILLS_ROSTER_SOURCE);
    assert!(root.join(".k2/context/catalog/skills-roster.md").is_file());

    let composed = show_composed(path).expect("compose");
    assert!(
        composed.contains("Wiki hygiene")
            && composed.contains("Heartbeats roster")
            && composed.contains("Skills roster"),
        "compose missing new packs; first 800:\n{}",
        &composed[..composed.len().min(800)]
    );

    cleanup_project(path, &_pid);
}

fn compose_banner_stamp(body: &str) -> &str {
    let marker = "<!-- GENERATED at ";
    let i = body.find(marker).expect("compose banner");
    let rest = &body[i + marker.len()..];
    let end = rest.find(' ').expect("stamp end");
    &rest[..end]
}

#[test]
fn heartbeat_mutate_refreshes_when_roster_stacked_fire_does_not() {
    crate::db::init_for_tests();
    let root = unique_root("hb-roster");
    let path = root.to_str().unwrap();
    let pid = register_project(path);
    crate::workspace::onboarding::set_agents_md_generate_enabled(path, true).unwrap();
    add_layer(path, None, Some("heartbeats:roster"), None).expect("stack hb roster");
    crate::workspace::skill_regen::write_workspace_skill_file(path);

    // HB7: a temp HOME under the shared HOME lock, and zero scheduler
    // calls — this add once replaced the real `dev.k2.heartbeat` job.
    crate::heartbeats::install::with_temp_home("roster-add", |_home| {
        crate::heartbeats::install::test_recorder::take();
        crate::heartbeats::k2_heartbeat_add(
            path.to_string(),
            "roster-hb".into(),
            "daily".into(),
            "{}".into(),
        )
        .expect("add heartbeat");
        assert_eq!(
            crate::heartbeats::install::test_recorder::take(),
            Vec::<String>::new(),
            "heartbeat add must not run launchctl/crontab in tests"
        );
    });

    let cwd = fs::read_to_string(root.join("AGENTS.md")).expect("cwd AGENTS.md");
    assert!(
        cwd.contains("roster-hb"),
        "heartbeat add with roster stacked must refresh AGENTS.md, got:\n{cwd}"
    );
    let stamp = compose_banner_stamp(&cwd).to_string();

    crate::heartbeats::stamp_heartbeat_fired(path, "roster-hb");
    let after_fire = fs::read_to_string(root.join("AGENTS.md")).expect("after fire");
    assert_eq!(
        compose_banner_stamp(&after_fire),
        stamp.as_str(),
        "heartbeat fire must not rewrite the compose banner"
    );

    cleanup_project(path, &pid);
}

#[test]
fn skill_write_refreshes_only_when_skills_roster_stacked() {
    crate::db::init_for_tests();
    let stacked = unique_root("sk-roster-on");
    let stacked_path = stacked.to_str().unwrap();
    let stacked_pid = register_project(stacked_path);
    crate::workspace::onboarding::set_agents_md_generate_enabled(stacked_path, true).unwrap();
    add_layer(stacked_path, None, Some("skills:roster"), None).expect("stack skills");
    crate::workspace::skill_regen::write_workspace_skill_file(stacked_path);

    crate::skills::crud::create(stacked_path, "roster-skill", None).expect("create skill");
    let stacked_body =
        fs::read_to_string(stacked.join("AGENTS.md")).expect("stacked cwd AGENTS.md");
    assert!(
        stacked_body.contains("roster-skill"),
        "skill write with roster stacked must refresh AGENTS.md, got:\n{stacked_body}"
    );

    let bare = unique_root("sk-roster-off");
    let bare_path = bare.to_str().unwrap();
    let bare_pid = register_project(bare_path);
    crate::workspace::onboarding::set_agents_md_generate_enabled(bare_path, true).unwrap();
    crate::workspace::skill_regen::write_workspace_skill_file(bare_path);
    let before = fs::read_to_string(bare.join("AGENTS.md")).expect("bare cwd");
    let stamp = compose_banner_stamp(&before).to_string();
    crate::skills::crud::create(bare_path, "quiet-skill", None).expect("create without layer");
    let after = fs::read_to_string(bare.join("AGENTS.md")).expect("bare after");
    assert_eq!(
        compose_banner_stamp(&after),
        stamp.as_str(),
        "skill write without the skills roster must not restamp AGENTS.md"
    );

    cleanup_project(stacked_path, &stacked_pid);
    cleanup_project(bare_path, &bare_pid);
}

#[test]
fn pack_catalog_materialize_on_first_add() {
    let root = unique_root("pack");
    let path = root.to_str().unwrap();
    let _pid = register_project(path);

    let manager_abs = root.join(".k2/context/catalog/manager.md");
    assert!(!manager_abs.exists());

    let layer = add_layer(path, None, Some("manager:pack"), None).expect("manager pack");
    assert_eq!(layer.source, "catalog:manager");
    assert_eq!(layer.path, ".k2/context/catalog/manager.md");
    assert!(manager_abs.is_file(), "manager pack must materialize");
    assert!(layer.exists);
    let body = fs::read_to_string(&manager_abs).unwrap();
    assert!(
        body.contains("always-on") || body.contains("standing"),
        "pack should be lean always-on orientation, got len={}",
        body.len()
    );
    assert!(
        body.len() < 4_000,
        "always-on pack must stay small (not full skill dump), len={}",
        body.len()
    );
    // Full skill remains loadable for depth.
    assert!(
        root.join(".k2/skills/workspace-manager/SKILL.md").is_file()
            || root
                .join(".k2so/skills/workspace-manager/SKILL.md")
                .is_file(),
        "loadable workspace-manager skill should be ensured on pack add"
    );

    let err = add_layer(path, None, Some("manager:pack"), None).expect_err("dup");
    assert_eq!(err.code(), "duplicate_layer");

    // 0.46.12 AT9: the old K2 Agent alias stacks Kessel's pack.
    let k2_layer = add_layer(path, None, Some("k2-agent"), None).expect("k2 alias");
    assert_eq!(k2_layer.source, KESSEL_PACK_SOURCE);
    assert_eq!(k2_layer.path, KESSEL_PACK_PATH);
    assert!(root.join(KESSEL_PACK_PATH).is_file());
    assert!(!root.join(".k2/context/catalog/k2-agent.md").exists(), "a new add never writes the old file");
    let k2_body = fs::read_to_string(root.join(KESSEL_PACK_PATH)).unwrap();
    assert!(k2_body.starts_with("# Kessel: K2's own agent"), "Kessel pack body: {k2_body}");
    assert!(
        k2_body.len() < 4_000,
        "kessel pack must stay lean, len={}",
        k2_body.len()
    );

    cleanup_project(path, &_pid);
}

/// prd-agent-templates-v1 AT9: `k2:pack` and every old K2 Agent
/// spelling resolve to `kessel:pack`; adding one after another is the same
/// layer (a duplicate), not a second one.
#[test]
fn k2_pack_alias_resolves_to_kessel_pack() {
    // Literals on purpose: the same test runs red on a K2 without Kessel's pack.
    for alias in ["k2:pack", "k2-agent", "k2", "k2so-agent", "catalog:k2-agent", "kessel", "catalog:kessel", "kessel:pack"] {
        let e = resolve_catalog_id(alias).unwrap_or_else(|err| panic!("{alias}: {err}"));
        assert_eq!(e.id, "kessel:pack", "{alias}");
        assert_eq!(e.source, "catalog:kessel", "{alias}");
        assert_eq!(e.path, ".k2/context/catalog/kessel.md", "{alias}");
    }
    let root = unique_root("kesselalias");
    let path = root.to_str().unwrap();
    let pid = register_project(path);
    let first = add_layer(path, None, Some("k2:pack"), None).expect("k2:pack");
    assert_eq!(first.source, "catalog:kessel");
    let err = add_layer(path, None, Some("kessel:pack"), None).expect_err("same layer twice");
    assert_eq!(err.code(), "duplicate_layer");
    cleanup_project(path, &pid);
}

#[test]
fn missing_layer_file_still_lists_with_exists_false() {
    let root = unique_root("missing");
    let path = root.to_str().unwrap();
    let pid = register_project(path);

    // Insert directly so we can reference a missing file without
    // needing the path to exist at add time (normalize still works).
    let layer = add_layer(path, Some("docs/gone.md"), None, None).unwrap();
    assert!(!layer.exists);
    assert_eq!(layer.bytes, 0);

    cleanup_project(path, &pid);
}

#[test]
fn bad_usage_both_path_and_catalog() {
    let root = unique_root("both");
    let path = root.to_str().unwrap();
    let pid = register_project(path);

    let err =
        add_layer(path, Some("a.md"), Some("wiki:index"), None).expect_err("both is bad_usage");
    assert_eq!(err.code(), "bad_usage");

    let err = add_layer(path, None, None, None).expect_err("neither");
    assert_eq!(err.code(), "bad_usage");

    cleanup_project(path, &pid);
}

#[test]
fn pinned_info_persona_old_new_and_both() {
    let root = unique_root("persona-pin");
    let path = root.to_str().unwrap();
    let pid = register_project(path);
    let dir = crate::workspace::agent_identity::workspace_agent_path(path);
    fs::create_dir_all(&dir).unwrap();

    fs::write(dir.join("AGENT.md"), "---\nname: old\n---\nold\n").unwrap();
    let pin = pinned_info(path);
    assert_eq!(pin[0].id, "pinned:agent");
    assert_eq!(pin[0].label, "Role (persona)");
    assert!(
        pin[0].path.ends_with("agent/AGENT.md"),
        "got {}",
        pin[0].path
    );
    assert!(pin[0].exists);

    fs::write(dir.join("ROLE.md"), "---\nname: neu\n---\nnew\n").unwrap();
    let pin = pinned_info(path);
    assert!(
        pin[0].path.ends_with("agent/ROLE.md"),
        "ROLE.md wins: {}",
        pin[0].path
    );
    assert!(pin[0].exists);

    fs::remove_file(dir.join("AGENT.md")).unwrap();
    let pin = pinned_info(path);
    assert!(pin[0].path.ends_with("agent/ROLE.md"));
    assert!(pin[0].exists);

    cleanup_project(path, &pid);
}

#[test]
fn create_user_pack_scans_as_static_not_recommended() {
    crate::tunnel::test_support::with_temp_home(|| {
        let created = create_user_pack("user:on-call", Some("On-call"), &["ops".into()])
            .expect("create user pack");
        assert_eq!(created.entry.id, "user:on-call");
        assert_eq!(created.entry.source, "catalog:user:on-call");
        assert_eq!(created.entry.kind, "static");
        assert!(!created.entry.recommended);
        assert_eq!(created.entry.tags, vec!["ops".to_string()]);
        assert_eq!(created.entry.path, ".k2/context/catalog/on-call.md");
        assert!(created.dir.join("pack.toml").is_file());
        assert!(created.dir.join("layer.md").is_file());
        let toml_body = fs::read_to_string(created.dir.join("pack.toml")).unwrap();
        assert!(
            !toml_body.contains("kind = \"live\""),
            "create must never write kind=live: {toml_body}"
        );
        assert!(
            !toml_body.to_ascii_lowercase().contains("recommended"),
            "create must not write recommended: {toml_body}"
        );
        let layer = fs::read_to_string(created.dir.join("layer.md")).unwrap();
        assert!(!layer.trim_start().starts_with("# "), "layer.md must have no H1");

        let catalog = list_catalog();
        let hit = catalog
            .iter()
            .find(|p| p.id == "user:on-call")
            .expect("scan must include created pack");
        assert_eq!(hit.kind, "static");
        assert!(!hit.recommended);
        assert_eq!(hit.source, "catalog:user:on-call");
        assert_eq!(hit.tags, vec!["ops".to_string()]);
        assert_eq!(catalog.iter().filter(|p| p.id.starts_with("wiki:") || p.id.ends_with(":roster") || p.id.ends_with(":pack")).count(), 12);
    });
}

#[test]
fn create_user_pack_rejects_reserved_ids() {
    crate::tunnel::test_support::with_temp_home(|| {
        for id in [
            "connections:roster",
            "roster",
            "users:foo",
            "people",
            "manager",
            "k2-agent",
            "kessel",
            "kessel:pack",
            "kessel:mine",
            "wiki-hygiene",
            "users-roster",
            "apps:roster",
            "skin:roster",
            "skin-users",
        ] {
            let err = create_user_pack(id, None, &[]).expect_err(id);
            assert_eq!(err.code(), "bad_usage", "{id} → {}", err);
        }
    });
}

#[test]
fn create_user_pack_rejects_kind_live() {
    crate::tunnel::test_support::with_temp_home(|| {
        let err = create_user_pack_inner("user:live-try", None, &[], Some("live"), None)
            .expect_err("kind=live");
        assert_eq!(err.code(), "bad_usage");
        assert!(
            err.hint().contains("live"),
            "hint should mention live: {}",
            err.hint()
        );
        let err = parse_user_pack_toml(
            "id = \"user:x\"\nname = \"X\"\nkind = \"live\"\nlicense = \"MIT\"\nauthor = \"u\"\ntags = []\n",
        )
        .expect_err("parse live");
        assert_eq!(err.code(), "bad_usage");
    });
}

#[test]
fn create_user_pack_rejects_oversized_layer() {
    crate::tunnel::test_support::with_temp_home(|| {
        let huge = "x".repeat(USER_PACK_LAYER_MAX_BYTES as usize + 1);
        let err = create_user_pack_inner("user:huge", None, &[], None, Some(&huge))
            .expect_err("pack_too_large");
        assert_eq!(err.code(), "pack_too_large");
    });
}

#[test]
fn scan_skips_kind_live_and_oversized_layer() {
    crate::tunnel::test_support::with_temp_home(|| {
        let live_dir = host_catalog_packs_dir().join("sneaky-live");
        fs::create_dir_all(&live_dir).unwrap();
        fs::write(
            live_dir.join("pack.toml"),
            "id = \"user:sneaky-live\"\nname = \"Sneaky\"\nkind = \"live\"\nlicense = \"MIT\"\nauthor = \"u\"\ntags = []\n",
        )
        .unwrap();
        fs::write(live_dir.join("layer.md"), "body\n").unwrap();

        let big_dir = host_catalog_packs_dir().join("too-big");
        fs::create_dir_all(&big_dir).unwrap();
        fs::write(
            big_dir.join("pack.toml"),
            "id = \"user:too-big\"\nname = \"Big\"\nkind = \"static\"\nlicense = \"MIT\"\nauthor = \"u\"\ntags = []\n",
        )
        .unwrap();
        fs::write(
            big_dir.join("layer.md"),
            "x".repeat(USER_PACK_LAYER_MAX_BYTES as usize + 2),
        )
        .unwrap();

        let catalog = list_catalog();
        assert!(
            catalog.iter().all(|p| p.id != "user:sneaky-live" && p.id != "user:too-big"),
            "scan must skip live + oversized; got {:?}",
            catalog.iter().map(|p| p.id.as_str()).collect::<Vec<_>>()
        );
    });
}

#[test]
fn add_layer_user_pack_materializes_layer_md_not_pack_toml_and_does_not_overwrite() {
    crate::tunnel::test_support::with_temp_home(|| {
        let created = create_user_pack("user:on-call", Some("On-call"), &[])
            .expect("create");
        fs::write(
            created.dir.join("layer.md"),
            "Call the on-call rotation. See [[runbook]].\n",
        )
        .unwrap();

        let root = unique_root("user-pack-mat");
        let path = root.to_str().unwrap();
        let pid = register_project(path);

        let before = {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT COUNT(*) FROM project_context_layers",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
        };
        let _ = before;

        let layer = add_layer(path, None, Some("user:on-call"), None).expect("add");
        assert_eq!(layer.source, "catalog:user:on-call");
        assert_eq!(layer.path, ".k2/context/catalog/on-call.md");
        let ws_file = root.join(".k2/context/catalog/on-call.md");
        assert!(ws_file.is_file(), "must materialize workspace layer.md");
        let body = fs::read_to_string(&ws_file).unwrap();
        assert!(
            body.contains("on-call rotation"),
            "must copy host layer.md: {body}"
        );
        assert!(
            !body.contains("kind = ") && !body.contains("license ="),
            "must not inline pack.toml: {body}"
        );

        let composed = show_composed(path).expect("compose");
        assert!(
            composed.contains("on-call rotation") && composed.contains("On-call"),
            "compose must inline layer.md body; first 600:\n{}",
            &composed[..composed.len().min(600)]
        );
        assert!(
            !composed.contains("license = \"MIT\"") && !composed.contains("kind = \"static\""),
            "compose must not inline pack.toml; first 600:\n{}",
            &composed[..composed.len().min(600)]
        );

        fs::write(&ws_file, "EDITED workspace copy\n").unwrap();
        let err = add_layer(path, None, Some("user:on-call"), None).expect_err("dup");
        assert_eq!(err.code(), "duplicate_layer");
        materialize_pack_if_needed(path, &created.entry, &layer.path).expect("re-mat");
        let after = fs::read_to_string(&ws_file).unwrap();
        assert_eq!(
            after, "EDITED workspace copy\n",
            "re-materialize must not overwrite an edited workspace copy"
        );

        cleanup_project(path, &pid);
    });
}

#[test]
fn create_user_pack_does_not_insert_project_context_layers() {
    crate::tunnel::test_support::with_temp_home(|| {
        let root = unique_root("no-auto-stack");
        let path = root.to_str().unwrap();
        let pid = register_project(path);
        let before = list_layers(path).unwrap();
        assert!(before.is_empty());

        create_user_pack("user:on-call", None, &[]).expect("create");

        let after = list_layers(path).unwrap();
        assert!(
            after.is_empty(),
            "create must not stack on any project; got {after:?}"
        );
        cleanup_project(path, &pid);
    });
}

// ── 0.46.11: Browser and computer use, built in ──────────────────
// Literals on purpose: these run red on a K2 without the built-in pack.

/// sha256 of the pack text: as it shipped as a user pack on rosson.k2.dev
/// (2406 bytes) plus, 0.46.12, the two lines on a Linux server's
/// `k2 desktop env` (prd-linux-virtual-desktop-v1 L8), plus, 0.46.18, the
/// two rules on Chrome's sandbox.
const BROWSER_PACK_SHA256: &str =
    "839a1dfa719635839b2f6619c8933bff32cea3b62ead1ea1409648dc348109f1";

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn assert_browser_pack_text(body: &str) {
    assert_eq!(body.len(), 3484, "browser pack length; got:\n{body}");
    assert_eq!(
        sha256_hex(body.as_bytes()),
        BROWSER_PACK_SHA256,
        "browser pack text must match the user pack it replaces; got:\n{body}"
    );
    for needle in [
        "## Browser and computer use",
        "https://github.com/vercel-labs/agent-browser",
        "https://github.com/vercel-labs/agent-desktop",
        "npm install -g agent-browser",
        "brew install agent-desktop",
        "agent-browser skills get core --full",
        "agent-desktop skills get desktop",
        "--auto-connect",
        "give it back to the window that had it before, in the same step",
        "never their values",
        "**Passwords:** never type them.",
        "ask your person before any irreversible",
        "agent-browser close",
        "eval \"$(k2 desktop env)\"",
        "Allow a virtual desktop",
        "$K2_DESKTOP_PROFILE",
        "never pass `--no-sandbox` (or `--disable-setuid-sandbox`) to Chrome",
        "If Chrome won't start without it, stop and tell your person.",
        "Don't set `CI` in your shell",
        "never run it as root or inside a container",
        "don't browse sites you don't trust from a compute job",
    ] {
        assert!(body.contains(needle), "browser pack must say {needle:?}");
    }
}

#[test]
fn browser_pack_is_a_builtin_opt_in_static_pack() {
    crate::tunnel::test_support::with_temp_home(|| {
        let catalog = list_catalog();
        let hits: Vec<_> = catalog.iter().filter(|p| p.id == "browser:pack").collect();
        assert_eq!(
            hits.len(),
            1,
            "browser:pack listed once; got {:?}",
            catalog.iter().map(|p| p.id.as_str()).collect::<Vec<_>>()
        );
        let b = hits[0];
        assert_eq!(b.path, ".k2/context/catalog/browser-and-computer-use.md");
        assert_eq!(b.source, "catalog:browser");
        assert_eq!(b.label, "Browser and computer use");
        assert_eq!(b.kind, "static");
        assert_eq!(b.version.as_deref(), Some("1.0.0"));
        assert_eq!(b.author.as_deref(), Some("K2"));
        assert!(b.dir.is_none(), "a built-in has no host pack dir");
        assert!(!b.recommended, "browser:pack is opt-in");

        // Not on by default: a fresh workspace's stack has no browser layer.
        let root = unique_root("browser-default");
        let path = root.to_str().unwrap();
        let pid = register_project(path);
        let stack = list_stack(path).expect("list_stack");
        assert!(
            stack.layers.iter().all(|l| !l.path.contains("browser")),
            "browser pack must not stack by itself: {:?}",
            stack.layers
        );
        assert!(!show_composed(path).unwrap().contains("agent-browser"));
        cleanup_project(path, &pid);
    });
}

#[test]
fn browser_pack_adds_by_every_alias_and_writes_its_text() {
    crate::tunnel::test_support::with_temp_home(|| {
        for alias in [
            "browser:pack",
            "browser",
            "browser-and-computer-use",
            "catalog:browser",
            "user:browser-and-computer-use",
            "catalog:user:browser-and-computer-use",
        ] {
            let e = resolve_catalog_id(alias).unwrap_or_else(|err| panic!("{alias}: {err}"));
            assert_eq!(e.id, "browser:pack", "{alias}");

            let root = unique_root("browser-alias");
            let path = root.to_str().unwrap();
            let pid = register_project(path);
            let file = root.join(".k2/context/catalog/browser-and-computer-use.md");
            assert!(!file.exists());

            let layer = add_layer(path, None, Some(alias), None)
                .unwrap_or_else(|err| panic!("add {alias}: {err}"));
            assert_eq!(layer.source, "catalog:browser", "{alias}");
            assert_eq!(layer.path, ".k2/context/catalog/browser-and-computer-use.md", "{alias}");
            assert_eq!(layer.label.as_deref(), Some("Browser and computer use"), "{alias}");
            assert!(layer.enabled && layer.exists, "{alias}");
            assert!(file.is_file(), "{alias} must write the pack file");
            assert_browser_pack_text(&fs::read_to_string(&file).unwrap());

            let composed = show_composed(path).expect("compose");
            assert!(
                composed.contains("agent-browser skills get core --full"),
                "{alias}: composed AGENTS.md carries the pack"
            );

            let err = add_layer(path, None, Some("browser:pack"), None).expect_err("twice");
            assert_eq!(err.code(), "duplicate_layer", "{alias}");
            cleanup_project(path, &pid);
        }
    });
}

/// A server that had "Browser and computer use" as a user pack, with a
/// workspace that stacked it, keeps working after the update.
#[test]
fn browser_user_pack_with_the_same_slug_yields_to_the_builtin() {
    crate::tunnel::test_support::with_temp_home(|| {
        let old_dir = host_catalog_packs_dir().join("browser-and-computer-use");
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(
            old_dir.join("pack.toml"),
            "id = \"user:browser-and-computer-use\"\nname = \"Browser and computer use\"\n\
                 description = \"old\"\nversion = \"1.0.0\"\nkind = \"static\"\n\
                 license = \"MIT\"\nauthor = \"K2\"\ntags = [\"browser\"]\n",
        )
        .unwrap();
        fs::write(old_dir.join("layer.md"), "## Browser and computer use\n\nOLD USER PACK\n")
            .unwrap();

        // The catalog lists the built-in once and skips the user pack.
        let catalog = list_catalog();
        assert!(
            catalog.iter().all(|p| p.id != "user:browser-and-computer-use"),
            "the old user pack must be skipped: {:?}",
            catalog.iter().map(|p| p.id.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(catalog.iter().filter(|p| p.id == "browser:pack").count(), 1);
        assert_eq!(
            catalog
                .iter()
                .filter(|p| p.path == ".k2/context/catalog/browser-and-computer-use.md")
                .count(),
            1,
            "one catalog row for the path"
        );
        // The old id now names the built-in.
        assert_eq!(resolve_catalog_id("user:browser-and-computer-use").unwrap().id, "browser:pack");

        // A workspace that stacked the user pack before the update.
        let root = unique_root("browser-old-row");
        let path = root.to_str().unwrap();
        let pid = register_project(path);
        let ws_file = root.join(".k2/context/catalog/browser-and-computer-use.md");
        fs::create_dir_all(ws_file.parent().unwrap()).unwrap();
        fs::write(&ws_file, "## Browser and computer use\n\nMY EDITED COPY\n").unwrap();
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO project_context_layers \
                     (id, project_id, path, enabled, position, source, label, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, 1, 0, ?4, ?5, ?6, ?6)",
                params![
                    Uuid::new_v4().to_string(),
                    pid,
                    ".k2/context/catalog/browser-and-computer-use.md",
                    "catalog:user:browser-and-computer-use",
                    "Browser and computer use",
                    "2026-11-01T00:00:00Z"
                ],
            )
            .unwrap();
        }

        let layers = list_layers(path).expect("list");
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].source, "catalog:browser", "the old row moves to the built-in");
        assert_eq!(layers[0].path, ".k2/context/catalog/browser-and-computer-use.md");
        assert!(layers[0].enabled && layers[0].exists);
        let composed = show_composed(path).expect("compose");
        assert!(composed.contains("MY EDITED COPY"), "the workspace's own copy still composes");
        assert_eq!(
            fs::read_to_string(&ws_file).unwrap(),
            "## Browser and computer use\n\nMY EDITED COPY\n",
            "the workspace copy is never rewritten"
        );
        let err = add_layer(path, None, Some("browser:pack"), None).expect_err("already stacked");
        assert_eq!(err.code(), "duplicate_layer");
        assert_eq!(list_layers(path).unwrap().len(), 1);
        // Turning it off by its path still works.
        set_enabled(path, ".k2/context/catalog/browser-and-computer-use.md", false).expect("off");
        assert!(!show_composed(path).unwrap().contains("MY EDITED COPY"));

        // The slug can't be made again, but the leftover folder can go.
        let err = create_user_pack("user:browser-and-computer-use", None, &[]).expect_err("reserved");
        assert_eq!(err.code(), "bad_usage");
        delete_user_pack("user:browser-and-computer-use").expect("delete the leftover user pack");
        assert!(!old_dir.exists());
        assert!(fs::read_to_string(&ws_file).unwrap().contains("MY EDITED COPY"));

        cleanup_project(path, &pid);
    });
}

// ── 0.46.18: an unedited browser pack follows K2's text ──────────────

/// The 0.46.12 to 0.46.17 text: today's minus the two sandbox rules.
fn browser_pack_0_46_17_text() -> String {
    let body = static_pack_body("catalog:browser").expect("browser body");
    let start = body.find("- **Chrome's sandbox:**").expect("sandbox rule");
    let old = body[..start].to_string();
    assert_eq!(
        sha256_hex(old.as_bytes()),
        "871747f60088be7eef49a2143e30c6eb059c1cccce99314b70c60531bda5ff6c",
        "the 0.46.17 text must rebuild byte for byte; got:\n{old}"
    );
    old
}

/// A registered workspace with browser:pack stacked: (pack file, id, root).
fn stack_browser_pack(tag: &str) -> (PathBuf, String, PathBuf) {
    let root = unique_root(tag);
    let path = root.to_str().unwrap().to_string();
    let pid = register_project(&path);
    add_layer(&path, None, Some("browser:pack"), None).expect("add browser:pack");
    let file = root.join(".k2/context/catalog/browser-and-computer-use.md");
    assert_browser_pack_text(&fs::read_to_string(&file).unwrap());
    (file, pid, root)
}

#[test]
fn browser_pack_history_lists_every_shipped_text() {
    let (_, past) = SHIPPED_PACK_HISTORY
        .iter()
        .find(|(s, _)| *s == "catalog:browser")
        .expect("browser history");
    assert_eq!(
        *past,
        &[
            "299199fe6cc1177061929fdab19e89687c427ea76ac3038ca121ef9772afcb4e",
            "871747f60088be7eef49a2143e30c6eb059c1cccce99314b70c60531bda5ff6c",
        ][..]
    );
    assert!(!past.contains(&BROWSER_PACK_SHA256), "the current text is not an earlier one");
}

#[test]
fn unedited_old_browser_pack_is_rewritten_on_the_next_context_write() {
    crate::tunnel::test_support::with_temp_home(|| {
        let (file, pid, root) = stack_browser_pack("browser-refresh-old");
        let path = root.to_str().unwrap();
        fs::write(&file, browser_pack_0_46_17_text()).unwrap();
        assert!(!fs::read_to_string(&file).unwrap().contains("--no-sandbox"));

        regen(path).expect("regen");
        assert_browser_pack_text(&fs::read_to_string(&file).unwrap());
        let agents = fs::read_to_string(root.join(".k2/AGENTS.md")).expect("AGENTS.md");
        assert!(
            agents.contains("never pass `--no-sandbox`"),
            "AGENTS.md carries the new rule after the rewrite"
        );
        assert!(!refresh_shipped_static_packs(path), "a current file is left alone");
        cleanup_project(path, &pid);
    });
}

#[test]
fn edited_browser_pack_is_kept() {
    crate::tunnel::test_support::with_temp_home(|| {
        let (file, pid, root) = stack_browser_pack("browser-refresh-edited");
        let path = root.to_str().unwrap();
        let edited = format!("{}- **Mine:** a rule I added.\n", browser_pack_0_46_17_text());
        fs::write(&file, &edited).unwrap();

        assert!(!refresh_shipped_static_packs(path), "an edited file is not written");
        regen(path).expect("regen");
        assert_eq!(fs::read_to_string(&file).unwrap(), edited, "the person's edit stays");
        let agents = fs::read_to_string(root.join(".k2/AGENTS.md")).expect("AGENTS.md");
        assert!(agents.contains("a rule I added."), "AGENTS.md composes the edited copy");
        cleanup_project(path, &pid);
    });
}

#[test]
fn missing_browser_pack_is_written_on_the_next_context_write() {
    crate::tunnel::test_support::with_temp_home(|| {
        let (file, pid, root) = stack_browser_pack("browser-refresh-missing");
        let path = root.to_str().unwrap();
        fs::remove_file(&file).unwrap();

        regen(path).expect("regen");
        assert!(file.is_file(), "the missing pack file is written again");
        assert_browser_pack_text(&fs::read_to_string(&file).unwrap());
        cleanup_project(path, &pid);
    });
}
