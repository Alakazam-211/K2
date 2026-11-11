//! The annotation verbs (what an agent types after `<host> doc <path>`): a thin parser over the
//! typed calls, so a verb and the call it names have the same effect by construction
//! ([`crate::session::BlobSession::run_annotation`]; their registry rows are
//! [`crate::parity::ANNOTATION_VERBS`]).
//!
//! ```text
//! comment (--doc | --node KEY [--version V] [--hint H]) --body TEXT [--mention PID]... [--proposal PATH]...
//! comment --delete ann-N [--n N]
//! reply ann-N --body TEXT [--mention PID]... [--proposal PATH]...
//! resolve ann-N [--reopen]
//! comments [--open | --resolved] [--version V] [--on KEY] [--since SEQ] [--after ann-N] [--limit N]
//! comments --log [ann-N] [--since SEQ] [--limit N]
//! reanchor ann-N (--doc | --node KEY [--version V] [--hint H])
//! ```

use super::{
    AnnotEvent, Attachment, NewAnchor, NewComment, Reanchored, Thread, ThreadId, ThreadPage,
    ThreadQuery, ANNOT_LOG_MAX,
};
use crate::parity::Api;
use crate::store::VersionId;
use crate::tools::ToolError;

/// One annotation verb, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnnotCall {
    Comment(NewComment),
    Reply {
        thread: ThreadId,
        body: String,
        mentions: Vec<String>,
        attachments: Vec<Attachment>,
    },
    Resolve(ThreadId),
    Reopen(ThreadId),
    Delete {
        thread: ThreadId,
        n: u32,
    },
    Comments(ThreadQuery),
    Log {
        thread: Option<ThreadId>,
        since: u64,
        limit: usize,
    },
    Reanchor {
        thread: ThreadId,
        to: NewAnchor,
    },
}

/// What an annotation verb returns.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnnotOutput {
    Thread(Thread),
    Page(ThreadPage),
    Log(Vec<AnnotEvent>),
    Reanchored(Reanchored),
}

/// The flags that take a value.
const VALUED: &[&str] = &[
    "--node",
    "--version",
    "--hint",
    "--body",
    "--mention",
    "--proposal",
    "--n",
    "--on",
    "--since",
    "--after",
    "--limit",
];
/// The flags that don't.
const SWITCHES: &[&str] = &[
    "--doc",
    "--delete",
    "--reopen",
    "--open",
    "--resolved",
    "--log",
];

