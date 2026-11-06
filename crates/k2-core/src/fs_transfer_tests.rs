use super::*;

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos();
        let p = std::env::temp_dir().join(format!("k2-transfer-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&p).expect("mkdir tmp");
        Tmp(dunce::canonicalize(&p).expect("canon tmp"))
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Box3 {
    t: Tmp,
    store: PathBuf,
    ledger: PathBuf,
    room: PathBuf,
}

fn setup(tag: &str) -> Box3 {
    let t = Tmp::new(tag);
    let store = t.0.join("uploads");
    let ledger = t.0.join("ledger");
    let room = t.0.join("room");
    fs::create_dir_all(room.join("in")).expect("room");
    Box3 { store, ledger, room, t }
}

fn holder(who: &str) -> Holder {
    Holder { principal: format!("principal:{who}"), pass_id: Some(format!("pass-{who}")), username: who.into() }
}

fn new_upload(b: &Box3, who: &str, size: u64, sha: Option<&str>) -> NewUpload {
    NewUpload {
        holder: holder(who),
        project_id: "room-1".into(),
        root: b.room.clone(),
        dir: b.room.join("in"),
        rel_dir: "in".into(),
        filename: "big.bin".into(),
        size,
        sha256: sha.map(str::to_string),
        drop: false,
        token_ttl_secs: 600,
    }
}

fn sha(bytes: &[u8]) -> String {
    super::sha256_hex(bytes)
}

const NOW: u64 = 1_800_000_000;

#[test]
fn range_set_merges_and_reports_gaps() {
    let mut r = RangeSet::default();
    r.insert(10, 20);
    r.insert(30, 40);
    assert_eq!(r.0, vec![(10, 20), (30, 40)]);
    assert_eq!(r.missing(50), vec![(0, 10), (20, 30), (40, 50)]);
    r.insert(20, 30); // touching on both sides
    assert_eq!(r.0, vec![(10, 40)]);
    r.insert(0, 15); // overlapping
    assert_eq!(r.0, vec![(0, 40)]);
    r.insert(5, 6); // inside
    assert_eq!(r.0, vec![(0, 40)]);
    assert_eq!(r.total(), 40);
    assert!(!r.complete(50));
    r.insert(40, 50);
    assert!(r.complete(50));
    assert_eq!(r.missing(50), Vec::<(u64, u64)>::new());
    assert_eq!(RangeSet::inclusive(&[(0, 10), (20, 21)]), vec![[0, 9], [20, 20]]);
    assert_eq!(RangeSet::header(&[(0, 10), (20, 21)]), "0-9,20-20");
    assert_eq!(RangeSet::header(&[]), "");
    assert!(RangeSet::default().complete(0), "an empty file is complete");
    let mut f = RangeSet::default();
    f.insert(0, 10);
    assert!(!f.would_fragment(10, 12), "touching merges");
    assert!(f.would_fragment(11, 12));
}

#[test]
fn tokens_have_a_shape_and_only_their_own_session() {
    let (tok, hash) = mint_token(UPLOAD_TOKEN_PREFIX, "0123456789abcdef0123456789abcdef").expect("mint");
    assert!(tok.starts_with("k2up_0123456789abcdef0123456789abcdef."), "{tok}");
    assert_eq!(token_id(UPLOAD_TOKEN_PREFIX, &tok), Some("0123456789abcdef0123456789abcdef"));
    assert_eq!(token_id(DOWNLOAD_TOKEN_PREFIX, &tok), None, "an upload token is never a download token");
    assert!(hash_matches(&hash, &tok));
    assert!(!hash_matches(&hash, &format!("{tok}x")));
    assert_eq!(token_id(UPLOAD_TOKEN_PREFIX, "k2up_nothex.abc"), None);
    assert_eq!(token_id(UPLOAD_TOKEN_PREFIX, "k2skn_whatever"), None);
    assert_eq!(clamp_ttl(None, TOKEN_TTL_DEFAULT_SECS), TOKEN_TTL_DEFAULT_SECS);
    assert_eq!(clamp_ttl(Some(1), 600), TOKEN_TTL_MIN_SECS);
    assert_eq!(clamp_ttl(Some(10 * 86_400), 600), TOKEN_TTL_MAX_SECS);
}

#[test]
fn out_of_order_ranges_complete_and_publish() {
    let b = setup("order");
    let data: Vec<u8> = (0..100u8).collect();
    let (s, tok) = create_upload(&b.store, &b.ledger, new_upload(&b, "alice", 100, Some(&sha(&data))), NOW).expect("create");
    assert!(b.room.join("in").join(&s.part).is_file(), "the empty part exists");
    assert!(gu::is_guest_part_name(&s.part), "{}", s.part);
    assert_eq!(authorize_upload(&b.store, &s.id, &tok, NOW).expect("auth").id, s.id);
    // Last range first, then the first, then the middle twice (idempotent).
    write_range(&b.store, &b.ledger, &s.id, 60, 100, &data[60..], None, NOW).expect("r3");
    let st = write_range(&b.store, &b.ledger, &s.id, 0, 100, &data[..30], Some(&sha(&data[..30])), NOW).expect("r1");
    assert_eq!(st.received.missing(100), vec![(30, 60)]);
    assert_eq!(st.status_json()["missing"], serde_json::json!([[30, 59]]));
    assert_eq!(
        complete_upload(&b.store, &b.ledger, &s.id, None, NOW),
        Err(TransferError::Incomplete { missing: vec![(30, 60)] })
    );
    write_range(&b.store, &b.ledger, &s.id, 30, 100, &data[30..60], None, NOW).expect("r2");
    write_range(&b.store, &b.ledger, &s.id, 30, 100, &data[30..60], None, NOW).expect("r2 again");
    let done = complete_upload(&b.store, &b.ledger, &s.id, Some(&sha(&data)), NOW).expect("complete");
    assert_eq!(done.rel_path, "in/big.bin");
    assert_eq!(fs::read(b.room.join("in/big.bin")).expect("published"), data);
    assert!(!b.room.join("in").join(&s.part).exists(), "part gone");
    // Idempotent; and the token still opens it to ask.
    assert_eq!(complete_upload(&b.store, &b.ledger, &s.id, None, NOW).expect("again"), done);
    assert!(authorize_upload(&b.store, &s.id, &tok, NOW).is_ok());
    assert_eq!(
        write_range(&b.store, &b.ledger, &s.id, 0, 100, &data[..1], None, NOW).expect_err("after done"),
        TransferError::AlreadyDone
    );
}

#[test]
fn bad_ranges_and_checksums_are_refused() {
    let b = setup("bad");
    let (s, _tok) = create_upload(&b.store, &b.ledger, new_upload(&b, "alice", 10, None), NOW).expect("create");
    for (start, total, bytes) in [(8u64, 10u64, &b"abc"[..]), (0, 11, &b"a"[..]), (0, 10, &b""[..]), (u64::MAX, 10, &b"a"[..])] {
        assert!(
            matches!(write_range(&b.store, &b.ledger, &s.id, start, total, bytes, None, NOW), Err(TransferError::BadRange(_))),
            "{start} {total} {}",
            bytes.len()
        );
    }
    let err = write_range(&b.store, &b.ledger, &s.id, 0, 10, b"0123456789", Some(&sha(b"other")), NOW).expect_err("range sha");
    assert!(matches!(err, TransferError::RangeChecksum { .. }), "{err:?}");
    assert_eq!(fs::metadata(b.room.join("in").join(&s.part)).expect("part").len(), 0, "nothing written");
    // A whole-file mismatch drops every range.
    write_range(&b.store, &b.ledger, &s.id, 0, 10, b"0123456789", None, NOW).expect("write");
    let err = complete_upload(&b.store, &b.ledger, &s.id, Some(&sha(b"different")), NOW).expect_err("file sha");
    assert!(matches!(err, TransferError::ChecksumMismatch { .. }), "{err:?}");
    let after = load_session(&b.store, &s.id).expect("load").expect("there");
    assert_eq!(after.received.total(), 0, "resend everything");
    assert!(!b.room.join("in/big.bin").exists(), "nothing published");
}

#[test]
fn the_declared_sha_is_enforced_and_must_agree() {
    let b = setup("declared");
    let (s, _) = create_upload(&b.store, &b.ledger, new_upload(&b, "alice", 3, Some(&sha(b"abc"))), NOW).expect("create");
    write_range(&b.store, &b.ledger, &s.id, 0, 3, b"abd", None, NOW).expect("write");
    assert!(matches!(complete_upload(&b.store, &b.ledger, &s.id, None, NOW), Err(TransferError::ChecksumMismatch { .. })));
    write_range(&b.store, &b.ledger, &s.id, 0, 3, b"abc", None, NOW).expect("rewrite");
    assert!(matches!(
        complete_upload(&b.store, &b.ledger, &s.id, Some(&sha(b"zzz")), NOW),
        Err(TransferError::Invalid(_))
    ));
    complete_upload(&b.store, &b.ledger, &s.id, None, NOW).expect("complete");
    assert_eq!(fs::read(b.room.join("in/big.bin")).expect("file"), b"abc");
}

#[test]
fn tokens_open_only_their_session_until_they_expire() {
    let b = setup("tokens");
    let (a, tok_a) = create_upload(&b.store, &b.ledger, new_upload(&b, "alice", 5, None), NOW).expect("a");
    let (other, tok_b) = create_upload(&b.store, &b.ledger, new_upload(&b, "bob", 5, None), NOW).expect("b");
    assert_eq!(authorize_upload(&b.store, &other.id, &tok_a, NOW).expect_err("wrong session"), TransferError::Unauthorized);
    assert_eq!(authorize_upload(&b.store, &a.id, &tok_b, NOW).expect_err("wrong token"), TransferError::Unauthorized);
    assert_eq!(authorize_upload(&b.store, &a.id, "k2up_garbage", NOW).expect_err("garbage"), TransferError::Unauthorized);
    // The token expires after its TTL (600 s here); a fresh one works.
    assert_eq!(authorize_upload(&b.store, &a.id, &tok_a, NOW + 601).expect_err("old token"), TransferError::TokenExpired);
    let (fresh, exp) = add_token(&b.store, &a.id, 3600, NOW + 601).expect("fresh");
    assert_eq!(exp, NOW + 601 + 3600);
    assert!(authorize_upload(&b.store, &a.id, &fresh, NOW + 700).is_ok());
    // The session itself expires after SESSION_TTL, token or not.
    let late = NOW + SESSION_TTL.as_secs();
    let (late_tok, _) = add_token(&b.store, &a.id, 3600, late - 10).expect("late token");
    assert_eq!(authorize_upload(&b.store, &a.id, &late_tok, late).expect_err("session over"), TransferError::Expired);
    assert_eq!(session_of(&b.store, &a.id, "principal:bob", NOW).expect_err("not bob's"), TransferError::Unauthorized);
}

#[test]
fn a_missing_part_restarts_the_session_and_abort_cleans_up() {
    let b = setup("gone");
    let (s, _) = create_upload(&b.store, &b.ledger, new_upload(&b, "alice", 6, None), NOW).expect("create");
    write_range(&b.store, &b.ledger, &s.id, 0, 6, b"abc", None, NOW).expect("half");
    fs::remove_file(b.room.join("in").join(&s.part)).expect("swept away");
    let st = write_range(&b.store, &b.ledger, &s.id, 3, 6, b"def", None, NOW).expect("second half");
    assert_eq!(st.received.0, vec![(3, 6)], "what the part held is gone");
    abort_upload(&b.store, &b.ledger, &s.id).expect("abort");
    assert!(!b.room.join("in").join(&s.part).exists());
    assert_eq!(load_session(&b.store, &s.id).expect("load"), None);
    assert_eq!(abort_upload(&b.store, &b.ledger, &s.id).expect_err("again"), TransferError::Unauthorized);
}

#[cfg(unix)]
#[test]
fn a_symlink_or_hard_link_at_the_part_is_never_written() {
    let b = setup("links");
    let (s, _) = create_upload(&b.store, &b.ledger, new_upload(&b, "alice", 4, None), NOW).expect("create");
    let part = b.room.join("in").join(&s.part);
    let victim = b.t.0.join("victim.txt");
    fs::write(&victim, b"keep").expect("victim");
    fs::remove_file(&part).expect("rm part");
    std::os::unix::fs::symlink(&victim, &part).expect("symlink");
    assert!(matches!(write_range(&b.store, &b.ledger, &s.id, 0, 4, b"EVIL", None, NOW), Err(TransferError::Invalid(_))));
    fs::remove_file(&part).expect("rm link");
    fs::hard_link(&victim, &part).expect("hard link");
    assert!(matches!(write_range(&b.store, &b.ledger, &s.id, 0, 4, b"EVIL", None, NOW), Err(TransferError::Invalid(_))));
    assert_eq!(fs::read(&victim).expect("victim"), b"keep");
}

#[test]
fn limits_size_and_live_sessions() {
    let b = setup("limits");
    assert!(matches!(
        create_upload(&b.store, &b.ledger, new_upload(&b, "alice", MAX_TRANSFER_SIZE + 1, None), NOW),
        Err(TransferError::TooLarge(_))
    ));
    for _ in 0..MAX_LIVE_UPLOADS_PER_HOLDER {
        create_upload(&b.store, &b.ledger, new_upload(&b, "alice", 1, None), NOW).expect("one");
    }
    assert!(matches!(
        create_upload(&b.store, &b.ledger, new_upload(&b, "alice", 1, None), NOW),
        Err(TransferError::TooMany(_))
    ));
    create_upload(&b.store, &b.ledger, new_upload(&b, "bob", 1, None), NOW).expect("bob is not alice");
    let (s, _) = create_upload(&b.store, &b.ledger, new_upload(&b, "carol", 20, None), NOW).expect("carol");
    let big = vec![0u8; (MAX_RANGE_LEN + 1) as usize];
    assert!(matches!(write_range(&b.store, &b.ledger, &s.id, 0, 20, &big, None, NOW), Err(TransferError::TooLarge(_))));
}

#[test]
fn sweep_removes_expired_sessions_parts_and_grants() {
    let b = setup("sweep");
    let downloads = b.t.0.join("downloads");
    let (old, _) = create_upload(&b.store, &b.ledger, new_upload(&b, "alice", 4, None), NOW).expect("old");
    let (fresh, _) = create_upload(&b.store, &b.ledger, new_upload(&b, "alice", 4, None), NOW + 100).expect("fresh");
    write_range(&b.store, &b.ledger, &old.id, 0, 4, b"ab", None, NOW).ok();
    let file = b.room.join("in/x.txt");
    fs::write(&file, b"x").expect("file");
    let (g, tok) = mint_download(&downloads, holder("alice"), file.clone(), "x.txt".into(), "text/plain".into(), Recheck::Person, 60, NOW)
        .expect("grant");
    assert_eq!(authorize_download(&downloads, &g.id, &tok, NOW + 10).expect("grant ok").path, file);
    assert_eq!(authorize_download(&downloads, &g.id, &tok, NOW + 60).expect_err("expired"), TransferError::Expired);
    let at = NOW + SESSION_TTL.as_secs() + 1;
    let r = sweep_expired(&b.store, &downloads, &b.ledger, at).expect("sweep");
    assert_eq!(r, SweepReport { uploads: 1, downloads: 1 });
    assert!(!b.room.join("in").join(&old.part).exists(), "expired part removed");
    assert!(b.room.join("in").join(&fresh.part).exists(), "live part kept");
    assert!(load_session(&b.store, &fresh.id).expect("load").is_some());
    assert!(file.exists(), "a grant's file is never touched");
}

#[test]
fn http_pieces() {
    assert_eq!(parse_content_range("bytes 0-99/1000"), Ok((0, 99, 1000)));
    assert_eq!(parse_content_range(" bytes 999-999/1000 "), Ok((999, 999, 1000)));
    for bad in ["bytes 0-1000/1000", "bytes 5-4/10", "bytes */10", "0-1/2", "bytes 0-1", "bytes a-b/c"] {
        assert!(parse_content_range(bad).is_err(), "{bad}");
    }
    assert_eq!(parse_range("bytes=0-9", 100), RangeAsk::Part(0, 9));
    assert_eq!(parse_range("bytes=90-", 100), RangeAsk::Part(90, 99));
    assert_eq!(parse_range("bytes=90-500", 100), RangeAsk::Part(90, 99));
    assert_eq!(parse_range("bytes=-10", 100), RangeAsk::Part(90, 99));
    assert_eq!(parse_range("bytes=-500", 100), RangeAsk::Part(0, 99));
    assert_eq!(parse_range("bytes=100-", 100), RangeAsk::Unsatisfiable);
    assert_eq!(parse_range("bytes=0-1,5-6", 100), RangeAsk::Full);
    assert_eq!(parse_range("items=0-1", 100), RangeAsk::Full);
    assert_eq!(parse_range("bytes=5-1", 100), RangeAsk::Full);
    assert!(if_range_allows(None, "\"a\""));
    assert!(if_range_allows(Some("\"a\""), "\"a\""));
    assert!(!if_range_allows(Some("\"b\""), "\"a\""));
    assert!(!if_range_allows(Some("Wed, 21 Oct 2015 07:28:00 GMT"), "\"a\""));
    assert_eq!(parse_sha256(Some(&"AB".repeat(32))), Ok(Some("ab".repeat(32))));
    assert_eq!(parse_sha256(Some("  ")), Ok(None));
    assert!(parse_sha256(Some("xyz")).is_err());
}

#[test]
fn the_etag_follows_the_file() {
    let t = Tmp::new("etag");
    let p = t.0.join("f.txt");
    fs::write(&p, b"one").expect("write");
    let a = file_etag(&fs::metadata(&p).expect("m"));
    assert!(a.starts_with('"') && a.ends_with('"') && a.len() == 26, "{a}");
    assert_eq!(a, file_etag(&fs::metadata(&p).expect("m")), "stable");
    fs::write(&p, b"two!").expect("rewrite");
    assert_ne!(a, file_etag(&fs::metadata(&p).expect("m")));
}
