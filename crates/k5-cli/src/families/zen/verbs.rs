//! `cmd_zen`'s own verbs (`cli/src/zen-retired.sh`): `validate`,
//! `history`, `reload`, `reset`, `doctor` and `path`, read by `_zen_args`
//! and printed by `_zen_print` ([`super::print`]); and the arms that only
//! print (`pages`, `grant`, an unknown verb).

use super::call::Zen;
use super::print::{self, g, zen_print, Env, Out};
use crate::exit::Exit;
use crate::families::Shown;
use crate::out::json::{Text, Value};
use crate::out::pylib::json as pyjson;
use crate::transport::encode::quote;
use k2_client::cli_name;

/// What `_zen_args` read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    pub json: bool,
    pub garden: String,
    pub file: String,
    pub to: String,
    pub theme: String,
    pub widget: String,
    pub no_sign: bool,
    pub types: String,
    pub dir: String,
    pub as_: String,
    pub from: String,
    pub strict: bool,
    pub sign: bool,
}

/// A usage line on stderr, exit 2.
pub fn usage(line: impl Into<String>) -> Shown {
    Shown::err(format!("{}\n", line.into()), Exit::Usage)
}

/// `_zen_args "$@"`.
pub fn args(words: &[&str]) -> Result<Args, Shown> {
    let mut a = Args::default();
    let mut i = 0;
    let need = |i: usize, msg: &str| -> Result<String, Shown> {
        match words.get(i + 1) {
            Some(v) if !v.is_empty() => Ok((*v).to_string()),
            _ => Err(usage(msg)),
        }
    };
    while i < words.len() {
        let w = words[i];
        let take2 = |slot: &mut String, msg: &str| -> Result<(), Shown> {
            *slot = need(i, msg)?;
            Ok(())
        };
        let after = |w: &str| {
            w.split_once('=')
                .map(|(_, v)| v.to_string())
                .unwrap_or_default()
        };
        match w {
            "--no-sign" => a.no_sign = true,
            "--sign" => a.sign = true,
            "--strict" => a.strict = true,
            "--dir" => {
                take2(&mut a.dir, "Error: --dir needs a staged widget folder")?;
                i += 1;
            }
            "--as" => {
                take2(&mut a.as_, "Error: --as needs a widget name")?;
                i += 1;
            }
            "--from" => {
                take2(
                    &mut a.from,
                    "Error: --from needs a staged Garden page (or - for stdin)",
                )?;
                i += 1;
            }
            "--widget" => {
                take2(
                    &mut a.widget,
                    concat!(
                        "Error: --widget needs a widget folder name (see ",
                        cli_name!(),
                        " zen widget list)"
                    ),
                )?;
                i += 1;
            }
            "--types" => a.types = "1".into(),
            "--types=all" => a.types = "all".into(),
            "--json" => a.json = true,
            "--garden" => {
                take2(
                    &mut a.garden,
                    concat!(
                        "Error: --garden needs a Garden name or id (see ",
                        cli_name!(),
                        " zen garden list)"
                    ),
                )?;
                i += 1;
            }
            "--file" => {
                take2(
                    &mut a.file,
                    "Error: --file needs zen.toml, gardens/<id>.toml or themes/<name>/theme.toml",
                )?;
                i += 1;
            }
            "--to" => {
                take2(
                    &mut a.to,
                    concat!(
                        "Error: --to needs a snapshot name (see ",
                        cli_name!(),
                        " zen history)"
                    ),
                )?;
                i += 1;
            }
            "--theme" => {
                take2(
                    &mut a.theme,
                    concat!(
                        "Error: --theme needs a theme name (see ",
                        cli_name!(),
                        " zen theme list)"
                    ),
                )?;
                i += 1;
            }
            _ if w.starts_with("--dir=") => a.dir = after(w),
            _ if w.starts_with("--as=") => a.as_ = after(w),
            _ if w.starts_with("--from=") => a.from = after(w),
            _ if w.starts_with("--widget=") => a.widget = after(w),
            _ if w.starts_with("--garden=") => a.garden = after(w),
            _ if w.starts_with("--file=") => a.file = after(w),
            _ if w.starts_with("--to=") => a.to = after(w),
            _ if w.starts_with("--theme=") => a.theme = after(w),
            _ if w == "--home" || w.starts_with("--home=") => return Err(no_home()),
            _ => {
                return Err(usage(format!(
                    concat!("Error: unexpected '{}' (see ", cli_name!(), " zen --help)"),
                    w
                )))
            }
        }
        i += 1;
    }
    let n = [&a.garden, &a.file, &a.theme, &a.widget, &a.dir]
        .iter()
        .filter(|s| !s.is_empty())
        .count();
    if n > 1 {
        return Err(usage(
            "Error: pass one of --garden, --theme, --widget, --dir or --file",
        ));
    }
    Ok(a)
}

