//! One accessibility-tree format for every lane (`prd-mobile-dev-v1` §4,
//! M-A21).
//!
//! A lane reads its native tree (uiautomator XML, AX JSON) into
//! [`RawNode`]s. The core turns a [`RawTree`] into a [`Snapshot`]:
//!
//! - **Ids** `e1`, `e2`, … in document order, valid for **one snapshot
//!   only** ([`Snapshot::id`] is `t<n>`; a stale id fails loudly through
//!   [`Snapshot::resolve`]).
//! - **Interactive by default**: [`Snapshot::render`] prints interactive
//!   elements (and the containers that give them context) unless `all`.
//! - **A short diff** between two snapshots for the one-line action result
//!   (`+e9 text "Saved"`), by element identity, not by id.

use serde::{Deserialize, Serialize};

use super::lane::{Point, RawTree};

/// The element kinds every lane maps to. Unknown native classes become
/// [`Role::Other`] with the native name kept in [`RawNode::native`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Window,
    Group,
    Toolbar,
    Text,
    Image,
    Button,
    Link,
    Textfield,
    Switch,
    Checkbox,
    Radio,
    Slider,
    Tab,
    List,
    Cell,
    Menu,
    MenuItem,
    Webview,
    Other,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Window => "window",
            Role::Group => "group",
            Role::Toolbar => "toolbar",
            Role::Text => "text",
            Role::Image => "image",
            Role::Button => "button",
            Role::Link => "link",
            Role::Textfield => "textfield",
            Role::Switch => "switch",
            Role::Checkbox => "checkbox",
            Role::Radio => "radio",
            Role::Slider => "slider",
            Role::Tab => "tab",
            Role::List => "list",
            Role::Cell => "cell",
            Role::Menu => "menu",
            Role::MenuItem => "menuitem",
            Role::Webview => "webview",
            Role::Other => "other",
        }
    }

    /// Kinds a person or agent acts on even when the native tree doesn't
    /// mark them clickable.
    pub fn inherently_interactive(self) -> bool {
        matches!(
            self,
            Role::Button
                | Role::Link
                | Role::Textfield
                | Role::Switch
                | Role::Checkbox
                | Role::Radio
                | Role::Slider
                | Role::Tab
                | Role::MenuItem
        )
    }

    /// Kinds shown as context above interactive children.
    pub fn is_container_context(self) -> bool {
        matches!(self, Role::Toolbar | Role::List | Role::Menu | Role::Webview | Role::Window)
    }
}

/// A rectangle in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn center(&self) -> Point {
        Point { x: self.x + self.w / 2.0, y: self.y + self.h / 2.0 }
    }
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.x + self.w && p.y < self.y + self.h
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
}

/// One element as a lane read it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RawNode {
    pub role: Option<Role>,
    /// The native class / type (`android.widget.Button`, `XCUIElementTypeButton`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub native: String,
    /// Visible text or accessibility label (content-desc wins on Android
    /// when text is empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Current value (a text field's text, a slider's value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Resource id / accessibility identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    pub frame: Option<Rect>,
    #[serde(default)]
    pub clickable: bool,
    #[serde(default)]
    pub focusable: bool,
    #[serde(default)]
    pub scrollable: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub selected: bool,
    /// `Some(on)` for switches and checkboxes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    /// A password / secure field: its value is never shown.
    #[serde(default)]
    pub secure: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<RawNode>,
}

fn yes() -> bool {
    true
}

impl RawNode {
    pub fn role(&self) -> Role {
        self.role.unwrap_or(Role::Other)
    }
    pub fn interactive(&self) -> bool {
        self.enabled && (self.clickable || self.scrollable || self.role().inherently_interactive())
    }
}

/// One element in a snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Element {
    /// `e1`, `e2`, … (this snapshot only).
    pub id: String,
    /// Index of the parent element in [`Snapshot::elements`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<usize>,
    pub depth: u16,
    pub role: Role,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub native: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Never set for a secure field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<Rect>,
    pub interactive: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub selected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub scrollable: bool,
}

