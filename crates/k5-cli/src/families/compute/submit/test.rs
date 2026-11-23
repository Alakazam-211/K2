//! `compute test [<node>] [--shards N] [--partition hash|count] [--profile
//! P] [--runner "cargo nextest run"] [run flags] [-- nextest args]`, as
//! `cmd_compute_test` answers it (`cli/src/compute-run.sh`): the words, the
//! nextest script its Python writes ([`script`]), then `compute run` with
//! the node (or `--any`), `--shards`, `--needs cargo-nextest` (unless a
//! need names it or the runner is another), the JUnit artifacts, the run
//! flags as given, and `-- sh -c <script>` ([`super::run`]).

use super::super::writes::Words;
use super::run;
use k2_client::{cli_name, product_name};

pub const UNKNOWN: &str = concat!(
    "usage: unknown flag '%s' (",
    cli_name!(),
    " compute test --help)"
);

/// The flags `test` passes to `run` with their value.
const VALUED: &[&str] = &[
    "--os",
    "--distro",
    "--arch",
    "--label",
    "--env",
    "--secret",
    "--cpus",
    "--mem",
    "--timeout",
    "--sha",
    "--workspace",
    "--artifact",
    "--artifacts",
    "--queue-timeout",
    "--retries",
    "--stall-timeout",
    "--tier",
    "--why",
    "--supersede",
    "--submitter",
    "--repo",
    "--cache",
    "--with",
    "--preset",
    "--tag",
    "--same-node-as",
    "--avoid",
    "--only",
    "--network",
    "--netns-dummy",
];

/// The flags `test` passes to `run` alone.
const BARE: &[&str] = &[
    "--detach",
    "-d",
    "--dirty",
    "--exclusive",
    "--receipt-v2",
    "--raw",
    "--json",
    "--no-retry",
    "--retry-interrupted",
    "--no-source",
    "--gate",
    "--critical",
    "--now",
    "--cache-required",
    "--plan",
    "--userns",
    "--no-addr-filter",
];

/// The words after `test`. `None`: the shell's.
pub fn parse(words: &[&str]) -> Option<Words<run::Ask>> {
    let (mut node, mut any, mut nshards, mut partition) =
        (String::new(), false, "1".to_string(), "hash".to_string());
    let (mut profile, mut runner, mut has_nextest_need) =
        (String::new(), "cargo nextest run".to_string(), false);
    let mut pass: Vec<String> = vec![];
    let mut targs: Vec<String> = vec![];
    let mut i = 0;
    let n = words.len();
    let val = |i: usize| words.get(i + 1).copied().unwrap_or("").to_string();
    while i < n {
        let w = words[i];
        match w {
            "--" => {
                targs = words[i + 1..].iter().map(|x| x.to_string()).collect();
                break;
            }
            "--any" => {
                any = true;
                i += 1;
                continue;
            }
            "--shards" | "--partition" | "--profile" | "--runner" => {
                let v = val(i);
                match w {
                    "--shards" => nshards = v,
                    "--partition" => partition = v,
                    "--profile" => profile = v,
                    _ => runner = v,
                }
            }
            "--needs" | "--need" => {
                let v = val(i);
                if v.contains("cargo-nextest") {
                    has_nextest_need = true;
                }
                pass.push(w.to_string());
                pass.push(v);
            }
            f if VALUED.contains(&f) || f == "--gpu-mem" => {
                pass.push(w.to_string());
                pass.push(val(i));
            }
            f if BARE.contains(&f) || f.starts_with("--gpu=") => {
                pass.push(w.to_string());
                i += 1;
                continue;
            }
            "--gpu" => {
                pass.push(w.to_string());
                match words.get(i + 1) {
                    Some(v) if !v.is_empty() && !v.starts_with('-') => {
                        pass.push(v.to_string());
                        i += 2;
                    }
                    _ => i += 1,
                }
                continue;
            }
            f if f.starts_with('-') => {
                return Some(Words::Usage(UNKNOWN.replacen("%s", f, 1)));
            }
            positional => {
                if node.is_empty() {
                    node = positional.to_string();
                    i += 1;
                    continue;
                }
                return Some(Words::Usage(
                    "usage: put nextest's own arguments after --".to_string(),
                ));
            }
        }
        // `shift 2`.
        if i + 2 > n {
            return Some(Words::ShiftFailed(words[i].to_string()));
        }
        i += 2;
    }
    if nshards.is_empty() || !nshards.bytes().all(|b| b.is_ascii_digit()) {
        return Some(Words::Usage("usage: --shards N (1–16)".to_string()));
    }
    if partition != "hash" && partition != "count" {
        return Some(Words::Usage("usage: --partition hash|count".to_string()));
    }
    // `[ "$nshards" -gt 1 ]`: past 18 digits the test's own error.
    let digits = nshards.trim_start_matches('0');
    if digits.len() > 18 {
        return None;
    }
    let many = digits.parse::<u64>().unwrap_or(0) > 1;
    if many && !node.is_empty() {
        return Some(Words::Usage(
            concat!(
                "usage: --shards picks a node for each shard itself (no node: ",
                product_name!(),
                " picks); drop the node name"
            )
            .to_string(),
        ));
    }
    if node.is_empty() {
        any = true;
    }
    // shlex's error is the shell's (its traceback, then exit 2).
    let script = script(&runner, &profile, many, &partition, &targs)?;
    let mut args: Vec<String> = vec![];
    if any {
        args.push("--any".into());
    } else {
        args.push(node);
    }
    if many {
        args.push("--shards".into());
        args.push(nshards);
    }
    if !has_nextest_need && runner == "cargo nextest run" {
        args.push("--needs".into());
        args.push("cargo-nextest".into());
    }
    args.push("--artifact".into());
    args.push("target/nextest/**/*.xml".into());
    args.extend(pass);
    args.extend(["--".into(), "sh".into(), "-c".into(), script]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run::parse(&refs)
}

/// The script `cmd_compute_test`'s Python writes (`$(…)` drops its last
/// newline). `None`: `shlex.split` refuses the runner.
pub fn script(
    runner: &str,
    profile: &str,
    many: bool,
    partition: &str,
    targs: &[String],
) -> Option<String> {
    let runner = shlex_split(runner)?;
    let fixed = fixed();
    let mut lines: Vec<String> = fixed.head.clone();
    let mut cmd: Vec<String> = runner.iter().map(|w| shlex_quote(w)).collect();
    if !profile.is_empty() {
        cmd.push("--profile".into());
        cmd.push(shlex_quote(profile));
    } else {
        lines.extend(fixed.cfg.iter().cloned());
        cmd.extend(fixed.config.iter().cloned());
    }
    if many {
        cmd.push("--partition".into());
        cmd.push(format!("{partition}:{{shard}}/{{shards}}"));
        cmd.push("--no-tests=pass".into());
    }
    cmd.extend(targs.iter().map(|a| shlex_quote(a)));
    lines.push(format!("exec {}", cmd.join(" ")));
    Some(lines.join("\n"))
}

/// The script's fixed text, as the shell's Python spells it.
pub struct Fixed {
    /// The first lines: `set -u`, no `K2*` variable, a runtime folder.
    pub head: Vec<String>,
    /// The nextest config file's lines (no `--profile`).
    pub cfg: Vec<String>,
    /// `--config-file "$cfg" --profile <name>`.
    pub config: Vec<String>,
}

/// [`Fixed`], read from the shell's Python (`cli/src/compute-run.sh`), so
/// the script is the shell's own text, byte for byte, in every brand.
pub fn fixed() -> &'static Fixed {
    static FIXED: std::sync::OnceLock<Fixed> = std::sync::OnceLock::new();
    FIXED.get_or_init(|| {
        let at = super::SRC
            .find("\ncmd_compute_test() {")
            .expect("cmd_compute_test is in the shell's source");
        let from = at
            + super::SRC[at..]
                .find("lines = [")
                .expect("its Python has its lines");
        let to = from
            + super::SRC[from..]
                .find("lines.append(\"exec \"")
                .expect("its Python ends with the exec line");
        let lits = py_literals(&super::SRC[from..to]);
        let config_at = lits
            .iter()
            .position(|l| l == "--config-file")
            .expect("the config file's flag");
        let cfg: Vec<String> = lits
            .iter()
            .skip(4)
            .filter(|l| l.starts_with("cfg=") || l.starts_with("printf "))
            .cloned()
            .collect();
        Fixed {
            head: lits[..3].to_vec(),
            cfg,
            config: lits[config_at..config_at + 4].to_vec(),
        }
    })
}

