//! The Org Chart's tables (migration 0283) and every rule on them
//! (prd-org-chart-v1 §4). Callers pass the connection and `now`, so tests
//! run on a scoped DB with a fixed clock. Permission checks that depend on
//! who is asking (the manage switch, people's roles) are the daemon's;
//! the rules that depend on the chart itself (head-only bulletins, own-org
//! updates and asks) are here.

use std::collections::{BTreeMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};

use super::{text, OrgError, Result, MAX_ORGS_PER_AGENT};

/// One live org.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Org {
    pub id: String,
    pub name: String,
    pub purpose: String,
    pub head: Option<String>,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// An org's bulletin, as the head last wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bulletin {
    pub org_id: String,
    pub lines: Vec<String>,
    pub updated_by: String,
    pub updated_at: i64,
}

/// What a line end is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeKind {
    Agent,
    Org,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Agent => "agent",
            NodeKind::Org => "org",
        }
    }

    pub fn parse(raw: &str) -> Option<NodeKind> {
        match raw {
            "agent" => Some(NodeKind::Agent),
            "org" => Some(NodeKind::Org),
            _ => None,
        }
    }
}

/// One end of a line, or one card on the canvas.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeRef {
    pub kind: NodeKind,
    pub id: String,
}

impl NodeRef {
    pub fn agent(id: &str) -> NodeRef {
        NodeRef { kind: NodeKind::Agent, id: id.to_string() }
    }
    pub fn org(id: &str) -> NodeRef {
        NodeRef { kind: NodeKind::Org, id: id.to_string() }
    }
}

/// A solid line: `below` (a sits above b) or `beside`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Below,
    Beside,
}

impl LineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LineKind::Below => "below",
            LineKind::Beside => "beside",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub id: String,
    pub kind: LineKind,
    pub a: NodeRef,
    pub b: NodeRef,
    pub created_by: String,
    pub created_at: i64,
}

/// A card's anchor on the canvas (the widget fixes both ends of a drag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    Top,
    Bottom,
    Left,
    Right,
}

impl Anchor {
    pub fn parse(raw: &str) -> Option<Anchor> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "top" => Some(Anchor::Top),
            "bottom" => Some(Anchor::Bottom),
            "left" => Some(Anchor::Left),
            "right" => Some(Anchor::Right),
            _ => None,
        }
    }
}

/// The never-top-to-top rule's sentence.
pub const TOP_TO_TOP_HINT: &str = "Lines go top to bottom (below) or side to side (beside), never top to top.";

/// A drag from `from`'s anchor to `to`'s anchor as a line: `(kind, swap)`,
/// where `swap` means `to` is the upper end. Bottom→top is below; top→bottom
/// is below the other way; side→side is beside; anything else (top→top,
/// bottom→bottom, a side to a top) is refused.
pub fn kind_from_anchors(from: Anchor, to: Anchor) -> Result<(LineKind, bool)> {
    use Anchor::*;
    match (from, to) {
        (Bottom, Top) => Ok((LineKind::Below, false)),
        (Top, Bottom) => Ok((LineKind::Below, true)),
        (Left | Right, Left | Right) => Ok((LineKind::Beside, false)),
        _ => Err(OrgError::new(400, "line_top_to_top", TOP_TO_TOP_HINT)),
    }
}

/// Who is changing something (the daemon resolves it from the credential).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// The owner token, a Connect login or a K2 shell tab. `admin` = Owner
    /// or Admin.
    Person { label: String, admin: bool },
    /// An agent, by its workspace.
    Agent { project_id: String, label: String },
}

impl Actor {
    pub fn label(&self) -> &str {
        match self {
            Actor::Person { label, .. } | Actor::Agent { label, .. } => label,
        }
    }
}

// ── helpers ─────────────────────────────────────────────────────────

/// `org-<10 hex>` and friends.
pub fn new_id(prefix: &str) -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}-{}", &u[..10])
}

/// The name K2 shows for an agent (its handle), or its id when it's gone.
pub fn agent_name(conn: &Connection, project_id: &str) -> String {
    crate::workspace_session_handles::workspace_address_name(conn, project_id).unwrap_or_else(|_| project_id.to_string())
}

