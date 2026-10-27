//! App guests never see a host path (0.46.5 patch-up item 14, Agent NSI
//! Vanilla).
//!
//! A K2 system line on a room's Thread (`post_workspace_note`: a test
//! guest added, a `k2 job` change) was posted with the workspace's
//! ADDRESS as both sender and recipient, and the address it was given is
//! the workspace's absolute folder (`w.path`). A guest reading the room
//! saw `/home/<owner>/AI Projects/<workspace>` as the sender line.
//!
//! Two layers:
//! - at the source, `overlay_routes::resolve_addr` answers a workspace
//!   named by its folder with the workspace's address name, so new posts
//!   store the handle;
//! - on every guest-facing read (the Thread page and its deltas, inbox
//!   catch-up, `thread/latest`, the overlay socket and the room stream),
//!   [`GuestScrub`] rewrites what was stored before: a sender or recipient
//!   that is a host path becomes the room's address name, and the room's
//!   folder inside any other string of the doc becomes that name too.
//!
//! The owner, Connect logins and agents keep the stored doc as is.

use serde_json::Value;

/// True for a string that names a place on the host's disk: an absolute
/// Unix path, a `~` path, a Windows drive path or anything with a
/// backslash. Addresses (`sales`, `ws/review`, `sales::host.k2.dev`,
/// `import:…`) are never host paths.
pub(crate) fn is_host_path(s: &str) -> bool {
    let s = s.trim();
    let b = s.as_bytes();
    s.starts_with('/')
        || s == "~"
        || s.starts_with("~/")
        || s.contains('\\')
        || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\'))
}

/// One room's guest view: its folder (as registered and as resolved on
/// disk) and the name a guest sees in its place.
#[derive(Debug, Clone, Default)]
pub(crate) struct GuestScrub {
    /// Longest first, never empty or `/`.
    roots: Vec<String>,
    name: String,
}

impl GuestScrub {
    pub(crate) fn new(root: Option<&str>, name: &str) -> Self {
        let mut roots: Vec<String> = Vec::new();
        if let Some(r) = root.map(|r| r.trim().trim_end_matches('/')).filter(|r| r.len() > 1) {
            roots.push(r.to_string());
            if let Ok(real) = dunce::canonicalize(&r) {
                let real = real.to_string_lossy().trim_end_matches('/').to_string();
                if real.len() > 1 && !roots.contains(&real) {
                    roots.push(real);
                }
            }
        }
        roots.sort_by_key(|r| std::cmp::Reverse(r.len()));
        let name = name.trim();
        Self { roots, name: if name.is_empty() { "agent".to_string() } else { name.to_string() } }
    }

    /// The room `project_id`'s folder and address name, from the database.
    pub(crate) fn for_room(project_id: &str) -> Self {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let root: Option<String> = conn
            .query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![project_id], |r| r.get(0))
            .ok();
        let name = k2_core::workspace_session_handles::workspace_address_name(&conn, project_id).unwrap_or_default();
        drop(conn);
        Self::new(root.as_deref(), &name)
    }

    #[cfg(test)]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    fn text(&self, s: &str) -> Option<String> {
        let mut out: Option<String> = None;
        for r in &self.roots {
            let cur = out.as_deref().unwrap_or(s);
            if cur.contains(r.as_str()) {
                out = Some(cur.replace(r.as_str(), &self.name));
            }
        }
        out
    }

    fn walk(&self, v: &mut Value) {
        match v {
            Value::String(s) => {
                if let Some(t) = self.text(s) {
                    *s = t;
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| self.walk(x)),
            Value::Object(o) => o.values_mut().for_each(|x| self.walk(x)),
            _ => {}
        }
    }

    /// A serialized Thread doc (or a `thread/latest` row) as a guest sees
    /// it: `from` / `to` that are host paths become the room's name, and
    /// the room's folder anywhere else in it becomes that name.
    pub(crate) fn doc(&self, doc: &mut Value) {
        if let Some(o) = doc.as_object_mut() {
            for key in ["from", "to"] {
                if let Some(Value::String(s)) = o.get(key) {
                    if is_host_path(s) {
                        o.insert(key.to_string(), Value::String(self.name.clone()));
                    }
                }
            }
        }
        self.walk(doc);
    }

    /// Any guest-facing JSON body (a ticket `show`, 0.46.6 TA17): the
    /// room's folder in any string becomes the room's name.
    pub(crate) fn json(&self, v: &mut Value) {
        self.walk(v);
    }

    /// Thread page items (`[{…, doc}]`).
    pub(crate) fn items(&self, items: &mut Value) {
        if let Some(arr) = items.as_array_mut() {
            for it in arr.iter_mut() {
                if let Some(d) = it.get_mut("doc") {
                    self.doc(d);
                }
            }
        }
    }

    /// One socket frame's JSON text: its `doc`, when it has one.
    pub(crate) fn frame_json(&self, json: String) -> String {
        let Ok(mut v) = serde_json::from_str::<Value>(&json) else { return json };
        match v.get_mut("doc") {
            Some(d) if d.is_object() => {
                self.doc(d);
                v.to_string()
            }
            _ => json,
        }
    }
}

