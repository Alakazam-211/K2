//! `k5-cli doc` against the shell CLI's `cli/k2 doc`, on the parity rig
//! ([`common::parity`]): the same requests, stdout, stderr and exit code
//! for every verb, its usage lines, refusals (the exit the doc table
//! names), no-answer cases, a large list, and help that never acts.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "doc";

fn r(name: &'static str, args: &'static [&'static str], replies: Vec<Reply>, code: i32) -> Case {
    let n = replies.len();
    case(name, args, &[], replies, owner_and_port, code, n)
}

fn stop(name: &'static str, args: &'static [&'static str], code: i32) -> Case {
    case(name, args, &[], vec![], owner_and_port, code, 0)
}

fn fed(input: &'static [u8], c: Case) -> Case {
    Case {
        stdin: Some(input),
        ..c
    }
}

fn set(setup: fn(&Rig), c: Case) -> Case {
    Case { setup, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn files(rig: &Rig) {
    owner_and_port(rig);
    std::fs::write(rig.proj.join("old.txt"), "old text\n").unwrap();
    std::fs::write(rig.proj.join("new.md"), "whole new\ncontent é\n").unwrap();
    let _ = std::fs::remove_file(rig.proj.join("out.pdf"));
    let set = rig.proj.join("set");
    std::fs::create_dir_all(set.join("sub")).unwrap();
    for f in ["b.DXF", "a.dwg", "sub/c.dxf", "note.txt"] {
        std::fs::write(set.join(f), "0\nSECTION\n").unwrap();
    }
}

const TOOL: &str = r#"{"stdout":"line one\nline two é\n","exit":0,"doc":{"path":"/w/a.md"},"fence":{"begin":"┄ a.md v3 ┄\n","end":"┄ end ┄\n"},"notes":"ada is on lines 1-2\n","ifVersion":2,"changed":true,"approval":{"onBehalfOf":"ada","state":"pending"}}"#;
const TOOL_NOEND: &str = r#"{"stdout":"no newline","stderr":"grep: 1 match\n\n","exit":1,"doc":{"path":"/w/a.md"},"fence":{"begin":"b\n","end":"e\n"}}"#;
const BLOB: &str = r#"{"stdout":"1 layer: Background\n","exit":0,"doc":{"path":"/w/a.psd"},"forEdit":"edit with: comment --on L\n"}"#;
const DOC: &str = r#""doc":{"path":"/w/a.md"}"#;

fn doc(rest: &str) -> String {
    format!("{{{DOC},{rest}}}")
}

fn big_log() -> String {
    let rows: Vec<String> = (0..5000)
        .map(|i| {
            if i % 7 == 0 {
                format!(
                    r#"{{"type":"version","id":{i},"atMs":{},"reason":"save","name":"v{i}","authorLabel":"Ada Lovelace","via":"cli","size":{i},"onBehalfOf":"bob"}}"#,
                    i * 1000
                )
            } else {
                format!(
                    r#"{{"change":{i},"atMs":{},"kind":"edit","linesAdded":{i},"linesRemoved":1,"author":"agent-{i}","via":"mcp","of":null}}"#,
                    i * 1000
                )
            }
        })
        .collect();
    let body = format!(r#"{{"log":[{}]}}"#, rows.join(","));
    assert!(body.len() > 256 * 1024, "the large case is above 256 KiB");
    body
}

#[test]
fn doc_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "all");
    let v = vec![
        // covers: doc
        stop("the help", &[], 0),
        stop("help", &["help"], 0),
        // covers: doc --json
        stop("--json alone: an unknown verb", &["--json"], 2),
        stop("an unknown verb with a capital", &["Cat", "a.md"], 2),
        // covers: doc cores
        r(
            "cores",
            &["cores"],
            vec![ok(r#"{"cores":[{"id":"markdown","name":"Markdown","on":true,"status":"beta","short":"Live .md files","open":{"files":2,"paths":["a.md","b é.md"]}},{"id":"text","name":"Text","on":true,"open":{"files":1}},{"id":"cad","name":"CAD","on":false,"description":"Drawings"}]}"#)],
            0,
        ),
        // covers: doc cores --json
        r("cores --json", &["cores", "--json"], vec![ok(r#"{"cores":[]}"#)], 0),
        r(
            "cores on",
            &["cores", "on", "markdown"],
            vec![ok(r#"{"multiplayerCores":{"markdown":true},"multiplayerCoresReport":{"changed":[{"core":"markdown"}]}}"#)],
            0,
        ),
        r(
            "cores on: already",
            &["cores", "on", "fab"],
            vec![ok(r#"{"multiplayerCoresReport":{"changed":[]}}"#)],
            0,
        ),
        r(
            "cores off with a file it couldn't write",
            &["cores", "off", "markdown"],
            vec![ok(r#"{"multiplayerCoresReport":{"changed":[{"core":"markdown"}],"closed":3,"paths":["a.md","b.md","c.md"],"failed":[{"path":"c.md","error":"disk full"}]}}"#)],
            1,
        ),
        r(
            "cores off board, --json",
            &["cores", "off", "board", "--json"],
            vec![ok(r#"{"multiplayerCores":{"board":false},"multiplayerCoresReport":{"changed":[{"core":"board"}]}}"#)],
            0,
        ),
        r(
            "cores on board",
            &["cores", "on", "board"],
            vec![ok(r#"{"multiplayerCoresReport":{"changed":[{"core":"board"}]}}"#)],
            0,
        ),
        r(
            "cores on: refused",
            &["cores", "on", "text"],
            vec![Reply::Answer(403, r#"{"error":"owner only"}"#.into())],
            3,
        ),
        stop("cores: no such core", &["cores", "on", "nope"], 2),
        stop("cores: a flag", &["cores", "-w", "x"], 2),
        stop("cores: one word", &["cores", "on"], 2),
        stop("cores --help", &["cores", "on", "markdown", "--help"], 0),
        case(
            "cores on in a session: people only",
            &["cores", "on", "markdown"],
            &[],
            vec![],
            port_only,
            3,
            0,
        )
        .with_env(with_passport(&[])),
        // covers: doc cad
        r(
            "cad",
            &["cad", "drawing", "info", "x.dxf"],
            vec![ok(r#"{"stdout":"units mm","next":["layer","list","--version","3","--after","@h:5"],"path":"/w/x.dxf"}"#)],
            0,
        ),
        // covers: doc cad --json
        r(
            "cad --json",
            &["cad", "--json", "layer", "list", "--unused", "-w", "ws", "x.dxf"],
            vec![ok(r#"{"stdout":"0 layers\n"}"#)],
            0,
        ),
        stop("cad: no read verb", &["cad", "x.dxf"], 2),
        // covers: doc av
        set(
            files,
            r(
                "av over a folder",
                &["av", "read", "--config", "av.conf", "set", "one.dxf"],
                vec![ok(r#"{"what":"read","stdout":"3 sheets\n"}"#)],
                0,
            ),
        ),
        r(
            "av summary",
            &["av", "summary", "--config=av.conf", "x.dwg"],
            vec![ok(r#"{"what":"summary","summary":"2 devices","stdout":"no"}"#)],
            0,
        ),
        // covers: doc av --json
        r(
            "av --json",
            &["av", "summary", "--config", "c", "--json", "x.dwg"],
            vec![ok(r#"{"what":"summary"}"#)],
            0,
        ),
        stop("av: no what", &["av", "list"], 2),
        stop("av: no config", &["av", "read", "x.dxf"], 2),
        stop("av: no drawings", &["av", "read", "--config", "c"], 2),
        // covers: doc cat
        r("cat", &["cat", "a.md"], vec![ok(TOOL)], 0),
        r("cat --fence", &["cat", "--fence", "a.md"], vec![ok(TOOL)], 0),
        r("cat --json", &["cat", "--json", "a.md"], vec![ok(r#"{"result":{"exit":3},"stdout":"{\"a\":1}\n\n","ifVersion":1,"changed":true}"#)], 3),
        r("cat --json, not a result", &["cat", "--json", "a.md"], vec![ok(r#"{"exit":2}"#)], 2),
        r("cat --json, not JSON", &["cat", "--json", "a.md"], vec![ok("nope")], 1),
        r("cat: bytes", &["cat", "a.bin"], vec![ok(r#"{"stdoutB64":"aGkK","exit":0,"doc":{"path":"/w/a.bin"}}"#)], 0),
        traceback(r("cat: not JSON", &["cat", "a.md"], vec![ok("<html>")], 1)),
        traceback(r("cat: a list", &["cat", "a.md"], vec![ok("[]")], 1)),
        r(
            "cat: refused",
            &["cat", "a.md"],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            3,
        ),
        r(
            "cat: core off",
            &["cat", "a.md"],
            vec![Reply::Answer(403, r#"{"error":{"code":"core_off","hint":"the core is off","exit":3}}"#.into())],
            3,
        ),
        r("cat: not there", &["cat", "a.md"], vec![Reply::Answer(404, "gone\n".into())], 4),
        r("cat: no answer", &["cat", "a.md"], vec![Reply::Close], 1),
        stop("cat: no doc", &["cat"], 2),
        stop("cat: a flag last", &["cat", "a.md", "-n"], 2),
        stop("-w with no value", &["cat", "-w"], 2),
        // covers: doc head
        r("head", &["head", "-n", "1", "a.md"], vec![ok(TOOL)], 0),
        // covers: doc tail
        r("tail", &["tail", "-n", "1", "--", "a.md"], vec![ok(TOOL)], 0),
        // covers: doc wc
        r("wc", &["wc", "-l", "-w", "ws", "a.md"], vec![ok(TOOL)], 0),
        // covers: doc grep
        r("grep", &["grep", "-n", "one", "a.md"], vec![ok(TOOL_NOEND)], 1),
        r("grep --fence", &["grep", "--fence", "x", "a.md"], vec![ok(TOOL_NOEND)], 1),
        // covers: doc sed
        r("sed", &["sed", "-n", "1p", "--on-behalf-of", "ada", "a.md"], vec![ok(TOOL)], 0),
        fed(b"from stdin\n", r("sed -i -", &["sed", "-i", "-", "a.md"], vec![ok(TOOL)], 0)),
        // covers: doc view
        r("view", &["view", "--lines", "1,2", "a.md"], vec![ok(TOOL)], 0),
        r("view --region", &["view", "--region", "page 3", "a.pdf"], vec![ok(r#"{"region":"page 3","doc":{"path":"/w/a.pdf"}}"#)], 0),
        r("view --region=", &["view", "--region=", "a.pdf"], vec![ok(r#"{"doc":{"path":"/w/a.pdf"}}"#)], 0),
        stop("view --region last", &["view", "a.pdf", "--region"], 1),
        // covers: doc diff
        fed(b"my copy\n", r("diff", &["diff", "a.md"], vec![ok(TOOL)], 0)),
        r(
            "diff --version",
            &["diff", "--version", "2", "--to", "3", "-U", "1", "-u", "a.md"],
            vec![ok(r#"{"text":"--- v2\n+++ v3\n","exit":1}"#)],
            1,
        ),
        r("diff --version --json", &["diff", "--version=2", "--json", "a.md"], vec![ok(r#"{"text":"","exit":0}"#)], 0),
        stop("diff --version x", &["diff", "--version", "x", "a.md"], 2),
        // covers: doc patch
        fed(b"--- a\n+++ b\n", r("patch", &["patch", "a.md"], vec![ok(TOOL)], 0)),
        fed(b"\xff\xfe", stop("patch: stdin not UTF-8", &["patch", "a.md"], 2)),
        // covers: doc append
        r("append --text", &["append", "--text", "more é", "a.md"], vec![ok(TOOL)], 0),
        r("append, no stdin", &["append", "a.md"], vec![ok(TOOL)], 0),
        // covers: doc insert
        fed(b"new\n", r("insert", &["insert", "--after", "1", "a.md"], vec![ok(TOOL)], 0)),
        // covers: doc replace
        r("replace exact", &["replace", "--old", "a", "--new=b", "a.md"], vec![ok(TOOL)], 0),
        set(files, r("replace --old-file", &["replace", "--old-file", "old.txt", "--new", "b", "a.md"], vec![ok(TOOL)], 0)),
        stop("replace --old-file missing", &["replace", "--old-file", "none.txt", "--new", "b", "a.md"], 4),
        stop("replace --new-file last", &["replace", "a.md", "--new-file"], 2),
        // covers: doc delete
        r("delete", &["delete", "--lines", "2", "a.md"], vec![ok(TOOL)], 0),
        // covers: doc replace-section
        fed(b"## Intro\nnew\n", r("replace-section", &["replace-section", "--heading", "Intro", "a.md"], vec![ok(TOOL)], 0)),
        r("a verb of the pack's own", &["frob", "a.md"], vec![Reply::Answer(400, r#"{"error":{"code":"unsupported","hint":"no such verb"}}"#.into())], 2),
        // covers: doc mv
        r("mv", &["mv", "a.md", "b.md"], vec![ok(r#"{"from":"/w/a.md","doc":{"path":"/w/b.md"}}"#)], 0),
        r("mv -w", &["mv", "-w", "ws", "a.md", "x/b.md"], vec![ok(r#"{"from":"a.md","doc":{"path":"x/b.md"}}"#)], 0),
        stop("mv: one word", &["mv", "a.md"], 2),
        // covers: doc write
        r("write --text", &["write", "--base", "live", "a.md", "--text", "new\n"], vec![ok(&doc(r#""result":{"linesAdded":1,"linesRemoved":0,"version":4},"notes":"saved\n","warning":"big","approval":{"onBehalfOf":"bob","state":"pending"}"#))], 0),
        set(files, r("write --file, any file", &["write", "--base=3", "--file", "new.md", "a.bin"], vec![ok(r#"{"tier":0,"doc":{"path":"/w/a.bin"},"result":{"changed":true,"size":3,"version":2}}"#)], 0)),
        fed(b"same\n", r("write from stdin, unchanged", &["write", "--base", "2", "a.bin"], vec![ok(r#"{"tier":0,"doc":{"path":"/w/a.bin"},"result":{"changed":false,"version":2}}"#)], 0)),
        r(
            "write: conflicts",
            &["write", "--base", "2", "--text", "x", "a.md"],
            vec![Reply::Answer(409, r#"{"error":{"code":"base_expired","hint":"read the doc again"},"conflicts":[{"lines":[1,2],"ours":"a","theirs":"b"},{"yourLines":[3,4],"currentLines":[5,6],"ours":"c\n","theirs":"d é"}]}"#.into())],
            8,
        ),
        stop("write: no base", &["write", "a.md"], 2),
        stop("write: no such --file", &["write", "--base", "1", "--file", "none.md", "a.md"], 4),
        // covers: doc undo
        r("undo: nothing", &["undo", "a.md"], vec![ok(r#"{"nothing":true}"#)], 0),
        r("undo --json", &["undo", "--json", "a.md"], vec![ok(r#"{"exit":1,"skipped":[]}"#)], 1),
        stop("undo --steps x", &["undo", "--steps", "x", "a.md"], 2),
        // covers: doc redo
        r(
            "redo: skipped and blocked",
            &["redo", "--steps", "2", "--partial", "--for", "ada", "a.md"],
            vec![ok(r#"{"steps":2,"result":{"linesAdded":1,"version":6},"skipped":[{"lines":[3,4],"kind":"edit"},{"kind":"move"}],"blocked":true,"exit":1}"#)],
            1,
        ),
        r("redo: history lost", &["redo", "a.md"], vec![ok(r#"{"historyLost":true}"#)], 0),
        // covers: doc revert
        r("revert", &["revert", "12", "--partial", "a.md"], vec![ok(r#"{"dropped":12}"#)], 0),
        stop("revert: not a number", &["revert", "x", "a.md"], 2),
        // covers: doc who
        r(
            "who",
            &["who", "a.md"],
            vec![ok(r#"{"roster":[{"name":"Ada","kind":"person","role":"editor","cursor":{"head":"3:4"},"view":{"first":1,"last":40},"claims":[{"lines":[2,5],"note":"intro","remainingMs":61500}],"lastEdit":[7,9],"holdsLease":true,"away":true,"connections":2},{"name":"agent-1","kind":"agent","role":"viewer","region":"page 2"}]}"#)],
            0,
        ),
        r("who: nobody", &["who", "a.md"], vec![ok(r#"{"roster":[]}"#)], 0),
        r("who --json", &["who", "--json", "a.md"], vec![ok(r#"{"roster":[]}"#)], 0),
        stop("who: two docs", &["who", "a.md", "b.md"], 2),
        // covers: doc follow
        r("follow", &["follow", "ADA", "a.md"], vec![ok(r#"{"roster":[{"name":"Ada","role":"editor","cursor":{"head":"3:4"},"view":{"first":1,"last":9},"holdsLease":true}]}"#)], 0),
        r("follow: by pid, no view", &["follow", "p7", "a.md"], vec![ok(r#"{"roster":[{"name":"Bo","pid":"p7","role":"viewer","region":"page 1"}]}"#)], 0),
        r("follow: not here", &["follow", "zed", "a.md"], vec![ok(r#"{"roster":[]}"#)], 4),
        stop("follow: no doc", &["follow", "ada"], 2),
        // covers: doc cursor
        r("cursor", &["cursor", "--at", "1:2", "--select", "1:9", "a.md"], vec![ok(r#"{"cursor":{"anchor":3,"head":10}}"#)], 0),
        stop("cursor: no --at", &["cursor", "a.md"], 2),
        stop("cursor --at last", &["cursor", "a.md", "--at"], 1),
        // covers: doc claim
        r(
            "claim",
            &["claim", "--lines", "1,3", "--ttl", "2m", "--note", "intro", "--hold", "--id=c9", "a.md"],
            vec![ok(&doc(r#""action":"take","claim":{"lines":[1,3],"id":"c9","remainingMs":120000,"hold":true,"waiters":["x","y"]}"#))],
            0,
        ),
        r("claim --release", &["claim", "--release", "a.md"], vec![ok(&doc(r#""action":"release","released":["c1","c2"]"#))], 0),
        r("claim --renew", &["claim", "--renew", "--no-hold", "a.md"], vec![ok(&doc(r#""action":"renew","claim":{"lines":[1,3],"id":"c9","remainingMs":-5}"#))], 0),
        r("claim by layer", &["claim", "--node", "n1", "--ttl", "30s", "a.psd"], vec![ok(BLOB)], 0),
        r("claim on a PSB", &["claim", "--ttl=250", "Layer 1", "a.PSB"], vec![ok(BLOB)], 0),
        stop("claim: no lines", &["claim", "a.md"], 2),
        stop("claim: a bad ttl", &["claim", "--lines", "1,2", "--ttl", "1h", "a.md"], 2),
        stop("claim by layer: a bad ttl", &["claim", "--ttl", "0", "a.psd"], 2),
        // covers: doc activity
        r("activity", &["activity", "-n", "5", "a.md"], vec![ok(r#"{"rendered":"ada edited lines 1-2\n"}"#)], 0),
        r("activity: none", &["activity", "a.md"], vec![ok("{}")], 0),
        stop("activity -n x", &["activity", "-n", "x", "a.md"], 2),
        // covers: doc say
        r("say", &["say", "a.md", "hello", "there", "--to", "bob"], vec![ok(r#"{"say":{"id":"s1","to":"bob"}}"#)], 0),
        r("say --text", &["say", "--text", "hi é", "--at", "L3", "a.md"], vec![ok(r#"{"say":{"id":"s2"}}"#)], 0),
        stop("say: what?", &["say", "a.md"], 2),
        stop("say --to last", &["say", "a.md", "--to"], 2),
        // covers: doc open
        r("open", &["open", "a.md"], vec![ok(&doc(r#""version":3"#))], 0),
        // covers: doc leave
        r("leave", &["leave", "a.md"], vec![ok(&doc("\"left\":true"))], 0),
        // covers: doc flush
        r("flush", &["flush", "a.md"], vec![ok(&doc(r#""disk":{"state":"clean"},"written":true"#))], 0),
        // covers: doc status
        r(
            "status: the pool",
            &["status"],
            vec![ok(r#"{"pool":{"running":true,"loaded":2,"estimatedBytes":1048576,"budgetBytes":67108864,"unloadedTotal":0,"diskSync":true,"anyFile":{"on":true,"loaded":1,"blobs":3,"bytes":2048,"unreferencedBytes":0,"maxFileBytes":104857600},"saves":{"commits":"grouped","groupMaxChanges":10,"groupMaxMs":50,"commitsOnDisk":5,"commitsMade":6,"restartNeeded":true}}}"#)],
            0,
        ),
        r("status: off", &["status"], vec![ok(r#"{"pool":{"running":false,"error":"no store"}}"#)], 0),
        r(
            "status of a doc",
            &["status", "a.md"],
            vec![ok(&doc(r#""version":3,"participants":2,"connections":3,"disk":{"state":"dirty","reason":"saving"},"rights":{"read":true,"history":true},"declined":"too big""#))],
            0,
        ),
        r(
            "status of any file",
            &["status", "a.bin"],
            vec![ok(r#"{"doc":{"path":"/w/a.bin"},"tier":0,"version":2,"current":{"size":9,"reason":"write","author":"p1","via":"cli","atMs":86400000},"lease":{"label":"Ada","expiresMs":3600000,"mine":true},"conflictCopies":[{"path":"a (conflict).bin","kept":1}],"rights":{"edit":true}}"#)],
            0,
        ),
        // covers: doc log
        r("log", &["log", "--by", "ada", "-n", "2", "--pending", "--for", "bob", "a.md"], vec![ok(r#"{"log":[{"type":"version","id":4,"atMs":0,"reason":"name","name":"draft"},{"change":7,"atMs":1000,"kind":"insert","of":3,"author":"ada","onBehalfOf":"bob","linesAdded":2}]}"#)], 0),
        r("log: empty", &["log", "a.md"], vec![ok(r#"{"log":[]}"#)], 0),
        r("log: a large list", &["log", "a.md"], vec![ok(&big_log())], 0),
        stop("log --since x", &["log", "--since", "x", "a.md"], 2),
        stop("log -n x", &["log", "-n", "x", "a.md"], 2),
        // covers: doc versions
        r("versions", &["versions", "a.md"], vec![ok(r#"{"versions":[{"id":3,"atMs":5000,"version":12,"reason":"named","name":"draft","authors":["ada","bob"]},{"id":1,"reason":"open"}]}"#)], 0),
        r("versions of any file", &["versions", "a.bin"], vec![ok(r#"{"tier":0,"versions":[{"id":2,"atMs":0,"reason":"restore","size":2097152,"author":"p1","via":"cli","restoredFrom":1,"conflictCopy":"c.bin","redacted":true,"current":true},{"id":1,"reason":"write","size":1500,"authorLabel":"Ada","via":"app"}]}"#)], 0),
        // covers: doc show
        r("show", &["show", "--version", "3", "a.md"], vec![ok(&doc(r#""text":"hello é\n","version":3"#))], 0),
        r("show bytes", &["show", "--version=3", "a.bin"], vec![ok(r#"{"bytesB64":"aGkK","version":3,"doc":{"path":"/w/a.bin"}}"#)], 0),
        r("show --json", &["show", "--json", "--version", "3", "a.md"], vec![ok(r#"{"text":"x"}"#)], 0),
        stop("show: no version", &["show", "a.md"], 2),
        stop("show --version x", &["show", "--version", "x", "a.md"], 2),
        stop("--out on versions", &["versions", "--out", "f", "a.md"], 2),
        // covers: doc blame
        r("blame", &["blame", "-L", "1,2", "a.md"], vec![ok(r#"{"rendered":"1 ada x\n","porcelain":"p\n"}"#)], 0),
        r("blame --porcelain", &["blame", "-L1", "--porcelain", "a.md"], vec![ok(r#"{"rendered":"1 ada x\n","porcelain":"p\n"}"#)], 0),
        stop("blame -L x", &["blame", "-L", "x", "a.md"], 2),
        // covers: doc restore
        r("restore", &["restore", "--version", "2", "a.md"], vec![ok(r#"{"restored":2,"result":{"linesAdded":1,"version":9}}"#)], 0),
        r("restore any file", &["restore", "--version", "2", "a.bin"], vec![ok(r#"{"tier":0,"restored":2,"result":{"changed":true,"version":5,"size":3}}"#)], 0),
        stop("restore: no version", &["restore", "a.md"], 2),
        // covers: doc version
        r("version --name", &["version", "--name", "draft é", "--version", "4", "a.md"], vec![ok(r#"{"version":4,"name":"draft é"}"#)], 0),
        stop("version: no name", &["version", "a.md"], 2),
        // covers: doc approvals
        r("approvals", &["approvals", "--pending", "--mine", "a.md"], vec![ok(r#"{"approvals":[{"change":7,"approval":{"state":"pending","author":"agent-1","onBehalfOf":"ada"}},{"change":8,"approval":{"state":"approved","author":"a","onBehalfOf":"b","by":"ada"}}]}"#)], 0),
        r("approvals: none", &["approvals", "a.md"], vec![ok("{}")], 0),
        stop("approvals --for last", &["approvals", "a.md", "--for"], 2),
        // covers: doc approve
        r("approve", &["approve", "7", "a.md"], vec![ok(r#"{"change":7,"approval":{"state":"approved"},"hint":"applied"}"#)], 0),
        stop("approve: not a number", &["approve", "x", "a.md"], 2),
        // covers: doc decline
        r("decline", &["decline", "2", "a.bin"], vec![ok(r#"{"tier":0,"change":2,"approval":{"state":"declined"}}"#)], 0),
        // covers: doc export
        r(
            "export",
            &["export", "--to", "pdf", "-O", "dpi=300", "--option=x=1", "a.md"],
            vec![ok(&doc(r#""job":"j1","state":"done","target":"pdf","docVersion":4,"size":100,"version":2,"codec":"lossy","placed":{"path":"a.pdf","abs":"/w/a.pdf"},"report":{"losses":[{"kind":"font","detail":"Comic"}],"notes":["one page"]}"#))],
            0,
        ),
        r(
            "export, polled",
            &["export", "--to", "html", "a.md"],
            vec![
                ok(r#"{"job":"j2","state":"running"}"#),
                ok(&doc(r#""job":"j2","state":"done","target":"html","docVersion":4,"size":7,"placed":{"path":"a.html"}"#)),
            ],
            0,
        ),
        set(
            files,
            r(
                "export, fetched",
                &["export", "--to", "pdf", "--out", "out.pdf", "--json", "a.md"],
                vec![
                    ok(&doc(r#""job":"j3","state":"done","fetch":true,"target":"pdf""#)),
                    ok(r#"{"bytesB64":"aGk=","offset":0,"eof":false}"#),
                    ok(r#"{"bytesB64":"Cg==","offset":2,"eof":true}"#),
                ],
                0,
            ),
        ),
        r(
            "export --list",
            &["export", "--list", "a.md"],
            vec![ok(r#"{"targets":[{"id":"pdf","fidelity":"high","extension":"pdf","runs":"worker","options":[{"name":"dpi","kind":{"type":"int","min":72,"max":600},"default":300},{"name":"size","kind":{"type":"choice","values":["a4","letter"]}}]},{"id":"md","fidelity":"exact","runs":"daemon"}],"worker":{"available":false,"reason":"not installed"}}"#)],
            0,
        ),
        stop("export --list with --to", &["export", "--list", "--to", "pdf", "a.md"], 2),
        stop("export: no target", &["export", "a.md"], 2),
        stop("export --to last", &["export", "a.md", "--to"], 2),
        // covers: doc lease
        r("lease", &["lease", "--ttl", "5m", "a.bin"], vec![ok(r#"{"action":"acquire","doc":{"path":"/w/a.bin"},"lease":{"expiresMs":0}}"#)], 0),
        r("lease --break", &["lease", "--break", "a.bin"], vec![ok(r#"{"action":"break","doc":{"path":"/w/a.bin"},"broken":{"label":"Ada"},"exit":0}"#)], 0),
        r("lease --release, none", &["lease", "--release", "a.bin"], vec![ok(r#"{"action":"release","doc":{"path":"/w/a.bin"},"exit":1}"#)], 1),
        stop("lease: a bad ttl", &["lease", "--ttl", "2d", "a.bin"], 2),
        // covers: doc lock
        r("lock", &["lock", "--renew", "a.bin"], vec![ok(r#"{"action":"renew","doc":{"path":"/w/a.bin"},"lease":{"expiresMs":1000}}"#)], 0),
        // covers: doc unlock
        r("unlock", &["unlock", "a.bin"], vec![ok(r#"{"action":"release","released":true,"doc":{"path":"/w/a.bin"}}"#)], 0),
        // covers: doc outline
        r("outline", &["outline", "a.psd"], vec![ok(BLOB)], 0),
        // covers: doc node
        r("node", &["node", "--id", "n1", "--depth", "2", "a.psd"], vec![ok(BLOB)], 0),
        // covers: doc layer
        r("layer", &["layer", "list", "--fence", "a.psd"], vec![ok(BLOB)], 0),
        // covers: doc comment
        r("comment", &["comment", "--body", "hi é", "--on", "1", "--json", "a.md"], vec![ok(BLOB)], 0),
        stop("comment --body last", &["comment", "a.md", "--body"], 2),
        // covers: doc reply
        r("reply", &["reply", "--id", "ann-1", "--body", "ok", "--", "a.md"], vec![ok(BLOB)], 0),
        // covers: doc resolve
        r("resolve", &["resolve", "--id", "ann-1", "-w", "ws", "a.md"], vec![ok(BLOB)], 0),
        // covers: doc comments
        r("comments", &["comments", "--read-budget", "20000", "a.md"], vec![ok(BLOB)], 0),
        stop("comments: no doc", &["comments", "--json"], 2),
        stop("comments: a bad budget", &["comments", "--read-budget=x", "a.md"], 2),
        // covers: doc reanchor
        r("reanchor", &["reanchor", "--id", "ann-1", "--on", "2", "a.md"], vec![ok(BLOB)], 0),
        // covers: doc purge-comments
        r("purge-comments", &["purge-comments", "--all", "--n", "3", "a.md"], vec![ok(&doc(r#""counts":{"comments":2,"anchors":1},"storeRewritten":true"#))], 0),
        stop("purge-comments: what?", &["purge-comments", "a.md"], 2),
        stop("purge-comments --n x", &["purge-comments", "--n", "x", "a.md"], 2),
        // covers: doc erase-participant
        r(
            "erase-participant",
            &["erase-participant", "--participant", "p1", "--pseudonym", "Guest", "a.md"],
            vec![ok(&doc(r#""erased":"p1","pseudonym":"Guest","counts":{"attributions":3},"notReached":[{"describe":"a backup","k2":"k2 x"},{"describe":"a copy"}]"#))],
            0,
        ),
        stop("erase-participant: no pseudonym", &["erase-participant", "--participant", "p1", "a.md"], 2),
        // covers: doc share
        r("share", &["share", "docs", "--with", "ws2", "-w", "ws1", "--write", "--comment"], vec![ok(r#"{"share":{"path":"docs","folder":true,"write":true,"comment":true}}"#)], 0),
        stop("share: no -w", &["share", "docs", "--with", "ws2"], 2),
        stop("share: no --with", &["share", "docs", "-w", "ws1"], 2),
        // covers: doc unshare
        r("unshare", &["unshare", "a.md", "--with=ws2", "--workspace", "ws1"], vec![ok(r#"{"removed":{"path":"a.md"}}"#)], 0),
        // covers: doc shares
        r("shares", &["shares", "--with", "ws2"], vec![ok(r#"{"shares":[{"owner":"ws1","path":"docs","folder":true,"sharee":"ws2","history":true},{"owner":"ws1","sharee":"ws3"}]}"#)], 0),
        r("shares: none", &["shares"], vec![ok("{}")], 0),
    ];
    for c in v {
        rig.check(&c);
    }
}

trait WithEnv {
    fn with_env(self, env: Vec<(&'static str, String)>) -> Case;
}

impl WithEnv for Case {
    fn with_env(self, env: Vec<(&'static str, String)>) -> Case {
        Case { env, ..self }
    }
}

/// `watch` and `tail -f` poll once a second; a refusal ends them (exit 3).
#[test]
fn watch_polls_until_refused() {
    let rig = Rig::new(VERB, "watch");
    let refused = || {
        Reply::Answer(
            403,
            r#"{"error":{"code":"forbidden","hint":"rights revoked"}}"#.into(),
        )
    };
    for c in [
        // covers: doc watch
        r(
            "watch",
            &["watch", "a.md"],
            vec![
                ok(&doc(r#""version":3,"tier":1"#)),
                ok(&doc(r#""version":3,"stdout":"a\nb\n""#)),
                ok(r#"{"version":4}"#),
                ok(&doc(
                    r#""version":4,"stdout":"a\nB\nc\n","fence":{"begin":"┄ contributors since you last read: Ada ┄\n"}"#,
                )),
                refused(),
            ],
            3,
        ),
        r(
            "tail -f, any file",
            &["tail", "-f", "a.bin"],
            vec![
                ok(r#"{"tier":0}"#),
                ok(r#"{"doc":{"path":"/w/a.bin"},"version":1}"#),
                ok(
                    r#"{"doc":{"path":"/w/a.bin"},"version":2,"current":{"reason":"write","author":"p1","size":4,"conflictCopy":"c.bin"},"lease":{"pid":"p1","label":"Ada"}}"#,
                ),
                refused(),
            ],
            3,
        ),
    ] {
        rig.check(&c);
    }
}

/// In a session: the cell socket with the passport, the fence on by
/// default, and a POST that went out with no answer never sent again.
#[test]
fn in_a_session() {
    let rig = Rig::new(VERB, "session");
    let sess = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    for c in [
        case(
            "cat on the socket, fenced",
            &["cat", "a.md"],
            &[],
            vec![ok(TOOL)],
            port_only,
            0,
            1,
        )
        .with_env(sess()),
        case(
            "cat --no-fence",
            &["cat", "--no-fence", "a.md"],
            &[],
            vec![ok(TOOL)],
            port_only,
            0,
            1,
        )
        .with_env(sess()),
        case(
            "who on the socket",
            &["who", "a.md"],
            &[],
            vec![ok(r#"{"roster":[]}"#)],
            port_only,
            0,
            1,
        )
        .with_env(sess()),
        Case {
            compare: Compare::Waited,
            ..case(
                "open: no answer on the socket",
                &["open", "a.md"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            )
            .with_env(sess())
        },
        Case {
            compare: Compare::Waited,
            ..case(
                "cat: no answer on the socket",
                &["cat", "a.md"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            )
            .with_env(sess())
        },
        case(
            "a session with no passport",
            &["cat", "a.md"],
            &[],
            vec![],
            owner_and_port,
            3,
            0,
        )
        .with_env(vec![("K2_HOOK_SOCK", "{sock}".to_string())]),
        case("a dead port", &["status"], &[], vec![], owner_only, 1, 0)
            .with_env(vec![("K2_PORT", dead_port())]),
    ] {
        rig.check(&c);
    }
}

/// Help anywhere on the line prints the help page and sends nothing, on
/// every verb that changes something.
#[test]
fn help_never_acts() {
    let rig = Rig::new(VERB, "help");
    let lines: &'static [&'static [&'static str]] = &[
        &["write", "--base", "live", "a.md", "-h"],
        &["append", "--text", "x", "a.md", "--help"],
        &["insert", "--after", "1", "-h", "a.md"],
        &["replace", "--old", "a", "--new", "b", "a.md", "-h"],
        &["delete", "--lines", "2", "a.md", "--help"],
        &["patch", "a.md", "-h"],
        &["sed", "-i", "s/a/b/", "a.md", "-h"],
        &["replace-section", "--heading", "x", "a.md", "-h"],
        &["mv", "a.md", "b.md", "-h"],
        &["undo", "a.md", "--help"],
        &["redo", "a.md", "-h"],
        &["revert", "3", "a.md", "-h"],
        &["restore", "--version", "2", "a.md", "-h"],
        &["version", "--name", "x", "a.md", "--help"],
        &["approve", "3", "a.md", "-h"],
        &["decline", "3", "a.md", "-h"],
        &["claim", "--lines", "1,2", "a.md", "-h"],
        &["lease", "a.bin", "--help"],
        &["lock", "a.bin", "-h"],
        &["unlock", "a.bin", "-h"],
        &["say", "a.md", "hi", "-h"],
        &["cursor", "--at", "1:1", "a.md", "-h"],
        &["open", "a.md", "-h"],
        &["leave", "a.md", "--help"],
        &["flush", "a.md", "-h"],
        &["comment", "--body", "x", "a.md", "-h"],
        &["reply", "--id", "a", "--body", "x", "a.md", "-h"],
        &["resolve", "--id", "a", "a.md", "--help"],
        &["reanchor", "--id", "a", "a.md", "-h"],
        &["purge-comments", "--all", "a.md", "-h"],
        &[
            "erase-participant",
            "--participant",
            "p",
            "--pseudonym",
            "q",
            "a.md",
            "-h",
        ],
        &["share", "d", "--with", "w", "-w", "v", "-h"],
        &["unshare", "d", "--with", "w", "-w", "v", "--help"],
        &["export", "--to", "pdf", "a.md", "-h"],
        &["watch", "a.md", "-h"],
        &["view", "--region", "p1", "a.pdf", "-h"],
        &["cad", "check", "--writeback", "x.dxf", "-h"],
        &["av", "read", "--config", "c", "x.dxf", "--help"],
        &["cores", "on", "markdown", "-h"],
        &["cores", "off", "board", "--help"],
    ];
    for args in lines {
        rig.check(&case(
            "help anywhere",
            args,
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ));
        rig.check(&case(
            "help anywhere, no daemon",
            args,
            &[],
            vec![],
            nothing,
            0,
            0,
        ));
    }
}

/// The usage lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`).
#[test]
fn the_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    let hint = format!("{cli} doc cursor --at L:C [--to L:C] <file.md>");
    let c = case(
        "brand",
        &["cursor", "a.md"],
        &[],
        vec![],
        owner_and_port,
        2,
        0,
    );
    let (out, log) = rig.binary(&c, false);
    assert_eq!(
        out.stderr,
        format!("Error: {hint}\n{{\"error\": {{\"code\": \"usage\", \"hint\": \"{hint}\"}}}}\n")
    );
    assert_eq!(out.code, Some(2));
    assert!(log.is_empty());
    let c = case(
        "brand",
        &["open", "a.md"],
        &[],
        vec![ok(&doc(r#""version":3"#))],
        owner_and_port,
        0,
        1,
    );
    let (out, _) = rig.binary(&c, false);
    assert_eq!(
        out.stdout,
        format!("in /w/a.md · v3 (stay present until {cli} doc leave)\n")
    );
}

/// A tool answer whose text holds the passport refusal's words.
const TOOL_WORDS: &str = r#"{"stdout":"Invalid or missing auth token\n","exit":0,"doc":{"path":"/w/a.md"}}"#;

/// `_doc_xfer` (a tool call, a whole file): the shell never tries a
/// healed passport there. The binary tries one once, and only on the
/// daemon's auth gate refusal (403 and exactly its body), which comes
/// before any handler runs: the refused call did nothing (the stub's
/// refusal writes only the session's re-minted passport, as the daemon
/// does), so the same call goes again. That is `scripts/cli-parity.sh`'s
/// named difference `passport-heal-once`. An answer the handler gave that
/// holds the refusal's words (a 200 after the edit, a 409 conflict) is
/// never sent again, on either side.
#[test]
fn xfer_heals_once_on_the_gate_s_refusal() {
    let rig = Rig::new(VERB, "heal");
    let reminted = |status: u16, body: &str| Reply::WriteThen {
        file: rig.secrets.join(".k2-passport"),
        content: format!("{HEALED}\n"),
        status,
        body: body.to_string(),
    };
    let append: &'static [&'static str] = &["append", "--text", "x", "a.md"];
    // Parity: the words in a handler's answer are no refusal of the
    // passport (the 409's note sends the shell's passport probe after).
    for c in [
        case(
            "a 200 holding the refusal's words: sent once",
            append,
            &[],
            vec![reminted(200, TOOL_WORDS)],
            port_only,
            0,
            1,
        ),
        case(
            "a 409 holding the refusal's words: sent once",
            append,
            &[],
            vec![reminted(
                409,
                r#"{"error":{"code":"conflict","hint":"Invalid or missing auth token"}}"#,
            )],
            port_only,
            1,
            2,
        ),
    ] {
        rig.check(&c.with_env(with_passport(&[])));
    }
    let auth = |log: &[String], i: usize| -> String {
        log[i]
            .lines()
            .find(|l| l.starts_with("Authorization: "))
            .unwrap_or_else(|| panic!("request {i} has no Authorization line: {}", log[i]))
            .to_string()
    };
    let body = |log: &[String], i: usize| -> String {
        log[i]
            .split_once("\n\n")
            .unwrap_or_else(|| panic!("request {i} has no body: {}", log[i]))
            .1
            .to_string()
    };
    // The named difference: the gate's refusal, then the call again with
    // the healed passport, once.
    let healed = case(
        "the gate's refusal: the binary heals once",
        append,
        &[],
        vec![heal(&rig), ok(TOOL)],
        port_only,
        3,
        1,
    )
    .with_env(with_passport(&[]));
    let (out, log) = rig.binary(&healed, false);
    assert_eq!(out.code, Some(0), "the binary after its heal: {out:?}");
    assert_eq!(out.stdout, "line one\nline two é\n", "{out:?}");
    assert_eq!(log.len(), 2, "the refused call and its one retry:\n{log:#?}");
    assert!(log.iter().all(|r| r.contains("POST /cli/docs/tool ")), "{log:#?}");
    assert_eq!(auth(&log, 0), format!("Authorization: Bearer {PASSPORT}"));
    assert_eq!(auth(&log, 1), format!("Authorization: Bearer {HEALED}"));
    assert_eq!(body(&log, 0), body(&log, 1), "the same call, sent again");
    assert_eq!(rig.stub().left(), 0);
    // The shell: the refusal, never sent again (its EXIT trap's probe
    // asks with the healed passport, which the stub's 200 takes).
    rig.prepare(Side::Bash, &healed);
    let out = rig.run(true, &healed);
    let sent = rig.read(Side::Bash, false, healed.name);
    assert_eq!(out.code, Some(3), "the shell: {out:?}");
    let tools = sent
        .iter()
        .filter(|r| String::from_utf8_lossy(&r.bytes).starts_with("POST /cli/docs/tool "))
        .count();
    assert_eq!(tools, 1, "the shell sends the tool call once");
    // Exactly once: a second refusal of the healed passport ends it
    // (exit 3); the tool call is not sent a third time (the closing
    // probe after it is a GET of its own).
    let twice = case(
        "the gate's refusal twice: one heal, then the refusal",
        append,
        &[],
        vec![heal(&rig), Reply::Answer(403, REFUSED.into())],
        port_only,
        3,
        1,
    )
    .with_env(with_passport(&[]));
    let (out, log) = rig.binary(&twice, false);
    assert_eq!(out.code, Some(3), "{out:?}");
    let tools: Vec<&String> = log
        .iter()
        .filter(|r| r.contains("POST /cli/docs/tool "))
        .collect();
    assert_eq!(tools.len(), 2, "one call and one retry, no more:\n{log:#?}");
}
