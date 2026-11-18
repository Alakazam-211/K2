//! The site → App copy (0.46.16): grants with scope, enabled flag and
//! role; a site-tagged role copied as `<role>-<app>`; a third App's role
//! and an existing App grant left alone; dry run and the expect variant.

use super::*;
use crate::skin::*;

const SITE: &str = "5a5a5a5a-1111-4222-8333-444444444444";
const APP: &str = "6b6b6b6b-1111-4222-8333-444444444444";
const OTHER: &str = "7c7c7c7c-1111-4222-8333-444444444444";

fn policy(room: &str, caps: &[&str]) -> RoomPolicy {
    let mut p = RoomPolicy::new();
    p.insert(room.to_string(), caps.iter().map(|c| (*c).to_string()).collect());
    p
}

fn grant(principal: &str, target: &str, role: Option<&str>, scope: Option<&str>) -> SkinGrant {
    create_grant(&SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: principal.into(),
        kind: "app".into(),
        target_id: target.into(),
        role: role.map(str::to_string),
        scope: scope.map(str::to_string),
        created_by: Some("owner-token".into()),
    })
    .expect("grant")
}

fn on(principal: &str, target: &str) -> Option<SkinGrant> {
    list_grants().expect("grants").into_iter().find(|g| g.subject_kind == "principal" && g.subject_id == principal && g.kind == "app" && g.target_id == target)
}

#[test]
fn a_sites_guests_land_on_the_app_with_their_rooms_roles_and_disabled_flag() {
    let _home = crate::test_env::TempHome::new();
    let room = uuid::Uuid::new_v4().to_string();
    let site_role = create_role_for_app("editor", &policy(&room, &[CAP_THREAD_READ, CAP_THREAD_POST]), Some(SITE)).expect("site role");
    let box_role = create_role("staff", &policy(&room, &[CAP_THREAD_READ])).expect("box role");
    let third = create_role_for_app("third", &RoomPolicy::new(), Some(OTHER)).expect("third role");

    let ada = add_principal("ada").expect("ada");
    grant(&ada.id, SITE, Some("editor"), None);
    let bob = add_principal("bob").expect("bob");
    grant(&bob.id, SITE, None, Some(&format!("[\"{room}\"]")));
    let cy = add_principal("cy").expect("cy");
    grant(&cy.id, SITE, Some("staff"), None);
    let dee = add_principal("dee").expect("dee");
    let gd = grant(&dee.id, SITE, None, Some("[]"));
    set_grant_enabled(&gd.id, false).expect("disable dee");
    // A grant on the site naming a third App's role (create_grant refuses
    // that shape now; older rows can hold it).
    let eve = add_principal("eve").expect("eve");
    let ge = grant(&eve.id, SITE, None, Some("[]"));
    with_conn(|c| {
        c.execute("UPDATE grants SET role_id = ?1, scope = NULL WHERE id = ?2", rusqlite::params![third.id, ge.id]).map_err(|e| e.to_string())?;
        Ok(())
    })
    .expect("eve's old row");
    let fay = add_principal("fay").expect("fay");
    grant(&fay.id, SITE, None, Some("[]"));
    let fay_app = grant(&fay.id, APP, None, Some("[]"));
    set_grant_enabled(&fay_app.id, false).expect("fay's App grant off");

    let dry = copy_site_grants_to_app(SITE, APP, "jobs", None, "owner-token", true).expect("dry");
    assert!(dry.dry_run);
    assert_eq!(dry.added, ["ada", "bob", "cy", "dee"].map(String::from).to_vec());
    assert_eq!(dry.kept_existing, vec![("fay".to_string(), false)]);
    assert_eq!(dry.skipped_other_app_role, vec!["eve".to_string()]);
    assert_eq!(dry.roles_copied, vec![("editor".to_string(), "editor-jobs".to_string())]);
    assert!(on(&ada.id, APP).is_none(), "a dry run writes nothing");
    assert!(find_role("editor-jobs").expect("lookup").is_none(), "a dry run copies no role");

    let real = copy_site_grants_to_app(SITE, APP, "jobs", None, "agent:jobs", false).expect("copy");
    assert_eq!(real.added, dry.added);
    let copy = find_role("editor-jobs").expect("lookup").expect("the App's copy of the role");
    assert_eq!(copy.app_id.as_deref(), Some(APP));
    assert_eq!(copy.room_policy, site_role.room_policy, "the same functions in the same rooms");
    assert_ne!(copy.id, site_role.id);

    let ga = on(&ada.id, APP).expect("ada copied");
    assert_eq!(ga.role_id.as_deref(), Some(copy.id.as_str()));
    assert_eq!(ga.created_by.as_deref(), Some("agent:jobs"));
    assert!(ga.enabled);
    assert_eq!(on(&cy.id, APP).expect("cy").role_id.as_deref(), Some(box_role.id.as_str()), "a box-wide role is used as it is");
    assert_eq!(on(&bob.id, APP).expect("bob").scope, on(&bob.id, SITE).expect("bob's site grant").scope, "the same rooms scope");
    assert!(!on(&dee.id, APP).expect("dee").enabled, "a disabled guest stays disabled");
    assert!(on(&eve.id, APP).is_none(), "a third App's role is never copied");
    assert!(!on(&fay.id, APP).expect("fay").enabled, "the App's own grant wins");

    // Guests sign in to the App with the rooms the site gave them.
    assert_eq!(create_app_session_token("ada", APP).expect("ada signs in").meta.rooms, vec![room.clone()]);
    assert_eq!(create_app_session_token("bob", APP).expect("bob signs in").meta.rooms, vec![room.clone()]);
    assert!(create_app_session_token("dee", APP).is_err(), "dee is disabled on the App too");

    // The site is untouched (a label return finds it as it was).
    assert_eq!(on(&ada.id, SITE).expect("ada's site grant").role_id.as_deref(), Some(site_role.id.as_str()));
    assert_eq!(find_role("editor").expect("lookup").expect("site role").app_id.as_deref(), Some(SITE));

    // Again: nothing new, and the role copy is not made twice.
    let again = copy_site_grants_to_app(SITE, APP, "jobs", None, "owner-token", false).expect("again");
    assert!(again.added.is_empty(), "{again:?}");
    assert_eq!(again.kept_existing.len(), 5, "{again:?}");
    assert!(list_roles().expect("roles").iter().all(|r| r.name != "editor-jobs-2"));
}

