//! The structure index's parity cases (A2, A5): for a pack with an index, its list, show,
//! diff and blame verbs (and `outline`, `node`) and the node claims, each run as a person (a
//! host's view: the typed calls) and as an agent (the registered verbs, through
//! [`crate::session::BlobSession::run_tool`]), with the same output byte for byte.
//!
//! The kit can't write a file of the pack's type, so the pack hands two versions of one:
//! [`kit_index_cases`]`(verbs, first, second)`, passed to [`super::check_blob`] as `extra`.
//! Each needs at least one node; `second` should change something (the diff and blame cases
//! show it). The export verb's case is the pack's (its exporter writes the bytes).

use super::action::PersonAction as P;
use super::blob::{BlobAgent, BlobAgentFn, BlobCase, BlobPerson, BlobPersonFn, BlobStage};
use super::harness::{Output, Step, VerbOut, VerbValue};
use crate::index::{IndexVerbs, ListQuery, NodeKey};
use crate::session::ConnId;
use crate::tools;

/// The cases for a pack whose index has `verbs`, on `first` then `second` (two versions of a
/// file of its type). Pass them to [`super::check_blob`] as `extra`, with the registry
/// [`super::Registry::for_blob_pack`] builds and a parity list whose P3, P8, P19 and P22 rows
/// name these verbs.
pub fn kit_index_cases(
    verbs: &'static IndexVerbs,
    first: &'static [u8],
    second: &'static [u8],
) -> Vec<BlobCase> {
    let c = |action: P,
             verb: &'static str,
             name: &'static str,
             person: BlobPersonFn,
             agent: BlobAgentFn| {
        BlobCase::new(action, verb, name, person, agent)
            .initial(first)
            .then(second)
            .setup(indexed)
    };
    vec![
        c(
            P::ReadAll,
            verbs.list,
            "the outline, as JSON",
            list_person,
            list_agent,
        ),
        c(
            P::ReadAll,
            "outline",
            "the outline by its kit name",
            list_person,
            outline_agent,
        ),
        c(
            P::ReadAll,
            verbs.show,
            "the first node and its children",
            show_person,
            show_agent,
        ),
        c(
            P::ReadAll,
            "node",
            "a node by the kit's name",
            show_person,
            node_agent,
        ),
        c(
            P::ReadAll,
            verbs.show,
            "a missing address: exit 4, every address listed",
            partial_person,
            partial_agent,
        ),
        c(
            P::History,
            verbs.diff,
            "node by node, version 1 to 2",
            diff_person,
            diff_agent,
        )
        .setup(two_versions),
        c(
            P::Blame,
            verbs.blame,
            "per node, after another's version",
            blame_person,
            blame_agent,
        )
        .setup(two_versions),
        c(
            P::Caret,
            "claim",
            "claim the first node",
            claim_person,
            claim_agent,
        ),
        c(
            P::Caret,
            "claim",
            "someone else's claim there: exit 5",
            claim_person,
            claim_agent,
        )
        .setup(claimed_by_other),
        c(
            P::Caret,
            "claim --renew",
            "renew my claim",
            renew_person,
            renew_agent,
        ),
        c(
            P::Caret,
            "claim --release",
            "release my claim",
            release_person,
            release_agent,
        ),
    ]
}

// ----- setups -----

fn indexed(st: &mut BlobStage) -> Result<(), String> {
    st.index_all()
}

fn two_versions(st: &mut BlobStage) -> Result<(), String> {
    let v1 = st.version()?;
    st.set_mark("v1", v1);
    let v2 = st.write_next_as("sam")?;
    st.set_mark("v2", v2);
    st.advance(1_000);
    st.index_all()
}

/// The key of node `#1` in the current version, as `conn` reads it.
fn first_key(st: &BlobStage, conn: ConnId) -> Result<NodeKey, String> {
    let o = st
        .session()
        .index_list(conn, &ListQuery::new())
        .map_err(|e| format!("outline: {e}"))?;
    o.nodes
        .first()
        .map(|n| n.key.clone())
        .ok_or_else(|| "the file has no node".to_owned())
}

