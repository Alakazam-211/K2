//! `ci evidence`, `ci provenance` and `ci logs` (`log`), as `cmd_ci`
//! answers them (`cli/src/ci.sh`), with the writers the first two share
//! (`_CI_OUT_PY` in `cli/src/compute-provenance.sh`: its `bundle`,
//! `statements` and `trust` modes; its `check` mode and its `put` are
//! [`super::outdir`]).
//!
//! - `evidence <run> --out <dir> [--no-files] [--reported <file>]
//!   [--json]`: the run's evidence bundle written to `<dir>`, each build
//!   statement under `<dir>/provenance/`, the files that check them under
//!   `<dir>/trust/`, then each job's kept files fetched into
//!   `<dir>/<label>/` as `compute fetch` does (its lines dropped, as the
//!   shell's `>/dev/null` drops them). `--reported` carries one named file
//!   of each job in the bundle.
//! - `provenance <run> --out <dir> [--json]`: the statements and `trust/`
//!   only.
//! - `logs <run|commit|branch> [--check NAME] [--failures]`: a heading
//!   per check of the run, then that job's log as `compute logs` prints
//!   it (its failures alone with `--failures`). A failing log never stops
//!   the walk (the shell's `|| true`).
//!
//! Where the shell's Python raises, this prints its one plain line
//! ([`py::plain`], never a traceback) and ends with 1, as the shell's
//! `|| exit 1` (or `set -e`) does. Every name the daemon's answer puts under `--out` is checked
//! before anything is written ([`super::outdir::check`]): one that is
//! absolute, has a `..` part, holds a NUL or a newline, goes through a
//! link or lands outside `--out` is refused with exit 3 and nothing is
//! written. Each file is then made with no link followed
//! ([`super::outdir::put`]).

use super::{call, outdir, py, Ask};
use crate::exit::Exit;
use crate::families::compute::logs;
use crate::families::compute::pyrt::{self as p, s};
use crate::families::compute::reads::Stop;
use crate::families::compute::writes::{dumps, fetch, object, print_body, str_value, Cp, Words};
use crate::families::{Raise, Shown};
use crate::out;
use crate::out::json::{self, Options, Text, Value};
use crate::out::pylib::json as pyjson;
use k2_client::brand::CLI_NAME as CLI;
use std::io::Write;

type R<T> = Result<T, Raise>;

fn at<T>(line: u32, r: p::R<T>) -> R<T> {
    r.map_err(|e| Raise::at(line, e))
}

fn empty() -> Value {
    Value::Object(Vec::new())
}

/// A dict key as Python's `str`.
fn key(k: &Text) -> String {
    p::str_of(&Value::Str(k.clone()))
}

/// `json.load(sys.stdin)` at `line`.
fn load(body: &[u8], line: u32) -> R<Value> {
    pyjson::loads(&String::from_utf8_lossy(body)).map_err(|f| Raise::at(line, f.last_line()))
}

/// `d.items()`.
fn items(v: &Value, line: u32) -> R<Vec<(Text, Value)>> {
    match v {
        Value::Object(pairs) => Ok(pairs.clone()),
        other => Err(Raise::no_attribute(line, other, "items")),
    }
}

/// `sorted(d.items())`: by key (keys never tie).
fn sorted_items(v: &Value, line: u32) -> R<Vec<(Text, Value)>> {
    let mut pairs = items(v, line)?;
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(pairs)
}

/// `d[k] = v` on a dict this code builds: a key already there keeps its
/// place.
fn put_key(pairs: &mut Vec<(Text, Value)>, k: Text, v: Value) {
    match pairs.iter_mut().find(|(have, _)| *have == k) {
        Some(slot) => slot.1 = v,
        None => pairs.push((k, v)),
    }
}

/// `os.path.join(a, b)`.
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// `os.path.basename(p)`.
fn basename(path: &str) -> &str {
    path.rfind('/').map_or(path, |i| &path[i + 1..])
}

