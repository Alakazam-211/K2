//! The one-line tool description a hook envelope may carry
//! (prd-daemon-activity-and-thread-working-v1 TW8, DA5).
//!
//! Built in the daemon from `tool_name` + `tool_input` at parse time, then
//! the raw input is dropped. The line is redacted BEFORE it is cut, has
//! control characters stripped, and is at most [`MAX_LINE_CHARS`] long.
//! Paths show basenames only. Tool output is never read.
//!
//! The line only ever reaches Member-floor Thread subscribers (S6). It is
//! not stored, logged, or put on the session-events bus.

use serde_json::Value;

/// Hard cap on the finished line, in characters.
pub const MAX_LINE_CHARS: usize = 80;

/// Literal marker that replaces a redacted value.
const REDACTED: &str = "***";

/// Token prefixes that mark a secret. Matched at a word start only, so
/// `task-list` never trips `sk-`.
const SECRET_PREFIXES: &[&str] = &[
    "k2skn_",
    "k2sk_",
    "k2rs_",
    "sk-",
    "ghp_",
    "gho_",
    "github_pat_",
    "xoxa-",
    "xoxb-",
    "xoxp-",
    "AKIA",
];

/// Flags whose following argument (or `=value`) is a secret. `-u` /
/// `--user` carry curl's `user:password` (URL userinfo by another name).
const SECRET_FLAGS: &[&str] = &["--password", "--token", "--secret", "-p", "-u", "--user"];

/// Describe one tool call as a short, redacted line.
pub fn describe(tool_name: &str, input: &Value) -> String {
    let field = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("");
    let line = match tool_name {
        "Bash" => {
            let first = field("command").lines().next().unwrap_or("");
            wrap("Running `", &redact(first), "`")
        }
        "Read" => wrap("Reading `", &basename(field("file_path")), "`"),
        "Edit" | "MultiEdit" | "Write" => wrap("Editing `", &basename(field("file_path")), "`"),
        "Grep" | "Glob" => wrap("Searching `", &redact(field("pattern")), "`"),
        "WebFetch" => wrap("Fetching ", &url_host(field("url")), ""),
        "WebSearch" => "Searching the web".to_string(),
        "Task" | "Agent" => "Starting a subagent".to_string(),
        "TodoWrite" => "Updating the task list".to_string(),
        other => match other.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
            Some((server, tool)) => {
                wrap("Using ", &format!("{} / {}", clean(server), clean(tool)), "")
            }
            None => wrap("Using ", &clean(other), ""),
        },
    };
    clean(&line)
}

/// `prefix + inner + suffix`, with `inner` cut so the whole line fits
/// [`MAX_LINE_CHARS`] (an ellipsis marks the cut; the wrapper survives).
fn wrap(prefix: &str, inner: &str, suffix: &str) -> String {
    let inner = clean(inner);
    let budget = MAX_LINE_CHARS
        .saturating_sub(prefix.chars().count())
        .saturating_sub(suffix.chars().count());
    let inner = if inner.chars().count() > budget {
        let keep: String = inner.chars().take(budget.saturating_sub(1)).collect();
        format!("{keep}…")
    } else {
        inner
    };
    let out = format!("{prefix}{inner}{suffix}");
    // Belt: never past the cap, whatever the wrapper length.
    out.chars().take(MAX_LINE_CHARS).collect()
}

/// Strip control characters and fold every whitespace run to one space.
fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !last_space && !out.is_empty() {
                out.push(' ');
            }
            last_space = true;
        } else if c.is_control() {
            continue;
        } else {
            out.push(c);
            last_space = false;
        }
    }
    out.trim_end().to_string()
}

/// The last path segment (either separator). Empty input stays empty.
fn basename(p: &str) -> String {
    let trimmed = p.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(trimmed)
        .to_string()
}

/// The host of a URL, with any userinfo and port dropped.
fn url_host(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host);
    host.to_string()
}

