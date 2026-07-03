//! Feedback F1 (prd-agent-feedback-notifications §4.1) — the durable
//! agent→human ask primitive.
//!
//! An agent that needs a person (a question, an approval, a heads-up)
//! files a `feedback` row instead of dying in an unwatched terminal
//! prompt. The item sits on the server's Feedback page until a human
//! answers or resolves it; a per-item comment thread
//! (`feedback_comments`) carries the discussion. The daemon's
//! `/cli/feedback/*` routes and the `k2 feedback` CLI verb are thin
//! wrappers over this module (daemon-first: all logic lives here).
//!
//! Status pipeline (v1, deliberately smaller than the AFSROW reference
//! board): `waiting → answered → resolved`, plus `dismissed`. `answer`
//! is denormalized onto the item so `k2 feedback ask --wait` reads one
//! row; the accepted answer ALSO lands in the thread as a comment.
//!
//! Addressing: items are UUIDs, resolvable by a SHORT UNIQUE PREFIX
//! (see [`resolve_id_prefix`]) — ambiguity is an error carrying the
//! matching candidates so the CLI can print a did-you-mean hint.

use rusqlite::params;

/// The valid `feedback.kind` values.
pub const KINDS: [&str; 3] = ["question", "approval", "fyi"];

/// The valid `feedback.status` values.
pub const STATUSES: [&str; 4] = ["waiting", "answered", "resolved", "dismissed"];

/// One `feedback` row + its thread size. Serializes camelCase — the
/// wire shape the routes return (matches the CLI mockup's `--json`
/// contract: `id`, `title`, `kind`, `priority`, `status`, `answer`,
/// `sessionId`, …).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackItem {
    pub id: String,
    pub project_id: String,
    pub session_id: Option<String>,
    pub session_kind: Option<String>,
    pub agent_name: String,
    pub kind: String,
    pub title: String,
    pub body: Option<String>,
    /// The structured choices, parsed back out of `options_json`
    /// (`None` when the ask carried no options).
    pub options: Option<Vec<String>>,
    pub priority: i64,
    pub status: String,
    pub answer: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub answered_at: Option<i64>,
    /// Thread size (message-count badge on the board card).
    pub comment_count: i64,
}

/// One `feedback_comments` row. camelCase for the same wire reason;
/// `created_at` also surfaces as the mockup's `at` alias route-side.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackComment {
    pub id: String,
    pub feedback_id: String,
    pub author: String,
    pub body: String,
    pub created_at: i64,
}

/// Input for [`create`]. `project_id` must be an existing
/// `projects.id`; everything session-ish is optional — an ask filed
/// outside any known session MUST still succeed (PRD §4.2).
#[derive(Debug, Clone, Default)]
pub struct NewFeedback {
    pub project_id: String,
    pub session_id: Option<String>,
    pub session_kind: Option<String>,
    pub agent_name: String,
    /// `question` | `approval` | `fyi`; empty → `question`.
    pub kind: String,
    pub title: String,
    pub body: Option<String>,
    pub options: Option<Vec<String>>,
    /// 1 (urgent) … 5 (whenever); 0 → default 3.
    pub priority: i64,
}

