//! `zen widget preview --dir <folder> --shot <png>` (`_zen_widget_shot`,
//! `cli/src/zen-widget-dev.sh`): the daemon answers the shot page (`POST
//! /cli/zen/widget/shot`, which writes nothing), and the shot runner
//! (`k2-widget-shot.mjs`, run by Node with Playwright's WebKit) renders it
//! and writes the PNG.
//!
//! The runner stays an outside program, as git does (K2's ruling D8): the
//! binary finds it where the shell does, beside the program people ran
//! ([`crate::fallthrough::script_dir_for`], the folder the front hands
//! the shell in `K5_CLI_SCRIPT_DIR`), and runs it with the `node` on
//! `PATH`, with the page in a temp file of its own (`--page`), never in
//! argv or the environment. The rest is the binary's: the body (with
//! `--data`'s fixture and its `blobs/` folder, each file's type from
//! Python's `mimetypes.guess_type`, [`crate::out::pylib::mimetypes`]),
//! the request, the check of the answer and the runner's report.
//!
//! Still the shell's ([`shells`]): no runner beside the program (the
//! shell then looks beside its own file, `/dev/fd/N`, and names a path
//! with its Python's process id), no `PATH` at all, a `blobs/` folder
//! with a name that is not UTF-8, or a system `mime.types` Python can't
//! read (it raises inside `mimetypes`).
//!
//! Named difference (for K2): bash hands the body's extra fields to its
//! Python in an environment variable (`ZD_EXTRA`), and the runner's
//! report in another (`ZO`). A `--data` fixture whose JSON (blobs and
//! all) is above what one variable may hold (128 KiB on Linux) stops bash
//! there ("Argument list too long", exit 126) before any request; the
//! binary sends it. That is `zen-large-answer`'s cause, on the way out.

use super::call::Zen;
use super::print::{self, blank, head200, indented, jstr, or, s, Out};
use super::verbs::{dir_body, usage, NameKey};
use super::widget::Step;
use crate::exit::Exit;
use crate::families::{py_get, py_iter, Raise, Shown};
use crate::out::json::{Text, Value};
use crate::out::py;
use crate::out::pylib::json::{decode_utf8, loads, os_error, read_text_file};
use crate::out::pylib::{mimetypes, ops, path as pypath};
use k2_client::{cli_name, product_name};
use std::path::{Path, PathBuf};

/// The runner's file name, beside the program.
pub const RUNNER: &str = "k2-widget-shot.mjs";

/// `--shot` and the flags that go with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flags {
    pub png: String,
    pub theme: String,
    pub zen_theme: String,
    pub width: String,
    pub height: String,
    pub data: String,
    pub libs: String,
    pub wait_ms: String,
}

impl Flags {
    /// `[ -n "$shot" ] || [ -n "$theme$zen_theme…$wait_ms" ]`.
    pub fn any(&self) -> bool {
        [
            &self.png,
            &self.theme,
            &self.zen_theme,
            &self.width,
            &self.height,
            &self.data,
            &self.libs,
            &self.wait_ms,
        ]
        .iter()
        .any(|v| !v.is_empty())
    }
}

/// A `preview` line with `--shot` or a flag that goes with it.
pub struct Shot<'a> {
    pub as_json: bool,
    pub clear: bool,
    pub dir: &'a str,
    pub pos: &'a [&'a str],
    pub flags: &'a Flags,
    pub cfg: &'a [String],
}

impl Shot<'_> {
    /// `"${pos[0]:-}"`.
    fn name(&self) -> &str {
        self.pos.first().copied().unwrap_or("")
    }
}

/// The usage lines of a shot line, in the shell's order: the preview's
/// own, then `_zen_widget_shot`'s first checks. `None`: none of them.
pub fn stops(a: &Shot<'_>) -> Option<Shown> {
    let f = a.flags;
    if f.png.is_empty() {
        return Some(usage(
            "Error: --theme, --zen-theme, --width, --height, --data, --libs and --wait-ms go with --shot <png>",
        ));
    }
    if a.clear {
        return Some(usage("Error: --shot takes a staged folder, not --clear"));
    }
    if a.dir.is_empty() || a.pos.len() > 1 {
        return Some(usage(SHOT_USAGE));
    }
    if !matches!(f.theme.as_str(), "" | "dark" | "light") {
        return Some(usage("Error: --theme is dark or light"));
    }
    if !f.png.ends_with(".png") {
        return Some(usage(format!(
            "Error: --shot writes a .png file (got {})",
            f.png
        )));
    }
    for n in [&f.width, &f.height, &f.wait_ms] {
        if !n.is_empty() && !n.bytes().all(|b| b.is_ascii_digit()) {
            return Some(usage(
                "Error: --width, --height and --wait-ms are whole numbers",
            ));
        }
    }
    if !f.data.is_empty() && !Path::new(&f.data).is_file() {
        return Some(usage(format!("Error: no --data file {}", f.data)));
    }
    None
}