struct Args {
    verb: String,
    values: Vec<(&'static str, String)>,
    switches: Vec<&'static str>,
    plain: Vec<String>,
}

fn usage(msg: impl Into<String>) -> ToolError {
    ToolError::usage(msg)
}

fn split(argv: &[String]) -> Result<Args, ToolError> {
    let Some((verb, rest)) = argv.split_first() else {
        return Err(usage("no verb"));
    };
    let mut a = Args {
        verb: verb.clone(),
        values: Vec::new(),
        switches: Vec::new(),
        plain: Vec::new(),
    };
    let mut it = rest.iter();
    while let Some(x) = it.next() {
        if !x.starts_with("--") {
            a.plain.push(x.clone());
            continue;
        }
        let (name, inline) = match x.split_once('=') {
            Some((n, v)) => (n, Some(v.to_owned())),
            None => (x.as_str(), None),
        };
        if let Some(f) = VALUED.iter().find(|f| **f == name) {
            let v = match inline {
                Some(v) => v,
                None => it
                    .next()
                    .cloned()
                    .ok_or_else(|| usage(format!("{f} needs a value")))?,
            };
            a.values.push((*f, v));
        } else if let Some(f) = SWITCHES.iter().find(|f| **f == name) {
            if inline.is_some() {
                return Err(usage(format!("{f} takes no value")));
            }
            a.switches.push(*f);
        } else {
            return Err(usage(format!("{} doesn't take {name}", a.verb)));
        }
    }
    Ok(a)
}

impl Args {
    fn one(&self, f: &str) -> Result<Option<&str>, ToolError> {
        let mut v = self.values.iter().filter(|(n, _)| *n == f);
        let first = v.next().map(|(_, x)| x.as_str());
        if v.next().is_some() {
            return Err(usage(format!("{f} given twice")));
        }
        Ok(first)
    }
    fn all(&self, f: &str) -> Vec<String> {
        self.values
            .iter()
            .filter(|(n, _)| *n == f)
            .map(|(_, v)| v.clone())
            .collect()
    }
    /// At most [`super::ANNOT_MENTIONS_MAX`] `--mention`s, refused before any is read.
    fn mention_cap(&self) -> Result<(), ToolError> {
        let n = self
            .values
            .iter()
            .filter(|(f, _)| *f == "--mention")
            .count();
        if n > super::ANNOT_MENTIONS_MAX {
            return Err(usage(format!(
                "at most {} --mention",
                super::ANNOT_MENTIONS_MAX
            )));
        }
        Ok(())
    }
    fn has(&self, f: &str) -> bool {
        self.switches.contains(&f)
    }
    /// Only these flags (and switches) may appear.
    fn only(&self, allowed: &[&str]) -> Result<(), ToolError> {
        let bad = self
            .values
            .iter()
            .map(|(n, _)| *n)
            .chain(self.switches.iter().copied())
            .find(|n| !allowed.contains(n));
        match bad {
            Some(n) => Err(usage(format!("{} doesn't take {n}", self.form()))),
            None => Ok(()),
        }
    }
    fn form(&self) -> String {
        crate::parity::verb_form(std::slice::from_ref(&self.verb))
    }
    fn num(&self, f: &str) -> Result<Option<u64>, ToolError> {
        self.one(f)?
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|_| usage(format!("{f}: not a number: {v:?}")))
            })
            .transpose()
    }
    fn thread(&self) -> Result<ThreadId, ToolError> {
        match self.plain.as_slice() {
            [t] => {
                ThreadId::parse(t).ok_or_else(|| usage(format!("not a thread id: {t:?} (ann-N)")))
            }
            [] => Err(usage(format!("{} needs a thread id (ann-N)", self.verb))),
            _ => Err(usage("one thread id")),
        }
    }
    fn no_plain(&self) -> Result<(), ToolError> {
        match self.plain.first() {
            Some(x) => Err(usage(format!("unexpected {x:?}"))),
            None => Ok(()),
        }
    }
    fn body(&self) -> Result<String, ToolError> {
        self.one("--body")?
            .map(str::to_owned)
            .ok_or_else(|| usage("--body TEXT"))
    }
    fn attachments(&self) -> Vec<Attachment> {
        self.all("--proposal")
            .into_iter()
            .map(Attachment::proposal)
            .collect()
    }
    fn anchor(&self) -> Result<NewAnchor, ToolError> {
        let node = self.one("--node")?;
        let version: Option<VersionId> = self.num("--version")?;
        let hint = self.one("--hint")?.map(str::to_owned);
        match (self.has("--doc"), node) {
            (true, None) if version.is_none() && hint.is_none() => Ok(NewAnchor::Doc),
            (true, None) => Err(usage("--doc takes no --version or --hint")),
            (false, Some(k)) => Ok(NewAnchor::Node {
                key: k.to_owned(),
                version,
                hint,
            }),
            _ => Err(usage("name one place: --doc, or --node KEY")),
        }
    }
}

