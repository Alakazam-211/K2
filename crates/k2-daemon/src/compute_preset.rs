//! `.k2/compute.toml`: per-repo compute defaults and named presets
//! (prd-compute-easy-mode-0467 CE-3; CE-A1: the daemon reads the file).
//!
//! ```toml
//! [defaults]
//! os = "linux"
//! mem = "8G"
//! needs = ["rust@1.99.1", "cmake@3", "cargo-nextest"]
//!
//! [presets.daemon]
//! mem = "7G"
//! needs = ["rust@1.99.1", "cmake@3", "cargo-nextest", "sys:pkg-config"]
//! env = { CARGO_BUILD_JOBS = "2" }
//! ```
//!
//! Keys: `os`, `arch`, `mem`, `cpus`, `needs`, `env`, `timeout`,
//! `stall_timeout`, `tier`, `cache`. A top-level `needs = [...]` (the
//! format the node already reads from a bundle) counts as
//! `[defaults].needs`.
//!
//! **Where:** `.k2` is gitignored, so the file never reaches a node in a
//! bundle; only the caller's machine has it. The daemon runs there and
//! looks, under the workspace jail, in the caller's checkout (a worktree
//! inside the workspace), then the `--repo` folder, then the workspace
//! folder; the first file found is used. The CLI only passes its folder
//! (`repoDir`, as for every run) and `preset`. A `computeToml` body field
//! (the file's text) stays for a CLI that talks to a daemon without the
//! checkout; this daemon parses it the same way.
//!
//! **Precedence:** flags > the preset > `[defaults]`. `needs` and `env`
//! merge per tool and per name in that order. Needs from the file are sent
//! as explicit needs (a failed install fails the job). The effective
//! settings and where each came from go in `--plan` and on the job
//! (`JobCtl.effective`).
//!
//! A bad file is a loud refusal (`compute_toml_invalid`, exit 2) naming
//! the line; an unknown preset is `preset_not_found` (exit 2) listing the
//! presets that exist. Never a silent skip.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The file, relative to a checkout or the workspace folder.
pub const FILE: &str = ".k2/compute.toml";
/// Largest file read.
pub const MAX_BYTES: u64 = 16 * 1024;

pub mod codes {
    /// The file doesn't parse or holds a bad value (exit 2).
    pub const COMPUTE_TOML_INVALID: &str = "compute_toml_invalid";
    /// `--preset NAME` names no preset (or there is no file) (exit 2).
    pub const PRESET_NOT_FOUND: &str = "preset_not_found";
}

/// One section's settings (`[defaults]` or `[presets.<name>]`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    pub os: Option<String>,
    pub arch: Option<String>,
    pub mem_bytes: Option<u64>,
    pub cpus: Option<f64>,
    pub needs: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub timeout_secs: Option<i64>,
    /// 0 = off.
    pub stall_secs: Option<i64>,
    /// `standard` (normal), `batch` or `ci`.
    pub tier: Option<String>,
    pub cache: Option<String>,
}

/// A parsed `.k2/compute.toml`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct File {
    pub defaults: Settings,
    pub presets: BTreeMap<String, Settings>,
}

/// The file a run uses: where it came from (relative to the workspace
/// folder, or `request` for a `computeToml` body) and what it says.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub source: String,
    pub file: File,
}

const KEYS: &[&str] = &["os", "arch", "mem", "cpus", "needs", "env", "timeout", "stall_timeout", "tier", "cache"];

/// The 1-based line of `key` in `[section]` (`None` = top level), for a
/// refusal that names it. Best effort: 0 when not found.
fn line_of(text: &str, section: Option<&str>, key: &str) -> usize {
    let mut cur: Option<String> = None;
    for (i, l) in text.lines().enumerate() {
        let t = l.trim();
        if let Some(h) = t.strip_prefix('[').and_then(|h| h.split(']').next()) {
            cur = Some(h.trim().replace(['"', '\'', ' '], ""));
            continue;
        }
        let same = match (section, cur.as_deref()) {
            (None, None) => true,
            (Some(s), Some(c)) => s == c,
            _ => false,
        };
        if same && t.strip_prefix(key).is_some_and(|r| r.trim_start().starts_with('=')) {
            return i + 1;
        }
    }
    0
}

