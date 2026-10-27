//! Tests for `widget_shot` (0.46.3): the daemon's frame is the renderer's
//! frame (the shared golden shape), themes resolve, a staged folder becomes
//! a shot page, and what can't be shot is refused. Fail loudly.

use super::*;

fn staged_builtin(name: &str, version: u32) -> BTreeMap<String, Vec<u8>> {
    let w = super::super::builtin_widgets::BUILTIN_WIDGETS
        .iter()
        .find(|w| w.name == name && w.version == version)
        .unwrap_or_else(|| panic!("no built-in {name}@{version}"));
    w.files.iter().map(|(n, t)| (n.to_string(), t.as_bytes().to_vec())).collect()
}

#[test]
fn the_daemon_frame_is_the_renderer_frame() {
    let nonce = "AAAAAAAAAAAAAAAAAAAAAA";
    let vars: BTreeMap<String, String> =
        [("--zen-canvas".to_string(), "#222222".to_string()), ("--zen-text".to_string(), "#111111".to_string())].into();
    let prelude = frame_prelude(nonce, &vars, Scheme::Dark, &["LIB_ONE".to_string(), "x</script>y".to_string()], false);
    let html = "<!DOCTYPE html>\n<html><body><p>shape</p><script nonce=\"AAAAAAAAAAAAAAAAAAAAAA\">k2.ready()</script></body></html>";
    let frame = frame_src_doc(html, nonce, &prelude)
        .replace(&script_text(SEALED_FRAME_GUARD), "{{GUARD}}")
        .replace(&script_text(K2_FRAME_JS), "{{K2FRAME}}");
    let golden = FRAME_SHAPE_GOLDEN.strip_suffix('\n').expect("the golden ends with a newline");
    let (want, features) = golden.split_once("\n<!-- features: ").expect("the golden's features line");
    assert_eq!(frame, want, "widget_shot's frame drifted from frameSrcDoc + zenFramePrelude (src/shared/zen-frame-shape.golden.html)");
    let features: Vec<&str> = features.strip_suffix(" -->").expect("features line").split(',').collect();
    assert_eq!(features, FRAME_FEATURES, "FRAME_FEATURES must be ZEN_FRAME_FEATURES");
}

#[test]
fn a_frame_without_a_doctype_gets_the_metas_first() {
    let d = frame_src_doc("<p>x</p>", "AAAAAAAAAAAAAAAAAAAAAA", "<i>pre</i>");
    assert!(d.starts_with(DNS_PREFETCH_OFF_META), "{d}");
    assert!(d.ends_with("<i>pre</i><p>x</p>"), "{d}");
    // `<!doctypex` is not a doctype.
    let d = frame_src_doc("<!doctypex><p>x</p>", "AAAAAAAAAAAAAAAAAAAAAA", "");
    assert!(d.starts_with(DNS_PREFETCH_OFF_META), "{d}");
}

#[test]
fn script_text_escapes_every_closing_script_in_any_case() {
    assert_eq!(script_text("a</script>b</SCRIPT >c"), "a<\\/script>b<\\/SCRIPT >c");
    assert_eq!(script_text("no tags"), "no tags");
}

#[test]
fn themes_resolve_over_basic_and_fixed_schemes_hold() {
    let (dark, s) = theme_vars("basic", Some(Scheme::Dark)).expect("basic dark");
    assert_eq!(s, Scheme::Dark);
    assert_eq!(dark.get("--zen-canvas").map(String::as_str), Some("#121316"));
    assert_eq!(dark.get("--zen-text").map(String::as_str), Some("#e6e8ec"));
    let (light, s) = theme_vars("basic", None).expect("basic");
    assert_eq!((s, light.get("--zen-canvas").map(String::as_str)), (Scheme::Light, Some("#f7f8fa")));
    // Glass is always dark: no scheme asked → dark, and its own radius.
    let (glass, s) = theme_vars("glass", None).expect("glass");
    assert_eq!(s, Scheme::Dark);
    assert_eq!(glass.get("--zen-radius").map(String::as_str), Some("22px"));
    assert!(glass.contains_key("--zen-font-family"), "{glass:?}");
    assert!(matches!(theme_vars("nope", None), Err(ZenError::BadRequest(m)) if m.contains("basic")));
}