impl AnnotCall {
    /// `argv` as an agent typed it (`argv[0]` the verb). Usage errors are exit 2.
    pub fn parse(argv: &[String]) -> Result<Self, ToolError> {
        let a = split(argv)?;
        const PLACE: &[&str] = &["--doc", "--node", "--version", "--hint"];
        match a.verb.as_str() {
            "comment" if a.has("--delete") => {
                a.only(&["--delete", "--n"])?;
                let n = a.num("--n")?.unwrap_or(0);
                let n = u32::try_from(n).map_err(|_| usage("--n: too large"))?;
                Ok(AnnotCall::Delete {
                    thread: a.thread()?,
                    n,
                })
            }
            "comment" => {
                a.only(&[PLACE, &["--body", "--mention", "--proposal"]].concat())?;
                a.mention_cap()?;
                a.no_plain()?;
                let mut c = NewComment::new(a.anchor()?, a.body()?);
                c.mentions = a.all("--mention");
                c.attachments = a.attachments();
                Ok(AnnotCall::Comment(c))
            }
            "reply" => {
                a.only(&["--body", "--mention", "--proposal"])?;
                a.mention_cap()?;
                Ok(AnnotCall::Reply {
                    thread: a.thread()?,
                    body: a.body()?,
                    mentions: a.all("--mention"),
                    attachments: a.attachments(),
                })
            }
            "resolve" => {
                a.only(&["--reopen"])?;
                let t = a.thread()?;
                Ok(if a.has("--reopen") {
                    AnnotCall::Reopen(t)
                } else {
                    AnnotCall::Resolve(t)
                })
            }
            "comments" if a.has("--log") => {
                a.only(&["--log", "--since", "--limit"])?;
                let thread = match a.plain.as_slice() {
                    [] => None,
                    _ => Some(a.thread()?),
                };
                let limit = a.num("--limit")?.map_or(ANNOT_LOG_MAX, |n| n as usize);
                Ok(AnnotCall::Log {
                    thread,
                    since: a.num("--since")?.unwrap_or(0),
                    limit,
                })
            }
            "comments" => {
                a.only(&[
                    "--open",
                    "--resolved",
                    "--version",
                    "--on",
                    "--since",
                    "--after",
                    "--limit",
                ])?;
                a.no_plain()?;
                let mut q = ThreadQuery::new();
                q.open = match (a.has("--open"), a.has("--resolved")) {
                    (true, true) => return Err(usage("--open or --resolved, not both")),
                    (true, false) => Some(true),
                    (false, true) => Some(false),
                    (false, false) => None,
                };
                q.version = a.num("--version")?;
                q.on = a.one("--on")?.map(str::to_owned);
                q.since = a.num("--since")?;
                q.after = match a.one("--after")? {
                    Some(t) => Some(
                        ThreadId::parse(t)
                            .ok_or_else(|| usage(format!("--after: not a thread id: {t:?}")))?,
                    ),
                    None => None,
                };
                q.limit = a.num("--limit")?.map(|n| n as usize);
                Ok(AnnotCall::Comments(q))
            }
            "reanchor" => {
                a.only(PLACE)?;
                Ok(AnnotCall::Reanchor {
                    thread: a.thread()?,
                    to: a.anchor()?,
                })
            }
            v => Err(usage(format!("{v:?} is not an annotation verb"))),
        }
    }

