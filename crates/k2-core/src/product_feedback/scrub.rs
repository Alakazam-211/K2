//! The diagnostics scrub (PRD §6a "Scrubbing"): a backstop, not the
//! control. The control is that nothing is collected without the tick and
//! the person reads every byte first.
//!
//! Run on every section, in this order:
//! 1. vault exact values (`redaction::redact`, the matcher the daemon
//!    installs at boot) → `[k2v:NAME]`;
//! 2. PEM blocks → `[pem removed]`; `Bearer <x>` / `Basic <x>` → the word
//!    kept, the value `[bearer]`;
//! 3. secret-looking flags (`--token x`, `--password=x`), `KEY=value` with
//!    a secret-looking key, `X-Api-Key: x`, URL userinfo and query values
//!    (`cli_folder_trust::scan::mask_url`) and long token-shaped words;
//! 4. K2 credentials by prefix (`k2c_`, `k2sk_`, `k2skn_`, `k2rs_`,
//!    `k2dl_`, `k2v://`), agent passports, JWTs, `sk-`/`sk-ant-` and other
//!    vendor key shapes;
//! 5. names: every known workspace path → `<workspace-N>`, the home path →
//!    `~`, workspace/App/agent names, federation peers and Connect labels
//!    other than this box's → `<workspace-N>`, `<app-N>`, `<agent-N>`,
//!    `<peer-N>`; email addresses → `<email>`; IP addresses → `<ip>`.
//!
//! Every replacement says what it was, so staff know a value was there.
//! Spacing is kept (log lines stay aligned).

use std::collections::HashMap;

/// Names this box knows, each with the placeholder it becomes. Built once
/// per bundle, so a name is the same placeholder in every section.
#[derive(Debug, Clone, Default)]
pub struct NameBook {
    /// `(path, placeholder)`, longest path first.
    paths: Vec<(String, String)>,
    /// `(name, placeholder)`, longest name first.
    names: Vec<(String, String)>,
    home: Option<String>,
}

/// A workspace the book replaces: its name, folder and handle.
#[derive(Debug, Clone, Default)]
pub struct Workspace {
    pub name: String,
    pub path: String,
    pub handle: Option<String>,
}

/// Words never replaced as names (too common to mean a workspace).
const STOP: &[&str] = &[
    "agent", "agents", "app", "apps", "daemon", "default", "home", "k2", "main", "server", "test", "tests", "the",
    "user", "users", "work", "workspace", "workspaces", "connect", "error", "info", "true", "false", "none", "null",
];

impl NameBook {
    /// `own_labels` are this box's own Connect labels: never replaced.
    pub fn new(
        workspaces: &[Workspace],
        apps: &[String],
        agents: &[String],
        peers: &[String],
        own_labels: &[String],
        home: Option<&str>,
    ) -> NameBook {
        let own: Vec<String> = own_labels.iter().map(|s| s.to_ascii_lowercase()).collect();
        let mut paths: Vec<(String, String)> = Vec::new();
        let mut names: Vec<(String, String)> = Vec::new();
        let mut seen: HashMap<String, String> = HashMap::new();
        let add_name = |names: &mut Vec<(String, String)>, seen: &mut HashMap<String, String>, n: &str, ph: &str| {
            let n = n.trim();
            if n.chars().count() < 3 || STOP.contains(&n.to_ascii_lowercase().as_str()) || own.contains(&n.to_ascii_lowercase()) {
                return;
            }
            if seen.contains_key(n) {
                return;
            }
            seen.insert(n.to_string(), ph.to_string());
            names.push((n.to_string(), ph.to_string()));
        };
        for (i, w) in workspaces.iter().enumerate() {
            let ph = format!("<workspace-{}>", i + 1);
            let p = w.path.trim().trim_end_matches('/');
            if p.len() > 1 {
                paths.push((p.to_string(), ph.clone()));
            }
            add_name(&mut names, &mut seen, &w.name, &ph);
            if let Some(h) = w.handle.as_deref() {
                add_name(&mut names, &mut seen, h, &ph);
            }
        }
        for (i, a) in apps.iter().enumerate() {
            add_name(&mut names, &mut seen, a, &format!("<app-{}>", i + 1));
        }
        for (i, a) in agents.iter().enumerate() {
            add_name(&mut names, &mut seen, a, &format!("<agent-{}>", i + 1));
        }
        for (i, p) in peers.iter().enumerate() {
            add_name(&mut names, &mut seen, p, &format!("<peer-{}>", i + 1));
        }
        paths.sort_by_key(|p| std::cmp::Reverse(p.0.len()));
        names.sort_by_key(|n| std::cmp::Reverse(n.0.len()));
        let home = home.map(|h| h.trim_end_matches('/').to_string()).filter(|h| h.len() > 1);
        NameBook { paths, names, home }
    }
}

