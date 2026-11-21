//! ticket-thread-v1 bites (k2-core): the facade's shapes, the move of old
//! comment rows, the lifetime rules (project delete leaves ticket rows,
//! ticket delete takes them, the sweep), the walkers that leave ticket
//! conversations out, and the clone export.

use super::*;
use crate::feedback::{self, ListFilter, NewFeedback};
use crate::overlay::ThreadAttachment;

fn pid(label: &str) -> String {
    format!("tt-{label}-{}", uuid::Uuid::new_v4())
}

fn file(path: &str) -> ThreadAttachment {
    ThreadAttachment {
        kind: "file".into(),
        path: path.into(),
        name: path.rsplit('/').next().unwrap_or(path).into(),
        size: 3,
        mime: "text/plain".into(),
        modified_at: 1,
        sha256: None,
        line: None,
        line_end: None,
    }
}

fn ask(project_id: &str, title: &str) -> feedback::FeedbackItem {
    feedback::create(NewFeedback {
        project_id: project_id.to_string(),
        agent_name: "agent-a".to_string(),
        title: title.to_string(),
        ..Default::default()
    })
    .expect("create ticket")
}

fn catalog_kind(conv: &str) -> Option<(String, Option<String>, String)> {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT kind, ticket_id, project_id FROM overlay_conversations WHERE conversation_id = ?1",
        params![conv],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
    .expect("catalog read")
}

/// A ticket as 0.46.16 left it: its messages in `feedback_comments`,
/// `comments_in_thread` 0, the counters from the migration's subqueries.
/// `rows`: (author, body, created_at, attachments_json).
fn legacy_ticket(project_id: &str, rows: &[(&str, &str, i64, Option<&str>)]) -> (String, Vec<String>) {
    let id = uuid::Uuid::new_v4().to_string();
    let db = crate::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO feedback (id, project_id, agent_name, kind, title, priority, status, created_at, updated_at) \
         VALUES (?1, ?2, 'agent-a', 'question', 'old ask', 3, 'waiting', 10, 10)",
        params![id, project_id],
    )
    .expect("legacy ticket");
    let mut ids = Vec::new();
    for (author, body, at, files) in rows {
        let cid = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO feedback_comments (id, feedback_id, author, body, created_at, attachments_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![cid, id, author, body, at, files],
        )
        .expect("legacy comment");
        ids.push(cid);
    }
    conn.execute(
        "UPDATE feedback SET comment_count = ?2, file_count = ?3 WHERE id = ?1",
        params![id, rows.len() as i64, rows.iter().filter(|r| r.3.is_some_and(|f| f.starts_with('['))).count() as i64],
    )
    .expect("legacy counts");
    (id, ids)
}

/// Bite (facade shapes): a new ticket's messages are Thread docs of
/// `ticket:<id>` (catalog kind `ticket`, each doc carrying `ticket`), and
/// `show`'s comments read back exactly what was written: id, author, body,
/// time, files. The list counters move with each message.
#[test]
fn a_new_ticket_lives_in_its_thread_with_the_same_comment_shape() {
    let p = pid("shape");
    let item = feedback::create_with_brief_and_files(
        NewFeedback { project_id: p.clone(), agent_name: "agent-a".into(), title: "Ship it?".into(), body: Some("details".into()), ..Default::default() },
        None,
        &[file("out/a.png")],
    )
    .expect("create");
    assert_eq!((item.comment_count, item.file_count), (1, 1), "the seed and its file are counted on the row");
    let conv = ticket::conversation_id(&item.id);
    assert_eq!(catalog_kind(&conv), Some(("ticket".into(), Some(item.id.clone()), p.clone())));
    assert!(in_thread(&item.id));

    let c = feedback::add_comment_with_files(&item.id, "alice", "looks good", &[file("b.md"), file("c.md")]).expect("comment");
    let docs = ticket::read_messages(&item.id).expect("thread read");
    assert_eq!(docs.len(), 2, "seed + comment, as Thread items 1 and 2");
    assert_eq!(docs.iter().map(|d| d.seq).collect::<Vec<_>>(), vec![1, 2]);
    assert!(docs.iter().all(|d| d.doc.ticket.as_deref() == Some(item.id.as_str())), "each doc carries its ticket");
    assert_eq!(docs[1].id, c.id, "the comment id IS the doc id");

    let (after, comments) = feedback::get_with_comments(&item.id).expect("show");
    assert_eq!((after.comment_count, after.file_count), (2, 3));
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].author, "agent-a");
    assert_eq!(comments[0].body, "Ship it?\ndetails");
    assert_eq!(comments[0].attachments.len(), 1);
    assert_eq!(comments[1].id, c.id);
    assert_eq!(comments[1].author, "alice");
    assert_eq!(comments[1].body, "looks good");
    assert_eq!(comments[1].created_at, c.created_at);
    assert_eq!(comments[1].attachments.iter().map(|a| a.path.as_str()).collect::<Vec<_>>(), vec!["b.md", "c.md"]);
    let one = feedback::get_comment(&item.id, &c.id).expect("one comment by id");
    assert_eq!(one.body, "looks good");
    assert!(feedback::get_comment(&item.id, "not-a-comment").is_none());

    let listed = feedback::list_for_project(&p, &ListFilter::All).expect("list");
    assert_eq!((listed[0].comment_count, listed[0].file_count), (2, 3), "the list reads the row's counters");
}