fn at(text: &str, section: Option<&str>, key: &str) -> String {
    let where_ = section.map(|s| format!("[{s}] ")).unwrap_or_default();
    match line_of(text, section, key) {
        0 => format!("{FILE}: {where_}{key}"),
        n => format!("{FILE} line {n}: {where_}{key}"),
    }
}

/// `90s`, `30m`, `2h`, `1d`, or whole seconds.
pub fn parse_secs(v: &toml::Value) -> Option<i64> {
    match v {
        toml::Value::Integer(n) if *n > 0 => Some(*n),
        toml::Value::String(s) => {
            let s = s.trim();
            let (num, mul) = match s.chars().last()? {
                's' => (&s[..s.len() - 1], 1),
                'm' => (&s[..s.len() - 1], 60),
                'h' => (&s[..s.len() - 1], 3600),
                'd' => (&s[..s.len() - 1], 86_400),
                c if c.is_ascii_digit() => (s, 1),
                _ => return None,
            };
            num.parse::<i64>().ok().filter(|n| *n > 0).map(|n| n * mul)
        }
        _ => None,
    }
}

/// `8G`, `512M`, `1.5G` (also `GB`/`GiB`), or whole bytes.
pub fn parse_bytes(v: &toml::Value) -> Option<u64> {
    match v {
        toml::Value::Integer(n) if *n > 0 => Some(*n as u64),
        toml::Value::String(s) => {
            let u = s.trim().to_ascii_uppercase();
            let u = u.trim_end_matches("IB").trim_end_matches('B');
            let (num, mul) = match u.chars().last()? {
                'G' => (&u[..u.len() - 1], 1u64 << 30),
                'M' => (&u[..u.len() - 1], 1u64 << 20),
                c if c.is_ascii_digit() => (u, 1),
                _ => return None,
            };
            let n: f64 = num.trim().parse().ok()?;
            (n > 0.0 && n.is_finite()).then(|| (n * mul as f64) as u64)
        }
        _ => None,
    }
}

fn section(text: &str, name: Option<&str>, t: &toml::Table) -> Result<Settings, String> {
    let mut out = Settings::default();
    for (k, v) in t {
        let bad = |what: &str| format!("{}: {what}", at(text, name, k));
        match k.as_str() {
            "os" | "arch" | "cache" | "tier" => {
                let s = v.as_str().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| bad("a string"))?.to_string();
                match k.as_str() {
                    "os" => out.os = Some(s),
                    "arch" => out.arch = Some(s),
                    "cache" => {
                        if !k2_core::compute::proto::cache::valid_name(&s) {
                            return Err(bad("a cache name: 1–64 of a-z 0-9 . _ -, starting with a letter or digit"));
                        }
                        out.cache = Some(s)
                    }
                    _ => {
                        let tier = match s.as_str() {
                            "standard" | "normal" => "normal",
                            "batch" => "batch",
                            "ci" => "ci",
                            "priority" => return Err(bad("priority needs a reason on the command line: --priority --why \"…\"")),
                            _ => return Err(bad("standard, batch or ci")),
                        };
                        out.tier = Some(tier.to_string())
                    }
                }
            }
            "mem" => out.mem_bytes = Some(parse_bytes(v).ok_or_else(|| bad("a size like \"8G\" or \"512M\""))?),
            "cpus" => {
                let c = match v {
                    toml::Value::Integer(n) => *n as f64,
                    toml::Value::Float(f) => *f,
                    _ => return Err(bad("a number of CPUs (e.g. 4 or 0.5)")),
                };
                if !(c > 0.0 && c <= 1024.0) {
                    return Err(bad("between 0 and 1024"));
                }
                out.cpus = Some(c);
            }
            "timeout" => out.timeout_secs = Some(parse_secs(v).filter(|s| *s <= 7 * 86_400).ok_or_else(|| bad("a duration like \"2h\" or \"90m\" (at most 7d)"))?),
            "stall_timeout" => {
                out.stall_secs = Some(match v {
                    toml::Value::String(s) if matches!(s.trim(), "off" | "none" | "0") => 0,
                    toml::Value::Integer(0) => 0,
                    _ => parse_secs(v).filter(|s| *s <= 7 * 86_400).ok_or_else(|| bad("a duration like \"45m\", or \"off\""))?,
                })
            }
            "needs" => {
                let a = v.as_array().ok_or_else(|| bad("a list like [\"rust@stable\", \"cmake@3\"]"))?;
                let raw: Vec<String> = a.iter().map(|x| x.as_str().map(str::to_string).ok_or_else(|| bad("strings like \"node@22\""))).collect::<Result<_, _>>()?;
                out.needs = crate::compute_place::check_needs(&raw).map_err(|e| bad(&e.message))?;
            }
            "env" => {
                let m = v.as_table().ok_or_else(|| bad("a table like { CARGO_BUILD_JOBS = \"2\" }"))?;
                for (name, x) in m {
                    let val = match x {
                        toml::Value::String(s) => s.clone(),
                        toml::Value::Integer(n) => n.to_string(),
                        _ => return Err(bad(&format!("{name}: a string value"))),
                    };
                    if let Some(code) = k2_core::compute::proto::env::env_name_refusal(name) {
                        return Err(bad(&format!("{name}: {code} (K2 tokens, sockets and the node's own variables can't be set)")));
                    }
                    out.env.insert(name.clone(), val);
                }
            }
            other => return Err(format!("{}: unknown key {other:?} (keys: {})", at(text, name, other), KEYS.join(", "))),
        }
    }
    Ok(out)
}