/// Prefix-resolution failure: either nothing matched, or the prefix is
/// ambiguous — the CLI turns both into exit 4, the ambiguous arm
/// listing `candidates` in the hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixError {
    NotFound,
    Ambiguous(Vec<String>),
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// File a new feedback item. Validates kind/priority/title, inserts the
/// row, and seeds the discussion thread with the ask itself (author =
/// the agent; body = the ask's body when present, else the title) so
/// the board card opens on a non-empty thread — matching the mockup's
/// `show` transcript.
///
/// Returns the full item (status `waiting`, `comment_count` 1).
pub fn create(new: NewFeedback) -> Result<FeedbackItem, String> {
    let title = new.title.trim().to_string();
    if title.is_empty() {
        return Err("title must not be empty".to_string());
    }
    let kind = if new.kind.is_empty() { "question".to_string() } else { new.kind.clone() };
    if !KINDS.contains(&kind.as_str()) {
        return Err(format!(
            "invalid kind '{kind}' — valid: question, approval, fyi"
        ));
    }
    let priority = if new.priority == 0 { 3 } else { new.priority };
    if !(1..=5).contains(&priority) {
        return Err("priority must be 1-5".to_string());
    }
    if new.project_id.trim().is_empty() {
        return Err("project_id must not be empty".to_string());
    }
    if new.agent_name.trim().is_empty() {
        return Err("agent_name must not be empty".to_string());
    }
    let body = new.body.as_deref().map(str::trim).filter(|b| !b.is_empty()).map(String::from);
    let options_json = match &new.options {
        Some(opts) if !opts.is_empty() => Some(
            serde_json::to_string(opts).map_err(|e| format!("options serialize: {e}"))?,
        ),
        _ => None,
    };

    let id = uuid::Uuid::new_v4().to_string();
    let now = now_secs();
    // The seed comment: the ask itself, in the agent's voice.
    let seed = match &body {
        Some(b) => format!("{title}\n{b}"),
        None => title.clone(),
    };
    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO feedback (id, project_id, session_id, session_kind, agent_name, \
         kind, title, body, options_json, priority, status, answer, \
         created_at, updated_at, answered_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'waiting', NULL, ?11, ?11, NULL)",
        params![
            id,
            new.project_id,
            new.session_id,
            new.session_kind,
            new.agent_name.trim(),
            kind,
            title,
            body,
            options_json,
            priority,
            now,
        ],
    )
    .map_err(|e| format!("feedback insert failed: {e}"))?;
    conn.execute(
        "INSERT INTO feedback_comments (id, feedback_id, author, body, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![uuid::Uuid::new_v4().to_string(), id, new.agent_name.trim(), seed, now],
    )
    .map_err(|e| format!("feedback seed comment insert failed: {e}"))?;
    drop(conn);
    get_item(&id).ok_or_else(|| "feedback row vanished after insert".to_string())
}

/// Which statuses a list read returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListFilter {
    /// The default: open items (`waiting` + `answered`).
    Open,
    /// Everything, including `resolved` and `dismissed`.
    All,
    /// Exactly one status (pre-validated against [`STATUSES`]).
    Status(String),
}

const ITEM_SELECT: &str = "SELECT f.id, f.project_id, f.session_id, f.session_kind, \
    f.agent_name, f.kind, f.title, f.body, f.options_json, f.priority, f.status, \
    f.answer, f.created_at, f.updated_at, f.answered_at, \
    (SELECT COUNT(*) FROM feedback_comments c WHERE c.feedback_id = f.id) \
    FROM feedback f";

fn row_to_item(row: &rusqlite::Row) -> rusqlite::Result<FeedbackItem> {
    let options_json: Option<String> = row.get(8)?;
    Ok(FeedbackItem {
        id: row.get(0)?,
        project_id: row.get(1)?,
        session_id: row.get(2)?,
        session_kind: row.get(3)?,
        agent_name: row.get(4)?,
        kind: row.get(5)?,
        title: row.get(6)?,
        body: row.get(7)?,
        options: options_json.and_then(|j| serde_json::from_str::<Vec<String>>(&j).ok()),
        priority: row.get(9)?,
        status: row.get(10)?,
        answer: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        answered_at: row.get(14)?,
        comment_count: row.get(15)?,
    })
}