/// Redact secrets from free text (a shell command, a search pattern).
///
/// Works token by token on whitespace boundaries, so spacing folds to
/// single spaces (the line is a display string, never re-executed).
pub fn redact(text: &str) -> String {
    if let Some(at) = text.find("-----BEGIN") {
        // A PEM block: drop it and everything after it.
        let head = redact(&text[..at]);
        return if head.is_empty() {
            REDACTED.to_string()
        } else {
            format!("{head} {REDACTED}")
        };
    }
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        // `Bearer <x>`: keep the word, drop the credential.
        if tok.trim_start_matches(['"', '\'']).eq_ignore_ascii_case("bearer") && i + 1 < tokens.len() {
            out.push(format!("{tok} {REDACTED}"));
            i += 2;
            continue;
        }
        // `--password x`, `--token=x`, `-p x`.
        if let Some(flag) = SECRET_FLAGS.iter().find(|f| tok == **f) {
            match tokens.get(i + 1) {
                Some(next) if !next.starts_with('-') => {
                    out.push(format!("{flag} {REDACTED}"));
                    i += 2;
                }
                _ => {
                    out.push(tok.to_string());
                    i += 1;
                }
            }
            continue;
        }
        if let Some(flag) = SECRET_FLAGS
            .iter()
            .filter(|f| f.starts_with("--"))
            .find(|f| tok.starts_with(&format!("{f}=")))
        {
            out.push(format!("{flag}={REDACTED}"));
            i += 1;
            continue;
        }
        out.push(redact_token(tok));
        i += 1;
    }
    out.join(" ")
}

/// Redact one whitespace-free token: env assignments, URL userinfo and
/// query strings, secret prefixes, and absolute home paths.
fn redact_token(tok: &str) -> String {
    // `NAME=value` env assignment (never a `--flag=value`).
    if let Some((name, _)) = tok.split_once('=') {
        if is_env_name(name) {
            return format!("{name}={REDACTED}");
        }
    }
    let mut t = tok.to_string();
    // URLs: userinfo and query string.
    if let Some(scheme_end) = t.find("://") {
        let (head, rest) = t.split_at(scheme_end + 3);
        let mut rest = rest.to_string();
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        if let Some(at) = rest[..authority_end].rfind('@') {
            rest = format!("{REDACTED}@{}", &rest[at + 1..]);
        }
        if let Some(q) = rest.find('?') {
            rest = format!("{}?{REDACTED}", &rest[..q]);
        }
        t = format!("{head}{rest}");
    }
    t = redact_prefixed_secrets(&t);
    home_to_tilde(&t)
}

fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Replace every secret-prefixed run (prefix at a word start, up to the
/// next quote, comma or separator) with the marker.
fn redact_prefixed_secrets(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    'scan: while !rest.is_empty() {
        for (idx, _) in rest.char_indices() {
            let at_word_start = idx == 0
                || !rest[..idx]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if !at_word_start {
                continue;
            }
            if let Some(p) = SECRET_PREFIXES.iter().find(|p| rest[idx..].starts_with(**p)) {
                let after = &rest[idx + p.len()..];
                let run = after
                    .find(|c: char| matches!(c, '"' | '\'' | ',' | ';' | '&' | '|' | ')' | '('))
                    .unwrap_or(after.len());
                if run == 0 {
                    continue;
                }
                out.push_str(&rest[..idx]);
                out.push_str(REDACTED);
                rest = &after[run..];
                continue 'scan;
            }
        }
        out.push_str(rest);
        break;
    }
    out
}

