//! The shell's Python readers of `cli/src/wiki.sh`, one function each: what
//! each prints for the answer it reads, and where it raises, the Python
//! 3.12 traceback. Line numbers are those `python3 -c` gives the reader's
//! lines (`tests` pins each to the shell).

use super::super::{get, py_get, py_iter, py_len, py_space, raised, s, Raise, Shown};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::py;
use crate::out::pylib::json::loads;
use crate::out::pylib::ops::{join, or, py_in};
use k2_client::{cli_name, product_name};

/// What a reader reads: the bytes as text (U+FFFD for a byte that is not
/// UTF-8, as the other native families read them).
fn text(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

/// `not raw.strip()`.
fn blank(raw: &str) -> bool {
    raw.chars().all(py_space)
}

fn dumps(v: &Value, compact: bool) -> String {
    json::to_string(
        v,
        &Options {
            compact,
            ..Options::default()
        },
    )
    .expect("a value from JSON always prints")
}

/// `isinstance(d, dict) and d.get("error") is not None`: the error.
fn error_of(d: &Value) -> Option<&Value> {
    match d {
        Value::Object(p) => get(p, "error").filter(|e| **e != Value::Null),
        _ => None,
    }
}

/// `err if not isinstance(err, dict) else (err.get("hint") or
/// err.get("message") or err)`, as `%s` prints it. `message`: the reader
/// asks for it (the serve-off reader doesn't).
fn hint_of(err: &Value, message: bool) -> String {
    match err {
        Value::Object(p) => {
            let keys: &[&str] = if message {
                &["hint", "message"]
            } else {
                &["hint"]
            };
            keys.iter()
                .filter_map(|k| get(p, k))
                .find(|v| py::truthy(v))
                .map_or_else(|| py::str_of(err), py::str_of)
        }
        other => py::str_of(other),
    }
}

/// `"Error: %s" % <hint>` on stderr, exit 1.
fn daemon_error(err: &Value) -> Shown {
    Shown::err(format!("Error: {}\n", hint_of(err, true)), Exit::Error)
}

/// `{"error": err if isinstance(err, dict) else {"code": "daemon", "hint":
/// str(err)}}`.
fn error_object(err: &Value) -> Value {
    let inner = match err {
        Value::Object(_) => err.clone(),
        other => Value::Object(vec![
            (Text::from("code"), s("daemon")),
            (Text::from("hint"), s(&py::str_of(other))),
        ]),
    };
    Value::Object(vec![(Text::from("error"), inner)])
}

/// `{"error":{"code":<code>,"hint":<hint>}}` as `json.dumps` writes it.
fn coded(code: &str, hint: &str) -> String {
    dumps(
        &Value::Object(vec![(
            Text::from("error"),
            Value::Object(vec![
                (Text::from("code"), s(code)),
                (Text::from("hint"), s(hint)),
            ]),
        )]),
        false,
    )
}

/// The head the text readers share (lines 3 to 12): the answer, or what
/// they print when there is none, it is not JSON or it is the daemon's
/// error.
fn head(raw: &[u8]) -> Result<Value, Shown> {
    let raw = text(raw);
    if blank(&raw) {
        return Err(Shown::err("Error: no response from daemon\n", Exit::Error));
    }
    let d = loads(&raw).map_err(|e| Shown::err(format!("Error: {}\n", e.message), Exit::Error))?;
    if let Some(err) = error_of(&d) {
        return Err(daemon_error(err));
    }
    Ok(d)
}

/// `index --json` and `note --json`: the answer as compact JSON, or the
/// error objects. `raw` is what the reader reads (`index`'s here-string
/// adds a newline).
pub fn as_json(raw: &[u8]) -> Shown {
    let raw = text(raw);
    if blank(&raw) {
        return Shown::err(
            format!("{}\n", coded("no_response", "no response from daemon")),
            Exit::Error,
        );
    }
    let Ok(d) = loads(&raw) else {
        let hint: String = raw.chars().take(200).collect();
        return Shown::err(format!("{}\n", coded("bad_json", &hint)), Exit::Error);
    };
    if let Some(err) = error_of(&d) {
        return Shown {
            stdout: format!("{}\n", dumps(&error_object(err), false)),
            stderr: String::new(),
            exit: Exit::Error,
        };
    }
    Shown::out(format!("{}\n", dumps(&d, true)))
}

/// `json.loads(s) if s.strip() else {}`, `{}` for anything Python can't
/// read.
fn load(raw: &str) -> Value {
    if blank(raw) {
        return Value::Object(Vec::new());
    }
    loads(raw).unwrap_or(Value::Object(Vec::new()))
}

/// `status`'s reader: the serve status (`raw`) and the index (`index`), as
/// `$(…)` held them, the workspace and `--json`.
pub fn status(raw: &[u8], index: &[u8], project: &str, as_json: bool) -> Shown {
    let raw = text(raw);
    let d = load(&raw);
    if blank(&raw) {
        return Shown::err(
            "Error: no response from daemon (wiki routes missing?)\n",
            Exit::Error,
        );
    }
    if let Some(err) = error_of(&d) {
        if as_json {
            return Shown {
                stdout: format!("{}\n", dumps(&error_object(err), true)),
                stderr: String::new(),
                exit: Exit::Error,
            };
        }
        return daemon_error(err);
    }
    let mut idx = load(&text(index));
    if error_of(&idx).is_some() {
        idx = Value::Object(Vec::new());
    }
    match status_lines(&d, &idx, project, as_json) {
        Ok(out) => Shown::out(out),
        Err(r) => raised(String::new(), String::new(), &r),
    }
}

fn truthy(v: Option<&Value>) -> Option<&Value> {
    v.filter(|x| py::truthy(x))
}

/// `a or b or ...` over `d.get(k)`s: the first true one.
fn first_of<'a>(d: &'a Value, keys: &[&str], line: u32) -> Result<Option<&'a Value>, Raise> {
    for k in keys {
        if let Some(v) = truthy(py_get(d, k, line)?) {
            return Ok(Some(v));
        }
    }
    Ok(None)
}

