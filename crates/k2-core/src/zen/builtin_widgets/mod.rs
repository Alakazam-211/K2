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
//!   `k2:diary@2`, and only a template moves to it, so a Garden pinned by
//!   look versions keeps `@1` (GU4, GU8, GU14).
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

/// Every built-in widget version K2 ships, oldest first. B1 adds
/// `diary` v1 (`builtin_widgets/diary/`) when its source lands; an entry is
/// never removed or edited once released.
pub const BUILTIN_WIDGETS: &[BuiltinWidget] = &[];

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
}
