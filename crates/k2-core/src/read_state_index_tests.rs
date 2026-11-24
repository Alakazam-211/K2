//! Speed pass P6: the unread summary counts from the per-doc facts
//! (`doc_meta`) instead of parsing documents. `old` below is the summary
//! as it was (every line past a cursor read and parsed); each test holds
//! the new path to it.

use super::*;
use crate::overlay::doc_meta::DocMeta;
use crate::overlay::store::{self, doc_meta_test, read_counts};
use crate::overlay::{MessageRef, PostAuthor, SecretProposal};

/// Tests that read the doc-parse counter, or leave docs without a row in
/// the shared store, run one at a time: a row-less pending card from the
/// upgrade test would be parsed by another test's summary.
static COUNTED: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// The read path before P6, kept verbatim as the oracle (plus the one
/// rule added since: K2's info notices never count, 0.46.18).
mod old {
    use super::*;

    fn wrote_thread_doc(person: &Person, doc: &OverlayDoc) -> bool {
        if let Some(g) = &person.guest {
            return doc.author.as_ref().is_some_and(|a| a.kind == overlay::doc::AUTHOR_GUEST && a.login == *g);
        }
        match &doc.author {
            Some(a) if a.kind == overlay::doc::AUTHOR_OWNER => person.key == OWNER,
            Some(a) if a.kind == overlay::doc::AUTHOR_USER => person.is_login(&a.login),
            _ => false,
        }
    }

    fn thread_doc_counts(person: &Person, doc: &OverlayDoc, now: i64) -> bool {
        if person.guest.is_some() && doc.is_action_card() {
            return false;
        }
        !wrote_thread_doc(person, doc)
            && doc.imported.is_none()
            && doc.external.is_none()
            && doc.via.as_deref() != Some("import")
            && !overlay::doc_meta::is_quiet_notice(doc)
            && !(overlay::is_waiting_now(doc, now) && card_waits_for(person, doc))
    }

    fn permission_floor(doc: &OverlayDoc) -> Option<&str> {
        let action = doc.choice.as_ref()?.action.as_ref()?;
        if action.id != crate::permission::CARD_ACTION_PERMISSION && action.id != crate::permission::CARD_ACTION_BATCH {
            return None;
        }
        let floor = action
            .view
            .as_ref()
            .and_then(|v| v.get("approvers"))
            .and_then(|a| a.get("floor"))
            .and_then(serde_json::Value::as_str);
        Some(match floor {
            Some("admin") => "admin",
            _ => "owner",
        })
    }

    fn card_waits_for(person: &Person, doc: &OverlayDoc) -> bool {
        if person.guest.is_some() {
            return !doc.is_action_card();
        }
        let Some(floor) = permission_floor(doc) else { return true };
        if person.key == OWNER {
            return true;
        }
        match crate::connect_users::role_for_user(&person.key) {
            Some(crate::connect_users::Role::Owner) => true,
            Some(crate::connect_users::Role::Admin) => floor == "admin",
            _ => false,
        }
    }