/// `/Users/<u>` and `/home/<u>` (and `C:\Users\<u>`) → `~`.
fn home_to_tilde(t: &str) -> String {
    let mut out = t.to_string();
    for root in ["/Users/", "/home/", "C:\\Users\\", "c:\\users\\"] {
        while let Some(start) = out.find(root) {
            let after = &out[start + root.len()..];
            let user_len = after.find(['/', '\\', '"', '\'']).unwrap_or(after.len());
            if user_len == 0 {
                break;
            }
            out = format!("{}~{}", &out[..start], &after[user_len..]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bash(cmd: &str) -> String {
        describe("Bash", &json!({ "command": cmd }))
    }

    /// T-S1e — the TW8 redaction table. Each row: input → exact line.
    #[test]
    fn redaction_table() {
        let rows: Vec<(String, &str)> = vec![
            (bash("cargo test --workspace"), "Running `cargo test --workspace`"),
            (bash("GITHUB_TOKEN=ghp_x cargo test"), "Running `GITHUB_TOKEN=*** cargo test`"),
            (bash("curl -u a:b https://h/x?k=v"), "Running `curl -u *** https://h/x?***`"),
            (bash("curl https://user:pw@example.com/path?q=1"), "Running `curl https://***@example.com/path?***`"),
            (bash("psql --password hunter2"), "Running `psql --password ***`"),
            (bash("psql --password=hunter2 -h db"), "Running `psql --password=*** -h db`"),
            (bash("mysql -u root -p s3cret"), "Running `mysql -u *** -p ***`"),
            (bash("tool --token abc123 run"), "Running `tool --token *** run`"),
            (bash("tool --secret=xyz"), "Running `tool --secret=***`"),
            (bash("tool --token --verbose"), "Running `tool --token --verbose`"),
            (bash("cat /Users/r/.env"), "Running `cat ~/.env`"),
            (bash("ls /home/someone/project/src"), "Running `ls ~/project/src`"),
            (bash("echo sk-abcdef123456"), "Running `echo ***`"),
            (bash("echo task-list"), "Running `echo task-list`"),
            (bash("echo \"k2skn_deadbeef\""), "Running `echo \"***\"`"),
            (bash("curl -H 'Authorization: Bearer abc.def' https://example.com"), "Running `curl -H 'Authorization: Bearer *** https://example.com`"),
            (bash("export AWS_KEY=AKIAABCDEFGHIJKL"), "Running `export AWS_KEY=***`"),
            (bash("echo AKIAABCDEFGHIJKL"), "Running `echo ***`"),
            (bash("git push https://ghp_abc@github.com/o/r"), "Running `git push https://***@github.com/o/r`"),
            (bash("echo github_pat_11AAA"), "Running `echo ***`"),
            (bash("slack xoxb-1-2-3"), "Running `slack ***`"),
            (bash("echo -----BEGIN PRIVATE KEY----- abc"), "Running `echo ***`"),
            (bash("npm run build\nnpm test"), "Running `npm run build`"),
            (bash("printf 'a\tb'"), "Running `printf 'a b'`"),
            (describe("Read", &json!({"file_path": "/Users/r/ai/sew/src/main.rs"})), "Reading `main.rs`"),
            (describe("Edit", &json!({"file_path": "/home/u/x/lib.rs"})), "Editing `lib.rs`"),
            (describe("Grep", &json!({"pattern": "TODO k2sk_live"})), "Searching `TODO ***`"),
            (describe("WebFetch", &json!({"url": "https://a:b@docs.example.com:8443/x?y=z"})), "Fetching docs.example.com"),
            (describe("mcp__github__create_issue", &json!({})), "Using github / create_issue"),
            (describe("TodoWrite", &json!({"todos": ["secret plan"]})), "Updating the task list"),
            (describe("Task", &json!({"prompt": "do the private thing"})), "Starting a subagent"),
            (describe("WebSearch", &json!({"query": "private"})), "Searching the web"),
            (describe("SomeNewTool", &json!({"x": 1})), "Using SomeNewTool"),
        ];
        for (got, want) in &rows {
            assert_eq!(got, want);
        }
        assert!(rows.len() >= 30, "TW8 table has {} rows", rows.len());
    }

    #[test]
    fn long_command_is_cut_to_the_cap_with_the_wrapper_kept() {
        let cmd = format!("echo {}", "x".repeat(200));
        let line = bash(&cmd);
        assert_eq!(line.chars().count(), MAX_LINE_CHARS, "{line}");
        assert!(line.starts_with("Running `echo xxx"), "{line}");
        assert!(line.ends_with("…`"), "{line}");
    }

    #[test]
    fn control_characters_never_survive() {
        let line = bash("echo \u{1b}[31mred\u{7}");
        assert!(!line.chars().any(char::is_control), "{line:?}");
    }

    #[test]
    fn redaction_runs_before_the_cut() {
        // A secret that straddles the cut point must not leak its head.
        let cmd = format!("{} GH=ghp_{}", "a".repeat(60), "b".repeat(40));
        let line = bash(&cmd);
        assert!(!line.contains("ghp_"), "{line}");
    }
}