/// A workspace on this server (not an audit sentinel).
pub fn agent_exists(conn: &Connection, project_id: &str) -> Result<bool> {
    if crate::db::AUDIT_SENTINEL_IDS.contains(&project_id) {
        return Ok(false);
    }
    Ok(conn
        .query_row("SELECT 1 FROM projects WHERE id = ?1", params![project_id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn require_agent(conn: &Connection, project_id: &str) -> Result<()> {
    if agent_exists(conn, project_id)? {
        Ok(())
    } else {
        Err(OrgError::not_found("agent", project_id, "k2 workspace list shows the agents on this server."))
    }
}

/// The chart's revision (bumped by every change).
pub fn revision(conn: &Connection) -> Result<i64> {
    let v: Option<String> =
        conn.query_row("SELECT value FROM org_chart_meta WHERE key = 'revision'", [], |r| r.get(0)).optional()?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}

/// Bump the revision; returns the new one.
pub fn bump(conn: &Connection) -> Result<i64> {
    conn.execute(
        "INSERT INTO org_chart_meta (key, value) VALUES ('revision', '1') \
         ON CONFLICT(key) DO UPDATE SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)",
        [],
    )?;
    revision(conn)
}

// ── orgs ─────────────────────────────────────────────────────────────

fn org_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Org> {
    Ok(Org {
        id: r.get(0)?,
        name: r.get(1)?,
        purpose: r.get(2)?,
        head: r.get(3)?,
        created_by: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
    })
}

const ORG_COLS: &str = "id, name, purpose, head_project_id, created_by, created_at, updated_at";

/// Every live org, by name.
pub fn orgs(conn: &Connection) -> Result<Vec<Org>> {
    let mut stmt = conn.prepare(&format!("SELECT {ORG_COLS} FROM org_chart_orgs WHERE deleted_at IS NULL ORDER BY name_key, id"))?;
    let rows = stmt.query_map([], org_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// One live org by id.
pub fn get_org(conn: &Connection, id: &str) -> Result<Option<Org>> {
    Ok(conn
        .query_row(&format!("SELECT {ORG_COLS} FROM org_chart_orgs WHERE id = ?1 AND deleted_at IS NULL"), params![id], org_from_row)
        .optional()?)
}

/// A live org by id or name (any case). `None` when there's none.
pub fn lookup_org(conn: &Connection, token: &str) -> Result<Option<Org>> {
    let t = token.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if let Some(o) = get_org(conn, t)? {
        return Ok(Some(o));
    }
    Ok(conn
        .query_row(
            &format!("SELECT {ORG_COLS} FROM org_chart_orgs WHERE name_key = ?1 AND deleted_at IS NULL"),
            params![text::name_key(t)],
            org_from_row,
        )
        .optional()?)
}

/// [`lookup_org`], refusing with `not_found` when there's none.
pub fn find_org(conn: &Connection, token: &str) -> Result<Org> {
    lookup_org(conn, token)?.ok_or_else(|| {
        OrgError::not_found("org", token.trim(), "k2 organization list shows the orgs on this server.")
    })
}

fn name_taken(conn: &Connection, name: &str, except: Option<&str>) -> Result<bool> {
    let hit: Option<String> = conn
        .query_row(
            "SELECT id FROM org_chart_orgs WHERE name_key = ?1 AND deleted_at IS NULL",
            params![text::name_key(name)],
            |r| r.get(0),
        )
        .optional()?;
    Ok(hit.is_some_and(|id| Some(id.as_str()) != except))
}

fn taken_error(name: &str) -> OrgError {
    OrgError::new(409, "org_name_taken", format!("There's already an org called {name}. Pick another name."))
}

/// Make an org. Its bucket appears on the canvas at once (auto layout).
pub fn create_org(conn: &Connection, name: &str, purpose: &str, actor: &str, now: i64) -> Result<Org> {
    let name = text::org_name(name)?;
    let purpose = text::purpose(purpose)?;
    if name_taken(conn, &name, None)? {
        return Err(taken_error(&name));
    }
    let id = new_id("org");
    conn.execute(
        "INSERT INTO org_chart_orgs (id, name, name_key, purpose, head_project_id, created_by, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?6)",
        params![id, name, text::name_key(&name), purpose, actor, now],
    )?;
    bump(conn)?;
    get_org(conn, &id)?.ok_or_else(|| OrgError::new(500, "internal", "the new org vanished"))
}

pub fn rename_org(conn: &Connection, org_id: &str, name: &str, now: i64) -> Result<Org> {
    let name = text::org_name(name)?;
    if name_taken(conn, &name, Some(org_id))? {
        return Err(taken_error(&name));
    }
    conn.execute(
        "UPDATE org_chart_orgs SET name = ?2, name_key = ?3, updated_at = ?4 WHERE id = ?1 AND deleted_at IS NULL",
        params![org_id, name, text::name_key(&name), now],
    )?;
    bump(conn)?;
    get_org(conn, org_id)?.ok_or_else(|| OrgError::not_found("org", org_id, "k2 organization list shows them."))
}

pub fn set_purpose(conn: &Connection, org_id: &str, purpose: &str, now: i64) -> Result<Org> {
    let purpose = text::purpose(purpose)?;
    conn.execute(
        "UPDATE org_chart_orgs SET purpose = ?2, updated_at = ?3 WHERE id = ?1 AND deleted_at IS NULL",
        params![org_id, purpose, now],
    )?;
    bump(conn)?;
    get_org(conn, org_id)?.ok_or_else(|| OrgError::not_found("org", org_id, "k2 organization list shows them."))
}

/// Delete an org: it goes soft, its members leave and its lines and
/// position go. Nothing else changes (agents and connections stay).
pub fn delete_org(conn: &Connection, org_id: &str, now: i64) -> Result<()> {
    let n = conn.execute(
        "UPDATE org_chart_orgs SET deleted_at = ?2, updated_at = ?2, head_project_id = NULL WHERE id = ?1 AND deleted_at IS NULL",
        params![org_id, now],
    )?;
    if n == 0 {
        return Err(OrgError::not_found("org", org_id, "k2 organization list shows them."));
    }
    // Members' spots inside the bucket go with it.
    conn.execute(
        "DELETE FROM org_chart_nodes WHERE node_kind = 'agent' AND node_id IN (SELECT project_id FROM org_chart_members WHERE org_id = ?1)",
        params![org_id],
    )?;
    conn.execute("DELETE FROM org_chart_members WHERE org_id = ?1", params![org_id])?;
    conn.execute(
        "DELETE FROM org_chart_lines WHERE (a_kind = 'org' AND a_id = ?1) OR (b_kind = 'org' AND b_id = ?1)",
        params![org_id],
    )?;
    conn.execute("DELETE FROM org_chart_nodes WHERE node_kind = 'org' AND node_id = ?1", params![org_id])?;
    bump(conn)?;
    Ok(())
}

// ── members ─────────────────────────────────────────────────────────

/// An org's members (project ids), oldest first.
pub fn members(conn: &Connection, org_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT project_id FROM org_chart_members WHERE org_id = ?1 ORDER BY added_at, project_id")?;
    let rows = stmt.query_map(params![org_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(rows)
}

/// The live orgs an agent is in.
pub fn orgs_of(conn: &Connection, project_id: &str) -> Result<Vec<Org>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM org_chart_orgs o JOIN org_chart_members m ON m.org_id = o.id \
         WHERE m.project_id = ?1 AND o.deleted_at IS NULL ORDER BY o.name_key",
        ORG_COLS.split(", ").map(|c| format!("o.{c}")).collect::<Vec<_>>().join(", ")
    ))?;
    let rows = stmt.query_map(params![project_id], org_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn is_member(conn: &Connection, org_id: &str, project_id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM org_chart_members WHERE org_id = ?1 AND project_id = ?2",
            params![org_id, project_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Put an agent in an org (the one-org rule, [`MAX_ORGS_PER_AGENT`]).
/// `Ok(false)` = it was already a member.
pub fn add_member(conn: &Connection, org_id: &str, project_id: &str, actor: &str, now: i64) -> Result<bool> {
    add_member_capped(conn, org_id, project_id, actor, now, MAX_ORGS_PER_AGENT)
}

/// [`add_member`] with the cap given: the tables are many-to-many, the cap
/// is the rule.
pub fn add_member_capped(conn: &Connection, org_id: &str, project_id: &str, actor: &str, now: i64, cap: usize) -> Result<bool> {
    let org = get_org(conn, org_id)?.ok_or_else(|| OrgError::not_found("org", org_id, "k2 organization list shows them."))?;
    require_agent(conn, project_id)?;
    if is_member(conn, org_id, project_id)? {
        return Ok(false);
    }
    let others = orgs_of(conn, project_id)?;
    if others.len() >= cap {
        let name = agent_name(conn, project_id);
        let other = others.first().map(|o| o.name.clone()).unwrap_or_default();
        return Err(OrgError::new(
            409,
            "agent_in_another_org",
            format!(
                "{name} is already in {other}. Move it with `k2 organization move {name} {}`, or remove it first (k2 organization remove {other} {name}).",
                org.name
            ),
        ));
    }
    conn.execute(
        "INSERT INTO org_chart_members (org_id, project_id, added_by, added_at) VALUES (?1, ?2, ?3, ?4)",
        params![org_id, project_id, actor, now],
    )?;
    // A spot it had in another bucket (or loose) means nothing here.
    conn.execute("DELETE FROM org_chart_nodes WHERE node_kind = 'agent' AND node_id = ?1", params![project_id])?;
    bump(conn)?;
    Ok(true)
}

/// Take an agent out of an org. Removing the head leaves the org with no
/// head until one is set. `Ok(false)` = it wasn't a member.
pub fn remove_member(conn: &Connection, org_id: &str, project_id: &str, now: i64) -> Result<bool> {
    let n = conn.execute(
        "DELETE FROM org_chart_members WHERE org_id = ?1 AND project_id = ?2",
        params![org_id, project_id],
    )?;
    if n == 0 {
        return Ok(false);
    }
    conn.execute(
        "UPDATE org_chart_orgs SET head_project_id = NULL, updated_at = ?3 WHERE id = ?1 AND head_project_id = ?2",
        params![org_id, project_id, now],
    )?;
    conn.execute("DELETE FROM org_chart_nodes WHERE node_kind = 'agent' AND node_id = ?1", params![project_id])?;
    bump(conn)?;
    Ok(true)
}

/// Move an agent out of its org(s) and into `org_id`. Returns the orgs it
/// left.
pub fn move_member(conn: &Connection, project_id: &str, org_id: &str, actor: &str, now: i64) -> Result<Vec<Org>> {
    get_org(conn, org_id)?.ok_or_else(|| OrgError::not_found("org", org_id, "k2 organization list shows them."))?;
    require_agent(conn, project_id)?;
    let mut left = Vec::new();
    for o in orgs_of(conn, project_id)? {
        if o.id == org_id {
            continue;
        }
        remove_member(conn, &o.id, project_id, now)?;
        left.push(o);
    }
    add_member(conn, org_id, project_id, actor, now)?;
    Ok(left)
}

/// Set (or clear, `None`) an org's head. The head must be a member.
pub fn set_head(conn: &Connection, org_id: &str, project_id: Option<&str>, now: i64) -> Result<()> {
    let org = get_org(conn, org_id)?.ok_or_else(|| OrgError::not_found("org", org_id, "k2 organization list shows them."))?;
    if let Some(p) = project_id {
        if !is_member(conn, org_id, p)? {
            let name = agent_name(conn, p);
            return Err(OrgError::new(
                400,
                "head_not_member",
                format!("{name} isn't in {}. The head must be a member: add it first (k2 organization add {} {name}).", org.name, org.name),
            ));
        }
    }
    conn.execute(
        "UPDATE org_chart_orgs SET head_project_id = ?2, updated_at = ?3 WHERE id = ?1",
        params![org_id, project_id, now],
    )?;
    bump(conn)?;
    Ok(())
}

/// The org this agent heads (live), if any.
pub fn headed_by(conn: &Connection, project_id: &str) -> Result<Vec<Org>> {
    let mut stmt =
        conn.prepare(&format!("SELECT {ORG_COLS} FROM org_chart_orgs WHERE head_project_id = ?1 AND deleted_at IS NULL ORDER BY name_key"))?;
    let rows = stmt.query_map(params![project_id], org_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ── lines ───────────────────────────────────────────────────────────

fn line_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Line> {
    let kind: String = r.get(1)?;
    let ak: String = r.get(2)?;
    let bk: String = r.get(4)?;
    Ok(Line {
        id: r.get(0)?,
        kind: if kind == "beside" { LineKind::Beside } else { LineKind::Below },
        a: NodeRef { kind: NodeKind::parse(&ak).unwrap_or(NodeKind::Agent), id: r.get(3)? },
        b: NodeRef { kind: NodeKind::parse(&bk).unwrap_or(NodeKind::Agent), id: r.get(5)? },
        created_by: r.get(6)?,
        created_at: r.get(7)?,
    })
}

/// Every line.
pub fn lines(conn: &Connection) -> Result<Vec<Line>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, a_kind, a_id, b_kind, b_id, created_by, created_at FROM org_chart_lines ORDER BY created_at, id",
    )?;
    let rows = stmt.query_map([], line_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn node_exists(conn: &Connection, n: &NodeRef) -> Result<bool> {
    match n.kind {
        NodeKind::Agent => agent_exists(conn, &n.id),
        NodeKind::Org => Ok(get_org(conn, &n.id)?.is_some()),
    }
}

/// The name a line end shows (an org's name, an agent's handle).
pub fn node_name(conn: &Connection, n: &NodeRef) -> String {
    match n.kind {
        NodeKind::Agent => agent_name(conn, &n.id),
        NodeKind::Org => get_org(conn, &n.id).ok().flatten().map(|o| o.name).unwrap_or_else(|| n.id.clone()),
    }
}

/// True when `from` already sits above `to` through `below` lines.
fn reaches(all: &[Line], from: &NodeRef, to: &NodeRef) -> bool {
    let mut seen: HashSet<NodeRef> = HashSet::new();
    let mut stack = vec![from.clone()];
    while let Some(n) = stack.pop() {
        if &n == to {
            return true;
        }
        if !seen.insert(n.clone()) {
            continue;
        }
        for l in all.iter().filter(|l| l.kind == LineKind::Below && l.a == n) {
            stack.push(l.b.clone());
        }
    }
    false
}

/// Draw a line. `below`: `a` sits above `b`. No line to itself, no
/// duplicate (a `beside` pair is stored once), and `below` lines never
/// loop.
pub fn add_line(conn: &Connection, kind: LineKind, a: NodeRef, b: NodeRef, actor: &str, now: i64) -> Result<Line> {
    if a == b {
        return Err(OrgError::new(400, "line_self", "A line joins two different cards; this one starts and ends on the same card."));
    }
    for n in [&a, &b] {
        if !node_exists(conn, n)? {
            return Err(OrgError::not_found(n.kind.as_str(), &n.id, "k2 organization list shows the orgs and their agents."));
        }
    }
    let (a, b) = if kind == LineKind::Beside && b < a { (b, a) } else { (a, b) };
    let all = lines(conn)?;
    let same_pair = |l: &&Line| (l.a == a && l.b == b) || (l.a == b && l.b == a);
    if let Some(existing) = all.iter().find(same_pair) {
        if existing.kind == kind && existing.a == a {
            return Err(OrgError::new(
                409,
                "line_exists",
                format!("{} and {} already have that line.", node_name(conn, &a), node_name(conn, &b)),
            ));
        }
        if kind == LineKind::Below && existing.kind == LineKind::Below {
            // The pair the other way round is the shortest loop.
            return Err(loop_error(conn, &b, &a));
        }
        return Err(OrgError::new(
            409,
            "line_exists",
            format!(
                "{} and {} already have a {} line. Unlink them first (k2 organization unlink {} {}).",
                node_name(conn, &a),
                node_name(conn, &b),
                existing.kind.as_str(),
                node_name(conn, &a),
                node_name(conn, &b)
            ),
        ));
    }
    if kind == LineKind::Below && reaches(&all, &b, &a) {
        return Err(loop_error(conn, &b, &a));
    }
    let id = new_id("line");
    conn.execute(
        "INSERT INTO org_chart_lines (id, kind, a_kind, a_id, b_kind, b_id, created_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, kind.as_str(), a.kind.as_str(), a.id, b.kind.as_str(), b.id, actor, now],
    )?;
    bump(conn)?;
    Ok(Line { id, kind, a, b, created_by: actor.to_string(), created_at: now })
}

fn loop_error(conn: &Connection, upper: &NodeRef, lower: &NodeRef) -> OrgError {
    let (u, l) = (node_name(conn, upper), node_name(conn, lower));
    OrgError::new(409, "line_loop", format!("{u} is already above {l}, so {l} can't be above {u}."))
}

/// Remove the line between two cards (whatever its kind and direction).
/// `Ok(0)` = there was none.
pub fn remove_line(conn: &Connection, a: &NodeRef, b: &NodeRef) -> Result<usize> {
    let n = conn.execute(
        "DELETE FROM org_chart_lines WHERE (a_kind = ?1 AND a_id = ?2 AND b_kind = ?3 AND b_id = ?4) \
            OR (a_kind = ?3 AND a_id = ?4 AND b_kind = ?1 AND b_id = ?2)",
        params![a.kind.as_str(), a.id, b.kind.as_str(), b.id],
    )?;
    if n > 0 {
        bump(conn)?;
    }
    Ok(n)
}

/// Remove a line by its id.
pub fn remove_line_id(conn: &Connection, id: &str) -> Result<usize> {
    let n = conn.execute("DELETE FROM org_chart_lines WHERE id = ?1", params![id])?;
    if n > 0 {
        bump(conn)?;
    }
    Ok(n)
}

// ── bulletins ───────────────────────────────────────────────────────

pub fn bulletin(conn: &Connection, org_id: &str) -> Result<Option<Bulletin>> {
    Ok(conn
        .query_row(
            "SELECT org_id, body, updated_by, updated_at FROM org_chart_bulletins WHERE org_id = ?1",
            params![org_id],
            |r| {
                let body: String = r.get(1)?;
                Ok(Bulletin {
                    org_id: r.get(0)?,
                    lines: body.lines().map(str::to_string).filter(|l| !l.is_empty()).collect(),
                    updated_by: r.get(2)?,
                    updated_at: r.get(3)?,
                })
            },
        )
        .optional()?)
}

/// May `actor` write this org's bulletin? Its head (an agent, no switch
/// needed), or an Owner/Admin person (Rosson, answer 8). Anyone else gets
/// `not_org_head`.
pub fn check_bulletin_writer(conn: &Connection, org: &Org, actor: &Actor) -> Result<()> {
    match actor {
        Actor::Person { admin: true, .. } => Ok(()),
        Actor::Agent { project_id, .. } if org.head.as_deref() == Some(project_id.as_str()) => Ok(()),
        _ => Err(match &org.head {
            Some(h) => OrgError::new(
                403,
                "not_org_head",
                format!("Only {}'s head ({}) writes its bulletin.", org.name, agent_name(conn, h)),
            ),
            None => OrgError::new(
                403,
                "not_org_head",
                format!(
                    "{} has no head yet, so only an Owner or Admin writes its bulletin. They set a head with k2 organization head {} <agent>.",
                    org.name, org.name
                ),
            ),
        }),
    }
}

/// Write an org's bulletin (live at once). Checks the writer, then the
/// 5-line / 160-character rules.
pub fn set_bulletin(conn: &Connection, org_id: &str, actor: &Actor, parts: &[String], now: i64) -> Result<Bulletin> {
    let org = get_org(conn, org_id)?.ok_or_else(|| OrgError::not_found("org", org_id, "k2 organization list shows them."))?;
    check_bulletin_writer(conn, &org, actor)?;
    let lines = text::bulletin(parts)?;
    conn.execute(
        "INSERT INTO org_chart_bulletins (org_id, body, updated_by, updated_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(org_id) DO UPDATE SET body = excluded.body, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
        params![org_id, lines.join("\n"), actor.label(), now],
    )?;
    bump(conn)?;
    Ok(Bulletin { org_id: org_id.to_string(), lines, updated_by: actor.label().to_string(), updated_at: now })
}

// ── read state ──────────────────────────────────────────────────────

/// Mark the server bulletin read for this agent (or sidecar) on `day`.
pub fn mark_read(conn: &Connection, project_id: &str, agent_address: &str, day: &str, now: i64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO org_chart_reads (project_id, agent_address, day, read_at) VALUES (?1, ?2, ?3, ?4)",
        params![project_id, agent_address, day, now],
    )?;
    Ok(())
}

/// Has this agent (or sidecar) read it on `day`?
pub fn has_read(conn: &Connection, project_id: &str, agent_address: &str, day: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM org_chart_reads WHERE project_id = ?1 AND agent_address = ?2 AND day = ?3",
            params![project_id, agent_address, day],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

// ── updates and asks ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub id: String,
    pub org_id: String,
    pub from: String,
    pub body: String,
    pub night: Option<String>,
    pub woke: bool,
    pub created_at: i64,
}

/// Store a member's update to its head.
pub fn add_update(conn: &Connection, org_id: &str, from: &str, body: &str, night: Option<&str>, now: i64) -> Result<Update> {
    let id = new_id("upd");
    conn.execute(
        "INSERT INTO org_chart_updates (id, org_id, from_project_id, body, night, woke, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
        params![id, org_id, from, body, night, now],
    )?;
    Ok(Update {
        id,
        org_id: org_id.to_string(),
        from: from.to_string(),
        body: body.to_string(),
        night: night.map(str::to_string),
        woke: false,
        created_at: now,
    })
}

pub fn set_update_woke(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("UPDATE org_chart_updates SET woke = 1 WHERE id = ?1", params![id])?;
    Ok(())
}

/// An org's updates at or after `since`, newest first (at most `limit`).
pub fn updates(conn: &Connection, org_id: &str, since: i64, limit: usize) -> Result<Vec<Update>> {
    let mut stmt = conn.prepare(
        "SELECT id, org_id, from_project_id, body, night, woke, created_at FROM org_chart_updates \
         WHERE org_id = ?1 AND created_at >= ?2 ORDER BY created_at DESC, id LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![org_id, since, limit as i64], |r| {
            Ok(Update {
                id: r.get(0)?,
                org_id: r.get(1)?,
                from: r.get(2)?,
                body: r.get(3)?,
                night: r.get(4)?,
                woke: r.get::<_, i64>(5)? == 1,
                created_at: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Updates an org got since `since` (the night run's count).
pub fn count_updates_since(conn: &Connection, org_id: &str, since: i64) -> Result<usize> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM org_chart_updates WHERE org_id = ?1 AND created_at >= ?2",
        params![org_id, since],
        |r| r.get(0),
    )?;
    Ok(n as usize)
}

/// Asks (one per question, however many members it went to) a head sent
/// since `since`.
pub fn asks_since(conn: &Connection, from: &str, since: i64) -> Result<usize> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT created_at || ':' || body) FROM org_chart_asks WHERE from_project_id = ?1 AND created_at >= ?2",
        params![from, since],
        |r| r.get(0),
    )?;
    Ok(n as usize)
}

pub fn add_ask(conn: &Connection, org_id: &str, from: &str, to: &str, body: &str, now: i64) -> Result<String> {
    let id = new_id("ask");
    conn.execute(
        "INSERT INTO org_chart_asks (id, org_id, from_project_id, to_project_id, body, woke, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
        params![id, org_id, from, to, body, now],
    )?;
    Ok(id)
}

pub fn set_ask_woke(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("UPDATE org_chart_asks SET woke = 1 WHERE id = ?1", params![id])?;
    Ok(())
}

// ── the night ledger and wakes ──────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NightRow {
    pub run_at: Option<i64>,
    pub run_result: Option<String>,
    pub last_call_at: Option<i64>,
    pub last_call_result: Option<String>,
}

pub fn night_row(conn: &Connection, org_id: &str, night: &str) -> Result<Option<NightRow>> {
    Ok(conn
        .query_row(
            "SELECT run_at, run_result, last_call_at, last_call_result FROM org_chart_nights WHERE org_id = ?1 AND night = ?2",
            params![org_id, night],
            |r| Ok(NightRow { run_at: r.get(0)?, run_result: r.get(1)?, last_call_at: r.get(2)?, last_call_result: r.get(3)? }),
        )
        .optional()?)
}

/// Record the night run: `at` is set once it was delivered (or failed for
/// good); `None` with a result (`agent_cap`) means try again next tick.
pub fn record_run(conn: &Connection, org_id: &str, night: &str, at: Option<i64>, result: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO org_chart_nights (org_id, night, run_at, run_result) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(org_id, night) DO UPDATE SET run_at = excluded.run_at, run_result = excluded.run_result",
        params![org_id, night, at, result],
    )?;
    Ok(())
}

pub fn record_last_call(conn: &Connection, org_id: &str, night: &str, at: Option<i64>, result: &str) -> Result<()> {
    conn.execute(
        "UPDATE org_chart_nights SET last_call_at = ?3, last_call_result = ?4 WHERE org_id = ?1 AND night = ?2",
        params![org_id, night, at, result],
    )?;
    Ok(())
}

/// The kinds of org wake (`org_chart_wakes.kind`).
pub const WAKE_NIGHT_RUN: &str = "night_run";
pub const WAKE_LAST_CALL: &str = "last_call";
pub const WAKE_UPDATE: &str = "update";
pub const WAKE_ASK: &str = "ask";
pub const WAKE_REACH: &str = "reach";

/// One org wake that reached `project_id` (counted by the wake budget).
pub fn record_wake(conn: &Connection, project_id: &str, kind: &str, at: i64) -> Result<()> {
    conn.execute("INSERT INTO org_chart_wakes (project_id, kind, at) VALUES (?1, ?2, ?3)", params![project_id, kind, at])?;
    Ok(())
}

/// Org wakes of this agent since `since` (for `wake_budget::wakes_since`).
pub fn wakes_since(conn: &Connection, project_id: &str, since: i64) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM org_chart_wakes WHERE project_id = ?1 AND at >= ?2",
        params![project_id, since],
        |r| r.get(0),
    )
}

/// Drop wake rows older than `before`.
pub fn prune_wakes(conn: &Connection, before: i64) -> Result<usize> {
    Ok(conn.execute("DELETE FROM org_chart_wakes WHERE at < ?1", params![before])?)
}

// ── reach ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reach {
    pub id: String,
    pub from: String,
    pub to: String,
    pub org_id: String,
    pub why: String,
    pub via: String,
    pub permission_request_id: Option<String>,
    pub feedback_id: Option<String>,
    pub state: String,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
    pub notified_at: Option<i64>,
}

const REACH_COLS: &str =
    "id, from_project_id, to_project_id, org_id, why, via, permission_request_id, feedback_id, state, created_at, resolved_at, notified_at";

fn reach_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Reach> {
    Ok(Reach {
        id: r.get(0)?,
        from: r.get(1)?,
        to: r.get(2)?,
        org_id: r.get(3)?,
        why: r.get(4)?,
        via: r.get(5)?,
        permission_request_id: r.get(6)?,
        feedback_id: r.get(7)?,
        state: r.get(8)?,
        created_at: r.get(9)?,
        resolved_at: r.get(10)?,
        notified_at: r.get(11)?,
    })
}

/// The open ask for this pair, if any (one per pair).
pub fn open_reach(conn: &Connection, from: &str, to: &str) -> Result<Option<Reach>> {
    Ok(conn
        .query_row(
            &format!("SELECT {REACH_COLS} FROM org_chart_reach WHERE from_project_id = ?1 AND to_project_id = ?2 AND state = 'open'"),
            params![from, to],
            reach_from_row,
        )
        .optional()?)
}

pub fn get_reach(conn: &Connection, id: &str) -> Result<Option<Reach>> {
    Ok(conn
        .query_row(&format!("SELECT {REACH_COLS} FROM org_chart_reach WHERE id = ?1"), params![id], reach_from_row)
        .optional()?)
}

/// Record a new ask (`via` = `card` or `ticket`).
#[allow(clippy::too_many_arguments)]
pub fn insert_reach(
    conn: &Connection,
    from: &str,
    to: &str,
    org_id: &str,
    why: &str,
    via: &str,
    permission_request_id: Option<&str>,
    feedback_id: Option<&str>,
    now: i64,
) -> Result<Reach> {
    let id = new_id("reach");
    conn.execute(
        "INSERT INTO org_chart_reach (id, from_project_id, to_project_id, org_id, why, via, permission_request_id, feedback_id, state, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'open', ?9)",
        params![id, from, to, org_id, why, via, permission_request_id, feedback_id, now],
    )?;
    get_reach(conn, &id)?.ok_or_else(|| OrgError::new(500, "internal", "the new ask vanished"))
}

/// Asks waiting to be told: `connected` or `declined`, not yet told.
pub fn reach_to_tell(conn: &Connection) -> Result<Vec<Reach>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {REACH_COLS} FROM org_chart_reach WHERE state IN ('connected', 'declined') AND notified_at IS NULL ORDER BY created_at"
    ))?;
    let rows = stmt.query_map([], reach_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Every open ask.
pub fn open_reaches(conn: &Connection) -> Result<Vec<Reach>> {
    let mut stmt = conn.prepare(&format!("SELECT {REACH_COLS} FROM org_chart_reach WHERE state = 'open' ORDER BY created_at"))?;
    let rows = stmt.query_map([], reach_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn set_reach_state(conn: &Connection, id: &str, state: &str, now: i64) -> Result<()> {
    conn.execute(
        "UPDATE org_chart_reach SET state = ?2, resolved_at = ?3 WHERE id = ?1 AND state = 'open'",
        params![id, state, now],
    )?;
    Ok(())
}

pub fn mark_reach_told(conn: &Connection, id: &str, now: i64) -> Result<()> {
    conn.execute("UPDATE org_chart_reach SET notified_at = ?2 WHERE id = ?1", params![id, now])?;
    Ok(())
}

// ── positions ───────────────────────────────────────────────────────

/// A position a person placed.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub node: NodeRef,
    pub x: f64,
    pub y: f64,
    pub w: Option<f64>,
    pub h: Option<f64>,
}

pub fn placed(conn: &Connection) -> Result<BTreeMap<NodeRef, Placed>> {
    let mut stmt = conn.prepare("SELECT node_kind, node_id, x, y, w, h FROM org_chart_nodes")?;
    let rows = stmt
        .query_map([], |r| {
            let k: String = r.get(0)?;
            Ok(Placed {
                node: NodeRef { kind: NodeKind::parse(&k).unwrap_or(NodeKind::Agent), id: r.get(1)? },
                x: r.get(2)?,
                y: r.get(3)?,
                w: r.get(4)?,
                h: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().map(|p| (p.node.clone(), p)).collect())
}

/// A person moved (or resized) a card or bucket. A member's `x, y` are
/// relative to its bucket.
pub fn place(conn: &Connection, p: &Placed, now: i64) -> Result<i64> {
    conn.execute(
        "INSERT INTO org_chart_nodes (node_kind, node_id, x, y, w, h, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(node_kind, node_id) DO UPDATE SET x = excluded.x, y = excluded.y, \
           w = COALESCE(excluded.w, org_chart_nodes.w), h = COALESCE(excluded.h, org_chart_nodes.h), updated_at = excluded.updated_at",
        params![p.node.kind.as_str(), p.node.id, p.x, p.y, p.w, p.h, now],
    )?;
    bump(conn)
}

/// "Tidy up": every card and bucket back on auto. Returns how many were
/// placed by a person.
pub fn tidy(conn: &Connection) -> Result<usize> {
    let n = conn.execute("DELETE FROM org_chart_nodes", [])?;
    bump(conn)?;
    Ok(n)
}

// ── the per-agent switch ───────────────────────────────────────────

/// "Manage the org chart" for this agent (off by default).
pub fn agents_can_manage(conn: &Connection, project_id: &str) -> bool {
    conn.query_row(
        "SELECT agents_can_manage_org_chart FROM projects WHERE id = ?1",
        params![project_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|v| v == 1)
    .unwrap_or(false)
}

pub fn set_agents_can_manage(conn: &Connection, project_id: &str, on: bool) -> std::result::Result<(), String> {
    let n = conn
        .execute(
            "UPDATE projects SET agents_can_manage_org_chart = ?2 WHERE id = ?1",
            params![project_id, i64::from(on)],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("workspace not found: {project_id}"));
    }
    Ok(())
}
