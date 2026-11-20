//! The steps for people only, on the vault socket after
//! `_vault_person_guard` (`cli/src/migrate.sh`, `cmd_vault`): the
//! recovery key's `show-once` and `rotate`, `rollback`, `backup` (and
//! `backup key`) and `restore`. Each confirm is
//! [`confirm::confirm_token`] (or the plan's, for a rollback root units
//! load); each secret a person types (the recovery words, a backup key)
//! goes only in a request body, never on a child's argv or in its
//! environment.

use super::py::{self, get, idx, is, items, left, or_list, render, s, t, Lines, R};
use super::verbs::{echo, kv, merge, obj, usage, Opts};
use super::Ctx;
use crate::confirm::{self, dumps};
use crate::exit::Exit;
use crate::families::Raise;
use crate::out;
use crate::out::json::{Text, Value};
use crate::prompt;
use k2_client::brand::{CLI_NAME as CLI, PRODUCT_NAME as PRODUCT};

/// `x="$(printf '%s' "$_SC_BODY" | python3 -c '<f>')"`: what the Python
/// prints, its last newlines dropped. A raise prints its traceback on
/// stderr and leaves it empty; the shell goes on.
fn py_text(body: &[u8], f: impl FnOnce(&Value) -> R<String>) -> String {
    match confirm::load(body).and_then(|v| f(&v)) {
        Ok(x) => x.trim_end_matches('\n').to_string(),
        Err(e) => {
            out::stderr_line(e.text().trim_end_matches('\n'));
            String::new()
        }
    }
}

/// `printf '%s' "$_SC_BODY" | python3 -c '…sys.exit(0 if <test> else 1)'`
/// as an `if` reads it: a raise prints its traceback and is false.
fn py_test(body: &[u8], f: impl FnOnce(&Value) -> R<bool>) -> bool {
    match confirm::load(body).and_then(|v| f(&v)) {
        Ok(b) => b,
        Err(e) => {
            out::stderr_line(e.text().trim_end_matches('\n'));
            false
        }
    }
}

/// `json.load(sys.stdin).get("recovery") == "confirmed"`.
fn recovery_confirmed(body: &[u8]) -> bool {
    py_test(body, |v| Ok(is(&get(v, "recovery")?, "confirmed")))
}

/// `tok="$(_vault_confirm_token …)" || exit 3`.
fn token(ctx: &mut Ctx, action: &str, id: &str, label: &str) -> Result<String, Exit> {
    match confirm::confirm_token(&mut ctx.confirm(), action, id, label) {
        Some(tok) => Ok(tok.trim_end_matches('\n').to_string()),
        None => Err(Exit::Refused),
    }
}

/// `_vault_sock_call GET <route>`, then `[ 200 ] || _vault_fail`.
fn sock_get(ctx: &mut Ctx, route: &str) -> Result<confirm::Split, Exit> {
    let r = ctx.sock(false, route, b"");
    if r.ok() {
        Ok(r)
    } else {
        Err(ctx.fail(&r))
    }
}

/// `_vault_sock_call POST <route> <body>`, then `[ 200 ] || _vault_fail`.
fn sock_post(ctx: &mut Ctx, route: &str, body: &[u8]) -> Result<confirm::Split, Exit> {
    let r = ctx.sock(true, route, body);
    if r.ok() {
        Ok(r)
    } else {
        Err(ctx.fail(&r))
    }
}

/// `_with_confirm_token <body> <tok>` of a body this module made.
fn with_token(body: &str, tok: &str) -> String {
    confirm::with_confirm_token(body, tok, None).unwrap_or_default()
}

/// `"%s" % x` of a `d[k]`.
fn at(v: &Value, k: &str) -> R<String> {
    Ok(s(&idx(v, k)?))
}

