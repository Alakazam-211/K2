//! `undo_enum`: the exhaustive enumerator of the undo spec's model check (spec §8 "Model
//! check"), over `spec_undo::model` itself through [`spec_undo::bounded`]: every action
//! sequence within a bound, states canonicalised (pid symmetry with `--sym`) and deduplicated,
//! the transition checks on every step. `docs/MODEL-CHECK.md` has the method and results.
//!
//! Modes:
//! - `bfs` (default): level by level with an exact visited set. Its distinct-state count is
//!   TLC's "distinct states found" on `spec/quint/undo.qnt` at the same bound (without
//!   `--sym`). For small bounds.
//! - `dfs`: for large bounds. The states at `--seed-depth` (found breadth-first, exactly)
//!   are seeds; each seed's subtree is explored depth-first over `--threads`, with a visited
//!   set of 128-bit fingerprints per seed (`--dedup seed`, O(subtree) memory; a state
//!   reachable from two seeds counts once in each) or shared by all seeds (`--dedup global`,
//!   exact distinct count up to fingerprint collisions, 16 bytes per state).
//!   `--checkpoint FILE` appends one line per finished seed; a rerun with the same file and
//!   configuration skips those seeds and adds their counts. Progress every `--progress`
//!   seconds (default 60). `--time-limit SECS` stops early (exit 3).
//! - `walk`: `--probes` random walks to the bound's full depth, every transition checked
//!   (not exhaustive: for bounds too large to enumerate, such as the literal G1 bound). The
//!   summary counts the transitions that hit each case class (E-HIST-13, `bounded::CLASSES`).
//!   A progress line (walks, steps, violations, classes so far) every `--progress` seconds.
//!   `--bias redo-after-partial` weighs the next step instead of drawing it uniformly: a
//!   step that redoes a partial undo's entry (the class `redo_after_partial`) weighs 256, a
//!   step that leaves a new such redo entry (an undo that skipped regions) 64, any other
//!   `undo --partial` 4, every other step 1. Every enabled step keeps a weight of at least
//!   1, so no path of the bound is left out, and every step is checked as in a plain walk;
//!   only the odds change (the walks are not a uniform sample of paths).
//! - `knuth`: Knuth's random-probe estimate of the search tree's size per depth (an upper
//!   bound on the states `dfs --dedup seed` visits), for runtime estimates.
//!
//! Every run ends with one `SUMMARY: ...` line (for people) and one `RUNREC {json}` line (the
//! run's record for `tools/evidence/`: `--run NAME`, default from the mode, bound and mutant;
//! status COMPLETE, PARTIAL (a time limit: a lower bound), VIOLATION, SAMPLED (walks: cite
//! `walks`, `steps` by field) or ESTIMATE). `SUMMARY:` has a colon so that the evidence
//! collector, which parses another enumerator's `SUMMARY ` lines, never reads this one: the
//! RUNREC line is this run's record. Exit: 0 complete, no violation of a `--stop`
//! rule; 1 a violation (its least depth and action sequence are printed); 2 usage; 3 time
//! limit.
//!
//! **A panic in the oracle** (one of its own assertions, e.g. `window.rs`'s shared-edge check
//! under a mutant) is caught at the step that raised it and counted as a violation of the
//! pseudo-rule `panic` (always a stop rule): `panic@<depth>` in the SUMMARY with the message
//! (`panic="..."`), the action sequence printed, every worker stopped, exit 1 (v0.9.3; before,
//! a walker's panic left the progress thread running and the process never exited).

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spec_undo::bounded::{self, Act, Bound, State, View, CLASSES, PANIC, STOP};
#[cfg(feature = "mutants")]
use spec_undo::model::mutant;
use spec_undo::model::Mode;

struct Opts {
    b: Bound,
    sym: bool,
    mode: String,
    stop: Vec<&'static str>,
    mutant: String,
    threads: usize,
    seed_depth: usize,
    dedup_global: bool,
    checkpoint: Option<String>,
    progress: u64,
    time_limit: Option<u64>,
    probes: u64,
    rng: u64,
    /// dfs: explore only this many seeds, chosen at random, and extrapolate.
    sample: Option<usize>,
    /// bfs: end with the level of the first violation of a `--stop` rule (as TLC stops).
    first: bool,
    /// bfs: write every distinct state as a line of canonical JSON (`View::json`).
    dump: Option<String>,
    /// The run's name in its `RUNREC` record (default: from the mode, bound and mutant).
    run: Option<String>,
    /// walk: how the next step is drawn ([`Bias`]).
    bias: Bias,
}

/// `walk --bias`: how a walk draws its next step among the enabled ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bias {
    /// Uniformly (the default).
    None,
    /// Toward the class `redo_after_partial` (module doc). Every step keeps a weight >= 1.
    RedoAfterPartial,
}

impl Bias {
    fn name(self) -> &'static str {
        match self {
            Bias::None => "none",
            Bias::RedoAfterPartial => "redo-after-partial",
        }
    }

    fn by_name(name: &str) -> Option<Bias> {
        [Bias::None, Bias::RedoAfterPartial]
            .into_iter()
            .find(|b| b.name() == name)
    }

    /// The weight of the step `a` from `s` to `t` (at least 1).
    fn weight(self, s: &State, a: &Act, t: &State) -> u64 {
        match self {
            Bias::None => 1,
            Bias::RedoAfterPartial => {
                if t.hit.contains(&"redo_after_partial") {
                    256
                } else if after_partial_redos(t) > after_partial_redos(s) {
                    64
                } else if matches!(
                    a,
                    Act::Undo {
                        mode: Mode::Partial,
                        ..
                    }
                ) {
                    4
                } else {
                    1
                }
            }
        }
    }
}