impl Element {
    /// Identity across snapshots (for the diff): role + identifier, else
    /// role + label, plus the frame rounded to 4 pt when neither exists.
    fn identity(&self) -> String {
        match (&self.identifier, &self.label) {
            (Some(i), _) if !i.is_empty() => format!("{}#{}", self.role.as_str(), i),
            (_, Some(l)) if !l.is_empty() => format!("{}'{}", self.role.as_str(), l),
            _ => {
                let f = self.frame.unwrap_or(Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 });
                format!(
                    "{}@{},{},{},{}",
                    self.role.as_str(),
                    (f.x / 4.0).round(),
                    (f.y / 4.0).round(),
                    (f.w / 4.0).round(),
                    (f.h / 4.0).round()
                )
            }
        }
    }

    /// `button "Save"`, `textfield "Email" value=""`, `switch "Wi-Fi" [on]`.
    pub fn describe(&self) -> String {
        let mut s = String::from(self.role.as_str());
        if let Some(l) = self.label.as_deref().filter(|l| !l.is_empty()) {
            s.push_str(&format!(" {}", quote(l)));
        } else if let Some(i) = self.identifier.as_deref().filter(|i| !i.is_empty()) {
            s.push_str(&format!(" #{}", short_identifier(i)));
        }
        if self.secure {
            s.push_str(" [secure]");
        } else if let Some(v) = &self.value {
            if self.role == Role::Textfield || self.role == Role::Slider {
                s.push_str(&format!(" value={}", quote(v)));
            }
        }
        if let Some(c) = self.checked {
            s.push_str(if c { " [on]" } else { " [off]" });
        }
        if !self.enabled {
            s.push_str(" [disabled]");
        }
        if self.focused {
            s.push_str(" [focused]");
        }
        if self.selected {
            s.push_str(" [selected]");
        }
        s
    }
}

/// Max characters of a label / value in rendered output.
pub const MAX_TEXT_CHARS: usize = 80;

fn quote(s: &str) -> String {
    let one_line: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut t: String = one_line.chars().take(MAX_TEXT_CHARS).collect();
    if one_line.chars().count() > MAX_TEXT_CHARS {
        t.push('…');
    }
    format!("\"{}\"", t.replace('"', "\\\""))
}

/// `com.acme:id/save_button` → `save_button`.
fn short_identifier(i: &str) -> &str {
    i.rsplit_once(":id/").map(|(_, r)| r).unwrap_or(i)
}

/// A numbered view of one screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// `t1`, `t2`, … per sim session.
    pub id: String,
    pub width_pt: f32,
    pub height_pt: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub elements: Vec<Element>,
    #[serde(default)]
    pub took_ms: u64,
}

/// Why an element id did not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// The id was taken from an older snapshot.
    Stale { given: String, current: String },
    /// No such id in this snapshot.
    Unknown { id: String },
    /// The element has no frame (can't be tapped).
    NoFrame { id: String },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::Stale { given, current } => write!(
                f,
                "element ids are from snapshot {given}; the screen is now snapshot {current}. Run `k2 sim tree` and use the new ids."
            ),
            ResolveError::Unknown { id } => write!(f, "no element {id} in this snapshot. Run `k2 sim tree` (or `--all`)."),
            ResolveError::NoFrame { id } => write!(f, "element {id} has no position on screen; tap with --at x,y instead."),
        }
    }
}