/// `k2 vault recovery show-once|rotate`: a new recovery key, shown once
/// on stderr, then three of its words typed back.
pub fn recovery(ctx: &mut Ctx, act: &str) -> Result<Exit, Exit> {
    ctx.person_guard()?;
    let st = sock_get(ctx, "status")?;
    let tok = if !recovery_confirmed(&st.body) {
        // No recovery key yet: this computer's own user check.
        token(ctx, "recovery-setup", "", "recovery key")?
    } else if act == "rotate" {
        token(ctx, "recovery-rotate", "", "recovery key")?
    } else {
        String::new()
    };
    let rb = if tok.is_empty() {
        "{}".to_string()
    } else {
        with_token("{}", &tok)
    };
    let r = sock_post(ctx, &format!("recovery/{act}"), rb.as_bytes())?;
    let rid = py_text(&r.body, |v| at(v, "id"));
    // The printer's stdout goes to stderr too (`>&2`): off a terminal
    // Python writes its traceback first and its buffered lines at exit.
    let shown = render(&r.body, phrase);
    if prompt::stderr_is_tty() {
        prompt::prompt(&shown.stdout);
        prompt::prompt(&shown.stderr);
    } else {
        prompt::prompt(&shown.stderr);
        prompt::prompt(&shown.stdout);
    }
    let mut words = Vec::new();
    for n in [3, 11, 20] {
        words.push(prompt::ask(&format!("Type word {n}: ")));
    }
    prompt::prompt("\x1b[2J\x1b[H");
    let body = dumps(&Value::Object(vec![
        kv("id", py::text(&rid)),
        kv(
            "words",
            Value::Array(
                words
                    .iter()
                    .map(|w| confirm::text_of_bytes(w.expose()))
                    .collect(),
            ),
        ),
    ]));
    drop(words);
    let body = prompt::Secret::new(body.into_bytes());
    sock_post(ctx, "recovery/confirm", body.expose())?;
    drop(body);
    Ok(echo(&format!(
        "recovery key confirmed. If it ever leaks: {CLI} vault recovery rotate, then {CLI} vault rekey."
    )))
}

/// The recovery key's printer: six rows of four numbered words.
fn phrase(v: &Value, l: &mut Lines) -> R<i32> {
    let ph = idx(v, "phrase")?;
    let w: Vec<Value> = match &ph {
        Value::Str(_) => s(&ph).split(' ').map(py::text).collect(),
        other => return Err(Raise::no_attribute(1, other, "split")),
    };
    l.p("");
    l.p(format!(
        "Your vault recovery key (write it down, keep it offline; {PRODUCT} keeps only the public half):"
    ));
    l.p("");
    for i in (0..24).step_by(4) {
        let mut row = Vec::new();
        for j in 0..4 {
            let Some(word) = w.get(i + j) else {
                return Err(Raise::at(1, "IndexError: list index out of range"));
            };
            row.push(format!("{:>2}. {}", i + j + 1, left(word, 9)));
        }
        l.p(format!("   {}", row.join("   ")));
    }
    l.p("");
    Ok(0)
}

/// `str.strip()` of what Python decodes `b` to: the white space at
/// either end taken off (a byte that isn't UTF-8 is never white space).
fn py_strip(b: &[u8]) -> &[u8] {
    let space_at = |i: usize| -> Option<usize> {
        let end = (i + 4).min(b.len());
        let c = b[i..end].utf8_chunks().next()?.valid().chars().next()?;
        crate::families::py_space(c).then_some(c.len_utf8())
    };
    let mut start = 0;
    while start < b.len() {
        match space_at(start) {
            Some(n) => start += n,
            None => break,
        }
    }
    let mut end = b.len();
    'back: while end > start {
        for n in 1..=4.min(end - start) {
            if std::str::from_utf8(&b[end - n..end]).is_ok() {
                let c = std::str::from_utf8(&b[end - n..end])
                    .ok()
                    .and_then(|x| x.chars().next());
                if c.is_some_and(crate::families::py_space) {
                    end -= n;
                    continue 'back;
                }
                break 'back;
            }
        }
        break;
    }
    &b[start..end]
}

/// `(p.get("version") or 0) > 0`.
fn above_zero(v: &Value) -> R<bool> {
    if !t(v) {
        return Ok(false);
    }
    match v {
        Value::Bool(b) => Ok(*b),
        Value::Int(d) => Ok(!d.starts_with('-')),
        Value::Float(f) => Ok(*f > 0.0),
        other => Err(Raise::at(
            1,
            format!(
                "TypeError: '>' not supported between instances of '{}' and 'int'",
                crate::out::py::type_name(other)
            ),
        )),
    }
}

