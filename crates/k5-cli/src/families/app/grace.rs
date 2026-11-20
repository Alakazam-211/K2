//! `_app_grace_done` (`cli/src/app-grace.sh`): a change the daemon held
//! for the owner (202) is its lines and exit 2; anything but 200 is
//! `_compute_fail`. Shared by the verbs that post a change an agent may
//! only ask for (`app grant <app> --kind guests-rooms`, `app grant
//! copy-host`).

use super::call::{Ctx, Split};
use super::py::{self, Out};
use super::wallet::raw;
use super::Answer;
use crate::exit::Exit;
use crate::families::Raise;
use crate::out::json::Value;

/// `_app_grace_done "$json_out"`, then `[ "$_CP_STATUS" = "200" ] ||
/// _compute_fail`: `Ok(body)` for a 200, else the answer the command ends
/// with.
pub fn done(ctx: &Ctx, s: Split, json: bool) -> Result<Vec<u8>, Answer> {
    if s.status == "202" {
        if json {
            let mut shown = raw(&s.body);
            shown.exit = Exit::Usage;
            return Err(Ok(shown));
        }
        let mut o = Out::default();
        let r = held_lines(&s.body, &mut o);
        // The printer's raise is its exit 1 (set -e); else exit 2.
        return Err(Ok(o.done(r.map(|()| Exit::Usage))));
    }
    if s.ok() {
        Ok(s.body)
    } else {
        Err(Err(ctx.fail(&s)))
    }
}

fn is_dict(v: &Value) -> bool {
    matches!(v, Value::Object(_))
}

/// The held answer's Python, line for line.
fn held_lines(body: &[u8], o: &mut Out) -> Result<(), Raise> {
    let v = py::load(body, 3)?;
    let applied = py::or(py::dget(&v, "applied", 4)?, py::empty_list());
    for line in py::iter(&applied, 4)? {
        o.line(&format!("applied: {}", py::s(&line)));
    }
    let mut helds: Vec<Value> = [py::dget(&v, "held", 6)?, py::dget(&v, "egressHeld", 6)?]
        .into_iter()
        .filter(is_dict)
        .collect();
    if helds.is_empty() {
        helds.push(v.clone());
    }
    for h in &helds {
        let r = py::dget(h, "request", 8)?;
        let r = if is_dict(&r) {
            r
        } else {
            Value::Object(Vec::new())
        };
        o.line(&format!(
            "held for the owner: {}  {}",
            py::s(&py::dget(&r, "id", 9)?),
            py::s(&py::dget_or(&r, "summary", py::text(""), 9)?)
        ));
        let mut d = py::dget(h, "diff", 10)?;
        if is_dict(&d) {
            d = py::dget(&d, "lines", 12)?;
        }
        if let Value::Array(lines) = &d {
            for line in lines {
                o.line(&format!("  {}", py::s(line)));
            }
        }
        let reasons = py::or(py::dget(h, "reasons", 15)?, py::empty_list());
        for why in py::iter(&reasons, 15)? {
            o.line(&format!("  why: {}", py::s(&why)));
        }
        if py::truthy(&py::dget(h, "reused", 19)?) {
            let ago = py::or(py::dget(h, "askedAgo", 20)?, py::text("a while"));
            o.line(&format!(
                "  this is your earlier request {} (asked {} ago), still pending; nothing new was filed",
                py::s(&py::dget(&r, "id", 20)?),
                py::s(&ago)
            ));
        }
        let c = py::dget(h, "card", 21)?;
        let c = if is_dict(&c) {
            c
        } else {
            Value::Object(Vec::new())
        };
        let where_ = py::dget(&c, "where", 22)?;
        if py::truthy(&where_) {
            o.line(&format!("  owner: {}", py::s(&py::idx(&c, "where", 23)?)));
        }
        let err = py::dget(&c, "error", 24)?;
        if py::truthy(&err) {
            let e = py::idx(&c, "error", 24)?;
            let said = py::or(py::dget(&e, "hint", 24)?, py::dget(&e, "code", 24)?);
            if py::truthy(&said) {
                let e = py::idx(&c, "error", 25)?;
                let said = py::or(py::dget(&e, "hint", 25)?, py::dget(&e, "code", 25)?);
                o.line(&format!("  card not raised: {}", py::s(&said)));
            }
        }
        o.line(&py::s(&py::dget_or(h, "hint", py::text(""), 26)?));
        if py::truthy(&py::dget(h, "afterApproval", 28)?) {
            o.line(&py::s(&py::idx(h, "afterApproval", 29)?));
        }
        if py::truthy(&py::dget(h, "passwordNotUsed", 30)?) {
            let note = py::or(
                py::dget(h, "passwordNote", 31)?,
                py::text("the password you sent was not used"),
            );
            o.stderr.push_str(&format!("note: {}\n", py::s(&note)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(body: &str) -> Result<String, String> {
        let mut o = Out::default();
        match held_lines(body.as_bytes(), &mut o) {
            Ok(()) => Ok(o.stdout),
            Err(e) => Err(e.exception),
        }
    }

    #[test]
    fn the_held_answer_reads_as_the_shell_prints_it() {
        assert_eq!(
            lines(r#"{"request":{"id":"r1","summary":"rooms"},"hint":"ask"}"#).unwrap(),
            "held for the owner: r1  rooms\nask\n"
        );
        assert_eq!(
            lines(r#"{"applied":["a"],"held":{"request":{"id":"r2"},"diff":{"lines":["+x"]},"reasons":["why"],"reused":true,"card":{"where":"Thread","error":{"code":"c"}}}}"#)
                .unwrap(),
            "applied: a\nheld for the owner: r2  \n  +x\n  why: why\n  this is your earlier request r2 (asked a while ago), still pending; nothing new was filed\n  owner: Thread\n  card not raised: c\n\n"
        );
        assert_eq!(
            lines("[]").unwrap_err(),
            "AttributeError: 'list' object has no attribute 'get'"
        );
        assert_eq!(
            lines(r#"{"card":{"error":"x"}}"#).unwrap_err(),
            "AttributeError: 'str' object has no attribute 'get'"
        );
    }
}