/// Parse the file's text. `Err` names the line (`.k2/compute.toml line 3:
/// [presets.mac] mem: a size like "8G"`).
pub fn parse(text: &str) -> Result<File, String> {
    let t: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        let line = e.span().map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1);
        format!("{FILE}{}: {}", line.map(|n| format!(" line {n}")).unwrap_or_default(), e.message())
    })?;
    let mut out = File::default();
    for (k, v) in &t {
        match k.as_str() {
            "defaults" => {
                let s = v.as_table().ok_or_else(|| format!("{}: [defaults] is a table", at(text, None, k)))?;
                let mut d = section(text, Some("defaults"), s)?;
                if !out.defaults.needs.is_empty() && !d.needs.is_empty() {
                    return Err(format!("{FILE}: needs is both top level and in [defaults]; keep one"));
                }
                if d.needs.is_empty() {
                    d.needs = std::mem::take(&mut out.defaults.needs);
                }
                out.defaults = d;
            }
            "presets" => {
                let ps = v.as_table().ok_or_else(|| format!("{}: presets are [presets.<name>] tables", at(text, None, k)))?;
                for (name, p) in ps {
                    if !valid_preset_name(name) {
                        return Err(format!("{FILE}: [presets.{name}]: a preset name is 1–40 of a-z 0-9 . _ -"));
                    }
                    let sec = format!("presets.{name}");
                    let p = p.as_table().ok_or_else(|| format!("{FILE}: [{sec}] is a table"))?;
                    out.presets.insert(name.clone(), section(text, Some(&sec), p)?);
                }
            }
            // The node's format: top-level needs = [...] (= [defaults].needs).
            "needs" => {
                let s = section(text, None, &toml::Table::from_iter([(k.clone(), v.clone())]))?;
                if !out.defaults.needs.is_empty() {
                    return Err(format!("{FILE}: needs is both top level and in [defaults]; keep one"));
                }
                out.defaults.needs = s.needs;
            }
            other => {
                return Err(format!("{}: unknown key {other:?} (the file holds [defaults], [presets.<name>] and a top-level needs)", at(text, None, other)))
            }
        }
    }
    Ok(out)
}

