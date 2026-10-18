//! Server-side dashboard layout edits (0.46.1 project settings fix, D1).
//!
//! A project dashboard's `layout_json` is the §6.8 v2 tree the renderer
//! draws (`src/renderer/components/Projects/dashboard-layout.ts`):
//! `{"version":2,"root":<node>}` where a node is
//! `{"type":"pane","pane":{kind,...}}` or
//! `{"type":"split","dir":"row"|"col","children":[{size,node}]}`.
//! v1 blobs (`{"version":1,"columns":[{widthPct,pane}]}`) convert on
//! read, and a blob with neither `root` nor `columns` (the
//! `EMPTY_LAYOUT_V1` seed 'Main' is born with) is *untouched*: it draws
//! the Point of Contact's terminal pane.
//!
//! Before this module, "Add to dashboard" read the cached blob in the
//! renderer, appended a pane and POSTed the whole blob back
//! (last write wins), so a change another client saved in between was
//! lost. [`add_pane`] / [`remove_pane`] do the read, the edit and the
//! write under ONE hold of the shared DB lock, so two clients adding at
//! the same time both land. The tree rules mirror the renderer exactly:
//! a new pane goes on the right edge (an equal share of a row root, or a
//! 50/50 wrap), a removed pane's split renormalizes and single-child
//! splits collapse, nested same-direction splits merge, sizes sum 100
//! and round to two decimals. Unknown pane kinds round-trip untouched.

use rusqlite::params;
use serde_json::{json, Map, Value};

use crate::project_groups::{get_dashboard, ProjectGroupDashboard};

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Pane(Value),
    Split { dir: String, children: Vec<(f64, Node)> },
}

/// What a stored blob reads as.
#[derive(Debug, Clone, PartialEq)]
enum Parsed {
    /// Never saved by a client: draws the PoC seed.
    Untouched,
    /// A saved layout; `None` = deliberately emptied.
    Layout(Option<Node>),
}

/// A pane the add/remove routes accept: an agent's terminal or an HTML
/// file. Its identity is the renderer's `paneKey` (`t:<ws>` /
/// `h:<ws>:<path>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneSpec {
    Terminal { workspace_id: String },
    HtmlDoc { workspace_id: String, file_path: String },
}

impl PaneSpec {
    fn to_value(&self) -> Value {
        match self {
            PaneSpec::Terminal { workspace_id } => {
                json!({ "kind": "terminal", "workspaceId": workspace_id })
            }
            PaneSpec::HtmlDoc { workspace_id, file_path } => {
                json!({ "kind": "htmlDoc", "workspaceId": workspace_id, "filePath": file_path })
            }
        }
    }

    fn matches(&self, pane: &Value) -> bool {
        let kind = pane.get("kind").and_then(Value::as_str);
        let ws = pane.get("workspaceId").and_then(Value::as_str);
        match self {
            PaneSpec::Terminal { workspace_id } => {
                kind == Some("terminal") && ws == Some(workspace_id.as_str())
            }
            PaneSpec::HtmlDoc { workspace_id, file_path } => {
                kind == Some("htmlDoc")
                    && ws == Some(workspace_id.as_str())
                    && pane.get("filePath").and_then(Value::as_str) == Some(file_path.as_str())
            }
        }
    }

    pub fn workspace_id(&self) -> &str {
        match self {
            PaneSpec::Terminal { workspace_id } | PaneSpec::HtmlDoc { workspace_id, .. } => {
                workspace_id
            }
        }
    }
}

/// The outcome of an add or remove.
#[derive(Debug, Clone)]
pub struct PaneEdit {
    pub dashboard: ProjectGroupDashboard,
    /// false = nothing to do (already there / not there); the blob and
    /// revision are untouched.
    pub changed: bool,
}

fn parse_pane(raw: &Value) -> Option<Value> {
    match raw {
        Value::Object(m) if m.get("kind").is_some_and(Value::is_string) => Some(raw.clone()),
        _ => None,
    }
}