    fn waiting_for(person: &Person, conversation_id: Option<&str>, now: i64) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (conv, card) in overlay::waiting_cards(conversation_id, now).expect("waiting cards") {
            match overlay::get_doc(&card).expect("card doc") {
                Some(doc) if !card_waits_for(person, &doc) => {}
                _ => out.push((conv, card)),
            }
        }
        out
    }

    fn thread_unread(person: &Person, conv: &str, after: i64, head: i64, now: i64) -> i64 {
        if head <= after {
            return 0;
        }
        let page = overlay::read_thread_page(conv, Some(after), None, UNREAD_CAP as usize).expect("page");
        if page.has_more {
            return UNREAD_CAP;
        }
        page.items.iter().filter(|i| thread_doc_counts(person, &i.doc, now)).count() as i64
    }

    pub fn item(conn: &Connection, person: &Person, id: &str, now: i64) -> ReadItem {
        let head = require_head(conn, KIND_THREAD, id).expect("head");
        let cur = cursor(conn, &person.key, KIND_THREAD, id).expect("cursor");
        let muted = cur.muted_at(now);
        ReadItem {
            kind: KIND_THREAD.to_string(),
            id: id.to_string(),
            project_id: thread_project_id(conn, id).expect("project"),
            unread: thread_unread(person, id, cur.last_read_seq, head, now),
            waiting: waiting_for(person, Some(id), now).len() as i64,
            muted,
            muted_until: if muted { cur.muted_until } else { None },
            last_read_seq: cur.last_read_seq,
            last_seq: head,
        }
    }

    /// The old summary's Thread half (the fixtures hold no project chat).
    pub fn summary(conn: &Connection, person: &Person, now: i64) -> Summary {
        let mut waiting: HashMap<String, i64> = HashMap::new();
        for (conv, _) in waiting_for(person, None, now) {
            *waiting.entry(conv).or_default() += 1;
        }
        let mut out = Summary::default();
        let mut stmt = conn
            .prepare(
                "SELECT c.conversation_id, c.project_id, c.last_thread_seq, COALESCE(r.last_read_seq, 0), \
                 COALESCE(r.muted, 0), r.muted_until \
                 FROM overlay_conversations c LEFT JOIN read_cursors r \
                 ON r.person = ?1 AND r.kind = 'thread' AND r.conv_id = c.conversation_id",
            )
            .expect("prepare");
        let rows: Vec<(String, String, i64, Cursor)> = stmt
            .query_map(params![person.key], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    Cursor { last_read_seq: r.get(3)?, muted: r.get::<_, i64>(4)? != 0, muted_until: r.get(5)? },
                ))
            })
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        for (conv, project_id, head, cur) in rows {
            let w = waiting.get(&conv).copied().unwrap_or(0);
            let muted = cur.muted_at(now);
            if head <= cur.last_read_seq && w == 0 && !muted {
                continue;
            }
            let unread = thread_unread(person, &conv, cur.last_read_seq, head, now);
            if unread == 0 && w == 0 && !muted {
                continue;
            }
            let item = ReadItem {
                kind: KIND_THREAD.to_string(),
                id: conv,
                project_id: Some(project_id),
                unread,
                waiting: w,
                muted,
                muted_until: if muted { cur.muted_until } else { None },
                last_read_seq: cur.last_read_seq,
                last_seq: head,
            };
            out.totals.add(&item);
            out.items.push(item);
        }
        out
    }
}

fn people() -> People {
    People::default()
}

fn owner() -> Person {
    Person::of_login(&people(), "owner")
}

fn db() -> (crate::db::TestDbGuard, String) {
    let g = crate::db::scoped_for_test();
    let id = uuid::Uuid::new_v4().to_string();
    {
        let h = g.handle();
        let conn = h.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            params![id, format!("ri{}", &id[..6]), format!("/tmp/ri-{id}")],
        )
        .expect("seed project");
    }
    (g, id)
}

fn conv() -> String {
    format!("ri-conv-{}", uuid::Uuid::new_v4())
}

fn agent_post(conn: &Connection, conv: &str, project: &str, text: &str) -> i64 {
    overlay::post_thread(conn, conv, project, "ri-agent", "ri-agent", text, "thread").expect("post").0.seq
}

fn person_post(conn: &Connection, conv: &str, project: &str, author: PostAuthor, text: &str) -> (String, i64) {
    let (item, _) =
        overlay::post_thread_with_origin(conn, conv, project, &author.login.clone(), "ri-agent", text, "compose", None, Some(author))
            .expect("person post");
    (item.id, item.seq)
}

/// K2's own line (thread-system-notice-v1) with `notice`. Returns its id.
fn k2_notice(conn: &Connection, conv: &str, project: &str, notice: overlay::SystemNotice) -> String {
    let body = format!("K2: {}", notice.code);
    overlay::post_system_notice(conn, conv, project, "ri-agent", &body, "k2", notice, None).expect("notice").0.id
}

fn ask(conn: &Connection, conv: &str, project: &str, expires_at: Option<i64>) -> String {
    overlay::post_choice(conn, conv, project, "ri-agent", "ri-agent", "Ship?", vec!["Go".into(), "Wait".into()], false, expires_at)
        .expect("ask")
        .0
        .id
}

