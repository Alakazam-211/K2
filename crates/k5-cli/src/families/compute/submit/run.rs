//! `compute run [<node>] [flags] -- <command…>`, as `cmd_compute_run`
//! answers it (`cli/src/compute-run.sh`):
//!
//! 1. The words ([`parse`]), each flag's usage line where the shell has
//!    one, then the checks between them, and the ones the shell's Python
//!    makes (`--label`, `--env`, `--secret`, `--with`), `--gpu-mem` with no
//!    `--gpu`, and more than 4 `--netns-dummy`.
//! 2. This checkout's folder and root (`git rev-parse`), a new
//!    `clientJobId`, and the body as `_compute_json` writes it ([`body`]).
//! 3. One POST of `/cli/compute/run`, sent again after no answer (status
//!    000) up to 3 times, 2 s apart, with the same `clientJobId`; outside a
//!    session, from a shell under a K2 terminal, each send first says the
//!    job goes as the owner. A status but 200: `_compute_fail`.
//! 4. `--plan`: the plan's lines on stderr and the answer on stdout
//!    ([`super::plan`]). Else the job's line ([`info`]), then: `--detach`
//!    prints its id (or with `--json` its result); `--wait` prints its id,
//!    then follows it ([`follow`]) or, with `--quiet`, `--json` or a split
//!    run, waits for it (`cmd_compute_wait`); attached, a split run is
//!    followed shard by shard ([`super::set`]) and one job as `--wait`
//!    follows it.

use super::super::follow::{self, Follow, Start};
use super::super::lines::py_line;
use super::super::pyrt::{self as p, s};
use super::super::render::{self, Cx};
use super::super::writes::{self, dumps, object, str_value, wait, Cp, Words};
use super::{bytes, distro, gpu_arg, head8, lower_digit, runs, secs, SRC};
use crate::exit::Exit;
use crate::families::{py_space, raised, Raise};
use crate::out;
use crate::out::json::{Text, Value};
use k2_client::{brand::CLI_NAME, cli_name, product_name};
use std::time::Duration;

/// The route.
pub const ENDPOINT: &str = "/cli/compute/run";
/// Sends with no answer before giving up.
pub const SENDS: u32 = 3;
/// Between them.
pub const RESEND_PAUSE: Duration = Duration::from_secs(2);

pub const USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " compute run [<node>] [flags] -- <command…>  (no node: ",
    product_name!(),
    " picks one)"
);
pub const AFTER_DASHES: &str = concat!(
    "usage: put the command after --: ",
    cli_name!(),
    " compute run [<node>] -- <command…>"
);
pub const UNKNOWN: &str = concat!("usage: unknown flag '%s' (", cli_name!(), " compute --help)");

/// What the words ask for, as the shell's variables hold it after its
/// checks.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    pub node: String,
    pub any: bool,
    pub os: String,
    pub arch: String,
    pub detach: bool,
    pub dirty: bool,
    pub sha: String,
    pub nosrc: bool,
    pub cpus: Num,
    pub mem: String,
    pub timeout: String,
    pub exclusive: bool,
    /// `""`, `"0"` or `"1"`.
    pub retry: String,
    pub raw: bool,
    pub workspace: String,
    pub wait: bool,
    pub wait_quiet: bool,
    /// `--max-wait`, in seconds.
    pub max_wait: Option<u64>,
    pub as_json: bool,
    pub qtimeout: String,
    pub retries: String,
    pub stall: String,
    pub nshards: String,
    pub plan: bool,
    pub gate: bool,
    pub tier: String,
    pub supersede: String,
    pub submitter: String,
    pub repo: String,
    pub cache: String,
    pub why: String,
    pub preset: String,
    pub now: bool,
    pub cache_required: bool,
    pub tag: String,
    pub same_node: String,
    pub distro: String,
    pub receipt_v: String,
    pub gpu: String,
    pub gpu_mem: String,
    pub mem_auto: bool,
    pub mem_auto_fb: String,
    pub cpus_burst: Num,
    pub network: String,
    pub labels: Value,
    pub env: Value,
    pub needs: Option<Value>,
    pub artifacts: Option<Value>,
    pub secrets: Option<Value>,
    pub withs: Option<Value>,
    pub avoids: Option<Value>,
    pub onlys: Option<Value>,
    pub netns: Option<Value>,
    pub argv: Vec<String>,
}

/// A value `_compute_json` reads as a number (`_n`): none, or its JSON.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Num(pub Option<String>);

impl Num {
    /// `float(v) if "." in v else int(v)` of digits, or digits, a dot and
    /// digits. `None`: anything else (the shell's: its Python's error).
    pub fn of(v: &str) -> Option<Num> {
        if v.is_empty() {
            return Some(Num(None));
        }
        let digits = |d: &str| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit());
        match v.split_once('.') {
            None if digits(v) => {
                let t = v.trim_start_matches('0');
                Some(Num(Some(if t.is_empty() { "0" } else { t }.to_string())))
            }
            Some((a, b)) if digits(a) && digits(b) => {
                let f: f64 = v.parse().ok()?;
                let text = crate::out::json::to_string(
                    &Value::Float(f),
                    &crate::out::json::Options::default(),
                )
                .ok()?;
                Some(Num(Some(text)))
            }
            _ => None,
        }
    }

    fn value(&self) -> Option<Value> {
        self.0
            .as_ref()
            .map(|t| crate::out::json::parse(t).unwrap_or_else(|_| Value::Int(t.clone())))
    }
}