fn parse_node(raw: &Value) -> Option<Node> {
    let obj = raw.as_object()?;
    match obj.get("type").and_then(Value::as_str) {
        Some("pane") => parse_pane(obj.get("pane")?).map(Node::Pane),
        Some("split") => {
            let dir = obj.get("dir").and_then(Value::as_str)?;
            if dir != "row" && dir != "col" {
                return None;
            }
            let mut children = Vec::new();
            for entry in obj.get("children")?.as_array()? {
                let Some(e) = entry.as_object() else { continue };
                let Some(node) = e.get("node").and_then(parse_node) else { continue };
                let size = e.get("size").and_then(Value::as_f64).unwrap_or(f64::NAN);
                children.push((size, node));
            }
            if children.is_empty() {
                return None;
            }
            Some(Node::Split { dir: dir.to_string(), children })
        }
        _ => None,
    }
}

fn normalize_sizes(children: Vec<(f64, Node)>) -> Vec<(f64, Node)> {
    if children.is_empty() {
        return children;
    }
    let sane = children.iter().all(|(s, _)| s.is_finite() && *s > 0.0);
    if !sane {
        let equal = 100.0 / children.len() as f64;
        return children.into_iter().map(|(_, n)| (equal, n)).collect();
    }
    let sum: f64 = children.iter().map(|(s, _)| s).sum();
    if (sum - 100.0).abs() < 0.01 {
        return children;
    }
    children.into_iter().map(|(s, n)| (s / sum * 100.0, n)).collect()
}

fn normalize(node: Node) -> Node {
    let Node::Split { dir, children } = node else { return node };
    let mut flat = Vec::new();
    for (size, child) in normalize_sizes(children) {
        match normalize(child) {
            Node::Split { dir: inner_dir, children: grand } if inner_dir == dir => {
                for (g, n) in grand {
                    flat.push((size * g / 100.0, n));
                }
            }
            inner => flat.push((size, inner)),
        }
    }
    if flat.len() == 1 {
        return flat.pop().map(|(_, n)| n).expect("one child");
    }
    Node::Split { dir, children: normalize_sizes(flat) }
}

fn convert_v1_columns(cols: &[Value]) -> Option<Node> {
    let mut columns = Vec::new();
    for entry in cols {
        let Some(e) = entry.as_object() else { continue };
        let Some(pane) = e.get("pane").and_then(parse_pane) else { continue };
        let width = e.get("widthPct").and_then(Value::as_f64).unwrap_or(f64::NAN);
        columns.push((width, Node::Pane(pane)));
    }
    match columns.len() {
        0 => None,
        1 => columns.pop().map(|(_, n)| n),
        _ => Some(normalize(Node::Split { dir: "row".to_string(), children: columns })),
    }
}

fn parse_layout(layout_json: &str) -> Parsed {
    let Ok(raw) = serde_json::from_str::<Value>(layout_json) else {
        return Parsed::Untouched;
    };
    let Some(obj) = raw.as_object() else { return Parsed::Untouched };
    if let Some(root) = obj.get("root") {
        if root.is_null() {
            return Parsed::Layout(None);
        }
        return match parse_node(root) {
            Some(n) => Parsed::Layout(Some(normalize(n))),
            None => Parsed::Untouched,
        };
    }
    if let Some(cols) = obj.get("columns").and_then(Value::as_array) {
        return Parsed::Layout(convert_v1_columns(cols));
    }
    Parsed::Untouched
}

/// The tree a client draws for this blob (untouched → the PoC seed).
fn adopt(layout_json: &str, poc_workspace_id: Option<&str>) -> Option<Node> {
    match parse_layout(layout_json) {
        Parsed::Untouched => poc_workspace_id
            .map(|ws| Node::Pane(json!({ "kind": "terminal", "workspaceId": ws }))),
        Parsed::Layout(root) => root,
    }
}

fn round2(n: f64) -> f64 {
    (n * 100.0).round() / 100.0
}

fn serialize_node(node: &Node) -> Value {
    match node {
        Node::Pane(p) => json!({ "type": "pane", "pane": p }),
        Node::Split { dir, children } => json!({
            "type": "split",
            "dir": dir,
            "children": children
                .iter()
                .map(|(s, n)| json!({ "size": round2(*s), "node": serialize_node(n) }))
                .collect::<Vec<_>>(),
        }),
    }
}

