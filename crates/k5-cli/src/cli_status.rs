//! `k5 cli status [--json]`: who answers each command family, and how
//! often each fell through to the shell CLI.
//!
//! The families are the route table's rows ([`crate::route::TABLE`]),
//! each with who answers it: the binary (`native`), the binary from its
//! `--schema` entry (`spec`), or the shell CLI (`bash`). For the owner it
//! adds the fallthrough counts per noun from `GET /cli/cli-usage`; the
//! daemon counts a noun under the word that was typed, so a row's count
//! is the sum over its words. A counted noun that is no row's (`other`,
//! an unknown word) gets a line of its own, answered by nobody (`-`).
//!
//! Only the owner reads the counts. A passport gets a 403, and a call
//! with no credential is not made: the table comes without counts and one
//! stderr line says they are the owner's. A daemon that can't be asked
//! gives the table and its one error line. Exit 0 in every case: the
//! table is the answer. The call goes through the transport, so a server
//! of another version gets its one line too ([`crate::transport::skew`]).
//!
//! `--json` prints one document, with Python's `json.dumps(indent=2)`
//! layout ([`crate::out::json`]):
//!
//! ```text
//! {"version": "<binary>", "native": n, "spec": n, "bash": n,
//!  "counts": "shown" | "owner-only" | "unavailable",
//!  "families": [{"family": "doc|docs", "words": ["doc", "docs"],
//!                "owner": "bash" | "native" | "spec" | null,
//!                "fallthroughs": n | null,
//!                "verbs": [{"words": [...], "owner": "..."}]}]}
//! ```

use crate::out::json::{self, Options, Text, Value};
use crate::route::{Noun, Owner};
use crate::transport::{Auth, Call, Client, Error, Response};
use k2_client::brand::PRODUCT_NAME;
use std::collections::BTreeMap;

/// The owner's read of the counts.
pub const PATH: &str = "/cli/cli-usage";

/// Whether the counts came.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Counts {
    /// Fallthroughs per counted noun.
    Shown(BTreeMap<String, i64>),
    /// Not the owner: a 403, or no credential at all.
    OwnerOnly,
    /// The daemon couldn't be asked, or its answer couldn't be read: the
    /// stderr line.
    Unavailable(String),
}

/// The stderr line for counts that are not shown.
pub const OWNER_ONLY_LINE: &str = "the fallthrough counts are the owner's only";

/// One family's line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Family {
    pub words: Vec<String>,
    /// `None` for a counted noun that is no row's.
    pub owner: Option<Owner>,
    pub fallthroughs: Option<i64>,
    pub verbs: Vec<(Vec<String>, Owner)>,
}

impl Family {
    pub fn name(&self) -> String {
        self.words.join("|")
    }
}

/// The report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub version: String,
    pub families: Vec<Family>,
    pub counts: Counts,
}

pub fn owner_name(owner: Owner) -> &'static str {
    match owner {
        Owner::Native => "native",
        Owner::Spec => "spec",
        Owner::Bash => "bash",
    }
}

/// The fallthrough counts in a `/cli/cli-usage` body
/// (`{"rows": [{"kind", "key", "count", ...}]}`): per key, the
/// `fallthrough` rows' counts summed. `None` when it is not that shape.
pub fn fallthroughs(body: &[u8]) -> Option<BTreeMap<String, i64>> {
    let doc = json::parse(std::str::from_utf8(body).ok()?).ok()?;
    let field = |v: &Value, key: &str| -> Option<Value> {
        let Value::Object(pairs) = v else { return None };
        pairs.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v.clone())
    };
    let Value::Array(rows) = field(&doc, "rows")? else {
        return None;
    };
    let mut out = BTreeMap::new();
    for row in &rows {
        let (Value::Str(kind), Value::Str(key), Value::Int(count)) = (field(row, "kind")?, field(row, "key")?, field(row, "count")?) else {
            return None;
        };
        if kind.as_str() != Some(FALLTHROUGH_KIND) {
            continue;
        }
        let n: i64 = count.parse().ok()?;
        *out.entry(key.as_str()?.to_string()).or_insert(0) += n;
    }
    Some(out)
}

