//! The helper's accessibility JSON → the core's [`RawNode`]s. iOS roles
//! arrive as the macOS AX roles the translator produces (`AXButton`,
//! `AXTextField` + subrole `AXSecureTextField`, …). The core assigns ids,
//! filters, renders and diffs (`sim::tree`).

use serde_json::Value as J;

use super::simctl::{SimError, SimResult};
use crate::sim::tree::{RawNode, Rect, Role};

/// AX role (+ subrole) → role.
pub fn role_of(role: &str, subrole: Option<&str>) -> Role {
    match (role, subrole) {
        (_, Some("AXSwitch")) | (_, Some("AXToggle")) => Role::Switch,
        (_, Some("AXTabButton")) => Role::Tab,
        ("AXButton", _) | ("AXPopUpButton", _) | ("AXMenuButton", _) | ("AXIncrementor", _) | ("AXDisclosureTriangle", _) => Role::Button,
        ("AXLink", _) => Role::Link,
        ("AXTextField", _) | ("AXTextArea", _) | ("AXSearchField", _) | ("AXComboBox", _) => Role::Textfield,
        ("AXCheckBox", _) => Role::Checkbox,
        ("AXSwitch", _) | ("AXToggle", _) => Role::Switch,
        ("AXRadioButton", _) => Role::Radio,
        ("AXSlider", _) => Role::Slider,
        ("AXTab", _) => Role::Tab,
        ("AXCell", _) | ("AXRow", _) => Role::Cell,
        ("AXMenu", _) => Role::Menu,
        ("AXMenuItem", _) => Role::MenuItem,
        ("AXStaticText", _) | ("AXHeading", _) => Role::Text,
        ("AXImage", _) => Role::Image,
        ("AXToolbar", _) | ("AXTabGroup", _) => Role::Toolbar,
        ("AXList", _) | ("AXTable", _) | ("AXCollection", _) | ("AXOutline", _) => Role::List,
        ("AXWebArea", _) => Role::Webview,
        ("AXGroup", _) | ("AXScrollArea", _) | ("AXGenericElement", _) => Role::Group,
        ("AXApplication", _) | ("AXWindow", _) => Role::Window,
        _ => Role::Other,
    }
}

/// An iOS text field's AX value, with "the placeholder standing in for
/// nothing" read as empty (seen live: an empty `TextField("Email")`
/// reports value "Email").
pub fn effective_value(value: Option<String>, placeholder: Option<&str>) -> Option<String> {
    match (value, placeholder) {
        (Some(v), Some(p)) if v == p => None,
        (v, _) => v.filter(|v| !v.is_empty()),
    }
}