fn action_card(conn: &Connection, conv: &str, project: &str, id: &str, floor: Option<&str>) -> String {
    let mut action = serde_json::json!({ "id": id, "params": {}, "dedupe": uuid::Uuid::new_v4().to_string() });
    if let Some(f) = floor {
        action["view"] = serde_json::json!({ "approvers": { "floor": f, "label": "x", "names": [] } });
    }
    let action: overlay::CardAction = serde_json::from_value(action).expect("action");
    overlay::post_choice_with_action(
        conn, conv, project, "ri-agent", "ri-agent", "Ask", vec!["Allow".into(), "Deny".into()], false, None, Some(action),
    )
    .expect("action card")
    .0
    .id
}

/// A Thread line as a bulk fixture writes it: an agent's post, or every
/// `own_every`th one the owner's.
fn line(i: usize, own_every: usize) -> OverlayDoc {
    let mut d = OverlayDoc::text(uuid::Uuid::new_v4().to_string(), "ri-agent".into(), "ri-agent".into(), format!("line {i}"), "thread");
    if own_every > 0 && i % own_every == 0 {
        d.author = Some(PostAuthor::owner());
    }
    d
}

/// Write `n` lines to a new conversation in one store write, with its
/// catalog row (head = n). Returns the conversation.
fn bulk_conversation(conn: &Connection, project: &str, n: usize, own_every: usize, with_meta: bool) -> (String, Vec<OverlayDoc>) {
    let c = conv();
    let docs: Vec<OverlayDoc> = (1..=n).map(|i| line(i, own_every)).collect();
    doc_meta_test::write_lines(&c, 1, &docs, with_meta).expect("write lines");
    conn.execute(
        "INSERT INTO overlay_conversations (conversation_id, project_id, last_thread_seq, last_chatter_seq) VALUES (?1, ?2, ?3, 0)",
        params![c, project, n as i64],
    )
    .expect("catalog");
    (c, docs)
}

fn assert_row_matches_doc(id: &str) {
    let doc = overlay::get_doc(id).expect("get").expect("doc exists");
    assert_eq!(doc_meta_test::row(id).expect("row"), Some(DocMeta::of(&doc)), "doc_meta of {id} vs its doc {doc:?}");
}

/// The whole read state of every person at two times, new path vs old,
/// plus each conversation's item.
fn assert_same_as_old(conn: &Connection, people: &[&Person], convs: &[String], nows: &[i64]) {
    for p in people {
        ensure_person(conn, &p.key).expect("seen");
        for &now in nows {
            let new = summary(conn, p, now).expect("summary");
            assert!(new.items.iter().all(|i| i.kind == KIND_THREAD), "fixture holds Threads only");
            assert_eq!(new, old::summary(conn, p, now), "summary for {} at {now}", p.key);
            for c in convs {
                assert_eq!(item(conn, p, KIND_THREAD, c, now).expect("item"), old::item(conn, p, c, now), "item {c} for {}", p.key);
            }
        }
    }
}

