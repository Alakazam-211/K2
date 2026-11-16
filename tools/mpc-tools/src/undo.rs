//! `undo`: the undo model check's tools.
//!
//! `undo ck-from-log LOG [--out DIR]`: rebuild the checkpoint files of the `undo_enum dfs` runs
//! in a job's log (tools/undo/g1.sh, "Resume"): for each run, its header line, then each
//! finished seed's `ckpt seed K states N max_depth D` line as `seed K states N max_depth D viol
//! -`. Writes `ck-<name>.txt` in DIR (default: the current folder), `<name>` as g1.sh names the
//! run (3x1u2r1, 2x2u2r1, 3x2u2r0), and prints each file's seed count and sha256. A seed is
//! checkpointed only when it finished without a violation, so `viol` is always `-`. Exit 1 when
//! a header lacks one of pids, events, undos, reverts, or a run lists a seed twice (the files
//! before it are written).

use std::collections::BTreeMap;
use std::path::PathBuf;

use regex::Regex;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{Ctx, Fail, Report, ERROR, OK};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Rebuild the dfs runs' checkpoint files from a job's log
    CkFromLog {
        /// The job's log
        log: PathBuf,
        /// Where the `ck-<name>.txt` files go (default: the current folder)
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
}

pub fn run(cmd: &Cmd, ctx: &Ctx) -> Result<Report, Fail> {
    match cmd {
        Cmd::CkFromLog { log, out } => ck_from_log(log, out.as_deref(), ctx),
    }
}

fn name(header: &str) -> Result<String, Fail> {
    let mut b = BTreeMap::new();
    for kv in crate::py::split(header) {
        if let Some((k, v)) = kv.split_once('=') {
            b.insert(k, v);
        }
    }
    let get = |k: &str| {
        b.get(k)
            .copied()
            .ok_or_else(|| Fail::error(format!("ck-from-log: no `{k}` in the header: {header}")))
    };
    Ok(format!(
        "{}x{}u{}r{}",
        get("pids")?,
        get("events")?,
        get("undos")?,
        get("reverts")?
    ))
}

fn ck_from_log(
    log: &std::path::Path,
    out: Option<&std::path::Path>,
    ctx: &Ctx,
) -> Result<Report, Fail> {
    let lp = ctx.path(log);
    let bytes = std::fs::read(&lp).map_err(|e| Fail::io(&lp, e))?;
    let text = crate::py::read_text_lossy(&bytes);
    let dir = ctx.path(out.unwrap_or(std::path::Path::new(".")));
    let ckpt = Regex::new(r"^ckpt seed \d+ states \d+ max_depth \d+$").expect("a fixed pattern");
    // runs in first-seen order; a header seen again starts its run over in the same place
    let mut files: Vec<(String, Vec<String>)> = Vec::new();
    let mut cur: Option<usize> = None;
    for raw in text.split_inclusive('\n') {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        if line.starts_with("undo_enum dfs ") {
            let n = name(line)?;
            let i = match files.iter().position(|(k, _)| *k == n) {
                Some(i) => {
                    files[i].1 = vec![line.to_string()];
                    i
                }
                None => {
                    files.push((n, vec![line.to_string()]));
                    files.len() - 1
                }
            };
            cur = Some(i);
        } else if let Some(i) = cur {
            if ckpt.is_match(line) {
                let w: Vec<&str> = line.split(' ').collect();
                files[i].1.push(format!(
                    "seed {} states {} max_depth {} viol -",
                    w[2], w[4], w[6]
                ));
            }
        }
    }
    let mut report = String::new();
    let mut written = Vec::new();
    for (n, lines) in &files {
        let seeds: Vec<&str> = lines[1..].iter().map(|l| crate::py::split(l)[1]).collect();
        let mut seen = std::collections::BTreeSet::new();
        if !seeds.iter().all(|s| seen.insert(*s)) {
            // the files before this run are written and reported; this one and those after aren't
            let mut r = Report::new(
                ERROR,
                report,
                json!({ "files": written, "error": "a seed twice", "run": n }),
            );
            r.notices.push(format!("ck-from-log: a seed twice in {n}"));
            return Ok(r);
        }
        let body = lines.join("\n") + "\n";
        let file = format!("ck-{n}.txt");
        let path = dir.join(&file);
        std::fs::write(&path, &body).map_err(|e| Fail::io(&path, e))?;
        let sha = Sha256::digest(body.as_bytes());
        let hex: String = sha.iter().map(|b| format!("{b:02x}")).collect();
        report += &format!("{file}: {} seeds, sha256 {hex}\n", seeds.len());
        written.push(json!({ "file": file, "seeds": seeds.len(), "sha256": hex }));
    }
    Ok(Report::new(OK, report, json!({ "files": written })))
}
