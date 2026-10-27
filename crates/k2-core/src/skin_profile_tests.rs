//! App user profile (core): the store, settings, names, pictures and
//! sessions (P1/P2/P5), and the P0 fixes on the live password paths.

use super::*;
use crate::skin::{
    add_principal_for_app, app_login, check_and_record_login, consume_password_reset, create_app_session_token,
    create_grant, create_role_for_app, invites, mint_password_reset, resolve_skin_token, set_principal_email,
    set_principal_full_name_as, set_principal_password, set_principal_password_staying, RoomPolicy, SkinGrantWrite,
    SkinLoginOutcome,
};
use crate::workspace::agent_look;

struct Fx {
    _home: crate::test_env::TempHome,
    _db: crate::db::TestDbGuard,
    app: String,
    other_app: String,
    pid: String,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_env::TempHome::new();
    let db = crate::db::scoped_for_test();
    let room = format!("room-{tag}");
    let other_room = format!("other-{tag}");
    let (app, other_app) = {
        let h = crate::db::shared();
        let conn = h.lock();
        for r in [&room, &other_room] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", params![r, format!("/tmp/pf-{r}")])
                .unwrap();
        }
        let a = crate::published_services::insert(&conn, &room, "jobs", "node s.js", "/tmp", 4300, "local", "running", "cmd", "").unwrap();
        let o = crate::published_services::insert(&conn, &other_room, "crm", "node s.js", "/tmp", 4301, "local", "running", "cmd", "")
            .unwrap();
        (a.id, o.id)
    };
    let mut policy = RoomPolicy::new();
    policy.insert(room.clone(), vec!["thread:read".into()]);
    create_role_for_app(&format!("r-{tag}"), &policy, Some(&app)).unwrap();
    let mut other = RoomPolicy::new();
    other.insert(other_room.clone(), vec!["thread:read".into()]);
    create_role_for_app(&format!("o-{tag}"), &other, Some(&other_app)).unwrap();
    let p = add_principal_for_app("dana", Some("dana@example.test"), &app, Some(&format!("r-{tag}")), "test").unwrap();
    set_principal_password("dana", Some("s3cret-horse")).unwrap();
    Fx { _home: home, _db: db, app, other_app, pid: p.id }
}

fn grant(pid: &str, app: &str, role: Option<&str>) {
    create_grant(&SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: pid.into(),
        kind: "app".into(),
        target_id: app.into(),
        role: role.map(str::to_string),
        scope: None,
        created_by: Some("test".into()),
    })
    .expect("app grant");
}

fn table_exists(name: &str) -> bool {
    with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", params![name], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())
    })
    .unwrap()
        == 1
}

// ── P1 store ─────────────────────────────────────────────────────────

#[test]
fn tables_come_at_open_and_the_schema_version_stays_6() {
    let _f = fx("open");
    for t in ["app_profile_settings", "principal_profiles", "session_meta"] {
        assert!(table_exists(t), "{t}");
    }
    let v: i64 = with_conn(|conn| conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(|e| e.to_string())).unwrap();
    assert_eq!(v, crate::skin::SKIN_SCHEMA_VERSION);
    assert_eq!(v, 6, "no numbered step for the profile tables");
    with_conn(|conn| ensure_schema(conn)).expect("a second run is a no-op");
}

#[test]
fn a_profile_row_goes_with_its_person() {
    let f = fx("cascade");
    set_display_name(&f.pid, Some("Dee")).unwrap();
    assert_eq!(row(&f.pid).unwrap().display_name.as_deref(), Some("Dee"));
    crate::skin::remove_principal("dana").unwrap();
    let n: i64 = with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM principal_profiles WHERE principal_id = ?1", params![f.pid], |r| r.get(0)).map_err(|e| e.to_string())
    })
    .unwrap();
    assert_eq!(n, 0, "ON DELETE CASCADE");
}

// ── settings ─────────────────────────────────────────────────────────