#[test]
fn a_copy_names_a_role_around_a_taken_name_and_refuses_host_or_itself() {
    let _home = crate::test_env::TempHome::new();
    let room = uuid::Uuid::new_v4().to_string();
    create_role_for_app("crew", &policy(&room, &[CAP_THREAD_READ]), Some(SITE)).expect("site role");
    // Someone already made a different `crew-jobs`.
    create_role("crew-jobs", &RoomPolicy::new()).expect("taken name");
    let ada = add_principal("ada").expect("ada");
    grant(&ada.id, SITE, Some("crew"), None);
    let r = copy_site_grants_to_app(SITE, APP, "jobs", None, "owner-token", false).expect("copy");
    assert_eq!(r.roles_copied, vec![("crew".to_string(), "crew-jobs-2".to_string())]);
    let copy = find_role("crew-jobs-2").expect("lookup").expect("copy");
    assert_eq!(on(&ada.id, APP).expect("ada").role_id.as_deref(), Some(copy.id.as_str()));
    assert!(copy_site_grants_to_app(HOST_APP_ID, APP, "jobs", None, "owner-token", true).is_err());
    assert!(copy_site_grants_to_app(SITE, HOST_APP_ID, "jobs", None, "owner-token", true).is_err());
    assert!(copy_site_grants_to_app(SITE, SITE, "jobs", None, "owner-token", true).is_err());
}