/// 1–40 of `a-z 0-9 . _ -`, starting with a letter or digit.
pub fn valid_preset_name(n: &str) -> bool {
    (1..=40).contains(&n.len())
        && n.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

/// Read `FILE` under `dir` (at most [`MAX_BYTES`]), only when it stays
/// inside `jail` after symlinks. `Ok(None)`: no file there.
fn read_in(dir: &Path, jail: &Path) -> Result<Option<String>, String> {
    let p = dir.join(FILE);
    if !p.is_file() {
        return Ok(None);
    }
    let real = dunce::canonicalize(&p).map_err(|e| format!("{FILE}: {e}"))?;
    if !real.starts_with(jail) {
        return Err(format!("{} leads outside the workspace (a symlink?); K2 reads only files inside it", p.display()));
    }
    let mut f = std::fs::File::open(&real).map_err(|e| format!("{FILE}: {e}"))?;
    if f.metadata().map(|m| m.len()).unwrap_or(0) > MAX_BYTES {
        return Err(format!("{FILE} is over {} KiB", MAX_BYTES / 1024));
    }
    let mut s = String::new();
    f.by_ref().take(MAX_BYTES).read_to_string(&mut s).map_err(|e| format!("{FILE}: {e} (it must be UTF-8 text)"))?;
    Ok(Some(s))
}

/// Find and parse the file for a run (CE-A1): the caller's checkout
/// (`repoDir`, when inside the workspace), then `extra` (the `--repo`
/// checkout), then the workspace folder; the first found wins. A
/// `computeToml` text in the body wins over all. `Ok(None)`: no file.
pub fn load(ws_path: &str, v: &Value, extra: &[PathBuf]) -> Result<Option<Loaded>, String> {
    if let Some(text) = v.get("computeToml").and_then(Value::as_str) {
        if text.len() as u64 > MAX_BYTES {
            return Err(format!("{FILE} is over {} KiB", MAX_BYTES / 1024));
        }
        return Ok(Some(Loaded { source: "request".into(), file: parse(text)? }));
    }
    let Ok(jail) = dunce::canonicalize(ws_path) else { return Ok(None) };
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = v.get("repoDir").and_then(Value::as_str).filter(|d| !d.is_empty()) {
        if let Ok(c) = dunce::canonicalize(d) {
            if c.starts_with(&jail) {
                dirs.push(c);
            }
        }
    }
    for d in extra {
        if let Ok(c) = dunce::canonicalize(d) {
            if c.starts_with(&jail) {
                dirs.push(c);
            }
        }
    }
    dirs.push(jail.clone());
    let mut seen: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if seen.contains(&d) {
            continue;
        }
        seen.push(d.clone());
        if let Some(text) = read_in(&d, &jail)? {
            let rel = d.strip_prefix(&jail).map(|r| r.join(FILE)).unwrap_or_else(|_| PathBuf::from(FILE));
            let source = rel.to_string_lossy().replace('\\', "/");
            return Ok(Some(Loaded { source, file: parse(&text)? }));
        }
    }
    Ok(None)
}

/// A flag set this body key (present, not null, not empty).
fn has(v: &Value, k: &str) -> bool {
    v.get(k).is_some_and(|x| !x.is_null() && x.as_str() != Some(""))
}

/// One scalar: the flag's (`None`: it is in the body already), else the
/// preset's, else the defaults'.
fn pick<T: Clone>(flag: bool, p: Option<&T>, d: Option<&T>) -> Option<(Option<T>, &'static str)> {
    if flag {
        Some((None, "flag"))
    } else if let Some(x) = p {
        Some((Some(x.clone()), "preset"))
    } else {
        d.map(|x| (Some(x.clone()), "defaults"))
    }
}