/// `k2 vault rollback NAME --to <version>`: the value of an older
/// version, as a new version. Called only when `--to` is a number
/// Python's `int()` reads ([`super::verbs::native`]).
pub fn rollback(
    ctx: &mut Ctx,
    o: &Opts,
    name: &str,
    sj: &[(Text, Value)],
    nj: &[(Text, Value)],
) -> Result<Exit, Exit> {
    if name.is_empty() || o.to_version.is_empty() {
        return Err(usage(format!(
            "usage: {CLI} vault rollback NAME --to <version>"
        )));
    }
    let version = super::verbs::py_int(&o.to_version).unwrap_or_default();
    ctx.person_guard()?;
    let base = merge(&[sj, nj]);
    let sh = sock_get(ctx, &format!("show?{}", py::urlencode(&base)))?;
    let sid = py_text(&sh.body, |v| at(&idx(v, "secret")?, "id"));
    let placed = py_test(&sh.body, |v| {
        for p in items(&or_list(get(v, "placements")?))? {
            if is(&get(&p, "target")?, "systemd_cred") && above_zero(&get(&p, "version")?)? {
                return Ok(true);
            }
        }
        Ok(false)
    });
    let accept = Value::Bool(o.accept_agent);
    let tok = if placed {
        // Root units load it: the confirm names the target version, who
        // wrote it, and the units it re-places (bound to them).
        let plan = obj(vec![
            kv("id", py::text(&sid)),
            kv("version", Value::Int(version.clone())),
            kv("acceptAgentValue", accept.clone()),
        ]);
        match confirm::confirm_token_plan(&mut ctx.confirm(), "rollback", &plan, "rollbackPlan") {
            Some(tok) => tok,
            None => return Err(Exit::Refused),
        }
    } else {
        token(ctx, "rollback", &sid, name)?
    };
    let vj = vec![
        kv("version", Value::Int(version)),
        kv("acceptAgentValue", accept),
    ];
    let body = with_token(&obj(merge(&[sj, nj, &vj])), &tok);
    sock_post(ctx, "rollback", body.as_bytes())?;
    Ok(echo(&format!(
        "{name}: back to the value of v{} (as a new version)",
        o.to_version
    )))
}

/// `d="$(cd "$dir" 2>/dev/null && pwd)"`: the folder a logical `cd`
/// reaches, `Some(None)` when it can't be entered; `None` where the binary
/// is not sure what bash does ([`super::verbs::native`] hands those back).
pub fn folder(dir: &str) -> Option<Option<String>> {
    if dir.is_empty() || dir.starts_with('-') || dir.contains('\n') {
        return None;
    }
    if std::env::var_os("CDPATH").is_some() && !dir.starts_with('/') {
        return None;
    }
    let pwd = super::tools::pwd()?;
    let abs = if dir.starts_with('/') {
        dir.to_string()
    } else if pwd.ends_with('/') {
        format!("{pwd}{dir}")
    } else {
        format!("{pwd}/{dir}")
    };
    if abs.starts_with("//") {
        return None;
    }
    // Not a folder at all: `cd` fails whichever way it goes.
    if !std::fs::metadata(&abs).is_ok_and(|m| m.is_dir()) {
        return Some(None);
    }
    let to = crate::families::workspace::register::canonpath(&abs)?;
    Some(enterable(&to).then_some(to))
}