#[test]
fn settings_default_on_and_an_agent_only_lowers() {
    let f = fx("settings");
    let s = settings(&f.app).unwrap();
    assert!(s.edit_names && s.edit_image, "{s:?}");
    let off = ProfileSettingsPatch { edit_names: Some(false), edit_image: None };
    let s = set_settings(&f.app, &off, "agent:ws", true).expect("an agent may turn names off");
    assert!(!s.edit_names && s.edit_image);
    let on = ProfileSettingsPatch { edit_names: Some(true), edit_image: None };
    assert_eq!(set_settings(&f.app, &on, "agent:ws", true), Err(SettingsError::NotLower("editNames")));
    assert!(!settings(&f.app).unwrap().edit_names, "nothing written on a refusal");
    // Setting an already-on field to on is not raising it.
    let same = ProfileSettingsPatch { edit_names: None, edit_image: Some(true) };
    assert!(set_settings(&f.app, &same, "agent:ws", true).is_ok());
    let s = set_settings(&f.app, &on, "owner-token", false).expect("the owner turns it on");
    assert!(s.edit_names);
    assert!(settings(&f.other_app).unwrap().edit_names, "per App");
}

// ── names ────────────────────────────────────────────────────────────

#[test]
fn names_are_cleaned_of_controls_bidi_and_zero_width() {
    assert_eq!(clean_name("  Dana \u{202E}Ruiz\u{200B}  ", 64).as_deref(), Some("Dana Ruiz"));
    assert_eq!(clean_name("\u{1b}[31mRed\u{1b}[0m", 64).as_deref(), Some("Red"));
    assert_eq!(clean_name("a\u{0007}b\nc", 64).as_deref(), Some("ab c"));
    assert_eq!(clean_name("\u{2066}\u{200D}", 64), None);
    assert_eq!(clean_name(&"x".repeat(80), 32).map(|s| s.chars().count()), Some(32));
}

#[test]
fn who_set_the_full_name_is_kept() {
    let f = fx("mark");
    set_principal_full_name_as("dana", Some("Dana Self"), &format!("{SELF_ACTOR_PREFIX}{}", f.pid)).unwrap();
    assert!(row(&f.pid).unwrap().name_self, "the person's own edit");
    set_principal_full_name_as("dana", Some("Dana Ruiz"), "owner-token").unwrap();
    assert!(!row(&f.pid).unwrap().name_self, "the owner's edit clears it");
    assert_eq!(marks().unwrap().get(&f.pid).map(|m| m.0), Some(false));
}

// ── pictures ─────────────────────────────────────────────────────────

fn data_url(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 90, if x < w / 2 { 0 } else { 255 }]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 7 % 256) as u8, (y * 3 % 256) as u8, 40]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(&img).unwrap();
    out.into_inner()
}

/// A JPEG with an EXIF block saying "rotate 90° clockwise" (orientation 6)
/// and a fake GPS-ish marker string.
fn jpeg_with_exif_orientation_6(w: u32, h: u32) -> Vec<u8> {
    let plain = jpeg(w, h);
    let mut tiff = b"MM\x00\x2a\x00\x00\x00\x08".to_vec();
    tiff.extend_from_slice(b"\x00\x01");
    tiff.extend_from_slice(b"\x01\x12\x00\x03\x00\x00\x00\x01\x00\x06\x00\x00");
    tiff.extend_from_slice(b"\x00\x00\x00\x00");
    tiff.extend_from_slice(b"GPS-SECRET-PLACE");
    let mut payload = b"Exif\x00\x00".to_vec();
    payload.extend_from_slice(&tiff);
    let len = (payload.len() + 2) as u16;
    let mut out = plain[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&plain[2..]);
    out
}

fn dims(jpeg: &[u8]) -> (u32, u32) {
    let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).expect("K2's JPEG decodes");
    (img.width(), img.height())
}

#[test]
fn a_picture_comes_back_upright_small_jpeg_and_without_exif() {
    let raw = jpeg_with_exif_orientation_6(80, 40);
    let out = agent_look::normalize_profile_image(&data_url("image/jpeg", &raw)).expect("a JPEG with EXIF");
    assert_eq!(&out[..3], &[0xFF, 0xD8, 0xFF], "JPEG out");
    assert_eq!(dims(&out), (40, 80), "orientation 6 applied: the landscape picture is now portrait");
    assert!(!out.windows(4).any(|w| w == b"Exif"), "EXIF dropped");
    assert!(!out.windows(10).any(|w| w == b"GPS-SECRET"), "nothing the person sent survives");

    let big = png(1200, 900);
    let out = agent_look::normalize_profile_image(&data_url("image/png", &big)).expect("a PNG with alpha");
    let (w, h) = dims(&out);
    assert!(w <= 256 && h <= 256, "{w}x{h}");
    assert!(out.len() <= agent_look::PROFILE_MAX_BYTES, "{}", out.len());
}