/// Bite: an option pick through the core is the answer (status answered,
/// the answer on the row and in the Thread).
#[test]
fn an_answer_is_stored_on_the_row_and_in_the_thread() {
    let p = pid("answer");
    let item = ask(&p, "Pick");
    let (after, c) = feedback::set_answer_by(
        &item.id,
        &feedback::CommentBy { name: "alice".into(), author: Some(PostAuthor::user("alice")) },
        "Yes",
        &[],
    )
    .expect("answer");
    assert_eq!(after.status, "answered");
    assert_eq!(after.answer.as_deref(), Some("Yes"));
    let docs = ticket::read_messages(&item.id).expect("thread");
    let last = docs.last().expect("answer doc");
    assert_eq!(last.id, c.id);
    assert_eq!(last.doc.author, Some(PostAuthor::user("alice")), "the credential's stamp rides the doc");
}

/// Bite (backfill): an old ticket reads its old rows, then moves whole:
/// the same comments (ids, authors, bodies, times, files; a corrupt column
/// still reads as none), the counters untouched, and a second move adds
/// nothing.
#[test]
fn an_old_ticket_moves_to_its_thread_with_the_same_comments() {
    let p = pid("move");
    let (id, cids) = legacy_ticket(
        &p,
        &[
            ("agent-a", "old ask", 10, None),
            ("owner", "first reply", 11, Some(r#"[{"kind":"file","path":"a.md","name":"a.md","size":1,"mime":"text/markdown","modifiedAt":1}]"#)),
            ("alice", "corrupt files", 11, Some("{not json")),
        ],
    );
    assert!(!in_thread(&id));
    let (before_item, before) = feedback::get_with_comments(&id).expect("old rows read");
    assert_eq!(before.len(), 3);

    {
        let db = crate::db::shared();
        let conn = db.lock();
        assert_eq!(move_ticket_with(&conn, &id).expect("move"), 3);
        assert_eq!(move_ticket_with(&conn, &id).expect("again"), 0, "a moved ticket is left alone");
    }
    assert!(in_thread(&id));
    let (after_item, after) = feedback::get_with_comments(&id).expect("Thread read");
    let shape = |c: &FeedbackComment| (c.id.clone(), c.author.clone(), c.body.clone(), c.created_at, c.attachments.len());
    assert_eq!(after.iter().map(shape).collect::<Vec<_>>(), before.iter().map(shape).collect::<Vec<_>>());
    assert_eq!(after.iter().map(|c| c.id.clone()).collect::<Vec<_>>(), cids);
    assert_eq!((after_item.comment_count, after_item.file_count), (before_item.comment_count, before_item.file_count));
    let docs = ticket::read_messages(&id).expect("docs");
    assert_eq!(docs[0].doc.author, None, "the ticket's agent: no stamp");
    assert_eq!(docs[1].doc.author, Some(PostAuthor::owner()));
    assert_eq!(docs[2].doc.author, Some(PostAuthor::user("alice")));
    assert_eq!(crate::overlay::store::import_id(&ticket::import_key(&cids[1])).expect("key"), Some(cids[1].clone()));

    // A failed move is all or nothing: undo the mark by hand and move again,
    // and the import keys keep the lines from landing twice.
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute("UPDATE feedback SET comments_in_thread = 0 WHERE id = ?1", params![id]).unwrap();
        assert_eq!(move_ticket_with(&conn, &id).expect("re-run"), 0);
    }
    assert_eq!(ticket::read_messages(&id).expect("docs").len(), 3, "resumable: no line twice");
}

/// A message on a ticket the backfill hasn't reached moves it first, so
/// the order holds.
#[test]
fn a_message_on_an_old_ticket_moves_it_first() {
    let p = pid("touch");
    let (id, _) = legacy_ticket(&p, &[("agent-a", "old ask", 10, None)]);
    let c = feedback::add_comment_with_files(&id, "alice", "new words", &[]).expect("comment");
    let (item, comments) = feedback::get_with_comments(&id).expect("show");
    assert_eq!(comments.iter().map(|c| c.body.as_str()).collect::<Vec<_>>(), vec!["old ask", "new words"]);
    assert_eq!(comments[1].id, c.id);
    assert_eq!(item.comment_count, 2);
}

/// The backfill moves every old ticket and stops.
#[test]
fn the_backfill_moves_every_old_ticket() {
    let _db = crate::db::scoped_for_test();
    let p = pid("backfill");
    let (a, _) = legacy_ticket(&p, &[("agent-a", "one", 1, None)]);
    let (b, _) = legacy_ticket(&p, &[("agent-a", "two", 1, None), ("owner", "reply", 2, None)]);
    let mut after = String::new();
    while let Some(last) = backfill_step(&after, 1).expect("step") {
        after = last;
    }
    assert!(in_thread(&a) && in_thread(&b));
    assert_eq!(feedback::get_with_comments(&b).expect("show").1.len(), 2);
    assert_eq!(backfill_step("", 10).expect("nothing left"), None);
}

/// Bite (cascade exclusion): deleting a workspace removes its Thread
/// catalog rows but never its tickets' rows; deleting a ticket removes its
/// row in the same statement.
#[test]
fn a_project_delete_keeps_ticket_rows_and_a_ticket_delete_takes_its_row() {
    let _db = crate::db::scoped_for_test();
    let p = pid("cascade");
    let db = crate::db::shared();
    {
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, 'cascade', ?2)", params![p, format!("/tmp/{p}")]).unwrap();
        crate::overlay::post_thread(&conn, "conv-cascade", &p, "agent-a", "agent-a", "hi", "thread").expect("thread post");
    }
    let item = ask(&p, "Survives?");
    let conv = ticket::conversation_id(&item.id);
    assert!(catalog_kind("conv-cascade").is_some());
    {
        let conn = db.lock();
        conn.execute("DELETE FROM projects WHERE id = ?1", params![p]).unwrap();
    }
    assert!(catalog_kind("conv-cascade").is_none(), "a workspace's Thread row goes with it");
    assert_eq!(catalog_kind(&conv).map(|k| k.0), Some("ticket".into()), "its ticket's row stays");
    assert_eq!(feedback::get_with_comments(&item.id).expect("the ticket still reads").1.len(), 1);
    {
        let conn = db.lock();
        conn.execute("DELETE FROM feedback WHERE id = ?1", params![item.id]).unwrap();
    }
    assert!(catalog_kind(&conv).is_none(), "a hard-deleted ticket takes its row");
    let swept = sweep_orphans().expect("sweep");
    assert!(swept.conversations.contains(&conv), "the sweep removes its stored messages: {swept:?}");
    assert!(ticket::read_messages(&item.id).expect("read").is_empty());
}