/// `chdir` may enter `p` (search permission on it).
#[cfg(unix)]
fn enterable(p: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(p) else {
        return false;
    };
    // SAFETY: `c` is a valid C string for the call.
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

#[cfg(not(unix))]
fn enterable(_p: &str) -> bool {
    false
}

/// `k2 vault backup <dir>` and `k2 vault backup key <age1…> | remove`.
pub fn backup(ctx: &mut Ctx, o: &Opts) -> Result<Exit, Exit> {
    let p = |i: usize| o.pos.get(i).cloned().unwrap_or_default();
    if p(0) == "key" {
        let key = p(1);
        if key.is_empty() {
            return Err(usage(format!(
                "usage: {CLI} vault backup key <age1…> | remove"
            )));
        }
        ctx.person_guard()?;
        if key == "remove" {
            // Bind the confirm to the key that's there now.
            let st = sock_get(ctx, "status")?;
            let bfp = py_text(&st.body, |v| {
                for h in items(&or_list(get(v, "holders")?))? {
                    if is(&get(&h, "kind")?, "backup") {
                        let f = get(&h, "fingerprint")?;
                        return Ok(if t(&f) { s(&f) } else { String::new() });
                    }
                }
                Ok(String::new())
            });
            if bfp.is_empty() {
                out::stderr_line("not_found: this vault has no backup key");
                return Err(Exit::Error);
            }
            let tok = token(ctx, "backup-key", &format!("remove:{bfp}"), "")?;
            let body = with_token(r#"{"remove": true}"#, &tok);
            let r = sock_post(ctx, "backup/recipient", body.as_bytes())?;
            return Ok(render(&r.body, |v, l| {
                l.p(format!(
                    "backup key {} removed; {}",
                    at(v, "removed")?,
                    at(v, "note")?
                ));
                Ok(0)
            })
            .print());
        }
        // No recovery key confirmed yet: a new key holder takes this
        // computer's own user check, bound to this key.
        let st = sock_get(ctx, "status")?;
        let action = if recovery_confirmed(&st.body) {
            "backup-key"
        } else {
            "backup-key-add"
        };
        let tok = token(ctx, action, &format!("add:{key}"), "")?;
        let body = with_token(&obj(vec![kv("recipient", py::text(&key))]), &tok);
        let r = sock_post(ctx, "backup/recipient", body.as_bytes())?;
        return Ok(render(&r.body, |v, l| {
            let replaced = if t(&get(v, "replaced")?) {
                format!(" (replaced {})", at(v, "replaced")?)
            } else {
                String::new()
            };
            l.p(format!(
                "backup key {} added{replaced}; a backup can now be opened with it on a new box",
                at(v, "fingerprint")?
            ));
            Ok(0)
        })
        .print());
    }
    let dir = p(0);
    if dir.is_empty() {
        return Err(usage(format!("usage: {CLI} vault backup <dir>")));
    }
    ctx.person_guard()?;
    let Some(Some(d)) = folder(&dir) else {
        return Err(usage(format!("usage: {dir} isn't a folder")));
    };
    let st = sock_get(ctx, "status")?;
    let btok = if recovery_confirmed(&st.body) {
        String::new()
    } else {
        // No recovery key yet: this computer's own user check, bound to
        // this folder.
        token(ctx, "backup-export-initial", &d, "")?
    };
    let body = with_token(&obj(vec![kv("destDir", py::text(&d))]), &btok);
    let r = sock_post(ctx, "backup", body.as_bytes())?;
    Ok(render(&r.body, |v, l| {
        l.p(format!("wrote {} ({})", at(v, "path")?, at(v, "note")?));
        Ok(0)
    })
    .print())
}

/// `k2 vault restore --from <dir> [--holder recovery|backup] [--dry-run]
/// [--replace]`: the recovery words or the backup key from stdin (no
/// echo on a terminal), only in the request body.
pub fn restore(ctx: &mut Ctx, o: &Opts) -> Result<Exit, Exit> {
    if o.from.is_empty() {
        return Err(usage(format!("usage: {CLI} vault restore --from <dir> [--holder recovery|backup] [--dry-run] [--replace]")));
    }
    ctx.person_guard()?;
    let h = if o.holder.is_empty() {
        "recovery"
    } else {
        o.holder.as_str()
    };
    let Some(Some(rd)) = folder(&o.from) else {
        return Err(usage(format!("usage: {} isn't a folder", o.from)));
    };
    if !std::path::Path::new(&format!("{rd}/vault.db")).is_file() {
        return Err(usage(format!("usage: no vault.db in {rd}")));
    }
    let secret = if prompt::stdin_is_tty() {
        prompt::ask_hidden(if h == "recovery" {
            "Recovery words (24, not shown): "
        } else {
            "Backup key (AGE-SECRET-KEY-1…, not shown): "
        })
    } else {
        prompt::read_all_stdin()
    };
    if secret.is_empty() {
        return Err(usage(
            "usage: no recovery words or backup key given".into(),
        ));
    }
    // `sys.stdin.read().strip()` of the value.
    let text = confirm::text_of_bytes(py_strip(secret.expose()));
    drop(secret);
    let field = if h == "recovery" {
        "phrase"
    } else {
        "identity"
    };
    let rb = obj(vec![
        kv("fromDir", py::text(&rd)),
        kv("holder", py::text(h)),
        kv("dryRun", Value::Bool(o.dry_run)),
        kv("replace", Value::Bool(o.replace)),
        kv(field, text),
    ]);
    let rb = if o.replace && !o.dry_run {
        let tok = token(ctx, "restore", "", "vault")?;
        with_token(&rb, &tok)
    } else {
        rb
    };
    let rb = prompt::Secret::new(rb.into_bytes());
    let r = sock_post(ctx, "restore", rb.expose())?;
    drop(rb);
    Ok(render(&r.body, |v, l| {
        if t(&get(v, "dryRun")?) {
            l.p(format!(
                "backup opens: vault {}, {} secret(s). Nothing changed; run again without --dry-run.",
                at(v, "vaultId")?,
                py::d(&idx(v, "secrets")?)?
            ));
        } else {
            l.p(format!(
                "restored vault {} ({} secret(s)). The previous vault file is kept at {}",
                at(v, "vaultId")?,
                py::d(&idx(v, "secrets")?)?,
                at(v, "previous")?
            ));
        }
        Ok(0)
    })
    .print())
}