/// "2h" / "90m" / "45s".
fn secs_words(s: i64) -> String {
    if s >= 3600 && s % 3600 == 0 {
        format!("{}h", s / 3600)
    } else if s >= 60 && s % 60 == 0 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

/// Fill the run body `v` from the file and `--preset` where no flag set
/// a value (flags > preset > defaults), and return the effective settings
/// with where each came from (`flag`, `preset`, `defaults`; `null` = not
/// set). `Err((code, message))`.
pub fn apply(v: &mut Value, loaded: Option<&Loaded>, preset: Option<&str>) -> Result<Value, (&'static str, String)> {
    let empty = Settings::default();
    let (defaults, chosen): (&Settings, Option<&Settings>) = match (loaded, preset) {
        (None, None) => (&empty, None),
        (None, Some(p)) => {
            return Err((codes::PRESET_NOT_FOUND, format!("--preset {p}: no {FILE} found (K2 looks in your checkout, the --repo folder, then the workspace folder)")))
        }
        (Some(l), None) => (&l.file.defaults, None),
        (Some(l), Some(p)) => match l.file.presets.get(p) {
            Some(s) => (&l.file.defaults, Some(s)),
            None => {
                let names: Vec<&str> = l.file.presets.keys().map(String::as_str).collect();
                return Err((
                    codes::PRESET_NOT_FOUND,
                    format!(
                        "--preset {p}: {} has {}",
                        l.source,
                        if names.is_empty() { "no presets".to_string() } else { format!("presets {}", names.join(", ")) }
                    ),
                ));
            }
        },
    };
    let p = chosen;
    let mut eff = json!({
        "source": loaded.map(|l| l.source.clone()),
        "preset": preset,
    });
    // os / arch / cache: strings, under the same key in the body.
    for key in ["os", "arch", "cache"] {
        let get = |s: &Settings| match key {
            "os" => s.os.clone(),
            "arch" => s.arch.clone(),
            _ => s.cache.clone(),
        };
        let (pv, dv) = (p.and_then(get), get(defaults));
        eff[key] = match pick(has(v, key), pv.as_ref(), dv.as_ref()) {
            Some((None, f)) => json!({"value": v[key].clone(), "from": f}),
            Some((Some(x), f)) => {
                v[key] = Value::String(x.clone());
                json!({"value": x, "from": f})
            }
            None => Value::Null,
        };
    }
    // mem.
    eff["mem"] = match pick(has(v, "memBytes"), p.and_then(|s| s.mem_bytes).as_ref(), defaults.mem_bytes.as_ref()) {
        Some((None, f)) => json!({"value": v["memBytes"].as_u64().map(crate::compute_place::mem_words), "bytes": v["memBytes"].clone(), "from": f}),
        Some((Some(b), f)) => {
            v["memBytes"] = json!(b);
            json!({"value": crate::compute_place::mem_words(b), "bytes": b, "from": f})
        }
        None => Value::Null,
    };
    eff["cpus"] = match pick(has(v, "cpus"), p.and_then(|s| s.cpus).as_ref(), defaults.cpus.as_ref()) {
        Some((None, f)) => json!({"value": v["cpus"].clone(), "from": f}),
        Some((Some(c), f)) => {
            v["cpus"] = json!(c);
            json!({"value": c, "from": f})
        }
        None => Value::Null,
    };
    eff["timeout"] = match pick(has(v, "timeoutSecs"), p.and_then(|s| s.timeout_secs).as_ref(), defaults.timeout_secs.as_ref()) {
        Some((None, f)) => json!({"value": v["timeoutSecs"].as_i64().map(secs_words), "secs": v["timeoutSecs"].clone(), "from": f}),
        Some((Some(t), f)) => {
            v["timeoutSecs"] = json!(t);
            json!({"value": secs_words(t), "secs": t, "from": f})
        }
        None => Value::Null,
    };
    eff["stall"] = match pick(has(v, "stallSecs"), p.and_then(|s| s.stall_secs).as_ref(), defaults.stall_secs.as_ref()) {
        Some((None, f)) => {
            let s = v["stallSecs"].as_i64();
            json!({"value": s.map(|s| if s == 0 { "off".to_string() } else { secs_words(s) }), "secs": s, "from": f})
        }
        Some((Some(t), f)) => {
            v["stallSecs"] = json!(t);
            json!({"value": if t == 0 { "off".to_string() } else { secs_words(t) }, "secs": t, "from": f})
        }
        None => Value::Null,
    };
    let tier_flag = has(v, "tier") || v.get("gate").and_then(Value::as_bool) == Some(true) || v.get("priority").and_then(Value::as_bool) == Some(true);
    eff["tier"] = match pick(tier_flag, p.and_then(|s| s.tier.clone()).as_ref(), defaults.tier.as_ref()) {
        Some((None, f)) => {
            let t = v.get("tier").and_then(Value::as_str).map(str::to_string).or_else(|| {
                if v.get("gate").and_then(Value::as_bool) == Some(true) {
                    Some("ci".into())
                } else {
                    Some("priority".into())
                }
            });
            json!({"value": t, "from": f})
        }
        Some((Some(t), f)) => {
            v["tier"] = Value::String(t.clone());
            json!({"value": t, "from": f})
        }
        None => Value::Null,
    };
    // needs: per tool, flag > preset > defaults.
    let flag_needs: Vec<String> = match v.get("needs") {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        Some(Value::String(s)) => s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    let key = |t: &str| -> String {
        crate::compute_place::split_rust_components(t)
            .ok()
            .and_then(|(b, _)| k2_core::compute::proto::needs::parse(&b).ok())
            .map(|n| n.key())
            .unwrap_or_else(|| t.to_string())
    };
    let mut needs: Vec<String> = flag_needs.clone();
    let mut sources: Vec<&str> = Vec::new();
    if !flag_needs.is_empty() {
        sources.push("flag");
    }
    for (list, src) in [(p.map(|s| s.needs.as_slice()).unwrap_or(&[]), "preset"), (defaults.needs.as_slice(), "defaults")] {
        let mut added = false;
        for t in list {
            if !needs.iter().any(|n| key(n) == key(t)) {
                needs.push(t.clone());
                added = true;
            }
        }
        if added {
            sources.push(src);
        }
    }
    eff["needs"] = if needs.is_empty() {
        Value::Null
    } else {
        if needs.len() > flag_needs.len() {
            v["needs"] = json!(needs);
        }
        json!({"value": needs, "from": sources.join("+")})
    };
    // env: per name, flag > preset > defaults. Names only in the record.
    let mut env: BTreeMap<String, String> = v.get("env").and_then(Value::as_object).map(|m| m.iter().filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string()))).collect()).unwrap_or_default();
    let mut env_sources: Vec<&str> = Vec::new();
    if !env.is_empty() {
        env_sources.push("flag");
    }
    for (m, src) in [(p.map(|s| &s.env), "preset"), (Some(&defaults.env), "defaults")] {
        let Some(m) = m else { continue };
        let mut added = false;
        for (k, x) in m {
            if !env.contains_key(k) {
                env.insert(k.clone(), x.clone());
                added = true;
            }
        }
        if added {
            env_sources.push(src);
        }
    }
    eff["env"] = if env.is_empty() {
        Value::Null
    } else {
        let names: Vec<&String> = env.keys().collect();
        let e = json!({"value": names, "from": env_sources.join("+")});
        v["env"] = json!(env);
        e
    };
    Ok(eff)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRD_FILE: &str = r#"