/// Counts equal the old parser's on a fixture of mixed authors, history,
/// cards (plain, permission at each floor, keep-as-mine, secret; pending,
/// answered, voided, expired, due), deleted posts, muted conversations,
/// partial reads and conversations at and over the cap; for the owner, a
/// Connect admin, a member, a plain login and an App guest.
#[test]
fn counts_match_the_old_parser_on_a_mixed_fixture() {
    let _counted = COUNTED.lock();
    let _home = crate::test_env::TempHome::new();
    crate::connect_users::add_user("rimaya", "a-long-password-1").expect("maya");
    crate::connect_users::set_role("rimaya", crate::connect_users::Role::Admin).expect("admin");
    crate::connect_users::add_user("ridana", "a-long-password-2").expect("dana");
    crate::connect_users::set_role("ridana", crate::connect_users::Role::Member).expect("member");
    let (g, project) = db();
    let h = g.handle();
    let conn = h.lock();
    let maya = Person::of_login(&people(), "rimaya");
    let dana = Person::of_login(&people(), "ridana");
    let alice = Person::of_login(&people(), "alice");
    let guest = Person::app_guest("guest:ri-principal", "riguest");
    let now = now_secs();
    let mut convs = Vec::new();

    // Authors and history.
    let c = conv();
    for i in 0..3 {
        agent_post(&conn, &c, &project, &format!("agent {i}"));
    }
    person_post(&conn, &c, &project, PostAuthor::owner(), "owner");
    person_post(&conn, &c, &project, PostAuthor::user("alice"), "alice");
    person_post(&conn, &c, &project, PostAuthor::user("rimaya"), "maya");
    person_post(&conn, &c, &project, PostAuthor::guest("riguest"), "guest");
    overlay::post_thread(&conn, &c, &project, "ri-agent", "ri-agent", "history", "import").expect("import via");
    k2_notice(&conn, &c, &project, overlay::SystemNotice::info("job.added"));
    k2_notice(&conn, &c, &project, overlay::SystemNotice::action("clone.proposed"));
    k2_notice(&conn, &c, &project, overlay::SystemNotice::delivery("delivery.not_taken.retry_failed"));
    let (gone, gone_seq) = person_post(&conn, &c, &project, PostAuthor::user("alice"), "sensitive");
    overlay::delete_thread_message(&c, &MessageRef::Seq(gone_seq), PostAuthor::user("alice"), |_| Ok(())).expect("delete");
    assert_row_matches_doc(&gone);
    convs.push(c);

    // Cards.
    let c = conv();
    ask(&conn, &c, &project, None);
    let answered = ask(&conn, &c, &project, None);
    overlay::answer_card(&c, &project, &answered, Some("Go"), None, Some(PostAuthor::owner())).expect("answer");
    let voided = ask(&conn, &c, &project, None);
    overlay::void_card(&c, &project, &voided, None).expect("void");
    let expired = ask(&conn, &c, &project, None);
    overlay::expire_card(&c, &expired).expect("expire");
    ask(&conn, &c, &project, Some(now + 60));
    overlay::post_secret(&conn, &c, &project, "ri-agent", "ri-agent", "API_KEY", None, SecretProposal::default(), None).expect("secret");
    let (set, _) = overlay::post_secret(&conn, &c, &project, "ri-agent", "ri-agent", "OTHER_KEY", None, SecretProposal::default(), None)
        .expect("secret 2");
    overlay::mark_secret(&c, &set.id, "set", Some("vault:x".into()), "secret set".into(), None).expect("set");
    agent_post(&conn, &c, &project, "after the cards");
    convs.push(c);

    // Action cards at each floor.
    let c = conv();
    action_card(&conn, &c, &project, crate::permission::CARD_ACTION_PERMISSION, Some("admin"));
    action_card(&conn, &c, &project, crate::permission::CARD_ACTION_PERMISSION, Some("owner"));
    action_card(&conn, &c, &project, crate::permission::CARD_ACTION_PERMISSION, None);
    action_card(&conn, &c, &project, crate::permission::CARD_ACTION_BATCH, Some("admin"));
    action_card(&conn, &c, &project, "keep-as-mine", None);
    convs.push(c);

    // Muted with unread; muted and read; partly read.
    let muted = conv();
    for i in 0..4 {
        agent_post(&conn, &muted, &project, &format!("m{i}"));
    }
    ask(&conn, &muted, &project, None);
    let quiet = conv();
    agent_post(&conn, &quiet, &project, "q");
    let partial = conv();
    for i in 0..6 {
        agent_post(&conn, &partial, &project, &format!("p{i}"));
    }
    convs.extend([muted.clone(), quiet.clone(), partial.clone()]);

    // At and over the cap: 101 lines (100 past a cursor at 1), 105 lines.
    let (at_cap, _) = bulk_conversation(&conn, &project, 101, 7, true);
    let (over_cap, _) = bulk_conversation(&conn, &project, 105, 0, true);
    convs.extend([at_cap.clone(), over_cap.clone()]);

    let everyone = [&owner(), &maya, &dana, &alice, &guest];
    for p in everyone {
        ensure_person(&conn, &p.key).expect("seen");
        set_mute(&conn, &p.key, KIND_THREAD, &muted, true, None).expect("mute");
        set_mute(&conn, &p.key, KIND_THREAD, &quiet, true, Some(now + 90)).expect("timed mute");
        ack(&conn, &p.key, KIND_THREAD, &quiet, None).expect("read quiet");
        ack(&conn, &p.key, KIND_THREAD, &partial, Some(4)).expect("read part");
        ack(&conn, &p.key, KIND_THREAD, &at_cap, Some(1)).expect("read one");
    }

    assert_same_as_old(&conn, &everyone, &convs, &[now, now + 120]);
    let s = summary(&conn, &owner(), now).expect("summary");
    let cap_item = s.items.iter().find(|i| i.id == at_cap).expect("at cap listed");
    assert!(cap_item.unread > 0 && cap_item.unread < UNREAD_CAP, "100 past the cursor, the owner's own skipped: {cap_item:?}");
    assert_eq!(s.items.iter().find(|i| i.id == over_cap).expect("over cap").unread, UNREAD_CAP);
    assert!(s.totals.waiting_total > 0 && s.items.iter().any(|i| i.muted), "the fixture exercises waiting and mute: {s:?}");
}