#[test]
fn pictures_refused_with_codes() {
    let svg = data_url("image/svg+xml", b"<svg xmlns='http://www.w3.org/2000/svg'/>");
    assert_eq!(agent_look::normalize_profile_image(&svg).unwrap_err().code(), "type_refused");
    let gif = data_url("image/gif", b"GIF89a\x01\x00\x01\x00\x00\x00\x00;");
    assert_eq!(agent_look::normalize_profile_image(&gif).unwrap_err().code(), "type_refused");
    let lie = data_url("image/png", &jpeg(10, 10));
    assert_eq!(agent_look::normalize_profile_image(&lie).unwrap_err().code(), "type_mismatch");
    let huge = data_url("image/png", &vec![0u8; agent_look::MAX_INPUT_IMAGE_BYTES + 10]);
    assert_eq!(agent_look::normalize_profile_image(&huge).unwrap_err().code(), "too_large");
    assert_eq!(agent_look::normalize_profile_image("not a data url").unwrap_err().code(), "not_data_url");
    // Over the profile decoder's side limit (4096 px): refused, not decoded.
    let wide = png(4100, 2);
    assert_eq!(agent_look::normalize_profile_image(&data_url("image/png", &wide)).unwrap_err().code(), "bad_image");
    assert!(crate::skin::guard::decode_slots_peak() <= crate::skin::guard::MAX_CONCURRENT_DECODES);
}

#[test]
fn a_stored_picture_has_a_hash_and_clears() {
    let f = fx("img");
    let out = agent_look::normalize_profile_image(&data_url("image/png", &png(64, 64))).unwrap();
    let hash = set_image(&f.pid, &out).unwrap();
    let r = row(&f.pid).unwrap();
    assert_eq!(r.image_hash.as_deref(), Some(hash.as_str()));
    assert_eq!(hash, image_hash(&out));
    assert!(r.image_data_url().unwrap().starts_with("data:image/jpeg;base64,"));
    assert!(clear_image(&f.pid).unwrap());
    assert!(!clear_image(&f.pid).unwrap(), "nothing left to clear");
    assert_eq!(row(&f.pid).unwrap().image, None);
    assert!(set_image(&f.pid, &vec![0u8; agent_look::PROFILE_MAX_BYTES + 1]).is_err(), "the 64 KiB cap holds at the store too");
}

#[test]
fn the_write_budget_is_twenty_per_ten_minutes() {
    let p = format!("budget-{}", uuid::Uuid::new_v4());
    for i in 0..WRITES_PER_WINDOW {
        assert!(take_write(&p).is_ok(), "{i}");
    }
    let retry = take_write(&p).expect_err("the 21st write");
    assert!(retry > 0 && retry <= WRITE_WINDOW_SECS);
}

// ── sessions ─────────────────────────────────────────────────────────

#[test]
fn user_agent_labels_are_coarse() {
    let safari = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15";
    assert_eq!(label_from_user_agent(safari).as_deref(), Some("Safari on macOS"));
    let chrome = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";
    assert_eq!(label_from_user_agent(chrome).as_deref(), Some("Chrome on Windows"));
    assert_eq!(label_from_user_agent("okhttp/4.12.0").as_deref(), Some("Android app"));
    assert_eq!(label_from_user_agent("Jobs/3 CFNetwork/1490 Darwin/23.0.0").as_deref(), Some("iOS app"));
    assert_eq!(label_from_user_agent(""), None);
    assert_eq!(label_from_user_agent("whatever/1.0"), None);
}