fn usage<T>(line: impl Into<String>) -> Option<Words<T>> {
    Some(Words::Usage(line.into()))
}

/// The words after `run`. `None`: the shell answers (see
/// [`super`]'s notes).
pub fn parse(words: &[&str]) -> Option<Words<Ask>> {
    let mut a = Ask::default();
    let (mut labels, mut envs, mut needs, mut artifacts) = (vec![], vec![], vec![], vec![]);
    let (mut secrets, mut withs, mut avoids, mut onlys, mut netns) =
        (vec![], vec![], vec![], vec![], vec![]);
    let (mut failures, mut max_wait) = (false, String::new());
    let mut cpus = String::new();
    let mut i = 0;
    let n = words.len();
    // The value after the flag, and whether `shift 2` works.
    let val = |i: usize| words.get(i + 1).copied().unwrap_or("");
    macro_rules! shift2 {
        () => {{
            if i + 2 > n {
                return Some(Words::ShiftFailed);
            }
            i += 2;
        }};
    }
    while i < n {
        let w = words[i];
        match w {
            "--" => {
                i += 1;
                break;
            }
            "--avoid" | "--only" => {
                let v = val(i).to_string();
                shift2!();
                if v.is_empty() {
                    return usage(if w == "--avoid" {
                        concat!("usage: --avoid <node> (a node ", product_name!(), " must not pick; repeatable)")
                    } else {
                        concat!("usage: --only <node> (", product_name!(), " picks only among the nodes you list; repeatable)")
                    });
                }
                if w == "--avoid" {
                    avoids.push(v);
                } else {
                    onlys.push(v);
                }
            }
            "--preset" => {
                a.preset = val(i).to_string();
                shift2!();
                if !a.preset.is_ascii() {
                    return None;
                }
                let tail = |c: u8| lower_digit(c) || matches!(c, b'.' | b'_' | b'-');
                if !runs(&a.preset, &[(&lower_digit, 1, 1), (&tail, 0, 39)]) {
                    return usage("usage: --preset NAME (a [presets.NAME] in .k2/compute.toml: 1–40 of a-z 0-9 . _ -)");
                }
            }
            "--now" => {
                a.now = true;
                i += 1;
            }
            "--tag" => {
                a.tag = val(i).to_string();
                shift2!();
                if !a.tag.is_ascii() {
                    return None;
                }
                let set = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'/' | b'-');
                if !runs(&a.tag, &[(&set, 1, 64)]) {
                    return usage("usage: --tag NAME (1–64 of A-Z a-z 0-9 . _ : / -, e.g. fuzz:p3)");
                }
            }
            "--same-node-as" => {
                a.same_node = val(i).to_string();
                shift2!();
                if a.same_node.is_empty() {
                    return usage("usage: --same-node-as <job> (one of this workspace's jobs)");
                }
            }
            "--repo" => {
                a.repo = val(i).to_string();
                shift2!();
                if a.repo.is_empty() {
                    return usage("usage: --repo <folder> (a git repo inside the workspace, e.g. k2-multiplayer-base)");
                }
            }
            "--with" => {
                let v = val(i).to_string();
                shift2!();
                if v.is_empty() {
                    return usage("usage: --with DIR[@SHA] (a git repo inside the workspace, or a worktree of one)");
                }
                withs.push(v);
            }
            "--cache" => {
                a.cache = val(i).to_string();
                shift2!();
                if !a.cache.is_ascii() {
                    return None;
                }
                let tail = |c: u8| lower_digit(c) || matches!(c, b'.' | b'_' | b'-');
                if !runs(&a.cache, &[(&lower_digit, 1, 1), (&tail, 0, 63)]) {
                    return usage("usage: --cache NAME (1–64 of a-z 0-9 . _ -, starting with a letter or digit)");
                }
            }
            "--cache-required" => {
                a.cache_required = true;
                i += 1;
            }
            "--gate" => {
                a.gate = true;
                a.tier = "ci".into();
                i += 1;
            }
            "--priority" | "--critical" => {
                a.tier = "priority".into();
                i += 1;
            }
            "--why" => {
                a.why = val(i).to_string();
                shift2!();
            }
            "--tier" => {
                a.tier = val(i).to_string();
                shift2!();
                match a.tier.as_str() {
                    "ci" | "priority" | "batch" => {}
                    "normal" | "standard" => a.tier = "normal".into(),
                    _ => {
                        return usage(concat!(
                            "usage: --tier standard|priority|batch|ci (",
                            cli_name!(),
                            " study compute: TIERS)"
                        ))
                    }
                }
                if a.tier == "ci" {
                    a.gate = true;
                }
            }
            "--supersede" => {
                a.supersede = val(i).to_string();
                shift2!();
                if !supersede_ok(&a.supersede) {
                    return usage("usage: --supersede KEY (1–120 characters, no control characters, no leading/trailing space)");
                }
            }
            "--submitter" => {
                a.submitter = val(i).to_string();
                shift2!();
                if !a.submitter.is_ascii() {
                    return None;
                }
                let set = |c: u8| lower_digit(c) || matches!(c, b'.' | b'_' | b'-');
                if !runs(&a.submitter, &[(&set, 1, 40)]) {
                    return usage("usage: --submitter NAME (1–40 of a-z 0-9 . _ -)");
                }
            }
            "--secret" => {
                secrets.push(val(i).to_string());
                shift2!();
            }
            "--wait" => {
                a.wait = true;
                i += 1;
            }
            "--quiet" | "-q" => {
                a.wait_quiet = true;
                i += 1;
            }
            "--failures" => {
                failures = true;
                i += 1;
            }
            "--max-wait" => {
                max_wait = val(i).to_string();
                shift2!();
            }
            "--any" => {
                a.any = true;
                i += 1;
            }
            "--os" => {
                a.os = val(i).to_string();
                shift2!();
            }
            "--distro" => {
                a.distro = match distro(val(i))? {
                    Ok(d) => d,
                    Err(stop) => return Some(stop),
                };
                shift2!();
            }
            "--arch" => {
                a.arch = val(i).to_string();
                shift2!();
            }
            "--label" => {
                labels.push(val(i).to_string());
                shift2!();
            }
            "--detach" | "-d" => {
                a.detach = true;
                i += 1;
            }
            "--dirty" => {
                a.dirty = true;
                i += 1;
            }
            "--sha" => {
                a.sha = val(i).to_string();
                shift2!();
            }
            "--no-source" => {
                a.nosrc = true;
                i += 1;
            }
            "--env" => {
                envs.push(val(i).to_string());
                shift2!();
            }
            "--needs" | "--need" => {
                needs.push(val(i).to_string());
                shift2!();
            }
            "--cpus" => {
                cpus = val(i).to_string();
                a.cpus_burst = Num(None);
                shift2!();
                if cpus.contains('+') {
                    let (c, b) = cpus.split_once('+').unwrap_or((&cpus, ""));
                    let num = |d: &str| {
                        let (x, y) = d.split_once('.').unwrap_or((d, "1"));
                        !x.is_empty()
                            && !y.is_empty()
                            && x.bytes().all(|b| b.is_ascii_digit())
                            && y.bytes().all(|b| b.is_ascii_digit())
                    };
                    if b.contains('+') || !num(c) || !num(b) {
                        return usage("usage: --cpus N or --cpus N+M (4+8: 4 guaranteed, up to 8 on idle cores)");
                    }
                    // `${cpus#*+}` and `${cpus%%+*}`.
                    a.cpus_burst = Num::of(b)?;
                    cpus = c.to_string();
                }
            }
            "--mem" => {
                match val(i) {
                    "auto" => {
                        a.mem_auto = true;
                        a.mem.clear();
                        a.mem_auto_fb.clear();
                    }
                    v if v.starts_with("auto:") => {
                        a.mem_auto = true;
                        a.mem.clear();
                        a.mem_auto_fb = match bytes(&v["auto:".len()..])? {
                            Ok(b) => b,
                            Err(stop) => return Some(stop),
                        };
                    }
                    v => {
                        a.mem = match bytes(v)? {
                            Ok(b) => b,
                            Err(stop) => return Some(stop),
                        };
                        a.mem_auto = false;
                        a.mem_auto_fb.clear();
                    }
                }
                shift2!();
            }
            "--timeout" => {
                a.timeout = match secs(val(i))? {
                    Ok(s) => s,
                    Err(stop) => return Some(stop),
                };
                shift2!();
            }
            "--exclusive" => {
                a.exclusive = true;
                i += 1;
            }
            gpu if gpu.starts_with("--gpu=") => {
                a.gpu = gpu["--gpu=".len()..].to_string();
                i += 1;
                if let Some(line) = gpu_arg(&a.gpu) {
                    return usage(line);
                }
            }
            "--gpu" => match words.get(i + 1) {
                None => {
                    a.gpu = "any".into();
                    i += 1;
                }
                Some(v) if v.is_empty() || v.starts_with('-') => {
                    a.gpu = "any".into();
                    i += 1;
                }
                Some(v) => {
                    a.gpu = v.to_string();
                    i += 2;
                    if let Some(line) = gpu_arg(&a.gpu) {
                        return usage(line);
                    }
                }
            },
            "--gpu-mem" => {
                a.gpu_mem = match bytes(val(i))? {
                    Ok(b) => b,
                    Err(stop) => return Some(stop),
                };
                shift2!();
            }
            "--network" => {
                match val(i) {
                    "none" => a.network = "none".into(),
                    "on" | "no-lan" | "full" => return usage("usage: --network none is the one value: a job can only narrow its network (its node's owner sets how much jobs get there)"),
                    _ => return usage("usage: --network none (no network for this job: loopback only)"),
                }
                shift2!();
            }
            "--netns-dummy" => {
                let v = val(i).to_string();
                shift2!();
                if !dummy_shaped(&v) {
                    return usage("usage: --netns-dummy NAME=ADDRESS/PREFIX (e.g. dummy0=10.254.254.1/32; private, link-local or test ranges only)");
                }
                netns.push(v);
            }
            "--receipt-v2" => {
                a.receipt_v = "2".into();
                i += 1;
            }
            "--retry-interrupted" => {
                a.retry = "1".into();
                i += 1;
            }
            "--no-retry" => {
                a.retry = "0".into();
                a.retries = "0".into();
                i += 1;
            }
            "--retries" => {
                a.retries = val(i).to_string();
                shift2!();
                if a.retries.is_empty() || !a.retries.bytes().all(|b| b.is_ascii_digit()) {
                    return usage("usage: --retries N (0–5)");
                }
            }
            "--queue-timeout" | "--stall-timeout" => {
                let v = match val(i) {
                    "off" | "none" | "0" => "0".to_string(),
                    other => match secs(other)? {
                        Ok(s) => s,
                        Err(stop) => return Some(stop),
                    },
                };
                if w == "--queue-timeout" {
                    a.qtimeout = v;
                } else {
                    a.stall = v;
                }
                shift2!();
            }
            "--raw" => {
                a.raw = true;
                i += 1;
            }
            "--artifact" | "--artifacts" => {
                artifacts.push(val(i).to_string());
                shift2!();
            }
            "--shards" => {
                a.nshards = val(i).to_string();
                shift2!();
                if a.nshards.is_empty() || !a.nshards.bytes().all(|b| b.is_ascii_digit()) {
                    return usage("usage: --shards N (2–16)");
                }
            }
            "--json" => {
                a.as_json = true;
                i += 1;
            }
            "--plan" | "--dry-run" => {
                a.plan = true;
                i += 1;
            }
            "--workspace" => {
                a.workspace = val(i).to_string();
                shift2!();
            }
            flag if flag.starts_with('-') => return usage(UNKNOWN.replacen("%s", flag, 1)),
            positional => {
                if a.node.is_empty() && !a.any {
                    a.node = positional.to_string();
                    i += 1;
                } else {
                    return usage(AFTER_DASHES);
                }
            }
        }
    }
    a.argv = words[i.min(n)..].iter().map(|w| w.to_string()).collect();
    if a.argv.is_empty() {
        return usage(USAGE);
    }
    if a.node.is_empty() {
        a.any = true;
    }
    if a.wait && a.detach {
        return usage("usage: --wait and --detach don't mix (--wait blocks until the end; --detach returns at once)");
    }
    if a.tier == "priority" && a.why.is_empty() {
        return usage(concat!(
            "usage: --priority needs a one-line reason: --priority --why \"release gate for 0.46.6\" (it shows in the queue and ",
            cli_name!(),
            " compute status; Priority is for the critical path only)"
        ));
    }
    if a.exclusive && a.why.is_empty() {
        return usage("usage: --exclusive needs a one-line reason: --exclusive --why \"benchmark needs the whole node\" (it shows in the queue; it also needs a grant with --ci)");
    }
    if a.now && (a.tier == "batch" || !a.nshards.is_empty()) {
        return usage("usage: --now doesn't mix with --tier batch (it waits for an idle node) or --shards (each shard is placed itself)");
    }
    if a.tier == "batch" {
        if a.wait {
            return usage(concat!(
                "usage: --tier batch doesn't mix with --wait: a Batch job starts only when the node is idle (maybe hours). It is detached; ",
                cli_name!(),
                " compute wait <id> later"
            ));
        }
        a.detach = true;
    }
    if (failures || !max_wait.is_empty() || a.wait_quiet) && !a.wait {
        return usage(concat!(
            "usage: --failures, --max-wait and --quiet go with --wait (",
            cli_name!(),
            " compute run <node> --wait --failures -- …)"
        ));
    }
    if a.wait && a.retry.is_empty() {
        a.retry = "1".into();
    }
    // `--failures` (the failing tests after a failing end) is the shell's.
    if failures {
        return None;
    }
    if !max_wait.is_empty() {
        a.max_wait = Some(wait::secs(&max_wait)?);
    }
    // The cpus as a number (`int()` or `float()` in `_compute_json`).
    a.cpus = Num::of(&cpus)?;
    a.labels = match pairs(&labels, "usage: --label key=value") {
        Ok(v) => v,
        Err(line) => return usage(line),
    };
    a.env = match pairs(&envs, "usage: --env NAME=value") {
        Ok(v) => v,
        Err(line) => return usage(line),
    };
    if !needs.is_empty() {
        a.needs = Some(split_needs(&needs));
    }
    if !artifacts.is_empty() {
        a.artifacts = Some(strs(&artifacts));
    }
    if !secrets.is_empty() {
        let mut out = vec![];
        for x in &secrets {
            if x.is_empty() || x.starts_with('=') {
                return usage("usage: --secret NAME[=ENVNAME]");
            }
            let (name, env) = x.split_once('=').unwrap_or((x, ""));
            let mut o = vec![("name", str_value(name))];
            if !env.is_empty() {
                o.push(("env", str_value(env)));
            }
            out.push(object(o));
        }
        a.secrets = Some(Value::Array(out));
    }
    if !withs.is_empty() {
        let mut out = vec![];
        for x in &withs {
            let (repo, sha) = match x.rsplit_once('@') {
                Some((r, s)) => (r, s),
                None => (x.as_str(), ""),
            };
            if repo.is_empty() {
                return usage("usage: --with DIR[@SHA]");
            }
            let mut o = vec![("repo", str_value(repo))];
            if !sha.is_empty() {
                o.push(("sha", str_value(sha)));
            }
            out.push(object(o));
        }
        a.withs = Some(Value::Array(out));
    }
    if !avoids.is_empty() {
        a.avoids = Some(strs(&avoids));
    }
    if !onlys.is_empty() {
        a.onlys = Some(strs(&onlys));
    }
    if !a.gpu_mem.is_empty() && a.gpu.is_empty() {
        return usage(concat!(
            "usage: --gpu-mem needs --gpu: it is the GPU memory budget of a GPU job (",
            cli_name!(),
            " compute run --gpu --gpu-mem 4G --mem 8G -- <command>)"
        ));
    }
    if !netns.is_empty() {
        if netns.len() > 4 {
            return usage("usage: at most 4 --netns-dummy per job");
        }
        a.netns = Some(strs(&netns));
        a.network = "none".into();
    }
    // The shell's temp files: `mktemp -t` (the follow, a split run's
    // follow) and `mktemp -d` (`cmd_compute_wait`).
    if !a.detach && !a.plan && (!follow::usable() || (a.wait && !wait::temp_usable())) {
        return None;
    }
    Some(Words::Ask(a))
}