/// List a workspace's feedback items, newest first.
pub fn list_for_project(project_id: &str, filter: &ListFilter) -> Result<Vec<FeedbackItem>, String> {
    if let ListFilter::Status(s) = filter {
        if !STATUSES.contains(&s.as_str()) {
            return Err(format!(
                "invalid status '{s}' — valid: waiting, answered, resolved, dismissed"
            ));
        }
    }
    let db = crate::db::shared();
    let conn = db.lock();
    let (where_clause, status_param): (&str, Option<&str>) = match filter {
        ListFilter::Open => (
            " WHERE f.project_id = ?1 AND f.status IN ('waiting','answered')",
            None,
        ),
        ListFilter::All => (" WHERE f.project_id = ?1", None),
        ListFilter::Status(s) => (
            " WHERE f.project_id = ?1 AND f.status = ?2",
            Some(s.as_str()),
        ),
    };
    // rowid tiebreak: same-second filings list in reverse-insertion
    // order (uuid ordering is random).
    let sql = format!("{ITEM_SELECT}{where_clause} ORDER BY f.created_at DESC, f.rowid DESC");
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("prepare: {e}"))?;
    let rows = match status_param {
        Some(s) => stmt.query_map(params![project_id, s], row_to_item),
        None => stmt.query_map(params![project_id], row_to_item),
    }
    .map_err(|e| format!("query: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("row: {e}"))
}

/// Fetch one item by FULL id. `None` when it doesn't exist.
pub fn get_item(id: &str) -> Option<FeedbackItem> {
    let db = crate::db::shared();
    let conn = db.lock();
    let sql = format!("{ITEM_SELECT} WHERE f.id = ?1");
    conn.query_row(&sql, params![id], row_to_item).ok()
}

/// Fetch one item + its full thread (chronological) by FULL id.
pub fn get_with_comments(id: &str) -> Option<(FeedbackItem, Vec<FeedbackComment>)> {
    let item = get_item(id)?;
    let db = crate::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            // rowid tiebreak: comments created within the same second
            // must render in INSERTION order (uuid ordering is random).
            "SELECT id, feedback_id, author, body, created_at FROM feedback_comments \
             WHERE feedback_id = ?1 ORDER BY created_at ASC, rowid ASC",
        )
        .ok()?;
    let comments = stmt
        .query_map(params![id], |row| {
            Ok(FeedbackComment {
                id: row.get(0)?,
                feedback_id: row.get(1)?,
                author: row.get(2)?,
                body: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .ok()?
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    Some((item, comments))
}

/// Resolve a (possibly short) id prefix to the FULL feedback id.
///
/// - Exactly one match → `Ok(full_id)`.
/// - Zero matches → `Err(PrefixError::NotFound)`.
/// - Two or more → `Err(PrefixError::Ambiguous(candidate_ids))` (up to
///   10, for the CLI's did-you-mean hint).
///
/// Resolution is GLOBAL (not workspace-scoped): ids are UUIDs, so
/// cross-workspace prefix collisions are as unlikely as same-workspace
/// ones, and `show <id>` must work from anywhere.
pub fn resolve_id_prefix(prefix: &str) -> Result<String, PrefixError> {
    let p = prefix.trim();
    if p.is_empty() {
        return Err(PrefixError::NotFound);
    }
    let db = crate::db::shared();
    let conn = db.lock();
    // Escape LIKE wildcards so a hostile prefix can't widen the match.
    let escaped = p.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let mut stmt = match conn.prepare(
        "SELECT id FROM feedback WHERE id LIKE ?1 ESCAPE '\\' ORDER BY created_at DESC LIMIT 11",
    ) {
        Ok(s) => s,
        Err(_) => return Err(PrefixError::NotFound),
    };
    let ids: Vec<String> = match stmt
        .query_map(params![format!("{escaped}%")], |row| row.get::<_, String>(0))
    {
        Ok(rows) => rows.filter_map(Result::ok).collect(),
        Err(_) => return Err(PrefixError::NotFound),
    };
    match ids.len() {
        0 => Err(PrefixError::NotFound),
        1 => Ok(ids.into_iter().next().expect("len checked")),
        _ => Err(PrefixError::Ambiguous(ids.into_iter().take(10).collect())),
    }
}

/// Append a comment to an item's thread and bump the item's
/// `updated_at`. `id` must be a FULL id (callers resolve prefixes
/// first). Comments do NOT change status — a comment on a `waiting`
/// item leaves it waiting (the answer flow is [`set_answer`]).
pub fn add_comment(id: &str, author: &str, body: &str) -> Result<FeedbackComment, String> {
    let author = author.trim();
    let body_t = body.trim();
    if author.is_empty() {
        return Err("author must not be empty".to_string());
    }
    if body_t.is_empty() {
        return Err("comment body must not be empty".to_string());
    }
    let now = now_secs();
    let comment_id = uuid::Uuid::new_v4().to_string();
    let db = crate::db::shared();
    let conn = db.lock();
    // FK enforcement would catch a missing parent, but check explicitly
    // so callers get a clean not-found instead of a constraint error.
    let updated = conn
        .execute(
            "UPDATE feedback SET updated_at = ?2 WHERE id = ?1",
            params![id, now],
        )
        .map_err(|e| format!("feedback touch failed: {e}"))?;
    if updated == 0 {
        return Err(format!("no feedback item with id {id}"));
    }
    conn.execute(
        "INSERT INTO feedback_comments (id, feedback_id, author, body, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![comment_id, id, author, body_t, now],
    )
    .map_err(|e| format!("comment insert failed: {e}"))?;
    Ok(FeedbackComment {
        id: comment_id,
        feedback_id: id.to_string(),
        author: author.to_string(),
        body: body_t.to_string(),
        created_at: now,
    })
}

/// Accept an answer: append it to the thread as a comment (in the
/// answerer's voice), denormalize it onto the item (`answer` +
/// `answered_at`), and move status to `answered`. `id` must be a FULL
/// id. Answering again overwrites the accepted answer (last answer
/// wins) — the thread keeps every attempt.
///
/// F1 stores ONLY — delivering the answer into the asking session
/// (`deliver_live`) is F3's layer, not this one.
pub fn set_answer(id: &str, author: &str, answer: &str) -> Result<FeedbackItem, String> {
    let answer_t = answer.trim();
    if answer_t.is_empty() {
        return Err("answer must not be empty".to_string());
    }
    add_comment(id, author, answer_t)?;
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    let updated = conn
        .execute(
            "UPDATE feedback SET answer = ?2, answered_at = ?3, status = 'answered', \
             updated_at = ?3 WHERE id = ?1",
            params![id, answer_t, now],
        )
        .map_err(|e| format!("answer update failed: {e}"))?;
    if updated == 0 {
        return Err(format!("no feedback item with id {id}"));
    }
    drop(conn);
    get_item(id).ok_or_else(|| "feedback row vanished after answer".to_string())
}

/// Set an item's status (validated against [`STATUSES`]). Used by
/// `resolve` (→ `resolved`) and `dismiss` (→ `dismissed`); `answered`
/// should go through [`set_answer`] so the answer is recorded, but is
/// not forbidden here (the renderer may re-open → `waiting` later).
pub fn set_status(id: &str, status: &str) -> Result<FeedbackItem, String> {
    if !STATUSES.contains(&status) {
        return Err(format!(
            "invalid status '{status}' — valid: waiting, answered, resolved, dismissed"
        ));
    }
    let now = now_secs();
    let db = crate::db::shared();
    let conn = db.lock();
    let updated = conn
        .execute(
            "UPDATE feedback SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, status, now],
        )
        .map_err(|e| format!("status update failed: {e}"))?;
    if updated == 0 {
        return Err(format!("no feedback item with id {id}"));
    }
    drop(conn);
    get_item(id).ok_or_else(|| "feedback row vanished after status update".to_string())
}

// ──────────────────────────────────────────────────────────────────────
// Inline unit tests
// ──────────────────────────────────────────────────────────────────────
//
// `db::shared()` in tests is the PROCESS-GLOBAL in-memory DB shared by
// every test in the binary — each test uses its own unique project_id
// so rows never collide (same discipline as workspace_routes' tests).

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(project_id: &str, title: &str) -> FeedbackItem {
        create(NewFeedback {
            project_id: project_id.to_string(),
            agent_name: "scout".to_string(),
            title: title.to_string(),
            ..Default::default()
        })
        .expect("create feedback")
    }

    fn pid(label: &str) -> String {
        format!("fb-core-{label}-{}", uuid::Uuid::new_v4())
    }

    #[test]
    fn create_defaults_and_seed_comment() {
        let p = pid("create");
        let item = create(NewFeedback {
            project_id: p.clone(),
            agent_name: "scout".to_string(),
            title: "Deploy?".to_string(),
            body: Some("Context here.".to_string()),
            options: Some(vec!["Yes".into(), "No".into()]),
            priority: 1,
            kind: "approval".to_string(),
            session_id: Some("sess-1".to_string()),
            session_kind: Some("sandbox".to_string()),
            ..Default::default()
        })
        .expect("create");
        assert_eq!(item.status, "waiting");
        assert_eq!(item.kind, "approval");
        assert_eq!(item.priority, 1);
        assert_eq!(item.options.as_deref(), Some(&["Yes".to_string(), "No".to_string()][..]));
        assert_eq!(item.comment_count, 1, "thread seeded with the ask");
        let (_, comments) = get_with_comments(&item.id).expect("get");
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].author, "scout");
        assert!(comments[0].body.contains("Deploy?") && comments[0].body.contains("Context here."));

        // Defaults: kind question, priority 3.
        let d = ask(&p, "plain");
        assert_eq!(d.kind, "question");
        assert_eq!(d.priority, 3);
        assert!(d.options.is_none());
        assert!(d.session_id.is_none(), "null session must not fail the ask");
    }

    #[test]
    fn create_validation_fails_loudly() {
        let p = pid("valid");
        let base = || NewFeedback {
            project_id: p.clone(),
            agent_name: "scout".to_string(),
            title: "t".to_string(),
            ..Default::default()
        };
        assert!(create(NewFeedback { title: "  ".into(), ..base() }).is_err());
        assert!(create(NewFeedback { kind: "bug".into(), ..base() }).is_err());
        assert!(create(NewFeedback { priority: 9, ..base() }).is_err());
        assert!(create(NewFeedback { priority: -1, ..base() }).is_err());
        assert!(create(NewFeedback { agent_name: "".into(), ..base() }).is_err());
    }

    #[test]
    fn prefix_resolution_unique_ambiguous_notfound() {
        let p = pid("prefix");
        let a = ask(&p, "first");
        let b = ask(&p, "second");

        // Full id resolves to itself; a UNIQUE short prefix resolves.
        assert_eq!(resolve_id_prefix(&a.id), Ok(a.id.clone()));
        // Find the shortest prefix of `a` that differs from `b`.
        let uniq_len = a
            .id
            .chars()
            .zip(b.id.chars())
            .position(|(x, y)| x != y)
            .expect("uuids differ")
            + 1;
        let uniq = &a.id[..uniq_len];
        assert_eq!(resolve_id_prefix(uniq), Ok(a.id.clone()));

        // Nothing matches → NotFound.
        assert_eq!(
            resolve_id_prefix("zzzz-not-a-real-prefix"),
            Err(PrefixError::NotFound)
        );
        assert_eq!(resolve_id_prefix("  "), Err(PrefixError::NotFound));

        // The empty-shared-prefix case: craft two rows whose ids share a
        // prefix by inserting directly (uuids rarely collide on the
        // first chars, so force it).
        let db = crate::db::shared();
        let conn = db.lock();
        for suffix in ["aaaa1", "aaaa2"] {
            conn.execute(
                "INSERT INTO feedback (id, project_id, agent_name, kind, title, priority, status, created_at, updated_at) \
                 VALUES (?1, ?2, 'scout', 'question', 't', 3, 'waiting', 0, 0)",
                params![format!("fbtest-{suffix}"), p],
            )
            .expect("insert");
        }
        drop(conn);
        match resolve_id_prefix("fbtest-aaaa") {
            Err(PrefixError::Ambiguous(c)) => {
                assert_eq!(c.len(), 2, "both candidates listed: {c:?}");
                assert!(c.iter().all(|id| id.starts_with("fbtest-aaaa")));
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
        // LIKE wildcards must not widen matching.
        assert_eq!(resolve_id_prefix("%"), Err(PrefixError::NotFound));
        assert_eq!(resolve_id_prefix("fbtest_aaaa"), Err(PrefixError::NotFound));
    }

    #[test]
    fn status_transitions_and_answer_flow() {
        let p = pid("status");
        let item = ask(&p, "Which color?");
        assert_eq!(item.status, "waiting");
        assert!(item.answer.is_none());
        assert!(item.answered_at.is_none());

        // Comment does NOT change status.
        add_comment(&item.id, "owner", "looking...").expect("comment");
        let after = get_item(&item.id).expect("get");
        assert_eq!(after.status, "waiting");
        assert_eq!(after.comment_count, 2);
        assert!(after.updated_at >= item.updated_at);

        // Answer: comment + denormalized answer + answered_at + status.
        let answered = set_answer(&item.id, "owner", "navy").expect("answer");
        assert_eq!(answered.status, "answered");
        assert_eq!(answered.answer.as_deref(), Some("navy"));
        assert!(answered.answered_at.is_some());
        assert_eq!(answered.comment_count, 3, "answer lands in the thread too");
        let (_, comments) = get_with_comments(&item.id).expect("get");
        assert_eq!(comments.last().expect("has comments").body, "navy");

        // Resolve, dismiss, invalid.
        let resolved = set_status(&item.id, "resolved").expect("resolve");
        assert_eq!(resolved.status, "resolved");
        assert_eq!(resolved.answer.as_deref(), Some("navy"), "answer survives resolve");
        let dismissed = set_status(&item.id, "dismissed").expect("dismiss");
        assert_eq!(dismissed.status, "dismissed");
        assert!(set_status(&item.id, "closed").is_err(), "invalid status fails loudly");

        // Unknown ids fail loudly everywhere.
        assert!(add_comment("nope", "owner", "x").is_err());
        assert!(set_answer("nope", "owner", "x").is_err());
        assert!(set_status("nope", "resolved").is_err());
        // Empty comment/answer bodies are rejected.
        assert!(add_comment(&item.id, "owner", "  ").is_err());
        assert!(set_answer(&item.id, "owner", "").is_err());
    }

    #[test]
    fn list_filtering_default_hides_closed() {
        let p = pid("list");
        let w = ask(&p, "waiting one");
        let a = ask(&p, "answered one");
        set_answer(&a.id, "owner", "yes").expect("answer");
        let r = ask(&p, "resolved one");
        set_status(&r.id, "resolved").expect("resolve");
        let d = ask(&p, "dismissed one");
        set_status(&d.id, "dismissed").expect("dismiss");

        let open = list_for_project(&p, &ListFilter::Open).expect("open");
        let open_ids: Vec<&str> = open.iter().map(|i| i.id.as_str()).collect();
        assert!(open_ids.contains(&w.id.as_str()));
        assert!(open_ids.contains(&a.id.as_str()));
        assert!(!open_ids.contains(&r.id.as_str()), "default hides resolved");
        assert!(!open_ids.contains(&d.id.as_str()), "default hides dismissed");

        let all = list_for_project(&p, &ListFilter::All).expect("all");
        assert_eq!(all.len(), 4);
        // Newest first.
        let times: Vec<i64> = all.iter().map(|i| i.created_at).collect();
        let mut sorted = times.clone();
        sorted.sort_by(|x, y| y.cmp(x));
        assert_eq!(times, sorted, "list is newest-first");

        let just_resolved =
            list_for_project(&p, &ListFilter::Status("resolved".to_string())).expect("status");
        assert_eq!(just_resolved.len(), 1);
        assert_eq!(just_resolved[0].id, r.id);

        assert!(
            list_for_project(&p, &ListFilter::Status("bogus".to_string())).is_err(),
            "invalid status filter fails loudly"
        );

        // Other projects' items never leak in.
        let other = pid("list-other");
        ask(&other, "elsewhere");
        let still = list_for_project(&p, &ListFilter::All).expect("all");
        assert_eq!(still.len(), 4);
    }
}