/// `_zen_no_home`.
pub fn no_home() -> Shown {
    usage(concat!(
        "Error: --home is gone: Zen pages are Gardens now. Use --garden <name|id> (",
        cli_name!(),
        " zen garden list)."
    ))
}

/// `_zen_validate_only_flags SUB`.
fn validate_only(sub: &str, a: &Args) -> Result<(), Shown> {
    if sub == "validate" {
        return Ok(());
    }
    if !a.dir.is_empty() || !a.as_.is_empty() || !a.from.is_empty() || a.strict || a.sign {
        return Err(usage(concat!(
            "Error: --dir, --as, --from, --strict and --sign are for ",
            cli_name!(),
            " zen validate"
        )));
    }
    Ok(())
}

/// `cmd_zen`'s own verbs, read by `_zen_args`.
pub fn is_core(sub: &str) -> bool {
    matches!(
        sub,
        "validate" | "history" | "reload" | "reset" | "doctor" | "path"
    )
}

/// What a core verb does before it asks: a usage error, or the plan.
pub enum Plan {
    Stop(Shown),
    /// `validate --dir`: the folder's body, built (or its usage error).
    Dir {
        a: Args,
        body: Vec<u8>,
    },
    /// `validate --garden … --from <file>|-`: the text read.
    From {
        a: Args,
        text: String,
    },
    Read {
        sub: String,
        a: Args,
    },
    Reload(Args),
    Reset(Args),
    Plain {
        sub: String,
        a: Args,
    },
}

/// The checks `cmd_zen` makes before its request.
pub fn plan(sub: &str, words: &[&str]) -> Plan {
    match plan_or_stop(sub, words) {
        Ok(p) => p,
        Err(s) => Plan::Stop(s),
    }
}

fn plan_or_stop(sub: &str, words: &[&str]) -> Result<Plan, Shown> {
    let a = args(words)?;
    validate_only(sub, &a)?;
    let any_sel = || {
        !(a.garden.is_empty()
            && a.file.is_empty()
            && a.to.is_empty()
            && a.theme.is_empty()
            && a.widget.is_empty())
    };
    match sub {
        "validate" | "history" => {
            if !a.to.is_empty() {
                return Err(usage(concat!(
                    "Error: --to is for ",
                    cli_name!(),
                    " zen reset"
                )));
            }
            if a.no_sign && (sub != "validate" || a.widget.is_empty()) {
                return Err(usage(concat!(
                    "Error: --no-sign is for ",
                    cli_name!(),
                    " zen validate --widget"
                )));
            }
            if a.sign && a.widget.is_empty() {
                return Err(usage(concat!(
                    "Error: --sign is for ",
                    cli_name!(),
                    " zen validate --widget <name>"
                )));
            }
            if a.sign && a.no_sign {
                return Err(usage("Error: pass --sign or --no-sign, not both"));
            }
            if !a.types.is_empty() && (sub != "validate" || a.widget.is_empty()) {
                return Err(usage(concat!(
                    "Error: --types is for ",
                    cli_name!(),
                    " zen validate --widget <name>"
                )));
            }
            if !a.as_.is_empty() && a.dir.is_empty() {
                return Err(usage(concat!(
                    "Error: --as names a staged folder: ",
                    cli_name!(),
                    " zen validate --dir <folder> --as <name>"
                )));
            }
            if !a.from.is_empty() && a.garden.is_empty() {
                return Err(usage(concat!(
                    "Error: --from checks a staged Garden page: ",
                    cli_name!(),
                    " zen validate --garden <name|id> --from <file>|-"
                )));
            }
            if !a.dir.is_empty() {
                let mut extra = Vec::new();
                if !a.as_.is_empty() {
                    extra.push((Text::from("widget"), print::jstr(&a.as_)));
                }
                let body = dir_body(&a.dir, extra, NameKey::Fallback)?;
                return Ok(Plan::Dir { a, body });
            }
            if !a.from.is_empty() {
                let text = read_text(&a.from)?;
                return Ok(Plan::From { a, text });
            }
            Ok(Plan::Read {
                sub: sub.to_string(),
                a,
            })
        }
        "reload" => {
            if any_sel() {
                return Err(usage(concat!(
                    "Error: ",
                    cli_name!(),
                    " zen reload takes only --json"
                )));
            }
            Ok(Plan::Reload(a))
        }
        "reset" => Ok(Plan::Reset(a)),
        _ => {
            if any_sel() {
                return Err(usage(format!(
                    concat!("Error: ", cli_name!(), " zen {} takes only --json"),
                    sub
                )));
            }
            Ok(Plan::Plain {
                sub: sub.to_string(),
                a,
            })
        }
    }
}

