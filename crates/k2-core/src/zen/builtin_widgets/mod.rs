//! Built-in custom widgets: `k2:<name>@<n>` (prd-zen-user-widgets-v2
//! UWB21, UWB26).
//!
//! **Day-0 interface (Zen v2, 2026-10-08). Owner: B1** (the sources and
//! [`BUILTIN_WIDGETS`]); B2 reads [`WidgetRef`] in the Garden schema and the
//! bundler. Frozen: the name rule below and the `k2:diary@1` name.
//!
//! **The name rule.**
//! - A **user widget** is a folder name under `~/.k2/zen/widgets/`: the theme
//!   name rule (`valid_theme_name`: lower-case letters, digits, `-`, `_`,
//!   starting with a letter or digit, up to 40). It can never contain `:`.
//! - A **built-in widget** is `k2:<name>@<n>`: `<name>` follows the same rule,
//!   `<n>` is a version from 1 with no leading zero. It is compiled into
//!   k2-core (`include_str!`), read-only, never a folder.
//! - **Immutable per version.** Once a release ships `k2:diary@1`, its bytes
//!   never change (TUWB6 pins them by hash). Better code ships as
//!   `k2:diary@2`, and only a template moves to it, so an own-copy Garden
//!   keeps `@1` (prd-zen-garden-sync-defaults-v1 GS16, GS45a); a synced
//!   Garden's grant carries to `@2` (SD7, `zen::grants::carries`).
//! - A **Garden file** must name a built-in with its version
//!   (`widget = "k2:diary@1"`). `k2 zen widget new my-diary --from k2:diary`
//!   (no version) copies the latest version into a user folder to edit.

use std::fmt;

/// The Diary widget (UWB21, UWB24; Rosson 2026-10-07).
pub const DIARY_WIDGET: &str = "k2:diary@1";

/// The prefix every built-in widget name starts with.
pub const BUILTIN_PREFIX: &str = "k2:";

/// One built-in widget version: the same flat file set a user folder has.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinWidget {
    pub name: &'static str,
    pub version: u32,
    /// `(file name, bytes)`: `manifest.json`, the entry HTML, `*.js`,
    /// `*.css`. Text only: images and fonts come from the standard library
    /// (`requires.libs`) or are inlined in CSS as `data:` URLs.
    pub files: &'static [(&'static str, &'static str)],
}

impl BuiltinWidget {
    /// `k2:<name>@<version>`.
    pub fn id(&self) -> String {
        format!("{BUILTIN_PREFIX}{}@{}", self.name, self.version)
    }
}

/// Every built-in widget version K2 ships, oldest first. An entry is never
/// removed or edited once released: better code is a new version with its
/// own folder (`diary-2/`), and only a template moves to it.
pub const BUILTIN_WIDGETS: &[BuiltinWidget] = &[BuiltinWidget {
    name: "diary",
    version: 1,
    files: &[
        ("manifest.json", include_str!("diary/manifest.json")),
        ("index.html", include_str!("diary/index.html")),
        ("diary.css", include_str!("diary/diary.css")),
        ("diary.js", include_str!("diary/diary.js")),
    ],
}];

/// The sha256 of each released built-in's files, `name\0bytes\0` in list
/// order (TUWB6). A changed byte fails `released_builtins_never_change`:
/// ship a new version instead.
pub const BUILTIN_WIDGET_HASHES: &[(&str, &str)] =
    &[("k2:diary@1", "c4f6e5a4e86937b29f429887f0ad451eaee854e08432ce67d2cac6b301cd9e64")];

/// The hash [`BUILTIN_WIDGET_HASHES`] pins.
pub fn builtin_widget_hash(w: &BuiltinWidget) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for (name, bytes) in w.files {
        h.update(name.as_bytes());
        h.update([0u8]);
        h.update(bytes.as_bytes());
        h.update([0u8]);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A parsed `widget = "…"` value or `--from` argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetRef {
    /// A folder under `~/.k2/zen/widgets/`.
    User(String),
    /// `k2:<name>@<n>`, or `k2:<name>` (latest; only valid for `--from`).
    Builtin { name: String, version: Option<u32> },
}

impl fmt::Display for WidgetRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WidgetRef::User(n) => f.write_str(n),
            WidgetRef::Builtin { name, version: Some(v) } => write!(f, "{BUILTIN_PREFIX}{name}@{v}"),
            WidgetRef::Builtin { name, version: None } => write!(f, "{BUILTIN_PREFIX}{name}"),
        }
    }
}