fn claimed_by_other(st: &mut BlobStage) -> Result<(), String> {
    st.index_all()?;
    let c = st.other("sam")?;
    let k = first_key(st, c)?;
    st.session()
        .claim_nodes(c, &[k], None, Some("sam is on it".into()))
        .map_err(|e| format!("sam's claim: {e}"))?;
    Ok(())
}

// ----- helpers -----

fn verbs(a: &mut BlobAgent<'_>) -> Result<&'static IndexVerbs, String> {
    a.stage()
        .index_verbs()
        .ok_or_else(|| "the pack has no index".to_owned())
}

/// `name` split into its words, then `rest`.
fn argv(name: &str, rest: &[&str]) -> Vec<String> {
    name.split(' ')
        .chain(rest.iter().copied())
        .map(str::to_owned)
        .collect()
}

fn run(a: &mut BlobAgent<'_>, argv: &[String]) -> Result<VerbOut, String> {
    let v: Vec<&str> = argv.iter().map(String::as_str).collect();
    a.verb(&v, b"")
}

/// A verb's stdout as text, with its exit.
fn text_step(out: &VerbOut) -> Result<Step, String> {
    match &out.value {
        VerbValue::Tool(t) => Ok(Step::exit(
            out.exit,
            Output::Text(unkeyed(&String::from_utf8_lossy(&t.stdout))),
        )),
        VerbValue::None if out.refused.is_some() => Ok(Step::exit(out.exit, Output::None)),
        v => Err(format!("{}: not a tool's output: {v:?}", out.verb)),
    }
}

/// `json` with each keyed hash's value blanked: hashes are keyed per space, and the two paths
/// run in two stores (as a blob id differs between them, `super::blob`).
fn unkeyed(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    let mut rest = json;
    loop {
        let at = ["\"record_hash\":\"", "\"content_hash\":\""]
            .iter()
            .filter_map(|m| rest.find(m).map(|i| i + m.len()))
            .min();
        let Some(at) = at else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..at]);
        let end = rest[at..].find('"').map_or(rest.len(), |e| at + e);
        out.push_str("(keyed)");
        rest = &rest[end..];
    }
}

fn person_err(e: crate::session::SessionError) -> String {
    format!("person: {e}")
}

// ----- the outline -----

fn list_person(p: &mut BlobPerson<'_>) -> Result<Step, String> {
    let c = p.conn()?;
    let o = p
        .session()
        .index_list(c, &ListQuery::new())
        .map_err(person_err)?;
    Ok(Step::ok(Output::Text(o.to_json() + "\n")))
}

fn list_agent(a: &mut BlobAgent<'_>) -> Result<Step, String> {
    let v = verbs(a)?;
    let out = run(a, &argv(v.list, &["--json"]))?;
    text_step(&out)
}

fn outline_agent(a: &mut BlobAgent<'_>) -> Result<Step, String> {
    let out = run(a, &argv("outline", &["--json"]))?;
    text_step(&out)
}

// ----- nodes -----

fn show_person(p: &mut BlobPerson<'_>) -> Result<Step, String> {
    let c = p.conn()?;
    let addrs = ["#1".to_owned()];
    let r = p
        .session()
        .index_nodes(c, &addrs, None, 1)
        .map_err(person_err)?;
    Ok(Step::ok(Output::Text(unkeyed(&(r.to_json(&addrs) + "\n")))))
}

fn show_agent(a: &mut BlobAgent<'_>) -> Result<Step, String> {
    let v = verbs(a)?;
    let out = run(a, &argv(v.show, &["#1", "--depth", "1", "--json"]))?;
    text_step(&out)
}

fn node_agent(a: &mut BlobAgent<'_>) -> Result<Step, String> {
    let out = run(a, &argv("node", &["#1", "--depth", "1", "--json"]))?;
    text_step(&out)
}

const MISSING: &str = "name:no node is called this";