/// The whole chain on one section's text.
pub fn scrub(text: &str, book: &NameBook) -> String {
    let vaulted = crate::redaction::redact(text);
    let no_pem = strip_pem(&vaulted);
    let mut out = String::with_capacity(no_pem.len());
    for (i, line) in no_pem.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&names_line(&words_line(line), book));
    }
    out
}

/// PEM blocks (multi-line) → `[pem removed]`; an unterminated one takes
/// the rest of the text.
fn strip_pem(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("-----BEGIN") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        match after.find("-----END") {
            Some(e) => {
                let tail = &after[e + 8..];
                let close = tail.find("-----").map(|c| c + 5).unwrap_or(tail.len());
                out.push_str("[pem removed]");
                rest = &tail[close..];
            }
            None => {
                out.push_str("[pem removed]");
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn secretish(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    let k = k.trim_start_matches('-');
    k == "key"
        || [
            "token", "secret", "passw", "pwd", "apikey", "api_key", "api-key", "auth", "credential", "private", "cookie",
            "signature", "access_key", "accesskey", "dsn",
        ]
        .iter()
        .any(|s| k.contains(s))
}

/// Split a word into leading punctuation, the core and trailing punctuation.
fn split_punct(w: &str) -> (&str, &str, &str) {
    let lead = w.len() - w.trim_start_matches(['"', '\'', '(', '[', '{', '<', '`']).len();
    let body = &w[lead..];
    let core = body.trim_end_matches(['"', '\'', ')', ']', '}', '>', ',', ';', '.', ':', '`', '!', '?']);
    (&w[..lead], core, &body[core.len()..])
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() })
}

fn is_b64url(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'=')
}

fn token_shaped(t: &str) -> bool {
    t.len() >= 24
        && !t.contains('/')
        && !t.contains('.')
        && !is_uuid(t)
        && !(t.len() == 40 && t.bytes().all(|b| b.is_ascii_hexdigit()))
        && t.chars().all(|c| c.is_ascii_alphanumeric() || "-_+=".contains(c))
        && t.chars().any(|c| c.is_ascii_digit())
        && t.chars().any(|c| c.is_ascii_alphabetic())
}

fn is_ipv4(s: &str) -> bool {
    let host = s.rsplit_once(':').filter(|(_, p)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).map(|(h, _)| h).unwrap_or(s);
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok_and(|n| n <= 255))
}

fn is_ipv6(s: &str) -> bool {
    let s = s.trim_start_matches('[').split(']').next().unwrap_or(s);
    let colons = s.matches(':').count();
    (s.contains("::") || colons >= 5)
        && colons >= 2
        && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.')
        && s.bytes().any(|b| b.is_ascii_hexdigit())
}

fn is_email(s: &str) -> bool {
    let Some((local, domain)) = s.rsplit_once('@') else { return false };
    !local.is_empty()
        && !local.contains('/')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && local.bytes().all(|b| b.is_ascii_alphanumeric() || b".+_-%".contains(&b))
}

const K2_PREFIXES: &[&str] = &["k2c_", "k2sk_", "k2skn_", "k2rs_", "k2dl_"];
const VENDOR_PREFIXES: &[&str] = &["sk-ant-", "sk-", "ghp_", "gho_", "ghs_", "ghu_", "github_pat_", "xoxb-", "xoxp-", "glpat-", "AKIA"];

/// One word's core → its replacement, or `None` to keep it.
fn classify_core(core: &str) -> Option<String> {
    if core.is_empty() {
        return None;
    }
    if core.contains("k2v://") {
        return Some("[k2v-ref]".into());
    }
    if K2_PREFIXES.iter().any(|p| core.starts_with(p) && core.len() >= p.len() + 6) {
        return Some("[k2-credential]".into());
    }
    if VENDOR_PREFIXES.iter().any(|p| core.starts_with(p) && core.len() >= p.len() + 12) {
        return Some("[api-key]".into());
    }
    // A JWT: three base64url parts, the header JSON (`eyJ`).
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() == 3 && core.starts_with("eyJ") && parts.iter().all(|p| is_b64url(p)) && parts[1].len() >= 8 {
        return Some("[jwt]".into());
    }
    // An agent passport: `<session uuid>.<secret>`.
    if let Some((sid, secret)) = core.split_once('.') {
        if is_uuid(sid) && secret.len() >= 16 && is_b64url(secret) {
            return Some("[passport]".into());
        }
    }
    if core.contains("://") {
        let masked = crate::cli_folder_trust::scan::mask_url(core);
        return (masked != core).then_some(masked);
    }
    if is_email(core) {
        return Some("<email>".into());
    }
    if is_ipv4(core) || is_ipv6(core) {
        return Some("<ip>".into());
    }
    if token_shaped(core) {
        return Some("[token]".into());
    }
    None
}