/// Parse a widget reference. Errors are one plain sentence.
pub fn parse_widget_ref(s: &str) -> Result<WidgetRef, String> {
    let Some(rest) = s.strip_prefix(BUILTIN_PREFIX) else {
        if super::valid_theme_name(s) {
            return Ok(WidgetRef::User(s.to_string()));
        }
        return Err(format!(
            "'{s}' isn't a widget name: lower-case letters, digits, - and _, up to 40, or a built-in like {DIARY_WIDGET}"
        ));
    };
    let (name, version) = match rest.split_once('@') {
        None => (rest, None),
        Some((n, v)) => {
            let ok = !v.is_empty() && !v.starts_with('0') && v.chars().all(|c| c.is_ascii_digit()) && v.len() <= 6;
            let v: u32 = if ok { v.parse().map_err(|_| format!("bad version in '{s}'"))? } else { 0 };
            if v == 0 {
                return Err(format!("'{s}': a built-in widget version is a whole number from 1, like {DIARY_WIDGET}"));
            }
            (n, Some(v))
        }
    };
    if !super::valid_theme_name(name) {
        return Err(format!("'{s}' isn't a built-in widget name; built-ins look like {DIARY_WIDGET}"));
    }
    Ok(WidgetRef::Builtin { name: name.to_string(), version })
}