/// The redo entries left by an undo step that skipped regions, over every pid.
fn after_partial_redos(s: &State) -> usize {
    s.m.stacks
        .values()
        .map(|st| st.redo.iter().filter(|b| b.after_partial).count())
        .sum()
}

fn usage(msg: &str) -> ! {
    eprintln!("undo_enum: {msg}");
    eprintln!(
        "usage: undo_enum [bfs|dfs|knuth|walk] [--pids N] [--events N] [--undos N] [--reverts N] [--restarts N] \
         [--maxlen N] [--init ab] [--no-multi] [--no-purge] [--no-reduce] [--u12] [--sym] \
         [--stop R1,R2|all|none] [--mutant NAME] [--threads N] [--seed-depth D] [--run NAME] \
         [--dedup seed|global] [--checkpoint FILE] [--progress SECS] [--time-limit SECS] \
         [--probes N] [--rng N] [--bias none|redo-after-partial] [--sample K] [--first] \
         [--dump FILE]"
    );
    std::process::exit(2)
}

fn parse() -> Opts {
    let mut o = Opts {
        b: Bound::small(),
        sym: false,
        mode: "bfs".into(),
        stop: STOP.to_vec(),
        mutant: "none".into(),
        threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
        seed_depth: 3,
        dedup_global: false,
        checkpoint: None,
        progress: 60,
        time_limit: None,
        probes: 100_000,
        rng: 1,
        sample: None,
        first: false,
        run: None,
        dump: None,
        bias: Bias::None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |name: &str| args.next().unwrap_or_else(|| usage(&format!("{name}?")));
        let num = |s: String| s.parse::<u64>().unwrap_or_else(|_| usage(&s));
        match a.as_str() {
            "bfs" | "dfs" | "knuth" | "walk" => o.mode = a.clone(),
            "--g1" => o.b = Bound::g1(),
            "--pids" => o.b.pids = num(val("--pids")) as usize,
            "--events" => o.b.events = num(val("--events")) as u8,
            "--undos" => o.b.undos = num(val("--undos")) as u8,
            "--reverts" => o.b.reverts = num(val("--reverts")) as u8,
            "--restarts" => o.b.restarts = num(val("--restarts")) as u8,
            "--maxlen" => o.b.maxlen = num(val("--maxlen")) as usize,
            "--init" => o.b.init = val("--init"),
            "--no-multi" => o.b.multi = false,
            "--no-purge" => o.b.purge = false,
            "--no-reduce" => o.b.reduce = false,
            "--u12" => o.b.u12 = true,
            "--sym" => o.sym = true,
            "--stop" => {
                let v = val("--stop");
                o.stop = match v.as_str() {
                    "all" => bounded::RULES.to_vec(),
                    "none" => Vec::new(),
                    _ => v
                        .split(',')
                        .map(|r| {
                            *bounded::RULES
                                .iter()
                                .find(|x| **x == r)
                                .unwrap_or_else(|| usage(&format!("no rule {r}")))
                        })
                        .collect(),
                }
            }
            "--mutant" => o.mutant = val("--mutant"),
            "--threads" => o.threads = num(val("--threads")).max(1) as usize,
            "--seed-depth" => o.seed_depth = num(val("--seed-depth")) as usize,
            "--dedup" => o.dedup_global = val("--dedup") == "global",
            "--checkpoint" => o.checkpoint = Some(val("--checkpoint")),
            "--progress" => o.progress = num(val("--progress")).max(1),
            "--time-limit" => o.time_limit = Some(num(val("--time-limit"))),
            "--probes" => o.probes = num(val("--probes")),
            "--rng" => o.rng = num(val("--rng")),
            "--bias" => {
                let v = val("--bias");
                o.bias = Bias::by_name(&v).unwrap_or_else(|| usage(&format!("bias {v}")));
            }
            "--run" => o.run = Some(val("--run")),
            "--sample" => o.sample = Some(num(val("--sample")) as usize),
            "--first" => o.first = true,
            "--dump" => o.dump = Some(val("--dump")),
            _ => usage(&format!("unknown argument {a}")),
        }
    }
    if o.bias != Bias::None && o.mode != "walk" {
        usage("--bias is for walk only");
    }
    // a panic in the oracle always stops a run
    o.stop.push(PANIC);
    o
}

fn set_mutant(name: &str) {
    #[cfg(feature = "mutants")]
    {
        let m = mutant::Mutant::by_name(name).unwrap_or_else(|| usage(&format!("mutant {name}")));
        mutant::set(m);
    }
    #[cfg(not(feature = "mutants"))]
    if name != "none" {
        usage("mutants need `--features mutants`");
    }
}

fn fingerprint(v: &View) -> u128 {
    let h = |salt: u8| {
        let mut s = DefaultHasher::new();
        salt.hash(&mut s);
        v.hash(&mut s);
        s.finish()
    };
    (u128::from(h(0)) << 64) | u128::from(h(1))
}

