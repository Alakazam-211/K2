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
//!   keeps `@1` (prd-zen-garden-sync-defaults-v1 GS16, GS45a). There is no
//!   grant to carry (no permissions since 2026-10-08).
//! - A **Garden file** must name a built-in with its version
//!   (`widget = "k2:diary@1"`). `k2 zen widget new my-diary --from k2:diary`
//!   (no version) copies the latest version into a user folder to edit.

use std::fmt;

/// The Diary widget (UWB21, UWB24; Rosson 2026-10-07), as first released.
pub const DIARY_WIDGET: &str = "k2:diary@1";

/// The Diary every new Diary Garden uses (Rosson 2026-10-08, for 0.45.2):
/// @1 plus your sent words staying dark ink, markdown built as nodes, and a
/// ghost teasing in the empty pen. `diary-2.toml` moves the template to it.
pub const DIARY_WIDGET_LATEST: &str = "k2:diary@2";

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
pub const BUILTIN_WIDGETS: &[BuiltinWidget] = &[
    BuiltinWidget {
        name: "diary",
        version: 1,
        files: &[
            ("manifest.json", include_str!("diary/manifest.json")),
            ("index.html", include_str!("diary/index.html")),
            ("diary.css", include_str!("diary/diary.css")),
            ("diary.js", include_str!("diary/diary.js")),
        ],
    },
    BuiltinWidget {
        name: "diary",
        version: 2,
        files: &[
            ("manifest.json", include_str!("diary-2/manifest.json")),
            ("index.html", include_str!("diary-2/index.html")),
            ("diary.css", include_str!("diary-2/diary.css")),
            ("diary.js", include_str!("diary-2/diary.js")),
        ],
    },
];

