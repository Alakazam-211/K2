//! The optional type stage (`k2 zen validate --widget <name> --types`;
//! decision-k2-runtime-v1 §5, D5): a widget's scripts checked against the
//! generated `k2.d.ts` by **tsc-rs** (pingdotgg/ts-rust, TS 7 rules), run as
//! a **subprocess** with a timeout, never linked into the daemon.
//!
//! - The checker is `K2_TSC_RS` (a path) or `tsc-rs` on `PATH`;
//!   `K2_TS_CHECKER=tsc7` swaps in Microsoft's TS 7 native (`tsgo`, or
//!   `K2_TSGO`), the parity oracle. K2 never downloads one here: with none
//!   installed the stage says so and how to get it, and the parse and lint
//!   stages still ran.
//! - The check runs in a throwaway folder holding copies of the widget's
//!   scripts, `k2.d.ts` and a generated `tsconfig.json` (allowJs + checkJs,
//!   not strict). It never runs the widget's code.
//! - "Strict contract, forgiving DOM": findings on lines that use `k2.` are
//!   kept (the contract); the rest (mostly DOM typing in plain JS, like
//!   `getElementById(...).value`) are counted, not listed, unless `all`.
//!   Every finding is a **warning**: types never block a Garden version.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use crate::zen::schema::Diagnostic;

/// The generated widget API the scripts are checked against.
pub const K2_DTS: &str = include_str!("../../../../sdk/generated/k2.d.ts");
/// A check that runs longer is killed.
pub const TIMEOUT: Duration = Duration::from_secs(60);

/// What the type stage found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeReport {
    /// The checker that ran (`tsc-rs`, `tsgo`), or `None` when none could.
    pub checker: Option<String>,
    pub warnings: Vec<Diagnostic>,
    /// Findings not on a `k2.` line, left out (`all` lists them).
    pub hidden: usize,
    /// Why the stage didn't run, or ran incompletely.
    pub note: Option<String>,
}

/// The checker binary to run: `(name, path)`.
pub fn find_checker() -> Option<(String, PathBuf)> {
    let oracle = std::env::var("K2_TS_CHECKER").is_ok_and(|v| v == "tsc7");
    let (var, bin, name) = if oracle { ("K2_TSGO", "tsgo", "tsgo") } else { ("K2_TSC_RS", "tsc-rs", "tsc-rs") };
    if let Ok(p) = std::env::var(var) {
        let p = PathBuf::from(p);
        return p.is_file().then(|| (name.to_string(), p));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| p.is_file()).map(|p| (name.to_string(), p))
}

/// Check `files` (a widget folder's `name → bytes`; `.js` files are
/// checked) with the checker at `checker`. `label` prefixes file names in
/// findings (`widgets/<name>`).
pub fn check_widget(label: &str, files: &BTreeMap<String, Vec<u8>>, checker: Option<(String, PathBuf)>, all: bool) -> TypeReport {
    let mut r = TypeReport::default();
    let Some((name, bin)) = checker else {
        r.note = Some(
            "no type checker installed: set K2_TSC_RS to a tsc-rs binary (npm tsc-rs@0.8.0, or its GitHub release), or put tsc-rs on PATH; parse and lint checks still ran"
                .into(),
        );
        return r;
    };
    r.checker = Some(name);
    let scripts: Vec<(&String, &Vec<u8>)> =
        files.iter().filter(|(n, _)| n.to_ascii_lowercase().ends_with(".js") && safe_name(n)).collect();
    if scripts.is_empty() {
        r.note = Some("no .js files to check".into());
        return r;
    }
    let dir = std::env::temp_dir().join(format!("k2-types-{}", uuid::Uuid::new_v4().simple()));
    let _cleanup = Cleanup(dir.clone());
    if let Err(e) = write_project(&dir, &scripts) {
        r.note = Some(format!("couldn't prepare the check: {e}"));
        return r;
    }
    let out = match run(&bin, &dir) {
        Ok(o) => o,
        Err(e) => {
            r.note = Some(e);
            return r;
        }
    };
    let texts: BTreeMap<&str, &str> =
        scripts.iter().filter_map(|(n, b)| std::str::from_utf8(b).ok().map(|t| (n.as_str(), t))).collect();
    for d in parse_output(&out) {
        let Some(text) = texts.get(d.file.as_str()) else { continue };
        let line_text = text.lines().nth(d.line.saturating_sub(1)).unwrap_or("");
        if !all && !line_text.contains("k2.") {
            r.hidden += 1;
            continue;
        }
        r.warnings.push(Diagnostic {
            file: format!("{label}/{}", d.file),
            line: d.line,
            col: d.col,
            message: format!("type: {}", d.message),
            suggestion: Some("check the verb's arguments and answer in k2 zen guide api (k2.d.ts); types never block a widget".into()),
        });
    }
    if r.hidden > 0 {
        r.note = Some(format!(
            "{} finding(s) not on a k2 line were left out (DOM typing in plain JS); --types=all lists them",
            r.hidden
        ));
    }
    r
}

