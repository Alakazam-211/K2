//! `k2 app validate <folder|seed> [--types]` (§7.2 step 2, S6).
//!
//! Errors block a build and a deploy; warnings never do. Checks:
//! - `manifest.json`: strict parse (line and column), schema 1, `kind:
//!   "app"`, id, name, version, `host`.
//! - The catalog contract: every cap exists and is exposed to `app`;
//!   binding and role caps nest inside the root `caps`; role slots exist;
//!   bindings are `room` slots; `requires.features` are known keys;
//!   `requires.k2` is met by this K2.
//! - `--types`: the TypeScript checker over the App's `tsconfig.json`.
//!   Its errors block (contract errors); a missing checker is an error
//!   when `--types` was asked for (never a silent skip).
//!
//! - `code`: stage 0's oxc pass (S4, `crate::jscheck::app`) over every
//!   JS/TS file and inline `<script>`: a parse error is an error (it would
//!   fail at start anyway); the leak lints and anything not read are
//!   warnings. The same check `k2 app validate --dir` runs on a plain
//!   folder.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::manifest::{self, Manifest, MANIFEST_FILE};

/// The name and version of the rule set, recorded in `createdWith`.
pub const CHECKER: &str = "k2-app-validate-1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

fn issue(code: &str, message: impl Into<String>, at: Option<String>) -> Issue {
    Issue { code: code.to_string(), message: message.into(), at }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub ok: bool,
    pub errors: Vec<Issue>,
    pub warnings: Vec<Issue>,
    /// Which rule sets ran (`manifest`, `contract`, `types:<checker>`).
    pub checks: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Run the TypeScript checker (`--types`).
    pub types: bool,
    /// Override the checker program (tests, `K2_APP_TSC`).
    pub checker: Option<PathBuf>,
}

/// Validate a folder (or a stored Seed tree). Returns the parsed manifest
/// when it parsed, so a build can record it.
pub fn validate_tree(root: &Path, opts: &Options) -> (Report, Option<Manifest>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut checks = vec!["manifest".to_string()];
    let mp = root.join(MANIFEST_FILE);
    let text = match std::fs::read_to_string(&mp) {
        Ok(t) => t,
        Err(e) => {
            errors.push(issue(
                "manifest_missing",
                format!("{MANIFEST_FILE} not readable at the folder root: {e}. An App needs one (k2 study apps)"),
                None,
            ));
            return (Report { ok: false, errors, warnings, checks }, None);
        }
    };
    let m = match manifest::parse(&text) {
        Ok(m) => m,
        Err(e) => {
            errors.push(issue("manifest_invalid", e.message, Some(format!("{MANIFEST_FILE}:{}:{}", e.line, e.column))));
            return (Report { ok: false, errors, warnings, checks }, None);
        }
    };
    check_manifest(&m, &mut errors, &mut warnings);
    checks.push("contract".into());
    check_contract(&m, &mut errors, &mut warnings);
    checks.push("code".into());
    check_code(root, &mut errors, &mut warnings);
    if opts.types {
        let label = check_types(root, opts, &mut errors, &mut warnings);
        checks.push(format!("types:{label}"));
    }
    (Report { ok: errors.is_empty(), errors, warnings, checks }, Some(m))
}

/// Stage 0 (S4): parse every JS/TS file and inline script, run the leak
/// lints. One `k2 app validate` for Seeds and plain folders (vs-live 2).
fn check_code(root: &Path, errors: &mut Vec<Issue>, warnings: &mut Vec<Issue>) {
    let r = crate::jscheck::app::check_dir(root);
    let text = |d: &crate::zen::schema::Diagnostic| match &d.suggestion {
        Some(s) => format!("{} (suggestion: {s})", d.message),
        None => d.message.clone(),
    };
    let at = |d: &crate::zen::schema::Diagnostic| Some(format!("{}:{}:{}", d.file, d.line, d.col));
    for d in &r.errors {
        errors.push(issue("code_parse_error", text(d), at(d)));
    }
    for d in &r.warnings {
        warnings.push(issue("code_lint", text(d), at(d)));
    }
    for line in r.skipped {
        warnings.push(issue("code_skipped", line, None));
    }
}