/// The daemon's name for the fallthrough kind (`k2_core::cli_usage`).
pub const FALLTHROUGH_KIND: &str = "fallthrough";

/// What the `/cli/cli-usage` call came to.
pub fn counts(result: Result<Response, Error>) -> Counts {
    let r = match result {
        Ok(r) => r,
        Err(Error::NoCredential { .. }) => return Counts::OwnerOnly,
        Err(e) => return Counts::Unavailable(e.to_string()),
    };
    match r.status {
        200 => match fallthroughs(&r.body) {
            Some(map) => Counts::Shown(map),
            None => Counts::Unavailable(format!("{PRODUCT_NAME}'s server's answer to {PATH} is not its rows")),
        },
        403 => Counts::OwnerOnly,
        other => Counts::Unavailable(format!("{PRODUCT_NAME}'s server answered {PATH} with {other}")),
    }
}

/// Asks for the counts: only with a credential, as `cli_request` does.
pub fn fetch(client: &mut Client) -> Counts {
    if client.auth == Auth::None {
        return Counts::OwnerOnly;
    }
    counts(client.send(PATH, Call::Request(&[])))
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|w| w.to_string()).collect()
}

/// The report from `table`, the binary's version and the counts.
pub fn status(table: &[Noun], version: &str, counts: Counts) -> Status {
    let shown = match &counts {
        Counts::Shown(map) => Some(map),
        _ => None,
    };
    let mut families: Vec<Family> = table
        .iter()
        .map(|n| Family {
            words: words(n.words),
            owner: Some(n.owner),
            fallthroughs: shown.map(|m| n.words.iter().filter_map(|w| m.get(*w)).sum()),
            verbs: n.verbs.iter().map(|v| (words(v.words), v.owner)).collect(),
        })
        .collect();
    families.sort_by_key(Family::name);
    if let Some(map) = shown {
        for (key, n) in map {
            if !table.iter().any(|row| row.names(key)) {
                families.push(Family { words: vec![key.clone()], owner: None, fallthroughs: Some(*n), verbs: Vec::new() });
            }
        }
    }
    Status { version: version.to_string(), families, counts }
}

impl Status {
    fn tally(&self, owner: Owner) -> usize {
        self.families.iter().filter(|f| f.owner == Some(owner)).count()
    }

    /// The summary line.
    pub fn summary(&self) -> String {
        format!(
            "k5 {}: {} native, {} spec, {} bash",
            self.version,
            self.tally(Owner::Native),
            self.tally(Owner::Spec),
            self.tally(Owner::Bash)
        )
    }

