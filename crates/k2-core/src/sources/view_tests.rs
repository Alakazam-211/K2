use super::*;
use crate::sources::extract::ProjectedItem;
use crate::sources::spec::{OnTargetError, Viewers};
use crate::sources::state::ItemState;
use serde_json::json;

pub(crate) fn row() -> SourceRow {
    SourceRow {
        id: "s1".into(),
        name: "deploys".into(),
        label: "Deploys".into(),
        kind: SourceKind::HttpJson,
        preset: None,
        spec_json: "{}".into(),
        spec_hash: "0".repeat(64),
        origins: vec!["https://api.example.com:443".into()],
        lan: false,
        secret_name: Some("K".into()),
        url_secret_name: None,
        secret_bound: true,
        every_secs: 60,
        timeout_secs: 10,
        max_bytes: 1 << 20,
        fail_after: 3,
        on_target_error: OnTargetError::Crit,
        link: None,
        viewers: Viewers::Admin,
        agent_access: AgentAccess::Shape,
        keep: Keep::Values,
        pinned: false,
        pinned_by: None,
        pinned_at: None,
        status: SourceStatus::Active,
        pause_reason: None,
        made_by: "owner".into(),
        made_by_project: None,
        approved_by: None,
        approved_at: None,
        approved_spec_hash: None,
        simulate_state: None,
        simulate_until: None,
        simulate_by: None,
        mute_until: None,
        mute_by: None,
        mute_reason: None,
        rev: 7,
        created_at: 0,
        updated_at: 0,
        removed_at: None,
        secret_workspace: None,
    }
}

fn state() -> (SourceState, Projection) {
    let mut st = SourceState::new("s1");
    st.phase = Phase::Live;
    st.health = Health::Crit;
    st.updated_at = Some(990);
    let mut it = ItemState::new("d1", false);
    it.health = Health::Crit;
    st.items.insert("d1".into(), it);
    st.items.insert("gone-from-projection".into(), ItemState::new("gone-from-projection", true));
    let p = Projection {
        fields: [("count".to_string(), json!(4)), ("team".to_string(), json!("acme"))].into(),
        items: vec![ProjectedItem { key: "d1".into(), fields: [("state".to_string(), json!("ERROR"))].into() }],
    };
    (st, p)
}

#[test]
fn agents_see_masked_values_on_shape_sources_and_people_see_values() {
    let (st, p) = state();
    let v = build(&row(), &st, Some(&p), Viewer::Agent, 1000);
    assert!(v.masked);
    assert_eq!(v.fields["count"], json!(0));
    assert_eq!(v.fields["team"], json!("•••"));
    assert_eq!(v.items[0].fields["state"], json!("•••"));
    assert_eq!(v.items[0].health, Health::Crit, "states are never masked");
    assert_eq!(v.items.len(), 2, "state-only items still show");
    let v = build(&row(), &st, Some(&p), Viewer::Person, 1000);
    assert!(!v.masked);
    assert_eq!(v.fields["team"], json!("acme"));
    assert_eq!((v.server_now, v.updated_at, v.display), (1000, Some(990), "crit"));
    let mut r = row();
    r.agent_access = AgentAccess::Values;
    assert!(!build(&r, &st, Some(&p), Viewer::Agent, 1000).masked);
}

#[test]
fn keep_states_shows_no_values_to_anyone() {
    let (st, p) = state();
    let mut r = row();
    r.keep = Keep::States;
    for who in [Viewer::Person, Viewer::Agent] {
        let v = build(&r, &st, Some(&p), who, 1000);
        assert!(v.fields.is_empty() && v.items[0].fields.is_empty() && !v.masked);
        assert_eq!(serde_json::to_string(&v).unwrap().matches("acme").count(), 0);
    }
}

#[test]
fn display_precedence_simulate_and_mute_overlays() {
    let (mut st, p) = state();
    let mut r = row();
    r.simulate_state = Some(SimulateState::Ok);
    r.simulate_until = Some(2000);
    r.simulate_by = Some("owner".into());
    let v = build(&r, &st, Some(&p), Viewer::Person, 1000);
    assert_eq!((v.health, v.display), (Health::Ok, "ok"));
    assert!(v.simulated.is_some());
    let v = build(&r, &st, Some(&p), Viewer::Person, 2001);
    assert_eq!(v.health, Health::Crit, "an ended simulation shows the real state");
    assert!(v.simulated.is_none());
    r.simulate_state = None;
    r.simulate_until = None;
    r.mute_until = Some(1500);
    r.mute_by = Some("agent:p1".into());
    assert!(build(&r, &st, Some(&p), Viewer::Person, 1000).muted.is_some());
    assert!(build(&r, &st, Some(&p), Viewer::Person, 1600).muted.is_none());
    st.phase = Phase::Stale;
    assert_eq!(build(&r, &st, Some(&p), Viewer::Person, 1000).display, "stale");
    r.status = SourceStatus::Paused;
    assert_eq!(build(&r, &st, Some(&p), Viewer::Person, 1000).display, "paused");
    st.phase = Phase::Pending;
    r.status = SourceStatus::Active;
    assert_eq!(build(&r, &st, None, Viewer::Person, 1000).display, "pending");
    let json = serde_json::to_value(build(&r, &st, None, Viewer::Person, 1000)).unwrap();
    for k in ["serverNow", "updatedAt", "reasonText", "httpStatus", "hardSince", "recoveredAt"] {
        assert!(json.get(k).is_some(), "{k} on the wire");
    }
}