fn check_manifest(m: &Manifest, errors: &mut Vec<Issue>, warnings: &mut Vec<Issue>) {
    let at = |f: &str| Some(format!("{MANIFEST_FILE} {f}"));
    // R39 (update continuity, 0.46.4): K2 up to 0.46.3 refuses a manifest
    // with these keys, so a K2 rollback can't run this Seed, and a template
    // meant for older boxes must not carry them.
    for k in m.keys_needing_0464() {
        warnings.push(issue(
            "needs_k2_0464",
            format!("{k} needs K2 0.46.4 or newer: K2 0.46.3 and older refuse this Seed (after a K2 rollback it won't start)"),
            at(k),
        ));
    }
    if m.schema != manifest::SCHEMA {
        errors.push(issue("manifest_invalid", format!("schema {} is not supported; this K2 reads schema 1", m.schema), at("schema")));
    }
    if m.kind != "app" {
        errors.push(issue(
            "manifest_invalid",
            format!("kind \"{}\" can't be installed on a box; an App manifest says \"app\"", m.kind),
            at("kind"),
        ));
    }
    if !super::valid_app_id(&m.id) {
        errors.push(issue("manifest_invalid", format!("id \"{}\": lower-case letters, digits and -, up to 40", m.id), at("id")));
    }
    let nlen = m.name.chars().count();
    if !(1..=40).contains(&nlen) {
        errors.push(issue("manifest_invalid", "name: 1 to 40 characters", at("name")));
    }
    if m.version.trim().is_empty() {
        errors.push(issue("manifest_invalid", "version is empty", at("version")));
    }
    match m.host.mode.as_str() {
        "cmd" | "next" => {
            if m.host.start.is_empty() || m.host.start[0].trim().is_empty() {
                errors.push(issue(
                    "manifest_invalid",
                    "host.start: give the server's argv, e.g. [\"node\", \"server.js\"]; it gets $PORT",
                    at("host.start"),
                ));
            }
        }
        "static" => warnings.push(issue(
            super::codes::STATIC_NOT_ON_RUNTIME,
            "static Apps are served by `k2 publish run --skin` in this release; the runtime deploys `cmd` Apps",
            at("host.mode"),
        )),
        other => errors.push(issue("manifest_invalid", format!("host.mode \"{other}\": use static, cmd or next"), at("host.mode"))),
    }
    if let Some(h) = &m.host.health {
        if let Some(p) = &h.path {
            if !p.starts_with('/') || p.contains(char::is_whitespace) {
                errors.push(issue("manifest_invalid", "host.health.path must start with / and hold no spaces", at("host.health.path")));
            }
        }
        if h.timeout_secs.is_some_and(|t| t == 0 || t > 900) {
            errors.push(issue("manifest_invalid", "host.health.timeoutSecs: 1 to 900", at("host.health.timeoutSecs")));
        }
        if let Some(p) = &h.ready_path {
            if !p.starts_with('/') || p.contains(char::is_whitespace) {
                errors.push(issue("manifest_invalid", "host.health.readyPath must start with / and hold no spaces", at("host.health.readyPath")));
            }
        }
        if h.as_test_guest {
            warnings.push(issue(
                super::codes::TEST_GUEST_UNAVAILABLE,
                "host.health.asTestGuest: logged-in health probes need test guests (`k2 app guest test`), which arrive in a later 0.46 \
                 release; until then a deploy of this Seed fails its health gate with test_guest_unavailable",
                at("host.health.asTestGuest"),
            ));
        }
    }
    if m.host.drain_secs.is_some_and(|d| d > 3600) {
        errors.push(issue("manifest_invalid", "host.drainSecs: at most 3600", at("host.drainSecs")));
    }
    for k in m.env.keys() {
        let ok = !k.is_empty()
            && k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && !k.starts_with(|c: char| c.is_ascii_digit());
        if !ok {
            errors.push(issue("manifest_invalid", format!("env name \"{k}\": upper-case letters, digits and _"), at("env")));
        } else if k.starts_with("K2_")
            || ["PORT", "PATH", "HOME", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY", "NODE_USE_ENV_PROXY"].contains(&k.as_str())
        {
            errors.push(issue("manifest_invalid", format!("env name \"{k}\" is set by K2 and can't be declared"), at("env")));
        } else if sensitive_name(k) {
            errors.push(issue(
                "env_secret_inline",
                format!("env \"{k}\" names a secret; a manifest never holds secret values (grant it from the vault)"),
                at("env"),
            ));
        }
    }
    if let Some(e) = &m.egress {
        if e.hosts.len() > super::egress::MAX_HOSTS_PER_REQUEST {
            errors.push(issue(
                "manifest_invalid",
                format!("egress.hosts: at most {} hosts", super::egress::MAX_HOSTS_PER_REQUEST),
                at("egress.hosts"),
            ));
        }
        for h in &e.hosts {
            if let Err(why) = super::egress::parse_spec(h) {
                errors.push(issue("egress_host_invalid", format!("egress.hosts: {why}"), at("egress.hosts")));
            }
        }
    }
    if m.box_services.len() > super::box_service::MAX_PER_APP {
        errors.push(issue(
            "manifest_invalid",
            format!("boxServices: at most {}", super::box_service::MAX_PER_APP),
            at("boxServices"),
        ));
    }
    for b in &m.box_services {
        if !super::box_service::name_ok(&b.name) {
            errors.push(issue(
                "box_service_invalid",
                format!("boxServices: '{}' is not a name (lowercase letters, digits and '-', like sew-api)", b.name),
                at("boxServices"),
            ));
        }
        if b.host_port.is_none() && super::box_service::preset(&b.name).is_none() {
            errors.push(issue(
                "box_service_invalid",
                format!("boxServices: '{}' needs a hostPort (only sew-api and sew-signal are known by name)", b.name),
                at("boxServices"),
            ));
        }
        if b.host_port.is_some_and(|p| p < 1024) {
            errors.push(issue("box_service_invalid", format!("boxServices: '{}' hostPort is a system port (below 1024)", b.name), at("boxServices")));
        }
        if b.app_port.is_some_and(|p| !super::box_service::app_port_ok(p)) {
            errors.push(issue(
                "box_service_invalid",
                format!("boxServices: '{}' appPort must be 1024 or more, not 8730 (the gateway link) and outside 38600-38999", b.name),
                at("boxServices"),
            ));
        }
        if b.proto.as_deref().is_some_and(|p| p != super::box_service::PROTO_TCP) {
            errors.push(issue("box_service_invalid", format!("boxServices: '{}' proto is tcp (HTTP and websockets ride it)", b.name), at("boxServices")));
        }
    }
    if !m.build.is_empty() {
        warnings.push(issue(
            "build_not_run",
            "build: the build runs inside the App's wall in a later 0.46 release; this release stores the folder as you built it",
            at("build"),
        ));
    }
}

fn check_contract(m: &Manifest, errors: &mut Vec<Issue>, _warnings: &mut Vec<Issue>) {
    use crate::contract::{catalog, Exposure};
    let cat = catalog();
    for c in &m.caps {
        match cat.cap(c) {
            None => errors.push(issue("cap_unknown", format!("cap \"{c}\" is not in K2's catalog"), Some(format!("caps: {c}")))),
            Some(row) if !row.exposure.contains(&Exposure::App) => {
                errors.push(issue("cap_not_for_apps", format!("cap \"{c}\" is not available to Apps"), Some(format!("caps: {c}"))))
            }
            Some(_) => {}
        }
    }
    for (slot, b) in &m.bindings {
        if b.kind != "room" {
            errors.push(issue(
                "binding_invalid",
                format!("binding \"{slot}\" has type \"{}\"; App bindings are \"room\" slots (\"home\" is for Garden bundles)", b.kind),
                Some(format!("bindings.{slot}")),
            ));
        }
        for c in &b.caps {
            if !m.caps.contains(c) {
                errors.push(issue(
                    "cap_outside_ceiling",
                    format!("binding \"{slot}\" asks for {c}, which is not in the App's caps"),
                    Some(format!("bindings.{slot}")),
                ));
            }
        }
    }
    for (role, slots) in &m.roles {
        for (slot, caps) in slots {
            if !m.bindings.contains_key(slot) {
                errors.push(issue(
                    "role_invalid",
                    format!("role \"{role}\" names slot \"{slot}\", which is not one of this App's bindings"),
                    Some(format!("roles.{role}")),
                ));
            }
            for c in caps {
                if !m.caps.contains(c) {
                    errors.push(issue(
                        "cap_outside_ceiling",
                        format!("role \"{role}\" asks for {c}, which is not in the App's caps"),
                        Some(format!("roles.{role}.{slot}")),
                    ));
                }
            }
        }
    }
    if let Some(req) = &m.requires {
        let catalog_features: Vec<&str> = cat.verbs.iter().map(|v| v.feature.as_str()).collect();
        for f in &req.features {
            if !crate::zen::widgets::KNOWN_FEATURES.contains(&f.as_str()) && !catalog_features.contains(&f.as_str()) {
                errors.push(issue("feature_unknown", format!("requires.features: \"{f}\" is not a K2 feature key"), Some("requires.features".into())));
            }
        }
        if let Some(range) = &req.k2 {
            match k2_range_ok(range, env!("CARGO_PKG_VERSION")) {
                Ok(true) => {}
                Ok(false) => errors.push(issue(
                    "k2_version_unmet",
                    format!("requires.k2 \"{range}\" is not met by this K2 ({})", env!("CARGO_PKG_VERSION")),
                    Some("requires.k2".into()),
                )),
                Err(e) => errors.push(issue("manifest_invalid", format!("requires.k2: {e}"), Some("requires.k2".into()))),
            }
        }
        // requires.sew / requires.voice (0.46.3): only the form here; the
        // box's Sew is checked at install and deploy (the daemon's
        // `apps::requires`), since validate can run off the box.
        if let Some(range) = &req.sew {
            if let Err(e) = range_form_ok(range) {
                errors.push(issue("manifest_invalid", format!("requires.sew: {e}"), Some("requires.sew".into())));
            }
        }
        // requires.postgres (0.46.4, PA4): the form only; the App's
        // database is checked at install, deploy and re-host.
        if let Some(range) = &req.postgres {
            if let Err(e) = pg_range_form_ok(range) {
                errors.push(issue("manifest_invalid", format!("requires.postgres: {e}"), Some("requires.postgres".into())));
            }
        }
    }
}

/// `requires.postgres` (PA4): `requires.k2`'s operators over whole
/// Postgres majors (`17`, `>=16`, `>=16 <18`). Does `major` satisfy it?
/// A part with a minor (`17.2`), a sign or a word is a form error.
pub fn pg_range_ok(range: &str, major: u32) -> Result<bool, String> {
    let mut any = false;
    for part in range.split_whitespace() {
        any = true;
        let v = part.trim_start_matches(['>', '<', '=']);
        let ops = &part[..part.len() - v.len()];
        if !matches!(ops, "" | ">=" | "<=" | ">" | "<" | "=") {
            return Err(format!("\"{part}\" is not a Postgres major range like >=17"));
        }
        if v.is_empty() || v.len() > 3 || !v.bytes().all(|b| b.is_ascii_digit()) || v.starts_with('0') {
            return Err(format!("\"{part}\" is not a whole Postgres major (like 17)"));
        }
    }
    if !any {
        return Err("empty range".into());
    }
    k2_range_ok(range, &major.to_string())
}

/// Form only (`k2 app validate`): a range of whole majors that some major
/// from 10 to 99 satisfies (`>=18 <17` names none and is refused).
pub fn pg_range_form_ok(range: &str) -> Result<(), String> {
    pg_range_ok(range, 0)?;
    if (10..=99).any(|m| pg_range_ok(range, m) == Ok(true)) {
        Ok(())
    } else {
        Err(format!("\"{range}\" is met by no Postgres major"))
    }
}

/// A range in `requires.k2`'s form (`>=0.3.0`, `>=0.3.0 <0.5.0`), checked
/// for form only.
pub fn range_form_ok(range: &str) -> Result<(), String> {
    k2_range_ok(range, "0.0.0").map(|_| ())
}

fn parse_ver(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s.trim().split(['.', '-', '+']).map(|p| p.parse::<u64>());
    Some((it.next()?.ok()?, it.next().unwrap_or(Ok(0)).ok()?, it.next().unwrap_or(Ok(0)).ok()?))
}

/// The vault's sensitive-name rule (`k2_vault::delivery::is_sensitive_name`,
/// §10.4); k2-core doesn't link the vault, so the rule is repeated here.
fn sensitive_name(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    k.ends_with("_API_KEY")
        || k == "API_KEY"
        || k.ends_with("_TOKEN")
        || k.ends_with("_SECRET")
        || k.contains("PASSWORD")
        || k == "DATABASE_URL"
        || k.ends_with("_DSN")
}

/// `>=0.46.0`, `>0.45`, `<=…`, `<…`, `=…` or a bare version, space-separated
/// (all must hold).
pub fn k2_range_ok(range: &str, have: &str) -> Result<bool, String> {
    let have = parse_ver(have).ok_or_else(|| format!("bad K2 version {have}"))?;
    let mut any = false;
    for part in range.split_whitespace() {
        any = true;
        let (op, v) = if let Some(v) = part.strip_prefix(">=") {
            (">=", v)
        } else if let Some(v) = part.strip_prefix("<=") {
            ("<=", v)
        } else if let Some(v) = part.strip_prefix('>') {
            (">", v)
        } else if let Some(v) = part.strip_prefix('<') {
            ("<", v)
        } else if let Some(v) = part.strip_prefix('=') {
            ("=", v)
        } else {
            ("=", part)
        };
        let want = parse_ver(v).ok_or_else(|| format!("\"{part}\" is not a version range like >=0.46.0"))?;
        let ok = match op {
            ">=" => have >= want,
            "<=" => have <= want,
            ">" => have > want,
            "<" => have < want,
            _ => have == want,
        };
        if !ok {
            return Ok(false);
        }
    }
    if !any {
        return Err("empty range".into());
    }
    Ok(true)
}

/// The checker to run: `opts.checker`, `K2_APP_TSC`, the App's own
/// `node_modules/.bin/{tsc-rs,tsc}`, then `tsc-rs`/`tsc` on PATH.
fn find_checker(root: &Path, opts: &Options) -> Option<PathBuf> {
    if let Some(c) = &opts.checker {
        return Some(c.clone());
    }
    if let Ok(c) = std::env::var("K2_APP_TSC") {
        if !c.trim().is_empty() {
            return Some(PathBuf::from(c.trim()));
        }
    }
    for n in ["tsc-rs", "tsc"] {
        let p = root.join("node_modules/.bin").join(n);
        if p.is_file() {
            return Some(p);
        }
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for n in ["tsc-rs", "tsc"] {
            let p = dir.join(n);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Largest checker output kept.
const TYPES_OUTPUT_CAP: usize = 256 * 1024;
const TYPES_TIMEOUT: Duration = Duration::from_secs(180);

fn check_types(root: &Path, opts: &Options, errors: &mut Vec<Issue>, warnings: &mut Vec<Issue>) -> String {
    let tsconfig = root.join("tsconfig.json");
    if !tsconfig.is_file() {
        warnings.push(issue("types_no_tsconfig", "--types: no tsconfig.json at the folder root, so there is nothing to type-check", None));
        return "none".into();
    }
    let Some(checker) = find_checker(root, opts) else {
        errors.push(issue(
            "types_checker_missing",
            "--types: no TypeScript checker found (tsc-rs or tsc in node_modules/.bin or on PATH, or K2_APP_TSC)",
            None,
        ));
        return "missing".into();
    };
    let label = checker.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "tsc".into());
    let mut cmd = Command::new(&checker);
    cmd.arg("--noEmit")
        .arg("-p")
        .arg(&tsconfig)
        .current_dir(root)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for k in ["PATH", "HOME", "LANG", "TMPDIR"] {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    // Windows programs (node's tsc among them) need the system's own
    // variables: without SystemRoot many can't even start.
    #[cfg(windows)]
    for k in ["SystemRoot", "SystemDrive", "windir", "ComSpec", "PATHEXT", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)", "ProgramData"] {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            errors.push(issue("types_checker_failed", format!("--types: can't run {}: {e}", checker.display()), None));
            return label;
        }
    };
    let out_pipe = child.stdout.take();
    let err_pipe = child.stderr.take();
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        if let Some(mut o) = out_pipe {
            let _ = o.by_ref().take(TYPES_OUTPUT_CAP as u64).read_to_end(&mut buf);
        }
        if let Some(mut e) = err_pipe {
            let _ = e.by_ref().take(TYPES_OUTPUT_CAP as u64).read_to_end(&mut buf);
        }
        buf
    });
    let deadline = Instant::now() + TYPES_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let text = String::from_utf8_lossy(&reader.join().unwrap_or_default()).to_string();
    let Some(status) = status else {
        errors.push(issue("types_checker_failed", format!("--types: {label} ran longer than {}s", TYPES_TIMEOUT.as_secs()), None));
        return label;
    };
    let diags = parse_tsc(&text);
    for d in &diags {
        errors.push(d.clone());
    }
    if !status.success() && diags.is_empty() {
        let first = super::framing::strip_controls(text.lines().next().unwrap_or(""));
        errors.push(issue("types_checker_failed", format!("--types: {label} failed without diagnostics: {first}"), None));
    }
    label
}