    /// The registry form it runs ([`crate::parity::ANNOTATION_VERBS`]).
    pub fn form(&self) -> &'static str {
        match self {
            AnnotCall::Comment(_) => "comment",
            AnnotCall::Reply { .. } => "reply",
            AnnotCall::Resolve(_) => "resolve",
            AnnotCall::Reopen(_) => "resolve --reopen",
            AnnotCall::Delete { .. } => "comment --delete",
            AnnotCall::Comments(_) => "comments",
            AnnotCall::Log { .. } => "comments --log",
            AnnotCall::Reanchor { .. } => "reanchor",
        }
    }

    /// The session call it makes.
    pub fn api(&self) -> Api {
        match self {
            AnnotCall::Comment(_) => Api::Comment,
            AnnotCall::Reply { .. } => Api::Reply,
            AnnotCall::Resolve(_) => Api::Resolve,
            AnnotCall::Reopen(_) => Api::Reopen,
            AnnotCall::Delete { .. } => Api::DeleteComment,
            AnnotCall::Comments(_) => Api::Comments,
            AnnotCall::Log { .. } => Api::AnnotationLog,
            AnnotCall::Reanchor { .. } => Api::Reanchor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &[&str]) -> Result<AnnotCall, ToolError> {
        AnnotCall::parse(&s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn verbs_parse_to_their_calls() {
        let c = p(&[
            "comment",
            "--node",
            "@n:4",
            "--version=3",
            "--body",
            "fix",
            "--mention",
            "bob",
        ])
        .unwrap();
        let AnnotCall::Comment(c) = c else { panic!() };
        assert_eq!(
            c.anchor,
            NewAnchor::Node {
                key: "@n:4".into(),
                version: Some(3),
                hint: None
            }
        );
        assert_eq!(
            (c.body.as_str(), c.mentions.as_slice()),
            ("fix", ["bob".to_owned()].as_slice())
        );
        assert_eq!(
            p(&["comment", "--delete", "ann-2", "--n", "1"]).unwrap(),
            AnnotCall::Delete {
                thread: ThreadId(2),
                n: 1
            }
        );
        assert_eq!(
            p(&["resolve", "ann-2", "--reopen"]).unwrap(),
            AnnotCall::Reopen(ThreadId(2))
        );
        assert_eq!(
            p(&["resolve", "2"]).unwrap(),
            AnnotCall::Resolve(ThreadId(2))
        );
        assert_eq!(
            p(&["comments", "--log", "ann-1", "--since", "4"]).unwrap(),
            AnnotCall::Log {
                thread: Some(ThreadId(1)),
                since: 4,
                limit: ANNOT_LOG_MAX
            }
        );
        let AnnotCall::Comments(q) =
            p(&["comments", "--open", "--on", "@n:1", "--after", "ann-3"]).unwrap()
        else {
            panic!()
        };
        assert_eq!(
            (q.open, q.on.as_deref(), q.after),
            (Some(true), Some("@n:1"), Some(ThreadId(3)))
        );
        assert_eq!(
            p(&["reanchor", "ann-1", "--doc"]).unwrap(),
            AnnotCall::Reanchor {
                thread: ThreadId(1),
                to: NewAnchor::Doc
            }
        );
        for (argv, form) in [
            (&["comment", "--doc", "--body", "x"][..], "comment"),
            (&["comment", "--delete", "ann-1"][..], "comment --delete"),
            (&["comments", "--log"][..], "comments --log"),
        ] {
            let call = p(argv).unwrap();
            assert_eq!(call.form(), form);
            let owned: Vec<String> = argv.iter().map(|x| (*x).to_owned()).collect();
            assert_eq!(
                crate::parity::verb_form(&owned),
                form,
                "the registry names it alike"
            );
        }
    }

    #[test]
    fn bad_verbs_are_usage_errors() {
        for argv in [
            &["comment", "--body", "x"][..],
            &["comment", "--doc", "--node", "@n:1", "--body", "x"],
            &["comment", "--doc"],
            &["comment", "--doc", "--body"],
            &["comment", "--doc", "--body", "x", "--wat"],
            &["comment", "--doc", "--body", "a", "--body", "b"],
            &["reply", "--body", "x"],
            &["reply", "ann-0", "--body", "x"],
            &["reply", "ann-x", "--body", "x"],
            &["resolve", "ann-1", "--body", "x"],
            &["comments", "--open", "--resolved"],
            &["comments", "--limit", "many"],
            &["comments", "ann-1"],
            &["reanchor", "ann-1"],
            &["comment", "--delete", "ann-1", "--n", "99999999999"],
            &["nope"],
        ] {
            let e = p(argv).unwrap_err();
            assert_eq!(e.exit(), crate::tools::exit::USAGE, "{argv:?}");
        }
    }
}
