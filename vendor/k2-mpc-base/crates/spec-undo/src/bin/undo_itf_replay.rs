//! `undo_itf_replay`: replay ITF traces of `spec/quint/undo.qnt` (exported by the simulator
//! through the wrapper that records `act` and `out`) into `spec_undo::model`, through the
//! same bounded world ([`spec_undo::bounded`]), comparing after **every** step: the action is
//! enabled on the Rust side too, the cut agrees, the outcome is equal, and the whole state
//! (text with authors, every stack, every record, budgets, violations) is equal. Any
//! difference is a mismatch, reported with the trace and step, never adjusted.
//!
//! Usage: `undo_itf_replay [bound flags as undo_enum] [--mutant NAME] <file or dir>...`.
//! Ends with one `SUMMARY ...` line; exit 0 with no mismatch, 1 otherwise, 2 on usage.

use std::path::PathBuf;

use spec_undo::bounded::{identity, Bound, State};
use spec_undo::itf;
#[cfg(feature = "mutants")]
use spec_undo::model::mutant;

fn usage(m: &str) -> ! {
    eprintln!("undo_itf_replay: {m}");
    eprintln!(
        "usage: undo_itf_replay [--pids N] [--events N] [--undos N] [--reverts N] [--maxlen N] \
         [--init ab] [--no-multi] [--no-purge] [--no-reduce] [--mutant NAME] [--show N] <file|dir>..."
    );
    std::process::exit(2)
}

fn main() {
    let mut b = Bound::small();
    let mut mutant_name = "none".to_string();
    let mut show = 3usize;
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |n: &str| args.next().unwrap_or_else(|| usage(&format!("{n}?")));
        let num = |s: String| s.parse::<u64>().unwrap_or_else(|_| usage(&s));
        match a.as_str() {
            "--pids" => b.pids = num(val("--pids")) as usize,
            "--events" => b.events = num(val("--events")) as u8,
            "--undos" => b.undos = num(val("--undos")) as u8,
            "--reverts" => b.reverts = num(val("--reverts")) as u8,
            "--maxlen" => b.maxlen = num(val("--maxlen")) as usize,
            "--init" => b.init = val("--init"),
            "--no-multi" => b.multi = false,
            "--no-purge" => b.purge = false,
            "--no-reduce" => b.reduce = false,
            "--mutant" => mutant_name = val("--mutant"),
            "--show" => show = num(val("--show")) as usize,
            _ if a.starts_with("--") => usage(&format!("unknown flag {a}")),
            _ => inputs.push(a.into()),
        }
    }
    #[cfg(feature = "mutants")]
    mutant::set(mutant::Mutant::by_name(&mutant_name).unwrap_or_else(|| usage(&mutant_name)));
    #[cfg(not(feature = "mutants"))]
    if mutant_name != "none" {
        usage("mutants need `--features mutants`");
    }
    let mut files = Vec::new();
    for p in inputs {
        if p.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(&p)
                .unwrap_or_else(|e| usage(&format!("{}: {e}", p.display())))
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.to_string_lossy().ends_with(".itf.json"))
                .collect();
            v.sort();
            files.extend(v);
        } else {
            files.push(p);
        }
    }
    if files.is_empty() {
        usage("no ITF files");
    }
    println!("undo_itf_replay {} mutant={mutant_name}", b.describe());
    let (mut traces, mut steps, mut bad, mut longest) = (0u64, 0u64, 0u64, 0usize);
    let mut kinds = std::collections::BTreeMap::<String, u64>::new();
    let mut viol = std::collections::BTreeSet::<String>::new();
    for f in &files {
        let text =
            std::fs::read_to_string(f).unwrap_or_else(|e| usage(&format!("{}: {e}", f.display())));
        match replay(&b, &text, &mut kinds, &mut viol) {
            Ok(n) => {
                traces += 1;
                steps += n as u64;
                longest = longest.max(n);
            }
            Err((n, what)) => {
                traces += 1;
                steps += n as u64;
                bad += 1;
                if bad as usize <= show {
                    println!("MISMATCH {} step {n}: {what}", f.display());
                }
            }
        }
    }
    println!("actions replayed by kind: {kinds:?}");
    println!(
        "SUMMARY itf traces={traces} steps={steps} longest={longest} mismatches={bad} viol_seen=[{}]",
        viol.into_iter().collect::<Vec<_>>().join(",")
    );
    std::process::exit(i32::from(bad > 0));
}

/// Replay one trace: the number of steps, or the first mismatch.
fn replay(
    b: &Bound,
    text: &str,
    kinds: &mut std::collections::BTreeMap<String, u64>,
    viol: &mut std::collections::BTreeSet<String>,
) -> Result<usize, (usize, String)> {
    let doc = itf::parse(text).map_err(|e| (0, format!("json: {e}")))?;
    let states = itf::states(&doc).map_err(|e| (0, e))?;
    let id = identity(b.pids);
    let mut s = State::init(b);
    let v0 = itf::view(states[0].0, b.pids).map_err(|e| (0, e))?;
    if v0 != s.view(&id) {
        return Err((
            0,
            format!("initial state:\n  quint {v0:?}\n  rust  {:?}", s.view(&id)),
        ));
    }
    for (i, (st, act, out)) in states.iter().enumerate().skip(1) {
        let a = itf::act(act).map_err(|e| (i, e))?;
        let k = format!("{a:?}");
        *kinds
            .entry(k.split([' ', '{']).next().unwrap_or("?").to_owned())
            .or_default() += 1;
        if !s.actions(b).contains(&a) {
            return Err((i, format!("{a:?} is not enabled on the Rust side")));
        }
        let Some((t, o)) = s.step(b, &a) else {
            return Err((i, format!("{a:?}: the Rust side cuts it, Quint took it")));
        };
        let qo = itf::outcome(out).map_err(|e| (i, e))?;
        if itf::normalise(&o) != qo {
            return Err((
                i,
                format!(
                    "{a:?}: outcome\n  quint {qo:?}\n  rust  {:?}",
                    itf::normalise(&o)
                ),
            ));
        }
        let qv = itf::view(st, b.pids).map_err(|e| (i, e))?;
        let rv = t.view(&id);
        if qv != rv {
            return Err((i, format!("{a:?}: state\n  quint {qv:?}\n  rust  {rv:?}")));
        }
        viol.extend(rv.viol.iter().cloned());
        s = t;
    }
    Ok(states.len() - 1)
}