#[test]
fn the_expect_copy_writes_only_the_guests_the_card_showed() {
    let _home = crate::test_env::TempHome::new();
    let ada = add_principal("ada").expect("ada");
    grant(&ada.id, SITE, None, Some("[]"));
    let bob = add_principal("bob").expect("bob");
    grant(&bob.id, SITE, None, Some("[]"));
    match copy_site_grants_to_app_expect(SITE, APP, "jobs", None, "owner-token", &["ada".to_string()]) {
        Err(ExpectError::Changed { now }) => assert_eq!(now, vec!["ada".to_string(), "bob".to_string()]),
        other => panic!("a grown list must refuse: {other:?}"),
    }
    assert!(on(&ada.id, APP).is_none(), "nothing written on a mismatch");
    let r = copy_site_grants_to_app_expect(SITE, APP, "jobs", None, "owner-token", &["BOB".to_string(), "ada".to_string()]).expect("the same list, any order");
    assert_eq!(r.added.len(), 2);
    assert!(on(&bob.id, APP).is_some());
}

/// A role's folder permit list (`k2 app role paths`) is copied exactly: a
/// guest permitted only docs/ on the site is permitted only docs/ on the
/// App, read back the way a login's request reads it (the App grant's
/// role → its `room_paths`).
#[test]
fn a_site_roles_folder_permit_list_is_copied_exactly() {
    let _home = crate::test_env::TempHome::new();
    let room = uuid::Uuid::new_v4().to_string();
    create_role_for_app("reader", &policy(&room, &[CAP_THREAD_READ, CAP_FILES_READ]), Some(SITE)).expect("site role");
    let scope = paths::RoomPathScope { read: vec!["docs/".to_string()], ..Default::default() };
    let site_role = paths::set_role_room_paths("reader", &room, &scope).expect("docs/ only on the site");
    let ada = add_principal("ada").expect("ada");
    grant(&ada.id, SITE, Some("reader"), None);

    let r = copy_site_grants_to_app(SITE, APP, "jobs", None, "owner-token", false).expect("copy");
    assert_eq!(r.roles_copied, vec![("reader".to_string(), "reader-jobs".to_string())]);

    let app_role = on(&ada.id, APP).expect("ada on the App").role_id.expect("a role");
    let (site_paths, app_paths) = with_conn(|c| Ok((paths::role_room_paths(c, &site_role.id)?, paths::role_room_paths(c, &app_role)?))).expect("paths");
    assert_eq!(app_paths, site_paths, "the App's copy holds the site role's permit list exactly");
    let line = app_paths.get(&room).expect("the room's line on the App");
    assert_eq!(line.read, vec!["docs".to_string()], "only docs/ (stored without the slash): {line:?}");
    assert!(line.write.is_empty(), "no write folder appears: {line:?}");
    let can = |rel: &str, need: paths::PathNeed| paths::allows_rel(line, rel, need).expect("a valid path");
    assert!(can("docs/plan.md", paths::PathNeed::Read), "docs/ is readable on the App");
    assert!(!can("private/notes.md", paths::PathNeed::Read), "outside docs/ stays hidden on the App");
    assert!(!can("docs/plan.md", paths::PathNeed::Write), "read only stays read only");

    // A role of that name with other folders is never re-used for the copy.
    create_role_for_app("crew", &policy(&room, &[CAP_FILES_READ]), Some(SITE)).expect("crew");
    paths::set_role_room_paths("crew", &room, &scope).expect("crew docs/");
    create_role_for_app("crew-jobs", &policy(&room, &[CAP_FILES_READ]), Some(APP)).expect("same policy, whole room");
    let bob = add_principal("bob").expect("bob");
    grant(&bob.id, SITE, Some("crew"), None);
    let r = copy_site_grants_to_app(SITE, APP, "jobs", None, "owner-token", false).expect("copy bob");
    assert_eq!(r.roles_copied, vec![("crew".to_string(), "crew-jobs-2".to_string())], "a whole-room crew-jobs isn't the copy");
}
