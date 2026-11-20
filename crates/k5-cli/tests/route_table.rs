//! The route table's hard rule (PRD 13.8 (b) and 6.2), against the shell
//! router it stands for (`cli/src/router.sh`):
//!
//! - every word of the router's top-level `case` has a row, and every row
//!   is an arm of it, with the same words in the same order;
//! - a `Native` or `Spec` row has a `k2 --schema` entry, or a named reason
//!   not to need one ([`exemptions`]).
//!
//! The router is read with a port of `tests/cli/schema_drift.sh`'s
//! parser, and the two are checked to read the same arms, left-out words
//! and aliases (that script prints them under `SCHEMA_DRIFT_LIST=1`).
//!
//! The schema is `bash cli/k2 --schema` with an empty environment but
//! `PATH` and a temporary `HOME`: no daemon, no K2 home.

#![cfg(unix)]

mod common;

use k5_cli::out::json::{self, Value};
use k5_cli::route::{Owner, TABLE};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The arms of the router's top-level `case`, by schema_drift.sh's
/// rules: from the line starting `case "${1:-help}" in` to the next line
/// that is exactly `esac`, each line that is four spaces, a pattern and
/// `)` (then nothing or `;;`) is an arm; its words are the pattern split
/// on `|`. The `*)` arm is left out. Python's pattern, ported:
/// `^    ([^\s#()][^()]*)\)\s*(;;.*)?$`.
fn router_arms(text: &str) -> Vec<Vec<String>> {
    let lines: Vec<&str> = text.split('\n').collect();
    let start = lines
        .iter()
        .position(|l| l.starts_with("case \"${1:-help}\" in"))
        .expect("router.sh has its top-level `case \"${1:-help}\" in`");
    let end = (start + 1..lines.len())
        .find(|&i| lines[i] == "esac")
        .expect("the top-level case ends with an `esac` line");
    let arms: Vec<Vec<String>> = lines[start + 1..end]
        .iter()
        .copied()
        .filter_map(arm_words)
        .filter(|words| words != &["*"])
        .collect();
    assert!(
        arms.len() >= 80,
        "read only {} router arms: the parser is out of date",
        arms.len()
    );
    arms
}

fn arm_words(line: &str) -> Option<Vec<String>> {
    let rest = line.strip_prefix("    ")?;
    let first = rest.chars().next()?;
    if first.is_whitespace() || "#()".contains(first) {
        return None;
    }
    let paren = rest.find(['(', ')'])?;
    if !rest[paren..].starts_with(')') {
        return None;
    }
    let tail = rest[paren + 1..].trim_start();
    if !(tail.is_empty() || tail.starts_with(";;")) {
        return None;
    }
    Some(rest[..paren].split('|').map(str::to_string).collect())
}

/// The words of schema-head.sh's `BEGIN_SCHEMA_LEFT_OUT` block, with
/// their reasons, by schema_drift.sh's rules: every line that is not
/// blank is `#`, spaces, the word, spaces, the reason.
fn left_out(text: &str) -> BTreeMap<String, String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let start = lines
        .iter()
        .position(|l| {
            l.strip_prefix("# BEGIN_SCHEMA_LEFT_OUT")
                .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
        })
        .expect("schema-head.sh has a BEGIN_SCHEMA_LEFT_OUT line");
    let end = (start + 1..lines.len())
        .find(|&i| lines[i].starts_with("# END_SCHEMA_LEFT_OUT"))
        .expect("schema-head.sh has an END_SCHEMA_LEFT_OUT line after it");
    let mut out = BTreeMap::new();
    for line in &lines[start + 1..end] {
        if line.trim().is_empty() {
            continue;
        }
        let body = line
            .strip_prefix('#')
            .filter(|b| b.starts_with(char::is_whitespace));
        let parts: Vec<&str> = body
            .map(|b| b.split_whitespace().collect())
            .unwrap_or_default();
        let reasons = ["retired", "stub", "internal", "person-only", "beta"];
        assert!(
            parts.len() == 2 && reasons.contains(&parts[1]),
            "left-out line is not `#   <word>  <reason>`: {line:?}"
        );
        out.insert(parts[0].to_string(), parts[1].to_string());
    }
    out
}