/// The canonical fingerprint: the least over every renaming of pids (`sym`), so states
/// equal up to renaming have one.
fn canon_fp(s: &State, sym: bool, perms: &[Vec<usize>]) -> u128 {
    if !sym {
        return fingerprint(&s.view(&perms[0]));
    }
    perms
        .iter()
        .map(|p| fingerprint(&s.view(p)))
        .min()
        .expect("a permutation")
}

fn print_path(b: &Bound, path: &[Act]) {
    let mut s = State::init(b);
    println!("  0: {:?}", s.text());
    for (i, a) in path.iter().enumerate() {
        let (t, o) = match s.try_step(b, a) {
            Ok(Some(x)) => x,
            Ok(None) => {
                println!("  {}: {a:?}\n     -> cut", i + 1);
                return;
            }
            Err(msg) => {
                println!("  {}: {a:?}\n     -> the oracle panicked: {msg}", i + 1);
                return;
            }
        };
        println!(
            "  {}: {a:?}\n     -> {:?} authors {:?} {o:?}",
            i + 1,
            t.text(),
            t.authors
        );
        s = t;
    }
}

/// ` panic="<message>"` for a SUMMARY line, when the oracle panicked.
fn panic_field(msg: Option<&str>) -> String {
    msg.map_or(String::new(), |m| {
        format!(" panic={:?}", m.lines().next().unwrap_or(""))
    })
}

/// One `RUNREC` line for the run (the evidence records, `tools/evidence/README.md`): the
/// run's name, `"tool":"enumerator"`, its mode, and `fields` (already JSON values).
fn runrec(o: &Opts, t0: Instant, exit: i32, fields: &[(&str, String)]) {
    let b = &o.b;
    let run = o.run.clone().unwrap_or_else(|| {
        format!(
            "{}_{}x{}_u{}_r{}_rs{}_{}{}{}",
            o.mode,
            b.pids,
            b.events,
            b.undos,
            b.reverts,
            b.restarts,
            o.mutant,
            if o.sym { "_sym" } else { "" },
            match o.bias {
                Bias::None => String::new(),
                bias => format!("_bias-{}", bias.name()),
            }
        )
    });
    let mut j = format!(
        "{{\"job\":null,\"run\":{:?},\"tool\":\"enumerator\",\"mode\":{:?}",
        run, o.mode
    );
    if o.bias != Bias::None {
        j.push_str(&format!(",\"bias\":{:?}", o.bias.name()));
    }
    for (k, v) in fields {
        j.push_str(&format!(",{k:?}:{v}"));
    }
    j.push_str(&format!(
        ",\"exit\":{exit},\"wall_s\":{},\"bound\":{:?}}}",
        t0.elapsed().as_secs(),
        b.describe()
    ));
    println!("RUNREC {j}");
}

/// A JSON string.
fn js(s: &str) -> String {
    format!("{s:?}")
}

fn header(o: &Opts) -> String {
    format!(
        "undo_enum {} {} sym={} mutant={} stop={} seed-depth={} dedup={}",
        o.mode,
        o.b.describe(),
        o.sym,
        o.mutant,
        o.stop.join(","),
        o.seed_depth,
        if o.dedup_global { "global" } else { "seed" }
    ) + &match o.bias {
        Bias::None => String::new(),
        bias => format!(" bias={}", bias.name()),
    }
}

fn main() {
    let o = parse();
    set_mutant(&o.mutant);
    println!("{}", header(&o));
    let code = match o.mode.as_str() {
        "bfs" => run_bfs(&o),
        "dfs" => run_dfs(&o),
        "knuth" => run_knuth(&o),
        "walk" => run_walk(&o),
        _ => unreachable!(),
    };
    std::process::exit(code);
}