/// `open(path, "w")`.
fn open_w(path: &str) -> Result<std::fs::File, String> {
    if path.contains('\0') {
        return Err("ValueError: embedded null byte".into());
    }
    std::fs::File::create(path).map_err(|e| pyjson::os_error(&e, path).last_line())
}

/// `f.write(text)`: a string only.
#[cfg(unix)]
pub(super) fn write_str(f: &mut std::fs::File, text: &Value, path: &str) -> Result<(), String> {
    let Value::Str(_) = text else {
        return Err(format!(
            "TypeError: write() argument must be str, not {}",
            p::tname(text)
        ));
    };
    f.write_all(p::str_of(text).as_bytes())
        .map_err(|e| pyjson::os_error(&e, path).last_line())
}

/// `json.dumps(v, ...)` with these options.
fn dumps_with(v: &Value, indent: Option<usize>, sort_keys: bool) -> p::R<String> {
    let o = Options {
        indent,
        sort_keys,
        ..Options::default()
    };
    json::to_string(v, &o).map_err(|e| format!("ValueError: {e:?}"))
}

/// `_compute_write_statements DIR` (`_CI_OUT_PY statements`) on the
/// answer: each statement as `DIR/provenance/<label>.intoto.jsonl`; how
/// many.
pub fn write_statements(body: &[u8], dir: &str) -> R<usize> {
    let v = load(body, 4)?;
    let st = p::or(at(121, p::get(&v, "statements"))?, empty());
    let mut lines: Vec<(Text, Value)> = Vec::new();
    for (k, one) in items(&st, 121)? {
        let line = at(121, p::idx(&one, "line"))?;
        put_key(&mut lines, k, line);
    }
    let bundle = p::or(at(122, p::get(&v, "bundle"))?, empty());
    let prov = p::or(at(122, p::get(&bundle, "provenance"))?, empty());
    for (k, env) in items(&prov, 122)? {
        let text = at(123, p::dumps(&env, true, None))?;
        put_key(&mut lines, k, s(&format!("{text}\n")));
    }
    lines.sort_by(|a, b| a.0.cmp(&b.0));
    for (k, line) in &lines {
        let rel = format!("provenance/{}.intoto.jsonl", key(k));
        outdir::put(dir, &rel, line, 1).map_err(|f| f.raise(125))?;
    }
    Ok(lines.len())
}

/// `_compute_write_trust DIR` (`_CI_OUT_PY trust`) on the answer: the
/// files under `DIR/trust/` that check a statement with no K2, each
/// worked out first, then written; how many.
pub fn write_trust(body: &[u8], dir: &str) -> R<usize> {
    let v = load(body, 4)?;
    let t = p::or(at(128, p::get(&v, "trust"))?, empty());
    let mut files: Vec<(String, Value)> = Vec::new();
    let keys = p::or(at(130, p::get(&t, "nodeKeys"))?, empty());
    for (fp, k) in sorted_items(&keys, 130)? {
        let pem = at(131, p::idx(&k, "pem"))?;
        files.push((format!("nodes/{}.pem", key(&fp)), pem));
    }
    let statements = p::or(at(132, p::get(&t, "statements"))?, Value::Array(vec![]));
    for one in at(132, p::iter(&statements))? {
        let b = at(133, p::idx(&one, "body"))?;
        let Value::Str(_) = &b else {
            return Err(Raise::no_attribute(133, &b, "encode"));
        };
        let sum = crate::families::fed::pubkey::sha256(p::str_of(&b).as_bytes());
        let hex: String = sum.iter().map(|x| format!("{x:02x}")).collect();
        let text = at(133, dumps_with(&one, None, true))?;
        files.push((format!("statements/{hex}.json"), s(&format!("{text}\n"))));
    }
    let owners = p::or(at(134, p::get(&t, "owners"))?, Value::Array(vec![]));
    let text = at(135, dumps_with(&owners, Some(1), true))?;
    files.push(("owners.json".to_string(), s(&format!("{text}\n"))));
    let rosters = p::or(at(136, p::get(&t, "rosters"))?, Value::Array(vec![]));
    let os_ = at(136, p::iter(&owners))?;
    let rs = at(136, p::iter(&rosters))?;
    for (o, r) in os_.iter().zip(rs.iter()) {
        let fp = at(137, p::idx(o, "rosterKeyFp"))?;
        let name = at(137, p::add(&s("rosters/"), &fp))?;
        let name = at(137, p::add(&name, &s(".json")))?;
        let text = at(137, dumps_with(r, None, true))?;
        files.push((p::str_of(&name), s(&format!("{text}\n"))));
    }
    for (rel, text) in &files {
        outdir::put(dir, &format!("trust/{rel}"), text, -1).map_err(|f| f.raise(139))?;
    }
    Ok(files.len())
}

