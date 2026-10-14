//! Grant, delegation and space routes (contract §4.5, §5.2, §12.1).

use serde_json::{json, Value};

use crate::garden_data::agent_routes::tests::{agent, call, env, err, ok, person_grant};
use crate::zen_data_routes::DataWho;

fn admin(agent_id: &str, level: &str, prefix: &str) -> Value {
    ok(
        &DataWho::Owner,
        "POST",
        "/cli/zen/data/admin",
        json!({ "widget": "board", "agent": agent_id, "level": level, "prefix": prefix }),
        &[],
    )
}

fn grant_by(who: &DataWho, agent_id: &str, level: &str, prefix: &str) -> (u16, Value) {
    call(who, "POST", "/cli/zen/data/grant", json!({ "widget": "board", "agent": agent_id, "level": level, "prefix": prefix }), &[])
}

#[test]
fn a_grant_admin_grants_inside_its_bounds_only() {
    let _e = env();
    let builder = agent("w-builder");
    // Not an admin yet.
    let (code, v) = grant_by(&builder, "w-keeper", "read", "project/7/");
    assert_eq!((code, v["error"]["code"].as_str()), (403, Some("not_granted")));
    admin("w-builder", "write", "project/");
    let (code, v) = grant_by(&builder, "w-keeper", "write", "project/7/");
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["grant"]["grantedBy"], "agent:w-builder");
    assert!(v["grant"]["viaGrant"].as_i64().is_some());
    assert_eq!(v["grant"]["admin"], false);
    // The keeper can now write there.
    ok(&agent("w-keeper"), "POST", "/cli/zen/data/put", json!({"widget": "board", "key": "project/7/x", "value": 1}), &[]);
    // Outside the prefix, itself, project ok.
    let (_, v) = grant_by(&builder, "w-keeper", "read", "other/");
    assert_eq!(v["error"]["code"], "not_granted");
    let (_, v) = grant_by(&builder, "w-builder", "read", "project/9/");
    assert_eq!(v["error"]["code"], "self_grant");
    let (code, v) = call(&builder, "POST", "/cli/zen/data/grant", json!({"widget": "board", "project": "pg-q4", "level": "read", "prefix": "project/7/"}), &[]);
    assert_eq!(code, 200, "{v}");
    assert_eq!(v["warning"], "project_grant_convenience");
    // No `admin` field exists in a grant body.
    let (code, _) = call(&builder, "POST", "/cli/zen/data/grant", json!({"widget": "board", "agent": "w-keeper", "level": "read", "prefix": "project/7/", "admin": true}), &[]);
    assert_eq!(code, 400);
    // A read delegate grants read only.
    admin("w-stranger", "read", "");
    let (_, v) = grant_by(&agent("w-stranger"), "w-keeper", "write", "z/");
    assert_eq!(v["error"]["code"], "level_too_high");
    // The person's grants are out of a delegate's reach.
    person_grant("board", "w-keeper", "read", "shared/");
    let (code, v) = grant_by(&builder, "w-keeper", "write", "project/7/");
    assert_eq!(code, 200, "re-granting its own row is fine: {v}");
    let (_, v) = call(&builder, "POST", "/cli/zen/data/revoke", json!({"widget": "board", "agent": "w-keeper", "prefix": "shared/"}), &[]);
    assert_eq!(v["error"]["code"], "not_own");
    admin("w-builder", "write", "");
    let (_, v) = grant_by(&builder, "w-keeper", "read", "shared/");
    assert_eq!(v["error"]["code"], "grant_exists");
    // Revoke its own.
    let v = ok(&builder, "POST", "/cli/zen/data/revoke", json!({"widget": "board", "agent": "w-keeper", "prefix": "project/7/"}), &[]);
    assert_eq!(v["revoked"], 1);
    // grants listing for the delegate shows what it made.
    let v = ok(&builder, "GET", "/cli/zen/data/grants", json!({}), &[("widget", "board")]);
    assert!(v["grants"].as_array().unwrap().iter().any(|g| g["from"]["kind"] == "delegation"));
    assert_eq!(v["made"].as_array().unwrap().len(), 1, "the project grant it made");
}