/// The name a guest sees for a project chat `author` that is a host path
/// (an agent that posted with its folder as `author`): that workspace's
/// address name, else the folder's last part. `None` when `author` is not
/// a host path (keep it).
pub(crate) fn author_name(author: &str) -> Option<String> {
    if !is_host_path(author) {
        return None;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let named = k2_core::workspace::agent_identity::resolve_project_id(&conn, author.trim())
        .and_then(|id| k2_core::workspace_session_handles::workspace_address_name(&conn, &id).ok());
    drop(conn);
    Some(named.unwrap_or_else(|| {
        let t = author.trim().trim_end_matches(['/', '\\']);
        let base = t.rsplit(['/', '\\']).next().unwrap_or("").trim();
        if base.is_empty() || base == "~" { "agent".to_string() } else { base.to_string() }
    }))
}

/// Every `author` in a project chat answer or frame that is a host path
/// becomes [`author_name`].
pub(crate) fn project_authors(v: &mut Value) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(project_authors),
        Value::Object(o) => {
            if let Some(Value::String(s)) = o.get("author") {
                if let Some(n) = author_name(s) {
                    o.insert("author".into(), Value::String(n));
                }
            }
            o.values_mut().for_each(project_authors);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn host_paths_are_spotted_and_addresses_are_not() {
        for p in ["/home/a/AI Projects/x", "/Users/z/x", "/tmp/x", "~", "~/x", "C:\\Users\\x", "c:/x", "a\\b"] {
            assert!(is_host_path(p), "{p}");
        }
        for a in ["sales", "ws/review", "sales::host.k2.dev", "import:abc", "k2", "", "agent-nsi-vanilla", "dana"] {
            assert!(!is_host_path(a), "{a}");
        }
    }

    #[test]
    fn a_doc_loses_the_rooms_folder_and_path_senders() {
        let s = GuestScrub::new(Some("/home/alakazamlabs/AI Projects/agent-nsi-vanilla/"), "agent-nsi-vanilla");
        let mut d = json!({
            "from": "/home/alakazamlabs/AI Projects/agent-nsi-vanilla",
            "to": "/srv/elsewhere",
            "body": "K2: saved /home/alakazamlabs/AI Projects/agent-nsi-vanilla/out/a.txt",
            "choice": {"prompt": "in /home/alakazamlabs/AI Projects/agent-nsi-vanilla?"},
            "author": {"kind": "guest", "login": "dana"},
        });
        s.doc(&mut d);
        let t = d.to_string();
        assert!(!t.contains("/home/"), "{t}");
        assert!(!t.contains("/srv/"), "a path recipient is renamed too: {t}");
        assert_eq!(d["from"], "agent-nsi-vanilla");
        assert_eq!(d["to"], "agent-nsi-vanilla");
        assert_eq!(d["body"], "K2: saved agent-nsi-vanilla/out/a.txt");
        assert_eq!(d["author"]["login"], "dana", "other fields untouched");
        // An address sender stays.
        let mut d = json!({"from": "sales", "to": "agent-nsi-vanilla", "body": "hi"});
        s.doc(&mut d);
        assert_eq!(d, json!({"from": "sales", "to": "agent-nsi-vanilla", "body": "hi"}));
    }

    #[test]
    fn a_root_of_slash_or_nothing_never_rewrites_text() {
        for root in [None, Some("/"), Some(""), Some("  ")] {
            let s = GuestScrub::new(root, "x");
            let mut d = json!({"from": "sales", "body": "/a/b and / c"});
            s.doc(&mut d);
            assert_eq!(d["body"], "/a/b and / c", "{root:?}");
        }
        assert_eq!(GuestScrub::new(None, " ").name(), "agent");
    }

    #[test]
    fn project_chat_path_authors_lose_the_path() {
        // Not a registered workspace: the folder's last part.
        let mut v = json!({"messages": [
            {"author": "/home/u/AI Projects/zz-unregistered-ws", "body": "a"},
            {"author": "owner", "body": "b"},
            {"author": "guest:dana", "body": "c"},
        ]});
        project_authors(&mut v);
        assert_eq!(v["messages"][0]["author"], "zz-unregistered-ws");
        assert_eq!(v["messages"][1]["author"], "owner");
        assert_eq!(v["messages"][2]["author"], "guest:dana");
        assert_eq!(author_name("sales"), None);
        assert_eq!(author_name("/").as_deref(), Some("agent"));
    }

    #[test]
    fn frames_keep_their_shape() {
        let s = GuestScrub::new(Some("/tmp/ws1"), "ws1");
        let f = json!({"collection": "thread", "seq": 3, "id": "d", "doc": {"from": "/tmp/ws1", "body": "x"}}).to_string();
        let v: Value = serde_json::from_str(&s.frame_json(f)).unwrap();
        assert_eq!(v, json!({"collection": "thread", "seq": 3, "id": "d", "doc": {"from": "ws1", "body": "x"}}));
        let no_doc = json!({"collection": "activity", "seq": 0, "id": "a", "activity": {"x": 1}}).to_string();
        assert_eq!(s.frame_json(no_doc.clone()), no_doc);
    }
}