/// `_zen_read_text SRC`: the file, or stdin for `-`, as Python's
/// `sys.stdin.read()` reads it (UTF-8).
pub fn read_text(src: &str) -> Result<String, Shown> {
    let bytes = if src == "-" {
        let mut b = Vec::new();
        let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut b);
        b
    } else {
        if !std::path::Path::new(src).is_file() {
            return Err(usage(format!("Error: no file {src}")));
        }
        std::fs::read(src).unwrap_or_default()
    };
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// `_zen_dir_body`'s `<name-key>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameKey {
    /// `widget`: the manifest's id (or the folder's name) when the extra
    /// has none.
    Widget,
    /// `fallback`: the folder's own name.
    Fallback,
}

/// `_zen_dir_body DIR OUT EXTRA NAME_KEY`: the JSON body (`json.dump`'s
/// bytes), or its usage error (exit 2). A note about skipped files goes
/// to stderr at once, as the shell's Python prints it.
pub fn dir_body(dir: &str, extra: Vec<(Text, Value)>, key: NameKey) -> Result<Vec<u8>, Shown> {
    use std::path::Path;
    let d = Path::new(dir);
    if !d.is_dir() {
        return Err(usage(format!("Error: no folder {dir}")));
    }
    const CODE: &[&str] = &["html", "js", "css", "json"];
    const ASSET: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "woff2", "svg"];
    let mut names: Vec<String> = std::fs::read_dir(d)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let mut files: Vec<Value> = Vec::new();
    let mut manifest: Option<String> = None;
    let mut skipped: Vec<String> = Vec::new();
    let mut total: usize = 0;
    for name in names {
        let p = crate::out::pylib::path::join(dir, &name);
        let low = name.to_lowercase();
        if low.starts_with('.')
            || low.starts_with("readme")
            || low == "license"
            || low.starts_with("license.")
        {
            continue;
        }
        let pp = Path::new(&p);
        if pp
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(usage(format!(
                "Error: {p} is a link; put the file itself in the folder"
            )));
        }
        if pp.is_dir() {
            skipped.push(format!("{name}/"));
            continue;
        }
        let ext = low.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        let raw;
        if CODE.contains(&ext) {
            raw = std::fs::read(pp).unwrap_or_default();
            match String::from_utf8(raw.clone()) {
                Ok(text) => {
                    if name == "manifest.json" {
                        manifest = Some(text.clone());
                    }
                    files.push(Value::Object(vec![
                        (Text::from("name"), print::jstr(&name)),
                        (Text::from("text"), print::jstr(&text)),
                    ]));
                }
                Err(_) => return Err(usage(format!("Error: {p} is not UTF-8 text"))),
            }
        } else if ASSET.contains(&ext) {
            raw = std::fs::read(pp).unwrap_or_default();
            files.push(Value::Object(vec![
                (Text::from("name"), print::jstr(&name)),
                (
                    Text::from("base64"),
                    print::jstr(&crate::families::store::widget::b64encode(&raw)),
                ),
            ]));
        } else {
            skipped.push(name);
            continue;
        }
        total += raw.len();
        if total > 4 * 1024 * 1024 {
            return Err(usage(format!("Error: {dir} is over 4 MB of widget files")));
        }
    }
    if !skipped.is_empty() {
        crate::out::stderr_line(&format!(
            "note: skipped {} (a widget is one flat folder of .html .js .css .json, images and .woff2 fonts)",
            skipped.join(", ")
        ));
    }
    if files.is_empty() {
        return Err(usage(format!(
            "Error: {dir} has no widget files (manifest.json, the entry .html, ...)"
        )));
    }
    let mut body = extra;
    let set = |body: &mut Vec<(Text, Value)>, k: &str, v: Value| {
        let k = Text::from(k);
        match body.iter_mut().find(|(n, _)| *n == k) {
            Some(slot) => slot.1 = v,
            None => body.push((k, v)),
        }
    };
    set(&mut body, "files", Value::Array(files));
    let abs = crate::out::pylib::path::abspath(dir);
    let own = crate::out::pylib::path::split(&abs).1;
    match key {
        NameKey::Widget => {
            let has = body
                .iter()
                .find(|(k, _)| k.as_str() == Some("widget"))
                .is_some_and(|(_, v)| crate::out::py::truthy(v));
            if !has {
                let mid = manifest
                    .and_then(|m| pyjson::loads(&m).ok())
                    .and_then(|v| g(&v, "id").cloned());
                let w = match mid {
                    Some(Value::Str(t)) if !t.code_points().is_empty() => Value::Str(t),
                    _ => print::jstr(&own),
                };
                set(&mut body, "widget", w);
            }
        }
        NameKey::Fallback => set(&mut body, "fallback", print::jstr(&own)),
    }
    Ok(
        crate::out::json::to_string(&Value::Object(body), &Default::default())
            .unwrap_or_default()
            .into_bytes(),
    )
}