/// The runner beside the program people ran, if it is there: what the
/// shell finds first (`$_K5_SCRIPT_DIR/k2-widget-shot.mjs`).
pub fn runner() -> Option<PathBuf> {
    crate::fallthrough::resolved_exe()
        .and_then(|exe| crate::fallthrough::script_dir_for(&exe))
        .map(|d| d.join(RUNNER))
        .filter(|p| p.is_file())
}

/// The shot lines the binary leaves to the shell (nothing printed or
/// sent yet): see the module's notes. A line with a usage error is the
/// binary's (the shell stops there before it looks for anything).
pub fn shells(a: &Shot<'_>) -> bool {
    if stops(a).is_some() {
        return false;
    }
    let flags = a.flags;
    if runner().is_none() || std::env::var_os("PATH").is_none() {
        return true;
    }
    if flags.data.is_empty() || !Path::new(&flags.data).is_file() {
        return false;
    }
    if std::env::current_dir().is_err() {
        return true;
    }
    let bdir = blobs_dir(&flags.data);
    if !Path::new(&bdir).is_dir() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(&bdir) else {
        // os.listdir's error: the binary raises it as Python does.
        return false;
    };
    if entries
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_str().is_none())
    {
        return true;
    }
    mimetypes::system().is_err()
}

/// `os.path.join(os.path.dirname(os.path.abspath(data)), "blobs")`.
fn blobs_dir(data: &str) -> String {
    pypath::join(&pypath::split(&pypath::abspath(data)).0, "blobs")
}

/// `$(mktemp "${TMPDIR:-/tmp}/<prefix>XXXXXX")`'s folder.
fn tmpdir() -> String {
    std::env::var("TMPDIR")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/tmp".into())
}

fn stop(s: Shown) -> Step {
    Ok(s.print())
}

/// `_zen_widget_shot`.
pub fn shot(z: &mut Zen, a: &Shot<'_>) -> Step {
    if let Some(s) = stops(a) {
        return stop(s);
    }
    let f = a.flags;
    let Some(runner) = runner() else {
        // Gone since [`shells`] looked: where the binary looked.
        let looked = crate::fallthrough::resolved_exe()
            .and_then(|exe| crate::fallthrough::script_dir_for(&exe))
            .map_or_else(|| PathBuf::from(RUNNER), |d| d.join(RUNNER));
        return stop(Shown::err(
            format!(
                concat!(
                    "Error: this ",
                    product_name!(),
                    " has no shot runner ({})\n"
                ),
                looked.to_string_lossy()
            ),
            Exit::Refused,
        ));
    };
    let node = std::env::var_os("PATH").and_then(|p| crate::daemon::find_on_path("node", p));
    let Some(node) = node else {
        return stop(Shown::err(
            "Error: --shot needs Node.js and Playwright's WebKit: install Node, then npm install -g playwright && npx playwright install webkit\n",
            Exit::Refused,
        ));
    };
    let extra = match extra(a) {
        Ok(e) => e,
        Err(s) => return stop(s),
    };
    if let Some(s) = super::temp_fails(false, "k2-zen-shot-body.", super::NO_TEMP_FILE) {
        return stop(s);
    }
    let body = match dir_body(a.dir, extra, NameKey::Fallback) {
        Ok(b) => b,
        Err(s) => return stop(s),
    };
    let answer = z.post_file_bytes("/cli/zen/widget/shot", &body)?;
    if let Some(s) = checked(a.as_json, &answer) {
        return stop(s);
    }
    let page = match page_file(&answer) {
        Ok(p) => p,
        Err(_) => return stop(Shown::err("Error: can't make a temp file\n", Exit::Error)),
    };
    let ran = run_runner(&node, &runner, &page, f);
    let _ = std::fs::remove_file(&page);
    let (out, rc) = ran;
    let rc = Exit::from_curl(rc);
    if out.is_empty() {
        return Ok(rc);
    }
    if a.as_json {
        let mut line = out;
        line.push(b'\n');
        if let Err(e) = crate::out::write_flushed(&mut std::io::stdout().lock(), &line) {
            return Ok(crate::out::after_stdout_error(&e));
        }
        return Ok(rc);
    }
    let shown = report(&String::from_utf8_lossy(&out));
    let exit = shown.print();
    Ok(if exit == Exit::Ok { rc } else { exit })
}