/// `class:count,...` over every class (a class never hit shows 0).
fn classes_line(counts: &[u64]) -> String {
    CLASSES
        .iter()
        .zip(counts)
        .map(|(c, n)| format!("{c}:{n}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn count_classes(counts: &mut [u64], s: &State) {
    for h in &s.hit {
        if let Some(k) = CLASSES.iter().position(|c| c == h) {
            counts[k] += 1;
        }
    }
}

fn run_bfs(o: &Opts) -> i32 {
    let t0 = Instant::now();
    let mut dump = o
        .dump
        .as_ref()
        .map(|f| std::io::BufWriter::new(std::fs::File::create(f).expect("create the dump")));
    let mut classes = vec![0u64; CLASSES.len()];
    let c = bounded::bfs(&o.b, o.sym, &o.stop, o.first, |s, v| {
        count_classes(&mut classes, s);
        if let Some(d) = dump.as_mut() {
            writeln!(d, "{}", v.json()).expect("write the dump");
        }
    });
    drop(dump);
    println!("per depth: {:?}", c.per_depth);
    for (rule, (depth, path)) in &c.violations {
        let stops = o.stop.contains(rule);
        println!(
            "violation {rule} at depth {depth}{}:",
            if stops { "" } else { " (observed)" }
        );
        print_path(&o.b, path);
    }
    let stopped: Vec<_> = c
        .violations
        .iter()
        .filter(|(r, _)| o.stop.contains(r))
        .map(|(r, (d, _))| format!("{r}@{d}"))
        .collect();
    let observed: Vec<_> = c
        .violations
        .iter()
        .filter(|(r, _)| !o.stop.contains(r))
        .map(|(r, (d, _))| format!("{r}@{d}"))
        .collect();
    println!(
        "SUMMARY: mode=bfs distinct={} transitions={} cut={} max_depth={} violations=[{}] observed=[{}] classes=[{}] wall={:.1}s{}",
        c.distinct(),
        c.transitions,
        c.cut,
        c.per_depth.len() - 1,
        stopped.join(","),
        observed.join(","),
        classes_line(&classes),
        t0.elapsed().as_secs_f64(),
        panic_field(c.panic.as_deref())
    );
    let exit = i32::from(!stopped.is_empty());
    let least = c
        .violations
        .iter()
        .filter(|(r, _)| o.stop.contains(r))
        .min_by_key(|(_, (d, _))| *d);
    let mut f = vec![
        ("states", c.distinct().to_string()),
        ("depth", (c.per_depth.len() - 1).to_string()),
        ("transitions", c.transitions.to_string()),
        ("cut", c.cut.to_string()),
        ("queued", "0".into()),
    ];
    match least {
        Some((r, (d, _))) => {
            f.push(("status", js("VIOLATION")));
            f.push(("violation", js(r)));
            f.push(("trace_states", (d + 1).to_string()));
            f.push(("violation_depth", d.to_string()));
        }
        None => f.push(("status", js("COMPLETE"))),
    }
    if let Some(m) = &c.panic {
        f.push(("error", js(m)));
    }
    runrec(o, t0, exit, &f);
    exit
}

/// The distinct states at `depth`, breadth-first and exact (canonical fingerprints), with
/// the violations met on the way.
/// The seeds (each with its action sequence), the states above them, the violations met,
/// and the oracle's panic message if it panicked (the search stopped there).
type Seeds = (Vec<(State, Vec<Act>)>, u64, Vec<String>, Option<String>);
/// Per finished seed: states, max depth, violations (`rule@depth`).
type SeedResult = (u64, usize, Vec<String>);

fn seeds(o: &Opts, perms: &[Vec<usize>]) -> Seeds {
    let b = &o.b;
    let s0 = State::init(b);
    let mut seen = HashSet::new();
    seen.insert(canon_fp(&s0, o.sym, perms));
    let mut level = vec![(s0, Vec::new())];
    let mut viol = Vec::new();
    for _ in 0..o.seed_depth {
        let mut next = Vec::new();
        for (s, path) in &level {
            for a in s.actions(b) {
                let t = match s.try_step(b, &a) {
                    Ok(Some((t, _))) => t,
                    Ok(None) => continue,
                    Err(msg) => {
                        let mut p = path.clone();
                        p.push(a);
                        println!("violation {PANIC} at depth {} (seed search):", p.len());
                        print_path(b, &p);
                        viol.push(format!("{PANIC}@{}", p.len()));
                        return (Vec::new(), seen.len() as u64, viol, Some(msg));
                    }
                };
                if !seen.insert(canon_fp(&t, o.sym, perms)) {
                    continue;
                }
                let mut p = path.clone();
                p.push(a);
                for r in t.viol.difference(&s.viol) {
                    viol.push(format!("{r}@{}", p.len()));
                    if o.stop.contains(r) {
                        println!("violation {r} at depth {} (seed search):", p.len());
                        print_path(b, &p);
                    }
                }
                if !t.viol.iter().any(|r| o.stop.contains(r)) {
                    next.push((t, p));
                }
            }
        }
        if next.is_empty() {
            break;
        }
        level = next;
    }
    // the states shallower than the seeds (each seed is counted by its own subtree); a
    // seed search that stopped early has its last level as the seeds
    let above = seen.len() as u64 - level.len() as u64;
    (level, above, viol, None)
}

/// One seed's subtree, depth first.
struct Walk<'a> {
    o: &'a Opts,
    perms: &'a [Vec<usize>],
    local: HashSet<u128>,
    global: Option<&'a [Mutex<HashSet<u128>>]>,
    states: u64,
    max_depth: usize,
    viol: BTreeMap<&'static str, (usize, Vec<Act>)>,
    counter: &'a AtomicU64,
    stop: &'a AtomicBool,
    /// The oracle panicked in this subtree: its message (the run stops).
    panic: Option<String>,
}

impl Walk<'_> {
    fn fresh(&mut self, fp: u128) -> bool {
        match self.global {
            Some(shards) => {
                let k = (fp as usize) % shards.len();
                shards[k].lock().expect("a shard").insert(fp)
            }
            None => self.local.insert(fp),
        }
    }

    fn go(&mut self, s: &State, path: &mut Vec<Act>) {
        if self.stop.load(Ordering::Relaxed) {
            return;
        }
        self.max_depth = self.max_depth.max(path.len());
        for a in s.actions(&self.o.b) {
            let t = match s.try_step(&self.o.b, &a) {
                Ok(Some((t, _))) => t,
                Ok(None) => continue,
                Err(msg) => {
                    path.push(a);
                    self.viol
                        .entry(PANIC)
                        .or_insert_with(|| (path.len(), path.clone()));
                    path.pop();
                    self.panic = Some(msg);
                    self.stop.store(true, Ordering::Relaxed);
                    return;
                }
            };
            if !self.fresh(canon_fp(&t, self.o.sym, self.perms)) {
                continue;
            }
            self.states += 1;
            if self.states % 4096 == 0 {
                self.counter.fetch_add(4096, Ordering::Relaxed);
            }
            path.push(a);
            for r in t.viol.difference(&s.viol) {
                self.viol
                    .entry(r)
                    .or_insert_with(|| (path.len(), path.clone()));
            }
            if !t.viol.iter().any(|r| self.o.stop.contains(r)) {
                self.go(&t, path);
            }
            path.pop();
        }
    }
}