/// The PRD's fixture: 50 conversations of 100 unread each. The old path
/// parses all 5,000 docs; the new one parses none and walks one pointer
/// range per conversation, with equal counts.
#[test]
fn a_summary_of_fifty_conversations_parses_no_document() {
    let _counted = COUNTED.lock();
    let (g, project) = db();
    let h = g.handle();
    let conn = h.lock();
    for _ in 0..50 {
        bulk_conversation(&conn, &project, 100, 10, true);
    }
    let now = now_secs();
    ensure_person(&conn, OWNER).expect("owner");

    read_counts::take();
    let old = old::summary(&conn, &owner(), now);
    let old_reads = read_counts::take();
    let new = summary(&conn, &owner(), now).expect("summary");
    let new_reads = read_counts::take();

    assert_eq!(new, old);
    assert_eq!(new.items.len(), 50);
    assert!(new.items.iter().all(|i| i.unread == 90), "the owner's every 10th line is their own: {:?}", new.items[0]);
    assert!(old_reads.doc_parses >= 5000, "the old path parsed every unread doc: {old_reads:?}");
    assert_eq!(new_reads.doc_parses, 0, "{new_reads:?}");
    assert_eq!(new_reads.pointer_ranges, 50, "one pointer range per conversation: {new_reads:?}");
}

/// What a count reads depends on the lines past the cursor, never on how
/// long the read history before it is.
#[test]
fn reads_stay_constant_as_history_grows() {
    let _counted = COUNTED.lock();
    let mut seen = Vec::new();
    for history in [10usize, 2000] {
        let (g, project) = db();
        let h = g.handle();
        let conn = h.lock();
        let (c, _) = bulk_conversation(&conn, &project, history + 5, 0, true);
        ensure_person(&conn, OWNER).expect("owner");
        ack(&conn, OWNER, KIND_THREAD, &c, Some(history as i64)).expect("read the history");
        let now = now_secs();
        read_counts::take();
        let it = item(&conn, &owner(), KIND_THREAD, &c, now).expect("item");
        let item_reads = read_counts::take();
        let s = summary(&conn, &owner(), now).expect("summary");
        let summary_reads = read_counts::take();
        assert_eq!(it.unread, 5);
        assert_eq!(s.totals.total, 5);
        assert_eq!(item_reads.doc_parses + summary_reads.doc_parses, 0, "{item_reads:?} {summary_reads:?}");
        seen.push((item_reads.meta_gets, item_reads.pointer_ranges, summary_reads.pointer_ranges));
    }
    assert_eq!(seen[0], seen[1], "10 vs 2000 lines of history read the same");
    assert_eq!(seen[0], (5, 1, 1), "five facts, one range");

    // Over the cap, no fact is read at all.
    let (g, project) = db();
    let h = g.handle();
    let conn = h.lock();
    let (c, _) = bulk_conversation(&conn, &project, 400, 0, true);
    read_counts::take();
    assert_eq!(item(&conn, &owner(), KIND_THREAD, &c, now_secs()).expect("item").unread, UNREAD_CAP);
    let r = read_counts::take();
    assert_eq!((r.doc_parses, r.meta_gets, r.pointer_ranges), (0, 0, 1), "{r:?}");
}