/// One flat file name (the widget folder is flat; nothing else is copied).
fn safe_name(n: &str) -> bool {
    !n.is_empty() && !n.contains(['/', '\\']) && !n.starts_with('.') && n != "k2.d.ts" && n != "tsconfig.json"
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_project(dir: &Path, scripts: &[(&String, &Vec<u8>)]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("k2.d.ts"), K2_DTS)?;
    let mut names = vec!["k2.d.ts".to_string()];
    for (n, b) in scripts {
        std::fs::write(dir.join(n), b)?;
        names.push((*n).clone());
    }
    let cfg = serde_json::json!({
        "compilerOptions": {
            "allowJs": true, "checkJs": true, "noEmit": true, "strict": false,
            "target": "es2022", "lib": ["es2022", "dom"], "types": [], "skipLibCheck": true,
        },
        "files": names,
    });
    std::fs::write(dir.join("tsconfig.json"), serde_json::to_string_pretty(&cfg).unwrap_or_default())
}

/// Run the checker in `dir`; stdout + stderr, or why it failed to run.
fn run(bin: &Path, dir: &Path) -> Result<String, String> {
    let mut child = crate::bg_proc::command(bin)
        .args(["-p", "tsconfig.json", "--pretty", "false"])
        .current_dir(dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't start {}: {e}", bin.display()))?;
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = so.as_mut() {
            let _ = o.take(4 * 1024 * 1024).read_to_string(&mut s);
        }
        if let Some(e) = se.as_mut() {
            let _ = e.take(1024 * 1024).read_to_string(&mut s);
        }
        s
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("the type check took over {}s and was stopped", TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(format!("the type check failed: {e}")),
        }
    }
    reader.join().map_err(|_| "the type check output couldn't be read".to_string())
}

/// One `file(line,col): error TSnnnn: message` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsLine {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub message: String,
}

/// Parse tsc's plain (`--pretty false`) output.
pub fn parse_output(out: &str) -> Vec<TsLine> {
    let mut v = Vec::new();
    for l in out.lines() {
        let Some((loc, rest)) = l.split_once("): ") else { continue };
        let Some((file, pos)) = loc.rsplit_once('(') else { continue };
        let Some((line, col)) = pos.split_once(',') else { continue };
        let (Ok(line), Ok(col)) = (line.trim().parse::<usize>(), col.trim().parse::<usize>()) else { continue };
        let file = file.trim().trim_start_matches("./").to_string();
        v.push(TsLine { file, line, col, message: rest.trim().to_string() });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_tsc_output_parses() {
        let out = "w.js(2,4): error TS2339: Property 'raed' does not exist on type 'ThreadVerbs'.\nnoise\nk2.d.ts(1,1): error TS1: x\n";
        let v = parse_output(out);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], TsLine { file: "w.js".into(), line: 2, col: 4, message: "error TS2339: Property 'raed' does not exist on type 'ThreadVerbs'.".into() });
    }

    #[test]
    fn no_checker_says_how_to_get_one_and_nothing_runs() {
        let files: BTreeMap<String, Vec<u8>> = [("w.js".to_string(), b"k2.x.y()".to_vec())].into();
        let r = check_widget("widgets/w", &files, None, false);
        assert_eq!(r.checker, None);
        assert!(r.note.as_deref().is_some_and(|n| n.contains("K2_TSC_RS")), "{r:?}");
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn the_api_declares_the_global_k2() {
        assert!(K2_DTS.contains("declare global") && K2_DTS.contains("const k2: K2"));
    }

    /// A fake checker (a shell script) stands in for tsc-rs: the project it
    /// is handed, the k2-line filter and the warning shape are what's tested.
    #[cfg(unix)]
    #[test]
    fn a_checker_runs_in_a_throwaway_project_and_k2_lines_are_kept() {
        use std::os::unix::fs::PermissionsExt;
        let bin_dir = std::env::temp_dir().join(format!("k2-fake-tsc-{}", uuid::Uuid::new_v4().simple()));
        let _c = Cleanup(bin_dir.clone());
        std::fs::create_dir_all(&bin_dir).unwrap_or_else(|e| panic!("mkdir: {e}"));
        let bin = bin_dir.join("tsc-rs");
        // Prove it got the project: it echoes a finding only when tsconfig.json
        // lists w.js and k2.d.ts is present.
        std::fs::write(
            &bin,
            "#!/bin/sh\n[ \"$1 $2 $3 $4\" = \"-p tsconfig.json --pretty false\" ] || exit 9\ngrep -q '\"w.js\"' tsconfig.json && [ -f k2.d.ts ] || exit 8\necho \"w.js(1,11): error TS2339: Property 'raed' does not exist on type 'ThreadVerbs'.\"\necho \"w.js(2,4): error TS2339: Property 'value' does not exist on type 'HTMLElement'.\"\nexit 2\n",
        )
        .unwrap_or_else(|e| panic!("write: {e}"));
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap_or_else(|e| panic!("chmod: {e}"));
        let files: BTreeMap<String, Vec<u8>> =
            [("w.js".to_string(), b"k2.thread.raed('k2/1')\nel.value = 3\n".to_vec()), ("../evil.js".to_string(), b"x".to_vec())].into();
        let r = check_widget("widgets/w", &files, Some(("tsc-rs".into(), bin.clone())), false);
        assert_eq!(r.checker.as_deref(), Some("tsc-rs"));
        assert_eq!(r.warnings.len(), 1, "{r:?}");
        assert_eq!((r.warnings[0].file.as_str(), r.warnings[0].line, r.warnings[0].col), ("widgets/w/w.js", 1, 11));
        assert!(r.warnings[0].message.starts_with("type: error TS2339"), "{r:?}");
        assert_eq!(r.hidden, 1);
        let all = check_widget("widgets/w", &files, Some(("tsc-rs".into(), bin)), true);
        assert_eq!(all.warnings.len(), 2, "{all:?}");
    }
}
