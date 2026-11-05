//! Ship kind `box-backup`: the pure judge of the box backup client's
//! files and timer, and the read path (daemon first, root helper when the
//! files are root-only). No real files, no real systemctl.

use serde_json::{json, Value};

use super::*;
use crate::sew::ops::fake;

const T: &str = "/var/lib/sew/backups";

fn text(s: &str) -> FileRead {
    FileRead::Text { text: s.to_string() }
}

fn facts(paths: FileRead, excludes: FileRead, timer: &str) -> BoxBackupFacts {
    BoxBackupFacts { paths, excludes, timer: timer.to_string() }
}

fn good() -> BoxBackupFacts {
    facts(text("/etc\n/var/lib\n/home\n"), text("/var/lib/docker\n*.tmp\n"), "enabled")
}

#[test]
fn covering_path_takes_the_folder_or_a_folder_above_it_by_whole_components() {
    for line in ["/", "/var", "/var/", "/var/lib", "/var/lib/sew", "/var/lib/sew/backups", "/var/lib/sew/backups/", "/var/./lib", "/var/lib/sew/backups/*", "/var/lib/sew/backups/**", "/var/l?b", "/var/*/sew", "/var/lib/s[a-z]w"] {
        assert_eq!(covering_path(&format!("# the box\n\n  {line}  \n"), T).as_deref(), Some(line), "{line} covers {T}");
    }
    for line in [
        "/var/lib/sewx",
        "/var/lib/se",
        "/var/li",
        "/var/lib/sew/backup",
        "/var/lib/sew/keyring",
        "/var/lib/sew/backups/2026*",
        "var/lib",
        "lib/sew",
        "/var/../var/lib",
        "# /var/lib",
        "/srv",
        "/var/lib/s[!e]w",
    ] {
        assert_eq!(covering_path(line, T), None, "{line} does not cover {T}");
    }
    assert_eq!(covering_path("", T), None);
    assert_eq!(covering_path("/srv\n/var/lib\n/var\n", T).as_deref(), Some("/var/lib"), "the first line that covers");
}

#[test]
fn excluding_pattern_finds_exact_parents_globs_and_bundles() {
    for pat in [
        "/var/lib/sew/backups",
        "/var/lib/sew/backups/",
        "/var/lib/sew",
        "/var/lib",
        "/var",
        "/",
        "/var/lib/*",
        "/var/*/sew",
        "/var/lib/sew/back*",
        "/var/lib/sew/backups/*",
        "/var/lib/sew/backups/2026*",
        "/var/lib/sew/**",
        "backups",
        "sew",
        "lib/sew",
        "**/sew/backups",
        "b?ckups",
        "/var/lib/[s]ew",
        "$SEW_DIR",
        "${DATA}/sew",
    ] {
        assert_eq!(excluding_pattern(&format!("# skip\n{pat}\n"), T).as_deref(), Some(pat), "{pat} drops {T}");
    }
    for pat in [
        "/var/lib/docker",
        "/var/lib/sewx",
        "/var/lib/sew/keyring",
        "/var/lib/sew/backups/*.tmp",
        "*.tmp",
        "/home/*/.cache",
        "$HOME/.cache",
        "/var/lib/sew/backups/[0-1]999*",
        "# /var/lib",
        "!/var/lib",
    ] {
        assert_eq!(excluding_pattern(pat, T), None, "{pat} keeps {T}");
    }
}

#[test]
fn excludes_are_last_match_wins_and_never_reach_under_an_excluded_folder() {
    // restic's documented way to keep one folder: exclude the siblings,
    // then re-include it.
    assert_eq!(excluding_pattern("/var/lib/*\n!/var/lib/sew\n", T), None);
    assert_eq!(excluding_pattern("/var/lib/sew/backups\n!/var/lib/sew/backups\n", T), None);
    // A later exclude wins again.
    assert_eq!(excluding_pattern("!/var/lib/sew\n/var/lib/sew\n", T).as_deref(), Some("/var/lib/sew"));
    // An excluded folder is never entered, whatever a later `!` says.
    assert_eq!(excluding_pattern("/var/lib\n!/var/lib/sew/backups\n", T).as_deref(), Some("/var/lib"));
}

#[test]
fn glob_and_name_matching() {
    assert!(name_match("a*c", "abbbc"));
    assert!(name_match("*", ""));
    assert!(!name_match("?", ""));
    assert!(name_match("[]x]", "]"));
    assert!(name_match("[^a]", "b"));
    assert!(!name_match("[!a]", "a"));
    assert!(!name_match("[ab", "a"), "an unclosed class matches nothing");
    assert!(name_match("\\*", "*"));
    assert!(!name_match("\\*", "x"));
    assert!(glob_match(&["**"], &[]));
    assert!(glob_match(&["**", "b"], &["a", "x", "b"]));
    assert!(!glob_match(&["a", "**", "c"], &["a", "b"]));
    assert!(glob_match(&["a", "**", "c"], &["a", "c"]));
}

