//! `/cli/app/catalog*`: addresses (default, edited, taken with a
//! suggestion, reserved), the up-front Pro check, one per box, the variant
//! rule, reserved ids, the route table and who may call.

use std::sync::atomic::Ordering;

use k2_core::suite::jobs;
use k2_core::suite::{codes as sc, testkit};
use serde_json::json;

use super::*;
use crate::apps::catalog_job::tests::{agent, assert_plain, catalog_with, facts, fx, seed, set_switches, write_tarball, Fake, WS};

fn hearth_cat(fx: &crate::apps::catalog_job::tests::Fx) -> (Catalog, Fake) {
    let s = seed("hearth", "0.2.4", "k2", testkit::K2_CAPS, &["sew-api", "sew-signal"], testkit::COMMIT);
    let c = catalog_with("sew/hearth", &s, testkit::K2_CAPS, &["sew-api", "sew-signal"]);
    (c, Fake::new(write_tarball(fx, &s)))
}

fn dry(c: &Catalog, f: &Fake, body: J) -> R<J> {
    let mut b = json!({ "workspace": WS, "dryRun": true });
    if let J::Object(m) = body {
        for (k, v) in m {
            b[k] = v;
        }
    }
    install_with(&Who::Owner, &b, c, &facts(), f)
}

fn job_count() -> i64 {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.query_row("SELECT COUNT(*) FROM suite_install_jobs", [], |r| r.get(0)).unwrap()
}

/// An App row taking `id` (a second copy then numbers itself).
fn installed(id: &str) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    let svc = k2_core::published_services::insert(&conn, WS, id, "(app)", "/tmp/x", 0, "local", "stopped", "cmd", "").unwrap();
    k2_core::deploy::store::insert_app(
        &conn,
        &k2_core::deploy::store::NewApp {
            app_id: id,
            service_id: &svc.id,
            seed_id: None,
            owner_workspace: WS,
            isolation: "container",
            memory_limit_bytes: None,
            front_port: 40999,
            created_by: "test",
        },
    )
    .unwrap();
}

/// R10: no address → the family label (`chat` for every Hearth look,
/// `meet` for Meet), as a suite label; the dry run changes nothing.
#[test]
fn the_default_address_is_the_family_label() {
    let fx = fx();
    let (c, f) = hearth_cat(&fx);
    let v = dry(&c, &f, json!({ "entry": "sew/hearth" })).unwrap();
    assert_eq!(v["plan"]["address"]["label"], "chat");
    assert_eq!(v["plan"]["address"]["kind"], jobs::ADDRESS_SUITE_LABEL);
    assert_eq!(v["plan"]["app"], "hearth", "R7: the install id isn't the address");
    assert_eq!(v["plan"]["steps"][3], "address", "A5: Address before Deploy");
    assert_eq!(v["plan"]["steps"][4], "deploy");
    assert!(v["plan"]["approves"].as_array().unwrap().iter().any(|a| a["name"] == "sew-api"), "{v}");
    assert_eq!(v["note"], "nothing changed");
    assert_eq!(job_count(), 0);
    let s = seed("meet", "0.2.4", "k2", testkit::K2_CAPS, &["sew-api"], testkit::COMMIT);
    let c = catalog_with("sew/meet", &s, testkit::K2_CAPS, &["sew-api"]);
    let v = dry(&c, &f, json!({ "entry": "sew/meet" })).unwrap();
    assert_eq!((v["plan"]["address"]["label"].as_str(), v["plan"]["app"].as_str()), (Some("meet"), Some("sew-meet")));
}

/// R13: an edited address is an ordinary label (`k2 app domain add`'s
/// checks); the family label typed in full is still the suite label; a
/// reserved label is only its own family's; one level only; a custom
/// domain waits for 1b.
#[test]
fn an_edited_address_and_its_refusals() {
    let fx = fx();
    let (c, f) = hearth_cat(&fx);
    let v = dry(&c, &f, json!({ "entry": "sew/hearth", "address": "Team-Chat" })).unwrap();
    assert_eq!((v["plan"]["address"]["label"].as_str(), v["plan"]["address"]["kind"].as_str()), (Some("team-chat"), Some(jobs::ADDRESS_LABEL)));
    let v = dry(&c, &f, json!({ "entry": "sew/hearth", "address": "chat.lztek.k2.dev" })).unwrap();
    assert_eq!(v["plan"]["address"]["kind"], jobs::ADDRESS_SUITE_LABEL);
    let e = dry(&c, &f, json!({ "entry": "sew/hearth", "address": "meet" })).unwrap_err();
    assert_eq!(e.code, sc::ADDRESS_RESERVED, "{}", e.hint);
    let e = dry(&c, &f, json!({ "entry": "sew/hearth", "address": "a.chat.lztek.k2.dev" })).unwrap_err();
    assert_eq!(e.code, sc::ADDRESS_INVALID);
    let e = dry(&c, &f, json!({ "entry": "sew/hearth", "domain": "chat.example.com" })).unwrap_err();
    assert_eq!(e.code, "custom_domain_later");
}

