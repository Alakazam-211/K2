//! thread-system-notice-v1: K2's own Thread lines in the store.

use super::doc::{NoticeAction, SystemNotice, AUTHOR_SYSTEM, NOTICE_ACTION_RESEND, NOTICE_RESEND_ROUTE};
use super::*;
use rusqlite::params;

fn seed(handle: &str) -> (std::sync::Arc<crate::db::DbLock>, String, String) {
    let dbh = crate::db::init_for_tests();
    let id = uuid::Uuid::new_v4().to_string();
    {
        let c = dbh.lock();
        c.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            params![id, handle, format!("/tmp/ovl-sys-{handle}-{id}")],
        )
        .expect("seed project");
    }
    (dbh, id, uuid::Uuid::new_v4().to_string())
}

/// The wire shape a client keys on, and docs from before it still parse.
#[test]
fn a_system_notice_round_trips_and_old_docs_are_unchanged() {
    let n = SystemNotice::delivery("delivery.not_taken.never_read_in").with_open_terminal(" sales ");
    let mut n2 = n.clone();
    n2.actions.insert(0, NoticeAction::resend("sales", "d-1"));
    let d = OverlayDoc::system_text("d-1".into(), "sales".into(), "K2: sales didn't take a message.".into(), "thread", n2);
    let v = serde_json::to_value(&d).expect("json");
    assert_eq!(v["author"], serde_json::json!({"kind": "system", "login": "k2"}));
    assert_eq!(v["from"], "k2");
    assert_eq!(v["kind"], "text");
    assert_eq!(v["via"], "thread");
    assert_eq!(v["body"], "K2: sales didn't take a message.");
    assert_eq!(
        v["systemNotice"],
        serde_json::json!({
            "kind": "delivery",
            "code": "delivery.not_taken.never_read_in",
            "actions": [
                {"id": "resend", "label": "Send again", "route": NOTICE_RESEND_ROUTE, "body": {"thread": "sales", "doc": "d-1"}},
                {"id": "open_terminal", "label": "Open its terminal", "workspace": "sales"},
            ],
        })
    );
    assert_eq!(serde_json::from_value::<OverlayDoc>(v).expect("parse"), d);
    assert!(d.by_system() && !d.by_person(), "K2's line is nobody's post");

    // An info notice writes no `actions`; an ordinary doc writes no field.
    let info = serde_json::to_string(&SystemNotice::info("job.added")).expect("json");
    assert_eq!(info, r#"{"kind":"info","code":"job.added"}"#);
    let plain = OverlayDoc::text("i".into(), "alice".into(), "k2/1".into(), "hi".into(), "compose");
    let s = serde_json::to_string(&plain).expect("json");
    assert!(!s.contains("systemNotice"), "{s}");
    let old = r#"{"id":"i","kind":"text","from":"sales","to":"sales","created_at":1,"body":"K2: an old notice","via":"thread"}"#;
    let old: OverlayDoc = serde_json::from_str(old).expect("old doc");
    assert_eq!((old.system_notice.as_ref(), old.author.as_ref()), (None, None));
    assert!(!old.by_system());
    // A later kind and a later action id still parse.
    let later = r#"{"id":"i","kind":"text","from":"k2","created_at":1,"author":{"kind":"system","login":"k2"},
        "systemNotice":{"kind":"warning","code":"x.y","actions":[{"id":"later","label":"L"}]}}"#;
    let later: OverlayDoc = serde_json::from_str(later).expect("later");
    assert_eq!(later.system_notice.expect("notice").actions[0].id, "later");
    assert_eq!(later.author.expect("author").kind, AUTHOR_SYSTEM);
}

/// The changes card and the mail notice are K2's too.
#[test]
fn k2s_cards_carry_the_stamp() {
    let c = OverlayDoc::changes_card("c".into(), "k2".into(), "s".into(), "Changed 1 file".into(), ThreadChanges::default());
    assert_eq!(c.author, Some(PostAuthor::system()));
    assert_eq!(c.system_notice.as_ref().map(|n| (n.kind.as_str(), n.code.as_str())), Some(("info", "turn.changes")));
    assert_eq!((c.kind.as_str(), c.from.as_str()), ("changes", "k2"));
}