/// Program lines of the shell's Python blocks, where they raise.
pub mod line {
    /// The extra fields' block (`extra=$(… python3 -c …)`).
    pub const WIDTH: u32 = 8;
    pub const HEIGHT: u32 = 9;
    pub const LISTDIR: u32 = 23;
    pub const READ_BLOB: u32 = 27;
    pub const GUESS: u32 = 29;
    /// The answer's check (`ZA=… python3 -c …`).
    pub const READ_ANSWER: u32 = 3;
    pub const ERROR: u32 = 10;
    pub const ERRORS: u32 = 19;
    pub const ERROR_LINE: u32 = 20;
    /// The runner's report (`ZO=… python3 -c …`).
    pub const LOAD: u32 = 3;
    pub const SHOT: u32 = 4;
    pub const READY: u32 = 5;
    pub const EACH_ERROR: u32 = 7;
    pub const REFUSALS: u32 = 9;
    pub const JOIN: u32 = 10;
}

/// `int(word)` of whole-number digits, as JSON prints it; Python's
/// `ValueError` above 4300 digits.
fn int_of(word: &str, at: u32) -> Result<Value, Raise> {
    if word.len() > crate::out::json::MAX_INT_DIGITS {
        return Err(Raise::at(
            at,
            format!(
                "ValueError: Exceeds the limit ({} digits) for integer string conversion: value has {} digits; use sys.set_int_max_str_digits() to increase the limit",
                crate::out::json::MAX_INT_DIGITS,
                word.len()
            ),
        ));
    }
    let digits = word.trim_start_matches('0');
    Ok(Value::Int(if digits.is_empty() {
        "0".into()
    } else {
        digits.into()
    }))
}

/// `d[key] = value` on a dict: a key already there keeps its place.
fn set(pairs: &mut Vec<(Text, Value)>, key: &str, value: Value) {
    let key = Text::from(key);
    match pairs.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = value,
        None => pairs.push((key, value)),
    }
}

/// The extra fields' block: `{"config": {...}}` with the widget, the
/// scheme, the Zen theme, the size and `--data`'s fixture (with its
/// `blobs/` folder), or the line it stops on (exit 2, or a traceback).
fn extra(a: &Shot<'_>) -> Result<Vec<(Text, Value)>, Shown> {
    let f = a.flags;
    let raise = |r: Raise| Out::default().raise(&r);
    let mut b: Vec<(Text, Value)> = vec![(Text::from("config"), Value::Object(Vec::new()))];
    if !a.name().is_empty() {
        b.push((Text::from("widget"), jstr(a.name())));
    }
    if !f.theme.is_empty() {
        b.push((Text::from("scheme"), jstr(&f.theme)));
    }
    if !f.zen_theme.is_empty() {
        b.push((Text::from("theme"), jstr(&f.zen_theme)));
    }
    if !f.width.is_empty() {
        b.push((
            Text::from("width"),
            int_of(&f.width, line::WIDTH).map_err(raise)?,
        ));
    }
    if !f.height.is_empty() {
        b.push((
            Text::from("height"),
            int_of(&f.height, line::HEIGHT).map_err(raise)?,
        ));
    }
    if !f.data.is_empty() {
        let read = read_text_file(&f.data).and_then(|t| loads(&t));
        let mut d = match read {
            Ok(Value::Object(pairs)) => pairs,
            Ok(_) => {
                return Err(usage(
                    "Error: --data is one JSON object (keys: \"<verb>\" or \"<verb> <address>\", and \"store\")",
                ))
            }
            Err(x) => {
                return Err(usage(format!(
                    "Error: --data {} is not JSON: {}",
                    f.data, x.message
                )))
            }
        };
        let bdir = blobs_dir(&f.data);
        if Path::new(&bdir).is_dir() {
            let blobs = blobs(&bdir).map_err(raise)?;
            set(&mut d, "blobs", Value::Object(blobs));
        }
        b.push((Text::from("data"), Value::Object(d)));
    }
    let mut config: Vec<(Text, Value)> = Vec::new();
    for kv in a.cfg {
        let Some((k, v)) = kv.split_once('=') else {
            return Err(usage(format!(
                "Error: --config takes key=value, not {}",
                py::repr(&jstr(kv))
            )));
        };
        set(
            &mut config,
            k.trim_matches(crate::families::py_space),
            jstr(v),
        );
    }
    b[0].1 = Value::Object(config);
    Ok(b)
}