/// R14 / AT3: a second Hearth with no address is `address_taken` naming
/// the holder and a suggestion (`<installId>-chat`, never `chat2`), with
/// `--address auto` taking it; the second copy's id is `hearth-2`.
#[test]
fn a_taken_family_label_is_address_taken_with_a_suggestion() {
    let fx = fx();
    let (c, f) = hearth_cat(&fx);
    installed("hearth");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        jobs::label_bind(&conn, &jobs::SuiteLabel { label: "chat".into(), app_id: "hearth".into(), front_port: 40999, bound_at: 1, bound_by: "owner".into() }).unwrap();
    }
    let e = dry(&c, &f, json!({ "entry": "sew/hearth" })).unwrap_err();
    assert_eq!((e.code, e.status), (sc::ADDRESS_TAKEN, 409));
    let x = e.extra.unwrap();
    assert_eq!(x["suggestion"], "hearth-2-chat");
    assert!(x["holder"].as_str().unwrap().contains("hearth"), "{x}");
    assert!(e.hint.contains("--address auto"), "{}", e.hint);
    let v = dry(&c, &f, json!({ "entry": "sew/hearth", "address": "auto" })).unwrap();
    assert_eq!((v["plan"]["app"].as_str(), v["plan"]["address"]["label"].as_str()), (Some("hearth-2"), Some("hearth-2-chat")));
    assert_eq!(v["plan"]["address"]["kind"], jobs::ADDRESS_LABEL);
    // A look's own altLabel is the suggestion when the entry has one.
    let mut c2 = c.clone();
    c2.entries[0].alt_label = Some("community".into());
    let x = dry(&c2, &f, json!({ "entry": "sew/hearth" })).unwrap_err().extra.unwrap();
    assert_eq!(x["suggestion"], "community");
    // An edited label another App serves is taken too.
    let mut cfg = super::super::labels::load_cfg("hearth").unwrap();
    cfg.subdomains.push(k2_core::app_grace::config::AttachedSubdomain { label: "team".into(), added_at: 1, added_by: "o".into(), previous_target: None });
    super::super::labels::save_cfg("hearth", &cfg).unwrap();
    let e = dry(&c, &f, json!({ "entry": "sew/hearth", "address": "team" })).unwrap_err();
    assert_eq!(e.code, sc::ADDRESS_TAKEN, "{}", e.hint);
}

/// AT14 / A17: without Pro the install is refused `pro_required` before
/// anything exists (no job, no App).
#[test]
fn pro_required_refuses_before_anything_exists() {
    let fx = fx();
    let (c, f) = hearth_cat(&fx);
    *f.pro_err.lock().unwrap() = Some((sc::PRO_REQUIRED, "needs K2 Connect Pro".into()));
    let e = install_with(&Who::Owner, &json!({ "entry": "sew/hearth", "workspace": WS }), &c, &facts(), &f).unwrap_err();
    assert_eq!(e.code, sc::PRO_REQUIRED);
    assert_eq!(job_count(), 0);
    assert_eq!(f.installs.load(Ordering::SeqCst), 0);
    // The live preflight's text maps to the same code.
    assert_eq!(super::super::catalog_job::pro_refusal("pro_required: nested subdomains need a Pro plan").code, sc::PRO_REQUIRED);
    // The cached tier says so in the dry run, with no network call.
    let mut fa = facts();
    fa.pro = Some(false);
    let v = install_with(&Who::Owner, &json!({ "entry": "sew/hearth", "workspace": WS, "dryRun": true }), &c, &fa, &f).unwrap();
    assert!(v["tier"].as_str().unwrap().contains("pro_required"), "{v}");
}