fn none_or(v: Option<&Value>) -> Value {
    v.cloned().unwrap_or(Value::Null)
}

/// `"http://127.0.0.1:%s" % port if port else None`, after `d.get("url")`.
fn url_of(d: &Value, port: Option<&Value>, line: u32) -> Result<Option<Value>, Raise> {
    if let Some(u) = truthy(py_get(d, "url", line)?) {
        return Ok(Some(u.clone()));
    }
    Ok(truthy(port).map(|p| s(&format!("http://127.0.0.1:{}", py::str_of(p)))))
}

/// `"port %s" % (port or "?")`.
fn port_words(port: Option<&Value>) -> String {
    format!("port {}", truthy(port).map_or("?".to_string(), py::str_of))
}

fn status_lines(d: &Value, idx: &Value, project: &str, as_json: bool) -> Result<String, Raise> {
    let mut note_count = none_or(py_get(idx, "noteCount", 30)?);
    if note_count == Value::Null {
        if let Some(Value::Array(nodes)) = py_get(idx, "nodes", 31)? {
            let mut n = 0u64;
            for node in nodes {
                let shown = match node {
                    Value::Object(p) => get(p, "exists").is_none_or(py::truthy),
                    other => {
                        return Err(Raise::inside(
                            32,
                            32,
                            "<genexpr>",
                            format!(
                                "AttributeError: '{}' object has no attribute 'get'",
                                py::type_name(other)
                            ),
                        ))
                    }
                };
                n += u64::from(shown);
            }
            note_count = Value::Int(n.to_string());
        }
    }
    let wiki_rel = truthy(py_get(idx, "wikiRel", 33)?)
        .cloned()
        .unwrap_or_else(|| s(".k2/wiki"));
    let ws_path = truthy(py_get(idx, "workspacePath", 34)?)
        .cloned()
        .unwrap_or_else(|| s(project));
    let enabled = first_of(d, &["enabled", "serving"], 35)?.is_some();
    let port = py_get(d, "port", 36)?;
    let url = url_of(d, port, 37)?;
    let publ = first_of(d, &["publishedLabel", "published_label", "label"], 38)?;
    // `a or b or c` is `c` when none is true, `None` only when it's missing.
    let publ_value = match publ {
        Some(p) => p.clone(),
        None => none_or(py_get(d, "label", 38)?),
    };
    let generated = match truthy(py_get(idx, "generatedAt", 39)?) {
        Some(g) => Some(g.clone()),
        None => py_get(d, "generatedAt", 39)?.cloned(),
    };
    if as_json {
        let mut out: Vec<(Text, Value)> = vec![
            (Text::from("workspacePath"), ws_path.clone()),
            (Text::from("wikiRel"), wiki_rel.clone()),
            (Text::from("enabled"), Value::Bool(enabled)),
            (Text::from("serving"), Value::Bool(enabled)),
            (Text::from("port"), none_or(port)),
            (Text::from("url"), url.clone().unwrap_or(Value::Null)),
            (Text::from("publishedLabel"), publ_value),
            (Text::from("noteCount"), note_count.clone()),
            (
                Text::from("generatedAt"),
                generated.clone().unwrap_or(Value::Null),
            ),
        ];
        if let Value::Object(pairs) = d {
            for (k, v) in pairs {
                if !out.iter().any(|(o, _)| o == k) {
                    out.push((k.clone(), v.clone()));
                }
            }
        }
        return Ok(format!("{}\n", dumps(&Value::Object(out), true)));
    }
    let mut o = String::new();
    let ws = if py::truthy(&ws_path) {
        py::str_of(&ws_path)
    } else {
        "?".into()
    };
    o.push_str(&format!("Wiki  {ws}/{}\n", py::str_of(&wiki_rel)));
    let notes = if note_count == Value::Null {
        "?".to_string()
    } else {
        py::str_of(&note_count)
    };
    o.push_str(&format!("  Notes:      {notes}\n"));
    if enabled {
        let at = url.as_ref().map_or_else(|| port_words(port), py::str_of);
        o.push_str(&format!("  Serving:    yes  {at}\n"));
    } else {
        o.push_str("  Serving:    no\n");
    }
    if truthy(py_get(d, "publicChatEnabled", 67)?).is_some() {
        o.push_str(
            "  Public chat: yes (older daemon \u{2014} update it; chat is disabled from 0.46.0)\n",
        );
    } else {
        o.push_str(concat!(
            "  Public chat: no  (unavailable \u{2014} coming back with the ",
            product_name!(),
            " runtime)\n"
        ));
    }
    match publ {
        Some(p) => o.push_str(&format!(
            "  Published:  {}  (nested subdomain label)\n",
            py::str_of(p)
        )),
        None => o.push_str("  Published:  \u{2014}\n"),
    }
    if let Some(g) = generated.as_ref().filter(|g| py::truthy(g)) {
        o.push_str(&format!("  Indexed:    {}\n", py::str_of(g)));
    }
    Ok(o)
}