fn node(v: &J, depth: usize) -> SimResult<RawNode> {
    if depth > 80 {
        return Err(SimError::new("tree_too_deep", "accessibility tree deeper than 80 levels"));
    }
    let s = |k: &str| v.get(k).and_then(J::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    let b = |k: &str| v.get(k).and_then(J::as_bool).unwrap_or(false);
    let ax_role = s("role").unwrap_or_default();
    let subrole = s("subrole");
    let f = v
        .get("frame")
        .and_then(J::as_array)
        .filter(|a| a.len() == 4)
        .ok_or_else(|| SimError::new("helper_output", format!("element {ax_role} has no frame")))?;
    let n = |i: usize| f[i].as_f64().unwrap_or(0.0) as f32;
    let role = role_of(&ax_role, subrole.as_deref());
    let secure = subrole.as_deref() == Some("AXSecureTextField");
    let placeholder = s("placeholder");
    let value = if secure { None } else { effective_value(s("value"), placeholder.as_deref()) };
    let checked = match role {
        Role::Switch | Role::Checkbox => value.as_deref().map(|v| v == "1" || v.eq_ignore_ascii_case("on")),
        _ => None,
    };
    let children = v
        .get("children")
        .and_then(J::as_array)
        .map(|c| c.iter().map(|c| node(c, depth + 1)).collect::<SimResult<Vec<_>>>())
        .transpose()?
        .unwrap_or_default();
    Ok(RawNode {
        role: Some(role),
        native: match &subrole {
            Some(sr) => format!("{ax_role}/{sr}"),
            None => ax_role,
        },
        // An empty field shows its placeholder as the label when it has none.
        label: s("label").or(if role == Role::Textfield { placeholder } else { None }),
        value: if checked.is_some() { None } else { value },
        identifier: s("identifier"),
        frame: Some(Rect { x: n(0), y: n(1), w: n(2), h: n(3) }),
        clickable: matches!(role, Role::Button | Role::Link | Role::Cell | Role::Tab | Role::MenuItem | Role::Switch | Role::Checkbox | Role::Radio),
        focusable: role == Role::Textfield,
        scrollable: false,
        enabled: v.get("enabled").and_then(J::as_bool).unwrap_or(true),
        focused: b("focused"),
        selected: b("selected"),
        checked,
        secure,
        children,
    })
}

/// Parses the helper's `tree` reply body (`{"tree": {...}}`) into the root node.
pub fn from_helper(reply: &J) -> SimResult<RawNode> {
    let t = reply.get("tree").ok_or_else(|| SimError::new("helper_output", "tree reply has no `tree`"))?;
    node(t, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roles_map_to_the_core_roles() {
        assert_eq!(role_of("AXButton", None), Role::Button);
        assert_eq!(role_of("AXTextField", None), Role::Textfield);
        assert_eq!(role_of("AXTextField", Some("AXSecureTextField")), Role::Textfield);
        assert_eq!(role_of("AXCheckBox", Some("AXSwitch")), Role::Switch);
        assert_eq!(role_of("AXRadioButton", Some("AXTabButton")), Role::Tab);
        assert_eq!(role_of("AXStaticText", None), Role::Text);
        assert_eq!(role_of("AXApplication", None), Role::Window);
        assert_eq!(role_of("AXSomethingNew", None), Role::Other);
    }

    #[test]
    fn the_fixture_tree_parses_and_hides_secure_values() {
        // Shape captured from the helper against the K2 fixture app (L-I spike).
        let reply = json!({"tree": {"role": "AXApplication", "label": "K2Fixture", "frame": [0, 0, 402, 874], "enabled": true,
            "children": [
                {"role": "AXStaticText", "label": "K2 Fixture", "identifier": "title", "frame": [140.2, 114.3, 121.7, 33.7], "enabled": true},
                {"role": "AXTextField", "identifier": "email", "placeholder": "Email", "value": "a@b.c", "frame": [40.3, 245.3, 321.3, 34], "enabled": true, "focused": true},
                {"role": "AXTextField", "subrole": "AXSecureTextField", "identifier": "pw", "value": "•••••", "frame": [40, 300, 321, 34], "enabled": true},
                {"role": "AXButton", "label": "Save", "identifier": "save", "frame": [182.7, 383, 37, 20], "enabled": false},
                {"role": "AXCheckBox", "subrole": "AXSwitch", "label": "Notifications", "value": "1", "frame": [0, 500, 50, 30]}
            ]}});
        let r = from_helper(&reply).unwrap();
        assert_eq!(r.role(), Role::Window);
        assert_eq!(r.children.len(), 5);
        let email = &r.children[1];
        assert_eq!((email.role(), email.value.as_deref(), email.focused, email.focusable), (Role::Textfield, Some("a@b.c"), true, true));
        let pw = &r.children[2];
        assert!(pw.secure);
        assert_eq!(pw.value, None);
        assert_eq!(pw.native, "AXTextField/AXSecureTextField");
        let save = &r.children[3];
        assert!(!save.enabled && save.clickable);
        assert_eq!(save.frame.unwrap().center().x, 182.7 + 18.5);
        let sw = &r.children[4];
        assert_eq!((sw.role(), sw.checked, sw.value.as_deref()), (Role::Switch, Some(true), None));
    }

    #[test]
    fn a_placeholder_standing_in_for_an_empty_value_is_no_value() {
        let reply = json!({"tree": {"role": "AXTextField", "value": "Email", "placeholder": "Email", "frame": [0, 0, 1, 1]}});
        let n = from_helper(&reply).unwrap();
        assert_eq!(n.value, None);
        assert_eq!(n.label.as_deref(), Some("Email"));
        assert_eq!(effective_value(Some("a".into()), Some("Email")), Some("a".into()));
        assert_eq!(effective_value(Some(String::new()), None), None);
    }

    #[test]
    fn an_element_without_a_frame_is_a_loud_error() {
        assert_eq!(from_helper(&json!({"tree": {"role": "AXApplication"}})).unwrap_err().code, "helper_output");
        assert_eq!(from_helper(&json!({})).unwrap_err().code, "helper_output");
    }
}