/// Steps 2–4 on one line, keeping its spacing.
fn words_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    // The next word is a secret value (after `Bearer`, `--token`, `X-Api-Key:`).
    let mut pending: Option<&'static str> = None;
    let mut i = 0;
    let bytes = line.as_bytes();
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let word = &line[start..i];
        let (lead, core, trail) = split_punct(word);
        // `Bearer <x>` / `Basic <x>`, also right after `Authorization:`.
        if core.eq_ignore_ascii_case("bearer") || core.eq_ignore_ascii_case("basic") {
            out.push_str(word);
            pending = Some("[bearer]");
            continue;
        }
        if let Some(label) = pending.take() {
            out.push_str(lead);
            out.push_str(label);
            out.push_str(trail);
            continue;
        }
        // `X-Api-Key:` / `Authorization:` / `token:` then the value.
        if word.ends_with(':') && !core.contains("://") && (secretish(core) || core.eq_ignore_ascii_case("authorization")) {
            out.push_str(word);
            pending = Some("[secret]");
            continue;
        }
        // `--token value` (a flag with no `=`).
        if core.starts_with('-') && !core.contains('=') && secretish(core) {
            out.push_str(word);
            pending = Some("[secret]");
            continue;
        }
        // `KEY=value` / `--token=value` / `key:value`.
        if let Some(eq) = core.find('=') {
            let (k, v) = (&core[..eq], &core[eq + 1..]);
            if !k.contains("://") && !k.is_empty() && !v.is_empty() {
                if v.eq_ignore_ascii_case("bearer") || v.eq_ignore_ascii_case("basic") {
                    out.push_str(word);
                    pending = Some("[bearer]");
                    continue;
                }
                out.push_str(lead);
                out.push_str(k);
                out.push('=');
                if secretish(k) {
                    out.push_str("[secret]");
                } else {
                    out.push_str(&classify_core(v).unwrap_or_else(|| v.to_string()));
                }
                out.push_str(trail);
                continue;
            }
        }
        match classify_core(core) {
            Some(r) => {
                out.push_str(lead);
                out.push_str(&r);
                out.push_str(trail);
            }
            None => out.push_str(word),
        }
    }
    out
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Replace `name` in `s` where it stands as a whole word.
fn replace_word(s: &str, name: &str, ph: &str) -> String {
    if name.is_empty() || !s.contains(name) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    let mut prev: Option<char> = None;
    while let Some(at) = rest.find(name) {
        let before = rest[..at].chars().next_back().or(prev);
        let after = rest[at + name.len()..].chars().next();
        out.push_str(&rest[..at]);
        if before.is_none_or(|c| !is_name_char(c)) && after.is_none_or(|c| !is_name_char(c)) {
            out.push_str(ph);
        } else {
            out.push_str(name);
        }
        prev = name.chars().next_back();
        rest = &rest[at + name.len()..];
    }
    out.push_str(rest);
    out
}

/// Step 5's names on one line: paths first (a workspace folder under the
/// home folder becomes its placeholder, not `~/…`), then the home folder,
/// then names as whole words.
fn names_line(line: &str, book: &NameBook) -> String {
    let mut s = line.to_string();
    for (p, ph) in &book.paths {
        if s.contains(p.as_str()) {
            s = replace_path(&s, p, ph);
        }
    }
    if let Some(h) = &book.home {
        if s.contains(h.as_str()) {
            s = replace_path(&s, h, "~");
        }
    }
    for (n, ph) in &book.names {
        s = replace_word(&s, n, ph);
    }
    s
}