/// The built-in for a reference: an exact version, or the latest when
/// `version` is `None`. `None` for a user widget or an unknown built-in.
pub fn builtin_widget(r: &WidgetRef) -> Option<&'static BuiltinWidget> {
    let WidgetRef::Builtin { name, version } = r else { return None };
    BUILTIN_WIDGETS
        .iter()
        .filter(|w| w.name == name && version.is_none_or(|v| w.version == v))
        .max_by_key(|w| w.version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_and_builtin_names_parse() {
        assert_eq!(parse_widget_ref("agent-arcade"), Ok(WidgetRef::User("agent-arcade".into())));
        assert_eq!(
            parse_widget_ref(DIARY_WIDGET),
            Ok(WidgetRef::Builtin { name: "diary".into(), version: Some(1) })
        );
        assert_eq!(parse_widget_ref("k2:diary"), Ok(WidgetRef::Builtin { name: "diary".into(), version: None }));
        assert_eq!(parse_widget_ref("k2:diary@12").map(|r| r.to_string()), Ok("k2:diary@12".into()));
    }

    #[test]
    fn bad_names_are_refused() {
        for bad in ["", "Diary", "k2:", "k2:Diary@1", "k2:diary@", "k2:diary@0", "k2:diary@01", "k2:diary@x",
            "k2:diary@1@2", "x:diary@1", "../diary", "k2:diary@1234567"]
        {
            assert!(parse_widget_ref(bad).is_err(), "'{bad}' must be refused");
        }
    }

    #[test]
    fn released_builtins_are_unique_and_named_by_the_rule() {
        for (i, w) in BUILTIN_WIDGETS.iter().enumerate() {
            assert!(w.version >= 1);
            assert_eq!(parse_widget_ref(&w.id()), Ok(WidgetRef::Builtin { name: w.name.into(), version: Some(w.version) }));
            assert!(
                !BUILTIN_WIDGETS[..i].iter().any(|o| o.name == w.name && o.version == w.version),
                "{} listed twice",
                w.id()
            );
            assert!(w.files.iter().any(|(f, _)| *f == "manifest.json"), "{} has a manifest", w.id());
        }
    }

    /// TUWB6: a released built-in's bytes are pinned. To change the Diary,
    /// add `k2:diary@2`; never edit `@1`.
    #[test]
    fn released_builtins_never_change() {
        for w in BUILTIN_WIDGETS {
            let id = w.id();
            let pinned = BUILTIN_WIDGET_HASHES
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, h)| *h)
                .unwrap_or_else(|| panic!("{id} has no pinned hash in BUILTIN_WIDGET_HASHES"));
            assert_eq!(builtin_widget_hash(w), pinned, "{id} changed; ship a new version instead");
        }
        assert_eq!(BUILTIN_WIDGET_HASHES.len(), BUILTIN_WIDGETS.len(), "one pin per built-in");
    }

    #[test]
    fn the_diary_is_a_clean_custom_widget() {
        let w = builtin_widget(&parse_widget_ref(DIARY_WIDGET).expect("parses")).expect("k2:diary@1 ships");
        assert_eq!(w.id(), DIARY_WIDGET);
        let file = |n: &str| w.files.iter().find(|(f, _)| *f == n).map(|(_, b)| *b).unwrap_or_else(|| panic!("diary has {n}"));
        let manifest: serde_json::Value = serde_json::from_str(file("manifest.json")).expect("manifest is JSON");
        assert_eq!(manifest["schema"], 1);
        assert_eq!(manifest["entry"], "index.html");
        let caps: Vec<&str> = manifest["caps"].as_array().expect("caps").iter().filter_map(|c| c.as_str()).collect();
        assert_eq!(caps, ["agents:read", "thread:read", "thread:post"]);
        for c in &caps {
            assert!(crate::zen::USER_WIDGET_CAPS.contains(c), "{c}");
            assert!(manifest["reasons"][*c].as_str().is_some_and(|r| r.len() <= 140), "a reason for {c}");
        }
        // Libraries from K2's standard library, named by major version so a
        // vendored minor bump never breaks an immutable built-in (UWB13,
        // UWB24: perfect-freehand and the Caveat handwriting font).
        let libs: Vec<crate::zen::stdlib::LibRef> = manifest["requires"]["libs"]
            .as_array()
            .expect("requires.libs")
            .iter()
            .map(|l| crate::zen::stdlib::LibRef::from_json(l).unwrap_or_else(|e| panic!("{e}")))
            .collect();
        let named: Vec<(String, String)> = libs
            .iter()
            .map(|l| match l {
                crate::zen::stdlib::LibRef::Named { id, version } => (id.clone(), version.clone()),
                other => panic!("a built-in never loads from a CDN: {other:?}"),
            })
            .collect();
        assert_eq!(named, [("perfect-freehand".to_string(), "1".to_string()), ("font-caveat".to_string(), "5".to_string())]);
        // The garden catalog grants exactly what the Diary asks for.
        if let Some(e) = crate::zen::garden_catalog::garden_catalog().iter().find(|e| e.meta.short == "diary") {
            let g = e.meta.grant.as_ref().expect("the Diary Garden grants in the create click");
            assert_eq!(g.widget, DIARY_WIDGET);
            assert_eq!(g.caps, caps);
        }
        let html = file("index.html");
        let js = file("diary.js");
        // UW10's errors: no inline handlers, no network, no frames.
        for bad in [" onclick=", " onload=", " onerror=", "<iframe", "<object", "<embed", "<base", "http-equiv", "src=\"http", "href=\"http", "//cdn"] {
            assert!(!html.contains(bad), "index.html has {bad}");
        }
        assert!(html.contains("<script src=\"diary.js\"></script>") && html.contains("href=\"diary.css\""));
        // UW10's warnings and the skill's rules: textContent, never innerHTML.
        for bad in ["innerHTML", "outerHTML", "insertAdjacentHTML", "document.write", "fetch(", "XMLHttpRequest", "WebSocket", "EventSource", "localStorage", "sessionStorage", "indexedDB", "eval(", "new Function", "import "] {
            assert!(!js.contains(bad), "diary.js uses {bad}");
        }
        // Only widget helpers the catalog exposes.
        let widget: Vec<String> = crate::contract::catalog()
            .verbs_exposed_to(crate::contract::Exposure::Widget)
            .iter()
            .map(|v| v.verb.clone())
            .collect();
        for (i, _) in js.match_indices("k2.") {
            let name: String = js[i + 3..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '.').collect();
            let name = name.trim_end_matches('.');
            let ok = widget.iter().any(|w| w == name)
                || crate::contract::gen::RUNTIME_MEMBERS.contains(&name)
                || ["thread", "agents", "theme", "conversation", "motion.reduced"].contains(&name);
            assert!(ok, "diary.js calls k2.{name}, which isn't a widget helper");
        }
        let code: usize = w.files.iter().filter(|(f, _)| *f != "manifest.json").map(|(_, b)| b.len()).sum();
        assert!(code <= 256 * 1024, "UW8: the Diary's code is {code} bytes");
    }

    /// Integration (Zen v2, B1 × B3): every library a built-in widget asks
    /// for resolves against the checked-in `zen-lib.json`, and is bundled,
    /// so a built-in works offline and under air-gap with no first-use
    /// download. The Diary's `perfect-freehand@1` and `font-caveat@5` are
    /// pinned by name so a rename in the stdlib fails here, not at mount.
    #[test]
    fn builtin_widget_libraries_resolve_in_zen_lib_json() {
        use crate::zen::stdlib::{find, zen_lib_manifest, LibRef, LibSource};
        let m = zen_lib_manifest();
        for w in BUILTIN_WIDGETS {
            let manifest = w
                .files
                .iter()
                .find(|(f, _)| *f == "manifest.json")
                .map(|(_, b)| *b)
                .unwrap_or_else(|| panic!("{} has a manifest.json", w.id()));
            let manifest: serde_json::Value =
                serde_json::from_str(manifest).unwrap_or_else(|e| panic!("{}: manifest.json: {e}", w.id()));
            let Some(libs) = manifest.get("requires").and_then(|r| r.get("libs")) else { continue };
            let libs = libs.as_array().unwrap_or_else(|| panic!("{}: requires.libs is a list", w.id()));
            for l in libs {
                let r = LibRef::from_json(l).unwrap_or_else(|e| panic!("{}: {e}", w.id()));
                let e = find(m, &r).unwrap_or_else(|| panic!("{}: {l} isn't in src/shared/zen-lib.json", w.id()));
                assert_eq!(e.source, LibSource::Bundled, "{}: {l} must be bundled, not a download", w.id());
                assert!(!e.files.is_empty(), "{}: {l} resolves to a row with no files", w.id());
            }
        }
        builtin_widget(&parse_widget_ref(DIARY_WIDGET).expect("parses")).expect("k2:diary@1 ships");
        for (id, v) in [("perfect-freehand", "1"), ("font-caveat", "5")] {
            let r = LibRef::Named { id: id.into(), version: v.into() };
            let e = find(m, &r).unwrap_or_else(|| panic!("the Diary's {id}@{v} isn't in zen-lib.json"));
            assert_eq!(e.source, LibSource::Bundled, "{id}@{v}");
        }
    }
}