/// `{"run": run}` and the workspace when there is one.
fn run_body(run: &str, ws: &str) -> String {
    let mut o = vec![("run", str_value(run))];
    if !ws.is_empty() {
        o.push(("workspace", str_value(ws)));
    }
    dumps(&object(o))
}

fn usage(line: String) -> Exit {
    out::stderr_line(&line);
    Exit::Usage
}

/// A reader that raised before it printed anything: its plain line, exit 1.
pub(super) fn fail(r: &Raise) -> Exit {
    py::unreadable(String::new(), r).print()
}

/// `mkdir -p "$out_dir" || { echo "can't make $out_dir" >&2; exit 1; }`
/// after the call ([`super::run`] hands a folder `mkdir -p` refuses to
/// the shell first; this is the race after that check).
fn made(dir: &str) -> bool {
    if std::fs::create_dir_all(dir).is_ok() {
        return true;
    }
    out::stderr_line(&format!("can't make {dir}"));
    false
}

/// What `IFS=$'\t' read -r a b` reads of each line of `text` as a
/// here-string or a pipe hands it on: whole lines only.
pub fn tab_fields(text: &str) -> Vec<(String, String)> {
    let mut v = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find('\n') {
        let line = rest[..i].trim_matches('\t');
        rest = &rest[i + 1..];
        let (a, b) = match line.find('\t') {
            Some(j) => (&line[..j], line[j..].trim_start_matches('\t')),
            None => (line, ""),
        };
        v.push((a.to_string(), b.to_string()));
    }
    v
}

/// `ci provenance`.
pub fn provenance(cp: &mut Cp<'_>, a: &Ask, ws: &str) -> Exit {
    let run = a.pos.first().map_or("", String::as_str);
    if run.is_empty() || a.out_dir.is_empty() {
        return usage(format!(
            "usage: {CLI} ci provenance <run> --out <dir> [--json]"
        ));
    }
    let body = match call(cp, true, "provenance", &run_body(run, ws)) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if let Err(e) = outdir::check(&body, "provenance", &a.out_dir) {
        return e;
    }
    if !made(&a.out_dir) {
        return Exit::Error;
    }
    let n_st = match write_statements(&body, &a.out_dir) {
        Ok(n) => n,
        Err(r) => return fail(&r),
    };
    let n_trust = match write_trust(&body, &a.out_dir) {
        Ok(n) => n,
        Err(r) => return fail(&r),
    };
    if a.json {
        return print_body(&body);
    }
    let mut o = String::new();
    let r = provenance_lines(&body, &a.out_dir, n_st, n_trust, &mut o);
    match r {
        Ok(()) => Shown::out(o).print(),
        Err(e) => py::unreadable(o, &e).print(),
    }
}

fn pl(o: &mut String, line: &str) {
    o.push_str(line);
    o.push('\n');
}