/// The double-quoted Python string literals in `code`, in order, each
/// read as Python reads it.
fn py_literals(code: &str) -> Vec<String> {
    let mut out = vec![];
    let mut chars = code.chars();
    let mut comment = false;
    while let Some(c) = chars.next() {
        match c {
            '\n' => comment = false,
            '#' => comment = true,
            '"' if !comment => {
                let mut lit = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => lit.push('\n'),
                            Some('t') => lit.push('\t'),
                            Some(e @ ('\\' | '"' | '\'')) => lit.push(e),
                            Some(e) => {
                                lit.push('\\');
                                lit.push(e);
                            }
                            None => {}
                        },
                        other => lit.push(other),
                    }
                }
                out.push(lit);
            }
            _ => {}
        }
    }
    out
}

/// `shlex.quote(s)`.
pub fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    let safe = |c: char| {
        c.is_ascii_alphanumeric()
            || matches!(c, '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-')
    };
    if s.chars().all(safe) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// `shlex.split(s)` (POSIX, no comments). `None`: a quote or escape with
/// no end (Python's `ValueError`).
pub fn shlex_split(s: &str) -> Option<Vec<String>> {
    let ws = |c: char| matches!(c, ' ' | '\t' | '\r' | '\n');
    let mut out = vec![];
    let mut token = String::new();
    let mut quoted = false;
    // ' ' (between words), 'a' (in a word), '\'' or '"' (in quotes).
    let mut state = ' ';
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match state {
            ' ' | 'a' => {
                if ws(c) {
                    if state == 'a' && (!token.is_empty() || quoted) {
                        out.push(std::mem::take(&mut token));
                        quoted = false;
                    }
                    state = ' ';
                } else if c == '\'' || c == '"' {
                    quoted = true;
                    state = c;
                } else if c == '\\' {
                    let next = chars.next()?;
                    token.push(next);
                    state = 'a';
                } else {
                    token.push(c);
                    state = 'a';
                }
            }
            q => {
                if c == q {
                    state = 'a';
                } else if q == '"' && c == '\\' {
                    let next = chars.next()?;
                    if next != '"' && next != '\\' {
                        token.push('\\');
                    }
                    token.push(next);
                } else {
                    token.push(c);
                }
            }
        }
    }
    if state == '\'' || state == '"' {
        return None;
    }
    if !token.is_empty() || quoted {
        out.push(token);
    }
    Some(out)
}