/// R15 / AT4: Meet is one per box: a second is refused naming the first.
#[test]
fn meet_is_one_per_box() {
    let fx = fx();
    let s = seed("meet", "0.2.4", "k2", testkit::K2_CAPS, &["sew-api"], testkit::COMMIT);
    let c = catalog_with("sew/meet", &s, testkit::K2_CAPS, &["sew-api"]);
    let f = Fake::new(write_tarball(&fx, &s));
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        jobs::lineage_insert(
            &conn,
            &jobs::Lineage {
                app_id: "sew-meet".into(),
                entry: "sew/meet".into(),
                variant: "k2".into(),
                version: "0.2.4".into(),
                seed_hash: "a".repeat(64),
                follow: true,
                forked: false,
                bad_versions: vec![],
                job_id: "sj-1".into(),
                installed_by: "owner".into(),
                installed_at: 1,
            },
        )
        .unwrap();
    }
    let e = dry(&c, &f, json!({ "entry": "sew/meet", "address": "meet-2" })).unwrap_err();
    assert_eq!(e.code, sc::ONE_PER_BOX);
    assert_eq!(e.extra.unwrap()["existing"], "sew-meet");
}

/// R27: the variant follows the Sew identity; forcing the other one, or a
/// box whose Sew has none pinned, is `variant_mismatch`. No Sew or no
/// Linux is refused with what to do.
#[test]
fn the_box_must_be_able_to_run_it() {
    let fx = fx();
    let (c, f) = hearth_cat(&fx);
    let e = dry(&c, &f, json!({ "entry": "sew/hearth", "variant": "standalone" })).unwrap_err();
    assert_eq!(e.code, sc::VARIANT_MISMATCH);
    let mut fa = facts();
    fa.sew = Ok(("standalone".into(), None));
    let e = install_with(&Who::Owner, &json!({ "entry": "sew/hearth", "workspace": WS, "dryRun": true }), &c, &fa, &f).unwrap_err();
    assert_eq!(e.code, sc::VARIANT_MISMATCH, "only the k2 variant is pinned");
    fa.sew = Err(Why::new("sew_not_installed", "Needs chat hosting", Some("k2 hostchat enable")));
    assert_eq!(install_with(&Who::Owner, &json!({ "entry": "sew/hearth", "workspace": WS }), &c, &fa, &f).unwrap_err().code, "sew_not_installed");
    let mut fa = facts();
    fa.linux = false;
    assert_eq!(install_with(&Who::Owner, &json!({ "entry": "sew/hearth", "workspace": WS }), &c, &fa, &f).unwrap_err().code, "needs_linux");
    let mut fa = facts();
    fa.sew = Ok(("k2".into(), Some("0.1.9".into())));
    assert_eq!(install_with(&Who::Owner, &json!({ "entry": "sew/hearth", "workspace": WS }), &c, &fa, &f).unwrap_err().code, "requires_unmet");
    // An entry with nothing pinned (the compiled catalog today).
    let real = k2_core::suite::catalog().unwrap();
    assert_eq!(install_with(&Who::Owner, &json!({ "entry": "sew/hearth", "workspace": WS }), real, &facts(), &f).unwrap_err().code, sc::NOT_PINNED);
    assert_eq!(install_with(&Who::Owner, &json!({ "entry": "sew/nope", "workspace": WS }), real, &facts(), &f).unwrap_err().code, sc::ENTRY_NOT_FOUND);
}

/// A16: `catalog` and `address` are reserved App ids (every `k2 app`
/// subcommand is), so `--id catalog` is refused.
#[test]
fn catalog_and_address_are_reserved_app_ids() {
    for id in ["catalog", "address"] {
        assert_eq!(k2_core::deploy::app_id_problem(id).map(|p| p.0), Some(codes::APP_ID_RESERVED), "{id}");
    }
    let fx = fx();
    let (c, f) = hearth_cat(&fx);
    let e = dry(&c, &f, json!({ "entry": "sew/hearth", "id": "catalog" })).unwrap_err();
    assert_eq!(e.code, codes::APP_ID_RESERVED);
    let e = dry(&c, &f, json!({ "entry": "sew/hearth", "id": "a-very-long-install-id-past-thirty-two" })).unwrap_err();
    assert_eq!(e.code, codes::APP_ID_INVALID, "the guarded engine's 32");
    let v = dry(&c, &f, json!({ "entry": "sew/hearth", "id": "community" })).unwrap();
    assert_eq!(v["plan"]["app"], "community");
}