/// Words with an arm of their own that run another command's verb, so
/// that command's entry covers them. schema_drift.sh holds the same
/// list (checked below).
const ALIASES: &[(&str, &str, &str)] = &[
    ("reminders", "remind", "runs `k2 remind list`"),
    ("help-advanced", "help", "runs `k2 help --advanced`"),
    ("help-internal", "help", "runs `k2 help --internal`"),
    ("help-deprecated", "help", "runs `k2 help deprecated`"),
];

/// Router words answered offline whatever the schema says.
const OFFLINE: &[(&str, &str)] = &[(
    "version",
    "answered by the front with no daemon (PRD 13.9 #3); its row may be native with or without an entry",
)];

/// Every router word that may be `Native` or `Spec` with no `--schema`
/// entry of its own, with the reason, in one list: the left-out block of
/// cli/src/schema-head.sh (read from the file), the aliases, and the
/// offline words.
fn exemptions(heads: &BTreeSet<String>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = left_out(&common::schema_head())
        .into_iter()
        .map(|(word, why)| {
            (
                word,
                format!("left out of --schema in cli/src/schema-head.sh: {why}"),
            )
        })
        .collect();
    for (word, target, what) in ALIASES {
        assert!(
            heads.contains(*target),
            "alias {word}: {target} has no --schema entry to cover it"
        );
        out.push((
            word.to_string(),
            format!("{what}: {target}'s entry covers it"),
        ));
    }
    for (word, why) in OFFLINE {
        out.push((word.to_string(), why.to_string()));
    }
    let words: BTreeSet<String> = router_arms(&common::router())
        .into_iter()
        .flatten()
        .collect();
    for (word, _) in &out {
        assert!(
            words.contains(word),
            "the exemption {word} is not a router word"
        );
    }
    out
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("k5-route-{tag}-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs `cmd` with an empty environment but `PATH`, `HOME` and `extra`;
/// returns its stdout. Fails unless it exits 0 with nothing on stderr.
fn run(cmd: &mut Command, home: &Path, extra: &[(&str, &str)]) -> String {
    cmd.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", home).envs(extra.iter().copied());
    let out = cmd.output().unwrap_or_else(|e| panic!("run {cmd:?}: {e}"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stderr.is_empty(),
        "{cmd:?} failed ({}): {stderr}",
        out.status
    );
    String::from_utf8(out.stdout).expect("UTF-8 output")
}

/// The first word of every `bash cli/k2 --schema` command's name.
fn schema_heads() -> BTreeSet<String> {
    let dir = temp_dir("schema");
    let text = run(
        Command::new("bash")
            .arg(common::cli_src().join("../k2"))
            .arg("--schema"),
        &dir,
        &[],
    );
    std::fs::remove_dir_all(&dir).unwrap();
    let doc = json::parse(&text).unwrap_or_else(|e| panic!("--schema is not JSON: {e:?}"));
    let field = |v: &Value, key: &str| -> Value {
        let Value::Object(pairs) = v else {
            panic!("not an object: {v:?}")
        };
        pairs
            .iter()
            .find(|(k, _)| k.as_str() == Some(key))
            .unwrap_or_else(|| panic!("no {key:?}"))
            .1
            .clone()
    };
    let Value::Array(commands) = field(&doc, "commands") else {
        panic!("--schema's commands is not a list")
    };
    let heads: BTreeSet<String> = commands
        .iter()
        .map(|c| match field(c, "name") {
            Value::Str(name) => name
                .as_str()
                .expect("a UTF-8 name")
                .split_whitespace()
                .next()
                .expect("a word")
                .to_string(),
            other => panic!("a command name that is not a string: {other:?}"),
        })
        .collect();
    assert!(
        heads.len() >= 50,
        "only {} commands in --schema",
        heads.len()
    );
    heads
}

fn table_rows() -> Vec<Vec<String>> {
    TABLE
        .iter()
        .map(|n| n.words.iter().map(|w| w.to_string()).collect())
        .collect()
}

#[test]
fn the_parser_reads_the_router_as_schema_drift_does() {
    let dir = temp_dir("drift");
    let script = common::cli_src().join("../../tests/cli/schema_drift.sh");
    let text = run(
        Command::new("bash").arg(&script),
        &dir,
        &[("SCHEMA_DRIFT_LIST", "1")],
    );
    std::fs::remove_dir_all(&dir).unwrap();
    let mut arms = Vec::new();
    let mut gone = BTreeMap::new();
    let mut aliases = BTreeSet::new();
    let mut verdict = None;
    for line in text.lines() {
        if let Some(arm) = line.strip_prefix("arm ") {
            arms.push(arm.split('|').map(str::to_string).collect::<Vec<_>>());
        } else if let Some(row) = line.strip_prefix("left-out ") {
            let (word, why) = row.split_once(' ').expect("left-out <word> <reason>");
            gone.insert(word.to_string(), why.to_string());
        } else if let Some(row) = line.strip_prefix("alias ") {
            let (word, target) = row.split_once(' ').expect("alias <word> <target>");
            aliases.insert((word.to_string(), target.to_string()));
        } else if line.starts_with("PASS: ") {
            verdict = Some(line.to_string());
        } else {
            panic!("schema_drift.sh printed a line it should not: {line:?}");
        }
    }
    let verdict = verdict.expect("schema_drift.sh passed");
    let ours = router_arms(&common::router());
    let words: usize = ours.iter().map(Vec::len).sum();
    println!(
        "router: {} arms, {words} words; table: {} rows",
        ours.len(),
        TABLE.len()
    );
    println!("schema_drift.sh: {verdict}");
    assert_eq!(ours, arms, "the two parsers read different arms");
    assert!(
        verdict.starts_with(&format!("PASS: {} router commands:", ours.len())),
        "{verdict}"
    );
    assert_eq!(
        left_out(&common::schema_head()),
        gone,
        "the two read different left-out words"
    );
    let mine: BTreeSet<(String, String)> = ALIASES
        .iter()
        .map(|(w, t, _)| (w.to_string(), t.to_string()))
        .collect();
    assert_eq!(mine, aliases, "ALIASES here and in schema_drift.sh differ");
}

#[test]
fn every_router_arm_has_its_row_and_every_row_is_an_arm() {
    let arms = router_arms(&common::router());
    let rows = table_rows();
    fn words(list: &[Vec<String>]) -> BTreeSet<String> {
        list.iter().flatten().cloned().collect()
    }
    let missing: Vec<String> = words(&arms).difference(&words(&rows)).cloned().collect();
    assert!(
        missing.is_empty(),
        "router words with no row in k5_cli::route::TABLE: {missing:?}"
    );
    let unknown: Vec<String> = words(&rows).difference(&words(&arms)).cloned().collect();
    assert!(
        unknown.is_empty(),
        "rows naming words the router does not take: {unknown:?}"
    );
    for row in &rows {
        assert!(
            arms.contains(row),
            "the row {row:?} is not one arm of the router (an arm's words, in its order)"
        );
    }
    assert_eq!(rows, arms, "the rows are not in the router's order");
    // A verb row is a family's second word, which this test can't check
    // against the router's top-level case: each family's verb rows are
    // checked against its own case, and a family with none here has none.
    for noun in TABLE {
        if noun.verbs.is_empty() {
            continue;
        }
        match noun.words {
            ["connect"] => connect_verbs_are_its_arms(noun),
            ["feedback", "tickets"] => feedback_verbs_are_its_arms(noun),
            ["workspace"] => workspace_verbs_are_its_arms(noun),
            ["compute"] => compute_verbs_are_its_arms(noun),
            ["app"] => app_verbs_are_its_arms(noun),
            ["chat"] => chat_verbs_are_its_arms(noun),
            ["mail"] => mail_verbs_are_its_arms(noun),
            ["hostmail"] => hostmail_verbs_are_its_arms(noun),
            ["daemon"] => case_verbs_are_its_arms(
                noun,
                "router.sh",
                "\n    daemon)\n        case \"${2:-status}\" in\n",
                &[],
            ),
            other => panic!("{other:?} has verb rows: check them against the family's case first"),
        }
    }
}

/// `compute`'s verb rows: each row's words are the words (or some of the
/// words) of one arm of `cmd_compute`'s case (`cli/src/compute-command.sh`),
/// and every word is one the binary's family answers.
fn compute_verbs_are_its_arms(noun: &k5_cli::route::Noun) {
    let sources = common::shell_sources();
    let text = &sources
        .iter()
        .find(|(name, _)| name == "compute-command.sh")
        .expect("cli/src/compute-command.sh")
        .1;
    let arms: Vec<Vec<&str>> = text
        .lines()
        .filter_map(|l| {
            let l = l.strip_prefix("        ")?;
            if l.starts_with(' ') {
                return None;
            }
            let (pat, _) = l.split_once(')')?;
            (!pat.is_empty()
                && pat
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '|' || c == '-'))
            .then(|| pat.split('|').collect())
        })
        .collect();
    for verb in noun.verb_rows() {
        let arm = arms
            .iter()
            .find(|a| a.contains(&verb.words[0]))
            .unwrap_or_else(|| panic!("{:?} is no arm of cmd_compute", verb.words));
        for w in verb.words {
            assert!(arm.contains(w), "{w} is not in the arm {arm:?}");
            assert!(
                k5_cli::families::compute::answers(w),
                "the compute family does not answer {w}"
            );
        }
    }
}

/// `feedback`'s verb rows: each is one arm of the `feedback|tickets)`
/// case in `cli/src/router.sh`, which hands it to its own function.
fn feedback_verbs_are_its_arms(noun: &k5_cli::route::Noun) {
    let sources = common::shell_sources();
    let text = &sources
        .iter()
        .find(|(name, _)| name == "router.sh")
        .expect("cli/src/router.sh")
        .1;
    let body = &text[text
        .find("\n    feedback|tickets)\n")
        .expect("the feedback|tickets arm")..];
    let body = &body[..body.find("\n        ;;\n").expect("its end")];
    for verb in noun.verb_rows() {
        assert_eq!(
            verb.words.len(),
            1,
            "{:?}: one word per verb row",
            verb.words
        );
        let word = verb.words[0];
        let arm = format!("            {word})");
        let call = format!(" shift 2; cmd_feedback_{word} \"$@\" ;;");
        assert!(
            body.lines()
                .any(|l| l.starts_with(&arm) && l.ends_with(&call)),
            "the feedback|tickets case has no {word} arm calling cmd_feedback_{word}"
        );
    }
}

/// `mail`'s verb rows: each is one arm of the `mail)` case in
/// `cli/src/router.sh`, which hands it to its own function (`addresses`
/// and `list` to `cmd_mail_inboxes`; a `-` in a word is a `_` in its
/// function's name), and one the binary's family answers.
fn mail_verbs_are_its_arms(noun: &k5_cli::route::Noun) {
    let sources = common::shell_sources();
    let text = &sources
        .iter()
        .find(|(name, _)| name == "router.sh")
        .expect("cli/src/router.sh")
        .1;
    let body = &text[text.find("\n    mail)\n").expect("the mail arm")..];
    let body = &body[..body.find("\n        ;;\n").expect("its end")];
    for verb in noun.verb_rows() {
        assert_eq!(
            verb.words.len(),
            1,
            "{:?}: one word per verb row",
            verb.words
        );
        let word = verb.words[0];
        let func = match word {
            "addresses" | "list" => "inboxes".to_string(),
            w => w.replace('-', "_"),
        };
        let arm = format!("            {word})");
        let call = format!(" shift 2; cmd_mail_{func} \"$@\" ;;");
        assert!(
            body.lines()
                .any(|l| l.starts_with(&arm) && l.ends_with(&call)),
            "the mail case has no {word} arm calling cmd_mail_{func}"
        );
        assert!(
            word == "watch"
                || k5_cli::families::mail::reads::VERBS.contains(&word)
                || k5_cli::families::mail::writes::VERBS.contains(&word)
                || k5_cli::families::mail::owner::VERBS.contains(&word)
                || k5_cli::families::mail::forward::VERBS.contains(&word),
            "{word}: a mail verb row no family module answers"
        );
    }
}

/// `hostmail`'s verb rows: each is one arm of `cmd_hostmail`'s case
/// (`cli/src/hostmail-app-cert-backup.sh`), which hands it to its own
/// function, and one the binary's family parses.
fn hostmail_verbs_are_its_arms(noun: &k5_cli::route::Noun) {
    let sources = common::shell_sources();
    let text = &sources
        .iter()
        .find(|(name, _)| name == "hostmail-app-cert-backup.sh")
        .expect("cli/src/hostmail-app-cert-backup.sh")
        .1;
    let body = &text[text.find("\ncmd_hostmail() {\n").expect("cmd_hostmail")..];
    let body = &body[..body.find("\n}\n").expect("its end")];
    for verb in noun.verb_rows() {
        assert_eq!(
            verb.words.len(),
            1,
            "{:?}: one word per verb row",
            verb.words
        );
        let word = verb.words[0];
        let arm = format!("        {word})");
        assert!(
            body.lines()
                .any(|l| l.starts_with(&arm) && l.contains(" \"$@\" ;;")),
            "cmd_hostmail's case has no {word} arm"
        );
        assert_ne!(
            k5_cli::families::hostmail::parse(word, &["--json"]),
            k5_cli::families::mail::core::Plan::Shell,
            "{word}: a hostmail verb row the family does not parse"
        );
    }
}

/// `workspace`'s verb rows: each is one arm of the `workspace)` case in
/// `cli/src/router.sh`, which hands it to its own function, and one the
/// binary's family answers.
/// `chat`'s verb rows: together, exactly the arms of `cmd_chat`'s verb
/// case in `cli/src/chat.sh` (not `*`), each the family's
/// ([`k5_cli::families::chat`]).
fn chat_verbs_are_its_arms(noun: &k5_cli::route::Noun) {
    let sources = common::shell_sources();
    let text = &sources
        .iter()
        .find(|(name, _)| name == "chat.sh")
        .expect("cli/src/chat.sh")
        .1;
    let body = &text[text.find("\ncmd_chat() {\n").expect("cmd_chat")..];
    let body = &body[body
        .find("\n    case \"$sub\" in\n        spaces)")
        .expect("cmd_chat's verb case")..];
    let body = &body[..body.find("\n    esac\n}\n").expect("its end")];
    let mut arms = BTreeSet::new();
    for line in body.lines() {
        let Some(head) = line.strip_prefix("        ") else {
            continue;
        };
        if head.starts_with(' ') || head.starts_with("*)") {
            continue;
        }
        let Some((words, _)) = head.split_once(')') else {
            continue;
        };
        if words
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '-' || c == '|')
        {
            arms.extend(words.split('|').map(str::to_string));
        }
    }
    let rows: BTreeSet<String> = noun
        .verbs
        .iter()
        .flat_map(|v| {
            assert_eq!(v.owner, Owner::Native, "{:?}", v.words);
            v.words.iter().map(|w| w.to_string())
        })
        .collect();
    assert_eq!(rows, arms, "chat's verb rows are not cmd_chat's arms");
}

fn workspace_verbs_are_its_arms(noun: &k5_cli::route::Noun) {
    let sources = common::shell_sources();
    let text = &sources
        .iter()
        .find(|(name, _)| name == "router.sh")
        .expect("cli/src/router.sh")
        .1;
    let body = &text[text.find("\n    workspace)\n").expect("the workspace arm")..];
    let body = &body[..body.find("\n        ;;\n").expect("its end")];
    for verb in noun.verb_rows() {
        assert_eq!(
            verb.words.len(),
            1,
            "{:?}: one word per verb row",
            verb.words
        );
        let word = verb.words[0];
        assert!(
            k5_cli::families::workspace::NATIVE_VERBS.contains(&word),
            "the workspace family does not answer {word}"
        );
        let arm = format!("            {word})");
        let call = format!(" shift 2; cmd_workspace_{word} \"$@\" ;;");
        assert!(
            body.lines()
                .any(|l| l.starts_with(&arm) && l.ends_with(&call)),
            "the workspace case has no {word} arm calling cmd_workspace_{word}"
        );
    }
}

/// `app`'s verb rows: each is one arm of `cmd_app`'s `case`
/// (`cli/src/app-wallet.sh`), with the row's words in the arm's order,
/// which hands it to a `cmd_app_` function of its own, one the router's
/// `app)` arm sends to `cmd_app`, and one the binary's family answers.
fn app_verbs_are_its_arms(noun: &k5_cli::route::Noun) {
    let sources = common::shell_sources();
    let text = &sources
        .iter()
        .find(|(name, _)| name == "app-wallet.sh")
        .expect("cli/src/app-wallet.sh")
        .1;
    let body = &text[text.find("\ncmd_app() {\n").expect("cmd_app")..];
    let body = &body[..body.find("\n}\n").expect("its end")];
    for verb in noun.verb_rows() {
        if k5_cli::families::app::SKIN_VERBS.contains(&verb.words[0]) {
            app_skin_verb_reaches_cmd_skin(body, verb);
            continue;
        }
        for word in verb.words {
            assert!(
                k5_cli::families::app::TAKES.contains(word),
                "the app family does not answer {word}"
            );
        }
        let head = format!("\n        {})", verb.words.join("|"));
        let at = body
            .find(&head)
            .unwrap_or_else(|| panic!("cmd_app has no arm{head}"));
        let arm = &body[at + 1..];
        let arm = &arm[..arm.find(";;").expect("the arm's end")];
        assert!(
            arm.contains("cmd_app_"),
            "cmd_app's {:?} arm does not call a function of its own: {arm}",
            verb.words
        );
    }
}

/// `connect`'s verb rows: each is one arm of `cmd_connect`'s `case`
/// (`cli/src/connect-account.sh`), which hands it to its own function.
fn connect_verbs_are_its_arms(noun: &k5_cli::route::Noun) {
    let sources = common::shell_sources();
    let text = &sources
        .iter()
        .find(|(name, _)| name == "connect-account.sh")
        .expect("cli/src/connect-account.sh")
        .1;
    let body = &text[text.find("\ncmd_connect() {\n").expect("cmd_connect")..];
    let body = &body[..body.find("\n}\n").expect("its end")];
    for verb in noun.verb_rows() {
        assert_eq!(
            verb.words.len(),
            1,
            "{:?}: one word per verb row",
            verb.words
        );
        let word = verb.words[0];
        let arm = format!("        {word})\n            cmd_connect_{word} \"$@\"\n");
        assert!(
            body.contains(&arm),
            "cmd_connect has no {word} arm calling cmd_connect_{word}"
        );
    }
}

#[test]
fn a_native_or_spec_row_has_a_schema_entry_or_a_named_reason() {
    let heads = schema_heads();
    let exempt = exemptions(&heads);
    println!("exemptions ({}):", exempt.len());
    for (word, why) in &exempt {
        println!("  {word}: {why}");
    }
    let mut bad = Vec::new();
    for noun in TABLE {
        let ours = noun.owner != Owner::Bash || noun.verbs.iter().any(|v| v.owner != Owner::Bash);
        if !ours {
            continue;
        }
        let entry = noun.words.iter().any(|w| heads.contains(*w));
        let reason = noun
            .words
            .iter()
            .any(|w| exempt.iter().any(|(e, _)| e == w));
        if !entry && !reason {
            bad.push(noun.words.join("|"));
        }
    }
    assert!(
        bad.is_empty(),
        "Native or Spec rows with no `k2 --schema` entry and no reason in the exemptions: {bad:?}"
    );
}

/// `app`'s skin verb rows: `cmd_app` has no arm of its own for `user`,
/// `role`, `front-door` or `hydra` (its `*)` arm is `cmd_skin`), its
/// `grant|grants)` arm hands the list words to `cmd_skin`, and its
/// `token|tokens)` arm is `cmd_skin_token`.
fn app_skin_verb_reaches_cmd_skin(body: &str, verb: &k5_cli::route::Verb) {
    assert!(
        body.contains("\n        *)\n            cmd_skin \"$@\"\n"),
        "cmd_app's *) arm"
    );
    for word in verb.words {
        match *word {
            "user" | "users" | "role" | "roles" | "front-door" | "frontdoor" | "hydra" => {
                assert!(
                    !body.contains(&format!("\n        {word})"))
                        && !body.contains(&format!("|{word})"))
                        && !body.contains(&format!("\n        {word}|")),
                    "cmd_app has its own arm for {word}"
                );
            }
            "grant" | "grants" => assert!(
                body.contains("        grant|grants)\n")
                    && body.contains("|copy-host|copyhost|-h|--help) cmd_skin \"$@\" ;;"),
                "cmd_app's grant arm"
            ),
            "token" | "tokens" => assert!(
                body.contains("        token|tokens)\n") && body.contains("cmd_skin_token \"$@\""),
                "cmd_app's token arm"
            ),
            other => panic!("app verb row {other}: not a skin verb"),
        }
    }
}

/// The router's forms (`tests/data/router_forms.tsv`) and the native
/// bar's ceiling (`tests/data/native_bar_ceiling.tsv`), as sets of words.
fn router_forms_and_ceiling() -> (BTreeSet<String>, BTreeSet<String>) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let read = |name: &str| {
        let p = root.join("tests/data").join(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
    };
    let column = |text: &str, head: &str, col: usize| -> BTreeSet<String> {
        let mut lines = text.lines().filter(|l| !l.starts_with('#'));
        assert_eq!(lines.next(), Some(head));
        lines
            .map(|l| l.split('\t').nth(col).expect("the column").to_string())
            .collect()
    };
    (
        column(
            &read("router_forms.tsv"),
            "form\targv\talias_of\tanchor\tentry\tjson\tsource",
            0,
        ),
        column(&read("native_bar_ceiling.tsv"), "class\tfamily\tform", 2),
    )
}

/// Both ways between the route table and the router's forms: every verb
/// row's word is a form of its family's arm, and every two-word form the
/// front hands straight to the shell (a `Bash` row's verb, or a help line
/// with no page) is listed in the native bar's ceiling, so no bash arm is
/// left out of the count.
#[test]
fn every_router_form_is_a_k5_row_or_listed_in_the_native_bar_ceiling() {
    let (forms, ceiling) = router_forms_and_ceiling();
    let mut problems = Vec::new();
    for noun in TABLE {
        for verb in noun.verb_rows() {
            for word in verb.words {
                if !noun
                    .words
                    .iter()
                    .any(|n| forms.contains(&format!("{n} {word}")))
                {
                    problems.push(format!(
                        "{:?} has a verb row {word:?} that is no form of its arm in router_forms.tsv",
                        noun.words
                    ));
                }
            }
        }
    }
    let mut checked = 0;
    for form in &forms {
        let words: Vec<std::ffi::OsString> = form.split(' ').map(Into::into).collect();
        if words.len() != 2 {
            continue;
        }
        checked += 1;
        let owner = k5_cli::route::find(TABLE, &words).owner;
        let route = k5_cli::front::route(&words);
        if route == k5_cli::front::Route::Fallthrough && !ceiling.contains(form) {
            problems.push(format!(
                "{form:?} goes to the shell (its row is {owner:?}) and the native bar's ceiling doesn't list it"
            ));
        }
    }
    assert!(checked > 500, "only {checked} two-word router forms");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// A family whose verbs are one `case` of the shell's (`daemon` in
/// `cli/src/router.sh`): each verb row's
/// words are words of that case's arms (up to its `esac`), or of `extra`
/// (verbs the function takes before the case).
fn case_verbs_are_its_arms(noun: &k5_cli::route::Noun, file: &str, start: &str, extra: &[&str]) {
    let sources = common::shell_sources();
    let text = &sources
        .iter()
        .find(|(name, _)| name == file)
        .unwrap_or_else(|| panic!("cli/src/{file}"))
        .1;
    let body = &text[text
        .find(start)
        .unwrap_or_else(|| panic!("{start:?} in {file}"))..];
    let case_line = start.trim_end_matches('\n');
    let case_line = &case_line[case_line.rfind('\n').map_or(0, |i| i + 1)..];
    let indent: String = case_line.chars().take_while(|c| *c == ' ').collect();
    let end = format!("\n{indent}esac\n");
    let body = &body[..body.find(&end).expect("its esac")];
    let arm_indent = format!("{indent}    ");
    let mut arms = BTreeSet::new();
    for line in body.lines() {
        let Some(head) = line.strip_prefix(arm_indent.as_str()) else {
            continue;
        };
        if head.starts_with(' ') {
            continue;
        }
        let Some((words, _)) = head.split_once(')') else {
            continue;
        };
        if words
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '-' || c == '|')
        {
            arms.extend(words.split('|').map(str::to_string));
        }
    }
    arms.extend(extra.iter().map(|w| w.to_string()));
    for verb in noun.verb_rows() {
        for w in verb.words {
            assert!(
                arms.contains(*w),
                "{w} is no arm of {file}'s case: {arms:?}"
            );
        }
    }
}