/// The daily sweep removes a ticket row whose ticket is gone (a row left
/// by anything but the trigger), and logs it.
#[test]
fn the_sweep_removes_ticket_rows_without_a_ticket() {
    let _db = crate::db::scoped_for_test();
    let p = pid("sweep");
    let item = ask(&p, "Gone soon");
    let conv = ticket::conversation_id(&item.id);
    {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.execute_batch("DROP TRIGGER overlay_conversations_ticket_deleted").unwrap();
        conn.execute("DELETE FROM feedback WHERE id = ?1", params![item.id]).unwrap();
    }
    assert!(catalog_kind(&conv).is_some(), "no trigger: the row is left behind");
    let r = sweep_orphans().expect("sweep");
    assert_eq!(r.conversations, vec![conv.clone()]);
    assert_eq!(r.messages, 1);
    assert!(catalog_kind(&conv).is_none());
    assert_eq!(sweep_orphans().expect("again"), SweepReport::default());
}

/// Bite (walkers): a ticket's unread messages never reach a person's
/// unread summary or totals, and Mark all as read leaves them alone.
#[test]
fn ticket_threads_stay_out_of_unread_totals_and_mark_all_read() {
    let _db = crate::db::scoped_for_test();
    let p = pid("walkers");
    let people = crate::notify_prefs::People { users: Vec::new(), linked: Vec::new() };
    let owner = crate::read_state::Person::of_login(&people, "owner");
    let db = crate::db::shared();
    {
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, 'walk', ?2)", params![p, format!("/tmp/{p}")]).unwrap();
        crate::read_state::ensure_person(&conn, "owner").unwrap();
    }
    let item = ask(&p, "Unread?");
    feedback::add_comment_with_files(&item.id, "agent-a", "more", &[]).unwrap();
    {
        let conn = db.lock();
        crate::overlay::post_thread(&conn, "conv-walk", &p, "agent-a", "agent-a", "agent line", "thread").unwrap();
        let s = crate::read_state::summary(&conn, &owner, crate::read_state::now_secs()).unwrap();
        let ids: Vec<&str> = s.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["conv-walk"], "only the agent's Thread counts");
        assert_eq!(s.totals.total, 1);
        let moved = crate::read_state::ack_project_threads(&conn, "owner", &p).unwrap();
        assert_eq!(moved.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), vec!["conv-walk"]);
        let conv = ticket::conversation_id(&item.id);
        let read: i64 = crate::read_state::read_through(&conn, "owner", crate::read_state::KIND_THREAD, &conv).unwrap();
        assert_eq!(read, 0, "the ticket's cursor did not move");
        // The ticket's own cursor works (the pane marks it read).
        assert!(crate::read_state::ack(&conn, "owner", crate::read_state::KIND_THREAD, &conv, None).unwrap());
        let item_state = crate::read_state::item(&conn, &owner, crate::read_state::KIND_THREAD, &conv, crate::read_state::now_secs()).unwrap();
        assert_eq!((item_state.unread, item_state.last_read_seq, item_state.last_seq), (0, 2, 2));
    }
}

