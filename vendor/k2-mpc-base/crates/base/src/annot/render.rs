//! An annotation verb's output for [`crate::session::BlobSession::run_tool`]: one line a
//! thread and one a comment (text), or the same as JSON. Bodies are people's text, printed
//! as given on their own line; a deleted or purged body is `[deleted]`.

use super::{AnnotEvent, AnnotOutput, Comment, Reanchored, Thread, ThreadStatus};
use crate::anchor::AnchorTarget;
use crate::errors::Value;

fn target(t: &AnchorTarget) -> String {
    match t {
        AnchorTarget::Doc => "doc".to_owned(),
        AnchorTarget::Node { key } => key.to_string(),
    }
}

fn state(t: &Thread) -> String {
    match &t.resolution {
        Some(r) => match r.state.doubt() {
            Some(d) => format!("{} ({})", r.state.name(), d.name()),
            None => r.state.name().to_owned(),
        },
        None => "unresolved".to_owned(),
    }
}

fn thread_text(t: &Thread, out: &mut String) {
    let status = match &t.status {
        ThreadStatus::Open => "open".to_owned(),
        ThreadStatus::Resolved { by } => format!("resolved by {}", by.pid),
    };
    out.push_str(&format!(
        "{} {} {} {status} by {}\n",
        t.id,
        target(&t.anchor.target),
        state(t),
        t.created.pid
    ));
    if let Some(p) = &t.pending_anchor {
        out.push_str(&format!(
            "  proposed: {} (v{}) by {}\n",
            target(&p.to),
            p.at,
            p.by.pid
        ));
    }
    for c in &t.comments {
        match &c.body {
            Some(b) => out.push_str(&format!("  #{} {}: {}\n", c.n, c.by.pid, b)),
            None => out.push_str(&format!("  #{} {}: [deleted]\n", c.n, c.by.pid)),
        }
    }
}

fn event_text(e: &AnnotEvent, out: &mut String) {
    let n = e.comment.map(|n| format!(" #{n}")).unwrap_or_default();
    out.push_str(&format!(
        "{} {}{} {} by {}\n",
        e.seq,
        e.thread,
        n,
        e.op.name(),
        e.by.pid
    ));
}

/// The output as text.
pub fn text(o: &AnnotOutput) -> String {
    let mut out = String::new();
    match o {
        AnnotOutput::Thread(t) => thread_text(t, &mut out),
        AnnotOutput::Reanchored(Reanchored::Applied(t)) => thread_text(t, &mut out),
        AnnotOutput::Reanchored(Reanchored::Proposed(t)) => {
            out.push_str("proposed (a person confirms it)\n");
            thread_text(t, &mut out);
        }
        AnnotOutput::Page(p) => {
            for t in &p.threads {
                thread_text(t, &mut out);
            }
            if let Some(n) = p.next {
                out.push_str(&format!("more: --after {n}\n"));
            }
            out.push_str(&format!("head: {}\n", p.head));
        }
        AnnotOutput::Log(l) => {
            for e in l {
                event_text(e, &mut out);
            }
        }
    }
    out
}

fn comment_value(c: &Comment) -> Value {
    Value::obj([
        ("n", Value::from(c.n)),
        ("by", Value::from(c.by.pid.as_str())),
        ("name", Value::from(c.by.name.as_str())),
        (
            "on_behalf_of",
            c.by.on_behalf_of
                .as_deref()
                .map_or(Value::Null, Value::from),
        ),
        ("body", c.body.as_deref().map_or(Value::Null, Value::from)),
        (
            "mentions",
            Value::list(c.mentions.iter().map(String::as_str)),
        ),
        (
            "attachments",
            Value::list(c.attachments.iter().map(|a| a.target.as_str())),
        ),
        (
            "deleted_by",
            c.deleted
                .as_ref()
                .map_or(Value::Null, |d| Value::from(d.pid.as_str())),
        ),
    ])
}

fn thread_value(t: &Thread) -> Value {
    Value::obj([
        ("id", Value::from(t.id.to_string())),
        ("anchor", Value::from(target(&t.anchor.target))),
        ("state", Value::from(state(t))),
        (
            "resolved_by",
            match &t.status {
                ThreadStatus::Open => Value::Null,
                ThreadStatus::Resolved { by } => Value::from(by.pid.as_str()),
            },
        ),
        ("by", Value::from(t.created.pid.as_str())),
        (
            "proposed",
            t.pending_anchor
                .as_ref()
                .map_or(Value::Null, |p| Value::from(target(&p.to))),
        ),
        (
            "comments",
            Value::List(t.comments.iter().map(comment_value).collect()),
        ),
    ])
}

/// The output as JSON.
pub fn json(o: &AnnotOutput) -> Value {
    match o {
        AnnotOutput::Thread(t) | AnnotOutput::Reanchored(Reanchored::Applied(t)) => thread_value(t),
        AnnotOutput::Reanchored(Reanchored::Proposed(t)) => {
            Value::obj([("proposed", Value::from(true)), ("thread", thread_value(t))])
        }
        AnnotOutput::Page(p) => Value::obj([
            (
                "threads",
                Value::List(p.threads.iter().map(thread_value).collect()),
            ),
            (
                "next",
                p.next.map_or(Value::Null, |n| Value::from(n.to_string())),
            ),
            ("head", Value::from(p.head)),
        ]),
        AnnotOutput::Log(l) => Value::List(
            l.iter()
                .map(|e| {
                    Value::obj([
                        ("seq", Value::from(e.seq)),
                        ("thread", Value::from(e.thread.to_string())),
                        ("comment", e.comment.map_or(Value::Null, Value::from)),
                        ("op", Value::from(e.op.name())),
                        ("by", Value::from(e.by.pid.as_str())),
                    ])
                })
                .collect(),
        ),
    }
}