/// Posted, read back, and "Send again" claimed once: the first claim takes
/// the kept delivery and marks the button done (a delta change); every
/// later claim is `Already` and takes nothing.
#[test]
fn a_notice_resend_is_claimed_once() {
    let (dbh, project_id, conv) = seed(&format!("ovl-sys-{}", &uuid::Uuid::new_v4().to_string()[..6]));
    let c = dbh.lock();
    let (item, links) = post_system_notice(
        &c,
        &conv,
        &project_id,
        "sales",
        "K2: sales didn't take a message.",
        "thread",
        SystemNotice::delivery("delivery.not_taken.retry_failed").with_open_terminal("sales"),
        Some(r#"{"job":1}"#),
    )
    .expect("post");
    drop(c);
    assert_eq!(links.len(), 1);
    let n = item.doc.system_notice.as_ref().expect("notice");
    assert_eq!(n.actions.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), vec![NOTICE_ACTION_RESEND, "open_terminal"]);
    assert_eq!(n.resend().and_then(|a| a.body.clone()), Some(serde_json::json!({"thread": "sales", "doc": item.id})));
    let back = read_thread(&conv, 0).expect("read");
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].doc, item.doc, "history keeps the doc as posted");
    assert_eq!(notice_resend_job(&item.id).expect("job"), Some(r#"{"job":1}"#.to_string()));

    let (claimed, job) = match claim_notice_resend(&conv, &item.id, PostAuthor::owner()).expect("claim") {
        ResendClaim::Claimed { item, job } => (item, job),
        other => panic!("the first press claims: {other:?}"),
    };
    assert_eq!(job, r#"{"job":1}"#);
    let a = claimed.doc.system_notice.as_ref().and_then(|n| n.resend()).expect("resend").clone();
    let at = a.done_at.expect("doneAt");
    assert_eq!(a.done_by, Some(PostAuthor::owner()));
    assert_eq!(claimed.doc.body, item.doc.body, "the text is unchanged");
    assert_eq!(notice_resend_job(&item.id).expect("job"), None, "the kept delivery is gone");
    let delta = read_thread_delta(&conv, 1, 50).expect("delta");
    assert!(delta.changed.iter().any(|i| i.id == item.id && i.doc == claimed.doc), "a delta reader sees the press: {:?}", delta.changed);

    for _ in 0..2 {
        match claim_notice_resend(&conv, &item.id, PostAuthor::user("alice")).expect("again") {
            ResendClaim::Already { done_at, item: again } => {
                assert_eq!(done_at, at);
                assert_eq!(again.doc, claimed.doc, "nothing changed");
            }
            other => panic!("a second press is Already: {other:?}"),
        }
    }
    assert_eq!(claim_notice_resend(&conv, "no-such", PostAuthor::owner()).expect_err("unknown"), NOTICE_NOT_FOUND);
    assert_eq!(claim_notice_resend(&uuid::Uuid::new_v4().to_string(), &item.id, PostAuthor::owner()).expect_err("other chat"), NOTICE_NOT_FOUND);
}

/// Only K2's own notice with a kept delivery can be sent again; a
/// person's post (even one shaped like a notice) can't.
#[test]
fn only_k2s_delivery_notice_resends() {
    let (dbh, project_id, conv) = seed(&format!("ovl-sysn-{}", &uuid::Uuid::new_v4().to_string()[..6]));
    let c = dbh.lock();
    let (info, _) = post_system_notice(&c, &conv, &project_id, "s", "K2: job added", "k2-job", SystemNotice::info("job.added"), None).expect("info");
    let (post, _) = post_thread_with_origin(&c, &conv, &project_id, "owner", "s", "hi", "compose", None, Some(PostAuthor::owner())).expect("post");
    // A caller's resend action is dropped: only a kept delivery adds one.
    let mut fake = SystemNotice::delivery("x");
    fake.actions.push(NoticeAction::resend("s", "elsewhere"));
    let (no_job, _) = post_system_notice(&c, &conv, &project_id, "s", "K2: x", "thread", fake, None).expect("no job");
    drop(c);
    assert!(no_job.doc.system_notice.as_ref().expect("notice").resend().is_none(), "{:?}", no_job.doc);
    for id in [&info.id, &post.id, &no_job.id] {
        assert_eq!(claim_notice_resend(&conv, id, PostAuthor::owner()).expect_err("not resendable"), NOTICE_NOT_RESENDABLE, "{id}");
    }
    // Every post funnel refuses K2's stamp.
    let c = dbh.lock();
    let refuse = |r: Result<(OverlayItem, Vec<OverlayLink>), String>| assert_eq!(r.expect_err("refused"), SYSTEM_AUTHOR_REFUSED);
    refuse(post_thread_with_origin(&c, &conv, &project_id, "k2", "s", "x", "thread", None, Some(PostAuthor::system())));
    refuse(post_choice_by(&c, &conv, &project_id, "k2", "s", "Ship?", vec!["Go".into()], false, None, Some(PostAuthor::system()), None));
    refuse(post_secret_by(&c, &conv, &project_id, "k2", "s", "API_KEY", None, SecretProposal::default(), None, Some(PostAuthor::system())));
    refuse(post_thread_design(
        &c,
        &conv,
        &project_id,
        "k2",
        "s",
        "x",
        "compose",
        DesignOrigin { item_id: None, url: "https://example.com/".into(), notes: 1 },
        Some(PostAuthor::system()),
    ));
}