impl Snapshot {
    /// Number the lane's tree. `seq` is the session's snapshot counter.
    pub fn build(seq: u64, raw: &RawTree) -> Snapshot {
        let mut elements = Vec::new();
        fn walk(n: &RawNode, parent: Option<usize>, depth: u16, out: &mut Vec<Element>) {
            let idx = out.len();
            out.push(Element {
                id: format!("e{}", idx + 1),
                parent,
                depth,
                role: n.role(),
                native: n.native.clone(),
                label: n.label.clone().filter(|s| !s.trim().is_empty()),
                value: if n.secure { None } else { n.value.clone() },
                identifier: n.identifier.clone().filter(|s| !s.trim().is_empty()),
                frame: n.frame,
                interactive: n.interactive(),
                enabled: n.enabled,
                focused: n.focused,
                selected: n.selected,
                checked: n.checked,
                secure: n.secure,
                scrollable: n.scrollable,
            });
            for c in &n.children {
                walk(c, Some(idx), depth.saturating_add(1), out);
            }
        }
        for r in &raw.roots {
            walk(r, None, 0, &mut elements);
        }
        Snapshot {
            id: format!("t{seq}"),
            width_pt: raw.width_pt,
            height_pt: raw.height_pt,
            app_id: raw.app_id.clone(),
            title: raw.title.clone(),
            elements,
            took_ms: raw.took_ms,
        }
    }

    /// Look an id up. Accepts `e5` (this snapshot) or `t7:e5` /
    /// `t7.e5` (checked against this snapshot's id; stale fails).
    pub fn resolve(&self, id: &str) -> Result<&Element, ResolveError> {
        let (snap, eid) = match id.split_once([':', '.']) {
            Some((s, e)) if s.starts_with('t') => (Some(s), e),
            _ => (None, id),
        };
        if let Some(s) = snap {
            if s != self.id {
                return Err(ResolveError::Stale { given: s.to_string(), current: self.id.clone() });
            }
        }
        self.elements.iter().find(|e| e.id == eid).ok_or_else(|| ResolveError::Unknown { id: eid.to_string() })
    }

    /// The point to tap for an element.
    pub fn tap_point(&self, id: &str) -> Result<Point, ResolveError> {
        let e = self.resolve(id)?;
        match e.frame {
            Some(f) if !f.is_empty() => Ok(f.center()),
            _ => Err(ResolveError::NoFrame { id: e.id.clone() }),
        }
    }

    /// The deepest element whose frame contains `p` (picking, S5).
    pub fn hit(&self, p: Point) -> Option<&Element> {
        self.elements
            .iter()
            .filter(|e| e.frame.map(|f| !f.is_empty() && f.contains(p)).unwrap_or(false))
            .max_by_key(|e| e.depth)
    }

    /// Indices shown by default: interactive elements, plus the ancestors
    /// that are containers or carry a label (so a button reads "inside the
    /// toolbar 'Settings'").
    fn shown(&self, all: bool) -> Vec<bool> {
        let mut show = vec![all; self.elements.len()];
        if all {
            return show;
        }
        for (i, e) in self.elements.iter().enumerate() {
            let text_leaf = e.role == Role::Text && e.label.is_some();
            if e.interactive || text_leaf {
                show[i] = true;
                let mut p = e.parent;
                while let Some(pi) = p {
                    let pe = &self.elements[pi];
                    if pe.role.is_container_context() && (pe.label.is_some() || pe.role == Role::Toolbar) {
                        show[pi] = true;
                    }
                    p = pe.parent;
                }
            }
        }
        show
    }