/// `json.dumps({"k": v, …})` of the pairs with a value, in order.
pub fn body_of(pairs: &[(&str, &str)]) -> Vec<u8> {
    let o: Vec<(Text, Value)> = pairs
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (Text::from(*k), print::jstr(v)))
        .collect();
    crate::out::json::to_string(&Value::Object(o), &Default::default())
        .unwrap_or_default()
        .into_bytes()
}

/// The validate answer is clean enough to sign: `d.get("ok") is True and
/// d.get("error") is None and not d.get("locked")`.
fn clean(raw: &[u8]) -> bool {
    let text = String::from_utf8_lossy(raw);
    let Ok(d @ Value::Object(_)) = pyjson::loads(&text) else {
        return false;
    };
    matches!(g(&d, "ok"), Some(Value::Bool(true)))
        && g(&d, "error").is_none_or(|e| matches!(e, Value::Null))
        && !g(&d, "locked").is_some_and(crate::out::py::truthy)
}

/// The core verb's requests and printing.
pub fn run(z: &mut Zen, p: Plan) -> Result<Exit, Exit> {
    Ok(match p {
        Plan::Stop(s) => s.print(),
        Plan::Dir { a, body } => {
            let raw = z.post_file("/cli/zen/widget/folder/check", &body)?;
            zen_print("validate", a.json, &raw, env(&a)).print()
        }
        Plan::From { a, text } => {
            let body = Value::Object(vec![
                (Text::from("garden"), print::jstr(&a.garden)),
                (Text::from("text"), print::jstr(&text)),
            ]);
            let body = crate::out::json::to_string(&body, &Default::default()).unwrap_or_default();
            let raw = z.session_post("/cli/zen/garden/check", body.as_bytes())?;
            zen_print("validate", a.json, &raw, env(&a)).print()
        }
        Plan::Read { sub, a } => {
            let mut q: Vec<String> = Vec::new();
            for (k, v) in [
                ("garden", &a.garden),
                ("file", &a.file),
                ("theme", &a.theme),
                ("widget", &a.widget),
            ] {
                if !v.is_empty() {
                    q.push(format!("{k}={}", quote(v)));
                }
            }
            if !a.types.is_empty() {
                q.push(format!("types={}", a.types));
            }
            let q: Vec<&str> = q.iter().map(String::as_str).collect();
            let mut raw = z.get(&format!("/cli/zen/{sub}"), &q)?;
            let mut signed: Option<Vec<u8>> = None;
            if sub == "validate" && !a.widget.is_empty() && a.sign && clean(&raw) {
                let body = body_of(&[("widget", &a.widget)]);
                let s = z.session_post("/cli/zen/widget/sign", &body)?;
                if a.json {
                    raw = with_signed(&raw, &s);
                }
                signed = Some(s);
            }
            let exit = zen_print(&sub, a.json, &raw, env(&a)).print();
            if exit != Exit::Ok {
                return Ok(exit);
            }
            match signed {
                Some(s) if !s.is_empty() && !a.json => {
                    let shown = super::sign::print("sign", false, &s);
                    if shown.print() != Exit::Ok {
                        crate::out::stderr_line(
                            "(validate was clean; signing failed, so its data calls keep the version's old maker)",
                        );
                    }
                    Exit::Ok
                }
                _ => Exit::Ok,
            }
        }
        Plan::Reload(a) => {
            let raw = z.post("/cli/zen/reload", b"{}")?;
            zen_print("reload", a.json, &raw, env(&a)).print()
        }
        Plan::Reset(a) => {
            let body = body_of(&[
                ("garden", &a.garden),
                ("file", &a.file),
                ("to", &a.to),
                ("theme", &a.theme),
                ("widget", &a.widget),
            ]);
            let raw = z.session_post("/cli/zen/reset", &body)?;
            zen_print("reset", a.json, &raw, env(&a)).print()
        }
        Plan::Plain { sub, a } => {
            let ep = if sub == "doctor" {
                "/cli/zen/doctor"
            } else {
                "/cli/zen/status"
            };
            let raw = z.get(ep, &[])?;
            zen_print(&sub, a.json, &raw, env(&a)).print()
        }
    })
}