/// Every write keeps the row equal to the doc it describes: a new post,
/// a read (writes nothing), an edit in place (answer, void, expire, a
/// secret set, a collected line updated), a delete, an import,
/// and a move or split (pointers renumbered, rows untouched). Counts stay
/// the old path's and parse nothing.
#[test]
fn every_write_keeps_the_index_exact() {
    let _counted = COUNTED.lock();
    let (g, project) = db();
    let h = g.handle();
    let conn = h.lock();
    let alice = Person::of_login(&people(), "alice");
    ensure_person(&conn, OWNER).expect("owner");
    ensure_person(&conn, "alice").expect("alice");
    let c = conv();

    // New.
    let s1 = agent_post(&conn, &c, &project, "new");
    let first = overlay::store::thread_id_at(&c, s1).expect("id").expect("posted");
    assert_row_matches_doc(&first);
    let (mine, mine_seq) = person_post(&conn, &c, &project, PostAuthor::user("alice"), "mine");
    assert_row_matches_doc(&mine);
    let card = ask(&conn, &c, &project, None);
    let secret = overlay::post_secret(&conn, &c, &project, "ri-agent", "ri-agent", "API_KEY", None, SecretProposal::default(), None)
        .expect("secret")
        .0
        .id;
    let to_void = ask(&conn, &c, &project, None);
    let to_expire = ask(&conn, &c, &project, Some(now_secs() + 3600));
    for id in [&card, &secret, &to_void, &to_expire] {
        assert_row_matches_doc(id);
        assert!(doc_meta_test::row(id).expect("row").expect("present").waiting_card, "{id} waits");
    }
    let convs = [c.clone()];
    let now = now_secs();
    assert_same_as_old(&conn, &[&owner(), &alice], &convs, &[now]);

    // Read: a cursor move writes no doc and no row.
    let before = doc_meta_test::row(&card).expect("row");
    ack(&conn, OWNER, KIND_THREAD, &c, Some(2)).expect("ack");
    assert_eq!(doc_meta_test::row(&card).expect("row"), before);
    assert_same_as_old(&conn, &[&owner(), &alice], &convs, &[now]);

    // Edit in place.
    overlay::answer_card(&c, &project, &card, Some("Go"), None, Some(PostAuthor::owner())).expect("answer");
    overlay::void_card(&c, &project, &to_void, None).expect("void");
    overlay::expire_card(&c, &to_expire).expect("expire");
    overlay::mark_secret(&c, &secret, "set", Some("vault:x".into()), "secret set".into(), None).expect("set");
    for id in [&card, &secret, &to_void, &to_expire] {
        assert_row_matches_doc(id);
        assert!(!doc_meta_test::row(id).expect("row").expect("present").waiting_card, "{id} closed");
    }
    assert_same_as_old(&conn, &[&owner(), &alice], &convs, &[now]);

    // Delete: the tombstone keeps its author, so it counts as before.
    overlay::delete_thread_message(&c, &MessageRef::Seq(mine_seq), PostAuthor::user("alice"), |_| Ok(())).expect("delete");
    assert_row_matches_doc(&mine);
    assert_same_as_old(&conn, &[&owner(), &alice], &convs, &[now]);

    // A collected line appended, then updated in place, in one write.
    let collected = format!("gb:ri-{}", uuid::Uuid::new_v4());
    let mut ext = OverlayDoc::text(uuid::Uuid::new_v4().to_string(), "bot".into(), "bot".into(), "draft".into(), "collected");
    ext.external = Some(overlay::CollectedMeta { source: "bot".into(), entry_id: "e1".into(), ..Default::default() });
    overlay::collected_write(&collected, |x| {
        x.append(&ext, &["e1"])?;
        let mut edited = ext.clone();
        edited.body = Some("final".into());
        x.put_doc(&edited)
    })
    .expect("collected");
    assert_row_matches_doc(&ext.id);
    assert!(doc_meta_test::row(&ext.id).expect("row").expect("present").history);

    // An import.
    let imported_conv = conv();
    let imp = OverlayDoc::text(uuid::Uuid::new_v4().to_string(), "ri-agent".into(), "ri-agent".into(), "old".into(), "import");
    store::import_docs(&imported_conv, 0, &[(format!("ri/{}", imp.id), imp.clone())], None, |_| Ok(())).expect("import");
    assert_row_matches_doc(&imp.id);

    // Move and split renumber pointers; rows follow the doc id, untouched.
    let moved = conv();
    overlay::move_conversation(&conn, &c, &moved).expect("move").expect("moved");
    let split = conv();
    let take = |d: &OverlayDoc| d.kind == "choice";
    overlay::split_conversation(&conn, &moved, &split, &project, &take).expect("split").expect("split moved");
    let convs = [moved.clone(), split.clone()];
    read_counts::take();
    assert_same_as_old(&conn, &[&owner(), &alice], &convs, &[now]);
    for id in [&first, &mine, &card, &secret, &to_void, &to_expire] {
        assert_row_matches_doc(id);
    }
    read_counts::take();
    summary(&conn, &owner(), now).expect("summary");
    item(&conn, &alice, KIND_THREAD, &moved, now).expect("item");
    assert_eq!(read_counts::take().doc_parses, 0);
}