/// The `blobs/` folder: each file (not a hidden one) by its sha256 ref,
/// with its name, its type and its bytes in base64, in name order.
fn blobs(bdir: &str) -> Result<Vec<(Text, Value)>, Raise> {
    let entries = std::fs::read_dir(bdir)
        .map_err(|e| Raise::at(line::LISTDIR, os_error(&e, bdir).last_line()))?;
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut db: Option<mimetypes::MimeTypes> = None;
    let mut out: Vec<(Text, Value)> = Vec::new();
    for name in names {
        let p = pypath::join(bdir, &name);
        if !Path::new(&p).is_file() || name.starts_with('.') {
            continue;
        }
        let raw = std::fs::read(&p)
            .map_err(|e| Raise::at(line::READ_BLOB, os_error(&e, &p).last_line()))?;
        let reference = format!(
            "k2blob:sha256-{}",
            k2_client::release_sums::sha256_hex(&raw)
        );
        if db.is_none() {
            db = Some(mimetypes::system().map_err(|x| Raise::at(line::GUESS, x.last_line()))?);
        }
        let mime = db
            .as_ref()
            .and_then(|db| db.guess_type(&name))
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "application/octet-stream".into());
        let entry = Value::Object(vec![
            (Text::from("name"), jstr(&name)),
            (Text::from("mime"), jstr(&mime)),
            (
                Text::from("base64"),
                jstr(&crate::families::store::widget::b64encode(&raw)),
            ),
        ]);
        set(&mut out, &reference, entry);
    }
    Ok(out)
}

const NO_ANSWER: &str = concat!(
    "Error: no response from the ",
    product_name!(),
    " daemon on this computer"
);

/// The answer's check: refused, a folder with errors, or the page
/// (`None`: go on and shoot it).
pub fn checked(as_json: bool, answer: &[u8]) -> Option<Shown> {
    let mut o = Out::default();
    // open(...).read(): UTF-8, universal newlines.
    let raw = match decode_utf8(answer) {
        Ok(t) => t.replace("\r\n", "\n").replace('\r', "\n"),
        Err(x) => return Some(o.raise(&Raise::at(line::READ_ANSWER, x.last_line()))),
    };
    if blank(&raw) {
        o.e(NO_ANSWER);
        return Some(o.done(Exit::Error));
    }
    let Ok(d) = loads(&raw) else {
        o.e(format!(
            "Error: the daemon answered something that is not JSON: {}",
            head200(&raw)
        ));
        return Some(o.done(Exit::Error));
    };
    let err = match py_get(&d, "error", line::ERROR) {
        Ok(e) => e,
        Err(r) => return Some(o.raise(&r)),
    };
    if let Some(err) = err.filter(|e| py::truthy(e)) {
        if as_json {
            o.o(indented(&d));
        } else {
            o.e(format!(
                "Error: {}: {}",
                py::str_of(err),
                or(print::g(&d, "message"), "")
            ));
        }
        let refused = ["person_only", "zen_local_only", "owner_only"]
            .iter()
            .any(|w| print::is(Some(err), w));
        return Some(o.done(if refused { Exit::Refused } else { Exit::Error }));
    }
    if !matches!(print::g(&d, "ok"), Some(Value::Bool(true))) {
        if as_json {
            o.o(indented(&d));
        } else {
            o.e("The folder has errors; nothing was shot:");
            let none = Value::Array(Vec::new());
            let errors = ops::or(print::g(&d, "errors"), &none);
            let each = match py_iter(errors, line::ERRORS) {
                Ok(v) => v,
                Err(r) => return Some(o.raise(&r)),
            };
            for e in each {
                let field = |k: &str| py_get(&e, k, line::ERROR_LINE).map(s);
                let row = (|| {
                    Ok::<_, Raise>(format!(
                        "{}:{}:{}: {}",
                        field("file")?,
                        field("line")?,
                        field("col")?,
                        field("message")?
                    ))
                })();
                match row {
                    Ok(r) => o.e(r),
                    Err(r) => return Some(o.raise(&r)),
                }
            }
        }
        return Some(o.done(Exit::Error));
    }
    None
}

