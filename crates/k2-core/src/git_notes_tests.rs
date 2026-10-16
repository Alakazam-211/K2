//! Review notes store, anchors and message format (C5).

use super::*;

fn conn_with_project() -> (crate::db::TestDbGuard, String) {
    let g = crate::db::scoped_for_test();
    let id = format!("p-notes-{}", uuid::Uuid::new_v4().simple());
    {
        let h = g.handle();
        let c = h.lock();
        c.execute("INSERT INTO projects (id, name, path) VALUES (?1, 'notes', '/t/notes')", params![id]).expect("project");
    }
    (g, id)
}

fn note(ws_id: &str, path: &str, start: i64, end: i64, body: &str) -> ReviewNote {
    let now = now_ms();
    ReviewNote {
        id: new_id(),
        workspace_id: ws_id.to_string(),
        workspace_path: "/t/notes".into(),
        path: path.into(),
        side: "new".into(),
        start_line: start,
        end_line: end,
        body: body.into(),
        author: "user:ana".into(),
        author_name: "ana".into(),
        created_at: now,
        updated_at: now,
        sent_at: None,
        delivery_id: None,
        resolved_at: None,
        resolved_by: None,
        rev: Some("r:1".into()),
        blob: None,
        anchor_text: None,
    }
}

#[test]
fn crud_filters_counts_and_send_once() {
    let (g, ws) = conn_with_project();
    let h = g.handle();
    let c = h.lock();
    let a = note(&ws, "src/b.rs", 4, 6, "range");
    let b = note(&ws, "src/a.rs", 2, 2, "one line");
    let r = note(&ws, "src/a.rs", 9, 9, "to resolve");
    for n in [&a, &b, &r] {
        insert(&c, n).expect("insert");
    }
    // Sorted by file then line.
    let open = list(&c, "/t/notes", Status::Open, None).expect("list");
    assert_eq!(open.iter().map(|n| n.body.as_str()).collect::<Vec<_>>(), vec!["one line", "to resolve", "range"]);
    assert_eq!(list(&c, "/t/notes", Status::Open, Some("src/b.rs")).expect("file").len(), 1);
    assert_eq!(list(&c, "/elsewhere", Status::All, None).expect("other").len(), 0);

    assert!(update_body(&c, &b.id, "edited", now_ms()).expect("update"));
    assert_eq!(get(&c, &b.id).expect("get").expect("row").body, "edited");
    assert!(set_resolved(&c, &r.id, Some("agent:k2"), now_ms()).expect("resolve"));
    let got = get(&c, &r.id).expect("get").expect("row");
    assert_eq!(got.resolved_by.as_deref(), Some("agent:k2"));
    assert!(got.resolved_at.is_some());

    // Send marks only open unsent notes, once.
    let ids = vec![a.id.clone(), b.id.clone(), r.id.clone()];
    assert_eq!(mark_sent(&c, &ids, Some("m_1"), now_ms()).expect("mark"), 2, "resolved one is skipped");
    assert_eq!(mark_sent(&c, &ids, Some("m_2"), now_ms()).expect("mark again"), 0, "already sent");
    assert_eq!(get(&c, &a.id).expect("get").expect("row").delivery_id.as_deref(), Some("m_1"));
    assert_eq!(counts(&c, "/t/notes").expect("counts"), NoteCounts { open: 2, unsent: 0, sent: 2, resolved: 1 });
    assert_eq!(list(&c, "/t/notes", Status::Sent, None).expect("sent").len(), 2);
    assert_eq!(list(&c, "/t/notes", Status::Resolved, None).expect("resolved").len(), 1);

    // Reopen clears sent so it goes out again.
    assert!(set_resolved(&c, &r.id, None, now_ms()).expect("reopen"));
    assert_eq!(list(&c, "/t/notes", Status::Unsent, None).expect("unsent").len(), 1);
    assert!(delete(&c, &a.id).expect("delete"));
    assert!(!delete(&c, &a.id).expect("delete twice"));
    assert_eq!(counts(&c, "/t/notes").expect("counts"), NoteCounts { open: 2, unsent: 1, sent: 1, resolved: 0 });
}

#[test]
fn notes_go_with_their_workspace_and_check_ranges() {
    let (g, ws) = conn_with_project();
    let h = g.handle();
    let c = h.lock();
    insert(&c, &note(&ws, "a", 1, 1, "x")).expect("insert");
    let mut bad = note(&ws, "a", 3, 2, "x");
    assert!(insert(&c, &bad).is_err(), "end before start");
    bad.start_line = 0;
    bad.end_line = 0;
    assert!(insert(&c, &bad).is_err(), "line 0");
    let mut side = note(&ws, "a", 1, 1, "x");
    side.side = "left".into();
    assert!(insert(&c, &side).is_err(), "side is old|new");
    c.execute("DELETE FROM projects WHERE id = ?1", params![ws]).expect("drop project");
    assert_eq!(list(&c, "/t/notes", Status::All, None).expect("list").len(), 0, "cascade");
}