/// Switch 1 is needed (the broad switch is named), and an agent acts for
/// its own workspace only.
#[test]
fn an_agent_needs_build_and_run_apps() {
    let fx = fx();
    let (c, f) = hearth_cat(&fx);
    set_switches(false, false);
    let e = install_with(&agent(), &json!({ "entry": "sew/hearth" }), &c, &facts(), &f).unwrap_err();
    assert_eq!(e.code, codes::APPS_MANAGE_OFF);
    assert!(e.hint.contains("Build and run Apps"), "{}", e.hint);
    set_switches(true, false);
    f.manage_sew.store(false, Ordering::SeqCst);
    let v = install_with(&agent(), &json!({ "entry": "sew/hearth", "dryRun": true }), &c, &facts(), &f).unwrap();
    assert_eq!(v["plan"]["workspace"], WS);
    let w = &v["plan"]["waits"][0];
    assert_eq!(w["reason"], jobs::WAIT_SEW_RIGHTS, "the dry run names the wait ahead: {v}");
    assert!(w["text"].as_str().unwrap().contains("Manage chat and voice"), "{w}");
    assert_eq!(w["hint"], format!("k2 agent access set {WS} chat_manage on"), "{w}");
    assert!(w.get("question").is_none(), "no open question is ever named: {w}");
    assert_plain(&v);
}

/// A15: the route shapes, POST-only writes, and every route answers
/// through `routes::handle` (with the compiled catalog: nothing pinned).
#[test]
fn the_routes_are_in_the_table_and_writes_are_post_only() {
    for p in ROUTES {
        assert!(super::super::routes::is_route(p), "{p}");
    }
    for p in [INSTALL, RETRY] {
        assert!(super::super::routes::is_post_route(p), "{p}");
    }
    for p in [LIST, SHOW, CHECK_ADDRESS, JOB] {
        assert!(!super::super::routes::is_post_route(p), "{p} is a read");
    }
    for p in ROUTES {
        assert!(crate::routes::route_policy::lookup(p).is_some(), "{p} has a route_policy row");
    }
    let fx = fx();
    let _ = &fx;
    let call = |path: &str, params: &[(&str, &str)]| {
        let p: HashMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let r = super::super::routes::handle(path, Who::Owner, b"", &p);
        (r.status, serde_json::from_str::<J>(&r.body).unwrap())
    };
    let (st, v) = call(LIST, &[]);
    assert_eq!(st, "200 OK", "{v}");
    let hearth = v["entries"].as_array().unwrap().iter().find(|e| e["entry"] == "sew/hearth").unwrap().clone();
    assert_eq!(hearth["canDeploy"], false, "nothing pinned yet");
    assert!(hearth["reasons"].as_array().unwrap().iter().any(|r| r["code"] == sc::NOT_PINNED), "{hearth}");
    assert_eq!(hearth["defaultAddress"]["label"], "chat");
    let (st, v) = call(SHOW, &[("entry", "sew/meet")]);
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!((v["installId"].as_str(), v["onePerBox"].as_bool()), (Some("sew-meet"), Some(true)));
    let (st, _) = call(SHOW, &[("entry", "sew/none")]);
    assert_eq!(st, "404 Not Found");
    let (st, v) = call(CHECK_ADDRESS, &[("entry", "sew/hearth"), ("address", "team"), ("workspace", WS)]);
    assert_eq!(st, "200 OK", "{v}");
    assert_eq!(v["address"]["label"], "team");
    let (st, v) = call(CHECK_ADDRESS, &[("entry", "sew/hearth"), ("address", "skin"), ("workspace", WS)]);
    assert_eq!((st, v["ok"].as_bool(), v["error"]["code"].as_str()), ("200 OK", Some(false), Some(sc::ADDRESS_RESERVED)));
    let (st, v) = call(JOB, &[("id", "sj-nope")]);
    assert_eq!(st, "404 Not Found", "{v}");
}