/// `--supersede`'s Python: 1 to 120 characters, none a control
/// character, no space around them.
fn supersede_ok(k: &str) -> bool {
    let n = k.chars().count();
    (1..=120).contains(&n)
        && k == k.trim_matches(py_space)
        && !k.chars().any(|c| (c as u32) < 32 || c as u32 == 127)
}

/// `[[ "$v" == ?*=?*/?* ]]`.
fn dummy_shaped(v: &str) -> bool {
    let c: Vec<char> = v.chars().collect();
    (1..c.len()).any(|i| {
        c[i] == '=' && (i + 2..c.len().saturating_sub(1)).any(|j| c[j] == '/')
    })
}

/// `json.dumps(sys.argv[1:])`.
fn strs(items: &[String]) -> Value {
    Value::Array(items.iter().map(|x| str_value(x)).collect())
}

/// `[x.strip() for x in a.split(",") if x.strip()]` over each word.
pub fn split_needs(items: &[String]) -> Value {
    Value::Array(
        items
            .iter()
            .flat_map(|a| a.split(','))
            .map(|x| x.trim_matches(py_space))
            .filter(|x| !x.is_empty())
            .map(str_value)
            .collect(),
    )
}

/// The `key=value` words as one object (a key again: its last value, at
/// its first place), or the usage line for a word with no `=`.
pub fn pairs(items: &[String], line: &'static str) -> Result<Value, &'static str> {
    let mut out: Vec<(Text, Value)> = vec![];
    for l in items {
        let Some((k, v)) = l.split_once('=') else {
            return Err(line);
        };
        let key = Text::from(k);
        match out.iter_mut().find(|(x, _)| *x == key) {
            Some(slot) => slot.1 = str_value(v),
            None => out.push((key, str_value(v))),
        }
    }
    Ok(Value::Object(out))
}

/// `_compute_json K2_CJ_…`: the body, its keys in the shell's order, an
/// empty value left out; `any` gone again when a node was named.
pub fn body(a: &Ask, cwd: &str, repo_dir: &str, ws: &str, client: &str) -> String {
    let b = |x: bool| Some(Value::Bool(x));
    let st = |x: &str| (!x.is_empty()).then(|| str_value(x));
    let num = |x: &str| Num::of(x).and_then(|n| n.value());
    let flag = |x: bool| x.then_some(Value::Bool(true));
    let retry = match a.retry.as_str() {
        "" => None,
        r => b(r == "1"),
    };
    let gpu = (!a.gpu.is_empty()).then(|| {
        object(vec![
            ("vendor", str_value(&a.gpu)),
            ("count", Value::Int("1".into())),
        ])
    });
    let fields: Vec<(&str, Option<Value>)> = vec![
        ("node", st(&a.node)),
        ("any", if a.any { b(true) } else { None }),
        ("os", st(&a.os)),
        ("distro", st(&a.distro)),
        ("arch", st(&a.arch)),
        ("avoid", a.avoids.clone()),
        ("only", a.onlys.clone()),
        ("labels", Some(a.labels.clone())),
        ("env", Some(a.env.clone())),
        ("argv", Some(strs(&a.argv))),
        ("needs", a.needs.clone()),
        ("artifacts", a.artifacts.clone()),
        ("secrets", a.secrets.clone()),
        ("queueTimeoutSecs", num(&a.qtimeout)),
        ("retries", num(&a.retries)),
        ("stallSecs", num(&a.stall)),
        ("shards", num(&a.nshards)),
        ("plan", b(a.plan)),
        ("detach", b(a.detach)),
        ("dirty", b(a.dirty)),
        ("sha", st(&a.sha)),
        ("noSource", b(a.nosrc)),
        ("cpus", a.cpus.value()),
        ("cpusBurst", a.cpus_burst.value()),
        ("memBytes", num(&a.mem)),
        ("memAuto", flag(a.mem_auto)),
        ("memAutoFallbackBytes", num(&a.mem_auto_fb)),
        ("timeoutSecs", num(&a.timeout)),
        ("exclusive", b(a.exclusive)),
        ("retryInterrupted", retry),
        ("cwd", st(cwd)),
        ("repoDir", st(repo_dir)),
        ("workspace", st(ws)),
        ("clientJobId", st(client)),
        ("gate", flag(a.gate)),
        ("tier", st(&a.tier)),
        ("why", st(&a.why)),
        ("supersede", st(&a.supersede)),
        ("submitter", st(&a.submitter)),
        ("repo", st(&a.repo)),
        ("cache", st(&a.cache)),
        ("with", a.withs.clone()),
        ("preset", st(&a.preset)),
        ("now", flag(a.now)),
        ("cacheRequired", flag(a.cache_required)),
        ("tag", st(&a.tag)),
        ("sameNodeAs", st(&a.same_node)),
        ("receiptV", num(&a.receipt_v)),
        ("gpu", gpu),
        ("gpuMemBytes", num(&a.gpu_mem)),
        ("network", st(&a.network)),
        ("netnsDummies", a.netns.clone()),
    ];
    dumps(&object(
        fields
            .into_iter()
            .filter_map(|(k, v)| v.map(|v| (k, v)))
            .collect(),
    ))
}

/// `uuid.uuid4().hex`.
pub fn client_job_id() -> String {
    crate::families::msg::send_once::uuid4().replace('-', "")
}

/// `_compute_in_k2_tree`: a `k2-ptyd` or `k2-daemon` among this
/// process's parents (up to 40 of them), as `ps -o ppid=,comm=` names
/// them.
pub fn in_k2_tree() -> bool {
    #[cfg(unix)]
    let mut pid: i64 = i64::from(unsafe { libc::getppid() });
    #[cfg(not(unix))]
    let mut pid: i64 = 0;
    for _ in 0..40 {
        if pid <= 1 {
            return false;
        }
        let Ok(o) = std::process::Command::new("ps")
            .args(["-o", "ppid=,comm=", "-p", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        else {
            return false;
        };
        if !o.status.success() {
            return false;
        }
        let line = String::from_utf8_lossy(&o.stdout);
        let line = line.trim_end_matches('\n');
        if line.is_empty() {
            return false;
        }
        let comm = line.trim_start_matches([' ', '\t', '\n']);
        let comm = comm
            .split_once(|c: char| c.is_whitespace())
            .map_or("", |(_, r)| r);
        if comm.contains("k2-ptyd") || comm.contains("k2-daemon") {
            return true;
        }
        let first = line.split_whitespace().next().unwrap_or("");
        match first.parse::<i64>() {
            Ok(p) => pid = p,
            Err(_) => return false,
        }
    }
    false
}

/// The submit, and what follows it.
pub fn send(cp: &mut Cp<'_>, a: &Ask) -> Exit {
    let (mut cwd, mut repo_dir) = (String::new(), String::new());
    if crate::daemon::usable_git().is_some() {
        cwd = super::git_rev_parse("--show-prefix");
        if let Some(c) = cwd.strip_suffix('/') {
            cwd = c.to_string();
        }
        repo_dir = super::git_rev_parse("--show-toplevel");
    }
    let client = client_job_id();
    let ws = cp.ws(&a.workspace);
    let body = body(a, &cwd, &repo_dir, &ws, &client);
    let in_session = writes::in_session(&cp.client.auth);
    let mut attempt = 0;
    let split = loop {
        if !in_session && in_k2_tree() {
            out::stderr_line(&format!(
                "[{CLI_NAME} compute] no passport in this shell: this job is submitted as the owner (its jobs show via: owner_token). Keep K2_* set in agent shells."
            ));
        }
        let split = match cp.send(true, ENDPOINT, &body, false) {
            Ok(s) => s,
            Err(e) => return e,
        };
        if split.status != writes::NO_STATUS {
            break split;
        }
        attempt += 1;
        if attempt >= SENDS {
            break split;
        }
        std::thread::sleep(RESEND_PAUSE);
    };
    if split.status != "200" {
        return writes::fail(&split.status, &split.body);
    }
    if a.plan {
        return super::plan::reader(&split.body).print();
    }
    let info = match info(&split.body) {
        Ok(i) => i,
        Err(e) => return e,
    };
    out::stderr_line(&info.line);
    if !info.placed.is_empty() {
        out::stderr_line(&info.placed);
    }
    let job8 = head8(&info.job);
    if a.detach {
        if a.as_json {
            if let Err(e) = follow::print_result(cp, &info.job) {
                return e;
            }
        } else if let Err(e) = out::stdout(&format!("{job8}\n")) {
            return out::after_stdout_error(&e);
        }
        out::stderr_line(&format!(
            "[{CLI_NAME} compute] detached. {CLI_NAME} compute wait {job8} blocks until it ends (agents in a {} tab also get a [compute] message)",
            product_name!()
        ));
        return Exit::Ok;
    }
    let set = info.kind == "set";
    if a.wait {
        if a.as_json {
            out::stderr_line(&job8);
        } else if let Err(e) = out::stdout(&format!("{job8}\n")) {
            return out::after_stdout_error(&e);
        }
        if !a.wait_quiet && !a.as_json && !set {
            let deadline = a.max_wait.map(|s| follow_now() + s as i64);
            return follow::follow(
                cp,
                &Follow {
                    job: info.job.clone(),
                    raw: a.raw,
                    start: Start::Cursor("0".into()),
                    as_json: false,
                    deadline,
                },
            );
        }
        return wait::send(
            cp,
            &wait::Ask {
                jobs: vec![info.job.clone()],
                quiet: false,
                json: a.as_json,
                any: false,
                max_wait: a.max_wait,
            },
        );
    }
    if set {
        return super::set::follow_set(cp, &info.job, a.as_json);
    }
    follow::follow(
        cp,
        &Follow {
            job: info.job.clone(),
            raw: a.raw,
            start: Start::Cursor("0".into()),
            as_json: a.as_json,
            deadline: None,
        },
    )
}

fn follow_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The job's line, as the shell splits its reader's one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub job: String,
    pub kind: String,
    pub line: String,
    pub placed: String,
}

macro_rules! line {
    ($m:literal) => {
        const { py_line(SRC, "    local info\n", $m) }
    };
}

/// The reader after the submit: its notes on stderr as it goes, then its
/// one line (`$(…)`, split at the tabs as the shell does). A raise is its
/// traceback and exit 1.
pub fn info(body: &[u8]) -> Result<Info, Exit> {
    let cx = Cx::new(0);
    let mut err = String::new();
    let r = info_reader(&cx, body, &mut err);
    let _ = out::write_flushed(&mut std::io::stderr().lock(), err.as_bytes());
    match r {
        Ok(text) => {
            let text = text.trim_end_matches('\n');
            let (job, rest) = text.split_once('\t').unwrap_or((text, text));
            let (kind, rest2) = rest.split_once('\t').unwrap_or((rest, rest));
            let (line, placed) = rest2.split_once('\t').unwrap_or((rest2, rest2));
            Ok(Info {
                job: job.to_string(),
                kind: kind.to_string(),
                line: line.to_string(),
                placed: placed.to_string(),
            })
        }
        Err(e) => Err(raised(String::new(), String::new(), &e).print()),
    }
}

fn info_reader(cx: &Cx, body: &[u8], err: &mut String) -> Result<String, Raise> {
    let v = render::load(cx, body, line!("v = json.load"))?;
    let l = line!("j = v[\"job\"]");
    let j = cx.at(l, p::idx(&v, "job"))?;
    let l = line!("c = (j.get(\"srcCommit\")");
    let c = cx.at(l, p::get(&j, "srcCommit"))?;
    let c = cx.at(l, p::slice(&p::or(c, s("")), None, Some(8)))?;
    let l = line!("q = v.get(\"queue\") or {}");
    let q = p::or(cx.at(l, p::get(&v, "queue"))?, Value::Object(vec![]));
    let l = line!("if v.get(\"shards\"):\n");
    let shards = cx.at(l, p::get(&v, "shards"))?;
    let mut line = if p::truthy(&shards) {
        let l = line!("line = \"[k2 compute] job %s: %d shards");
        let short = cx.at(l, p::idx(&j, "short"))?;
        let all = cx.at(l, p::idx(&v, "shards"))?;
        let count = cx.at(l, p::len(&all))?;
        let all = cx.at(l, p::idx(&v, "shards"))?;
        let mut parts = vec![];
        for x in cx.at(l, p::iter(&all))? {
            let index = cx.at(l, p::idx(&x, "index"))?;
            let node = p::or(cx.at(l, p::get(&x, "node"))?, s("?"));
            parts.push(cx.at(l, p::pct("%s:%s", &[index, node]))?);
        }
        cx.at(
            l,
            p::pct(
                &format!("[{CLI_NAME} compute] job %s: %d shards \u{2192} %s"),
                &[short, p::int(count as i128), s(&p::join_strs(", ", &parts))],
            ),
        )?
    } else {
        let l = line!("line = \"[k2 compute] job %s on %s\"");
        let short = cx.at(l, p::idx(&j, "short"))?;
        let node = cx.at(l, p::get(&j, "node"))?;
        let mut line = cx.at(
            l,
            p::pct(&format!("[{CLI_NAME} compute] job %s on %s"), &[short, node]),
        )?;
        if p::truthy(&c) {
            line = add(cx, line!("if c: line +="), &line, " \u{b7} commit ", &c)?;
        }
        let l = line!("line += \" · \" + j[\"state\"]");
        let state = cx.at(l, p::idx(&j, "state"))?;
        line = add(cx, l, &line, " \u{b7} ", &state)?;
        if p::truthy(&q) {
            let l = line!("if q: line +=");
            let pos = cx.at(l, p::get(&q, "position"))?;
            let reason = cx.at(l, p::get(&q, "reason"))?;
            line.push_str(&cx.at(l, p::pct(" (position %s: %s)", &[pos, reason]))?);
        }
        line
    };
    let l = line!("for w in v.get(\"warnings\")");
    let warnings = p::or(cx.at(l, p::get(&v, "warnings"))?, Value::Array(vec![]));
    for w in cx.at(l, p::iter(&warnings))? {
        let l = line!("sys.stderr.write(\"[k2 compute] %s\\n\" % (w if");
        let hint = cx.at(l, p::startswith(&w, "hint: "))?;
        let text = if hint {
            w
        } else {
            s(&cx.at(l, plus("note: ", &w))?)
        };
        err.push_str(&cx.at(
            l,
            p::pct(&format!("[{CLI_NAME} compute] %s\n"), &[text]),
        )?);
    }
    let l = line!("for n in v.get(\"notices\")");
    let notices = p::or(cx.at(l, p::get(&v, "notices"))?, Value::Array(vec![]));
    for n in cx.at(l, p::iter(&notices))? {
        let l = line!("sys.stderr.write(\"note: %s\\n\"");
        let m = cx.at(l, p::get(&n, "message"))?;
        let m = if p::truthy(&m) {
            m
        } else {
            cx.at(l, p::get(&n, "code"))?
        };
        err.push_str(&cx.at(l, p::pct("note: %s\n", &[m]))?);
    }
    let l = line!("rep = v.get(\"replaced\")");
    let rep = p::or(cx.at(l, p::get(&v, "replaced"))?, Value::Array(vec![]));
    if p::truthy(&rep) {
        let l = line!("sys.stderr.write(\"[k2 compute] replaced");
        let count = cx.at(l, p::len(&rep))?;
        let ids: Vec<String> = cx
            .at(l, p::iter(&rep))?
            .iter()
            .map(|r| p::str_of(r).chars().take(8).collect())
            .collect();
        err.push_str(&cx.at(
            l,
            p::pct(
                &format!("[{CLI_NAME} compute] replaced %d queued job(s) with the same --supersede key: %s\n"),
                &[p::int(count as i128), s(&p::join_strs(", ", &ids))],
            ),
        )?);
    }
    let l = line!("if j.get(\"tier\") and");
    let tier = cx.at(l, p::get(&j, "tier"))?;
    if p::truthy(&tier) && !p::eq_s(&tier, "normal") {
        let l = line!("line += \" · tier \"");
        let t = cx.at(l, p::idx(&j, "tier"))?;
        line = add(cx, l, &line, " \u{b7} tier ", &t)?;
    }
    let l = line!("if j.get(\"tierAsked\") and");
    let asked = cx.at(l, p::get(&j, "tierAsked"))?;
    if p::truthy(&asked) && !p::eq(&asked, &cx.at(l, p::get(&j, "tier"))?) {
        let l = line!("line += \" (asked %s)\"");
        let t = cx.at(l, p::idx(&j, "tierAsked"))?;
        line.push_str(&cx.at(l, p::pct(" (asked %s)", &[t]))?);
    }
    let l = line!("src = v.get(\"source\")");
    let src = cx.at(l, p::get(&v, "source"))?;
    if p::is_dict(&src) {
        err.push_str(&source_line(cx, &src, |m| line_of(m))?);
    }
    let l = line!("p = v.get(\"placement\") or");
    let pl = cx.at(l, p::get(&v, "placement"))?;
    let pl = if p::truthy(&pl) {
        pl
    } else {
        p::or(cx.at(l, p::get(&j, "placement"))?, Value::Object(vec![]))
    };
    let mut placed = String::new();
    let l = line!("if p.get(\"mode\") == \"auto\":");
    if p::eq_s(&cx.at(l, p::get(&pl, "mode"))?, "auto") {
        let l = line!("placed = \"[k2 compute] placed on");
        let node = cx.at(l, p::get(&pl, "node"))?;
        let why = cx.at(l, p::get(&pl, "why"))?;
        placed = cx.at(
            l,
            p::pct(
                &format!("[{CLI_NAME} compute] placed on %s (auto): %s"),
                &[node, why],
            ),
        )?;
        let l = line!("if p.get(\"fits\"): placed");
        if p::truthy(&cx.at(l, p::get(&pl, "fits"))?) {
            let f = cx.at(l, p::idx(&pl, "fits"))?;
            placed = add(cx, l, &placed, " \u{b7} ", &f)?;
        }
    }
    let l = line!("print(j[\"id\"] + \"\\t\"");
    let id = cx.at(l, p::idx(&j, "id"))?;
    let id = cx.at(l, plus_value(&id, "\t"))?;
    let kind = if p::truthy(&cx.at(l, p::get(&v, "shards"))?) {
        "set"
    } else {
        "one"
    };
    let clean = |x: &str| x.replace(['\t', '\n'], " ");
    Ok(format!(
        "{id}{kind}\t{}\t{}\n",
        clean(&line),
        clean(&placed)
    ))
}

fn line_of(marker: &str) -> u32 {
    // The source line's markers are found in the info reader's Python.
    match marker {
        "where" => line!("where = src.get(\"checkout\")"),
        "repo" => line!("if src.get(\"repo\") and"),
        "path" => line!("if src.get(\"path\"): where"),
        "s" => line!("s = \"[k2 compute] source:"),
        "d" => line!("d = src.get(\"dirty\")"),
        "dirty" => line!("s += \" +dirty"),
        _ => line!("sys.stderr.write(s + \"\\n\")"),
    }
}

/// The `source` line both readers write: where the job's source comes
/// from, and `+dirty` with its counts.
pub fn source_line(cx: &Cx, src: &Value, at: impl Fn(&str) -> u32) -> Result<String, Raise> {
    let l = at("where");
    let where_ = p::or(cx.at(l, p::get(src, "checkout"))?, s("workspace"));
    let l = at("repo");
    let repo = cx.at(l, p::get(src, "repo"))?;
    let mut where_ = if p::truthy(&repo) && !p::eq_s(&cx.at(l, p::get(src, "checkout"))?, "worktree")
    {
        s("repo")
    } else {
        where_
    };
    let l = at("path");
    if p::truthy(&cx.at(l, p::get(src, "path"))?) {
        let path = cx.at(l, p::idx(src, "path"))?;
        let with_space = cx.at(l, p::add(&where_, &s(" ")))?;
        where_ = cx.at(l, p::add(&with_space, &path))?;
    }
    let l = at("s");
    let commit = p::or(cx.at(l, p::get(src, "commit"))?, s(""));
    let commit = cx.at(l, p::slice(&commit, None, Some(8)))?;
    let mut line = cx.at(
        l,
        p::pct(
            &format!("[{CLI_NAME} compute] source: %s @ %s"),
            &[where_, commit],
        ),
    )?;
    let l = at("d");
    let d = cx.at(l, p::get(src, "dirty"))?;
    if p::is_dict(&d) {
        let l = at("dirty");
        let changed = cx.at(l, p::get(&d, "changed"))?;
        let untracked = cx.at(l, p::get(&d, "untracked"))?;
        line.push_str(&cx.at(
            l,
            p::pct(
                " +dirty (%s changed, %s untracked)",
                &[changed, untracked],
            ),
        )?);
    }
    Ok(line + "\n")
}

/// `line += text + x`: `x` must be a string.
fn add(cx: &Cx, l: u32, line: &str, text: &str, x: &Value) -> Result<String, Raise> {
    Ok(format!("{line}{}", cx.at(l, plus(text, x))?))
}

/// `"text" + x`.
pub fn plus(text: &str, x: &Value) -> p::R<String> {
    match x {
        Value::Str(_) => Ok(format!("{text}{}", p::str_of(x))),
        other => Err(format!(
            "TypeError: can only concatenate str (not \"{}\") to str",
            p::tname(other)
        )),
    }
}

/// `x + "text"`.
pub fn plus_value(x: &Value, text: &str) -> p::R<String> {
    match x {
        Value::Str(_) => Ok(format!("{}{text}", p::str_of(x))),
        Value::Array(_) => Err("TypeError: can only concatenate list (not \"str\") to list".into()),
        other => Err(format!(
            "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
            p::tname(other)
        )),
    }
}