    /// The text the agent reads (`k2 sim tree`). `find` keeps only
    /// elements whose label / value / identifier contains it (case-blind).
    pub fn render(&self, header: &str, all: bool, find: Option<&str>, boxes: bool) -> String {
        let show = self.shown(all || find.is_some());
        let needle = find.map(|f| f.to_lowercase());
        let mut out = String::new();
        out.push_str(header);
        out.push_str(&format!(" · snapshot {} · {}x{} pt\n", self.id, self.width_pt.round(), self.height_pt.round()));
        let min_depth = self
            .elements
            .iter()
            .zip(&show)
            .filter(|(_, s)| **s)
            .map(|(e, _)| e.depth)
            .min()
            .unwrap_or(0);
        let mut count = 0usize;
        for (e, s) in self.elements.iter().zip(&show) {
            if !*s {
                continue;
            }
            if let Some(n) = &needle {
                let hay = [&e.label, &e.value, &e.identifier]
                    .iter()
                    .filter_map(|v| v.as_deref())
                    .any(|v| v.to_lowercase().contains(n.as_str()));
                if !hay {
                    continue;
                }
            }
            count += 1;
            let indent = "  ".repeat(e.depth.saturating_sub(min_depth).min(12) as usize);
            out.push_str(&format!("{:<4}{}{}", e.id, indent, e.describe()));
            if boxes {
                if let Some(f) = e.frame {
                    out.push_str(&format!(" @{},{} {}x{}", f.x.round(), f.y.round(), f.w.round(), f.h.round()));
                }
            }
            out.push('\n');
        }
        let total = self.elements.len();
        if count < total {
            out.push_str(&format!("({count} of {total} elements; --all for the rest)\n"));
        }
        out
    }