/// A failed job whose step can be retried goes on from that step; any
/// other state is `not_retryable`.
#[test]
fn retry_resumes_a_retryable_failure_only() {
    let fx = fx();
    let (c, f) = hearth_cat(&fx);
    *f.fetch_err.lock().unwrap() = Some(crate::sew::fetch::FetchError::Unreachable("github.com timed out".into()));
    let v = install_with(&Who::Owner, &json!({ "entry": "sew/hearth", "workspace": WS }), &c, &facts(), &f).unwrap();
    let id = v["job"]["id"].as_str().unwrap().to_string();
    let j = super::super::catalog_job::advance(&f, &id).unwrap();
    assert_eq!(j.error_code.as_deref(), Some("download_failed"));
    let v = retry(&Who::Owner, &json!({ "job": id })).unwrap();
    assert_eq!(v["job"]["state"], "running");
    *f.fetch_err.lock().unwrap() = None;
    let j = super::super::catalog_job::advance(&f, &id).unwrap();
    assert_eq!(j.state, jobs::STATE_DONE, "{j:?}");
    assert_eq!(retry(&Who::Owner, &json!({ "job": id })).unwrap_err().code, "not_retryable");
}

/// The badge: "Verified by K2" only on an entry with a pinned, published
/// version whose files K2 checks by sha256. The compiled catalog pins
/// nothing yet, so it shows no badge (`verified: null`).
#[test]
fn verified_by_k2_shows_only_on_a_pinned_published_version() {
    let fx = fx();
    let (c, _f) = hearth_cat(&fx);
    let v = list(&Who::Owner, &c, &facts()).unwrap();
    assert_eq!(v["entries"][0]["verified"], "Verified by K2", "{v}");
    assert!(c.entries[0].verified());
    // The same entry with nothing pinned: no badge.
    let mut bare = c.clone();
    bare.entries[0].versions.clear();
    assert!(!bare.entries[0].verified());
    let v = list(&Who::Owner, &bare, &facts()).unwrap();
    assert_eq!(v["entries"][0]["verified"], J::Null, "{v}");
    // And the compiled catalog today (Hearth and Meet not published yet).
    let real = k2_core::suite::catalog().unwrap();
    let v = list(&Who::Owner, real, &facts()).unwrap();
    for e in v["entries"].as_array().unwrap() {
        assert!(e["version"].is_null(), "{e}");
        assert_eq!(e["verified"], J::Null, "an unpublished entry shows no badge: {e}");
    }
}

/// A catalog reason is a plain sentence for people plus, apart, the
/// command for agents; nothing a person reads names a command.
#[test]
fn a_reason_is_a_sentence_and_its_command_apart() {
    let fx = fx();
    let (c, f) = hearth_cat(&fx);
    let mut fa = facts();
    fa.sew = Err(Why::new("sew_not_installed", "Needs chat hosting on this server: turn it on in Settings → Cores → Chat / Voice", Some("k2 hostchat enable")));
    fa.pro = Some(false);
    fa.linux = false;
    let v = list(&Who::Owner, &c, &fa).unwrap();
    let reasons = v["entries"][0]["reasons"].as_array().unwrap().clone();
    let codes: Vec<&str> = reasons.iter().map(|r| r["code"].as_str().unwrap()).collect();
    assert_eq!(codes, vec!["needs_linux", "sew_not_installed", sc::PRO_REQUIRED], "{v}");
    for r in &reasons {
        let text = r["text"].as_str().unwrap();
        assert!(!text.contains("k2 ") && !text.contains("(Sew)"), "a command or jargon in the people's words: {text}");
    }
    assert_eq!(reasons[1]["hint"], "k2 hostchat enable");
    assert_eq!(reasons[0]["hint"], J::Null);
    assert_plain(&v);
    // The same reason as a refusal: the CLI's hint keeps the command, and
    // the plain sentence rides along.
    let e = install_with(&Who::Owner, &json!({ "entry": "sew/hearth", "workspace": WS }), &c, &Facts { linux: true, ..fa }, &f).unwrap_err();
    assert_eq!(e.code, "sew_not_installed");
    assert!(e.hint.contains("k2 hostchat enable"), "{}", e.hint);
    assert_eq!(e.extra.unwrap()["text"], "Needs chat hosting on this server: turn it on in Settings → Cores → Chat / Voice");
}