fn provenance_lines(body: &[u8], d: &str, n_st: usize, n_trust: usize, o: &mut String) -> R<()> {
    let v = load(body, 3)?;
    let short = at(5, p::idx(&v, "short"))?;
    let t = at(
        5,
        p::pct(
            "build statements of run %s: %s in %s/provenance/; %s file(s) to check them in %s/trust/",
            &[short, s(&n_st.to_string()), s(d), s(&n_trust.to_string()), s(d)],
        ),
    )?;
    pl(o, &t);
    let st = p::or(at(6, p::get(&v, "statements"))?, empty());
    for (name, one) in sorted_items(&st, 6)? {
        let short = at(7, p::idx(&one, "short"))?;
        let files = at(7, p::idx(&one, "files"))?;
        let name = Value::Str(name);
        let t = at(
            7,
            p::pct(
                "  %-16s job %s, %d file(s) -> provenance/%s.intoto.jsonl",
                &[name.clone(), short, files, name],
            ),
        )?;
        pl(o, &t);
    }
    let missing = p::or(at(8, p::get(&v, "missing"))?, empty());
    for (name, why) in sorted_items(&missing, 8)? {
        let t = at(
            9,
            p::pct("  %-16s no statement: %s", &[Value::Str(name), why]),
        )?;
        pl(o, &t);
    }
    pl(
        o,
        &format!(
            "check one: {CLI} compute verify {d}/provenance/<check>.intoto.jsonl --file <its file>"
        ),
    );
    Ok(())
}

/// The bundle's writer (`_CI_OUT_PY bundle`): `<dir>/<file>`, what it
/// prints.
fn bundle_file(body: &[u8], dir: &str) -> R<String> {
    let v = load(body, 4)?;
    let f = at(117, p::idx(&v, "file"))?;
    let Value::Str(_) = &f else {
        return Err(Raise::at(
            117,
            format!(
                "TypeError: join() argument must be str, bytes, or os.PathLike object, not '{}'",
                p::tname(&f)
            ),
        ));
    };
    let name = p::str_of(&f);
    let path = join(dir, &name);
    let bundle = at(118, p::idx(&v, "bundle"))?;
    let text = at(118, dumps_with(&bundle, Some(1), true))?;
    outdir::put(dir, &name, &s(&format!("{text}\n")), 0).map_err(|e| e.raise(118))?;
    Ok(path)
}

/// The jobs' reader (`< <(python3 …)`): its lines, and what it raised if
/// it did (the loop still reads what it printed).
fn job_lines(body: &[u8]) -> (String, Option<Raise>) {
    let mut o = String::new();
    let r = (|| -> R<()> {
        let v = load(body, 3)?;
        let jobs = at(3, p::idx(&v, "jobs"))?;
        for (label, j) in sorted_items(&jobs, 3)? {
            let job = at(4, p::idx(&j, "job"))?;
            let t = at(4, p::pct("%s\t%s", &[Value::Str(label), job]))?;
            pl(&mut o, &t);
        }
        Ok(())
    })();
    (o, r.err())
}

/// `( cmd_compute_fetch <job> --out <dir> >/dev/null )`: whether it
/// worked. Its stderr stays; its notes count for the EXIT trap.
fn fetch_quiet(cp: &mut Cp<'_>, job: &str, dir: &str) -> bool {
    match fetch::parse(&[job, "--out", dir]) {
        Words::Ask(a) => quiet_stdout(|| fetch::send(cp, &a)) == Exit::Ok,
        Words::Usage(line) => {
            out::stderr_line(&line);
            false
        }
        Words::ShiftFailed(_) => false,
    }
}

/// Runs `f` with stdout on `/dev/null`, as `>/dev/null` does.
#[cfg(unix)]
fn quiet_stdout<T>(f: impl FnOnce() -> T) -> T {
    use std::os::fd::AsRawFd;
    let _ = std::io::stdout().flush();
    let null = std::fs::OpenOptions::new().write(true).open("/dev/null");
    // SAFETY: plain dup and dup2 on fd 1 and a file this function owns.
    let saved = unsafe { libc::dup(1) };
    let swapped = match &null {
        Ok(n) if saved >= 0 => {
            // SAFETY: as above.
            let rc = unsafe { libc::dup2(n.as_raw_fd(), 1) };
            rc >= 0
        }
        _ => false,
    };
    let r = f();
    let _ = std::io::stdout().flush();
    if swapped {
        // SAFETY: `saved` is the duplicate of fd 1 made above.
        unsafe { libc::dup2(saved, 1) };
    }
    if saved >= 0 {
        // SAFETY: closes only the duplicate made above.
        unsafe { libc::close(saved) };
    }
    r
}