    /// What changed from `before` to `self`, for the action line:
    /// `+e9 text "Saved"`, `-button "Save"`, `~e3 textfield "Email" value="a@b"`.
    /// At most `max` entries, plus `(+n more)`.
    pub fn diff(&self, before: &Snapshot, max: usize) -> Vec<String> {
        use std::collections::HashMap;
        let key = |e: &Element| e.identity();
        let mut old: HashMap<String, &Element> = HashMap::new();
        for e in &before.elements {
            old.entry(key(e)).or_insert(e);
        }
        let mut new_keys = std::collections::HashSet::new();
        let mut out = Vec::new();
        let worth = |e: &Element| e.interactive || (e.role == Role::Text && e.label.is_some());
        for e in &self.elements {
            let k = key(e);
            new_keys.insert(k.clone());
            if !worth(e) {
                continue;
            }
            match old.get(&k) {
                None => out.push(format!("+{} {}", e.id, e.describe())),
                Some(o) => {
                    if o.value != e.value || o.checked != e.checked || o.enabled != e.enabled || o.focused != e.focused || o.selected != e.selected {
                        out.push(format!("~{} {}", e.id, e.describe()));
                    }
                }
            }
        }
        for e in &before.elements {
            if worth(e) && !new_keys.contains(&key(e)) {
                out.push(format!("-{}", e.describe()));
            }
        }
        if out.len() > max {
            let more = out.len() - max;
            out.truncate(max);
            out.push(format!("(+{more} more)"));
        }
        if before.app_id != self.app_id {
            if let Some(a) = &self.app_id {
                out.insert(0, format!("app {a}"));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: Role, label: &str, clickable: bool, frame: (f32, f32, f32, f32)) -> RawNode {
        RawNode {
            role: Some(role),
            label: (!label.is_empty()).then(|| label.to_string()),
            clickable,
            enabled: true,
            frame: Some(Rect { x: frame.0, y: frame.1, w: frame.2, h: frame.3 }),
            ..Default::default()
        }
    }

    fn fixture(saved: bool) -> RawTree {
        let mut save = node(Role::Button, "Save", true, (300.0, 800.0, 100.0, 48.0));
        save.identifier = Some("com.acme:id/save".into());
        let mut email = node(Role::Textfield, "Email", true, (16.0, 200.0, 380.0, 56.0));
        email.value = Some(String::new());
        let mut pw = node(Role::Textfield, "Password", true, (16.0, 270.0, 380.0, 56.0));
        pw.value = Some("hunter2".into());
        pw.secure = true;
        let mut sw = node(Role::Switch, "Notifications", true, (16.0, 340.0, 380.0, 48.0));
        sw.checked = Some(true);
        let mut toolbar = node(Role::Toolbar, "Settings", false, (0.0, 0.0, 412.0, 64.0));
        toolbar.children.push(node(Role::Button, "Navigate up", true, (0.0, 0.0, 56.0, 56.0)));
        let mut root = node(Role::Window, "", false, (0.0, 0.0, 412.0, 915.0));
        root.children.push(toolbar);
        root.children.push(node(Role::Group, "", false, (0.0, 64.0, 412.0, 800.0)));
        root.children.push(email);
        root.children.push(pw);
        root.children.push(sw);
        root.children.push(save);
        if saved {
            root.children.push(node(Role::Text, "Saved", false, (16.0, 860.0, 200.0, 20.0)));
        }
        RawTree { width_pt: 412.0, height_pt: 915.0, app_id: Some("com.acme.app".into()), title: None, roots: vec![root], took_ms: 5 }
    }

    #[test]
    fn ids_are_per_snapshot_and_stale_ids_fail() {
        let s = Snapshot::build(7, &fixture(false));
        assert_eq!(s.id, "t7");
        let save = s.elements.iter().find(|e| e.label.as_deref() == Some("Save")).expect("save");
        assert_eq!(s.resolve(&save.id).expect("resolves").label.as_deref(), Some("Save"));
        assert_eq!(s.resolve(&format!("t7:{}", save.id)).expect("qualified").id, save.id);
        match s.resolve(&format!("t6:{}", save.id)) {
            Err(ResolveError::Stale { given, current }) => {
                assert_eq!(given, "t6");
                assert_eq!(current, "t7");
            }
            other => panic!("expected a stale error, got {other:?}"),
        }
        assert!(matches!(s.resolve("e999"), Err(ResolveError::Unknown { .. })));
        let p = s.tap_point(&save.id).expect("point");
        assert_eq!((p.x, p.y), (350.0, 824.0));
    }

    #[test]
    fn default_render_is_interactive_only_and_hides_secure_values() {
        let s = Snapshot::build(1, &fixture(false));
        let text = s.render("sim s1 · android · Pixel 9 · com.acme.app", false, None, false);
        assert!(text.starts_with("sim s1 · android · Pixel 9 · com.acme.app · snapshot t1 · 412x915 pt\n"), "{text}");
        assert!(text.contains("toolbar \"Settings\""), "{text}");
        assert!(text.contains("button \"Navigate up\""), "{text}");
        assert!(text.contains("textfield \"Email\" value=\"\""), "{text}");
        assert!(text.contains("textfield \"Password\" [secure]"), "{text}");
        assert!(!text.contains("hunter2"), "secure value leaked: {text}");
        assert!(text.contains("switch \"Notifications\" [on]"), "{text}");
        assert!(!text.contains("group"), "a plain group is not interactive: {text}");
        assert!(text.contains("6 of 8 elements; --all for the rest"), "{text}");
        let all = s.render("h", true, None, true);
        assert!(all.contains("group"), "{all}");
        assert!(all.contains(" @300,800 100x48"), "{all}");
        let found = s.render("h", false, Some("SAVE"), false);
        assert_eq!(found.lines().filter(|l| l.starts_with('e')).count(), 1, "{found}");
    }

    #[test]
    fn diff_reports_added_removed_and_changed() {
        let a = Snapshot::build(1, &fixture(false));
        let b = Snapshot::build(2, &fixture(true));
        let d = b.diff(&a, 5);
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].starts_with("+e") && d[0].ends_with("text \"Saved\""), "{d:?}");
        let back = a.diff(&b, 5);
        assert_eq!(back, vec!["-text \"Saved\"".to_string()]);
        let mut changed = fixture(false);
        changed.roots[0].children[2].value = Some("a@b.c".into());
        let c = Snapshot::build(3, &changed);
        let d = c.diff(&a, 5);
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].starts_with("~e") && d[0].contains("value=\"a@b.c\""), "{d:?}");
    }

    #[test]
    fn hit_picks_the_deepest_element() {
        let s = Snapshot::build(1, &fixture(false));
        let e = s.hit(Point { x: 10.0, y: 10.0 }).expect("hit");
        assert_eq!(e.label.as_deref(), Some("Navigate up"));
    }
}