#[test]
fn judge_clears_only_with_all_three_facts() {
    let e = judge(&good());
    assert!(e.ok, "{e:?}");
    assert!(e.reasons.is_empty());
    assert_eq!(e.covered_by.as_deref(), Some("/var/lib"));
    assert_eq!(e.excluded_by, None);
    assert_eq!(e.line(), "the box backup (k2-backup) carries Sew's bundles");
    assert_eq!(e.json()["reason"], Value::Null);
    // No exclude file: nothing is excluded.
    assert!(judge(&facts(text("/"), FileRead::Missing, "enabled")).ok);

    let one = |f: BoxBackupFacts, want: &str| {
        let e = judge(&f);
        assert!(!e.ok, "{f:?}");
        assert_eq!(e.reasons.len(), 1, "{:?}", e.reasons);
        assert!(e.reasons[0].contains(want), "'{}' should say '{want}'", e.reasons[0]);
        assert!(e.line().contains(want), "{}", e.line());
        assert_eq!(e.json()["reason"], json!(e.reasons[0]));
    };
    let g = good();
    one(facts(FileRead::Missing, g.excludes.clone(), "enabled"), "/etc/k2-backup/paths is missing");
    one(facts(FileRead::Error { error: "permission denied".into() }, g.excludes.clone(), "enabled"), "can't read /etc/k2-backup/paths: permission denied");
    one(facts(text("/etc\n/home\n"), g.excludes.clone(), "enabled"), "does not cover /var/lib/sew/backups");
    one(facts(g.paths.clone(), text("/var/lib/sew\n"), "enabled"), "excludes /var/lib/sew/backups ('/var/lib/sew' in /etc/k2-backup/excludes)");
    one(facts(g.paths.clone(), FileRead::Error { error: "not a regular file".into() }, "enabled"), "can't read /etc/k2-backup/excludes");
    for w in ["disabled", "masked", "not-found", "static", "unknown", "enabled-runtime"] {
        one(facts(g.paths.clone(), g.excludes.clone(), w), &format!("k2-backup.timer is not enabled (systemctl is-enabled says '{w}')"));
    }
    // Every reason at once, in order.
    let e = judge(&facts(text("/srv"), text("/"), "disabled"));
    assert_eq!(e.reasons.len(), 3, "{:?}", e.reasons);
    assert!(e.json()["reason"].as_str().expect("joined").contains("does not cover"));
}

fn helper(protocol: u32, flag: bool) -> RootHelperState {
    let mut h = fake::helper_ok(protocol, true, k2_root_proto::sew::pinned().version);
    if flag {
        h.version_json.as_mut().expect("version json")["namespaces"]["sewboxbackup"] = json!(true);
    }
    h
}

#[test]
fn read_uses_the_daemon_first_and_the_helper_only_for_root_only_files() {
    let never = |_: &[String]| -> Result<Value, String> { panic!("readable files never call the helper") };
    let direct = |name: &str| match name {
        "paths" => Direct::Read(text("/var/lib\n")),
        "excludes" => Direct::Read(FileRead::Missing),
        other => panic!("only the two fixed files are read, not {other}"),
    };
    let f = read_with(&helper(13, true), direct, || "enabled".into(), never);
    assert_eq!(f, facts(text("/var/lib\n"), FileRead::Missing, "enabled"));
    assert!(judge(&f).ok);

    // Root-only: the helper's whole answer.
    let denied = |name: &str| if name == "paths" { Direct::Denied } else { Direct::Read(FileRead::Missing) };
    let mut seen = None;
    let f = read_with(&helper(13, true), denied, || panic!("the helper's timer word is used"), |argv| {
        seen = Some(argv.to_vec());
        Ok(json!({"paths": {"state": "text", "text": "/\n"}, "excludes": {"state": "missing"}, "timer": "enabled"}))
    });
    assert_eq!(seen.expect("the helper was asked"), ["engine", "sew", "box-backup"]);
    assert_eq!(f, facts(text("/\n"), FileRead::Missing, "enabled"));
    let v = k2_root_proto::verbs::Verb::parse(&["engine", "sew", "box-backup"]).expect("the helper parses that argv");
    assert_eq!(v.argv(), ["engine", "sew", "box-backup"]);

    // A helper without the verb (protocol 12, or 13 without the flag) is
    // never called; the file says why.
    for st in [helper(12, true), helper(13, false), RootHelperState::missing()] {
        let f = read_with(&st, denied, || "enabled".into(), |_| panic!("never called"));
        assert_eq!(f.paths, FileRead::Error { error: HELPER_TOO_OLD.into() });
        let e = judge(&f);
        assert!(!e.ok);
        assert!(e.reasons[0].contains("needs root helper protocol 13"), "{:?}", e.reasons);
    }

    // The helper failing or answering garbage: both files are errors.
    let f = read_with(&helper(13, true), denied, || "enabled".into(), |_| Err("exit 2".into()));
    assert_eq!(f.paths, FileRead::Error { error: "the root helper couldn't read it: exit 2".into() });
    assert_eq!(f.excludes, f.paths);
    let f = read_with(&helper(13, true), denied, || "enabled".into(), |_| Ok(json!({"paths": "x"})));
    assert!(matches!(&f.paths, FileRead::Error { error } if error.starts_with("the root helper couldn't read it: answer:")), "{f:?}");
}

#[test]
fn the_cache_reuses_a_reading_until_forgotten() {
    forget();
    let first = cached(good);
    assert_eq!(first, good());
    let again = cached(|| panic!("a fresh reading within the cache window"));
    assert_eq!(again, first);
    forget();
    let other = facts(FileRead::Missing, FileRead::Missing, "disabled");
    assert_eq!(cached(|| other.clone()), other);
    forget();
}