/// Upgrade: docs written before `doc_meta` have no row. Until the
/// backfill reaches them they are read the old way (exact counts); the
/// backfill, resumed over small batches, gives each one its row; after
/// it a summary parses nothing and counts the same.
#[test]
fn the_upgrade_backfill_fills_every_row_and_counts_never_change() {
    let _counted = COUNTED.lock();
    let (g, project) = db();
    let h = g.handle();
    let conn = h.lock();
    ensure_person(&conn, OWNER).expect("owner");
    let mut convs = Vec::new();
    let mut legacy: Vec<OverlayDoc> = Vec::new();
    for n in [3usize, 40, 120] {
        let (c, docs) = bulk_conversation(&conn, &project, n, 3, false);
        convs.push(c);
        legacy.extend(docs);
    }
    // A card waiting from before the index, and a card answered since.
    let (cards, mut docs) = bulk_conversation(&conn, &project, 0, 0, false);
    let waiting_id = ask(&conn, &cards, &project, None);
    let answered_id = ask(&conn, &cards, &project, None);
    overlay::answer_card(&cards, &project, &answered_id, Some("Go"), None, Some(PostAuthor::owner())).expect("answer");
    doc_meta_test::forget(&[&waiting_id, &answered_id]).expect("as an older build wrote them");
    docs.push(overlay::get_doc(&waiting_id).expect("get").expect("card"));
    docs.push(overlay::get_doc(&answered_id).expect("get").expect("card"));
    legacy.extend(docs);
    convs.push(cards);
    for d in &legacy {
        assert_eq!(doc_meta_test::row(&d.id).expect("row"), None, "no row before the backfill");
    }

    let now = now_secs();
    read_counts::take();
    let before = summary(&conn, &owner(), now).expect("summary before");
    let fallback = read_counts::take();
    assert_eq!(before, old::summary(&conn, &owner(), now), "exact before the backfill");
    // 3 + 40 + 120 lines (the last over the cap: not read) + 2 cards.
    assert!(fallback.doc_parses >= 3 + 40 + 2, "row-less docs are read the old way: {fallback:?}");
    assert_eq!(before.totals.waiting_total, 1);

    doc_meta_test::restart_backfill().expect("restart");
    let mut steps = 0;
    while !store::backfill_doc_meta_step(37).expect("step") {
        steps += 1;
        assert!(steps < 1_000_000, "the backfill must finish");
    }
    assert!(steps > 0, "resumed over several batches");
    assert!(store::backfill_doc_meta_step(37).expect("again"), "done stays done");
    for d in &legacy {
        assert_row_matches_doc(&d.id);
    }

    read_counts::take();
    let after = summary(&conn, &owner(), now).expect("summary after");
    assert_eq!(read_counts::take().doc_parses, 0, "every row is there now");
    assert_eq!(after, before);
    assert_same_as_old(&conn, &[&owner()], &convs, &[now, now + 7 * 24 * 3600 + 60]);
}