#[test]
fn the_list_is_this_apps_only_with_devices_and_current() {
    let f = fx("list");
    grant(&f.pid, &f.other_app, Some("o-list"));
    let a = create_app_session_token("dana", &f.app).unwrap();
    record_session(&a.meta.id, VIA_PASSWORD, Some("Safari on macOS"));
    let b = create_app_session_token("dana", &f.app).unwrap();
    record_session(&b.meta.id, VIA_PASSWORD, Some("  Chrome\u{202E} on Linux "));
    let other = create_app_session_token("dana", &f.other_app).unwrap();
    let phone = app_login::login_device("dana", &f.app, &app_login::Device { id: "ph-1".into(), name: "Dana's iPhone".into() })
        .unwrap()
        .expect("stay signed in is on");
    let list = sessions_for(&f.pid, &f.app, &a.meta.id).unwrap();
    let keys: Vec<&str> = list.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(list.len(), 3, "{keys:?}");
    assert!(list[0].current && list[0].key == a.meta.id, "the caller's first");
    assert!(!keys.contains(&other.meta.id.as_str()), "another App's session never shows");
    assert!(!keys.contains(&phone.session.meta.id.as_str()), "a device's pass shows as the device");
    let dev = list.iter().find(|s| s.kind == "device").expect("the device sign-in");
    assert_eq!((dev.key.as_str(), dev.client.as_deref()), (phone.device_login_id.as_str(), Some("Dana's iPhone")));
    let b_row = list.iter().find(|s| s.key == b.meta.id).unwrap();
    assert_eq!(b_row.client.as_deref(), Some("Chrome on Linux"), "labels are cleaned");
    assert_eq!(b_row.via.as_deref(), Some(VIA_PASSWORD));
    assert!(last_sign_in(&f.pid, &f.app).unwrap().is_some());
}

#[test]
fn the_box_login_never_shows_in_an_apps_list() {
    let f = fx("host");
    // An old session row with no App is the box login's.
    let a = create_app_session_token("dana", &f.app).unwrap();
    with_conn(|conn| conn.execute("UPDATE tokens SET app_service_id = NULL WHERE id = ?1", params![a.meta.id]).map_err(|e| e.to_string()))
        .unwrap();
    assert!(sessions_for(&f.pid, &f.app, "none").unwrap().iter().all(|s| s.key != a.meta.id));
}

#[test]
fn revoke_is_own_app_only_and_sign_out_others_keeps_the_caller() {
    let f = fx("revoke");
    grant(&f.pid, &f.other_app, Some("o-revoke"));
    let me = create_app_session_token("dana", &f.app).unwrap();
    let web2 = create_app_session_token("dana", &f.app).unwrap();
    let other_app = create_app_session_token("dana", &f.other_app).unwrap();
    let phone = app_login::login_device("dana", &f.app, &app_login::Device { id: "ph-2".into(), name: "Phone".into() }).unwrap().unwrap();
    assert!(!revoke_browser_session(&f.pid, &f.app, &other_app.meta.id).unwrap(), "another App's session: not found");
    assert!(!revoke_browser_session(&f.pid, &f.app, &phone.session.meta.id).unwrap(), "a device's pass is revoked as the device");
    assert!(!revoke_browser_session("someone-else", &f.app, &web2.meta.id).unwrap(), "not theirs");
    assert!(revoke_browser_session(&f.pid, &f.app, &web2.meta.id).unwrap());
    assert!(resolve_skin_token(&web2.raw).is_none());
    let web3 = create_app_session_token("dana", &f.app).unwrap();
    let n = sign_out_others(&f.pid, &f.app, &me.meta.id).unwrap();
    assert_eq!(n, 3, "web3, the phone's pass and the phone's sign-in");
    assert!(resolve_skin_token(&me.raw).is_some(), "the caller stays");
    assert!(resolve_skin_token(&web3.raw).is_none());
    assert!(resolve_skin_token(&phone.session.raw).is_none());
    assert!(app_login::devices_for(&f.pid, &f.app).unwrap().is_empty());
    assert!(resolve_skin_token(&other_app.raw).is_some(), "another App's session is not this App's to end");
}