/// `index`'s text reader.
pub fn index(raw: &[u8]) -> Shown {
    let d = match head(raw) {
        Ok(d) => d,
        Err(shown) => return shown,
    };
    let mut o = String::new();
    match index_lines(&d, &mut o) {
        Ok(()) => Shown::out(o),
        Err(r) => raised(o, String::new(), &r),
    }
}

/// `n.get("exists", True)` for each node (line `line`).
fn exists(n: &Value, line: u32) -> Result<bool, Raise> {
    Ok(py_get(n, "exists", line)?.is_none_or(py::truthy))
}

/// `(x.get("title") or x.get("id") or "").lower()`: the sort key.
fn sort_key(x: &Value) -> Result<String, Raise> {
    let k = first_of(x, &["title", "id"], 28)?;
    match k {
        None => Ok(String::new()),
        Some(Value::Str(t)) => Ok(py::str_of(&Value::Str(t.clone())).to_lowercase()),
        Some(other) => Err(Raise::inside(
            28,
            28,
            "<lambda>",
            format!(
                "AttributeError: '{}' object has no attribute 'lower'",
                py::type_name(other)
            ),
        )),
    }
}

/// `(n.get("id") or "")[:28]`, as `%s` prints it.
fn id_cut(n: &Value) -> Result<String, Raise> {
    match truthy(py_get(n, "id", 29)?) {
        None => Ok(String::new()),
        Some(Value::Str(t)) => Ok(t
            .code_points()
            .into_iter()
            .take(28)
            .map(|cp| py::str_of(&Value::Str(super::super::code_point(cp))))
            .collect()),
        Some(Value::Array(items)) => Ok(py::str_of(&Value::Array(
            items.iter().take(28).cloned().collect(),
        ))),
        Some(Value::Object(_)) => Err(Raise::at(29, "KeyError: slice(None, 28, None)")),
        Some(other) => Err(Raise::at(
            29,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

fn index_lines(d: &Value, o: &mut String) -> Result<(), Raise> {
    let empty = Value::Array(Vec::new());
    let nodes = or(py_get(d, "nodes", 13)?, &empty).clone();
    let links = or(py_get(d, "links", 14)?, &empty).clone();
    let mut present = Vec::new();
    for n in py_iter(&nodes, 15)? {
        if exists(&n, 15)? {
            present.push(n);
        }
    }
    let mut missing = 0usize;
    for n in py_iter(&nodes, 16)? {
        if !exists(&n, 16)? {
            missing += 1;
        }
    }
    let ws = first_of(d, &["workspacePath", "wikiRel"], 17)?.map_or_else(String::new, py::str_of);
    o.push_str(&format!("Wiki index  {ws}\n"));
    let generated =
        truthy(py_get(d, "generatedAt", 18)?).map_or("\u{2014}".to_string(), py::str_of);
    o.push_str(&format!("  Generated:  {generated}\n"));
    o.push_str(&format!(
        "  Notes:      {}  (+ {missing} missing stubs)\n",
        present.len()
    ));
    o.push_str(&format!("  Links:      {}\n", py_len(&links, 20)?));
    if present.is_empty() {
        o.push('\n');
        o.push_str(concat!(
            "  Empty vault. Seed with: ",
            cli_name!(),
            " wiki seed\n"
        ));
        o.push_str("  Or drop Markdown under .k2/wiki/\n");
        return Ok(());
    }
    o.push('\n');
    o.push_str(&format!("  {}  TITLE\n", super::super::ljust("ID", 28)));
    let mut keyed = Vec::with_capacity(present.len());
    for n in &present {
        keyed.push((sort_key(n)?, n));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, n) in keyed.iter().take(50) {
        let id = id_cut(n)?;
        let title = truthy(py_get(n, "title", 29)?).map_or_else(String::new, py::str_of);
        o.push_str(&format!("  {}  {title}\n", super::super::ljust(&id, 28)));
    }
    if present.len() > 50 {
        o.push_str(&format!(
            "  \u{2026} {} more (use --json)\n",
            present.len() - 50
        ));
    }
    Ok(())
}

/// `note`'s text reader.
pub fn note(raw: &[u8]) -> Shown {
    let d = match head(raw) {
        Ok(d) => d,
        Err(shown) => return shown,
    };
    let mut o = String::new();
    match note_lines(&d, &mut o) {
        Ok(()) => Shown::out(o),
        Err(r) => raised(o, String::new(), &r),
    }
}

fn note_lines(d: &Value, o: &mut String) -> Result<(), Raise> {
    let title = first_of(d, &["title", "id"], 13)?.map_or("?".to_string(), py::str_of);
    o.push_str(&format!("# {title}\n"));
    let empty = Value::Array(Vec::new());
    let tags = or(py_get(d, "tags", 15)?, &empty);
    if py::truthy(tags) {
        o.push_str(&format!("tags: {}\n", join(tags, ", ", 17)?));
    }
    o.push('\n');
    let body = match py_get(d, "body", 19)? {
        Some(b) if *b != Value::Null => b.clone(),
        _ => first_of(d, &["content", "markdown"], 21)?
            .cloned()
            .unwrap_or_else(|| s("")),
    };
    match &body {
        Value::Str(_) => {
            let text = py::str_of(&body);
            o.push_str(&text);
            if !text.ends_with('\n') {
                o.push('\n');
            }
            Ok(())
        }
        other => Err(Raise::no_attribute(22, other, "endswith")),
    }
}

/// `seed`'s reader.
pub fn seed(raw: &[u8]) -> Shown {
    let d = match head(raw) {
        Ok(d) => d,
        Err(shown) => return shown,
    };
    let mut o = String::new();
    match seed_lines(&d, &mut o) {
        Ok(()) => Shown::out(o),
        Err(r) => raised(o, String::new(), &r),
    }
}

fn seed_lines(d: &Value, o: &mut String) -> Result<(), Raise> {
    let mut created = first_of(d, &["created", "files"], 13)?
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    if matches!(created, Value::Str(_)) {
        created = Value::Array(vec![created]);
    }
    if py::truthy(&created) {
        o.push_str("Seeded wiki:\n");
        for f in py_iter(&created, 18)? {
            o.push_str(&format!("  + {}\n", py::str_of(&f)));
        }
    } else {
        o.push_str("Wiki already seeded (Home.md / _Index.md present).\n");
    }
    let wiki = first_of(d, &["wikiRel", "path"], 22)?.map_or(".k2/wiki".to_string(), py::str_of);
    o.push_str(&format!("  Vault: {wiki}\n"));
    o.push_str(concat!(
        "  View:  ",
        cli_name!(),
        " wiki index   or open View Wiki in the app\n"
    ));
    Ok(())
}

/// `serve`'s reader. `want`: `ENABLED=true`.
pub fn serve(raw: &[u8], want: bool) -> Shown {
    let d = match head(raw) {
        Ok(d) => d,
        Err(shown) => return shown,
    };
    let mut o = String::new();
    match serve_lines(&d, want, &mut o) {
        Ok(()) => Shown::out(o),
        Err(r) => raised(o, String::new(), &r),
    }
}

fn serve_lines(d: &Value, want: bool, o: &mut String) -> Result<(), Raise> {
    let enabled = if py_in(&s("enabled"), d, 14)? {
        py_get(d, "enabled", 14)?.is_some_and(py::truthy)
    } else {
        match py_get(d, "serving", 14)? {
            Some(v) => py::truthy(v),
            None => want,
        }
    };
    let port = py_get(d, "port", 15)?;
    let url = url_of(d, port, 16)?;
    if enabled {
        let at = url.as_ref().map_or_else(|| port_words(port), py::str_of);
        o.push_str(&format!("Wiki serve ON  {at}\n"));
        o.push_str(concat!(
            "  Bind is 127.0.0.1 only (read-only). Publish with: ",
            cli_name!(),
            " wiki publish [--label LABEL]\n"
        ));
        o.push_str(concat!(
            "  Public chat: OFF (unavailable \u{2014} coming back with the ",
            product_name!(),
            " runtime)\n"
        ));
    } else {
        o.push_str("Wiki serve OFF\n");
        if let Some(label) = first_of(d, &["publishedLabel", "published_label"], 23)? {
            o.push_str(concat!(
                "  Note: nested subdomain may still point here \u{2014} run ",
                cli_name!(),
                " wiki unpublish or\n"
            ));
            o.push_str(&format!(
                "  {} publish subdomain rm {}\n",
                cli_name!(),
                py::str_of(label)
            ));
        }
    }
    Ok(())
}

/// `chat off`'s reader.
pub fn chat_off(raw: &[u8]) -> Shown {
    let d = match head(raw) {
        Ok(d) => d,
        Err(shown) => return shown,
    };
    match py_get(&d, "publicChatEnabled", 13) {
        Err(r) => raised(String::new(), String::new(), &r),
        Ok(Some(v)) if py::truthy(v) => Shown::err(
            "Error: the daemon still reports public chat on (older daemon?) \u{2014} update it\n",
            Exit::Error,
        ),
        Ok(_) => Shown::out(concat!(
            "Wiki public chat OFF\n",
            "  Public chat is coming back with the ",
            product_name!(),
            " runtime.\n"
        )),
    }
}

/// `_wiki_apply_project`'s read of `/cli/workspace/resolve`: the path, as
/// `$(…)` holds what Python printed.
pub fn resolved(raw: &[u8]) -> String {
    let raw = text(raw);
    let raw = if raw.is_empty() {
        "{}".to_string()
    } else {
        raw
    };
    let d = loads(&raw).unwrap_or(Value::Object(Vec::new()));
    if !matches!(d, Value::Object(_)) {
        return String::new();
    }
    let mut out = first_of(&d, &["path", "projectPath", "project_path"], 8)
        .ok()
        .flatten()
        .map_or_else(String::new, py::str_of);
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// `unpublish`'s read of the serve status: the published label, as
/// `$(…)` holds what Python printed (nothing when it raised).
pub fn published_label(raw: &[u8]) -> String {
    let raw = text(raw);
    let raw = if raw.is_empty() {
        "{}".to_string()
    } else {
        raw
    };
    let d = loads(&raw).unwrap_or(Value::Object(Vec::new()));
    let Ok(label) = first_of(&d, &["publishedLabel", "published_label", "label"], 5) else {
        return String::new();
    };
    let mut out = label.map_or_else(String::new, py::str_of);
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// `unpublish`'s reader of the serve-off answer (its code is ignored).
pub fn serve_off(raw: &[u8]) -> Shown {
    let raw = text(raw);
    if blank(&raw) {
        return Shown::out("Wiki serve: (no daemon response \u{2014} continuing)\n");
    }
    let Ok(d) = loads(&raw) else {
        return Shown::out("Wiki serve: (unparseable daemon response \u{2014} continuing)\n");
    };
    if let Some(err) = error_of(&d) {
        return Shown::err(
            format!("Warning: serve off failed: {}\n", hint_of(err, false)),
            Exit::Ok,
        );
    }
    Shown::out("Wiki serve OFF\n")
}

/// `unpublish --json`'s line.
pub fn unpublished_json(label: &str) -> String {
    let hint = if label.is_empty() {
        format!("{} publish subdomain list", cli_name!())
    } else {
        format!("{} publish subdomain rm {label}", cli_name!())
    };
    let v = Value::Object(vec![
        (Text::from("ok"), Value::Bool(true)),
        (Text::from("serving"), Value::Bool(false)),
        (
            Text::from("publishedLabel"),
            if label.is_empty() {
                Value::Null
            } else {
                s(label)
            },
        ),
        (Text::from("hint"), s(&hint)),
    ]);
    format!("{}\n", dumps(&v, true))
}

/// `_wiki_err CODE HINT` with JSON asked for: Python's line.
pub fn err_json(code: &str, hint: &str) -> String {
    format!("{}\n", coded(code, hint))
}
