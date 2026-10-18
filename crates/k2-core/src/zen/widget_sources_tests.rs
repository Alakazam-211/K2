//! Garden v3 data sources, the Zen side (contract-sources-v1 §6, §11.4,
//! §12.3): the `sources` manifest key, the reach note (0.46.1; was GV48's taint), run reports
//! without text (GV49), placement addresses, `locked_placement_sources`
//! (GV16) and `k2 zen widget show`. Every Zen folder is under the temp dir.

use super::*;
use crate::zen::widget_access::WidgetPauses;

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn set_up(tag: &str) -> (Tmp, ZenFiles) {
    let d = std::env::temp_dir().join(format!("k2-zen-sources-{tag}-{}", uuid::Uuid::new_v4().simple()));
    let f = ZenFiles::new(d.join(".k2").join("zen"));
    assert!(f.root().starts_with(std::env::temp_dir()), "tests never touch the real Zen folder");
    f.setup().expect("setup");
    (Tmp(d), f)
}

fn gid(f: &ZenFiles, name: &str) -> String {
    f.find_garden(name).unwrap_or_else(|| panic!("no Garden {name}")).1.id
}

fn write_garden(f: &ZenFiles, id: &str, text: &str) {
    fs::write(f.path_of(&ZenFile::Garden(id.into())), text).expect("write garden");
}

/// A widget folder `name` asking for `caps`, with `sources` (raw JSON).
fn source_widget(f: &ZenFiles, name: &str, caps: &str, sources: &str) {
    if !f.widget_dir(name).is_dir() {
        f.new_widget(name, Some("hello")).expect("new widget");
    }
    let extra = if sources.is_empty() { String::new() } else { format!(", \"sources\": {sources}") };
    let m = format!("{{\"schema\": 1, \"name\": \"{name}\", \"caps\": {caps}{extra}}}");
    fs::write(f.widget_dir(name).join("manifest.json"), m).expect("write manifest");
}

fn source_page(widget: &str, config: &str) -> String {
    format!(
        "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n[[widget]]\nid = \"card\"\nkind = \"custom\"\nwidget = \"{widget}\"\ncolumn = 0\n[widget.props.config]\n{config}\n"
    )
}

/// GV49: a widget that reads sources keeps class, code and line only.
#[test]
fn a_sources_widget_run_report_keeps_no_text() {
    let (_t, f) = set_up("gv49");
    source_widget(&f, "fleet-card", "[\"sources:read\"]", "\"config\"");
    let h = "b".repeat(64);
    let detail = runs::RunDetail {
        message: Some("TypeError: 4242.00 USD is not a function"),
        stack: Some("at about:srcdoc:9:2 secret-ish"),
        kind: Some("error"),
        class: Some("TypeError"),
        code: None,
        line: Some(9),
        address: None,
    };
    f.record_widget_run_with("fleet-card", &h, "error", &detail).expect("error");
    let refusal = runs::RunDetail { kind: Some("refusal"), class: Some("sources.read"), code: Some("not_bound"), ..Default::default() };
    f.record_widget_run_with("fleet-card", &h, "error", &refusal).expect("refusal");
    let e = f.widget_errors("fleet-card", None).expect("errors");
    let text = e.to_string();
    assert!(!text.contains("4242") && !text.contains("secret-ish"), "no message or stack text: {e}");
    assert_eq!(e["errors"][0]["class"], "TypeError");
    assert_eq!(e["errors"][0]["line"], 9);
    assert_eq!(e["errors"][0]["redacted"], true);
    assert!(e["errors"][0].get("message").is_none() && e["errors"][0].get("stack").is_none(), "{e}");
    assert_eq!((e["errors"][1]["kind"].as_str(), e["errors"][1]["code"].as_str()), (Some("refusal"), Some("not_bound")));
    assert!(e["errors"][1]["suggestion"].as_str().is_some_and(|s| s.contains("config.source")), "{e}");
    let on_disk = fs::read_to_string(f.widget_history_dir("fleet-card").join("runtime.jsonl")).expect("log");
    assert!(!on_disk.contains("4242") && !on_disk.contains("secret-ish"), "never written: {on_disk}");

    // A widget that doesn't read sources keeps its text (and the fixed fields).
    f.new_widget("plain", Some("hello")).expect("plain");
    f.record_widget_run_with("plain", &h, "error", &detail).expect("plain error");
    let p = f.widget_errors("plain", None).expect("errors");
    assert!(p["errors"][0]["message"].as_str().is_some_and(|m| m.contains("4242")), "{p}");
    assert_eq!(p["errors"][0]["class"], "TypeError");

    // Bad fixed fields are refused.
    let bad = runs::RunDetail { class: Some("not a class"), ..Default::default() };
    assert!(matches!(f.record_widget_run_with("plain", &h, "error", &bad), Err(ZenError::BadRequest(_))));
    let bad = runs::RunDetail { kind: Some("crash"), ..Default::default() };
    assert!(matches!(f.record_widget_run_with("plain", &h, "error", &bad), Err(ZenError::BadRequest(_))));
}

