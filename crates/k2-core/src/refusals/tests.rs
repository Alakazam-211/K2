//! The registry's own rules: unique ids, known areas, exits that match the
//! CLI, a retired row for `static_not_on_runtime`, and lookups.

use super::*;

#[test]
fn ids_are_unique_snake_case_and_aliases_never_shadow_an_id() {
    let mut seen = std::collections::BTreeSet::new();
    for row in all() {
        assert!(!row.id.is_empty() && row.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'), "{}: snake_case", row.id);
        assert!(seen.insert(row.id), "{} has two rows", row.id);
    }
    for row in all() {
        for a in row.aliases {
            assert!(seen.insert(a), "alias {a} of {} is also an id or another alias", row.id);
        }
    }
}

#[test]
fn every_row_has_a_known_area_text_and_a_status_its_exit_matches() {
    let areas: Vec<&str> = AREAS.iter().map(|(a, _)| *a).collect();
    for row in all() {
        assert!(areas.contains(&row.area), "{}: unknown area {}", row.id, row.area);
        assert!(!row.what.trim().is_empty() && !row.fix.trim().is_empty(), "{}: what and fix", row.id);
        assert!(row.what.len() <= 400 && row.fix.len() <= 300, "{}: keep it short", row.id);
        assert!((400..=599).contains(&row.http), "{}: HTTP {}", row.id, row.http);
        assert!(!row.what.contains("TODO") && !row.fix.contains("TODO"), "{}", row.id);
        // An exit that isn't the HTTP-derived one is listed (and pinned
        // against its CLI surface in k2-daemon's refusal_registry test).
        if row.exit != cli_exit(row.http) {
            assert!(EXIT_OVERRIDES.contains(&(row.id, row.exit)), "{}: exit {} isn't the CLI's {} for HTTP {}; add it to EXIT_OVERRIDES with its surface", row.id, row.exit, cli_exit(row.http), row.http);
        }
    }
    for (id, exit) in EXIT_OVERRIDES {
        let row = lookup(id).unwrap_or_else(|| panic!("override {id} has no row"));
        assert_eq!(row.exit, *exit, "{id}");
    }
}

#[test]
fn the_cli_exit_mirrors_compute_fail() {
    assert_eq!(cli_exit(400), 2);
    assert_eq!(cli_exit(413), 2);
    for s in [401, 403, 404, 405, 409, 410, 422, 423, 429, 500, 503] {
        assert_eq!(cli_exit(s), 3, "{s}");
    }
}

#[test]
fn static_not_on_runtime_stays_as_retired_and_aliases_resolve() {
    let row = lookup("static_not_on_runtime").expect("row");
    assert_eq!(row.status, Status::Retired);
    assert_eq!(lookup(" STATIC_NOT_ON_RUNTIME ").map(|r| r.id), Some("static_not_on_runtime"), "case and spaces");
    assert_eq!(lookup("pairing_needs_k2_front").map(|r| r.id), Some("needs_k2_front"), "the old pairing id is an alias");
    assert!(lookup("no_such_code_ever").is_none());
    assert!(lookup("apps_manage_off").expect("row").askable, "the Build and run Apps switch is asked for on a card");
}

#[test]
fn the_texts_list_every_area_and_row() {
    let list = list_text();
    let page = study_page();
    for row in all() {
        assert!(list.contains(row.id), "list: {}", row.id);
        assert!(page.contains(row.id), "study page: {}", row.id);
        let t = row_text(row);
        assert!(t.starts_with(row.id) && t.contains("fix: "), "{t}");
    }
    assert!(list.contains("static_not_on_runtime (retired)"), "{list}");
    assert!(page.lines().count() <= 600, "the study page stays bounded");
    let block = cli_block("run it", "# <<< generated");
    assert!(block.starts_with(CLI_BEGIN) && block.trim_end().ends_with("# <<< generated"), "{block}");
    let json_line = block.lines().nth(3).expect("the JSON line");
    let v: serde_json::Value = serde_json::from_str(json_line).expect("one JSON line");
    assert_eq!(v["schema"], 1);
    assert_eq!(v["rows"].as_array().map(Vec::len), Some(all().len()));
}