fn run_dfs(o: &Opts) -> i32 {
    let t0 = Instant::now();
    let perms = bounded::perms(o.b.pids);
    let (mut seeds, above, seed_viol, seed_panic) = seeds(o, &perms);
    if let Some(msg) = seed_panic {
        println!(
            "SUMMARY: mode=dfs states={above} seeds=0/0 complete=false max_depth=0 dedup={} violations=[{}] observed=[] wall={:.1}s{}",
            if o.dedup_global { "global" } else { "seed" },
            seed_viol
                .iter()
                .filter(|v| v.starts_with(PANIC))
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
            t0.elapsed().as_secs_f64(),
            panic_field(Some(&msg))
        );
        runrec(
            o,
            t0,
            1,
            &[
                ("status", js("VIOLATION")),
                ("violation", js(PANIC)),
                ("error", js(&msg)),
            ],
        );
        return 1;
    }
    let all_seeds = seeds.len();
    if let Some(k) = o.sample {
        // a uniform sample of k seeds (a partial Fisher-Yates with a fixed xorshift)
        let mut x = o.rng.max(1);
        for i in 0..k.min(seeds.len()) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let j = i + (x % (seeds.len() - i) as u64) as usize;
            seeds.swap(i, j);
        }
        seeds.truncate(k);
        println!("sampling {} of {all_seeds} seeds", seeds.len());
    }
    let n = seeds.len();
    println!(
        "seeds: {n} at depth {} ({above} distinct states above them), {} threads",
        o.seed_depth, o.threads
    );
    // checkpoint: header, then "seed K states N max_depth D viol R@d,..."
    let mut done: BTreeMap<usize, SeedResult> = BTreeMap::new();
    if let Some(f) = &o.checkpoint {
        if let Ok(text) = std::fs::read_to_string(f) {
            let mut lines = text.lines();
            if let Some(h) = lines.next() {
                if h != header(o) {
                    eprintln!("checkpoint {f} is for another configuration:\n  {h}");
                    return 2;
                }
            }
            for l in lines {
                let w: Vec<&str> = l.split_whitespace().collect();
                if w.len() >= 6 && w[0] == "seed" {
                    let viol = w
                        .get(7)
                        .map(|v| v.split(',').map(str::to_owned).collect())
                        .unwrap_or_default();
                    done.insert(
                        w[1].parse().expect("seed"),
                        (
                            w[3].parse().expect("states"),
                            w[5].parse().expect("depth"),
                            viol,
                        ),
                    );
                }
            }
            println!("resuming: {} seeds done", done.len());
        } else {
            std::fs::write(f, format!("{}\n", header(o))).expect("write the checkpoint");
        }
    }
    let ck = o.checkpoint.as_ref().map(|f| {
        Mutex::new(
            std::fs::OpenOptions::new()
                .append(true)
                .open(f)
                .expect("open the checkpoint"),
        )
    });
    let shards: Option<Vec<Mutex<HashSet<u128>>>> = o
        .dedup_global
        .then(|| (0..256).map(|_| Mutex::new(HashSet::new())).collect());
    let next = AtomicUsize::new(0);
    let counter = Arc::new(AtomicU64::new(0));
    let stop = AtomicBool::new(false);
    let results: Mutex<BTreeMap<usize, SeedResult>> = Mutex::new(done.clone());
    let first_viol: Mutex<BTreeMap<&'static str, (usize, Vec<Act>)>> = Mutex::new(BTreeMap::new());
    let panicked: Mutex<Option<String>> = Mutex::new(None);
    let finished = AtomicUsize::new(done.len());
    std::thread::scope(|sc| {
        // progress
        sc.spawn(|| {
            let mut last = Instant::now();
            while finished.load(Ordering::Relaxed) < n && !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(200));
                if let Some(t) = o.time_limit {
                    if t0.elapsed() >= Duration::from_secs(t) {
                        stop.store(true, Ordering::Relaxed);
                    }
                }
                if last.elapsed() >= Duration::from_secs(o.progress) {
                    last = Instant::now();
                    let st = counter.load(Ordering::Relaxed);
                    let el = t0.elapsed().as_secs_f64();
                    println!(
                        "progress: ~{st} states, seeds {}/{n}, {:.0} states/s, rss {} MiB, {:.0}s",
                        finished.load(Ordering::Relaxed),
                        st as f64 / el.max(1e-9),
                        rss_mib(),
                        el
                    );
                    let _ = std::io::stdout().flush();
                }
            }
        });
        for _ in 0..o.threads {
            sc.spawn(|| loop {
                let k = next.fetch_add(1, Ordering::Relaxed);
                if k >= n || stop.load(Ordering::Relaxed) {
                    break;
                }
                if done.contains_key(&k) {
                    continue;
                }
                let (s, p) = &seeds[k];
                let mut w = Walk {
                    o,
                    perms: &perms,
                    local: HashSet::new(),
                    global: shards.as_deref(),
                    states: 1,
                    max_depth: p.len(),
                    viol: BTreeMap::new(),
                    counter: &counter,
                    stop: &stop,
                    panic: None,
                };
                let mut path = p.clone();
                w.go(s, &mut path);
                let vs: Vec<String> = w
                    .viol
                    .iter()
                    .map(|(r, (d, _))| format!("{r}@{d}"))
                    .collect();
                {
                    let mut fv = first_viol.lock().expect("lock");
                    for (r, (d, path)) in w.viol {
                        let e = fv.entry(r).or_insert((d, path.clone()));
                        if d < e.0 {
                            *e = (d, path);
                        }
                    }
                }
                if let Some(msg) = w.panic {
                    panicked.lock().expect("lock").get_or_insert(msg);
                }
                if stop.load(Ordering::Relaxed) {
                    break; // an unfinished seed is not checkpointed
                }
                if let Some(ck) = &ck {
                    let mut f = ck.lock().expect("lock");
                    writeln!(
                        f,
                        "seed {k} states {} max_depth {} viol {}",
                        w.states,
                        w.max_depth,
                        if vs.is_empty() {
                            "-".into()
                        } else {
                            vs.join(",")
                        }
                    )
                    .expect("write the checkpoint");
                    println!(
                        "ckpt seed {k} states {} max_depth {}",
                        w.states, w.max_depth
                    );
                }
                results
                    .lock()
                    .expect("lock")
                    .insert(k, (w.states, w.max_depth, vs));
                finished.fetch_add(1, Ordering::Relaxed);
            });
        }
    });
    let res = results.into_inner().expect("results");
    let states: u64 = above + res.values().map(|r| r.0).sum::<u64>();
    let max_depth = res.values().map(|r| r.1).max().unwrap_or(0);
    let mut viol: BTreeSet<String> = seed_viol.into_iter().collect();
    for r in res.values() {
        viol.extend(r.2.iter().filter(|v| *v != "-").cloned());
    }
    let mut least: BTreeMap<String, usize> = BTreeMap::new();
    for v in &viol {
        if let Some((r, d)) = v.split_once('@') {
            let d: usize = d.parse().expect("depth");
            let e = least.entry(r.to_owned()).or_insert(d);
            *e = (*e).min(d);
        }
    }
    let first_viol = first_viol.into_inner().expect("viol");
    // a panic (in a seed the run left unfinished) counts too
    for (r, (d, _)) in &first_viol {
        let e = least.entry((*r).to_owned()).or_insert(*d);
        *e = (*e).min(*d);
    }
    let panicked = panicked.into_inner().expect("panic");
    for (r, (d, path)) in first_viol {
        if o.stop.contains(&r) {
            println!("violation {r} at depth {d}:");
            print_path(&o.b, &path);
        }
    }
    let fmt = |stops: bool| -> String {
        least
            .iter()
            .filter(|(r, _)| o.stop.contains(&r.as_str()) == stops)
            .map(|(r, d)| format!("{r}@{d}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let complete = res.len() == n;
    if o.sample.is_some() && panicked.is_none() {
        let sub: Vec<f64> = res.values().map(|r| r.0 as f64).collect();
        let mean = sub.iter().sum::<f64>() / sub.len().max(1) as f64;
        let var =
            sub.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (sub.len().max(2) - 1) as f64;
        let est = above as f64 + mean * all_seeds as f64;
        let se = (var / sub.len().max(1) as f64).sqrt() * all_seeds as f64;
        println!(
            "SUMMARY: mode=dfs-sample estimate={est:.3e} se={se:.2e} seeds={}/{all_seeds} mean_per_seed={mean:.3e} max_per_seed={:.3e} wall={:.1}s ({:.0} states/s)",
            res.len(),
            sub.iter().cloned().fold(0.0, f64::max),
            t0.elapsed().as_secs_f64(),
            sub.iter().sum::<f64>() / t0.elapsed().as_secs_f64()
        );
        runrec(
            o,
            t0,
            0,
            &[
                ("status", js("ESTIMATE")),
                ("estimate", format!("{}", est.round() as u64)),
                ("seeds", res.len().to_string()),
            ],
        );
        return 0;
    }
    println!(
        "SUMMARY: mode=dfs states={states} seeds={}/{n} complete={complete} max_depth={max_depth} dedup={} violations=[{}] observed=[{}] wall={:.1}s{}",
        res.len(),
        if o.dedup_global { "global" } else { "seed" },
        fmt(true),
        fmt(false),
        t0.elapsed().as_secs_f64(),
        panic_field(panicked.as_deref())
    );
    let exit = if !fmt(true).is_empty() {
        1
    } else if !complete {
        3
    } else {
        0
    };
    let least_stop = least
        .iter()
        .filter(|(r, _)| o.stop.contains(&r.as_str()))
        .min_by_key(|(_, d)| **d);
    let mut f = vec![
        ("states", states.to_string()),
        ("depth", max_depth.to_string()),
        ("seeds", format!("{}", res.len())),
        ("seeds_all", n.to_string()),
    ];
    match least_stop {
        Some((r, d)) => {
            f.push(("status", js("VIOLATION")));
            f.push(("violation", js(r)));
            f.push(("trace_states", (d + 1).to_string()));
        }
        // a run its time limit stopped: its count is a lower bound
        None if !complete => {
            f.push(("status", js("PARTIAL")));
            f.push(("queued", (n - res.len()).to_string()));
            if let Some(t) = o.time_limit {
                f.push(("cap_s", t.to_string()));
            }
        }
        None => {
            f.push(("status", js("COMPLETE")));
            f.push(("queued", "0".into()));
        }
    }
    if let Some(m) = &panicked {
        f.push(("error", js(m)));
    }
    runrec(o, t0, exit, &f);
    exit
}

/// Knuth's estimator: random root-to-leaf probes over the search tree (successors not cut,
/// no dedup); the mean of the products of branching factors estimates the nodes per depth.
fn run_knuth(o: &Opts) -> i32 {
    let t0 = Instant::now();
    let mut x = o.rng.max(1);
    let mut rnd = move |n: usize| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x % n as u64) as usize
    };
    let mut per: Vec<f64> = Vec::new();
    for _ in 0..o.probes {
        let mut s = State::init(&o.b);
        let mut weight = 1.0f64;
        let mut depth = 0;
        loop {
            if per.len() <= depth {
                per.push(0.0);
            }
            per[depth] += weight;
            let mut kids: Vec<State> = Vec::new();
            for a in s.actions(&o.b) {
                match s.try_step(&o.b, &a) {
                    Ok(Some((t, _))) => kids.push(t),
                    Ok(None) => {}
                    Err(msg) => {
                        println!(
                            "SUMMARY: mode=knuth violations=[{PANIC}@{}] wall={:.1}s{}",
                            depth + 1,
                            t0.elapsed().as_secs_f64(),
                            panic_field(Some(&msg))
                        );
                        runrec(
                            o,
                            t0,
                            1,
                            &[
                                ("status", js("VIOLATION")),
                                ("violation", js(PANIC)),
                                ("trace_states", (depth + 2).to_string()),
                                ("error", js(&msg)),
                            ],
                        );
                        return 1;
                    }
                }
            }
            if kids.is_empty() {
                break;
            }
            weight *= kids.len() as f64;
            let k = rnd(kids.len());
            s = kids.into_iter().nth(k).expect("a child");
            depth += 1;
        }
    }
    let per: Vec<f64> = per.iter().map(|v| v / o.probes as f64).collect();
    let total: f64 = per.iter().sum();
    let probe_s = t0.elapsed().as_secs_f64() / o.probes as f64;
    println!(
        "estimate per depth: {:?}",
        per.iter().map(|v| format!("{v:.3e}")).collect::<Vec<_>>()
    );
    println!(
        "SUMMARY: mode=knuth tree_nodes~{total:.3e} probes={} max_depth={} wall={:.1}s ({probe_s:.2e} s/probe)",
        o.probes,
        per.len() - 1,
        t0.elapsed().as_secs_f64()
    );
    runrec(
        o,
        t0,
        0,
        &[
            ("status", js("ESTIMATE")),
            ("tree_nodes", format!("{}", total.round() as u64)),
            ("probes", o.probes.to_string()),
        ],
    );
    0
}