/// 0.46.1 S3: a refused `sources.*` call names its address in `widget
/// errors` only when a placement's config (or the manifest) names it.
#[test]
fn a_refusal_names_a_config_address_and_withholds_any_other() {
    let (_t, f) = set_up("refused-address");
    source_widget(&f, "fleet-edge", "[\"sources:read\"]", "\"config\"");
    let g = gid(&f, "Garden 2");
    write_garden(&f, &g, &source_page("fleet-edge", "sources = \"fleet, k2-ops-edge\""));
    assert_eq!(f.configured_source_addresses("fleet-edge"), vec!["fleet".to_string(), "k2-ops-edge".to_string()]);
    let h = "c".repeat(64);
    let named = runs::RunDetail {
        kind: Some("refusal"),
        class: Some("sources.subscribe"),
        code: Some("not_bound"),
        address: Some("K2-Ops-Edge"),
        ..Default::default()
    };
    f.record_widget_run_with("fleet-edge", &h, "error", &named).expect("named refusal");
    // An address no placement names could carry a value read from a source.
    let other = runs::RunDetail { address: Some("eur-4242"), ..named.clone() };
    f.record_widget_run_with("fleet-edge", &h, "error", &other).expect("other refusal");
    let e = f.widget_errors("fleet-edge", None).expect("errors");
    let rows = e["errors"].as_array().expect("rows");
    assert_eq!(rows[0]["address"], "k2-ops-edge", "{e}");
    assert!(runs::run_text(&rows[0]).contains("(not_bound) for k2-ops-edge"), "{}", runs::run_text(&rows[0]));
    assert!(rows[0]["suggestion"].as_str().is_some_and(|s| s.contains("named in this widget's placement config")), "{e}");
    assert!(rows[1].get("address").is_none(), "{e}");
    assert_eq!(rows[1]["addressWithheld"], true, "{e}");
    assert!(rows[1]["suggestion"].as_str().is_some_and(|s| s.contains("add it to the placement's config.sources")), "{e}");
    assert!(!e.to_string().contains("4242"), "the unnamed address never lands: {e}");
    let on_disk = fs::read_to_string(f.widget_history_dir("fleet-edge").join("runtime.jsonl")).expect("log");
    assert!(!on_disk.contains("4242"), "{on_disk}");
    // A bad shape is withheld too.
    let odd = runs::RunDetail { address: Some("fleet; rm -rf"), ..named.clone() };
    f.record_widget_run_with("fleet-edge", &h, "error", &odd).expect("odd");
    let e = f.widget_errors("fleet-edge", Some(2)).expect("errors");
    assert_eq!(e["errors"][0]["addressWithheld"], true, "{e}");
}

/// Manifest `sources`: needs the cap, takes "config" or names, and keeps
/// thread:post with a reach note (0.46.1, Rosson 2026-10-17).
#[test]
fn manifest_sources_need_the_cap_and_keep_thread_post_with_a_note() {
    let (_t, f) = set_up("manifest");
    source_widget(&f, "no-cap", "[\"agents:read\"]", "\"config\"");
    let v = f.validate_widget("no-cap").expect("validate");
    assert_eq!(v["ok"], false, "{v}");
    assert!(v.to_string().contains("sources_needs_cap"), "{v}");

    source_widget(&f, "both", "[\"thread:post\", \"sources:read\"]", "[\"fleet\", \"deploys\"]");
    let v = f.validate_widget("both").expect("validate");
    assert_eq!(v["ok"], true, "{v}");
    assert!(v["warnings"].to_string().contains("sources_reach_thread"), "{v}");
    let st = f.widget_status("both");
    assert_eq!(st.manifest.as_ref().and_then(|m| m.sources.clone()), Some(json!(["fleet", "deploys"])));
    assert_eq!(
        effective_caps(WidgetOrigin::of("both"), &st.requested()),
        vec!["thread:post".to_string(), "sources:read".to_string()]
    );

    for (bad, why) in [
        ("[\"fleet::akzm.k2.dev\"]", "no ::host in a manifest"),
        ("[\"Fleet\"]", "bad name"),
        ("\"everything\"", "not config"),
        ("[1]", "not a string"),
    ] {
        source_widget(&f, "bad", "[\"sources:read\"]", bad);
        let v = f.validate_widget("bad").expect("validate");
        assert_eq!(v["ok"], false, "{why}: {v}");
    }
    // net and secrets stay refused.
    f.new_widget("net", Some("hello")).expect("net");
    fs::write(f.widget_dir("net").join("manifest.json"), "{\"schema\": 1, \"name\": \"n\", \"net\": [\"x.test\"]}").expect("w");
    assert_eq!(f.validate_widget("net").expect("validate")["ok"], false);
}