fn serialize(root: Option<&Node>) -> String {
    let mut m = Map::new();
    m.insert("version".to_string(), json!(2));
    m.insert("root".to_string(), root.map(serialize_node).unwrap_or(Value::Null));
    Value::Object(m).to_string()
}

fn contains(node: &Node, spec: &PaneSpec) -> bool {
    match node {
        Node::Pane(p) => spec.matches(p),
        Node::Split { children, .. } => children.iter().any(|(_, n)| contains(n, spec)),
    }
}

/// The renderer's `insertEdge(root, 'right', pane)`.
fn insert_right(root: Option<Node>, pane: Value) -> Node {
    let fresh = Node::Pane(pane);
    match root {
        None => fresh,
        Some(Node::Split { dir, children }) if dir == "row" => {
            let share = 100.0 / (children.len() + 1) as f64;
            let mut scaled: Vec<(f64, Node)> = children
                .into_iter()
                .map(|(s, n)| (s * ((100.0 - share) / 100.0), n))
                .collect();
            scaled.push((share, fresh));
            normalize(Node::Split { dir, children: scaled })
        }
        Some(other) => normalize(Node::Split {
            dir: "row".to_string(),
            children: vec![(50.0, other), (50.0, fresh)],
        }),
    }
}

/// Remove the FIRST pane (reading order) matching `spec`, without
/// normalizing; `(tree, removed)`. `None` tree = it emptied.
fn remove_first(node: Node, spec: &PaneSpec, done: &mut bool) -> Option<Node> {
    match node {
        Node::Pane(p) => {
            if !*done && spec.matches(&p) {
                *done = true;
                None
            } else {
                Some(Node::Pane(p))
            }
        }
        Node::Split { dir, children } => {
            let kept: Vec<(f64, Node)> = children
                .into_iter()
                .filter_map(|(s, n)| remove_first(n, spec, done).map(|n| (s, n)))
                .collect();
            if kept.is_empty() {
                None
            } else {
                Some(Node::Split { dir, children: kept })
            }
        }
    }
}

/// Pure: the blob after adding `spec` on the right edge, or `None` when
/// the dashboard already shows it.
pub fn layout_with_pane(
    layout_json: &str,
    poc_workspace_id: Option<&str>,
    spec: &PaneSpec,
) -> Option<String> {
    let root = adopt(layout_json, poc_workspace_id);
    if root.as_ref().is_some_and(|r| contains(r, spec)) {
        return None;
    }
    Some(serialize(Some(&insert_right(root, spec.to_value()))))
}

/// Pure: the blob after removing `spec`, or `None` when it isn't there.
pub fn layout_without_pane(
    layout_json: &str,
    poc_workspace_id: Option<&str>,
    spec: &PaneSpec,
) -> Option<String> {
    let root = adopt(layout_json, poc_workspace_id)?;
    if !contains(&root, spec) {
        return None;
    }
    let mut done = false;
    let next = remove_first(root, spec, &mut done).map(normalize);
    Some(serialize(next.as_ref()))
}