    /// The table, aligned: the summary, a header, a line per family (and
    /// per verb row under it). The count column only with counts.
    pub fn table(&self) -> String {
        let with_counts = matches!(self.counts, Counts::Shown(_));
        let mut rows: Vec<[String; 3]> = vec![["family".into(), "by".into(), "fallthroughs".into()]];
        for f in &self.families {
            let count = f.fallthroughs.map_or_else(String::new, |n| n.to_string());
            rows.push([f.name(), f.owner.map_or("-", owner_name).into(), count]);
            for (verb, owner) in &f.verbs {
                rows.push([format!("{} {}", f.words[0], verb.join("|")), owner_name(*owner).into(), String::new()]);
            }
        }
        let w0 = rows.iter().map(|r| r[0].chars().count()).max().unwrap_or(0);
        let w1 = rows.iter().map(|r| r[1].chars().count()).max().unwrap_or(0);
        let mut out = format!("{}\n", self.summary());
        for r in &rows {
            let line = if with_counts { format!("{:w0$}  {:w1$}  {}", r[0], r[1], r[2]) } else { format!("{:w0$}  {}", r[0], r[1]) };
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    /// The JSON document, with its trailing newline.
    pub fn json(&self) -> String {
        let s = |t: &str| Value::Str(Text::from(t));
        let int = |n: usize| Value::Int(n.to_string());
        let list = |ws: &[String]| Value::Array(ws.iter().map(|w| s(w)).collect());
        let families = self
            .families
            .iter()
            .map(|f| {
                obj(vec![
                    ("family", s(&f.name())),
                    ("words", list(&f.words)),
                    ("owner", f.owner.map_or(Value::Null, |o| s(owner_name(o)))),
                    ("fallthroughs", f.fallthroughs.map_or(Value::Null, |n| Value::Int(n.to_string()))),
                    (
                        "verbs",
                        Value::Array(f.verbs.iter().map(|(w, o)| obj(vec![("words", list(w)), ("owner", s(owner_name(*o)))])).collect()),
                    ),
                ])
            })
            .collect();
        let counts = match self.counts {
            Counts::Shown(_) => "shown",
            Counts::OwnerOnly => "owner-only",
            Counts::Unavailable(_) => "unavailable",
        };
        let doc = obj(vec![
            ("version", s(&self.version)),
            ("native", int(self.tally(Owner::Native))),
            ("spec", int(self.tally(Owner::Spec))),
            ("bash", int(self.tally(Owner::Bash))),
            ("counts", s(counts)),
            ("families", Value::Array(families)),
        ]);
        let options = Options { indent: Some(2), ..Options::default() };
        let mut text = json::to_string(&doc, &options).expect("plain ASCII and UTF-8 strings always print");
        text.push('\n');
        text
    }

    /// The stderr line, when the counts are not shown.
    pub fn note(&self) -> Option<String> {
        match &self.counts {
            Counts::Shown(_) => None,
            Counts::OwnerOnly => Some(OWNER_ONLY_LINE.to_string()),
            Counts::Unavailable(line) => Some(line.clone()),
        }
    }
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `k5 cli --help`.
pub const HELP: &str = "\
Usage: k5 cli status [--json]

  status    Which command families the k5 binary answers itself (native),
            from their --schema entry (spec) or hands to the shell CLI
            (bash), and its version. For the owner, how many calls of each
            family fell through to the shell CLI (GET /cli/cli-usage).
  --json    One JSON document instead of the table.

Answered by the k5 binary only; the shell CLI's `cli` points here.
New k5 command (Beta): off by default; the owner turns it on in Settings,
General, Server, Advanced. Not on Windows yet.
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{Verb, TABLE};
    use crate::transport::Channel;

    static T: &[Noun] = &[
        Noun { words: &["doc", "docs"], owner: Owner::Bash, verbs: &[] },
        Noun { words: &["mail"], owner: Owner::Bash, verbs: &[Verb { words: &["read"], owner: Owner::Native }] },
        Noun { words: &["cli"], owner: Owner::Native, verbs: &[] },
    ];

    fn shown(pairs: &[(&str, i64)]) -> Counts {
        Counts::Shown(pairs.iter().map(|(k, n)| (k.to_string(), *n)).collect())
    }

    fn ok(status: u16, body: &str) -> Result<Response, Error> {
        Ok(Response { status, body: body.as_bytes().to_vec(), channel: Channel::Tcp, permission: None, server_version: None })
    }

    #[test]
    fn the_rows_counts_are_summed_per_key_and_other_kinds_left_out() {
        let body = r#"{"rows":[
            {"kind":"fallthrough","key":"docs","count":2,"first_at":1,"last_at":2},
            {"kind":"fallthrough","key":"doc","count":3,"first_at":1,"last_at":2},
            {"kind":"would_refuse","key":"ws-1","count":7,"first_at":1,"last_at":2},
            {"kind":"fallthrough","key":"other","count":1,"first_at":1,"last_at":2}]}"#;
        assert_eq!(counts(ok(200, body)), shown(&[("doc", 3), ("docs", 2), ("other", 1)]));
        assert_eq!(counts(ok(200, r#"{"rows":[]}"#)), shown(&[]));
        for bad in ["[]", r#"{"rows":{}}"#, r#"{"rows":[{"kind":"fallthrough","key":"x","count":"1"}]}"#, "nope"] {
            assert_eq!(counts(ok(200, bad)), Counts::Unavailable("K2's server's answer to /cli/cli-usage is not its rows".into()), "{bad}");
        }
    }

    #[test]
    fn a_403_or_no_credential_is_owner_only() {
        assert_eq!(counts(ok(403, r#"{"error":"only the owner can read the CLI usage counts"}"#)), Counts::OwnerOnly);
        assert_eq!(counts(Err(Error::NoCredential { endpoint: PATH.into() })), Counts::OwnerOnly);
        assert_eq!(counts(ok(500, "")), Counts::Unavailable("K2's server answered /cli/cli-usage with 500".into()));
        let mut c = Client::new("127.0.0.1", 1, "/p", Auth::None);
        assert_eq!(fetch(&mut c), Counts::OwnerOnly);
    }

    #[test]
    fn the_table_with_counts() {
        let s = status(T, "0.46.16", shown(&[("docs", 2), ("doc", 3), ("nosuch", 4)]));
        assert_eq!(
            s.table(),
            "k5 0.46.16: 1 native, 0 spec, 2 bash\n\
             family     by      fallthroughs\n\
             cli        native  0\n\
             doc|docs   bash    5\n\
             mail       bash    0\n\
             mail read  native\n\
             nosuch     -       4\n"
        );
        assert_eq!(s.note(), None);
    }

    #[test]
    fn the_table_without_counts_and_its_line() {
        let s = status(T, "0.46.16", Counts::OwnerOnly);
        assert_eq!(
            s.table(),
            "k5 0.46.16: 1 native, 0 spec, 2 bash\nfamily     by\ncli        native\ndoc|docs   bash\nmail       bash\nmail read  native\n"
        );
        assert_eq!(s.note().as_deref(), Some(OWNER_ONLY_LINE));
        assert_eq!(status(T, "1", Counts::Unavailable("down".into())).note().as_deref(), Some("down"));
    }

    #[test]
    fn the_json_matches_the_table() {
        let s = status(T, "0.46.16", shown(&[("doc", 1)]));
        let text = s.json();
        assert!(text.ends_with("}\n"));
        let v = json::parse(&text).unwrap();
        // Python's indent=2 layout.
        assert!(text.starts_with("{\n  \"version\": \"0.46.16\",\n  \"native\": 1,\n"), "{text}");
        assert_eq!(json::reprint(&text, &Options { indent: Some(2), ..Options::default() }).unwrap() + "\n", text);
        let Value::Object(top) = v else { panic!() };
        let get = |k: &str| top.iter().find(|(n, _)| n.as_str() == Some(k)).unwrap().1.clone();
        assert_eq!(get("counts"), Value::Str("shown".into()));
        let Value::Array(fams) = get("families") else { panic!() };
        assert_eq!(fams.len(), s.families.len());
        let names: Vec<String> = fams
            .iter()
            .map(|f| {
                let Value::Object(p) = f else { panic!() };
                let Value::Str(n) = &p[0].1 else { panic!() };
                n.as_str().unwrap().to_string()
            })
            .collect();
        assert_eq!(names, ["cli", "doc|docs", "mail"]);
        let s = status(T, "0.46.16", Counts::OwnerOnly);
        assert!(s.json().contains("\"counts\": \"owner-only\""));
        assert!(s.json().contains("\"fallthroughs\": null"));
    }

    #[test]
    fn the_real_table_has_every_row_once() {
        let s = status(TABLE, "x", Counts::OwnerOnly);
        assert_eq!(s.families.len(), TABLE.len());
        assert!(s.families.iter().any(|f| f.words == ["cli"] && f.owner == Some(Owner::Native)));
    }
}