/// Elsewhere [`super::run`] hands a fetching `evidence` to the shell.
#[cfg(not(unix))]
fn quiet_stdout<T>(f: impl FnOnce() -> T) -> T {
    f()
}

/// `sorted(x)` of what a bundle's `receipts` holds.
fn sorted_labels(v: &Value) -> Result<Vec<Value>, String> {
    let mut items = p::iter(v)?;
    if items.iter().all(|x| matches!(x, Value::Str(_))) {
        items.sort_by(|a, b| match (a, b) {
            (Value::Str(x), Value::Str(y)) => x.cmp(y),
            _ => std::cmp::Ordering::Equal,
        });
        return Ok(items);
    }
    if items.iter().all(|x| p::num(x).is_some()) {
        let mut bad = Ok(());
        items.sort_by(|a, b| match p::cmp("<", a, b) {
            Ok(true) => std::cmp::Ordering::Less,
            Ok(false) => match p::cmp("<", b, a) {
                Ok(true) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            },
            Err(e) => {
                bad = Err(e);
                std::cmp::Ordering::Equal
            }
        });
        bad?;
        return Ok(items);
    }
    let (x, y) = (&items[0], &items[1.min(items.len() - 1)]);
    Err(format!(
        "TypeError: '<' not supported between instances of '{}' and '{}'",
        p::tname(y),
        p::tname(x)
    ))
}

/// The `--reported` script: each job's named file into the bundle. `Err`:
/// what it printed on stderr before `exit 1`.
fn reported(path: &str, out_dir: &str, name: &str) -> Result<String, String> {
    if name.starts_with('/') || name.split('/').any(|x| x == "..") {
        return Err(format!(
            "--reported {name}: a path inside each job's files (dist/build-info.json)\n"
        ));
    }
    let text = pyjson::read_text_file(path).map_err(|f| py::plain(&f.last_line()))?;
    let mut b = pyjson::loads(&text).map_err(|f| py::plain(&f.last_line()))?;
    let receipts = p::idx(&b, "receipts").map_err(|e| py::plain(&e))?;
    let labels = sorted_labels(&receipts).map_err(|e| py::plain(&e))?;
    let mut added: Vec<String> = Vec::new();
    for label in labels {
        let Value::Str(_) = &label else {
            let e = format!(
                "TypeError: join() argument must be str, bytes, or os.PathLike object, not '{}'",
                p::tname(&label)
            );
            return Err(py::plain(&e));
        };
        let label = p::str_of(&label);
        let f = join(&join(out_dir, &label), name);
        if f.contains('\0') || !std::path::Path::new(&f).is_file() {
            continue;
        }
        let data =
            std::fs::read(&f).map_err(|e| py::plain(&pyjson::os_error(&e, &f).last_line()))?;
        let enc = crate::families::store::widget::b64encode(&data);
        set_reported(&mut b, &label, name, enc).map_err(|e| py::plain(&e))?;
        added.push(label);
    }
    if added.is_empty() {
        return Err(format!(
            "--reported {name}: no job of the run wrote that file\n"
        ));
    }
    let mut fh = open_w(path).map_err(|e| py::plain(&e))?;
    let text = dumps_with(&b, Some(1), true).map_err(|e| py::plain(&e))?;
    fh.write_all(format!("{text}\n").as_bytes())
        .map_err(|e| py::plain(&pyjson::os_error(&e, path).last_line()))?;
    Ok(format!(
        "reported {name} of {} carried in the bundle\n",
        added.join(", ")
    ))
}