/// A path where it ends at a separator or a non-path character (so
/// `/Users/al` never eats `/Users/alice`).
fn replace_path(s: &str, path: &str, ph: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find(path) {
        let after = rest[at + path.len()..].chars().next();
        out.push_str(&rest[..at]);
        if after.is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.')) {
            out.push_str(ph);
        } else {
            out.push_str(path);
        }
        rest = &rest[at + path.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> NameBook {
        NameBook::new(
            &[
                Workspace { name: "Acme Payroll".into(), path: "/Users/alice/work/acme-payroll".into(), handle: Some("payroll".into()) },
                Workspace { name: "K2".into(), path: "/Users/alice/dev/K2".into(), handle: None },
            ],
            &["sales-portal".into()],
            &["Cortana".into()],
            &["bob-laptop".into()],
            &["alice".into()],
            Some("/Users/alice"),
        )
    }

    fn s(t: &str) -> String {
        scrub(t, &book())
    }

    #[test]
    fn bearer_and_pem() {
        assert_eq!(s("curl -H Authorization: Bearer abc.def.ghi"), "curl -H Authorization: Bearer [bearer]");
        assert_eq!(s("auth=Bearer  xyz"), "auth=Bearer  [bearer]");
        assert_eq!(s("token: Basic dXNlcjpwYXNz"), "token: Basic [bearer]");
        let pem = "key:\n-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC\n-----END PRIVATE KEY-----\nafter";
        assert_eq!(s(pem), "key:\n[pem removed]\nafter");
        assert_eq!(s("x -----BEGIN CERTIFICATE----- MIIB"), "x [pem removed]");
    }

    #[test]
    fn flags_keys_and_urls() {
        assert_eq!(s("k2 x --token abc123 --name n"), "k2 x --token [secret] --name n");
        assert_eq!(s("--password=hunter2 ok"), "--password=[secret] ok");
        assert_eq!(s("API_KEY=xyz PATH=/bin"), "API_KEY=[secret] PATH=/bin");
        assert_eq!(s("X-Api-Key: v123 next"), "X-Api-Key: [secret] next");
        assert_eq!(s("GET https://u:p@relay.example/x?token=abc&n=1 200"), "GET https://***@relay.example/x?token=***&n=*** 200");
        assert_eq!(s("word aZ3kLm9Qx2Vb7Np4Rt6Wy8Ze1Hj5 end"), "word [token] end");
    }

    #[test]
    fn k2_and_vendor_credentials() {
        for (input, want) in [
            ("k2c_abcdef123456", "[k2-credential]"),
            ("k2sk_abcdef123456", "[k2-credential]"),
            ("k2skn_abcdef123456", "[k2-credential]"),
            ("k2rs_abcdef123456", "[k2-credential]"),
            ("k2dl_abcdef123456", "[k2-credential]"),
            ("k2v://server/STRIPE_KEY", "[k2v-ref]"),
            ("sk-ant-api03-abcdefghijklmnop", "[api-key]"),
            ("sk-proj-abcdefghijklmnop", "[api-key]"),
            ("ghp_abcdefghijklmnop12", "[api-key]"),
            ("eyJhbGciOiJFUzI1NiJ9.eyJzdWIiOiJyb3Nzb24ifQ.c2lnbmF0dXJl", "[jwt]"),
            ("0e5f5de8-7619-4ba4-9753-32c5470b2346.Zm9vYmFyYmF6cXV4cXV1", "[passport]"),
        ] {
            assert_eq!(s(&format!("got {input}.")), format!("got {want}."), "{input}");
        }
        // A plain session uuid and a git sha stay (they are not secrets).
        assert_eq!(s("session 0e5f5de8-7619-4ba4-9753-32c5470b2346"), "session 0e5f5de8-7619-4ba4-9753-32c5470b2346");
        assert_eq!(s("at 3c0240d91a1b2c3d4e5f60718293a4b5c6d7e8f9"), "at 3c0240d91a1b2c3d4e5f60718293a4b5c6d7e8f9");
    }

    #[test]
    fn emails_ips_home_and_names() {
        assert_eq!(s("mail to alice@example.com failed"), "mail to <email> failed");
        assert_eq!(s("dial 192.168.1.20:38471 refused"), "dial <ip> refused");
        assert_eq!(s("peer fe80::1ff:fe23:4567:890a down"), "peer <ip> down");
        assert_eq!(s("at 12:34:56 ok"), "at 12:34:56 ok", "a time is not an address");
        assert_eq!(
            s(r#"[agent-hooks] path=Some("/Users/alice/work/acme-payroll/src") ok"#),
            r#"[agent-hooks] path=Some("<workspace-1>/src") ok"#
        );
        assert_eq!(s("cwd /Users/alice/Downloads"), "cwd ~/Downloads");
        assert_eq!(s("cwd /Users/alicebob/x"), "cwd /Users/alicebob/x", "a longer name is not the home");
        assert_eq!(s("woke payroll and Acme Payroll"), "woke <workspace-1> and <workspace-1>");
        assert_eq!(s("app sales-portal crashed; agent Cortana; peer bob-laptop"), "app <app-1> crashed; agent <agent-1>; peer <peer-1>");
        assert_eq!(s("payrolls stays"), "payrolls stays", "whole words only");
        assert_eq!(s("own label alice stays"), "own label alice stays");
        assert_eq!(s("the K2 path /Users/alice/dev/K2/target"), "the K2 path <workspace-2>/target", "K2 is too short to be a name, its path still goes");
    }

    #[test]
    fn spacing_is_kept() {
        assert_eq!(s("a    b\t\tc"), "a    b\t\tc");
        assert_eq!(s("line1\n  line2  \n"), "line1\n  line2  \n");
    }
}