/// Random walks to full depth over `--threads`, every transition checked; each rule's
/// shortest witness seen is printed.
fn run_walk(o: &Opts) -> i32 {
    let t0 = Instant::now();
    let done = AtomicU64::new(0);
    let finished = AtomicU64::new(0);
    let steps = AtomicU64::new(0);
    let first: Mutex<BTreeMap<&'static str, Vec<Act>>> = Mutex::new(BTreeMap::new());
    let classes: Vec<AtomicU64> = CLASSES.iter().map(|_| AtomicU64::new(0)).collect();
    let all_done = AtomicBool::new(false);
    // the oracle panicked: every walker stops (its message)
    let halt = AtomicBool::new(false);
    let panicked: Mutex<Option<String>> = Mutex::new(None);
    /// Ends the progress thread however the walkers end (a panic included), so the scope
    /// can join it.
    struct Done<'a>(&'a AtomicBool);
    impl Drop for Done<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }
    std::thread::scope(|sc| {
        let done_guard = Done(&all_done);
        // progress, at least every `--progress` seconds (a node stops a job that prints
        // nothing for its stall timeout)
        sc.spawn(|| {
            let mut last = Instant::now();
            while !all_done.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(200));
                if last.elapsed() < Duration::from_secs(o.progress) {
                    continue;
                }
                last = Instant::now();
                let counts: Vec<u64> = classes.iter().map(|c| c.load(Ordering::Relaxed)).collect();
                let viol: Vec<String> = first
                    .lock()
                    .expect("lock")
                    .iter()
                    .map(|(r, p)| format!("{r}@{}", p.len()))
                    .collect();
                println!(
                    "progress: walks {}/{} steps {} violations=[{}] classes=[{}] {:.0}s",
                    finished.load(Ordering::Relaxed),
                    o.probes,
                    steps.load(Ordering::Relaxed),
                    viol.join(","),
                    classes_line(&counts),
                    t0.elapsed().as_secs_f64()
                );
                let _ = std::io::stdout().flush();
            }
        });
        let workers: Vec<_> = (0..o.threads)
            .map(|t| {
                let (done, finished, steps, first, classes, halt, panicked) =
                    (&done, &finished, &steps, &first, &classes, &halt, &panicked);
                sc.spawn(move || {
                    let mut x = o.rng.max(1).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ (t as u64 + 1);
                    'walks: while !halt.load(Ordering::Relaxed)
                        && done.fetch_add(1, Ordering::Relaxed) < o.probes
                    {
                        let mut s = State::init(&o.b);
                        let mut path: Vec<Act> = Vec::new();
                        loop {
                            if halt.load(Ordering::Relaxed) {
                                break 'walks;
                            }
                            let mut kids: Vec<(Act, State)> = Vec::new();
                            for a in s.actions(&o.b) {
                                match s.try_step(&o.b, &a) {
                                    Ok(Some((t, _))) => kids.push((a, t)),
                                    Ok(None) => {}
                                    Err(msg) => {
                                        // the oracle panicked: a violation, and the run stops
                                        let mut p = path.clone();
                                        p.push(a);
                                        let mut f = first.lock().expect("lock");
                                        let e = f.entry(PANIC).or_insert_with(|| p.clone());
                                        if p.len() < e.len() {
                                            *e = p;
                                        }
                                        panicked.lock().expect("lock").get_or_insert(msg);
                                        halt.store(true, Ordering::Relaxed);
                                        break 'walks;
                                    }
                                }
                            }
                            if kids.is_empty() {
                                break;
                            }
                            x ^= x << 13;
                            x ^= x >> 7;
                            x ^= x << 17;
                            let pick = match o.bias {
                                // uniform: the same draw as before the bias existed
                                Bias::None => (x % kids.len() as u64) as usize,
                                bias => {
                                    let w: Vec<u64> =
                                        kids.iter().map(|(a, t)| bias.weight(&s, a, t)).collect();
                                    let mut r = x % w.iter().sum::<u64>();
                                    w.iter()
                                        .position(|&k| {
                                            if r < k {
                                                true
                                            } else {
                                                r -= k;
                                                false
                                            }
                                        })
                                        .expect("r < the total weight")
                                }
                            };
                            let (a, t) = kids.swap_remove(pick);
                            path.push(a);
                            steps.fetch_add(1, Ordering::Relaxed);
                            for h in &t.hit {
                                if let Some(k) = CLASSES.iter().position(|c| c == h) {
                                    classes[k].fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            for r in t.viol.difference(&s.viol) {
                                let mut f = first.lock().expect("lock");
                                let e = f.entry(r).or_insert_with(|| path.clone());
                                if path.len() < e.len() {
                                    *e = path.clone();
                                }
                            }
                            // a state past a stop rule is not explored (as bfs and dfs)
                            if t.viol.iter().any(|r| o.stop.contains(r)) {
                                break;
                            }
                            s = t;
                        }
                        finished.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect();
        for w in workers {
            if let Err(p) = w.join() {
                // a panic outside the oracle's steps: reported like one
                panicked
                    .lock()
                    .expect("lock")
                    .get_or_insert(bounded::panic_message(p.as_ref()));
                first.lock().expect("lock").entry(PANIC).or_default();
            }
        }
        drop(done_guard);
    });
    let panicked = panicked.into_inner().expect("panic");
    let classes: Vec<u64> = classes.iter().map(|c| c.load(Ordering::Relaxed)).collect();
    for (c, n) in CLASSES.iter().zip(&classes) {
        println!("class {c:24} {n}");
    }
    println!(
        "classes outside the bounded world (the trace test covers them): leave/rejoin, \
         multi-byte text, write --base, the revert window; concurrency: later"
    );
    let first = first.into_inner().expect("first");
    for (r, path) in &first {
        if o.stop.contains(r) {
            println!("violation {r} at depth {}:", path.len());
            print_path(&o.b, path);
        }
    }
    let fmt = |stops: bool| -> String {
        first
            .iter()
            .filter(|(r, _)| o.stop.contains(*r) == stops)
            .map(|(r, p)| format!("{r}@{}", p.len()))
            .collect::<Vec<_>>()
            .join(",")
    };
    println!(
        "SUMMARY: mode=walk walks={} steps={} violations=[{}] observed=[{}] classes=[{}] wall={:.1}s{}",
        finished.load(Ordering::Relaxed),
        steps.load(Ordering::Relaxed),
        fmt(true),
        fmt(false),
        classes_line(&classes),
        t0.elapsed().as_secs_f64(),
        panic_field(panicked.as_deref())
    );
    let exit = i32::from(!fmt(true).is_empty());
    // walks sample the bound: no state count; cite `walks` and `steps` by field
    let least = first
        .iter()
        .filter(|(r, _)| o.stop.contains(*r))
        .min_by_key(|(_, p)| p.len());
    let mut f = vec![
        ("walks", finished.load(Ordering::Relaxed).to_string()),
        ("walks_asked", o.probes.to_string()),
        ("steps", steps.load(Ordering::Relaxed).to_string()),
    ];
    for (c, n) in CLASSES.iter().zip(&classes) {
        f.push((c, n.to_string()));
    }
    match least {
        Some((r, p)) => {
            f.push(("status", js("VIOLATION")));
            f.push(("violation", js(r)));
            f.push(("trace_states", (p.len() + 1).to_string()));
        }
        None => f.push(("status", js("SAMPLED"))),
    }
    if let Some(m) = &panicked {
        f.push(("error", js(m)));
    }
    runrec(o, t0, exit, &f);
    exit
}

fn rss_mib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
        })
        .map_or(0, |kb| kb / 1024)
}