#[test]
fn ending_a_delegation_cascades_and_the_person_can_take_rows_over() {
    let _e = env();
    admin("w-builder", "write", "");
    let builder = agent("w-builder");
    for a in ["w-keeper", "w-stranger"] {
        let (code, v) = grant_by(&builder, a, "read", "p/");
        assert_eq!(code, 200, "{v}");
    }
    let v = ok(&DataWho::Owner, "POST", "/cli/zen/data/admin/revoke", json!({"widget": "board", "agent": "w-builder", "dryRun": true}), &[]);
    assert_eq!((v["revoked"].as_i64(), v["cascaded"].as_i64()), (Some(0), Some(2)));
    // A delegation can't be revoked as a plain grant.
    let (code, c) = err(&DataWho::Owner, "POST", "/cli/zen/data/revoke", json!({"widget": "board", "agent": "w-builder"}), &[]);
    assert_eq!((code, c.as_str()), (409, "use_admin_revoke"));
    // The person takes one delegate-made row over; it survives the cascade.
    let v = ok(&DataWho::Owner, "POST", "/cli/zen/data/grant", json!({"widget": "board", "agent": "w-keeper", "level": "read", "prefix": "p/"}), &[]);
    assert_eq!(v["grant"]["grantedBy"], "owner");
    assert_eq!(v["grant"]["viaGrant"], Value::Null);
    let v = ok(&DataWho::Owner, "POST", "/cli/zen/data/admin/revoke", json!({"widget": "board", "agent": "w-builder"}), &[]);
    assert_eq!((v["revoked"].as_i64(), v["cascaded"].as_i64()), (Some(1), Some(1)));
    let (_, c) = err(&agent("w-stranger"), "GET", "/cli/zen/data/list", json!({}), &[("widget", "board")]);
    assert_eq!(c, "not_granted");
    ok(&agent("w-keeper"), "GET", "/cli/zen/data/list", json!({}), &[("widget", "board")]);
    let (code, c) = err(&DataWho::Owner, "POST", "/cli/zen/data/admin/revoke", json!({"widget": "board", "agent": "w-builder"}), &[]);
    assert_eq!((code, c.as_str()), (404, "grant_not_found"));
    let (code, c) = err(&DataWho::Owner, "POST", "/cli/zen/data/grant", json!({"widget": "board", "agent": "nobody-here", "level": "read"}), &[]);
    assert_eq!((code, c.as_str()), (404, "agent_not_found"));
    let (code, c) = err(&DataWho::Owner, "POST", "/cli/zen/data/grant", json!({"widget": "board", "agent": "w-keeper", "project": "pg-q4", "level": "read"}), &[]);
    assert_eq!((code, c.as_str()), (400, "bad_request"));
}

#[test]
fn spaces_show_delete_and_move() {
    let _e = env();
    let o = DataWho::Owner;
    ok(&o, "POST", "/cli/zen/data/put", json!({"widget": "board-old", "key": "a", "value": 1}), &[]);
    let v = ok(&o, "POST", "/cli/zen/data/grant", json!({"widget": "board-old", "agent": "w-keeper", "level": "read"}), &[]);
    assert_eq!(v["created"], true);
    let v = ok(&o, "GET", "/cli/zen/data/spaces", json!({}), &[]);
    let row = &v["spaces"][0];
    assert_eq!((row["widget"].as_str(), row["keys"].as_i64(), row["grants"].as_i64()), (Some("board-old"), Some(1), Some(1)));
    let v = ok(&o, "GET", "/cli/zen/data/show", json!({}), &[("widget", "board-old")]);
    assert_eq!(v["grants"][0]["subjectName"], "board-keeper");
    let (code, c) = err(&o, "GET", "/cli/zen/data/show", json!({}), &[("widget", "ghost")]);
    assert_eq!((code, c.as_str()), (404, "space_not_found"));
    let v = ok(&o, "POST", "/cli/zen/data/space/move", json!({"from": "board-old", "to": "board"}), &[]);
    assert_eq!(v["grantsMoved"], 1);
    let v = ok(&agent("w-keeper"), "GET", "/cli/zen/data/get", json!({}), &[("widget", "board"), ("key", "a")]);
    assert_eq!(v["entry"]["value"], 1, "data and grants moved together");
    let (code, c) = err(&o, "POST", "/cli/zen/data/space/delete", json!({"widget": "board", "confirm": "nope"}), &[]);
    assert_eq!((code, c.as_str()), (400, "confirm_required"));
    let v = ok(&o, "POST", "/cli/zen/data/space/delete", json!({"widget": "board", "confirm": "board"}), &[]);
    assert_eq!(v["grantsRemoved"], 1);
    let v = ok(&o, "GET", "/cli/zen/data/spaces", json!({}), &[]);
    assert_eq!(v["spaces"], json!([]));
    let (code, c) = err(&o, "POST", "/cli/zen/data/space/delete", json!({"widget": "board", "confirm": "board"}), &[]);
    assert_eq!((code, c.as_str()), (404, "space_not_found"));
    // Person routes refuse passports.
    let (code, c) = err(&agent("w-keeper"), "GET", "/cli/zen/data/spaces", json!({}), &[]);
    assert_eq!((code, c.as_str()), (403, "person_only"));
}

#[test]
fn deleting_a_workspace_ends_its_access() {
    let _e = env();
    admin("w-builder", "write", "");
    let (code, _) = grant_by(&agent("w-builder"), "w-keeper", "read", "p/");
    assert_eq!(code, 200);
    k2_core::projects_ops::projects_delete("w-builder").expect("delete");
    let (_, c) = err(&agent("w-keeper"), "GET", "/cli/zen/data/list", json!({}), &[("widget", "board")]);
    assert_eq!(c, "not_granted", "the grants its delegation made went with it");
}
