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
//! Stage 0's oxc lints (S4) plug in here as more warnings.

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
    if opts.types {
        let label = check_types(root, opts, &mut errors, &mut warnings);
        checks.push(format!("types:{label}"));
    }
    (Report { ok: errors.is_empty(), errors, warnings, checks }, Some(m))
}

fn check_manifest(m: &Manifest, errors: &mut Vec<Issue>, warnings: &mut Vec<Issue>) {
    let at = |f: &str| Some(format!("{MANIFEST_FILE} {f}"));
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
        } else if k.starts_with("K2_") || k == "PORT" || k == "PATH" || k == "HOME" {
            errors.push(issue("manifest_invalid", format!("env name \"{k}\" is set by K2 and can't be declared"), at("env")));
        } else if sensitive_name(k) {
            errors.push(issue(
                "env_secret_inline",
                format!("env \"{k}\" names a secret; a manifest never holds secret values (grant it from the vault)"),
                at("env"),
            ));
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
    }
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
        assert_eq!(r.checks, vec!["manifest", "contract"]);
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

    #[test]
    fn version_ranges() {
        assert_eq!(k2_range_ok(">=0.46.0", "0.46.0"), Ok(true));
        assert_eq!(k2_range_ok(">=0.46.0 <0.47", "0.46.3"), Ok(true));
        assert_eq!(k2_range_ok(">=0.46.0", "0.45.9"), Ok(false));
        assert!(k2_range_ok("~1", "0.46.0").is_err());
    }
}