/// The answer in a temp file of its own (`mktemp`'s name and mode).
fn page_file(answer: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let stem = format!("{}/k2-zen-shot-page.", tmpdir());
    let mut last = std::io::Error::other("no name left");
    for n in 0..100u32 {
        let p = PathBuf::from(format!("{stem}{}", suffix(n)));
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        match o.open(&p) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(answer) {
                    let _ = std::fs::remove_file(&p);
                    return Err(e);
                }
                return Ok(p);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

/// Six characters as mktemp picks them: letters and digits, from the
/// time, the process and the try.
fn suffix(n: u32) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seed = format!("{nanos}-{}-{n}", std::process::id());
    let hash = blake3::hash(seed.as_bytes());
    hash.as_bytes()[..6]
        .iter()
        .map(|b| ALPHA[usize::from(*b) % ALPHA.len()] as char)
        .collect()
}

/// `out="$(node "$runner" --page <page> --out <png> [--libs …]
/// [--wait-ms …])" || rc=$?`: its stdout as `$(…)` holds it (NULs and
/// trailing newlines gone) and its exit (128 and the signal when one
/// ended it). Its stdin and stderr are this command's; its environment is
/// this command's without the front's variables (the shell takes them
/// out first).
fn run_runner(node: &Path, runner: &Path, page: &Path, f: &Flags) -> (Vec<u8>, i32) {
    let mut cmd = std::process::Command::new(node);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.arg0("node");
    }
    cmd.arg(runner)
        .arg("--page")
        .arg(page)
        .arg("--out")
        .arg(&f.png);
    if !f.libs.is_empty() {
        cmd.arg("--libs").arg(&f.libs);
    }
    if !f.wait_ms.is_empty() {
        cmd.arg("--wait-ms").arg(&f.wait_ms);
    }
    for v in crate::called_as::FRONT_VARS {
        cmd.env_remove(v);
    }
    cmd.stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());
    let ran = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            crate::out::stderr_line(&format!("{}: {e}", node.to_string_lossy()));
            return (Vec::new(), 126);
        }
    };
    let rc = match ran.status.code() {
        Some(c) => c,
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                128 + ran.status.signal().unwrap_or(0)
            }
            #[cfg(not(unix))]
            {
                1
            }
        }
    };
    let mut out = super::super::without_nuls(&ran.stdout);
    while out.last() == Some(&b'\n') {
        out.pop();
    }
    (out, rc)
}

/// The runner's report, as the shell's Python prints it.
pub fn report(out: &str) -> Shown {
    let mut o = Out::default();
    let d = match loads(out) {
        Ok(d) => d,
        Err(x) => return o.raise(&Raise::at(line::LOAD, x.last_line())),
    };
    match report_lines(&d, &mut o) {
        Ok(()) => o.done(Exit::Ok),
        Err(r) => o.raise(&r),
    }
}

fn report_lines(d: &Value, o: &mut Out) -> Result<(), Raise> {
    let mut shot = Vec::new();
    for k in ["png", "scheme", "width", "height", "ms"] {
        shot.push(py::str_of(ops::index(d, k, line::SHOT)?));
    }
    o.o(format!(
        "Shot {} ({}, {}x{}) in {} ms",
        shot[0], shot[1], shot[2], shot[3], shot[4]
    ));
    if !py::truthy(ops::index(d, "ready", line::READY)?) {
        o.o("The widget never said ready (k2.ready()): in Zen it would be stopped as not started.");
    }
    for e in py_iter(ops::index(d, "errors", line::EACH_ERROR)?, line::EACH_ERROR)? {
        o.o(format!("error: {}", py::str_of(&e)));
    }
    let refusals = ops::index(d, "refusals", line::REFUSALS)?;
    if py::truthy(refusals) {
        o.o(format!(
            "refused: {}",
            ops::join(refusals, ", ", line::JOIN)?
        ));
    }
    o.o(concat!(
        "The data is the ",
        product_name!(),
        " samples (alpha, bravo, charlie; a sample agent and Thread) unless --data <file.json> gave your own."
    ));
    Ok(())
}

/// `k2 zen widget preview --dir <folder> [<name>] --shot <png> …`: the
/// usage line of a shot line that is not one.
pub const SHOT_USAGE: &str = concat!(
    "Error: ",
    cli_name!(),
    " zen widget preview --dir <folder> [<name>] --shot <png> [--theme dark|light] [--width N] [--height N]"
);

#[cfg(test)]
mod tests;