[defaults]
os = "linux"
mem = "8G"
needs = ["rust@1.99.1", "cmake@3", "cargo-nextest"]

[presets.daemon]
mem = "7G"
needs = ["rust@1.99.1", "cmake@3", "cargo-nextest", "sys:pkg-config"]
env = { CARGO_BUILD_JOBS = "2" }

[presets.mac]
os = "macos"
mem = "6G"
needs = ["rust@1.99.1", "cmake@3", "bun@1"]
"#;

    fn loaded(text: &str) -> Loaded {
        Loaded { source: FILE.into(), file: parse(text).expect("parses") }
    }

    /// CE-3 acceptance 1: defaults fill a bare run, each `from: defaults`.
    #[test]
    fn defaults_fill_a_bare_run() {
        let l = loaded(PRD_FILE);
        let mut v = json!({"argv": ["true"]});
        let e = apply(&mut v, Some(&l), None).unwrap();
        assert_eq!((v["os"].as_str(), v["memBytes"].as_u64()), (Some("linux"), Some(8 << 30)));
        assert_eq!(v["needs"], json!(["rust@1.99.1", "cmake@3", "cargo-nextest"]));
        assert_eq!(e["os"], json!({"value": "linux", "from": "defaults"}));
        assert_eq!(e["mem"], json!({"value": "8G", "bytes": 8u64 << 30, "from": "defaults"}));
        assert_eq!(e["needs"]["from"], "defaults");
        assert_eq!((e["source"].as_str(), e["preset"].is_null(), e["cpus"].is_null(), e["env"].is_null()), (Some(FILE), true, true, true));
        for k in ["source", "preset", "os", "arch", "mem", "cpus", "needs", "env", "timeout", "stall", "tier", "cache"] {
            assert!(e.get(k).is_some(), "{k} always present: {e}");
        }
    }

    /// CE-3 acceptance 2: `--preset daemon --mem 24G`: mem from the flag,
    /// needs and env from the preset, os from the defaults.
    #[test]
    fn a_flag_beats_the_preset_beats_the_defaults() {
        let l = loaded(PRD_FILE);
        let mut v = json!({"argv": ["true"], "memBytes": 24u64 << 30});
        let e = apply(&mut v, Some(&l), Some("daemon")).unwrap();
        assert_eq!(e["mem"]["from"], "flag");
        assert_eq!(v["memBytes"].as_u64(), Some(24 << 30), "the flag stays");
        assert_eq!(e["needs"], json!({"value": ["rust@1.99.1", "cmake@3", "cargo-nextest", "sys:pkg-config"], "from": "preset"}));
        assert_eq!(e["env"], json!({"value": ["CARGO_BUILD_JOBS"], "from": "preset"}), "names only");
        assert_eq!(v["env"]["CARGO_BUILD_JOBS"], "2", "the value goes in the body");
        assert_eq!(e["os"]["from"], "defaults");
        // A flag need replaces only its own tool; the rest still come.
        let mut v = json!({"needs": ["rust@nightly", "node@22"], "env": {"CARGO_BUILD_JOBS": "8"}});
        let e = apply(&mut v, Some(&l), Some("mac")).unwrap();
        assert_eq!(v["needs"], json!(["rust@nightly", "node@22", "cmake@3", "bun@1", "cargo-nextest"]));
        assert_eq!(e["needs"]["from"], "flag+preset+defaults");
        assert_eq!(v["env"]["CARGO_BUILD_JOBS"], "8", "the flag's env wins");
        assert_eq!((e["os"]["value"].as_str(), e["os"]["from"].as_str()), (Some("macos"), Some("preset")));
        // A tier flag (--gate) keeps the file's tier out.
        let l = loaded("[defaults]\ntier = \"batch\"\n");
        let mut v = json!({"gate": true});
        assert_eq!(apply(&mut v, Some(&l), None).unwrap()["tier"], json!({"value": "ci", "from": "flag"}));
        assert!(v.get("tier").is_none());
    }

    /// CE-3 acceptance 3: a bad file names the line; nothing is guessed.
    #[test]
    fn a_bad_file_is_refused_with_its_line() {
        let e = parse("[defaults]\nos = \"linux\"\nmem = [\n").unwrap_err();
        assert!(e.starts_with(".k2/compute.toml line "), "{e}");
        let e = parse("[defaults]\nos = \"linux\"\n\n[presets.mac]\nmem = \"lots\"\n").unwrap_err();
        assert!(e.starts_with(".k2/compute.toml line 5: [presets.mac] mem: a size"), "{e}");
        let e = parse("[defaults]\ncolour = \"blue\"\n").unwrap_err();
        assert!(e.contains("line 2") && e.contains("unknown key \"colour\""), "{e}");
        let e = parse("[defaults]\nneeds = [\"make@4\"]\n").unwrap_err();
        assert!(e.contains("line 2: [defaults] needs:") && e.contains("doesn't install"), "{e}");
        let e = parse("[defaults]\ntier = \"priority\"\n").unwrap_err();
        assert!(e.contains("--why"), "{e}");
        let e = parse("[defaults]\nenv = { K2_HOOK_TOKEN = \"x\" }\n").unwrap_err();
        assert!(e.contains("env_reaches_daemon"), "{e}");
        let e = parse("needs = [\"node@22\"]\n[defaults]\nneeds = [\"node@20\"]\n").unwrap_err();
        assert!(e.contains("both"), "{e}");
        // The node's own format: top-level needs = [defaults].needs.
        let f = parse("needs = [\"node@22\", \"rust+clippy\"]\n[defaults]\nmem = \"512M\"\n").unwrap();
        assert_eq!((f.defaults.needs.clone(), f.defaults.mem_bytes), (vec!["node@22".to_string(), "rust@stable+clippy".to_string()], Some(512 << 20)));
        // Durations and sizes.
        assert_eq!(parse("[defaults]\ntimeout = \"2h\"\nstall_timeout = \"off\"\ncpus = 0.5\n").unwrap().defaults, Settings {
            timeout_secs: Some(7200),
            stall_secs: Some(0),
            cpus: Some(0.5),
            ..Default::default()
        });
        assert_eq!(parse_bytes(&toml::Value::String("1.5G".into())), Some(3 << 29));
        assert_eq!(parse_bytes(&toml::Value::String("8GiB".into())), Some(8 << 30));
        assert_eq!(parse_secs(&toml::Value::String("90m".into())), Some(5400));
        assert_eq!(parse_secs(&toml::Value::String("soon".into())), None);
    }

    /// An unknown preset lists the ones there are; a preset without a file
    /// says where K2 looked.
    #[test]
    fn an_unknown_preset_lists_the_presets() {
        let l = loaded(PRD_FILE);
        let (code, msg) = apply(&mut json!({}), Some(&l), Some("linux")).unwrap_err();
        assert_eq!(code, codes::PRESET_NOT_FOUND);
        assert!(msg.contains("presets daemon, mac"), "{msg}");
        let (code, msg) = apply(&mut json!({}), None, Some("daemon")).unwrap_err();
        assert_eq!(code, codes::PRESET_NOT_FOUND);
        assert!(msg.contains("no .k2/compute.toml found"), "{msg}");
        // No file, no preset: the record still says what the flags set.
        let mut v = json!({"memBytes": 2u64 << 30, "os": "linux"});
        let e = apply(&mut v, None, None).unwrap();
        assert_eq!((e["source"].is_null(), e["mem"]["from"].as_str(), e["os"]["value"].as_str()), (true, Some("flag"), Some("linux")));
    }

    /// The file is read from the caller's checkout first, then the
    /// workspace folder; never through a symlink out of the workspace.
    #[test]
    fn the_file_is_found_under_the_workspace_jail() {
        let base = std::env::temp_dir().join(format!("k2-preset-{}", std::process::id()));
        let ws = base.join("ws");
        let wt = ws.join(".claude/worktrees/x");
        std::fs::create_dir_all(wt.join(".k2")).unwrap();
        std::fs::create_dir_all(ws.join(".k2")).unwrap();
        std::fs::write(ws.join(FILE), "[defaults]\nmem = \"4G\"\n").unwrap();
        let ws_s = ws.to_string_lossy().to_string();
        let got = load(&ws_s, &json!({}), &[]).unwrap().expect("the workspace file");
        assert_eq!((got.source.as_str(), got.file.defaults.mem_bytes), (FILE, Some(4 << 30)));
        std::fs::write(wt.join(FILE), "[defaults]\nmem = \"2G\"\n").unwrap();
        let got = load(&ws_s, &json!({"repoDir": wt.to_string_lossy()}), &[]).unwrap().expect("the checkout's file");
        assert_eq!((got.source.as_str(), got.file.defaults.mem_bytes), (".claude/worktrees/x/.k2/compute.toml", Some(2 << 30)));
        // A repoDir outside the workspace is ignored (the workspace file wins).
        let got = load(&ws_s, &json!({"repoDir": base.to_string_lossy()}), &[]).unwrap().expect("file");
        assert_eq!(got.file.defaults.mem_bytes, Some(4 << 30));
        // A symlink out of the jail is refused, loudly.
        let outside = base.join("outside.toml");
        std::fs::write(&outside, "[defaults]\nmem = \"1G\"\n").unwrap();
        std::fs::remove_file(wt.join(FILE)).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, wt.join(FILE)).unwrap();
            let e = load(&ws_s, &json!({"repoDir": wt.to_string_lossy()}), &[]).unwrap_err();
            assert!(e.contains("outside the workspace"), "{e}");
        }
        // computeToml in the body (a CLI talking to a remote daemon).
        let got = load(&ws_s, &json!({"computeToml": "[defaults]\nos = \"macos\"\n"}), &[]).unwrap().expect("text");
        assert_eq!((got.source.as_str(), got.file.defaults.os.as_deref()), ("request", Some("macos")));
        let _ = std::fs::remove_dir_all(&base);
    }
}