/// 0.46.18: K2's info notices never count as unread; its action and
/// delivery notices, a later kind and a K2 line without `systemNotice` do.
/// The row says which, so a count still parses no document; a row an
/// older build wrote for a K2 line is read from its doc (an info notice
/// stays quiet, one without `systemNotice` counts: the safe default).
#[test]
fn info_notices_never_count_and_old_rows_read_their_doc() {
    let _counted = COUNTED.lock();
    let (g, project) = db();
    let h = g.handle();
    let conn = h.lock();
    ensure_person(&conn, OWNER).expect("owner");
    let c = conv();
    let unread = || item(&conn, &owner(), KIND_THREAD, &c, now_secs()).expect("item").unread;
    let listed = || summary(&conn, &owner(), now_secs()).expect("summary").items.iter().find(|i| i.id == c).map_or(0, |i| i.unread);

    let info = k2_notice(&conn, &c, &project, overlay::SystemNotice::info("job.added"));
    assert_row_matches_doc(&info);
    assert_eq!(doc_meta_test::row(&info).expect("row").expect("present").quiet_notice, Some(true));
    assert_eq!((unread(), listed()), (0, 0), "an info notice is never unread");
    let action = k2_notice(&conn, &c, &project, overlay::SystemNotice::action("clone.proposed"));
    assert_eq!((unread(), listed()), (1, 1), "an action notice counts");
    let delivery = k2_notice(&conn, &c, &project, overlay::SystemNotice::delivery("delivery.not_taken.never_read_in"));
    assert_eq!((unread(), listed()), (2, 2), "a delivery notice counts");
    let later = k2_notice(&conn, &c, &project, overlay::SystemNotice::new("later", "x.y"));
    assert_eq!((unread(), listed()), (3, 3), "a kind this build doesn't know counts");
    k2_notice(&conn, &c, &project, overlay::SystemNotice::info("reminder.fired"));
    assert_eq!((unread(), listed()), (3, 3));
    // VIP mail is info on the wire and still counts (the person's important
    // mail); a changed-files card is quiet.
    k2_notice(&conn, &c, &project, overlay::SystemNotice::info(overlay::doc::NOTICE_CODE_MAIL_VIP));
    assert_eq!((unread(), listed()), (4, 4), "a VIP mail notice counts");
    k2_notice(&conn, &c, &project, overlay::SystemNotice::info(overlay::doc::NOTICE_CODE_TURN_CHANGES));
    assert_eq!((unread(), listed()), (4, 4), "a changed-files card doesn't");
    for id in [&action, &delivery, &later] {
        assert_row_matches_doc(id);
        assert_eq!(doc_meta_test::row(id).expect("row").expect("present").quiet_notice, Some(false), "{id}");
    }
    read_counts::take();
    assert_eq!(unread(), 4);
    assert_eq!(read_counts::take().doc_parses, 0, "the row says which notice it is");

    // Rows an older build wrote (no notice flag): the doc answers.
    let bare = format!("ri-bare-{}", uuid::Uuid::new_v4());
    let mut old_line = OverlayDoc::text(bare.clone(), "k2".into(), "ri-agent".into(), "K2: old line".into(), "k2");
    old_line.author = Some(PostAuthor::system());
    let head = catalog_head(&conn, &c);
    doc_meta_test::write_lines(&c, head + 1, std::slice::from_ref(&old_line), false).expect("old line");
    conn.execute("UPDATE overlay_conversations SET last_thread_seq = ?2 WHERE conversation_id = ?1", params![c, head + 1])
        .expect("head");
    let v1 = "1\tu\t\t\tsystem\tk2";
    for id in [info.as_str(), action.as_str(), bare.as_str()] {
        doc_meta_test::put_raw(id, v1).expect("v1 row");
    }
    read_counts::take();
    assert_eq!(
        (unread(), listed()),
        (5, 5),
        "an old row of a K2 line without systemNotice counts; the old info row stays quiet; the old action row counts"
    );
    assert!(read_counts::take().doc_parses > 0, "old K2 rows are read from the doc");
    assert_same_as_old(&conn, &[&owner()], std::slice::from_ref(&c), &[now_secs()]);
}

fn catalog_head(conn: &Connection, conv: &str) -> i64 {
    conn.query_row("SELECT last_thread_seq FROM overlay_conversations WHERE conversation_id = ?1", params![conv], |r| r.get(0))
        .expect("head")
}
