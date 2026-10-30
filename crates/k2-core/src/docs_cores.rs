//! The multiplayer cores a box can switch on or off (prd-multiplayer-cores-settings-v1
//! §2.1): Settings → Cores → Multiplayer, `k2 doc cores`.
//!
//! Plain data. k2-core holds it so [`crate::app_settings::update`] can refuse an
//! unknown core; the daemon maps a file to its core by name (`docs::cores`).
//!
//! The switch lives in `AppSettings.multiplayer_cores` (wire `multiplayerCores`,
//! e.g. `{"markdown": true}`). **A missing key means off**: a box that updates
//! starts with every core off, and while a core is off the daemon opens no live
//! doc of that kind (`core_off`).

/// What a person sees for a core (the multiplayer view, or none yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreView {
    /// Files of this kind open in the multiplayer view while the core is on.
    Live,
    /// No multiplayer view for people yet: they open the file the usual way;
    /// agents use `k2 doc`.
    None,
}

impl CoreView {
    pub fn wire(self) -> &'static str {
        match self {
            CoreView::Live => "live",
            CoreView::None => "none",
        }
    }
}

/// One core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Core {
    /// The wire id (`multiplayerCores` key, `k2 doc cores on <id>`).
    pub id: &'static str,
    pub name: &'static str,
    /// The status pill: `beta`.
    pub status: &'static str,
    pub description: &'static str,
    /// The CLI's one-line summary (`k2 doc cores`).
    pub short: &'static str,
    pub view: CoreView,
}

pub const MARKDOWN: &str = "markdown";
pub const TEXT: &str = "text";
pub const ANY_FILE: &str = "any-file";

/// Every core, in the order Settings and the CLI list them.
pub const CORES: &[Core] = &[
    Core {
        id: MARKDOWN,
        name: "Markdown core",
        status: "beta",
        description: ".md and .markdown files. Opens beside the workspace's agent, with live editing, presence and history.",
        short: ".md and .markdown files",
        view: CoreView::Live,
    },
    Core {
        id: TEXT,
        name: "Text and code core",
        status: "beta",
        description: "Code, config, plain text, logs, HTML/XML/SVG and diagram sources, and CSV/TSV as text.",
        short: "code, config, plain text, logs, CSV/TSV as text",
        view: CoreView::Live,
    },
    Core {
        id: ANY_FILE,
        name: "Any other file",
        status: "beta",
        description: "Images, PDFs, office files and the rest: presence, a lease, versions and restore. People still open these the usual way for now; agents use k2 doc.",
        short: "presence, a lease, versions and restore (agents only for now)",
        view: CoreView::None,
    },
];

/// Cores that are planned, shown as plain text ("Coming later").
pub const LATER: &[&str] = &["CSV grid", "Word"];

/// The core with this id.
pub fn by_id(id: &str) -> Option<&'static Core> {
    CORES.iter().find(|c| c.id == id)
}

/// `markdown, text, any-file`, for refusals.
pub fn ids_list() -> String {
    CORES.iter().map(|c| c.id).collect::<Vec<_>>().join(", ")
}

/// The settings key (wire name).
pub const SETTING_KEY: &str = "multiplayerCores";

/// Check a `multiplayerCores` value from a settings write: an object of known
/// core ids to booleans. The refusal names the first bad entry; nothing is
/// written on a refusal.
pub fn validate_partial(v: &serde_json::Value) -> Result<(), String> {
    let Some(obj) = v.as_object() else {
        return Err(format!("{SETTING_KEY} must be an object of core ids to true or false (got {v})"));
    };
    for (k, val) in obj {
        if by_id(k).is_none() {
            return Err(format!("{SETTING_KEY}.{k}: no such core ({})", ids_list()));
        }
        if !val.is_boolean() {
            return Err(format!("{SETTING_KEY}.{k} must be true or false (got {val})"));
        }
    }
    Ok(())
}

/// Whether `id` is on in a stored map (a missing key is off).
pub fn is_on(map: &std::collections::BTreeMap<String, bool>, id: &str) -> bool {
    map.get(id).copied().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn three_cores_in_order() {
        let ids: Vec<&str> = CORES.iter().map(|c| c.id).collect();
        assert_eq!(ids, ["markdown", "text", "any-file"]);
        assert_eq!(by_id("any-file").expect("any-file").view, CoreView::None);
        assert_eq!(by_id("markdown").expect("markdown").view, CoreView::Live);
        assert!(by_id("word").is_none());
        assert_eq!(ids_list(), "markdown, text, any-file");
    }

    #[test]
    fn a_write_names_known_cores_with_booleans_only() {
        validate_partial(&json!({"markdown": true, "text": false, "any-file": true})).expect("ok");
        validate_partial(&json!({})).expect("empty is fine");
        assert_eq!(
            validate_partial(&json!({"word": true})).expect_err("unknown"),
            "multiplayerCores.word: no such core (markdown, text, any-file)"
        );
        assert_eq!(
            validate_partial(&json!({"markdown": "yes"})).expect_err("not a bool"),
            "multiplayerCores.markdown must be true or false (got \"yes\")"
        );
        assert!(validate_partial(&json!(true)).is_err());
        assert!(validate_partial(&json!(["markdown"])).is_err());
    }
}