#[test]
fn sign_out_others_keeps_the_callers_device_chain() {
    let f = fx("chain");
    let phone = app_login::login_device("dana", &f.app, &app_login::Device { id: "ph-3".into(), name: "Phone".into() }).unwrap().unwrap();
    let web = create_app_session_token("dana", &f.app).unwrap();
    sign_out_others(&f.pid, &f.app, &phone.session.meta.id).unwrap();
    assert!(resolve_skin_token(&phone.session.raw).is_some());
    assert_eq!(app_login::devices_for(&f.pid, &f.app).unwrap().len(), 1, "the caller's device sign-in stays");
    assert!(resolve_skin_token(&web.raw).is_none());
}

#[test]
fn last_seen_lives_in_memory_until_the_flush_and_dead_rows_go() {
    let f = fx("seen");
    let a = create_app_session_token("dana", &f.app).unwrap();
    record_session(&a.meta.id, VIA_PASSWORD, None);
    with_conn(|conn| conn.execute("UPDATE session_meta SET last_seen_at = 1 WHERE token_id = ?1", params![a.meta.id]).map_err(|e| e.to_string()))
        .unwrap();
    assert!(resolve_skin_token(&a.raw).is_some());
    let stored = |id: &str| -> Option<i64> {
        with_conn(|conn| {
            conn.query_row("SELECT last_seen_at FROM session_meta WHERE token_id = ?1", params![id], |r| r.get(0))
                .optional()
                .map_err(|e| e.to_string())
        })
        .unwrap()
        .flatten()
    };
    assert_eq!(stored(&a.meta.id), Some(1), "resolving a pass writes nothing to skin.db");
    let listed = sessions_for(&f.pid, &f.app, &a.meta.id).unwrap();
    assert!(listed[0].last_seen_at.unwrap() > 1, "the list reads the remembered time");
    flush_last_seen().unwrap();
    assert!(stored(&a.meta.id).unwrap() > 1, "the flush stamps it");
    crate::skin::revoke_token(&a.meta.id).unwrap();
    flush_last_seen().unwrap();
    assert!(stored(&a.meta.id).is_some(), "a just-ended session's row stays (the last sign-in survives a sign-out)");
    assert!(last_sign_in(&f.pid, &f.app).unwrap().is_some());
    with_conn(|conn| conn.execute("UPDATE tokens SET revoked_at = 1 WHERE id = ?1", params![a.meta.id]).map_err(|e| e.to_string())).unwrap();
    flush_last_seen().unwrap();
    assert_eq!(stored(&a.meta.id), None, "a long-ended session's row goes");
}

// ── P0: the live password paths ──────────────────────────────────────

fn lock_out(name: &str) {
    for _ in 0..3 {
        let _ = check_and_record_login(name, "wrong-wrong-wrong");
    }
    assert!(matches!(check_and_record_login(name, "s3cret-horse"), SkinLoginOutcome::LockedOut { .. }), "locked first");
}

fn live_invites(pid: &str) -> Vec<invites::InviteRow> {
    invites::rows_for(pid).unwrap().into_iter().filter(|r| r.used_at.is_none()).collect()
}

#[test]
fn a_used_reset_link_clears_the_lockout_in_both_stores() {
    let f = fx("lock");
    lock_out("dana");
    let m = invites::mint(&f.pid, &f.app, invites::PURPOSE_RESET, 3600, Some("dana@example.test"), false, None, "test").unwrap();
    invites::activate(&m.id).unwrap();
    assert_eq!(invites::consume(&m.token, "brand-new-pw-1").unwrap(), invites::Consume::Ok);
    assert!(matches!(check_and_record_login("dana@example.test", "brand-new-pw-1"), SkinLoginOutcome::Ok(_)), "unlocked by email too");

    lock_out("dana@example.test");
    let mint = mint_password_reset("dana").unwrap().expect("password + email");
    assert_eq!(consume_password_reset(&mint.token, "brand-new-pw-2").unwrap(), crate::skin::SkinResetConsume::Ok);
    assert!(matches!(check_and_record_login("dana", "brand-new-pw-2"), SkinLoginOutcome::Ok(_)));
}

#[test]
fn the_owners_password_set_still_does_not_unlock() {
    let _f = fx("nolock");
    lock_out("dana");
    set_principal_password("dana", Some("owner-set-pw")).unwrap();
    assert!(matches!(check_and_record_login("dana", "owner-set-pw"), SkinLoginOutcome::LockedOut { .. }), "Rosson 2026-09-29 split");
}