fn env(a: &Args) -> Env<'static> {
    Env {
        strict: a.strict,
        sync_raw: None,
    }
}

/// The validate answer with `"signed"` set, as the shell's Python
/// re-prints it (`json.dumps(d)`).
fn with_signed(raw: &[u8], signed: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(raw);
    let Ok(Value::Object(mut d)) = pyjson::loads(&text) else {
        return raw.to_vec();
    };
    let st = String::from_utf8_lossy(signed);
    let sv = pyjson::loads(if st.is_empty() { "null" } else { &st }).unwrap_or(Value::Null);
    let k = Text::from("signed");
    match d.iter_mut().find(|(n, _)| *n == k) {
        Some(slot) => slot.1 = sv,
        None => d.push((k, sv)),
    }
    crate::out::json::to_string(&Value::Object(d), &Default::default())
        .unwrap_or_default()
        .into_bytes()
}

/// `cmd_help_zen`: the page, on stdout (exit 0) or stderr.
pub fn help(to_stderr: bool, exit: Exit) -> Exit {
    let page = crate::text::help("zen").expect("the zen help page is embedded");
    if to_stderr {
        let _ = crate::out::write_flushed(&mut std::io::stderr().lock(), page.as_bytes());
        return exit;
    }
    match crate::out::stdout(page) {
        Ok(()) => exit,
        Err(e) => crate::out::after_stdout_error(&e),
    }
}

/// The arms that only print.
pub fn only_prints(sub: &str) -> Option<Exit> {
    match sub {
        "" | "help" => Some(help(false, Exit::Ok)),
        "pages" => Some(
            usage(concat!(
                "Error: there is no '",
                cli_name!(),
                " zen pages' any more: Zen pages are Gardens now. Use ",
                cli_name!(),
                " zen garden list."
            ))
            .print(),
        ),
        "grant" | "grants" | "allow" => {
            let mut o = Out::default();
            o.e(format!(
                concat!(
                    "Error: there is no '",
                    cli_name!(),
                    " zen {}'. Widgets in your own Gardens need no permissions:"
                ),
                sub
            ));
            o.e("they can talk to your agents right away.");
            Some(o.done(Exit::Usage).print())
        }
        _ => None,
    }
}

/// An unknown verb: its line, the page on stderr, exit 2.
pub fn unknown(sub: &str) -> Exit {
    crate::out::stderr_line(&format!(
        concat!("Unknown '", cli_name!(), " zen' subcommand: {}"),
        sub
    ));
    help(true, Exit::Usage)
}