/// What deploying approves, in words about what the App may then do; no
/// "client API", no "guarded wall", and each line says why.
#[test]
fn what_deploying_approves_is_in_plain_words() {
    let fx = fx();
    let (c, f) = hearth_cat(&fx);
    let v = dry(&c, &f, json!({ "entry": "sew/hearth" })).unwrap();
    let approves = v["plan"]["approves"].as_array().unwrap();
    assert_eq!(approves.len(), 2 + testkit::K2_CAPS.len(), "{v}");
    for a in approves {
        let why = a["why"].as_str().unwrap_or_else(|| panic!("every line says why: {a}"));
        assert!(why.starts_with("This App") || why.starts_with("People signed in to this App"), "{why}");
        for jargon in ["client API", "signalling", "guarded", "wall", "Seed", "Sew"] {
            assert!(!why.contains(jargon), "{jargon} in {why}");
        }
    }
    let sew_api = approves.iter().find(|a| a["name"] == "sew-api").unwrap();
    assert_eq!(sew_api["why"], "This App can talk to the chat service on this server, for chat");
    for line in v["plan"]["willDo"].as_array().unwrap() {
        let line = line.as_str().unwrap();
        assert!(!line.contains("guarded wall") && !line.contains("Sew's"), "{line}");
    }
    assert_plain(&v);
}

/// No catalog, address check or install answer ever carries an internal
/// reference (a review id, a PRD name, an open question), whatever state
/// it is in: the compiled catalog, a pinned one, a taken address, a dry
/// run, a started job, a failed one.
#[test]
fn no_catalog_or_install_answer_carries_an_internal_reference() {
    let fx = fx();
    let call = |path: &str, params: &[(&str, &str)]| {
        let p: HashMap<String, String> = params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let r = super::super::routes::handle(path, Who::Owner, b"", &p);
        serde_json::from_str::<J>(&r.body).unwrap()
    };
    for v in [
        call(LIST, &[]),
        call(SHOW, &[("entry", "sew/hearth")]),
        call(SHOW, &[("entry", "sew/meet")]),
        call(CHECK_ADDRESS, &[("entry", "sew/hearth"), ("address", "skin"), ("workspace", WS)]),
    ] {
        assert_plain(&v);
    }
    let (c, f) = hearth_cat(&fx);
    assert_plain(&list(&Who::Owner, &c, &facts()).unwrap());
    let mut show_params = HashMap::new();
    show_params.insert("entry".to_string(), "sew/hearth".to_string());
    assert_plain(&show(&Who::Owner, &c, &show_params, &facts()).unwrap());
    assert_plain(&dry(&c, &f, json!({ "entry": "sew/hearth" })).unwrap());
    // A failed download: a sentence, and the retry command apart.
    *f.fetch_err.lock().unwrap() = Some(crate::sew::fetch::FetchError::Unreachable("github.com timed out".into()));
    let v = install_with(&Who::Owner, &json!({ "entry": "sew/hearth", "workspace": WS }), &c, &facts(), &f).unwrap();
    assert_plain(&v);
    let id = v["job"]["id"].as_str().unwrap().to_string();
    let j = super::super::catalog_job::advance(&f, &id).unwrap();
    let jv = super::super::catalog_job::job_view(&j);
    assert_eq!(jv["error"]["code"], "download_failed");
    let text = jv["error"]["text"].as_str().unwrap();
    assert!(!text.contains("k2 ") && !text.contains(&id), "{text}");
    assert_eq!(jv["error"]["hint"], format!("k2 app catalog retry {id}"));
    assert_eq!(jv["error"]["retry"], true);
    assert_plain(&jv);
    let mut jp = HashMap::new();
    jp.insert("id".to_string(), id.clone());
    assert_plain(&job_route(&Who::Owner, &jp).unwrap());
    assert_plain(&list(&Who::Owner, &c, &facts()).unwrap());
    // A taken address: the plain sentence for people, the command for the CLI.
    installed("hearth");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        jobs::label_bind(&conn, &jobs::SuiteLabel { label: "chat".into(), app_id: "hearth".into(), front_port: 40999, bound_at: 1, bound_by: "owner".into() }).unwrap();
    }
    let mut p = HashMap::new();
    p.insert("entry".to_string(), "sew/hearth".to_string());
    p.insert("address".to_string(), "chat".to_string());
    p.insert("workspace".to_string(), WS.to_string());
    let v = check_address(&Who::Owner, &c, &p, &facts()).unwrap();
    assert_eq!(v["error"]["code"], sc::ADDRESS_TAKEN, "{v}");
    let text = v["error"]["text"].as_str().unwrap();
    assert!(!text.contains("k2 ") && !text.contains("--address"), "{text}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("--address auto"), "{v}");
    assert_plain(&v);
}