/// The sha256 of each released built-in's files, `name\0bytes\0` in list
/// order (TUWB6). A changed byte fails `released_builtins_never_change`:
/// ship a new version instead.
pub const BUILTIN_WIDGET_HASHES: &[(&str, &str)] = &[
    ("k2:diary@1", "d1b03245b1dbed5d01c6498be2b5c8626084fa8b594f8a3f5d3a4ae69348166d"),
    ("k2:diary@2", "a38b8bb61324f22d6803d3dc4de38138f913337e90082bcf2b4a9fa3269ec07f"),
];

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

    /// Every released Diary version, oldest first.
    fn diaries() -> Vec<&'static BuiltinWidget> {
        let all: Vec<&BuiltinWidget> = BUILTIN_WIDGETS.iter().filter(|w| w.name == "diary").collect();
        assert_eq!(all.iter().map(|w| w.id()).collect::<Vec<_>>(), [DIARY_WIDGET, DIARY_WIDGET_LATEST]);
        all
    }

    fn file(w: &BuiltinWidget, n: &str) -> &'static str {
        w.files.iter().find(|(f, _)| *f == n).map(|(_, b)| *b).unwrap_or_else(|| panic!("{} has {n}", w.id()))
    }

    #[test]
    fn the_diary_is_a_clean_custom_widget() {
        for w in diaries() {
            clean_diary(w);
        }
        assert_eq!(builtin_widget(&parse_widget_ref("k2:diary").expect("parses")).map(|w| w.id()).as_deref(), Some(DIARY_WIDGET_LATEST));
    }

    fn clean_diary(w: &'static BuiltinWidget) {
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
        // @2 adds PixiJS (the paper under the words) and Tone.js (the room's
        // music), Rosson 2026-10-08.
        let want: &[(&str, &str)] = if w.id() == DIARY_WIDGET {
            &[("perfect-freehand", "1"), ("font-caveat", "5")]
        } else {
            &[("perfect-freehand", "1"), ("font-caveat", "5"), ("pixi.js", "8"), ("tone", "15")]
        };
        let want: Vec<(String, String)> = want.iter().map(|(i, v)| (i.to_string(), v.to_string())).collect();
        assert_eq!(named, want, "{} names its libraries by major version", w.id());
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
        assert!(code <= 256 * 1024, "UW8: {} is {code} bytes of code", w.id());
        // Rosson 2026-10-08: one page per agent on THIS computer. K2 binds
        // the grant to `{server: "local"}`; the page list keeps only
        // `<handle>::local` rows as a second lock.
        assert!(js.contains("var LOCAL_HOST = 'local'") && js.contains(".filter(isLocal)"), "the Diary keeps local agents only");
        // Pages turn by drag, click and keys; reduced motion stills it all.
        for needle in ["'pointerdown'", "'pointermove'", "'ArrowRight'", "'ArrowLeft'", "'PageDown'", "rotateY("] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        let css = file("diary.css");
        assert!(css.contains("@media (prefers-reduced-motion: reduce)") && css.contains(":root.reduced"), "reduced motion");
        assert!(js.contains("if (reduced()) {"), "a reduced-motion turn is instant");
        // Nothing but haunted controls on the page: no agent chooser, no
        // theme or model buttons.
        for bad in ["id=\"search\"", "id=\"back\"", "Contents", "theme.get", "theme.changed"] {
            assert!(!html.contains(bad) && !js.contains(bad), "the Diary still has {bad}");
        }
    }

    /// k2:diary@2 (Rosson 2026-10-08): what you sent stays dark ink, words
    /// read as markdown built as nodes, and the ghost in the empty pen only
    /// ever teases for fears, secrets, names and memories, never anything
    /// real and sensitive (its words go to an agent).
    #[test]
    fn diary_2_keeps_your_ink_reads_markdown_and_its_ghost_asks_for_nothing_real() {
        let w = builtin_widget(&parse_widget_ref(DIARY_WIDGET_LATEST).expect("parses")).expect("k2:diary@2 ships");
        let (js, css, html) = (file(w, "diary.js"), file(w, "diary.css"), file(w, "index.html"));
        // Your words: no rule fades, blurs or slants them.
        let mine: Vec<&str> = css.split('}').filter(|r| r.split('{').next().is_some_and(|sel| sel.contains(".entry.mine"))).collect();
        assert!(!mine.is_empty(), "diary.css styles your words");
        for rule in mine {
            for bad in ["opacity", "blur", "filter", "italic"] {
                assert!(!rule.contains(bad), "your sent words fade again ({bad}): {rule}");
            }
        }
        assert!(!js.contains("absorbed"), "nothing marks sent words to fade");
        // Markdown is built node by node (the clean check refuses innerHTML).
        for needle in ["function markdown(", "createTreeWalker", "'md-link'", "'md-url'"] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        assert!(!js.contains(".href") && !js.contains("'href'") && !js.contains("setAttribute('on"), "a link is words, never a target");
        // The ghost: a placeholder over the pen, never its value.
        assert!(html.contains("id=\"ghost\" aria-hidden=\"true\" hidden"), "the ghost is hidden from readers");
        assert!(!js.contains("ink.value = GHOST") && !js.contains("ink.value = words.charAt"), "the ghost never writes into the textarea");
        for needle in ["'visibilitychange'", "ghostFreeze()", "if (reduced()) {", "GHOST_STILL", "if (!ghost.woke) ghostStart(false)"] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        // No plain "write here" anywhere, and no buttons on the page but its
        // corners (Rosson 2026-10-08: no seal, no whispers switch).
        assert!(!html.contains("placeholder=") && !js.contains(".placeholder = '") && !html.contains("id=\"hint\""), "a plain prompt is back");
        assert!(html.contains("aria-label=\"Write in the diary\""), "the pen keeps a name for screen readers");
        assert!(!html.contains("id=\"send\"") && !html.contains("id=\"whispers\"") && !js.contains("AudioContext"), "seal and whispers are gone");
        // The pen scratches while the agent works: SVG built in code, each
        // stroke drawn by its dash offset; the page glides, one write a frame.
        for needle in ["createElementNS(SVG_NS, 'path')", "strokeDashoffset", "function glideStep(", "Math.exp(-dt / GLIDE_TAU_MS)"] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        assert!(!js.contains("scrollIntoView"), "the page glides; it never jumps to a line");
        // Ghost counts (0.45.2's AgentRow.counts) are read only when K2 sends
        // them; an older K2's row shows nothing extra.
        assert!(js.contains("function counted(row)") && js.contains("if (!c || typeof c !== 'object') return null"), "counts are feature-detected");
        assert!(js.contains("clamp(q[0], SCRIBE_PAD, SCRIBE_W - SCRIBE_PAD)"), "scribble points stay inside the drawing");
        // Ink, paper and sound (Rosson 2026-10-08). The working scribble is
        // the old wild stroked pen ("more scary/chaotic"); perfect-freehand
        // draws only the flourish, the blots and the speaker.
        for needle in ["function inkOutline(", "simulatePressure: o.pressure !== false", "function frustration()", "p.style.strokeDashoffset = String(len)"] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        assert!(!js.contains("SCRIBE_INK") && !js.contains("scribe-hand-"), "the scribble went back to filled outlines");
        // The paper: one WebGL canvas under the words, drawn on demand (30
        // fps at most while ink is wet), halted when unseen, one still frame
        // under reduced motion; the candle steps with the CSS flicker.
        for needle in ["window.PIXI", "preference: 'webgl'", "autoStart: false", "autoDensity: true", "dom.page.insertBefore(canvas, dom.page.firstChild)", "var r = dom.page.getBoundingClientRect()", "new window.ResizeObserver(fxResized)"] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        // One flame drives every candle (the room's, the page glow, the
        // shader's light), sampled once a frame; no CSS clock of its own.
        for needle in ["function flameAt(ms)", "var FLAME_FPS = 20", "dom.candle.style.opacity", "dom.glow.style.opacity", "u.uLight[3] = light", "var on = shown() && !reduced()"] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        assert!(!css.contains("@keyframes flicker") && !css.contains("@keyframes gutter"), "a candle runs on its own CSS clock again");
        // The music: made in code, quiet, on arrival, gone when unseen or
        // removed, muted from the top left or with M; never an AudioWorklet
        // node (refused in a sealed widget).
        for needle in ["window.Tone", "var MUSIC_DB = -24", "music.arrived = true", "'pagehide', musicClose", "raw.suspend()", "(e.key === 'm' || e.key === 'M') && e.target !== dom.ink"] {
            assert!(js.contains(needle), "diary.js lost {needle}");
        }
        for bad in ["Freeverb", "JCReverb", "FeedbackCombFilter", "BitCrusher", "Tone.start", "Player(", ".mp3", ".ogg", ".wav"] {
            assert!(!js.contains(bad), "the music uses {bad}");
        }
        assert!(html.contains("<button class=\"hush\" id=\"hush\" type=\"button\" aria-label=\"Mute the music (M)\" aria-pressed=\"false\">"), "the speaker: named and always shown");
        assert!(js.contains("function musicState()") && js.contains("'blocked by the browser'") && js.contains("'waiting for a click'"), "the speaker says what the music is doing");
        let hush = css.split("\n.hush {").nth(1).and_then(|r| r.split('}').next()).expect(".hush rule");
        assert!(hush.contains("top: 58px;") && hush.contains("left: 14px;") && !hush.contains("right:"), "the speaker sits top left, under K2's floating top band");
        assert!(hush.contains("border-radius: 999px;") && hush.contains("background: var(--zen-surface"), "the speaker is a Zen glass tile");
        // Corners curl on reach; no dog-ears; square pages.
        assert!(js.contains("function curlSvg(name)") && !css.contains("var(--night) 0 50%"), "the corners are dog-eared again");
        assert!(css.contains(".haunt") && css.contains(":root.asleep .haunt *") && css.contains(":root.reduced .haunt *"), "the room moves, stills and sleeps");
        let start = js.find("var GHOST_LINES = [").expect("GHOST_LINES");
        let end = start + js[start..].find("\n  ]").expect("GHOST_LINES ends");
        let lines: Vec<String> = js[start..end].lines().skip(1).filter_map(|l| l.trim().strip_prefix('\'')?.strip_suffix("',").map(str::to_string)).collect();
        assert!((15..=20).contains(&lines.len()), "15 to 20 teases, got {}: {lines:?}", lines.len());
        let words = ["pin", "pins", "card", "cards", "code", "codes", "ssn", "cvv", "cvc", "otp", "iban", "key", "keys", "bank", "token", "account", "accounts", "zip"];
        let parts = ["password", "passcode", "passphrase", "login", "log in", "sign in", "credential", "credit", "debit", "social security",
            "routing", "address", "email", "e-mail", "phone", "birthday", "date of birth", "maiden", "security question", "username",
            "wallet", "seed phrase", "recovery", "verification", "postcode", "license", "passport"];
        for l in &lines {
            let lower = l.to_lowercase();
            for p in parts {
                assert!(!lower.contains(p), "the ghost asks for something real ({p}): {l}");
            }
            for t in lower.split(|c: char| !c.is_alphanumeric()) {
                assert!(!words.contains(&t), "the ghost asks for something real ({t}): {l}");
            }
        }
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
        for (id, v) in [("perfect-freehand", "1"), ("font-caveat", "5"), ("pixi.js", "8"), ("tone", "15")] {
            let r = LibRef::Named { id: id.into(), version: v.into() };
            let e = find(m, &r).unwrap_or_else(|| panic!("the Diary's {id}@{v} isn't in zen-lib.json"));
            assert_eq!(e.source, LibSource::Bundled, "{id}@{v}");
        }
    }
}