/// `b.setdefault("reported", {}).setdefault(label, {})[name] = enc`.
fn set_reported(b: &mut Value, label: &str, name: &str, enc: String) -> Result<(), String> {
    let no_setdefault = |v: &Value| {
        format!(
            "AttributeError: '{}' object has no attribute 'setdefault'",
            p::tname(v)
        )
    };
    let pairs = match b {
        Value::Object(pairs) => pairs,
        other => return Err(no_setdefault(other)),
    };
    let rep = match slot(pairs, "reported") {
        Value::Object(rep) => rep,
        other => return Err(no_setdefault(other)),
    };
    match slot(rep, label) {
        Value::Object(files) => {
            put_key(files, Text::from(name), s(&enc));
            Ok(())
        }
        Value::Array(_) => {
            Err("TypeError: list indices must be integers or slices, not str".into())
        }
        other => Err(format!(
            "TypeError: '{}' object does not support item assignment",
            p::tname(other)
        )),
    }
}

/// `d.setdefault(k, {})`.
fn slot<'a>(pairs: &'a mut Vec<(Text, Value)>, k: &str) -> &'a mut Value {
    let k = Text::from(k);
    let i = match pairs.iter().position(|(have, _)| *have == k) {
        Some(i) => i,
        None => {
            pairs.push((k, empty()));
            pairs.len() - 1
        }
    };
    &mut pairs[i].1
}

/// `ci evidence`.
pub fn evidence(cp: &mut Cp<'_>, a: &Ask, ws: &str) -> Exit {
    let run = a.pos.first().map_or("", String::as_str);
    if run.is_empty() || a.out_dir.is_empty() {
        return usage(format!(
            "usage: {CLI} ci evidence <run> --out <dir> [--no-files] [--reported <file>] [--json]"
        ));
    }
    if !a.reported.is_empty() && a.no_files {
        return usage("usage: --reported reads the fetched files: drop --no-files".into());
    }
    let body = match call(cp, true, "evidence", &run_body(run, ws)) {
        Ok(b) => b,
        Err(e) => return e,
    };
    // Every name the answer puts under --out, before anything is written.
    if let Err(e) = outdir::check(&body, "evidence", &a.out_dir) {
        return e;
    }
    if !made(&a.out_dir) {
        return Exit::Error;
    }
    let mut file = match bundle_file(&body, &a.out_dir) {
        Ok(p) => p,
        Err(r) => return fail(&r),
    };
    // `file="$(…)"` drops the trailing newlines.
    while file.ends_with('\n') {
        file.pop();
    }
    let n_st = match write_statements(&body, &a.out_dir) {
        Ok(n) => n,
        Err(r) => return fail(&r),
    };
    let n_trust = match write_trust(&body, &a.out_dir) {
        Ok(n) => n,
        Err(r) => return fail(&r),
    };
    let mut fetched = 0usize;
    if !a.no_files {
        let (lines, raise) = job_lines(&body);
        if let Some(r) = raise {
            let _ = out::write_flushed(
                &mut std::io::stderr().lock(),
                py::plain(&r.exception).as_bytes(),
            );
        }
        for (label, job) in tab_fields(&lines) {
            if job.is_empty() {
                continue;
            }
            let dir = format!("{}/{label}", a.out_dir);
            if !fetch_quiet(cp, &job, &dir) {
                out::stderr_line(&format!(
                    "fetching job {job}'s files failed: {CLI} compute fetch {job} --out {dir}"
                ));
                return Exit::Error;
            }
            fetched += 1;
        }
    }
    if !a.reported.is_empty() {
        match reported(&file, &a.out_dir, &a.reported) {
            Ok(note) => {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), note.as_bytes());
            }
            Err(text) => {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
                return Exit::Error;
            }
        }
    }
    if a.json {
        return evidence_json(&body, &file);
    }
    let mut o = String::new();
    let r = evidence_lines(&body, &file, &a.out_dir, fetched, n_st, n_trust, &mut o);
    match r {
        Ok(()) => Shown::out(o).print(),
        Err(e) => py::unreadable(o, &e).print(),
    }
}

/// `v["path"] = file; v.pop("bundle", None); print(json.dumps(v))`.
fn evidence_json(body: &[u8], file: &str) -> Exit {
    let mut v = match load(body, 1) {
        Ok(v) => v,
        Err(r) => return fail(&r),
    };
    if let Value::Object(pairs) = &mut v {
        put_key(pairs, Text::from("path"), str_value(file));
        pairs.retain(|(k, _)| *k != Text::from("bundle"));
    }
    Shown::out(format!("{}\n", dumps(&v))).print()
}