/// The resolved page carries the manifest's sources and the bound addresses;
/// a bad address in a placement's config is a Garden finding.
#[test]
fn placements_carry_source_addresses_and_bad_ones_are_findings() {
    let (_t, f) = set_up("placement");
    source_widget(&f, "fleet-card", "[\"sources:read\", \"thread:post\"]", "\"config\"");
    let g = gid(&f, "Garden 2");
    write_garden(
        &f,
        &g,
        &source_page("fleet-card", "source = \"fleet::akzm.k2.dev\"\nsources = \"deploys, money\"\nlatest = \"release\"\ntitle = \"Boxes\""),
    );
    let page = f.resolve(Some(&g)).expect("resolve");
    let w = &page["page"]["widgets"][0];
    assert_eq!(w["sources"], "config", "{w}");
    assert_eq!(w["sourceAddresses"], json!(["fleet::akzm.k2.dev", "deploys", "money", "release"]), "{w}");
    assert_eq!(w["caps"], json!(["sources:read"]), "thread:post dropped: {w}");

    write_garden(&f, &g, &source_page("fleet-card", "source = \"Fleet::https://x\""));
    let v = f.validate(Some(&ZenFile::Garden(g.clone()))).expect("validate garden");
    assert!(v.to_string().contains("isn't a source address"), "{v}");
}

/// The binding follows the folder's current manifest whenever it is clean:
/// a brand-new widget binds its placement config before it has ever run
/// (no last good yet), and a new version with a changed source list binds
/// the new list, not the last good one's.
#[test]
fn bound_addresses_follow_the_current_manifest_not_last_good() {
    let (_t, f) = set_up("current");
    source_widget(&f, "alerts", "[\"sources:read\"]", "\"config\"");
    let g = gid(&f, "Garden 2");
    write_garden(&f, &g, &source_page("alerts", "source = \"k2-ops-watch\"\nsources = \"k2-ops-billing\""));
    assert!(f.widget_status("alerts").last_good_at.is_none(), "never validated or run: no last good");
    let page = f.resolve(Some(&g)).expect("resolve");
    assert_eq!(page["page"]["widgets"][0]["sourceAddresses"], json!(["k2-ops-watch", "k2-ops-billing"]), "{page}");

    // Fixed names, kept as last good; then the folder names others.
    source_widget(&f, "alerts", "[\"sources:read\"]", "[\"money\"]");
    f.refresh_widget("alerts").expect("refresh");
    assert!(f.widget_status("alerts").last_good_at.is_some(), "refresh kept a last good");
    source_widget(&f, "alerts", "[\"sources:read\"]", "[\"fleet\", \"deploys\"]");
    let page = f.resolve(Some(&g)).expect("resolve");
    assert_eq!(page["page"]["widgets"][0]["sourceAddresses"], json!(["fleet", "deploys"]), "{page}");
}

/// GV16: `locked_placement_sources` lists the bare and this-host names
/// that locked Gardens read, from their frozen copies.
#[test]
fn locked_gardens_report_the_sources_they_read() {
    let (_t, f) = set_up("locked");
    source_widget(&f, "fleet-card", "[\"sources:read\"]", "\"config\"");
    source_widget(&f, "fixed", "[\"sources:read\"]", "[\"money\"]");
    let g = gid(&f, "Garden 2");
    let page = format!(
        "{}\n[[widget]]\nid = \"fixed\"\nkind = \"custom\"\nwidget = \"fixed\"\ncolumn = 0\n",
        source_page("fleet-card", "sources = \"fleet, deploys::me.k2.dev, other::akzm.k2.dev\"")
    );
    write_garden(&f, &g, &page);
    assert!(f.locked_placement_sources_for(Some("me.k2.dev")).is_empty(), "nothing is locked yet");
    f.set_garden_lock(&g, true).expect("lock");
    let names: Vec<String> = f.locked_placement_sources_for(Some("me.k2.dev")).into_iter().collect();
    assert_eq!(names, vec!["deploys", "fleet", "money"]);
    let names: Vec<String> = f.locked_placement_sources_for(None).into_iter().collect();
    assert_eq!(names, vec!["fleet", "money"], "no Connect name: only bare addresses");
    f.set_garden_lock(&g, false).expect("unlock");
    assert!(f.locked_placement_sources_for(Some("me.k2.dev")).is_empty());
}

/// `k2 zen widget show`: placements, caps, the reach note and sources.
#[test]
fn widget_show_lists_caps_reach_and_sources() {
    let (_t, f) = set_up("show");
    source_widget(&f, "fleet-card", "[\"thread:post\", \"sources:read\"]", "\"config\"");
    let g = gid(&f, "Garden 2");
    write_garden(&f, &g, &source_page("fleet-card", "source = \"fleet::akzm.k2.dev\""));
    let s = f.widget_show("fleet-card", &WidgetPauses::empty()).expect("show");
    assert_eq!(s["caps"], json!(["thread:post", "sources:read"]), "{s}");
    assert_eq!(s["dropped"], json!([]), "{s}");
    assert!(s["reach"].as_str().is_some_and(|t| t.contains("post to the Thread")), "{s}");
    assert_eq!(s["sources"], "config");
    assert_eq!(s["placements"][0]["sources"], json!(["fleet::akzm.k2.dev"]), "{s}");
    assert_eq!(s["placements"][0]["locked"], false);
    assert!(matches!(f.widget_show("nope", &WidgetPauses::empty()), Err(ZenError::NotFound(_))));
}