/// `file(line,col): error TS2322: message` lines (tsc and tsc-rs).
fn parse_tsc(text: &str) -> Vec<Issue> {
    let mut out = Vec::new();
    for l in text.lines() {
        let Some((loc, rest)) = l.split_once("): error ") else { continue };
        let Some((code, msg)) = rest.split_once(':') else { continue };
        let loc = loc.replacen('(', ":", 1);
        out.push(issue(
            "type_error",
            format!("{} {}", code.trim(), super::framing::strip_controls(msg.trim())),
            Some(super::framing::strip_controls(&loc)),
        ));
        if out.len() >= 200 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::{write_executable, TempHome};

    fn app(home: &Path, manifest: &str) -> PathBuf {
        let f = home.join("app");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join(MANIFEST_FILE), manifest).unwrap();
        f
    }

    const GOOD: &str = r#"{"schema":1,"kind":"app","id":"jim","name":"Jim","version":"1.0.0",
      "caps":["thread:read","thread:post"],
      "bindings":{"room":{"type":"room","max":1,"caps":["thread:read"]}},
      "roles":{"client":{"room":["thread:read"]}},
      "requires":{"k2":">=0.1.0","features":["thread-latest"]},
      "host":{"mode":"cmd","start":["node","server.js"]}}"#;

    #[test]
    fn a_good_manifest_passes() {
        let h = TempHome::new();
        let (r, m) = validate_tree(&app(h.path(), GOOD), &Options::default());
        assert!(r.ok, "{r:?}");
        assert_eq!(m.unwrap().id, "jim");
        assert_eq!(r.checks, vec!["manifest", "contract", "code"]);
    }

    /// 0.46.3: `requires.sew` must be a range in requires.k2's form and
    /// `requires.voice` a bool; validate never asks the box (Sew may not
    /// be installed where the App is written).
    #[test]
    fn requires_sew_form_is_checked_and_voice_is_a_bool() {
        let h = TempHome::new();
        let with = |req: &str| GOOD.replace(r#""requires":{"k2":">=0.1.0","features":["thread-latest"]}"#, req);
        for ok in [r#""requires":{"sew":">=0.3.0","voice":true}"#, r#""requires":{"sew":">=0.2.0 <0.5.0"}"#, r#""requires":{"voice":false}"#] {
            let (r, m) = validate_tree(&app(h.path(), &with(ok)), &Options::default());
            assert!(r.ok, "{ok}: {r:?}");
            assert!(m.unwrap().requires.is_some(), "{ok}");
        }
        for (bad, needle) in [
            (r#""requires":{"sew":"newest"}"#, "requires.sew"),
            (r#""requires":{"sew":""}"#, "requires.sew"),
            (r#""requires":{"sew":">=x.y"}"#, "requires.sew"),
        ] {
            let (r, _) = validate_tree(&app(h.path(), &with(bad)), &Options::default());
            assert!(!r.ok, "{bad}: {r:?}");
            let e = r.errors.iter().find(|e| e.at.as_deref() == Some("requires.sew")).unwrap_or_else(|| panic!("{bad}: {r:?}"));
            assert_eq!(e.code, "manifest_invalid", "{e:?}");
            assert!(e.message.contains(needle), "{e:?}");
        }
        // Not a bool: a parse error with a place, never a silent default.
        let (r, m) = validate_tree(&app(h.path(), &with(r#""requires":{"voice":"on"}"#)), &Options::default());
        assert!(!r.ok && m.is_none(), "{r:?}");
        assert_eq!(r.errors[0].code, "manifest_invalid", "{r:?}");
        assert!(range_form_ok(">=0.3.0").is_ok());
        assert!(range_form_ok("0.3").is_ok(), "a bare version means exactly it");
        assert!(range_form_ok("^0.3.0").is_err(), "caret ranges aren't in K2's form");
    }

    /// vs-live 2: Seed validate runs stage 0's oxc pass too. A parse error
    /// blocks (errors), a leak lint doesn't (warnings), files are named.
    #[test]
    fn code_parse_errors_block_and_leak_lints_warn() {
        let h = TempHome::new();
        let f = app(h.path(), GOOD);
        std::fs::write(f.join("server.js"), "const a = ;\n").unwrap();
        let (r, _) = validate_tree(&f, &Options::default());
        assert!(!r.ok, "{r:?}");
        let e = r.errors.iter().find(|e| e.code == "code_parse_error").expect("parse error reported");
        assert_eq!(e.at.as_deref(), Some("server.js:1:11"), "{e:?}");

        std::fs::write(
            f.join("server.js"),
            "const app = require('express')()\nconst hits = []\napp.get('/', (req, res) => { hits.push(req.ip); res.end() })\n",
        )
        .unwrap();
        let (r, _) = validate_tree(&f, &Options::default());
        assert!(r.ok, "a leak lint is a warning, never a block: {r:?}");
        let w = r.warnings.iter().find(|w| w.code == "code_lint").expect("leak lint reported");
        assert!(w.at.as_deref().is_some_and(|a| a.starts_with("server.js:3:")), "{w:?}");
    }

    #[test]
    fn contract_errors_block() {
        let h = TempHome::new();
        let bad = GOOD
            .replace("\"thread:post\"]", "\"thread:post\",\"no:such\"]")
            .replace("\"roles\":{\"client\":{\"room\":[\"thread:read\"]}}", "\"roles\":{\"client\":{\"nope\":[\"files:write\"]}}")
            .replace("\"thread-latest\"", "\"made-up\"")
            .replace(">=0.1.0", ">=99.0.0");
        let (r, _) = validate_tree(&app(h.path(), &bad), &Options::default());
        assert!(!r.ok);
        let codes: Vec<&str> = r.errors.iter().map(|e| e.code.as_str()).collect();
        for want in ["cap_unknown", "role_invalid", "cap_outside_ceiling", "feature_unknown", "k2_version_unmet"] {
            assert!(codes.contains(&want), "{want} missing from {codes:?}");
        }
    }

    #[test]
    fn missing_or_broken_manifest() {
        let h = TempHome::new();
        let f = h.path().join("empty");
        std::fs::create_dir_all(&f).unwrap();
        let (r, m) = validate_tree(&f, &Options::default());
        assert!(!r.ok && m.is_none());
        assert_eq!(r.errors[0].code, "manifest_missing");
        let (r, _) = validate_tree(&app(h.path(), "{\"schema\":1,"), &Options::default());
        assert_eq!(r.errors[0].code, "manifest_invalid");
        assert!(r.errors[0].at.as_deref().unwrap().starts_with("manifest.json:"), "{r:?}");
    }

    #[test]
    fn types_errors_block_and_a_missing_checker_is_an_error() {
        let h = TempHome::new();
        let f = app(h.path(), GOOD);
        // No tsconfig: a warning, not a pass-by-skip of an asked check.
        let (r, _) = validate_tree(&f, &Options { types: true, checker: None });
        assert!(r.ok);
        assert_eq!(r.warnings.iter().filter(|w| w.code == "types_no_tsconfig").count(), 1);
        std::fs::write(f.join("tsconfig.json"), "{}").unwrap();
        let (r, _) = validate_tree(&f, &Options { types: true, checker: Some(h.path().join("no-such-tsc")) });
        assert!(!r.ok);
        assert_eq!(r.errors[0].code, "types_checker_failed");
        let fake = h.path().join("bin/fake-tsc");
        std::fs::create_dir_all(fake.parent().unwrap()).unwrap();
        write_executable(&fake, "#!/bin/sh\necho 'src/a.ts(3,7): error TS2322: Type \"x\" is not assignable.'\nexit 2\n");
        let (r, _) = validate_tree(&f, &Options { types: true, checker: Some(fake.clone()) });
        assert!(!r.ok);
        assert_eq!(r.errors[0].code, "type_error");
        assert_eq!(r.errors[0].at.as_deref(), Some("src/a.ts:3,7"));
        assert!(r.checks.iter().any(|c| c == "types:fake-tsc"));
        write_executable(&fake, "#!/bin/sh\nexit 0\n");
        let (r, _) = validate_tree(&f, &Options { types: true, checker: Some(fake) });
        assert!(r.ok, "{r:?}");
    }

    /// 0.46.4 (PA4): `requires.postgres` is a range of whole majors;
    /// validate checks only the form, never the box.
    #[test]
    fn requires_postgres_form_is_checked() {
        let h = TempHome::new();
        let with = |req: &str| GOOD.replace(r#""requires":{"k2":">=0.1.0","features":["thread-latest"]}"#, req);
        for ok in [r#""requires":{"postgres":"17"}"#, r#""requires":{"postgres":">=16 <18"}"#, r#""requires":{"postgres":">=17","sew":">=0.3.0"}"#] {
            let (r, m) = validate_tree(&app(h.path(), &with(ok)), &Options::default());
            assert!(r.ok, "{ok}: {r:?}");
            assert!(m.unwrap().requires.unwrap().postgres.is_some(), "{ok}");
        }
        for bad in [
            r#""requires":{"postgres":"17.2"}"#,
            r#""requires":{"postgres":"latest"}"#,
            r#""requires":{"postgres":""}"#,
            r#""requires":{"postgres":">=18 <17"}"#,
            r#""requires":{"postgres":"=>17"}"#,
            r#""requires":{"postgres":"^17"}"#,
        ] {
            let (r, _) = validate_tree(&app(h.path(), &with(bad)), &Options::default());
            assert!(!r.ok, "{bad}: {r:?}");
            let e = r.errors.iter().find(|e| e.at.as_deref() == Some("requires.postgres")).unwrap_or_else(|| panic!("{bad}: {r:?}"));
            assert_eq!(e.code, "manifest_invalid", "{e:?}");
        }
        // A number, not a string: a parse error with a place.
        let (r, m) = validate_tree(&app(h.path(), &with(r#""requires":{"postgres":17}"#)), &Options::default());
        assert!(!r.ok && m.is_none(), "{r:?}");
        assert_eq!(pg_range_ok("17", 17), Ok(true));
        assert_eq!(pg_range_ok("17", 16), Ok(false));
        assert_eq!(pg_range_ok(">=16 <18", 17), Ok(true));
        assert_eq!(pg_range_ok(">=16 <18", 18), Ok(false));
        assert_eq!(pg_range_ok(">16", 16), Ok(false));
        assert_eq!(pg_range_ok("<=16", 16), Ok(true));
        assert!(pg_range_ok("17.0", 17).is_err());
        assert!(pg_range_ok("017", 17).is_err());
    }

    #[test]
    fn version_ranges() {
        assert_eq!(k2_range_ok(">=0.46.0", "0.46.0"), Ok(true));
        assert_eq!(k2_range_ok(">=0.46.0 <0.47", "0.46.3"), Ok(true));
        assert_eq!(k2_range_ok(">=0.46.0", "0.45.9"), Ok(false));
        assert!(k2_range_ok("~1", "0.46.0").is_err());
    }
}