fn evidence_lines(
    body: &[u8],
    path: &str,
    dir: &str,
    fetched: usize,
    n_st: usize,
    n_trust: usize,
    o: &mut String,
) -> R<()> {
    let v = load(body, 3)?;
    let project = at(4, p::get(&v, "project"))?;
    let version = at(4, p::get(&v, "version"))?;
    let short = at(4, p::idx(&v, "short"))?;
    let t = at(
        4,
        p::pct(
            "evidence for %s %s (run %s): %s",
            &[project.clone(), version.clone(), short.clone(), s(path)],
        ),
    )?;
    pl(o, &t);
    pl(
        o,
        &format!("  build statements: {n_st} in {dir}/provenance/; {n_trust} file(s) to check them in {dir}/trust/"),
    );
    let jobs = at(6, p::idx(&v, "jobs"))?;
    for (label, j) in sorted_items(&jobs, 6)? {
        let mut args = vec![Value::Str(label.clone())];
        for k in ["role", "build", "short", "node"] {
            args.push(at(7, p::idx(&j, k))?);
        }
        let arts = at(7, p::idx(&j, "artifacts"))?;
        args.push(p::int(at(7, p::len(&arts))? as i128));
        args.push(s(&if fetched != 0 {
            format!(" -> {dir}/{}/", key(&label))
        } else {
            String::new()
        }));
        let t = at(
            7,
            p::pct("  %-4s %-6s build %-16s job %s on %s, %d file(s)%s", &args),
        )?;
        pl(o, &t);
    }
    let dual = p::or(at(9, p::get(&v, "dual"))?, empty());
    for (build, d) in sorted_items(&dual, 9)? {
        let c = at(10, p::idx(&d, "compare"))?;
        let b = at(11, p::idx(&d, "build"))?;
        let verify = at(11, p::idx(&d, "verify"))?;
        let matched = at(11, p::idx(&c, "matched"))?;
        let n = p::int(at(11, p::len(&matched))? as i128);
        let mut tail = Vec::new();
        for (k, lead) in [("differ", ", DIFFER: "), ("only_one", ", in one only: ")] {
            let x = at(12, p::idx(&c, k))?;
            tail.push(s(&if p::truthy(&x) {
                format!("{lead}{}", at(12, p::join(", ", &x))?)
            } else {
                String::new()
            }));
        }
        let mut args = vec![Value::Str(build), b, verify, n];
        args.extend(tail);
        let t = at(
            11,
            p::pct(
                "  dual %s: %s = %s on %d file(s)%s%s (checked here only; the signer decides)",
                &args,
            ),
        )?;
        pl(o, &t);
    }
    let notes = p::or(at(13, p::get(&v, "notes"))?, Value::Array(vec![]));
    for n in at(13, p::iter(&notes))? {
        let line = at(14, p::add(&s("  note: "), &n))?;
        pl(o, &p::str_of(&line));
    }
    let ev = basename(path);
    pl(
        o,
        &format!("next, at the signing machine (a person; {CLI} study ci-release):"),
    );
    let t = at(
        17,
        p::pct(
            "  1. sudo -n -u _k2sign /usr/local/libexec/k2-signer/k2-signer rehearse %s < %s",
            &[project.clone(), s(ev)],
        ),
    )?;
    pl(o, &t);
    let t = at(
        18,
        p::pct(
            "  2. the same with: evidence %s %s < %s, then challenge, approve and sign each file",
            &[project, version.clone(), s(ev)],
        ),
    )?;
    pl(o, &t);
    let short = at(19, p::idx(&v, "short"))?;
    let fmt = format!("  3. here: {CLI} app updates push <app> --channel beta --version %s --artifact <platform>=<file> --sig <platform>=<file>.sig --evidence %s");
    let t = at(19, p::pct(&fmt, &[version, short]))?;
    pl(o, &t);
    Ok(())
}