#[test]
fn a_built_in_card_becomes_a_shot_page_and_writes_nothing() {
    let files = staged_builtin("fleet-grid", 2);
    let mut o = ShotOptions { scheme: Some(Scheme::Dark), ..ShotOptions::default() };
    o.config.insert("source".into(), "fleet".into());
    let v = shot_page("fleet-grid", &files, &o).expect("shot page");
    assert_eq!((v["ok"].as_bool(), v["scheme"].as_str(), v["width"].as_u64()), (Some(true), Some("dark"), Some(420)), "{v}");
    let nonce = v["nonce"].as_str().expect("nonce");
    let frame = v["frame"].as_str().expect("frame");
    assert!(frame.contains(&format!("'nonce-{nonce}'")), "the CSP names this nonce");
    assert!(frame.contains("color-scheme:dark!important"), "the Garden's scheme wins");
    assert!(frame.contains("--zen-canvas:#121316;"), "basic dark vars");
    assert!(frame.contains("K2Cards"), "the card kit is bundled in");
    assert!(!frame.contains("__K2_SHOT_LIB_"), "no libraries, no slots");
    let host = v["host"].as_str().expect("host");
    assert!(host.contains("__K2_SHOT_FRAME__"), "the runner fills the frame");
    assert!(host.contains("\"config\":{\"source\":\"fleet\"}"), "the placement config reaches the hello");
    assert!(host.contains("\"caps\":[\"sources:read\"]"), "{host}");
    assert!(host.contains("background:#121316"), "the Garden canvas behind the frame");
    assert!(host.contains("sandbox=\"allow-scripts\""), "sandboxed like a Zen frame");
    for slot in ["__K2_SHOT_HELLO__", "__K2_SHOT_THEME__", "__K2_SHOT_DATA__", "__K2_SHOT_SCHEME__", "__K2_SHOT_WIDTH__"] {
        assert!(!host.contains(slot), "{slot} filled");
    }
}

#[test]
fn a_bundled_library_is_a_slot_and_a_downloaded_one_is_refused() {
    let mut files = staged_builtin("fleet-grid", 2);
    let manifest = |libs: &str| {
        format!("{{\"schema\": 1, \"name\": \"Trend\", \"entry\": \"index.html\", \"caps\": [\"sources:read\"], \"sources\": \"config\", \"requires\": {{\"libs\": [{libs}]}}}}")
    };
    // 0.46.5: a line, resolved by the one resolver to the row K2 ships.
    files.insert("manifest.json".into(), manifest("\"uplot@1\"").into_bytes());
    let shipped = crate::zen::stdlib::resolve_one(crate::zen::stdlib::zen_lib_manifest(), "uplot", "1").expect("uplot line 1").0.key();
    let v = shot_page("trend", &files, &ShotOptions::default()).expect("shot page");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["libs"][0]["key"], json!(shipped), "{v}");
    assert_eq!(v["libs"][0]["slot"], "__K2_SHOT_LIB_0__");
    assert_eq!(v["libs"][0]["files"][0]["name"], "uPlot.iife.min.js");
    assert!(v["frame"].as_str().expect("frame").contains(">__K2_SHOT_LIB_0__</script>"));
    files.insert("manifest.json".into(), manifest("\"plotly.js-dist-min@4.1.1\"").into_bytes());
    assert!(matches!(shot_page("trend", &files, &ShotOptions::default()), Err(ZenError::BadRequest(m)) if m.contains("downloads on first use")));
}

