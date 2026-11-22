//! `_zen_sign_print VERB AS_JSON RAW` (`cli/src/zen-guide-data.sh`): a
//! `widget/sign` or `widget/claim` answer. Exit 3 when only the person
//! can, 4 for no such widget, 1 for any other error.

use super::print::{compact, g, indented, is, or, s, t, text_of, Out};
use crate::exit::Exit;
use crate::families::Shown;
use crate::out::json::Value;
use crate::out::py;
use crate::out::pylib::json as pyjson;
use k2_client::{cli_name, product_name};

/// `madeBy.kind` of a built-in widget: the daemon's wire value, the same
/// in every brand.
fn built_in(kind: Option<&Value>) -> bool {
    matches!(kind, Some(Value::Str(t)) if t.as_str() == Some("k2"))
}

pub fn print(verb: &str, as_json: bool, raw: &[u8]) -> Shown {
    let raw = text_of(raw);
    let mut o = Out::default();
    let d = if super::print::blank(&raw) {
        None
    } else {
        pyjson::loads(&raw).ok()
    };
    let d = match d {
        Some(v @ Value::Object(_)) => v,
        _ => {
            o.e(concat!(
                "Error: no answer from the ",
                product_name!(),
                " daemon on this computer"
            ));
            return o.done(Exit::Error);
        }
    };
    if let Some(err) = g(&d, "error").filter(|e| !matches!(e, Value::Null)) {
        if as_json {
            o.o(compact(&d));
        }
        let code = match err {
            Value::Object(_) => g(err, "code").cloned().unwrap_or(Value::Null),
            other => other.clone(),
        };
        let msg = match g(&d, "message") {
            Some(m) if py::truthy(m) => py::str_of(m),
            _ => match g(err, "hint") {
                Some(h) if py::truthy(h) && matches!(err, Value::Object(_)) => py::str_of(h),
                _ => py::str_of(&code),
            },
        };
        let c = Some(&code);
        if is(c, "owner_only") || is(c, "zen_local_only") {
            if verb == "claim" {
                o.e(format!(
                    concat!(
                        "Only the person can {} this widget: click It's mine in Settings → Gardens → the Garden → Local Data, or run ",
                        cli_name!(),
                        " zen widget claim outside ",
                        product_name!(),
                        "."
                    ),
                    verb
                ));
            } else {
                o.e(format!("Error: {msg}"));
            }
            return o.done(Exit::Refused);
        }
        if is(c, "sign_needs_person") {
            if !as_json {
                o.e(format!("Error: {msg}"));
                if t(g(&d, "changes")) {
                    o.e(format!("The card shows: {}", s(g(&d, "changes"))));
                }
                if t(g(&d, "card")) {
                    o.e(format!(
                        concat!(
                            "Card: {} (",
                            product_name!(),
                            " tells you when the person answers)"
                        ),
                        s(g(&d, "card"))
                    ));
                }
            }
            return o.done(Exit::Refused);
        }
        o.e(format!("Error: {msg}"));
        let nf = is(c, "unknown_widget") || is(c, "not_found");
        return o.done(if nf { Exit::NotFound } else { Exit::Error });
    }
    if as_json {
        o.o(indented(&d));
        return o.done(Exit::Ok);
    }
    let w = s(g(&d, "widget"));
    if t(g(&d, "asked")) {
        if t(g(&d, "card")) {
            o.o(format!(
                concat!(
                    "Asked the person to sign {}: ",
                    product_name!(),
                    " put a card on your Thread{}."
                ),
                w,
                if t(g(&d, "waiting")) {
                    " (one was already waiting)"
                } else {
                    ""
                }
            ));
        } else if t(g(&d, "note")) {
            o.o(format!(
                concat!(
                    "You have no Thread with the person, so ",
                    product_name!(),
                    " left them an inbox note to sign {}."
                ),
                w
            ));
        } else {
            o.o(format!(
                concat!(
                    "No new card: ",
                    product_name!(),
                    " already asked the person about this version of {}."
                ),
                w
            ));
        }
        if t(g(&d, "changes")) {
            o.o(format!("The card shows: {}", s(g(&d, "changes"))));
        }
        return o.done(Exit::Ok);
    }
    let empty = Value::Object(Vec::new());
    let m = match g(&d, "madeBy") {
        Some(x) if py::truthy(x) => x,
        _ => &empty,
    };
    let kind = g(m, "kind");
    let who = if is(kind, "owner") {
        "you".to_string()
    } else if built_in(kind) {
        product_name!().to_string()
    } else {
        or(g(m, "handle"), "an agent")
    };
    let whose = if is(kind, "owner") || built_in(kind) {
        "your"
    } else {
        "its"
    };
    let has_vouched =
        matches!(&d, Value::Object(p) if p.iter().any(|(k, _)| k.as_str() == Some("vouched")));
    if verb == "claim" {
        o.o(format!(
            "It's yours: this version of {w} now reads and writes data with your rights."
        ));
    } else if has_vouched && !built_in(kind) {
        let where_ = if is(kind, "owner") {
            "in every Garden".to_string()
        } else {
            format!("in each Garden whose list has {who}")
        };
        o.o(format!(
            "{} {who}: this version of {w} runs with the Garden's data {where_}.",
            if t(g(&d, "stamped")) {
                "Signed as"
            } else {
                "Already signed by"
            }
        ));
    } else if built_in(kind) {
        o.o(format!(
            concat!(
                "{} is a built-in: it is ",
                product_name!(),
                "'s and needs no signing."
            ),
            w
        ));
    } else if t(g(&d, "stamped")) {
        o.o(format!(
            "Signed as {who}: this widget now reads and writes data with {whose} grants."
        ));
    } else {
        o.o(format!(
            "Already signed by {who} (the first signer of a version keeps it): this widget reads and writes data with {whose} grants."
        ));
    }
    if let Some(r @ Value::Object(_)) = g(&d, "review") {
        if t(g(r, "ask")) {
            let changes = or(g(r, "changes"), "");
            let changes = changes.trim_end_matches('.').replace(
                "Since it was signed, this widget",
                "changed since it was signed: it",
            );
            // This binary's name before the daemon's askTail (C5).
            let ask = crate::transport::permission::ask_of(r);
            o.o(format!(
                "Note: {w} {changes}. Ask the person to look: {}",
                s(Some(&ask))
            ));
        }
    }
    o.done(Exit::Ok)
}