/// The checks' reader of `ci logs`: its lines (`<job>\t<heading>`).
fn check_lines(body: &[u8], check: &str) -> R<String> {
    let mut o = String::new();
    let v = load(body, 3)?;
    let r = at(3, p::idx(&v, "run"))?;
    let want: Vec<&str> = check.split(',').filter(|c| !c.is_empty()).collect();
    let checks = p::or(at(5, p::get(&r, "checks"))?, Value::Array(vec![]));
    for c in at(5, p::iter(&checks))? {
        if !want.is_empty() {
            let name = at(6, p::idx(&c, "name"))?;
            let wanted =
                matches!(&name, Value::Str(_)) && want.contains(&p::str_of(&name).as_str());
            if !wanted {
                continue;
            }
        }
        let job = p::or(at(8, p::get(&c, "job"))?, s("-"));
        let name = at(8, p::idx(&c, "name"))?;
        let short = super::py::head(&job, 8, 8)?;
        let state = at(8, p::idx(&c, "state"))?;
        let desc = p::or(at(8, p::get(&c, "description"))?, s(""));
        let t = at(
            8,
            p::pct(
                "%s\t%s · job %s · %s · %s",
                &[job, name, short, state, desc],
            ),
        )?;
        pl(&mut o, &t);
    }
    Ok(o)
}

/// `ci logs` (`log`).
pub fn logs(cp: &mut Cp<'_>, a: &Ask, ws: &str) -> Exit {
    let p0 = a.pos.first().map_or("", String::as_str);
    if p0.is_empty() {
        return usage(format!(
            "usage: {CLI} ci logs <run|commit|branch> [--check NAME] [--failures]"
        ));
    }
    let pairs: Vec<(&str, &str)> = [("workspace", ws), ("run", p0)]
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .collect();
    let q = crate::transport::urllib::urlencode(&pairs);
    let body = match call(cp, false, "status", &q) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let mut lines = match check_lines(&body, &a.check) {
        Ok(l) => l,
        Err(r) => return fail(&r),
    };
    while lines.ends_with('\n') {
        lines.pop();
    }
    if lines.is_empty() {
        out::stderr_line(&format!(
            "ci_check_not_found: no such check in that run ({CLI} ci status {p0})"
        ));
        return Exit::NotFound;
    }
    lines.push('\n');
    for (job, head) in tab_fields(&lines) {
        if let Err(e) = out::stdout(&format!("── {head} ──\n")) {
            return out::after_stdout_error(&e);
        }
        if job == "-" {
            if let Err(e) = out::stdout("(never submitted: no log)\n") {
                return out::after_stdout_error(&e);
            }
            continue;
        }
        job_log(cp, &job, a.failures);
    }
    Exit::Ok
}

/// `( cmd_compute_logs <job> [--failures] ) || true`.
fn job_log(cp: &mut Cp<'_>, job: &str, failures: bool) {
    let mut words = vec![job];
    if failures {
        words.push("--failures");
    }
    match logs::parse(&words) {
        Some(Ok(l)) => {
            let _ = logs::run(cp.client, &mut cp.closing, &l);
        }
        Some(Err(Stop::Usage(line))) => out::stderr_line(&line),
        Some(Err(Stop::Shift(flag))) => out::stderr_line(&out::need_value(&flag)),
        None => match logs::parse_search(&words) {
            Ok(a) => {
                let w = if a.ws.is_empty() {
                    cp.ws("")
                } else {
                    a.ws.clone()
                };
                let q = logs::search_query(&a, &w);
                if let Ok(b) = cp.call(false, "/cli/compute/logs/search", &q, false) {
                    let _ = if a.json {
                        print_body(&b)
                    } else {
                        logs::search_lines(&b, p::now()).print()
                    };
                }
            }
            Err(Stop::Usage(line)) => out::stderr_line(&line),
            Err(Stop::Shift(flag)) => out::stderr_line(&out::need_value(&flag)),
        },
    }
}