#[test]
fn a_broken_folder_answers_its_errors_and_limits_are_checked() {
    let mut files = staged_builtin("fleet-grid", 2);
    files.insert("manifest.json".into(), b"{ not json".to_vec());
    let v = shot_page("fleet-grid", &files, &ShotOptions::default()).expect("answer");
    assert_eq!((v["ok"].as_bool(), v["status"].as_str()), (Some(false), Some("failed")), "{v}");
    assert!(v["errors"].as_array().is_some_and(|e| !e.is_empty()), "{v}");
    assert!(v.get("host").is_none(), "no page for a broken folder");
    let good = staged_builtin("fleet-grid", 2);
    for (w, h) in [(119, 300), (300, 2401)] {
        let o = ShotOptions { width: w, height: h, ..ShotOptions::default() };
        assert!(matches!(shot_page("fleet-grid", &good, &o), Err(ZenError::BadRequest(m)) if m.contains("--width")), "{w}x{h}");
    }
    let o = ShotOptions { data: Some(json!([1, 2])), ..ShotOptions::default() };
    assert!(matches!(shot_page("fleet-grid", &good, &o), Err(ZenError::BadRequest(m)) if m.contains("--data")));
}

#[test]
fn fixtures_cant_break_out_of_the_host_script() {
    let good = staged_builtin("fleet-grid", 2);
    let o = ShotOptions { data: Some(json!({"sources.subscribe": {"label": "</script><script>alert(1)</script>"}})), ..ShotOptions::default() };
    let v = shot_page("fleet-grid", &good, &o).expect("shot page");
    let host = v["host"].as_str().expect("host");
    assert!(!host.contains("</script><script>alert"), "fixture text stays inside its JSON");
    assert!(host.contains("\\u003c/script>"), "{host}");
    // No fixture or config value can spell a slot the runner fills.
    let mut o = ShotOptions { data: Some(json!({"x": "__K2_SHOT_FRAME__"})), ..ShotOptions::default() };
    o.config.insert("title".into(), "__K2_SHOT_DATA__".into());
    let v = shot_page("fleet-grid", &good, &o).expect("shot page");
    let host = v["host"].as_str().expect("host");
    assert_eq!(host.matches("__K2_SHOT_FRAME__").count(), 1, "only the template's own slot");
    assert!(!host.contains("__K2_SHOT_DATA__"));
}

#[test]
fn a_widget_with_its_own_scheme_keeps_it_like_in_zen() {
    let vars: BTreeMap<String, String> = [("--zen-text".to_string(), "#111".to_string())].into();
    let own = frame_prelude("AAAAAAAAAAAAAAAAAAAAAA", &vars, Scheme::Dark, &[], true);
    assert!(own.contains("color-scheme:dark;background"), "no !important: {own}");
    assert!(own.contains("setAttribute('data-zen-scheme', \"dark\");document.documentElement.setAttribute('data-k2-scheme', 'own');"));
    let mut files = staged_builtin("fleet-grid", 2);
    let m: J = serde_json::from_slice(&files["manifest.json"]).expect("manifest");
    let mut m = m.as_object().expect("object").clone();
    m.insert("colorScheme".into(), json!("own"));
    files.insert("manifest.json".into(), serde_json::to_vec(&m).expect("encode"));
    let v = shot_page("fleet-grid", &files, &ShotOptions::default()).expect("shot page");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["colorScheme"], "own", "{v}");
    assert!(!v["frame"].as_str().expect("frame").contains("!important"), "own scheme: no !important");
    let v = shot_page("fleet-grid", &staged_builtin("fleet-grid", 2), &ShotOptions::default()).expect("shot page");
    assert_eq!(v["colorScheme"], "garden");
}

/// The stand-in Thread speaks the real Thread view's units: the person's
/// own messages are `via: "compose"` (how k2-thread@1 and k2:kanban@3 tell
/// yours from the agent's) and a turn's `since` is in milliseconds (their
/// working clock is `Date.now() - since`). A shot once drew every bubble
/// on the agent's side and a 497,431-hour clock.
#[test]
fn the_sample_thread_uses_the_real_views_units() {
    let host = super::HOST_TEMPLATE;
    assert!(host.contains("via: from === 'you' ? 'compose' : null"), "own messages are via compose");
    assert!(host.contains("since: NOW * 1000 - 5000"), "a turn's since is in milliseconds");
    assert!(host.contains("var NOW = Math.floor(Date.now() / 1000)"), "NOW stays seconds (created_at)");
}