#[test]
fn status_parse() {
    assert_eq!(Status::parse(None), Some(Status::Open));
    assert_eq!(Status::parse(Some("sent")), Some(Status::Sent));
    assert_eq!(Status::parse(Some("unsent")), Some(Status::Unsent));
    assert_eq!(Status::parse(Some("resolved")), Some(Status::Resolved));
    assert_eq!(Status::parse(Some("all")), Some(Status::All));
    assert_eq!(Status::parse(Some("closed")), None);
}

#[test]
fn clean_body_keeps_lines_and_drops_control_chars() {
    assert_eq!(clean_body("  a\r\nb\u{1b}[201~\tc\u{7}  "), "a\nb[201~\tc");
}

#[test]
fn anchors_follow_moved_lines_and_flag_rewrites() {
    let text = "a\nb\nc\nd\n";
    assert_eq!(split_lines(text), vec!["a", "b", "c", "d"]);
    assert_eq!(split_lines("a\r\nb"), vec!["a", "b"]);
    assert_eq!(split_lines(""), Vec::<&str>::new());
    assert_eq!(anchor_from_text(text, 2, 3).as_deref(), Some("b\nc"));
    assert_eq!(anchor_from_text(text, 4, 5), None);
    assert_eq!(anchor_from_text("x\n\ny\n", 2, 2).as_deref(), Some(""), "a blank line is an anchor too");

    // Same place.
    assert_eq!(locate(Some("b\nc"), 2, 3, text), Some((2, 3)));
    // Two lines were added above: follow.
    assert_eq!(locate(Some("b\nc"), 2, 3, "new\nnew\na\nb\nc\nd\n"), Some((4, 5)));
    // The nearest copy wins.
    assert_eq!(locate(Some("x"), 5, 5, "x\n1\n2\n3\n4\n5\nx\n"), Some((7, 7)));
    // Rewritten: outdated.
    assert_eq!(locate(Some("b\nc"), 2, 3, "a\nB\nc\nd\n"), None);
    // Blank-line anchor still matches.
    assert_eq!(locate(Some(""), 2, 2, "x\n\ny\n"), Some((2, 2)));
    // No anchor: stays while the range exists.
    assert_eq!(locate(None, 3, 4, text), Some((3, 4)));
    assert_eq!(locate(None, 4, 5, text), None);
}

#[test]
fn message_has_file_lines_and_comment_blocks() {
    let mut a = note("w", "src/a.rs", 12, 12, "Handle None here.");
    a.id = "n_aaaaaaaaaaaa".into();
    let mut b = note("w", "src/b.rs", 4, 9, "Two lines\nof comment");
    b.id = "n_bbbbbbbbbbbb".into();
    b.side = "old".into();
    let (msg, n) = format_message(&[a, b], "/t/notes");
    assert_eq!(n, 2);
    assert!(msg.starts_with("Review notes on your changes in /t/notes (2 notes)."), "{msg}");
    assert!(msg.contains("File: src/a.rs\nLine: 12\nComment: Handle None here.\nNote id: n_aaaaaaaaaaaa"), "{msg}");
    assert!(
        msg.contains("File: src/b.rs\nLines: 4-9 (in the text before the change)\nComment: Two lines\n  of comment\nNote id: n_bbbbbbbbbbbb"),
        "{msg}"
    );
    assert!(msg.ends_with("List them any time with `k2 changes notes`."), "{msg}");
    assert_eq!(format_message(&[], "/t"), (String::new(), 0));
}

#[test]
fn message_stops_at_the_size_cap_but_carries_at_least_one() {
    let big = "x".repeat(MAX_MESSAGE_CHARS / 3);
    let notes: Vec<ReviewNote> = (0..5).map(|i| note("w", &format!("f{i}"), 1, 1, &big)).collect();
    let (msg, n) = format_message(&notes, "/t");
    assert_eq!(n, 2, "two of five fit");
    assert!(msg.len() <= MAX_MESSAGE_CHARS, "{}", msg.len());
    assert!(msg.contains("(2 notes)"));
    let huge = note("w", "f", 1, 1, &"y".repeat(MAX_MESSAGE_CHARS * 2));
    assert_eq!(format_message(&[huge], "/t").1, 1, "one oversized note still goes");
}