/// Read → edit → write under one hold of the shared DB lock, so a
/// concurrent add/remove/save can't slip in between. `edit` returns the
/// new blob, or `None` for a no-op (no write, no revision bump).
fn edit_layout(
    dashboard_id: &str,
    edit: impl FnOnce(&str, Option<&str>) -> Option<String>,
) -> Result<PaneEdit, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let (layout_json, poc): (String, Option<String>) = conn
        .query_row(
            "SELECT d.layout_json, g.poc_workspace_id FROM project_group_dashboards d \
             JOIN project_groups g ON g.id = d.group_id WHERE d.id = ?1",
            params![dashboard_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| format!("no dashboard with id {dashboard_id}"))?;
    let changed = match edit(&layout_json, poc.as_deref()) {
        Some(next) => {
            conn.execute(
                "UPDATE project_group_dashboards SET layout_json = ?2, \
                 revision = revision + 1, updated_at = ?3 WHERE id = ?1",
                params![dashboard_id, next, chrono_now()],
            )
            .map_err(|e| format!("layout save failed: {e}"))?;
            true
        }
        None => false,
    };
    drop(conn);
    let dashboard =
        get_dashboard(dashboard_id).ok_or_else(|| "dashboard vanished after edit".to_string())?;
    Ok(PaneEdit { dashboard, changed })
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Add one pane to a dashboard (right edge). Idempotent: a pane already
/// shown is `changed: false`. `dashboard_id` must be a FULL id.
pub fn add_pane(dashboard_id: &str, spec: &PaneSpec) -> Result<PaneEdit, String> {
    edit_layout(dashboard_id, |blob, poc| layout_with_pane(blob, poc, spec))
}

/// Remove one pane from a dashboard. A pane that isn't there is
/// `changed: false`. `dashboard_id` must be a FULL id.
pub fn remove_pane(dashboard_id: &str, spec: &PaneSpec) -> Result<PaneEdit, String> {
    edit_layout(dashboard_id, |blob, poc| layout_without_pane(blob, poc, spec))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_groups::{add_member, create_group, list_dashboards, EMPTY_LAYOUT_V1};

    fn term(ws: &str) -> PaneSpec {
        PaneSpec::Terminal { workspace_id: ws.to_string() }
    }

    fn doc(ws: &str, path: &str) -> PaneSpec {
        PaneSpec::HtmlDoc { workspace_id: ws.to_string(), file_path: path.to_string() }
    }

    fn root_of(blob: &str) -> Value {
        let v: Value = serde_json::from_str(blob).expect("json");
        assert_eq!(v["version"], 2, "{blob}");
        v["root"].clone()
    }

    #[test]
    fn untouched_add_materializes_the_poc_seed_then_appends_right() {
        let next = layout_with_pane(EMPTY_LAYOUT_V1, Some("poc"), &doc("w", "/a.html"))
            .expect("added");
        let root = root_of(&next);
        assert_eq!(root["type"], "split");
        assert_eq!(root["dir"], "row");
        assert_eq!(root["children"][0]["node"]["pane"]["workspaceId"], "poc");
        assert_eq!(root["children"][0]["size"], 50.0);
        assert_eq!(root["children"][1]["node"]["pane"]["filePath"], "/a.html");
        assert_eq!(root["children"][1]["size"], 50.0);
    }

    #[test]
    fn add_into_a_row_takes_an_equal_share_and_is_idempotent() {
        let two = layout_with_pane(EMPTY_LAYOUT_V1, Some("poc"), &term("b")).expect("b");
        let three = layout_with_pane(&two, Some("poc"), &term("c")).expect("c");
        let root = root_of(&three);
        let kids = root["children"].as_array().expect("children");
        assert_eq!(kids.len(), 3, "{three}");
        for k in kids {
            assert_eq!(k["size"], 33.33, "{three}");
        }
        assert_eq!(layout_with_pane(&three, Some("poc"), &term("c")), None, "already there");
        assert_eq!(layout_with_pane(&three, Some("poc"), &term("poc")), None, "seed is there");
    }

    #[test]
    fn emptied_layout_stays_empty_until_the_first_add() {
        let empty = r#"{"version":2,"root":null}"#;
        assert_eq!(layout_without_pane(empty, Some("poc"), &term("poc")), None);
        let next = layout_with_pane(empty, Some("poc"), &term("x")).expect("added");
        assert_eq!(root_of(&next), json!({ "type": "pane", "pane": { "kind": "terminal", "workspaceId": "x" } }));
    }

    #[test]
    fn remove_collapses_and_renormalizes_and_keeps_unknown_panes() {
        let blob = json!({
            "version": 2,
            "root": { "type": "split", "dir": "row", "children": [
                { "size": 25, "node": { "type": "pane", "pane": { "kind": "terminal", "workspaceId": "a" } } },
                { "size": 25, "node": { "type": "pane", "pane": { "kind": "mystery", "x": 1 } } },
                { "size": 50, "node": { "type": "split", "dir": "col", "children": [
                    { "size": 50, "node": { "type": "pane", "pane": { "kind": "terminal", "workspaceId": "b" } } },
                    { "size": 50, "node": { "type": "pane", "pane": { "kind": "htmlDoc", "workspaceId": "b", "filePath": "/d.html" } } }
                ] } }
            ] }
        })
        .to_string();
        let next = layout_without_pane(&blob, None, &term("b")).expect("removed");
        let root = root_of(&next);
        let kids = root["children"].as_array().expect("children");
        assert_eq!(kids.len(), 3, "the col split collapsed into its doc: {next}");
        assert_eq!(kids[1]["node"]["pane"], json!({ "kind": "mystery", "x": 1 }));
        assert_eq!(kids[2]["node"]["pane"]["filePath"], "/d.html");
        assert_eq!(layout_without_pane(&next, None, &term("b")), None, "gone already");
        // Removing the rest leaves only the unknown pane, untouched.
        let mut cur = next;
        for spec in [term("a"), doc("b", "/d.html")] {
            cur = layout_without_pane(&cur, None, &spec).expect("removed");
        }
        assert_eq!(root_of(&cur)["pane"]["kind"], "mystery");
        // A last pane removed leaves a deliberate empty root (not the seed).
        let single = layout_with_pane(r#"{"version":2,"root":null}"#, None, &term("z")).expect("z");
        let gone = layout_without_pane(&single, Some("poc"), &term("z")).expect("removed");
        assert_eq!(root_of(&gone), Value::Null);
    }

    #[test]
    fn v1_columns_convert_before_an_add() {
        let v1 = json!({ "version": 1, "columns": [
            { "widthPct": 60, "pane": { "kind": "terminal", "workspaceId": "a" } },
            { "widthPct": 40, "pane": { "kind": "terminal", "workspaceId": "b" } }
        ] })
        .to_string();
        let next = layout_with_pane(&v1, None, &term("c")).expect("added");
        let kids = root_of(&next)["children"].as_array().expect("children").clone();
        assert_eq!(kids.len(), 3);
        assert_eq!(kids[0]["size"], 40.0);
        assert_eq!(kids[2]["node"]["pane"]["workspaceId"], "c");
    }

    /// Two clients adding at the same time: both panes survive, and the
    /// revision counts both writes.
    #[test]
    fn concurrent_adds_from_two_clients_both_survive() {
        let g = create_group(&format!("pdl-conc-{}", uuid::Uuid::new_v4())).expect("group");
        add_member(&g.id, &format!("ws-poc-{}", uuid::Uuid::new_v4())).expect("poc");
        let dash = list_dashboards(&g.id).expect("dashboards").remove(0);
        let before = dash.revision;
        let specs: Vec<PaneSpec> = (0..8)
            .map(|i| doc("w", &format!("/client-{i}.html")))
            .collect();
        let handles: Vec<_> = specs
            .iter()
            .cloned()
            .map(|spec| {
                let id = dash.id.clone();
                std::thread::spawn(move || add_pane(&id, &spec).expect("add"))
            })
            .collect();
        for h in handles {
            assert!(h.join().expect("thread").changed);
        }
        let after = get_dashboard(&dash.id).expect("dashboard");
        assert_eq!(after.revision, before + 8, "every add wrote once");
        let root = adopt(&after.layout_json, None).expect("root");
        for spec in &specs {
            assert!(contains(&root, spec), "lost {spec:?}: {}", after.layout_json);
        }
        // A concurrent add and remove: the remove only takes its pane.
        let id_a = dash.id.clone();
        let id_b = dash.id.clone();
        let rm = specs[0].clone();
        let add = doc("w", "/late.html");
        let add2 = add.clone();
        let t1 = std::thread::spawn(move || remove_pane(&id_a, &rm).expect("remove"));
        let t2 = std::thread::spawn(move || add_pane(&id_b, &add2).expect("add"));
        assert!(t1.join().expect("t1").changed);
        assert!(t2.join().expect("t2").changed);
        let last = get_dashboard(&dash.id).expect("dashboard");
        let root = adopt(&last.layout_json, None).expect("root");
        assert!(!contains(&root, &specs[0]), "{}", last.layout_json);
        assert!(contains(&root, &add), "{}", last.layout_json);
        for spec in &specs[1..] {
            assert!(contains(&root, spec), "lost {spec:?}: {}", last.layout_json);
        }
        // Unknown dashboard → error, not a silent no-op.
        assert!(add_pane("no-such-dashboard", &add).is_err());
    }
}