/// Bite (clone export, K2's rule b): a clone carries no tickets, so it
/// carries none of their conversations.
#[test]
fn a_clone_export_leaves_ticket_conversations_out() {
    let _db = crate::db::scoped_for_test();
    let p = pid("export");
    let db = crate::db::shared();
    {
        let conn = db.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, 'export', ?2, 'export-ws')", params![p, format!("/tmp/{p}")]).unwrap();
        crate::overlay::post_thread(&conn, "conv-export", &p, "agent-a", "agent-a", "kept", "thread").unwrap();
    }
    let item = ask(&p, "Not cloned");
    let conn = db.lock();
    let opts = crate::overlay::transfer::ExportOptions::new("clone-1", "host-a", crate::overlay::transfer::ThreadsChoice::All);
    let export = crate::overlay::transfer::export_project_threads(&conn, &p, &opts).expect("export");
    let keys: Vec<&str> = export.conversations.iter().map(|c| c.source_conversation_id.as_str()).collect();
    assert_eq!(keys, vec!["conv-export"]);
    assert!(!keys.contains(&ticket::conversation_id(&item.id).as_str()));
}

/// A deleted ticket message leaves `show` and the counts.
#[test]
fn a_deleted_message_leaves_show_and_the_counts() {
    let p = pid("delete");
    let item = ask(&p, "Oops");
    let c = feedback::add_comment_with_files(&item.id, "alice", "secret words", &[file("x.md")]).unwrap();
    let conv = ticket::conversation_id(&item.id);
    crate::overlay::delete_thread_message(&conv, &crate::overlay::MessageRef::Id(c.id.clone()), PostAuthor::user("alice"), |_| Ok(()))
        .expect_err("a message with no person stamp isn't a person's to delete");
    {
        let db = crate::db::shared();
        let conn = db.lock();
        assert_eq!(note_deleted(&conn, &conv, 1).unwrap(), Some(item.id.clone()));
        assert_eq!(note_deleted(&conn, "conv-other", 1).unwrap(), None);
    }
    let after = feedback::get_item(&item.id).unwrap();
    assert_eq!((after.comment_count, after.file_count), (1, 0));
}