fn partial_person(p: &mut BlobPerson<'_>) -> Result<Step, String> {
    let c = p.conn()?;
    let addrs = ["#1".to_owned(), MISSING.to_owned()];
    let r = p
        .session()
        .index_nodes(c, &addrs, None, 0)
        .map_err(person_err)?;
    // a view shows the miss; the verb's exit for it is 4 (K-READ-1)
    let exit = if r.missing.is_empty() {
        tools::exit::OK
    } else {
        tools::exit::NOT_FOUND
    };
    Ok(Step::exit(
        exit,
        Output::Text(unkeyed(&(r.to_json(&addrs) + "\n"))),
    ))
}

fn partial_agent(a: &mut BlobAgent<'_>) -> Result<Step, String> {
    let v = verbs(a)?;
    let out = run(a, &argv(v.show, &["#1", MISSING, "--json"]))?;
    text_step(&out)
}

// ----- history -----

fn diff_person(p: &mut BlobPerson<'_>) -> Result<Step, String> {
    let (v1, v2) = (p.stage().mark("v1")?, p.stage().mark("v2")?);
    let c = p.conn()?;
    let d = p.session().index_diff(c, v1, v2).map_err(person_err)?;
    Ok(Step::ok(Output::Text(d.to_json() + "\n")))
}

fn diff_agent(a: &mut BlobAgent<'_>) -> Result<Step, String> {
    let (v1, v2) = (a.stage().mark("v1")?, a.stage().mark("v2")?);
    let v = verbs(a)?;
    let out = run(
        a,
        &argv(v.diff, &[&v1.to_string(), &v2.to_string(), "--json"]),
    )?;
    text_step(&out)
}

fn blame_person(p: &mut BlobPerson<'_>) -> Result<Step, String> {
    let c = p.conn()?;
    let b = p.session().index_blame(c, None).map_err(person_err)?;
    Ok(Step::ok(Output::Text(b.to_json() + "\n")))
}

fn blame_agent(a: &mut BlobAgent<'_>) -> Result<Step, String> {
    let v = verbs(a)?;
    let out = run(a, &argv(v.blame, &["--json"]))?;
    text_step(&out)
}

// ----- claims -----

const NOTE: &str = "working on it";

fn person_claim(
    p: &mut BlobPerson<'_>,
) -> Result<Result<(), crate::session::SessionError>, String> {
    let c = p.conn()?;
    let k = first_key(p.stage(), c)?;
    Ok(p.session()
        .claim_nodes(c, &[k], None, Some(NOTE.into()))
        .map(|_| ()))
}

fn claim_person(p: &mut BlobPerson<'_>) -> Result<Step, String> {
    Ok(match person_claim(p)? {
        Ok(()) => Step::ok(Output::None),
        Err(e) => Step::exit(tools::session_exit(&e), Output::None),
    })
}

fn agent_claim(a: &mut BlobAgent<'_>) -> Result<VerbOut, String> {
    let v = a.stage().version()?.to_string();
    run(
        a,
        &argv("claim", &["--node", "#1", "--version", &v, "--note", NOTE]),
    )
}

fn claim_agent(a: &mut BlobAgent<'_>) -> Result<Step, String> {
    let out = agent_claim(a)?;
    Ok(Step::exit(out.exit, Output::None))
}

fn renew_person(p: &mut BlobPerson<'_>) -> Result<Step, String> {
    person_claim(p)?.map_err(person_err)?;
    let c = p.conn()?;
    p.session()
        .renew_claim(c, None, Some(60_000))
        .map_err(person_err)?;
    Ok(Step::ok(Output::None))
}

fn renew_agent(a: &mut BlobAgent<'_>) -> Result<Step, String> {
    agent_claim(a)?;
    let out = run(a, &argv("claim", &["--renew", "--ttl", "60000"]))?;
    Ok(Step::exit(out.exit, Output::None))
}

fn release_person(p: &mut BlobPerson<'_>) -> Result<Step, String> {
    person_claim(p)?.map_err(person_err)?;
    let c = p.conn()?;
    let gone = p.session().release_claim(c, None).map_err(person_err)?;
    Ok(Step::exit(u8::from(gone.is_empty()), Output::None))
}

fn release_agent(a: &mut BlobAgent<'_>) -> Result<Step, String> {
    agent_claim(a)?;
    let out = run(a, &argv("claim", &["--release"]))?;
    Ok(Step::exit(out.exit, Output::None))
}