#[test]
fn an_owner_reset_used_voids_every_other_link_on_every_app() {
    let f = fx("owner");
    grant(&f.pid, &f.other_app, Some("o-owner"));
    let app_b = invites::mint(&f.pid, &f.other_app, invites::PURPOSE_RESET, 3600, Some("dana@example.test"), false, None, "pass:b").unwrap();
    invites::activate(&app_b.id).unwrap();
    let old = mint_password_reset("dana").unwrap().unwrap();
    let owner = invites::mint(&f.pid, &f.app, invites::PURPOSE_OWNER_RESET, 3600, None, false, None, "owner-token").unwrap();
    invites::activate(&owner.id).unwrap();
    assert_eq!(invites::consume(&owner.token, "owners-reset-pw").unwrap(), invites::Consume::Ok);
    assert_eq!(invites::consume(&app_b.token, "app-b-takeover").unwrap(), invites::Consume::BadToken, "App B's leftover link is dead");
    assert_eq!(consume_password_reset(&old.token, "old-store-pw").unwrap(), crate::skin::SkinResetConsume::BadToken);
    assert!(live_invites(&f.pid).is_empty());
}

#[test]
fn an_app_reset_waits_while_the_owners_reset_is_live() {
    let f = fx("prec");
    let owner = invites::mint(&f.pid, &f.app, invites::PURPOSE_OWNER_RESET, 3600, None, false, None, "owner-token").unwrap();
    assert!(invites::owner_reset_pending(&f.pid).unwrap(), "held (not yet sent) counts");
    let err = invites::mint(&f.pid, &f.other_app, invites::PURPOSE_RESET, 3600, None, false, None, "pass:x").unwrap_err();
    assert_eq!(err, invites::MintError::OwnerResetPending);
    let err = invites::mint_requested(&f.pid, &f.app, 3600, None, None, "pass:x").unwrap_err();
    assert_eq!(err, invites::MintError::OwnerResetPending);
    invites::activate(&owner.id).unwrap();
    assert_eq!(invites::consume(&owner.token, "owners-reset-pw").unwrap(), invites::Consume::Ok);
    assert!(!invites::owner_reset_pending(&f.pid).unwrap());
    assert!(invites::mint(&f.pid, &f.app, invites::PURPOSE_RESET, 3600, None, false, None, "pass:x").is_ok());
}

#[test]
fn a_password_change_voids_unused_links_but_keeps_the_owners_reset() {
    let f = fx("void");
    let reset = invites::mint(&f.pid, &f.app, invites::PURPOSE_RESET, 3600, None, false, None, "pass:x").unwrap();
    invites::activate(&reset.id).unwrap();
    let old = mint_password_reset("dana").unwrap().unwrap();
    set_principal_password("dana", Some("changed-pw-1")).unwrap();
    assert_eq!(invites::consume(&reset.token, "reset-after-change").unwrap(), invites::Consume::BadToken);
    assert_eq!(consume_password_reset(&old.token, "old-after-change").unwrap(), crate::skin::SkinResetConsume::BadToken);
    let owner = invites::mint(&f.pid, &f.app, invites::PURPOSE_OWNER_RESET, 3600, None, false, None, "owner-token").unwrap();
    invites::activate(&owner.id).unwrap();
    set_principal_password("dana", Some("changed-pw-2")).unwrap();
    assert_eq!(invites::consume(&owner.token, "owner-still-works").unwrap(), invites::Consume::Ok, "the owner's remedy stays");
}

#[test]
fn a_request_from_outside_never_kills_the_live_link() {
    let f = fx("keep");
    let first = invites::mint_requested(&f.pid, &f.app, 3600, Some("dana@example.test"), None, "pass:x").unwrap();
    invites::activate(&first.id).unwrap();
    let second = invites::mint_requested(&f.pid, &f.app, 3600, Some("dana@example.test"), None, "pass:x").unwrap();
    invites::activate(&second.id).unwrap();
    let third = invites::mint_requested(&f.pid, &f.app, 3600, Some("dana@example.test"), None, "pass:x").unwrap();
    invites::activate(&third.id).unwrap();
    let live: Vec<String> = live_invites(&f.pid).into_iter().map(|r| r.id).collect();
    assert_eq!(live.len(), 2, "at most two live: {live:?}");
    assert!(live.contains(&third.id) && live.contains(&second.id), "the newest live one stays with the new one");
    assert_eq!(invites::consume(&second.token, "requested-pw-1").unwrap(), invites::Consume::Ok);
    assert_eq!(invites::consume(&third.token, "requested-pw-2").unwrap(), invites::Consume::BadToken, "using one voids the rest");
}

