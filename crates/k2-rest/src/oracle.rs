//! The oracle rig (PRD §12): real PostgREST as a TEST ORACLE, never
//! shipped. Two databases load the same fixture with the same row policies;
//! PostgREST serves one, K2's grammar the other; a corpus of the reference
//! app's request shapes (synthetic values) runs on both, as anon, as a
//! signed-in user and as the service role, and every difference is listed.
//!
//! What is compared: the status; `Content-Range` (reads); the body as JSON
//! values (key order ignored). Error answers compare status and `code` only
//! (the words differ between versions). A K2 refusal of a part v1 doesn't
//! carry (`K2REST001`) is never a match: the corpus holds only parts v1
//! carries.
//!
//! Runs only where ORACLE_POSTGREST names a PostgREST binary (12.x) and
//! REALPG_COMPAT=1 (a compute job: `scripts/test/compat-doors.sh oracle`).

use std::time::Duration;

use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::tests::real::{db, exec, Answer, Db};
use crate::testkit;
use crate::Request;

const SECRET: &str = "k2-oracle-jwt-secret-of-at-least-32-characters";
pub const ANN: &str = "11111111-1111-4111-8111-111111111111";

/// The row policies both sides load on top of the fixture.
pub const POLICIES: &str = "
ALTER TABLE public.projects ENABLE ROW LEVEL SECURITY;
CREATE POLICY anon_open ON public.projects FOR SELECT TO anon USING (status = 'open');
CREATE POLICY mine ON public.projects FOR ALL TO authenticated
  USING (owner_id = auth.uid() OR reviewer_id = auth.uid()) WITH CHECK (owner_id = auth.uid());
ALTER TABLE public.tasks ENABLE ROW LEVEL SECURITY;
CREATE POLICY via_project ON public.tasks FOR ALL TO authenticated
  USING (EXISTS (SELECT 1 FROM public.projects p WHERE p.id = project_id AND p.owner_id = auth.uid()));
CREATE POLICY anon_tasks ON public.tasks FOR SELECT TO anon USING (true);
ALTER TABLE public.devices ENABLE ROW LEVEL SECURITY;
CREATE POLICY own_devices ON public.devices FOR ALL TO authenticated USING (owner_id = auth.uid()) WITH CHECK (owner_id = auth.uid());
";

/// Who a request runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    Anon,
    Ann,
    Service,
}

impl Who {
    fn role(self) -> &'static str {
        match self {
            Who::Anon => "anon",
            Who::Ann => "authenticated",
            Who::Service => "service_role",
        }
    }
    fn claims(self) -> Value {
        match self {
            Who::Anon => json!({"role": "anon"}),
            Who::Ann => json!({"role": "authenticated", "sub": ANN, "email": "ann@example.com", "aud": "authenticated"}),
            Who::Service => json!({"role": "service_role"}),
        }
    }
    /// PostgREST's bearer: an HS256 JWT of the claims.
    fn jwt(self) -> String {
        let b = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let mut c = self.claims();
        c["exp"] = json!(4_102_444_800i64);
        let head = b.encode(r#"{"alg":"HS256","typ":"JWT"}"#);
        let body = b.encode(c.to_string());
        let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).expect("key");
        mac.update(format!("{head}.{body}").as_bytes());
        format!("{head}.{body}.{}", b.encode(mac.finalize().into_bytes()))
    }
}

/// One corpus entry.
#[derive(Debug, Clone)]
pub struct Case {
    pub who: Who,
    pub method: &'static str,
    /// After `/rest/v1`: `/projects?select=…`.
    pub target: String,
    pub headers: Vec<(&'static str, &'static str)>,
    pub body: &'static str,
}

fn case(who: Who, method: &'static str, target: &str, headers: &[(&'static str, &'static str)], body: &'static str) -> Case {
    Case { who, method, target: target.to_string(), headers: headers.to_vec(), body }
}