#[test]
fn passwords_over_256_bytes_are_refused_everywhere() {
    let f = fx("long");
    let long = "p".repeat(crate::skin::MAX_PASSWORD_BYTES + 1);
    assert_eq!(set_principal_password("dana", Some(&long)).unwrap_err(), crate::skin::PASSWORD_TOO_LONG);
    assert!(set_principal_password("dana", Some(&"p".repeat(crate::skin::MAX_PASSWORD_BYTES))).is_ok(), "256 is fine");
    let m = invites::mint(&f.pid, &f.app, invites::PURPOSE_RESET, 3600, None, false, None, "t").unwrap();
    invites::activate(&m.id).unwrap();
    assert!(invites::consume(&m.token, &long).is_err());
    assert_eq!(invites::consume(&m.token, "a-fine-password").unwrap(), invites::Consume::Ok, "the refusal spent nothing");
}

#[test]
fn junk_reset_tokens_spend_no_hash() {
    let _f = fx("junk");
    // A junk token is answered from the lookup alone: far under one
    // argon2 run even with every hash slot busy elsewhere.
    let slots: Vec<_> = (0..crate::skin::guard::MAX_CONCURRENT_HASHES).map(|_| crate::skin::guard::hash_slot()).collect();
    let t = std::time::Instant::now();
    assert_eq!(invites::consume("k2inv_deadbeef", "whatever-pw-1").unwrap(), invites::Consume::BadToken);
    assert_eq!(consume_password_reset("deadbeef", "whatever-pw-1").unwrap(), crate::skin::SkinResetConsume::BadToken);
    assert!(t.elapsed() < std::time::Duration::from_secs(2), "no slot was waited on: {:?}", t.elapsed());
    drop(slots);
}

#[test]
fn staying_keeps_the_caller_and_its_device_and_ends_the_rest_everywhere() {
    let f = fx("stay");
    grant(&f.pid, &f.other_app, Some("o-stay"));
    let phone = app_login::login_device("dana", &f.app, &app_login::Device { id: "ph-s".into(), name: "Phone".into() }).unwrap().unwrap();
    let web = create_app_session_token("dana", &f.app).unwrap();
    let elsewhere = create_app_session_token("dana", &f.other_app).unwrap();
    set_principal_password_staying("dana", "stay-signed-in-1", &phone.session.meta.id).unwrap();
    assert!(resolve_skin_token(&phone.session.raw).is_some(), "the caller's pass stays");
    assert_eq!(app_login::devices_for(&f.pid, &f.app).unwrap().len(), 1, "its device sign-in stays");
    assert!(resolve_skin_token(&web.raw).is_none());
    assert!(resolve_skin_token(&elsewhere.raw).is_none(), "every App");
    set_principal_email("dana", None).unwrap();
}

#[test]
fn wrong_current_passwords_are_throttled_per_person() {
    let f = fx("cur");
    for _ in 0..crate::skin::guard::CURRENT_PASSWORD_LIMIT {
        assert_eq!(crate::skin::check_current_password(&f.pid, "dana", "nope-nope"), crate::skin::CurrentPassword::Wrong);
    }
    assert!(matches!(
        crate::skin::check_current_password(&f.pid, "dana", "s3cret-horse"),
        crate::skin::CurrentPassword::Throttled { .. }
    ));
    assert!(matches!(check_and_record_login("dana", "s3cret-horse"), SkinLoginOutcome::Ok(_)), "never the login lockout");
    crate::skin::guard::expire_current_password_window(&f.pid);
    assert_eq!(crate::skin::check_current_password(&f.pid, "dana", "s3cret-horse"), crate::skin::CurrentPassword::Ok);
}