/// The corpus: the shapes the reference app's calls take (PRD §7.2), with
/// the fixture's values. Order matters (writes change what later reads see,
/// the same on both sides).
pub fn corpus() -> Vec<Case> {
    use Who::*;
    let rep: &[(&'static str, &'static str)] = &[("Prefer", "return=representation")];
    let count: &[(&'static str, &'static str)] = &[("Prefer", "count=exact")];
    let obj: &[(&'static str, &'static str)] = &[("Accept", "application/vnd.pgrst.object+json")];
    let mut v = Vec::new();
    for who in [Anon, Ann, Service] {
        for t in [
            "/projects?select=*&order=id",
            "/projects?select=id,title&status=eq.open&order=id.desc",
            "/projects?select=id&id=in.(1,3,5)&order=id",
            "/projects?select=id&due=is.null&order=id",
            "/projects?select=id&due=not.is.null&order=due.desc.nullslast",
            "/projects?select=id&title=ilike.*a*&order=id",
            "/projects?select=id&title=like.*a&order=id",
            "/projects?select=id&tags=cs.{red}&order=id",
            "/projects?select=id&data=cs.{\"kind\":\"web\"}&order=id",
            "/projects?select=id,kind:data->>kind,score:data->score&order=id",
            "/projects?select=id&data->>kind=eq.web&order=id",
            "/projects?select=id&or=(status.eq.closed,id.eq.5)&order=id",
            "/projects?select=id&not.or=(status.eq.open,status.eq.closed)&order=id",
            "/projects?select=id&status=neq.open&org_id=eq.2&order=id",
            "/projects?select=id,owner:profiles!owner_id(name,org:orgs(name))&order=id",
            "/projects?select=id,tasks(title,done)&tasks.order=pos.asc.nullslast&order=id",
            "/projects?select=id,tasks!inner(title)&tasks.done=eq.true&order=id",
            "/projects?select=id,project_settings(theme)&order=id",
            "/projects?select=id,reviewer:profiles!reviewer_id(name)&order=id",
            "/projects?select=id&order=id&limit=2&offset=1",
            "/orgs?select=name,projects(id)&projects.order=id.desc&order=id",
            "/orgs?select=id,profiles(name)&profiles.limit=1&order=id",
            "/tasks?select=id,title,project:projects(title)&order=id",
            "/profiles?select=id,meta->role,langs:meta->langs&order=id",
            "/devices?select=*",
            "/open_projects_v?select=*&order=id",
            "/rpc/tag_list",
            "/rpc/add?a=2&b=5",
            "/rpc/org_projects?p_org=1&select=id,title&order=id",
            "/rpc/first_project?select=id,title",
            "/rpc/project_titles?pid=gt.2",
            "/projects?select=id,owner:profiles!owner_id!inner(name)&owner.name=eq.Ann&order=id",
            "/projects?select=id,owner:profiles!owner_id(name)&owner.name=eq.Ann&order=id",
            "/projects?select=id&due=gte.2026-02-01&order=id",
            "/projects?select=id&title=match.^[A-G]&order=id",
            "/projects?select=id&title=imatch.^d&order=id",
            "/projects?select=id&tags=cd.{red,blue}&order=id",
            "/projects?select=id,budget::text&order=budget.desc.nullsfirst,id",
            "/projects?select=id&order=data->>kind.asc,id.desc",
            "/projects?select=id,tasks(id)&tasks.limit=1&tasks.order=id.desc&order=id",
            "/projects?select=id&and=(status.eq.open,org_id.eq.1)&order=id",
            "/projects?select=id&id=not.in.(1,2)&order=id",
            "/tasks?select=id&done=is.true&order=id",
            "/tasks?select=id&pos=is.null",
            "/profiles?select=name&name=not.is.null&order=name.desc",
            "/projects?select=title&title=like.*%_*",
        ] {
            v.push(case(who, "GET", t, &[], ""));
        }
        v.push(case(who, "GET", "/projects?select=id&order=id&limit=2", count, ""));
        v.push(case(who, "HEAD", "/projects?select=*", count, ""));
        v.push(case(who, "GET", "/projects?select=id,title&id=eq.1", obj, ""));
        v.push(case(who, "GET", "/projects?select=id&status=eq.open", obj, ""));
        v.push(case(who, "GET", "/projects?select=*,profiles(*)", &[], ""));
        v.push(case(who, "GET", "/nope", &[], ""));
        v.push(case(who, "POST", "/rpc/add", &[], r#"{"a": 1, "b": 2}"#));
        v.push(case(who, "POST", "/rpc/echo_json", &[], r#"{"x": [1, "two", null]}"#));
    }
    // Writes, as the signed-in user and the service role.
    v.push(case(Ann, "POST", "/projects?select=id,title,status", rep, r#"{"org_id": 1, "owner_id": "11111111-1111-4111-8111-111111111111", "title": "Ann new"}"#));
    v.push(case(Ann, "POST", "/projects", &[], r#"{"org_id": 1, "owner_id": "22222222-2222-4222-8222-222222222222", "title": "forged"}"#));
    v.push(case(Anon, "POST", "/projects", &[], r#"{"org_id": 1, "title": "anon"}"#));
    v.push(case(Service, "POST", "/tasks?columns=\"project_id\",\"title\",\"pos\"&select=id,title,pos", rep, r#"[{"project_id": 1, "title": "t1", "pos": 9}, {"project_id": 1, "title": "t2"}]"#));
    v.push(case(Service, "POST", "/orgs", &[], r#"{"id": 1, "name": "dup"}"#));
    v.push(case(Service, "POST", "/orgs?select=*", &[("Prefer", "return=representation,resolution=merge-duplicates")], r#"[{"id": 1, "name": "Acme 2"}, {"id": 7, "name": "Seven"}]"#));
    v.push(case(Service, "POST", "/orgs?select=*", &[("Prefer", "return=representation,resolution=ignore-duplicates")], r#"[{"id": 1, "name": "no"}, {"id": 8, "name": "Eight"}]"#));
    v.push(case(Ann, "PATCH", "/projects?id=eq.1&select=id,status", rep, r#"{"status": "review"}"#));
    v.push(case(Ann, "PATCH", "/projects?id=eq.3", rep, r#"{"status": "stolen"}"#));
    v.push(case(Service, "PATCH", "/projects?status=eq.open&select=id", &[("Prefer", "return=representation,count=exact")], r#"{"budget": 1}"#));
    v.push(case(Service, "DELETE", "/orgs?id=in.(7,8)&select=id", rep, ""));
    v.push(case(Ann, "DELETE", "/tasks?id=eq.5", rep, ""));
    v.push(case(Service, "POST", "/rpc/close_project", &[], r#"{"p_id": 4}"#));
    v.push(case(Service, "POST", "/devices?on_conflict=owner_id,label&select=label", &[("Prefer", "return=representation,resolution=merge-duplicates")], r#"{"id": "aaaaaaaa-0000-4000-8000-000000000002", "owner_id": "11111111-1111-4111-8111-111111111111", "label": "phone"}"#));
    v.push(case(Service, "POST", "/projects?columns=\"id\",\"org_id\",\"title\",\"status\"&select=id,status", &[("Prefer", "return=representation,missing=default")], r#"[{"id": 30, "org_id": 1, "title": "m1"}, {"id": 31, "org_id": 2, "title": "m2", "status": "closed"}]"#));
    v.push(case(Service, "PATCH", "/tasks?id=in.(1,2)&select=id,done", rep, r#"{"done": false}"#));
    v.push(case(Service, "PATCH", "/projects?id=eq.99", &[("Prefer", "return=representation"), ("Accept", "application/vnd.pgrst.object+json")], r#"{"title": "x"}"#));
    v.push(case(Anon, "GET", "/projects?select=id&order=id", &[], ""));
    v.push(case(Service, "GET", "/projects?select=id,status,budget&order=id", &[], ""));
    v.push(case(Service, "GET", "/tasks?select=id,title,pos&order=id", &[], ""));
    v
}

fn to_request(c: &Case) -> Request {
    let (path, query) = c.target.split_once('?').unwrap_or((c.target.as_str(), ""));
    let pairs = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (p.to_string(), String::new()),
        })
        .collect();
    let mut headers: Vec<(String, String)> = c.headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())).collect();
    if !c.body.is_empty() {
        headers.push(("content-type".into(), "application/json".into()));
    }
    Request { method: c.method.into(), path: path.into(), query: pairs, headers, body: c.body.as_bytes().to_vec() }
}

/// What a side answered, reduced to what is compared.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub status: u16,
    pub range: Option<String>,
    /// Error answers: `{"code": …}` only.
    pub body: Value,
}

fn reduce(status: u16, range: Option<String>, body: Value, method: &str) -> Seen {
    let body = if status >= 300 { json!({ "code": body.get("code").cloned().unwrap_or(Value::Null) }) } else { body };
    // Content-Range on reads only (writes word it differently by version).
    let range = if matches!(method, "GET" | "HEAD") { range } else { None };
    Seen { status, range, body }
}

/// PostgREST's answer over plain HTTP/1.0 (a body ends with the connection).
async fn postgrest(port: u16, c: &Case) -> Seen {
    // Query values as sent on the wire: spaces and quotes encoded.
    let target = c.target.replace('%', "%25").replace(' ', "%20").replace('"', "%22").replace('{', "%7B").replace('}', "%7D").replace('>', "%3E");
    let mut req = format!("{} /{} HTTP/1.0\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\n", c.method, target.trim_start_matches('/'), c.who.jwt());
    for (k, v) in &c.headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if !c.body.is_empty() {
        req.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", c.body.len()));
    }
    req.push_str("\r\n");
    req.push_str(c.body);
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.expect("PostgREST");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(30), s.read_to_end(&mut raw)).await.expect("PostgREST answered").unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status: u16 = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).expect("a status line");
    let range = head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-range")).map(|(_, v)| v.trim().to_string()));
    let body: Value = if body.trim().is_empty() { Value::Null } else { serde_json::from_str(body).unwrap_or_else(|_| Value::String(body.to_string())) };
    reduce(status, range, body, c.method)
}

async fn k2(d: &mut Db, c: &Case) -> Seen {
    let a: Answer = exec(d, c.who.role(), c.who.claims(), to_request(c)).await;
    let range = a.header("Content-Range").map(str::to_string);
    reduce(a.status, range, a.body, c.method)
}

/// Start PostgREST on `db` (the authenticator's login), wait for it.
async fn start_postgrest(bin: &str, s: &testkit::Server, db: &str) -> (std::process::Child, u16) {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let conf = s.root.join(format!("{db}.postgrest.conf"));
    std::fs::write(
        &conf,
        format!(
            "db-uri = \"postgres://{db}_authenticator:{pw}@127.0.0.1:{pg}/{db}\"\ndb-schemas = \"public\"\ndb-anon-role = \"anon\"\n\
             jwt-secret = \"{SECRET}\"\nserver-host = \"127.0.0.1\"\nserver-port = {port}\ndb-max-rows = 1000\ndb-pool = 4\n",
            pw = testkit::AUTHENTICATOR_PASSWORD,
            pg = s.port
        ),
    )
    .unwrap();
    let log = std::fs::File::create(s.root.join(format!("{db}.postgrest.log"))).unwrap();
    let child = std::process::Command::new(bin).arg(&conf).stdout(log.try_clone().unwrap()).stderr(log).spawn().expect("start PostgREST");
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            // Ready once it has read the schema cache.
            let probe = Case { who: Who::Service, method: "GET", target: "/orgs?select=id&limit=1".into(), headers: vec![], body: "" };
            if postgrest(port, &probe).await.status == 200 {
                return (child, port);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("PostgREST never answered: {}", std::fs::read_to_string(s.root.join(format!("{db}.postgrest.log"))).unwrap_or_default());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "the oracle rig: ORACLE_POSTGREST=<postgrest binary> and REALPG_COMPAT=1 on a compute node"]
async fn oracle_postgrest_answers_match() {
    let bin = std::env::var("ORACLE_POSTGREST").expect("ORACLE_POSTGREST names the PostgREST binary");
    let s = testkit::shared();
    let mut ours = db("oracle_k2").await;
    let theirs = db("oracle_pgrst").await;
    s.su(&ours.name, &[POLICIES]);
    s.su(&theirs.name, &[POLICIES]);
    // Our cache was read before the policies; nothing in it changed.
    let (mut child, port) = start_postgrest(&bin, s, &theirs.name).await;
    let mut diffs = Vec::new();
    let cases = corpus();
    for c in &cases {
        let a = k2(&mut ours, c).await;
        let b = postgrest(port, c).await;
        if a != b {
            diffs.push(format!("{:?} {} {}\n  K2:        {} {:?} {}\n  PostgREST: {} {:?} {}", c.who, c.method, c.target, a.status, a.range, a.body, b.status, b.range, b.body));
        }
    }
    let _ = child.kill();
    eprintln!("[oracle] {} requests, {} differences", cases.len(), diffs.len());
    assert!(diffs.is_empty(), "{} of {} answers differ from PostgREST:\n{}", diffs.len(), cases.len(), diffs.join("\n"));
}
