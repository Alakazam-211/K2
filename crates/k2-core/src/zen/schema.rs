//! Zen schema v1 (prd-zen-mode-v1 Z9, Z13, Z21–Z24, amendments Z50/Z51).
//!
//! A Zen file is parsed with spans (`toml_edit`) and walked by hand, so
//! every error and warning carries `file:line:col`. A clean file becomes a
//! flat [`Layer`] (`"colors.light.canvas" → "#fff"`); layers stack
//! built-in theme → `themes/<name>/theme.toml` → `zen.toml` →
//! `gardens/<id>.toml` and resolve into the JSON `/cli/zen/get` returns
//! (tokens, font, terminal palette, background, chrome, motion).
//!
//! A Garden file (prd-zen-gardens-v1 G10, G38) may also name its template
//! and lay out K2's built-in widgets. Those land in the layer under
//! `page.template`, `page.layout` and `page.widgets`, so they ride the same
//! last-good, history and fingerprint path as the theme keys.
//!
//! Every token, range and preset lives in the constants below. The `k2-zen`
//! skill is generated from the same constants, so a token added here without
//! docs fails its content test.

use std::collections::BTreeMap;
use std::ops::Range;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value as J};
use toml_edit::{Item, TableLike, Value};

/// The only schema this K2 reads.
pub const SCHEMA_VERSION: i64 = 1;
/// Garden 1's page template (the texting page).
pub const TEMPLATE_ID: &str = "k2.texting@1";
/// The template every new Garden starts from: empty, with Ask my agent.
pub const BLANK_TEMPLATE_ID: &str = "k2.blank@1";
/// Every page template K2 ships (G11).
pub const TEMPLATE_IDS: &[&str] = &[TEMPLATE_ID, BLANK_TEMPLATE_ID];

/// Every template a Garden may name: [`TEMPLATE_IDS`] (the two starts)
/// then the Garden catalog's ids, every shipped version
/// (prd-zen-user-widgets-v2 §15.4), read through `Defaults::live()`.
pub fn template_ids() -> Vec<&'static str> {
    super::Defaults::live().template_ids()
}

/// Whether a Garden may name `id` as its template.
pub fn is_template_id(id: &str) -> bool {
    super::Defaults::live().has_template(id)
}
/// Files over this size are refused (they are themes, not data).
pub const MAX_FILE_BYTES: usize = 64 * 1024;
/// Minimum contrast of `text` and `accent` against `canvas` (Z13).
pub const MIN_CONTRAST: f64 = 3.0;

/// Colour tokens, the same keys in `[colors.light]` and `[colors.dark]`.
/// Decision 9 (2026-10-04): no unread tracking, so there is no `unread`
/// colour; `idle` colours an agent that is not working.
pub const COLOR_TOKENS: &[&str] = &[
    "canvas",
    "surface",
    "surface-raised",
    "border",
    "text",
    "text-muted",
    "accent",
    "accent-text",
    "bubble-me",
    "bubble-me-text",
    "bubble-agent",
    "bubble-agent-text",
    "working",
    "idle",
    "needs-you",
    "danger",
];

/// Terminal palette tokens, the same keys in `[terminal.light]` and
/// `[terminal.dark]` (Omarchy addition 1: agent terminals in Zen match the
/// theme). The 16 ANSI colours plus foreground, background, cursor and
/// selection.
pub const TERMINAL_TOKENS: &[&str] = &[
    "foreground",
    "background",
    "cursor",
    "cursor-text",
    "selection",
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright-black",
    "bright-red",
    "bright-green",
    "bright-yellow",
    "bright-blue",
    "bright-magenta",
    "bright-cyan",
    "bright-white",
];

pub const SCHEMES: &[&str] = &["auto", "light", "dark"];

/// `[font] family` (Omarchy addition 5): one font for the whole Zen page,
/// terminals included. `(name, css stack, monospace)`. System stacks and
/// fonts the app already bundles only; no fonts from the network.
/// Terminals need fixed-width glyphs, so a proportional family pairs with
/// [`TERMINAL_PARTNER`] inside terminals; a monospace family is used there
/// as is.
pub const FONT_FAMILIES: &[(&str, &str, bool)] = &[
    ("system", "-apple-system, BlinkMacSystemFont, \"Segoe UI\", system-ui, sans-serif", false),
    ("rounded", "ui-rounded, \"SF Pro Rounded\", -apple-system, BlinkMacSystemFont, system-ui, sans-serif", false),
    ("serif", "ui-serif, \"New York\", Charter, Georgia, \"Times New Roman\", serif", false),
    ("mono", "ui-monospace, \"SF Mono\", Menlo, Consolas, \"DejaVu Sans Mono\", monospace", true),
    ("meslo", "\"MesloLGM Nerd Font\", \"MesloLGM Nerd Font Mono\", Menlo, Monaco, \"Courier New\", monospace", true),
    ("jetbrains-mono", "\"JetBrains Mono\", ui-monospace, Menlo, Consolas, monospace", true),
    ("fira-code", "\"Fira Code\", ui-monospace, Menlo, Consolas, monospace", true),
    ("lilex", "\"Lilex\", ui-monospace, Menlo, Consolas, monospace", true),
];
/// The family terminals use when `[font] family` is proportional (K2's
/// terminal default today).
pub const TERMINAL_PARTNER: &str = "meslo";
/// Names of [`FONT_FAMILIES`].
pub const FAMILIES: &[&str] =
    &["system", "rounded", "serif", "mono", "meslo", "jetbrains-mono", "fira-code", "lilex"];

/// `(css stack, monospace)` of a family name.
pub fn font_family(name: &str) -> Option<(&'static str, bool)> {
    FONT_FAMILIES.iter().find(|(n, _, _)| *n == name).map(|(_, s, m)| (*s, *m))
}

/// `[background] fit` (theme bundles only).
pub const BACKGROUND_FITS: &[&str] = &["cover", "contain", "tile", "center"];
/// Background image types: `(extension, mime)`. No SVG (it can carry script).
pub const BACKGROUND_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("webp", "image/webp"),
    ("gif", "image/gif"),
];
/// A background image is at most this many bytes (it travels to the
/// renderer as a `data:` URL inside `/cli/zen/get`).
pub const MAX_BACKGROUND_BYTES: u64 = 2 * 1024 * 1024;
/// `[chrome] corners` (Z51: only the two values K2 has ever sent).
pub const CORNERS: &[&str] = &["system", "square"];
/// `[chrome] stoplights` (Z50: macOS does not let K2 hide them).
pub const STOPLIGHTS: &[&str] = &["round", "square"];
/// `[chrome] stoplight-offset = [x, y]`, each 0..=this many px.
pub const STOPLIGHT_OFFSET_MAX: f64 = 24.0;

/// A numeric token with its inclusive range.
#[derive(Debug, Clone, Copy)]
pub struct NumToken {
    pub table: &'static str,
    pub key: &'static str,
    pub min: f64,
    pub max: f64,
}

pub const NUM_TOKENS: &[NumToken] = &[
    NumToken { table: "font", key: "size", min: 12.0, max: 20.0 },
    NumToken { table: "font", key: "line-height", min: 1.2, max: 1.8 },
    NumToken { table: "shape", key: "radius", min: 0.0, max: 28.0 },
    NumToken { table: "shape", key: "bubble-radius", min: 0.0, max: 28.0 },
    NumToken { table: "shape", key: "gap", min: 4.0, max: 24.0 },
    NumToken { table: "shape", key: "list-width", min: 240.0, max: 420.0 },
    NumToken { table: "background", key: "opacity", min: 0.0, max: 1.0 },
];

/// The animation tree (Z22): `(name, parent)`. A child takes its parent's
/// line until it is set.
pub const ANIMATION_TREE: &[(&str, Option<&str>)] = &[
    ("global", None),
    ("zen", Some("global")),
    ("zenIn", Some("zen")),
    ("zenOut", Some("zen")),
    ("rows", Some("global")),
    ("rowIn", Some("rows")),
    ("rowMove", Some("rows")),
    ("messages", Some("global")),
    ("messageIn", Some("messages")),
    ("conversation", Some("global")),
    ("conversationSwitch", Some("conversation")),
    ("working", Some("global")),
    ("workingPulse", Some("working")),
];

/// Fixed animation style presets. `popin` takes a percentage (`popin 92%`).
pub const ANIMATION_STYLES: &[&str] = &["fade", "slide", "slidefade", "popin"];
/// `speed` is in tenths of a second (Hyprland's unit), 0..=this.
pub const SPEED_MAX_DS: f64 = 100.0;
/// Bezier control-point y range (y may leave 0–1 for overshoot).
pub const BEZIER_Y_MIN: f64 = -2.0;
pub const BEZIER_Y_MAX: f64 = 3.0;

/// Curves every file can name without defining them.
pub const BUILTIN_BEZIERS: &[(&str, [f64; 4])] = &[
    ("linear", [0.0, 0.0, 1.0, 1.0]),
    ("ease", [0.25, 0.1, 0.25, 1.0]),
    ("glide", [0.22, 1.0, 0.36, 1.0]),
    ("soft", [0.45, 0.0, 0.55, 1.0]),
    ("overshot", [0.34, 1.56, 0.64, 1.0]),
];

/// Top-level tables every Zen file may carry (`zen.toml`, pages, themes).
pub const THEME_TABLES: &[&str] =
    &["theme", "colors", "font", "shape", "terminal", "chrome", "bezier", "animation"];
/// Tables only a theme bundle (`themes/<name>/theme.toml`) may carry: the
/// background image lives next to the bundle's own file.
pub const BUNDLE_TABLES: &[&str] = &["background"];
/// Top-level keys that only a Garden file may carry (G38).
pub const PAGE_TABLES: &[&str] = &["layout", "widget", "widgets", "control", "controls", "caps"];
/// The warning for `[[control]]` in a Garden file (prd-zen-freeform-chrome
/// FC52): a Garden places its controls with `[[widget]]` (chrome kinds), so
/// this table does nothing. A warning, not an error, so old files load.
pub const CONTROL_WARNING: &str =
    "[[control]] is ignored: place controls with [[widget]] kind = \"zen-toggle\" (see k2 zen guide bands)";

// ── Garden pages: layout and built-in widgets (G38) ─────────────────────

/// `[layout] kind`.
pub const LAYOUT_KINDS: &[&str] = &["columns"];
/// A Garden page has 1 to this many `[[layout.column]]`.
pub const MAX_COLUMNS: usize = 3;
/// `[[layout.column]] min-width`, px, 0..=this.
pub const COLUMN_MIN_WIDTH_MAX: f64 = 800.0;
/// At most this many content widgets (`WIDGET_KINDS`) on one page. Counted
/// after each table's kind is known (FC47): chrome has its own limit.
pub const MAX_WIDGETS: usize = 12;
/// At most this many chrome widgets (`CHROME_KINDS`) on one page (FC13).
pub const MAX_CHROME_WIDGETS: usize = 10;
/// Longest text prop (a Home or agent name).
pub const MAX_PROP_TEXT: usize = 200;

/// Built-in widget kinds a Garden file may place, with what they show.
pub const WIDGET_KINDS: &[(&str, &str)] = &[
    (
        "agents",
        "a Home's agents with live status (working, idle, needs you) and their last message; the whole Home, or one agent filtered from it",
    ),
    (
        "conversation",
        "one agent's Thread and a box to message it: the agent picked in an Agents widget, or one agent pinned by name",
    ),
    (
        "nav-rail",
        "a thin icon rail: in a column, a strip drawn at the left edge of its column (it takes no share of the column's box); with `slot = \"top\"`, a row of icons in the top band, right of the Garden switcher. Its items: My Home (this Garden's own page), then Agents, Projects and Tickets, which switch the Garden's view in this window inside Zen (Agents: this server's agents, by focus group; Projects: coming soon; Tickets: the Tickets page, chat only); the view shown is the current item; Tickets carries the top bar's waiting badge",
    ),
];
/// `[[widget]] slot`: where a widget sits (prd-zen-freeform-chrome FC7).
/// `column` (the default) places it in a layout column (`column = n`; a row
/// item there sits at the column's top or bottom `edge`); `top` and
/// `bottom` place it in the page's top or bottom band; `menu` places a Zen
/// control inside a `menu` widget (`menu = "<id>"`).
///
/// `slot` is a string so later slots are new values here, not a new key.
pub const WIDGET_SLOTS: &[&str] = &["column", "top", "bottom", "menu"];
/// The slot a widget gets when its table names none.
pub const DEFAULT_SLOT: &str = "column";
/// The top band's slot.
pub const TOP_SLOT: &str = "top";
/// The bottom band's slot (drawn only when it holds something, FC10).
pub const BOTTOM_SLOT: &str = "bottom";
/// The slot of an item inside a `menu` widget.
pub const MENU_SLOT: &str = "menu";
/// The band slots: full-width rows above and below the columns.
pub const BAND_SLOTS: &[&str] = &["top", "bottom"];
/// `align` (FC8): where an item sits in a band or column edge. In each
/// group items keep file order; in `end` the LAST item is in the corner.
pub const ALIGNS: &[&str] = &["start", "center", "end"];
/// The `align` an item gets when its table names none.
pub const DEFAULT_ALIGN: &str = "start";
/// `edge` (FC8): which edge of its column a row item sits at.
pub const EDGES: &[&str] = &["top", "bottom"];
/// The `edge` a row item in a column gets when its table names none.
pub const DEFAULT_EDGE: &str = "top";
/// THE allowlist of CONTENT kinds that fit a band (one row, the band's
/// height). Every chrome kind ([`CHROME_KINDS`]) fits a band too. Any other
/// kind in a band is an error naming both lists. Nothing else (renderer
/// included) keeps its own list.
pub const BAND_WIDGET_KINDS: &[&str] = &["nav-rail"];
/// At most this many CONTENT widgets in one band (FC47).
pub const MAX_BAND_WIDGETS: usize = 2;
/// At most this many items (content and chrome) in one band (FC13).
pub const MAX_BAND_ITEMS: usize = 8;
/// At most this many items at one column edge (FC13).
pub const MAX_EDGE_ITEMS: usize = 4;
/// At most this many `menu` widgets on one page (FC13).
pub const MAX_MENUS: usize = 3;
/// A menu holds 1 to this many items (FC13).
pub const MAX_MENU_ITEMS: usize = 6;
/// Content kinds that fill their column: they take no `edge` or `align` (FC9).
/// `custom` is column-only content (prd-zen-user-widgets-v2 UWA14).
pub const FILL_WIDGET_KINDS: &[&str] = &["agents", "conversation", CUSTOM_KIND];

/// A custom widget placement (prd-zen-user-widgets-v2 UW6): code from a
/// folder under `~/.k2/zen/widgets/<name>/` or a built-in `k2:<name>@<n>`,
/// run in a sealed frame. Not in [`WIDGET_KINDS`] (those are K2's own);
/// a column-only content kind that fills its column.
pub const CUSTOM_KIND: &str = "custom";
/// What `custom` places, for errors and docs.
pub const CUSTOM_KIND_DOC: &str = "a custom widget: an agent-written page in a sealed frame, from widgets/<name>/ (widget = \"<name>\") or a built-in (widget = \"k2:diary@1\"); it gets only the caps you allow with a click in the K2 app";
/// At most this many custom widgets on one page (UW6, UW29).
pub const MAX_CUSTOM_WIDGETS: usize = 6;
/// A custom placement's `config` table, as JSON, is at most this long (UW7).
pub const MAX_CONFIG_BYTES: usize = 4 * 1024;
/// A custom placement's props (UW7): `home` and `agent` (text) and `config`
/// (a table of strings, numbers and booleans, handed to the widget as
/// `k2.config`). `kind` is [`CUSTOM_KIND`].
pub const CUSTOM_PROPS: &[WidgetProp] = &[
    WidgetProp {
        kind: CUSTOM_KIND,
        name: "home",
        ty: PropType::Text,
        default: None,
        doc: "the Home the widget asks to see, by name or id; the human picks the scope when they allow it, and changing this asks again",
    },
    WidgetProp {
        kind: CUSTOM_KIND,
        name: "agent",
        ty: PropType::Text,
        default: None,
        doc: "one agent's name or address the widget asks to see; changing this asks again",
    },
];

/// Chrome kinds (prd-zen-freeform-chrome FC1): K2's own controls, placed
/// with `[[widget]]` like any widget. Declaring any of them replaces ALL of
/// the template's chrome (FC3). `zen-toggle` and `garden-switcher` are
/// required (`REQUIRED_CONTROLS`): a file that places chrome must place both,
/// once each, directly or as a top-level menu item.
pub const CHROME_KINDS: &[(&str, &str)] = &[
    (
        "garden-switcher",
        "the Garden switcher (required): the Garden's name; it opens the Garden list and + New Garden. In a menu: a Gardens section listing every Garden, then + New Garden",
    ),
    ("zen-toggle", "the Zen toggle (required): the way out of Zen. In a menu: the item Exit Zen Mode"),
    ("usage", "the subscription usage chip and its menu (optional). In a menu: the item Usage"),
    (
        "theme-picker",
        "the theme control (optional; Ctrl+Cmd+. still cycles themes without it). In a menu: the item Theme",
    ),
    (
        "menu",
        "a menu button K2 draws (optional) that holds other Zen controls (`slot = \"menu\"`, `menu = \"<this id>\"`); it opens with a click, Enter, Space or Down",
    ),
];
/// The kinds a `menu` may hold (FC9): no menu inside a menu, no content.
pub const MENU_ITEM_KINDS: &[&str] = &["zen-toggle", "garden-switcher", "theme-picker", "usage"];
/// `menu` `icon` (FC15).
pub const MENU_ICONS: &[&str] = &["dots", "bars", "zen"];
/// Longest `menu` `label`, in characters (FC15).
pub const MENU_LABEL_MAX: usize = 24;
/// The kind people try to place for window drag; K2 refuses it (FC5).
pub const DRAG_REGION_KIND: &str = "drag-region";
/// The error for `kind = "drag-region"` (FC5).
pub const DRAG_REGION_ERROR: &str =
    "K2 makes the empty space in every band drag the window; there is nothing to place.";

/// Is `kind` a chrome kind ([`CHROME_KINDS`])?
pub fn is_chrome_kind(kind: &str) -> bool {
    CHROME_KINDS.iter().any(|(k, _)| *k == kind)
}

/// The names of [`CHROME_KINDS`].
pub fn chrome_kind_names() -> Vec<&'static str> {
    CHROME_KINDS.iter().map(|(k, _)| *k).collect()
}

/// FC24 rule 1: a file that places chrome without a required control
/// (`zen-toggle` or `garden-switcher`).
pub fn required_chrome_error(kind: &str) -> String {
    let (why, align) =
        if kind == "zen-toggle" { ("the way out", "end") } else { ("the way to your other Gardens", "start") };
    format!(
        "This file places Zen controls, so it replaces the template's. It must also place a {kind} ({why}): add [[widget]] kind = \"{kind}\" slot = \"top\" align = \"{align}\", or put it in a menu. See k2 zen guide required."
    )
}
/// `nav-rail` `orientation`: a row of icons, or a column (strip).
pub const NAV_RAIL_ORIENTATIONS: &[&str] = &["row", "column"];

/// Kinds only a template places (never a Garden file).
pub const TEMPLATE_WIDGET_KINDS: &[(&str, &str)] = &[(
    "garden-empty",
    "the empty-Garden message with Ask my agent; shown until the Garden's file declares widgets",
)];

/// The Agents widget's live statuses (decision 9: no unread tracking).
pub const AGENT_STATUSES: &[&str] = &["working", "idle", "needs-you"];
/// The Agents widget's modes (Rosson 2026-10-04, answer 5).
pub const AGENTS_MODES: &[&str] = &["home", "agent"];
/// The Agents widget's row orders.
pub const AGENTS_ORDERS: &[&str] = &["home"];

/// The type of a widget prop.
#[derive(Debug, Clone, Copy)]
pub enum PropType {
    Bool,
    /// A short string (a name, an id or an address).
    Text,
    OneOf(&'static [&'static str]),
    /// A list whose items come from the set, no repeats.
    SubsetOf(&'static [&'static str]),
}

/// One prop of a built-in widget kind. `default` is JSON; `None` means
/// unset unless the page sets it (or, for `agents.mode`, derived).
#[derive(Debug, Clone, Copy)]
pub struct WidgetProp {
    pub kind: &'static str,
    pub name: &'static str,
    pub ty: PropType,
    pub default: Option<&'static str>,
    pub doc: &'static str,
}

pub const WIDGET_PROPS: &[WidgetProp] = &[
    WidgetProp {
        kind: "agents",
        name: "mode",
        ty: PropType::OneOf(AGENTS_MODES),
        default: None,
        doc: "`home` shows every agent of the Home (the whole Home surface); `agent` shows one agent filtered from that Home (set `agent`). Default: `agent` when `agent` is set, else `home`",
    },
    WidgetProp {
        kind: "agents",
        name: "home",
        ty: PropType::Text,
        default: None,
        doc: "the Home to show, by name or id. Default: the Garden's starting Home, else the window's Home",
    },
    WidgetProp {
        kind: "agents",
        name: "agent",
        ty: PropType::Text,
        default: None,
        doc: "one agent's name or address in that Home: the widget shows only that agent (`mode = \"agent\"`)",
    },
    WidgetProp {
        kind: "agents",
        name: "home-picker",
        ty: PropType::Bool,
        default: Some("false"),
        doc: "a Home picker in the widget's header; the pick belongs to this Garden and never moves the Home page",
    },
    WidgetProp {
        kind: "agents",
        name: "order",
        ty: PropType::OneOf(AGENTS_ORDERS),
        default: Some("\"home\""),
        doc: "row order: the Home's own order",
    },
    WidgetProp {
        kind: "agents",
        name: "server-tag",
        ty: PropType::Bool,
        default: Some("true"),
        doc: "tag rows from another server with its name",
    },
    WidgetProp {
        kind: "agents",
        name: "preview",
        ty: PropType::Bool,
        default: Some("true"),
        doc: "show each agent's last message",
    },
    WidgetProp {
        kind: "agents",
        name: "status",
        ty: PropType::SubsetOf(AGENT_STATUSES),
        default: Some("[\"working\", \"idle\", \"needs-you\"]"),
        doc: "which live statuses to show (`monitoring` shows with `working`, `unverifiable` with `idle`)",
    },
    WidgetProp {
        kind: "conversation",
        name: "agents",
        ty: PropType::Text,
        default: None,
        doc: "the id of the Agents widget whose picked agent this shows. Default: the page's first Agents widget",
    },
    WidgetProp {
        kind: "conversation",
        name: "agent",
        ty: PropType::Text,
        default: None,
        doc: "pin one agent's conversation by name or address (no Agents widget needed); not with `agents`",
    },
    WidgetProp {
        kind: "conversation",
        name: "home",
        ty: PropType::Text,
        default: None,
        doc: "with `agent`: the Home to look that agent up in, by name or id",
    },
    WidgetProp {
        kind: "conversation",
        name: "compose",
        ty: PropType::Bool,
        default: Some("true"),
        doc: "show the box to message the agent",
    },
    WidgetProp {
        kind: "conversation",
        name: "attachments",
        ty: PropType::Bool,
        default: Some("true"),
        doc: "allow attachments in the box",
    },
    WidgetProp {
        kind: "conversation",
        name: "load-older",
        ty: PropType::Bool,
        default: Some("true"),
        doc: "load older messages on scroll",
    },
    WidgetProp {
        kind: "nav-rail",
        name: "orientation",
        ty: PropType::OneOf(NAV_RAIL_ORIENTATIONS),
        default: None,
        doc: "`row` draws the icons side by side, `column` top to bottom. Default: `row` in a band (`slot = \"top\"` or `\"bottom\"`) and at a column `edge`, the only orientation that fits there; `column` in a column",
    },
    WidgetProp {
        kind: "menu",
        name: "icon",
        ty: PropType::OneOf(MENU_ICONS),
        default: Some("\"dots\""),
        doc: "the button's icon: `dots` (three dots), `bars` (three bars) or `zen` (the enso)",
    },
    WidgetProp {
        kind: "menu",
        name: "label",
        ty: PropType::Text,
        default: None,
        doc: "up to 24 characters, shown next to the icon and used as the tooltip and the screen-reader name. Without one the name is \"More\"",
    },
];

/// The props of a widget kind.
pub fn widget_props(kind: &str) -> impl Iterator<Item = &'static WidgetProp> + '_ {
    WIDGET_PROPS.iter().filter(move |p| p.kind == kind)
}

/// A widget id: letters, digits, `-` and `_`, starting with a letter or
/// digit, up to 64 (the same rule as a Garden id).
pub fn valid_widget_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && id.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Fill a widget's unset props with K2's defaults and derive `agents.mode`.
/// Applied to template widgets and Garden widgets alike, so the renderer
/// always reads every prop.
pub fn normalize_props(kind: &str, props: &mut Map<String, J>) {
    // Defaults are read through `Defaults::live()` (sync-defaults D0).
    normalize_props_in(super::Defaults::live(), kind, props)
}

/// [`normalize_props`] with the defaults of the set `d`
/// (prd-zen-garden-sync-defaults-v1 GS20). A prop the set doesn't list
/// (added by a later K2) takes the live default (GS22).
pub fn normalize_props_in(d: &super::Defaults, kind: &str, props: &mut Map<String, J>) {
    let live = super::Defaults::live();
    for p in widget_props(kind) {
        if props.contains_key(p.name) {
            continue;
        }
        let v = if d.knows_widget_prop(p.kind, p.name) {
            d.widget_prop_default(p.kind, p.name)
        } else {
            live.widget_prop_default(p.kind, p.name)
        };
        if let Some(v) = v {
            props.insert(p.name.to_string(), v.clone());
        }
    }
    if kind == "agents" && !props.contains_key("mode") {
        let mode = if props.contains_key("agent") { "agent" } else { "home" };
        props.insert("mode".into(), json!(mode));
    }
}

/// Fill props that depend on where the widget sits: a `nav-rail` with no
/// `orientation` draws as a `row` in a band or at a column `edge`, and a
/// `column` in a column. Applied to template and Garden widgets alike.
pub fn normalize_slot_props(kind: &str, slot: &str, edge: Option<&str>, props: &mut Map<String, J>) {
    if kind == "nav-rail" && !props.contains_key("orientation") {
        let o = if BAND_SLOTS.contains(&slot) || edge.is_some() { "row" } else { "column" };
        props.insert("orientation".into(), json!(o));
    }
}

/// One error or warning with its position (1-based line and column).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub message: String,
}

impl Diagnostic {
    /// `zen.toml:7:3: unknown key 'acent'`.
    pub fn render(&self) -> String {
        format!("{}:{}:{}: {}", self.file, self.line, self.col, self.message)
    }
}

/// Which kind of file is being checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// `zen.toml`: the theme for every Garden.
    Zen,
    /// `gardens/<id>.toml`: the template line, an optional theme override,
    /// and optional `[layout]` + `[[widget]]` for built-in widgets (G38).
    Garden,
    /// `themes/<name>/theme.toml`: a theme bundle (or the user's override
    /// of a built-in theme), plus `[background]`.
    Theme,
}

/// A flat, validated theme layer: `"colors.light.canvas" → "#fff"`,
/// `"animation.zenIn" → {on, speed, curve, style?}`.
pub type Layer = BTreeMap<String, J>;

/// The result of checking one file's text.
#[derive(Debug, Clone, Default)]
pub struct Checked {
    pub layer: Layer,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
    /// `(line, col)` of each layer key, for checks that need the disk
    /// (the background image) to point at the line that named it.
    pub positions: BTreeMap<String, (usize, usize)>,
}

impl Checked {
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }
}

struct Ctx<'a> {
    file: &'a str,
    src: &'a str,
    /// A Garden file's template when it doesn't name one (its list entry's).
    default_template: &'a str,
    /// The defaults a Garden file is checked on (GS20); `None` = live.
    defaults: Option<&'a super::Defaults>,
    line_starts: Vec<usize>,
    out: Checked,
    /// Where each layer key was set, for cross-checks after the walk.
    pos: BTreeMap<String, (usize, usize)>,
}

impl<'a> Ctx<'a> {
    fn new(file: &'a str, src: &'a str) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Ctx {
            file,
            src,
            default_template: TEMPLATE_ID,
            defaults: None,
            line_starts,
            out: Checked::default(),
            pos: BTreeMap::new(),
        }
    }

    fn at_offset(&self, off: usize) -> (usize, usize) {
        let off = off.min(self.src.len());
        let line_idx = match self.line_starts.binary_search(&off) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let start = self.line_starts[line_idx];
        let col = self.src.get(start..off).map(|s| s.chars().count()).unwrap_or(0) + 1;
        (line_idx + 1, col)
    }

    fn at(&self, span: Option<Range<usize>>, fallback: (usize, usize)) -> (usize, usize) {
        span.map(|r| self.at_offset(r.start)).unwrap_or(fallback)
    }

    fn error(&mut self, pos: (usize, usize), message: impl Into<String>) {
        self.out.errors.push(Diagnostic {
            file: self.file.to_string(),
            line: pos.0,
            col: pos.1,
            message: message.into(),
        });
    }

    fn warn(&mut self, pos: (usize, usize), message: impl Into<String>) {
        self.out.warnings.push(Diagnostic {
            file: self.file.to_string(),
            line: pos.0,
            col: pos.1,
            message: message.into(),
        });
    }

    fn set(&mut self, key: String, value: J, pos: (usize, usize)) {
        self.pos.insert(key.clone(), pos);
        self.out.layer.insert(key, value);
    }
}

/// Levenshtein distance, for "did you mean" hints.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn unknown_key(key: &str, where_: &str, allowed: &[&str]) -> String {
    let best = allowed
        .iter()
        .map(|a| (distance(key, a), *a))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d);
    match best {
        Some((_, a)) => format!("unknown key '{key}' in {where_}; did you mean '{a}'?"),
        None if allowed.is_empty() => format!("unknown key '{key}' in {where_}; it takes none"),
        None => format!("unknown key '{key}' in {where_}; allowed: {}", allowed.join(", ")),
    }
}

fn key_pos(ctx: &Ctx, t: &dyn TableLike, key: &str, item: &Item, fallback: (usize, usize)) -> (usize, usize) {
    let span = t.key(key).and_then(|k| k.span()).or_else(|| item.span());
    ctx.at(span, fallback)
}

fn item_pos(ctx: &Ctx, item: &Item, fallback: (usize, usize)) -> (usize, usize) {
    ctx.at(item.span(), fallback)
}

fn value_pos(ctx: &Ctx, v: &Value, fallback: (usize, usize)) -> (usize, usize) {
    ctx.at(v.span(), fallback)
}

fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Integer(i) => Some(*i.value() as f64),
        Value::Float(f) => Some(*f.value()),
        _ => None,
    }
}

fn num_json(n: f64) -> J {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        json!(n as i64)
    } else {
        json!(n)
    }
}

/// Parse a colour: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(r, g, b)`,
/// `rgba(r, g, b, a)`. Returns `[r, g, b, a]` with rgb 0–255 and a 0–1.
pub fn parse_color(s: &str) -> Option<[f64; 4]> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let d = |i: usize, n: usize| u8::from_str_radix(&hex[i..i + n], 16).ok();
        let expand = |c: u8| (c * 17) as f64;
        return match hex.len() {
            3 => Some([expand(d(0, 1)?), expand(d(1, 1)?), expand(d(2, 1)?), 1.0]),
            4 => Some([
                expand(d(0, 1)?),
                expand(d(1, 1)?),
                expand(d(2, 1)?),
                expand(d(3, 1)?) / 255.0,
            ]),
            6 => Some([d(0, 2)? as f64, d(2, 2)? as f64, d(4, 2)? as f64, 1.0]),
            8 => Some([d(0, 2)? as f64, d(2, 2)? as f64, d(4, 2)? as f64, d(6, 2)? as f64 / 255.0]),
            _ => None,
        };
    }
    let lower = s.to_ascii_lowercase();
    let (inner, want) = if let Some(r) = lower.strip_prefix("rgba(") {
        (r.strip_suffix(')')?, 4)
    } else if let Some(r) = lower.strip_prefix("rgb(") {
        (r.strip_suffix(')')?, 3)
    } else {
        return None;
    };
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() != want {
        return None;
    }
    let mut out = [0.0, 0.0, 0.0, 1.0];
    for (i, p) in parts.iter().enumerate().take(3) {
        let v: f64 = p.parse().ok()?;
        if !(0.0..=255.0).contains(&v) {
            return None;
        }
        out[i] = v;
    }
    if want == 4 {
        let a: f64 = parts[3].parse().ok()?;
        if !(0.0..=1.0).contains(&a) {
            return None;
        }
        out[3] = a;
    }
    Some(out)
}

fn channel(c: f64) -> f64 {
    let c = c / 255.0;
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn luminance(rgb: [f64; 3]) -> f64 {
    0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2])
}

fn over(fg: [f64; 4], bg: [f64; 3]) -> [f64; 3] {
    let a = fg[3];
    [
        fg[0] * a + bg[0] * (1.0 - a),
        fg[1] * a + bg[1] * (1.0 - a),
        fg[2] * a + bg[2] * (1.0 - a),
    ]
}

/// WCAG contrast of `fg` on `canvas`. A translucent canvas sits on white in
/// the light scheme and on black in the dark one; a translucent `fg` sits on
/// the canvas.
pub fn contrast(fg: [f64; 4], canvas: [f64; 4], dark: bool) -> f64 {
    let base = if dark { [0.0, 0.0, 0.0] } else { [255.0, 255.0, 255.0] };
    let bg = over(canvas, base);
    let fg = over(fg, bg);
    let (l1, l2) = (luminance(fg), luminance(bg));
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

fn valid_curve_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && name.len() <= 32
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `fade` | `slide` | `slidefade` | `popin <n>%` (n 0–100).
pub fn parse_style(s: &str) -> Result<String, String> {
    let t = s.trim();
    match t {
        "fade" | "slide" | "slidefade" => return Ok(t.to_string()),
        "popin" => return Ok("popin 80%".to_string()),
        _ => {}
    }
    if let Some(rest) = t.strip_prefix("popin ") {
        if let Some(n) = rest.trim().strip_suffix('%') {
            if let Ok(n) = n.trim().parse::<u32>() {
                if n <= 100 {
                    return Ok(format!("popin {n}%"));
                }
            }
        }
        return Err(format!("'{t}' needs a percentage from 0 to 100, like 'popin 92%'"));
    }
    Err(format!("unknown style '{t}'; use fade, slide, slidefade or popin <n>%"))
}

fn check_scalar_enum(ctx: &mut Ctx, key: String, item: &Item, pos: (usize, usize), allowed: &[&str], what: &str) {
    match item.as_value().and_then(Value::as_str) {
        Some(s) if allowed.contains(&s) => ctx.set(key, json!(s), pos),
        Some(s) => {
            let p = item_pos(ctx, item, pos);
            ctx.error(p, format!("{what} '{s}' is not one of: {}", allowed.join(", ")));
        }
        None => {
            let p = item_pos(ctx, item, pos);
            ctx.error(p, format!("{what} must be a string: {}", allowed.join(", ")));
        }
    }
}

fn require_table<'i>(ctx: &mut Ctx, item: &'i Item, pos: (usize, usize), name: &str) -> Option<&'i dyn TableLike> {
    match item.as_table_like() {
        Some(t) => Some(t),
        None => {
            ctx.error(pos, format!("`{name}` must be a table, like [{name}]"));
            None
        }
    }
}

fn check_theme(ctx: &mut Ctx, t: &dyn TableLike, at: (usize, usize)) {
    for (k, item) in t.iter() {
        let pos = key_pos(ctx, t, k, item, at);
        match k {
            "scheme" => check_scalar_enum(ctx, "theme.scheme".into(), item, pos, SCHEMES, "scheme"),
            other => ctx.error(pos, unknown_key(other, "[theme]", &["scheme"])),
        }
    }
}

/// `[colors.*]` and `[terminal.*]`: a light and a dark table of colours.
fn check_colors(ctx: &mut Ctx, table: &str, tokens: &[&str], t: &dyn TableLike, at: (usize, usize)) {
    for (scheme, item) in t.iter() {
        let pos = key_pos(ctx, t, scheme, item, at);
        if scheme != "light" && scheme != "dark" {
            ctx.error(pos, unknown_key(scheme, &format!("[{table}]"), &["light", "dark"]));
            continue;
        }
        let name = format!("{table}.{scheme}");
        let Some(sub) = require_table(ctx, item, pos, &name) else { continue };
        for (tok, v) in sub.iter() {
            let tpos = key_pos(ctx, sub, tok, v, pos);
            if !tokens.contains(&tok) {
                ctx.error(tpos, unknown_key(tok, &format!("[{name}]"), tokens));
                continue;
            }
            match v.as_value().and_then(Value::as_str) {
                Some(s) if parse_color(s).is_some() => {
                    ctx.set(format!("{name}.{tok}"), json!(s.trim()), tpos)
                }
                Some(s) => {
                    let p = item_pos(ctx, v, tpos);
                    ctx.error(p, format!("'{s}' is not a colour; use #rrggbb, #rgb, rgb(r, g, b) or rgba(r, g, b, a)"));
                }
                None => {
                    let p = item_pos(ctx, v, tpos);
                    ctx.error(p, format!("{name}.{tok} must be a colour string like \"#1f1c18\""));
                }
            }
        }
    }
}

fn check_numbers(ctx: &mut Ctx, table: &str, t: &dyn TableLike, at: (usize, usize), extra: &[&str]) {
    let mut allowed: Vec<&str> =
        NUM_TOKENS.iter().filter(|n| n.table == table).map(|n| n.key).collect();
    allowed.extend_from_slice(extra);
    for (k, item) in t.iter() {
        let pos = key_pos(ctx, t, k, item, at);
        if let Some(tok) = NUM_TOKENS.iter().find(|n| n.table == table && n.key == k) {
            match item.as_value().and_then(as_number) {
                Some(n) if n >= tok.min && n <= tok.max => {
                    ctx.set(format!("{table}.{k}"), num_json(n), pos)
                }
                Some(n) => {
                    let p = item_pos(ctx, item, pos);
                    ctx.error(p, format!("{table}.{k} = {n} is out of range; use {} to {}", num_json(tok.min), num_json(tok.max)));
                }
                None => {
                    let p = item_pos(ctx, item, pos);
                    ctx.error(p, format!("{table}.{k} must be a number from {} to {}", num_json(tok.min), num_json(tok.max)));
                }
            }
        } else if table == "font" && k == "family" {
            check_scalar_enum(ctx, "font.family".into(), item, pos, FAMILIES, "family");
        } else if table == "background" && k == "fit" {
            check_scalar_enum(ctx, "background.fit".into(), item, pos, BACKGROUND_FITS, "fit");
        } else if table == "background" && k == "image" {
            match item.as_value().and_then(Value::as_str).map(check_image_name) {
                Some(Ok(name)) => ctx.set("background.image".into(), json!(name), pos),
                Some(Err(msg)) => {
                    let p = item_pos(ctx, item, pos);
                    ctx.error(p, msg);
                }
                None => {
                    let p = item_pos(ctx, item, pos);
                    ctx.error(p, "background.image must be a file name in this theme's folder, like \"background.jpg\"");
                }
            }
        } else {
            ctx.error(pos, unknown_key(k, &format!("[{table}]"), &allowed));
        }
    }
}

/// The extension of an allowed background file name, lower case.
pub fn image_ext(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    BACKGROUND_TYPES.iter().find(|(e, _)| *e == ext).map(|(e, _)| *e)
}

/// `[background] image`: a plain file name inside the theme's own folder
/// (no folders, no `..`, not hidden) with a raster image extension.
pub fn check_image_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    let plain = !n.is_empty()
        && n.len() <= 128
        && !n.starts_with('.')
        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
    if !plain {
        return Err(format!(
            "background.image '{n}' must be a plain file name in this theme's folder (letters, digits, - _ .), like \"background.jpg\""
        ));
    }
    if image_ext(n).is_none() {
        return Err(format!(
            "background.image '{n}' must be one of: {} (no SVG)",
            BACKGROUND_TYPES.iter().map(|(e, _)| format!(".{e}")).collect::<Vec<_>>().join(", ")
        ));
    }
    Ok(n.to_string())
}

fn check_chrome(ctx: &mut Ctx, t: &dyn TableLike, at: (usize, usize)) {
    for (k, item) in t.iter() {
        let pos = key_pos(ctx, t, k, item, at);
        match k {
            "corners" => check_scalar_enum(ctx, "chrome.corners".into(), item, pos, CORNERS, "corners"),
            "stoplights" => {
                if item.as_value().and_then(Value::as_str) == Some("hidden") {
                    let p = item_pos(ctx, item, pos);
                    ctx.error(p, "stoplights can't be 'hidden': macOS doesn't let K2 hide the window buttons; use round or square");
                } else {
                    check_scalar_enum(ctx, "chrome.stoplights".into(), item, pos, STOPLIGHTS, "stoplights");
                }
            }
            "stoplight-offset" => {
                let arr = item.as_value().and_then(Value::as_array);
                let nums: Option<Vec<f64>> = arr.map(|a| a.iter().filter_map(as_number).collect());
                match (arr, nums) {
                    (Some(a), Some(n)) if a.len() == 2 && n.len() == 2 => {
                        if n.iter().all(|v| *v >= 0.0 && *v <= STOPLIGHT_OFFSET_MAX) {
                            ctx.set("chrome.stoplight-offset".into(), json!([num_json(n[0]), num_json(n[1])]), pos);
                        } else {
                            let p = item_pos(ctx, item, pos);
                            ctx.error(p, "stoplight-offset values must be 0 to 24 px each");
                        }
                    }
                    _ => {
                        let p = item_pos(ctx, item, pos);
                        ctx.error(p, "stoplight-offset must be [x, y] in px, each 0 to 24");
                    }
                }
            }
            "corner-radius" | "radius" => {
                ctx.error(pos, "corners are 'system' or 'square' in Zen v1; other radii are untested on macOS");
            }
            other => ctx.error(pos, unknown_key(other, "[chrome]", &["corners", "stoplights", "stoplight-offset"])),
        }
    }
}

fn check_bezier(ctx: &mut Ctx, t: &dyn TableLike, at: (usize, usize)) {
    for (name, item) in t.iter() {
        let pos = key_pos(ctx, t, name, item, at);
        if !valid_curve_name(name) {
            ctx.error(pos, format!("curve name '{name}' must start with a letter and use letters, digits, - or _"));
            continue;
        }
        let arr = item.as_value().and_then(Value::as_array);
        let nums: Vec<f64> = arr.map(|a| a.iter().filter_map(as_number).collect()).unwrap_or_default();
        if arr.map(|a| a.len()) != Some(4) || nums.len() != 4 {
            let p = item_pos(ctx, item, pos);
            ctx.error(p, format!("bezier '{name}' must be [x1, y1, x2, y2]"));
            continue;
        }
        if !(0.0..=1.0).contains(&nums[0]) || !(0.0..=1.0).contains(&nums[2]) {
            let p = item_pos(ctx, item, pos);
            ctx.error(p, format!("bezier '{name}': x1 and x2 must be 0 to 1"));
            continue;
        }
        if !(BEZIER_Y_MIN..=BEZIER_Y_MAX).contains(&nums[1]) || !(BEZIER_Y_MIN..=BEZIER_Y_MAX).contains(&nums[3]) {
            let p = item_pos(ctx, item, pos);
            ctx.error(p, format!("bezier '{name}': y1 and y2 must be -2 to 3"));
            continue;
        }
        ctx.set(format!("bezier.{name}"), json!(nums.iter().map(|n| num_json(*n)).collect::<Vec<_>>()), pos);
    }
}

fn check_animation(ctx: &mut Ctx, t: &dyn TableLike, at: (usize, usize)) {
    let names: Vec<&str> = ANIMATION_TREE.iter().map(|(n, _)| *n).collect();
    for (name, item) in t.iter() {
        let pos = key_pos(ctx, t, name, item, at);
        if !names.contains(&name) {
            ctx.error(pos, unknown_key(name, "[animation]", &names));
            continue;
        }
        let vpos = item_pos(ctx, item, pos);
        let Some(arr) = item.as_value().and_then(Value::as_array) else {
            ctx.error(vpos, format!("{name} must be [on, speed, curve] or [on, speed, curve, style]"));
            continue;
        };
        let vals: Vec<&Value> = arr.iter().collect();
        let on = match vals.first() {
            Some(Value::Integer(i)) if *i.value() == 0 || *i.value() == 1 => *i.value() == 1,
            Some(Value::Boolean(b)) => *b.value(),
            _ => {
                ctx.error(vpos, format!("{name}: the first value is on/off, 1 or 0"));
                continue;
            }
        };
        if !on && vals.len() == 1 {
            ctx.set(format!("animation.{name}"), json!({ "on": false }), pos);
            continue;
        }
        if vals.len() < 3 || vals.len() > 4 {
            ctx.error(vpos, format!("{name} must be [on, speed, curve] or [on, speed, curve, style]"));
            continue;
        }
        let speed = match as_number(vals[1]) {
            Some(s) if (0.0..=SPEED_MAX_DS).contains(&s) => s,
            _ => {
                let p = value_pos(ctx, vals[1], vpos);
                ctx.error(p, format!("{name}: speed is tenths of a second, 0 to 100"));
                continue;
            }
        };
        let curve = match vals[2].as_str() {
            Some(c) if valid_curve_name(c) => c.to_string(),
            _ => {
                let p = value_pos(ctx, vals[2], vpos);
                ctx.error(p, format!("{name}: the third value is a curve name, like \"glide\""));
                continue;
            }
        };
        let mut spec = json!({ "on": on, "speed": num_json(speed), "curve": curve });
        if let Some(sv) = vals.get(3) {
            match sv.as_str().map(parse_style) {
                Some(Ok(style)) => {
                    spec["style"] = json!(style);
                }
                Some(Err(msg)) => {
                    let p = value_pos(ctx, sv, vpos);
                    ctx.error(p, format!("{name}: {msg}"));
                    continue;
                }
                None => {
                    let p = value_pos(ctx, sv, vpos);
                    ctx.error(p, format!("{name}: style must be a string"));
                    continue;
                }
            }
        }
        // Remember the curve's own position for the cross-check.
        let cpos = value_pos(ctx, vals[2], vpos);
        ctx.pos.insert(format!("animation.{name}#curve"), cpos);
        ctx.set(format!("animation.{name}"), spec, pos);
    }
}

/// Check one file's text. `base` is every layer below this file, builtin
/// included (it supplies curves and the canvas for the contrast check). A
/// Garden file checked here defaults to the texting template; the store
/// uses [`check_garden`] with the Garden's own.
pub fn check(file: &str, src: &str, kind: FileKind, base: &Layer) -> Checked {
    check_with(file, src, kind, base, TEMPLATE_ID, None)
}

/// Check a Garden file whose list entry names `default_template` (used when
/// the file has no `template` line): its widgets' columns and links are
/// checked against that template when the file doesn't replace them.
pub fn check_garden(file: &str, src: &str, base: &Layer, default_template: &str) -> Checked {
    check_with(file, src, FileKind::Garden, base, default_template, None)
}

/// [`check_garden`] on the set `d`: its templates and prop defaults (GS20).
pub fn check_garden_in(d: &super::Defaults, file: &str, src: &str, base: &Layer, default_template: &str) -> Checked {
    check_with(file, src, FileKind::Garden, base, default_template, Some(d))
}

fn check_with(
    file: &str,
    src: &str,
    kind: FileKind,
    base: &Layer,
    default_template: &str,
    defaults: Option<&super::Defaults>,
) -> Checked {
    let mut ctx = Ctx::new(file, src);
    ctx.default_template = default_template;
    ctx.defaults = defaults;
    if src.len() > MAX_FILE_BYTES {
        ctx.error((1, 1), format!("this file is {} bytes; Zen files must be under 64 KB", src.len()));
        return ctx.out;
    }
    let doc = match toml_edit::Document::parse(src) {
        Ok(d) => d,
        Err(e) => {
            let pos = ctx.at(e.span(), (1, 1));
            let msg = e.message().lines().next().unwrap_or("invalid TOML").trim().to_string();
            ctx.error(pos, format!("invalid TOML: {msg}"));
            return ctx.out;
        }
    };
    let root = doc.as_table();
    let mut saw_schema = false;
    let mut page_pos: Option<(usize, usize)> = None;
    for (k, item) in root.iter() {
        let pos = key_pos(&ctx, root, k, item, (1, 1));
        match k {
            "schema" => {
                saw_schema = true;
                match item.as_value() {
                    Some(Value::Integer(i)) if *i.value() == SCHEMA_VERSION => {}
                    Some(Value::Integer(i)) => {
                        let p = item_pos(&ctx, item, pos);
                        ctx.error(p, format!("schema = {} is not supported; this K2 reads Zen schema 1", i.value()));
                    }
                    _ => {
                        let p = item_pos(&ctx, item, pos);
                        ctx.error(p, "schema must be the number 1; this K2 reads Zen schema 1");
                    }
                }
            }
            "template" if kind == FileKind::Garden => match item.as_value().and_then(Value::as_str) {
                Some(t) if is_template_id(t) => {
                    page_pos.get_or_insert(pos);
                    ctx.set("page.template".into(), json!(t), pos)
                }
                Some(other) => {
                    let p = item_pos(&ctx, item, pos);
                    ctx.error(p, format!("unknown template '{other}'; Zen templates are: {}", template_ids().join(", ")));
                }
                None => {
                    let p = item_pos(&ctx, item, pos);
                    ctx.error(p, format!("template must be a string, one of: {}", template_ids().join(", ")));
                }
            },
            "template" if kind == FileKind::Theme => {
                ctx.error(pos, "unknown key 'template' in a theme; the template line belongs in gardens/<id>.toml")
            }
            "template" => ctx.error(pos, "unknown key 'template' in zen.toml; the template line belongs in gardens/<id>.toml"),
            "type" => ctx.error(pos, "[type] is now [font] (family, size, line-height): one font for Zen and its terminals"),
            "background" if kind != FileKind::Theme => ctx.error(
                pos,
                "[background] belongs in a theme bundle (~/.k2/zen/themes/<name>/theme.toml), next to its image; make one with k2 zen theme new <name>",
            ),
            k if PAGE_TABLES.contains(&k) && kind != FileKind::Garden => ctx.error(
                pos,
                format!("[{k}] belongs in a Garden file (~/.k2/zen/gardens/<id>.toml); list them with k2 zen garden list"),
            ),
            "control" | "controls" => ctx.warn(pos, CONTROL_WARNING),
            "caps" => ctx.error(pos, "a page can't name caps: built-in widgets get K2's caps"),
            "widgets" => ctx.error(pos, "use one [[widget]] block per widget (not `widgets`)"),
            "layout" => {
                page_pos.get_or_insert(pos);
                check_layout(&mut ctx, item, pos);
            }
            "widget" => {
                page_pos.get_or_insert(pos);
                check_widgets(&mut ctx, item, pos);
            }
            "theme" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "theme") {
                    check_theme(&mut ctx, t, pos);
                }
            }
            "colors" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "colors") {
                    check_colors(&mut ctx, "colors", COLOR_TOKENS, t, pos);
                }
            }
            "terminal" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "terminal") {
                    check_colors(&mut ctx, "terminal", TERMINAL_TOKENS, t, pos);
                }
            }
            "font" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "font") {
                    check_numbers(&mut ctx, "font", t, pos, &["family"]);
                }
            }
            "background" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "background") {
                    check_numbers(&mut ctx, "background", t, pos, &["image", "fit"]);
                }
            }
            "shape" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "shape") {
                    check_numbers(&mut ctx, "shape", t, pos, &[]);
                }
            }
            "chrome" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "chrome") {
                    check_chrome(&mut ctx, t, pos);
                }
            }
            "bezier" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "bezier") {
                    check_bezier(&mut ctx, t, pos);
                }
            }
            "animation" => {
                if let Some(t) = require_table(&mut ctx, item, pos, "animation") {
                    check_animation(&mut ctx, t, pos);
                }
            }
            other => {
                let mut allowed: Vec<&str> = vec!["schema"];
                if kind == FileKind::Garden {
                    allowed.extend_from_slice(&["template", "layout", "widget"]);
                }
                allowed.extend_from_slice(THEME_TABLES);
                if kind == FileKind::Theme {
                    allowed.extend_from_slice(BUNDLE_TABLES);
                }
                ctx.error(pos, unknown_key(other, "the top level", &allowed));
            }
        }
    }
    if !saw_schema {
        ctx.error((1, 1), "missing `schema = 1` (the first line of every Zen file)");
    }
    if kind == FileKind::Garden {
        if let Some(at) = page_pos {
            cross_check_page(&mut ctx, at);
        }
    }
    cross_check(&mut ctx, base);
    ctx.out.positions = std::mem::take(&mut ctx.pos);
    ctx.out
}

/// The tables of `[[name]]` (or `name = [{…}, …]`). `None` for any other shape.
fn tables_of(item: &Item) -> Option<Vec<&dyn TableLike>> {
    if let Some(a) = item.as_array_of_tables() {
        return Some(a.iter().map(|t| t as &dyn TableLike).collect());
    }
    let arr = item.as_value().and_then(Value::as_array)?;
    arr.iter().map(|v| v.as_inline_table().map(|t| t as &dyn TableLike)).collect()
}

/// Position of a table in an array: its first key, else the fallback.
fn table_pos(ctx: &Ctx, t: &dyn TableLike, fallback: (usize, usize)) -> (usize, usize) {
    t.iter()
        .next()
        .map(|(k, item)| key_pos(ctx, t, k, item, fallback))
        .unwrap_or(fallback)
}

/// `[layout]`: `kind = "columns"` and 1–3 `[[layout.column]]` whose
/// `size` (percent) adds up to 100. Sets `page.layout`.
fn check_layout(ctx: &mut Ctx, item: &Item, at: (usize, usize)) {
    let Some(t) = require_table(ctx, item, at, "layout") else { return };
    let mut kind: Option<String> = None;
    let mut columns: Vec<J> = Vec::new();
    let mut ok = true;
    let mut saw_columns = false;
    for (k, v) in t.iter() {
        let pos = key_pos(ctx, t, k, v, at);
        match k {
            "kind" => match v.as_value().and_then(Value::as_str) {
                Some(s) if LAYOUT_KINDS.contains(&s) => kind = Some(s.to_string()),
                Some(s) => {
                    let p = item_pos(ctx, v, pos);
                    ctx.error(p, format!("layout kind '{s}' is not one of: {}", LAYOUT_KINDS.join(", ")));
                    ok = false;
                }
                None => {
                    let p = item_pos(ctx, v, pos);
                    ctx.error(p, format!("layout kind must be a string: {}", LAYOUT_KINDS.join(", ")));
                    ok = false;
                }
            },
            "column" => {
                saw_columns = true;
                let Some(cols) = tables_of(v) else {
                    ctx.error(pos, "columns are [[layout.column]] blocks, each with size and min-width");
                    ok = false;
                    continue;
                };
                if cols.is_empty() || cols.len() > MAX_COLUMNS {
                    ctx.error(pos, format!("a Garden page has 1 to {MAX_COLUMNS} columns; this one has {}", cols.len()));
                    ok = false;
                    continue;
                }
                let mut sum = 0.0;
                for (i, c) in cols.iter().enumerate() {
                    let cpos = table_pos(ctx, *c, pos);
                    let mut col = Map::new();
                    let mut size = None;
                    for (ck, cv) in c.iter() {
                        let kp = key_pos(ctx, *c, ck, cv, cpos);
                        match ck {
                            "size" => match cv.as_value().and_then(as_number) {
                                Some(n) if n > 0.0 && n <= 100.0 => size = Some(n),
                                _ => {
                                    let p = item_pos(ctx, cv, kp);
                                    ctx.error(p, "column size is a percent of the width, more than 0 and at most 100");
                                    ok = false;
                                }
                            },
                            "min-width" => match cv.as_value().and_then(as_number) {
                                Some(n) if (0.0..=COLUMN_MIN_WIDTH_MAX).contains(&n) => {
                                    col.insert("min-width".into(), num_json(n));
                                }
                                _ => {
                                    let p = item_pos(ctx, cv, kp);
                                    ctx.error(p, format!("column min-width is px, 0 to {}", num_json(COLUMN_MIN_WIDTH_MAX)));
                                    ok = false;
                                }
                            },
                            "widget" => match cv.as_value().and_then(Value::as_str) {
                                Some(w) => {
                                    let wp = item_pos(ctx, cv, kp);
                                    ctx.pos.insert(format!("page.layout.column.{i}.widget"), wp);
                                    col.insert("widget".into(), json!(w));
                                }
                                None => {
                                    let p = item_pos(ctx, cv, kp);
                                    ctx.error(p, "column widget must be a widget id (a string)");
                                    ok = false;
                                }
                            },
                            other => {
                                ctx.error(kp, unknown_key(other, "[[layout.column]]", &["size", "min-width", "widget"]));
                                ok = false;
                            }
                        }
                    }
                    match size {
                        Some(n) => {
                            sum += n;
                            col.insert("size".into(), num_json(n));
                        }
                        None => {
                            if c.get("size").is_none() {
                                ctx.error(cpos, "each [[layout.column]] needs a size (percent of the width)");
                            }
                            ok = false;
                        }
                    }
                    col.entry("min-width").or_insert(json!(0));
                    columns.push(J::Object(col));
                }
                if ok && (sum - 100.0).abs() > 0.01 {
                    ctx.error(pos, format!("column sizes add up to {}; they must add up to 100", num_json(sum)));
                    ok = false;
                }
            }
            other => {
                ctx.error(pos, unknown_key(other, "[layout]", &["kind", "column"]));
                ok = false;
            }
        }
    }
    if t.get("kind").is_none() {
        ctx.error(at, "[layout] needs kind = \"columns\"");
        ok = false;
    }
    if !saw_columns {
        ctx.error(at, "[layout] needs 1 to 3 [[layout.column]] blocks");
        ok = false;
    }
    if ok {
        ctx.pos.insert("page.layout".into(), at);
        ctx.set("page.layout".into(), json!({ "kind": kind, "columns": columns }), at);
    }
}

fn check_prop(ctx: &mut Ctx, p: &WidgetProp, v: &Item, pos: (usize, usize)) -> Option<J> {
    let vp = item_pos(ctx, v, pos);
    let what = format!("{}.{}", p.kind, p.name);
    match p.ty {
        PropType::Bool => match v.as_value().and_then(Value::as_bool) {
            Some(b) => Some(json!(b)),
            None => {
                ctx.error(vp, format!("{what} must be true or false"));
                None
            }
        },
        PropType::Text => match v.as_value().and_then(Value::as_str).map(str::trim) {
            Some(s) if !s.is_empty() && s.chars().count() <= MAX_PROP_TEXT && !s.chars().any(char::is_control) => {
                Some(json!(s))
            }
            _ => {
                ctx.error(vp, format!("{what} must be a name or id (text, 1 to {MAX_PROP_TEXT} characters)"));
                None
            }
        },
        PropType::OneOf(allowed) => match v.as_value().and_then(Value::as_str) {
            Some(s) if allowed.contains(&s) => Some(json!(s)),
            _ => {
                ctx.error(vp, format!("{what} must be one of: {}", allowed.join(", ")));
                None
            }
        },
        PropType::SubsetOf(allowed) => {
            let items: Option<Vec<&str>> = v
                .as_value()
                .and_then(Value::as_array)
                .and_then(|a| a.iter().map(|x| x.as_str()).collect::<Option<Vec<_>>>());
            match items {
                Some(xs)
                    if !xs.is_empty()
                        && xs.iter().all(|x| allowed.contains(x))
                        && xs.iter().collect::<std::collections::BTreeSet<_>>().len() == xs.len() =>
                {
                    Some(json!(xs))
                }
                _ => {
                    ctx.error(vp, format!("{what} must be a list of one or more of: {} (no repeats)", allowed.join(", ")));
                    None
                }
            }
        }
    }
}

/// A custom widget's `config` (UW7): a table of strings, numbers and
/// booleans, at most [`MAX_CONFIG_BYTES`] as JSON. K2 gives it no meaning.
fn check_config(ctx: &mut Ctx, v: &Item, pos: (usize, usize)) -> Option<J> {
    let Some(t) = require_table(ctx, v, pos, "widget.props.config") else { return None };
    let mut out = Map::new();
    let mut ok = true;
    for (k, item) in t.iter() {
        let kp = key_pos(ctx, t, k, item, pos);
        let val = match item.as_value() {
            Some(Value::String(s)) => Some(json!(s.value())),
            Some(Value::Integer(n)) => Some(json!(*n.value())),
            Some(Value::Float(f)) if f.value().is_finite() => Some(json!(*f.value())),
            Some(Value::Boolean(b)) => Some(json!(*b.value())),
            _ => None,
        };
        match val {
            Some(j) => {
                out.insert(k.to_string(), j);
            }
            None => {
                ctx.error(kp, format!("config.{k} must be a string, a number or true/false (no lists or tables)"));
                ok = false;
            }
        }
    }
    let size = J::Object(out.clone()).to_string().len();
    if size > MAX_CONFIG_BYTES {
        ctx.error(pos, format!("config is {size} bytes as JSON; a custom widget's config is at most {MAX_CONFIG_BYTES} (4 KB)"));
        ok = false;
    }
    ok.then_some(J::Object(out))
}

/// One `[[widget]]` after the walk, for the page-wide rules.
struct Placed {
    kind: String,
    slot: &'static str,
    id: String,
    pos: (usize, usize),
    /// `(column, edge)` of a row item at a column edge (the edge defaulted).
    at_edge: Option<(u64, &'static str)>,
    /// `slot = "menu"`: the menu it names, and where.
    menu: Option<(String, (usize, usize))>,
}

/// "nav-rail, garden-switcher, …": every kind that fits a band.
fn band_kinds() -> String {
    BAND_WIDGET_KINDS.iter().copied().chain(chrome_kind_names()).collect::<Vec<_>>().join(", ")
}

/// `[[widget]]`: K2's built-in widgets and Zen controls (prd-zen-gardens-v1
/// G38; prd-zen-freeform-chrome FC1–FC24). Sets `page.widgets`: every
/// declared widget, content and chrome, in file order (props normalized
/// with K2's defaults; caps are never read from the file). `garden_page`
/// splits the two groups: chrome never reaches the answer's `widgets`.
///
/// - `slot`: `column` (default; `column = n`, and a row item sits at the
///   column's `edge`), `top` / `bottom` (the bands), `menu` (`menu = id`).
/// - Bands take [`BAND_WIDGET_KINDS`] and every chrome kind; a menu takes
///   [`MENU_ITEM_KINDS`]; a column takes anything, chrome at its edge.
/// - A widget outside a column's body (a band, an edge, a menu) may leave
///   `id` out: it is then its kind. One id namespace per page.
/// - Limits are counted after the kinds are known (FC47).
fn check_widgets(ctx: &mut Ctx, item: &Item, at: (usize, usize)) {
    let Some(tables) = tables_of(item) else {
        ctx.error(at, "widgets are [[widget]] blocks (one per widget), each with id, kind and column");
        return;
    };
    if tables.is_empty() {
        ctx.error(at, "declare at least one [[widget]], or leave them out to keep the template's");
        return;
    }
    let mut content_kinds: Vec<&str> = WIDGET_KINDS.iter().map(|(k, _)| *k).collect();
    let chrome_kinds = chrome_kind_names();
    let kind_list = format!(
        "built-in widgets are: {}; Zen controls are: {}; a custom widget is kind = \"{CUSTOM_KIND}\" with widget = \"<folder>\"",
        content_kinds.join(", "),
        chrome_kinds.join(", ")
    );
    content_kinds.push(CUSTOM_KIND);
    let mut out: Vec<J> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    let mut ok = true;
    let mut placed: Vec<Placed> = Vec::new();
    for (i, t) in tables.iter().enumerate() {
        let tpos = table_pos(ctx, *t, at);
        ctx.pos.insert(format!("page.widget.{i}"), tpos);
        // Kind first: it decides which slots, keys and props are allowed.
        let kind: Option<String> = match t.get("kind") {
            None => {
                ctx.error(tpos, format!("each [[widget]] needs a kind: {kind_list}"));
                ok = false;
                None
            }
            Some(v) => {
                let kp = key_pos(ctx, *t, "kind", v, tpos);
                let vp = item_pos(ctx, v, kp);
                match v.as_value().and_then(Value::as_str) {
                    Some(k) if content_kinds.contains(&k) || is_chrome_kind(k) => Some(k.to_string()),
                    Some(k) if k == DRAG_REGION_KIND => {
                        ctx.error(vp, format!("'{DRAG_REGION_KIND}' can't be placed: {DRAG_REGION_ERROR}"));
                        ok = false;
                        None
                    }
                    Some(k) if TEMPLATE_WIDGET_KINDS.iter().any(|(n, _)| *n == k) => {
                        ctx.error(
                            vp,
                            format!(
                                "'{k}' comes from the blank template and shows until the Garden has widgets; place {} instead",
                                content_kinds.join(" or ")
                            ),
                        );
                        ok = false;
                        None
                    }
                    Some(k) => {
                        ctx.error(vp, format!("unknown widget kind '{k}'; {kind_list}"));
                        ok = false;
                        None
                    }
                    None => {
                        ctx.error(vp, format!("widget kind must be a string; {kind_list}"));
                        ok = false;
                        None
                    }
                }
            }
        };
        let chrome = kind.as_deref().is_some_and(is_chrome_kind);
        // Then the slot: it decides whether `column`, `edge`, `align` and
        // `menu` belong.
        let slot: Option<&'static str> = match t.get("slot") {
            None => Some(DEFAULT_SLOT),
            Some(v) => {
                let sp = key_pos(ctx, *t, "slot", v, tpos);
                let vp = item_pos(ctx, v, sp);
                match v.as_value().and_then(Value::as_str) {
                    Some(x) if BAND_SLOTS.contains(&x) => {
                        let band: &'static str = if x == TOP_SLOT { TOP_SLOT } else { BOTTOM_SLOT };
                        if let Some(k) =
                            kind.as_deref().filter(|k| !BAND_WIDGET_KINDS.contains(k) && !is_chrome_kind(k))
                        {
                            ctx.error(
                                vp,
                                format!(
                                    "'{k}' can't go in the {band} band; slot = \"{band}\" takes: {}. Place '{k}' in a column (drop slot, set column)",
                                    band_kinds()
                                ),
                            );
                            ok = false;
                        }
                        Some(band)
                    }
                    Some(x) if x == MENU_SLOT => {
                        match kind.as_deref() {
                            Some("menu") => {
                                ctx.error(
                                    vp,
                                    "a menu can't hold another menu: place this menu in a band (slot = \"top\" or \"bottom\") or at a column edge",
                                );
                                ok = false;
                            }
                            Some(k) if !MENU_ITEM_KINDS.contains(&k) => {
                                ctx.error(
                                    vp,
                                    format!("'{k}' can't go in a menu; a menu holds only: {}", MENU_ITEM_KINDS.join(", ")),
                                );
                                ok = false;
                            }
                            _ => {}
                        }
                        Some(MENU_SLOT)
                    }
                    Some(x) if x == DEFAULT_SLOT => Some(DEFAULT_SLOT),
                    _ => {
                        ctx.error(
                            vp,
                            format!(
                                "widget slot must be one of: {} (\"column\", the default, places it with column = n; \"top\" and \"bottom\" put it in the page's top or bottom band; \"menu\" puts a Zen control in a menu, with menu = \"<menu id>\")",
                                WIDGET_SLOTS.join(", ")
                            ),
                        );
                        ok = false;
                        None
                    }
                }
            }
        };
        let in_band = slot.is_some_and(|s| BAND_SLOTS.contains(&s));
        let in_menu = slot == Some(MENU_SLOT);
        let in_column = slot == Some(DEFAULT_SLOT);
        let fills = kind.as_deref().filter(|k| FILL_WIDGET_KINDS.contains(k));
        let mut w = Map::new();
        let mut props = Map::new();
        let mut edge: Option<&'static str> = None;
        let mut align: Option<&'static str> = None;
        let mut menu: Option<(String, (usize, usize))> = None;
        for (k, v) in t.iter() {
            let pos = key_pos(ctx, *t, k, v, tpos);
            match k {
                "kind" | "slot" => {}
                "id" => match v.as_value().and_then(Value::as_str) {
                    Some(id) if valid_widget_id(id) => {
                        if ids.iter().any(|x| x == id) {
                            let p = item_pos(ctx, v, pos);
                            ctx.error(p, format!("widget id '{id}' is used twice; ids must be unique on the page"));
                            ok = false;
                        } else {
                            ids.push(id.to_string());
                            w.insert("id".into(), json!(id));
                        }
                    }
                    _ => {
                        let p = item_pos(ctx, v, pos);
                        ctx.error(p, "widget id must be letters, digits, - and _ (up to 64), starting with a letter or digit");
                        ok = false;
                    }
                },
                "column" if in_band => {
                    let band = slot.unwrap_or(TOP_SLOT);
                    ctx.error(
                        pos,
                        format!(
                            "a {band}-band widget (slot = \"{band}\") has no column; drop column, or drop slot to place it in a column"
                        ),
                    );
                    ok = false;
                }
                "column" if in_menu => {
                    ctx.error(pos, "a menu item (slot = \"menu\") has no column: its menu says where it sits; drop column");
                    ok = false;
                }
                "column" => match v.as_value() {
                    Some(Value::Integer(n)) if *n.value() >= 0 && (*n.value() as usize) < MAX_COLUMNS => {
                        let cp = item_pos(ctx, v, pos);
                        ctx.pos.insert(format!("page.widget.{i}.column"), cp);
                        w.insert("column".into(), json!(*n.value()));
                    }
                    _ => {
                        let p = item_pos(ctx, v, pos);
                        ctx.error(p, format!("widget column is a column number, 0 to {}", MAX_COLUMNS - 1));
                        ok = false;
                    }
                },
                "edge" | "align" if fills.is_some() => {
                    let f = fills.unwrap_or_default();
                    ctx.error(pos, format!("'{f}' fills its column and takes no edge or align; drop {k}"));
                    ok = false;
                }
                "edge" if slot.is_some() && !in_column => {
                    ctx.error(
                        pos,
                        "edge picks the top or bottom of a column; it goes only with slot = \"column\" (a band is already an edge of the page; a menu item sits in its menu)",
                    );
                    ok = false;
                }
                "edge" => match v.as_value().and_then(Value::as_str).and_then(|s| EDGES.iter().find(|e| **e == s)) {
                    Some(e) => edge = Some(e),
                    None => {
                        let p = item_pos(ctx, v, pos);
                        ctx.error(p, format!("edge must be one of: {}", EDGES.join(", ")));
                        ok = false;
                    }
                },
                "align" if in_menu => {
                    ctx.error(
                        pos,
                        "align places an item in a band or at a column edge; a menu lists its items in file order, so drop align",
                    );
                    ok = false;
                }
                "align" => match v.as_value().and_then(Value::as_str).and_then(|s| ALIGNS.iter().find(|a| **a == s)) {
                    Some(a) => align = Some(a),
                    None => {
                        let p = item_pos(ctx, v, pos);
                        ctx.error(p, format!("align must be one of: {}", ALIGNS.join(", ")));
                        ok = false;
                    }
                },
                "menu" if slot.is_some() && !in_menu => {
                    ctx.error(
                        pos,
                        "menu = \"<id>\" goes only with slot = \"menu\": it names the menu that holds this item",
                    );
                    ok = false;
                }
                "menu" => match v.as_value().and_then(Value::as_str) {
                    Some(m) if valid_widget_id(m) => menu = Some((m.to_string(), item_pos(ctx, v, pos))),
                    _ => {
                        let p = item_pos(ctx, v, pos);
                        ctx.error(p, "menu must be the id of a menu widget (letters, digits, - and _)");
                        ok = false;
                    }
                },
                "caps" if kind.as_deref() == Some(CUSTOM_KIND) => {
                    ctx.error(
                        pos,
                        "a page can't grant caps: the widget's manifest.json asks for them, and the human allows them with a click in the K2 app",
                    );
                    ok = false;
                }
                "caps" => {
                    ctx.error(pos, "a page can't name caps: built-in widgets get K2's caps");
                    ok = false;
                }
                "widget" if kind.as_deref() == Some(CUSTOM_KIND) => {
                    let vp = item_pos(ctx, v, pos);
                    match v.as_value().and_then(Value::as_str).map(super::builtin_widgets::parse_widget_ref) {
                        Some(Ok(super::builtin_widgets::WidgetRef::Builtin { version: None, name })) => {
                            ctx.error(
                                vp,
                                format!("name a built-in widget with its version, like k2:{name}@1 (k2:{name} without a version is only for k2 zen widget new --from)"),
                            );
                            ok = false;
                        }
                        Some(Ok(r)) => {
                            ctx.pos.insert(format!("page.widget.{i}.widget"), vp);
                            w.insert("widget".into(), json!(r.to_string()));
                        }
                        Some(Err(m)) => {
                            ctx.error(vp, m);
                            ok = false;
                        }
                        None => {
                            ctx.error(vp, "widget must be the widget's folder name (a string), or a built-in like k2:diary@1");
                            ok = false;
                        }
                    }
                }
                "widget" => {
                    ctx.error(
                        pos,
                        format!("widget = names a custom widget's folder; it goes only with kind = \"{CUSTOM_KIND}\""),
                    );
                    ok = false;
                }
                "props" if kind.as_deref() == Some(CUSTOM_KIND) => {
                    let Some(pt) = require_table(ctx, v, pos, "widget.props") else {
                        ok = false;
                        continue;
                    };
                    for (pk, pv) in pt.iter() {
                        let ppos = key_pos(ctx, pt, pk, pv, pos);
                        let checked = match pk {
                            "config" => check_config(ctx, pv, ppos),
                            _ => match CUSTOM_PROPS.iter().find(|p| p.name == pk) {
                                Some(p) => check_prop(ctx, p, pv, ppos),
                                None => {
                                    ctx.error(ppos, unknown_key(pk, "a custom widget's props", &["home", "agent", "config"]));
                                    None
                                }
                            },
                        };
                        match checked {
                            Some(val) => {
                                let vp = item_pos(ctx, pv, ppos);
                                ctx.pos.insert(format!("page.widget.{i}.props.{pk}"), vp);
                                props.insert(pk.to_string(), val);
                            }
                            None => ok = false,
                        }
                    }
                }
                "props" => {
                    let Some(pt) = require_table(ctx, v, pos, "widget.props") else {
                        ok = false;
                        continue;
                    };
                    let Some(kind) = kind.as_deref() else { continue };
                    let allowed: Vec<&str> = widget_props(kind).map(|p| p.name).collect();
                    for (pk, pv) in pt.iter() {
                        let ppos = key_pos(ctx, pt, pk, pv, pos);
                        match widget_props(kind).find(|p| p.name == pk) {
                            Some(p) => match check_prop(ctx, p, pv, ppos) {
                                Some(val) => {
                                    let vp = item_pos(ctx, pv, ppos);
                                    ctx.pos.insert(format!("page.widget.{i}.props.{pk}"), vp);
                                    props.insert(pk.to_string(), val);
                                }
                                None => ok = false,
                            },
                            None => {
                                ctx.error(ppos, unknown_key(pk, &format!("a {kind} widget's props"), &allowed));
                                ok = false;
                            }
                        }
                    }
                }
                other => {
                    ctx.error(
                        pos,
                        unknown_key(other, "[[widget]]", &["id", "kind", "slot", "column", "edge", "align", "menu", "props", "widget"]),
                    );
                    ok = false;
                }
            }
        }
        if kind.as_deref() == Some(CUSTOM_KIND) && t.get("widget").is_none() {
            ctx.error(
                tpos,
                "a custom widget needs widget = \"<folder name>\" (its folder under ~/.k2/zen/widgets/; make one with k2 zen widget new <name>)",
            );
            ok = false;
        }
        if in_menu && t.get("menu").is_none() {
            ctx.error(tpos, "a menu item (slot = \"menu\") needs menu = \"<menu id>\": the menu that holds it");
            ok = false;
        }
        // A widget whose kind already failed gets no second error for its id.
        if t.get("id").is_none() && kind.is_some() {
            // Outside a column's body (a band, an edge, a menu) the id may be
            // left out: it is then the kind (FC14).
            let optional = slot.is_some_and(|s| s != DEFAULT_SLOT) || chrome || edge.is_some();
            match kind.as_deref().filter(|_| optional) {
                Some(k) if ids.iter().any(|x| x == k) => {
                    ctx.error(tpos, format!("widget id '{k}' (this widget's default id) is used twice; give it its own id"));
                    ok = false;
                }
                Some(k) => {
                    ids.push(k.to_string());
                    w.insert("id".into(), json!(k));
                }
                None => {
                    ctx.error(tpos, "each [[widget]] needs an id (letters, digits, - and _)");
                    ok = false;
                }
            }
        }
        if t.get("column").is_none() && in_column {
            ctx.error(tpos, "each [[widget]] needs a column (0 is the first), or slot = \"top\" for the top band");
            ok = false;
        }
        let Some(kind) = kind else { continue };
        // Rosson 2026-10-04 (answer 5): one agent filtered from a Home, or the
        // whole Home.
        let prop_at = |ctx: &Ctx, name: &str| ctx.pos.get(&format!("page.widget.{i}.props.{name}")).copied().unwrap_or(tpos);
        if kind == "agents" {
            match (props.get("mode").and_then(J::as_str), props.contains_key("agent")) {
                (Some("agent"), false) => {
                    let p = prop_at(ctx, "mode");
                    ctx.error(p, "mode = \"agent\" needs agent = \"<name or address>\": the one agent to show");
                    ok = false;
                }
                (Some("home"), true) => {
                    let p = prop_at(ctx, "agent");
                    ctx.error(
                        p,
                        "agent filters the widget to one agent; use mode = \"agent\" (or drop mode), or drop agent to show the whole Home",
                    );
                    ok = false;
                }
                _ => {}
            }
        }
        if kind == "conversation" {
            if props.contains_key("agents") && props.contains_key("agent") {
                let p = prop_at(ctx, "agent");
                ctx.error(p, "a Conversation either follows an Agents widget (agents) or pins one agent (agent), not both");
                ok = false;
            }
            if props.contains_key("home") && !props.contains_key("agent") {
                let p = prop_at(ctx, "home");
                ctx.error(
                    p,
                    "home picks where `agent` is looked up; set agent too, or follow an Agents widget with agents = \"<widget id>\"",
                );
                ok = false;
            }
        }
        let orientation = props.get("orientation").and_then(J::as_str);
        if kind == "nav-rail" && orientation == Some("column") {
            if let Some(band) = slot.filter(|_| in_band) {
                let p = prop_at(ctx, "orientation");
                ctx.error(
                    p,
                    format!("the {band} band is one row high: a nav-rail there draws as a row (orientation = \"row\", or leave it out)"),
                );
                ok = false;
            } else if edge.is_some() {
                let p = prop_at(ctx, "orientation");
                ctx.error(
                    p,
                    "a column edge is one row high: a nav-rail at an edge draws as a row (orientation = \"row\", or leave it out)",
                );
                ok = false;
            }
        }
        if kind == "nav-rail" && in_column && edge.is_none() && orientation != Some("row") && align.is_some() {
            let p = ctx.pos.get(&format!("page.widget.{i}")).copied().unwrap_or(tpos);
            ctx.error(
                p,
                "align places a row item; this nav-rail is the strip down its column's left side. Set edge = \"top\" or \"bottom\" (or orientation = \"row\") to make it a row, or drop align",
            );
            ok = false;
        }
        if kind == "menu" {
            if let Some(label) = props.get("label").and_then(J::as_str) {
                if label.chars().count() > MENU_LABEL_MAX {
                    let p = prop_at(ctx, "label");
                    ctx.error(p, format!("menu.label is at most {MENU_LABEL_MAX} characters; this one has {}", label.chars().count()));
                    ok = false;
                }
            }
        }
        if let Some(sl) = slot {
            if sl != DEFAULT_SLOT {
                w.insert("slot".into(), json!(sl));
            }
            if let Some(e) = edge {
                w.insert("edge".into(), json!(e));
            }
            if let Some(a) = align {
                w.insert("align".into(), json!(a));
            }
            if let Some((m, _)) = &menu {
                w.insert("menu".into(), json!(m));
            }
            // A row item at a column edge: chrome, an `edge`, or a row rail.
            let row_in_column = in_column && (chrome || edge.is_some() || (kind == "nav-rail" && orientation == Some("row")));
            let at_edge = match (row_in_column, w.get("column").and_then(J::as_u64)) {
                (true, Some(c)) => Some((c, edge.unwrap_or(DEFAULT_EDGE))),
                _ => None,
            };
            let id = w.get("id").and_then(J::as_str).unwrap_or(kind.as_str()).to_string();
            placed.push(Placed { kind: kind.clone(), slot: sl, id, pos: tpos, at_edge, menu: menu.clone() });
        }
        normalize_props_in(ctx.defaults.unwrap_or_else(|| super::Defaults::live()), &kind, &mut props);
        normalize_slot_props(&kind, slot.unwrap_or(DEFAULT_SLOT), edge, &mut props);
        w.insert("kind".into(), json!(kind));
        w.insert("props".into(), J::Object(props));
        out.push(J::Object(w));
    }

    // Limits, counted after the kinds are known (FC47).
    let content = placed.iter().filter(|p| !is_chrome_kind(&p.kind)).count();
    if content > MAX_WIDGETS {
        ctx.error(at, format!("a Garden page holds at most {MAX_WIDGETS} widgets; this one has {content}"));
        ok = false;
    }
    let custom = placed.iter().filter(|p| p.kind == CUSTOM_KIND).count();
    if custom > MAX_CUSTOM_WIDGETS {
        ctx.error(at, format!("a Garden page holds at most {MAX_CUSTOM_WIDGETS} custom widgets; this one has {custom}"));
        ok = false;
    }
    let chrome_n = placed.len() - content;
    if chrome_n > MAX_CHROME_WIDGETS {
        ctx.error(
            at,
            format!("a Garden page places at most {MAX_CHROME_WIDGETS} Zen controls (chrome widgets); this one has {chrome_n}"),
        );
        ok = false;
    }
    // Each Zen control once per page, except `menu` (FC13).
    for (j, p) in placed.iter().enumerate() {
        if is_chrome_kind(&p.kind) && p.kind != "menu" && placed[..j].iter().any(|q| q.kind == p.kind) {
            ctx.error(p.pos, format!("'{}' is placed twice; each Zen control goes on the page once (only menu may repeat)", p.kind));
            ok = false;
        }
    }
    for band in BAND_SLOTS {
        let items: Vec<&Placed> = placed.iter().filter(|p| p.slot == *band).collect();
        let content_in = items.iter().filter(|p| !is_chrome_kind(&p.kind)).count();
        if content_in > MAX_BAND_WIDGETS {
            ctx.error(at, format!("the {band} band holds at most {MAX_BAND_WIDGETS} widgets; this page puts {content_in} there"));
            ok = false;
        }
        if items.len() > MAX_BAND_ITEMS {
            ctx.error(at, format!("the {band} band holds at most {MAX_BAND_ITEMS} items; this page puts {} there", items.len()));
            ok = false;
        }
    }
    let mut edges: BTreeMap<(u64, &'static str), usize> = BTreeMap::new();
    for e in placed.iter().filter_map(|p| p.at_edge) {
        *edges.entry(e).or_default() += 1;
    }
    for ((c, e), n) in edges {
        if n > MAX_EDGE_ITEMS {
            ctx.error(at, format!("column {c}'s {e} edge holds at most {MAX_EDGE_ITEMS} items; this page puts {n} there"));
            ok = false;
        }
    }
    // Menus: each named menu exists; each menu holds 1 to 6 items (FC16).
    let menus: Vec<&Placed> = placed.iter().filter(|p| p.kind == "menu").collect();
    if menus.len() > MAX_MENUS {
        ctx.error(at, format!("a page has at most {MAX_MENUS} menus; this one has {}", menus.len()));
        ok = false;
    }
    let menu_ids: Vec<&str> = menus.iter().map(|m| m.id.as_str()).collect();
    // A menu placed inside a menu is already an error; don't also call it empty.
    let menus: Vec<&Placed> = menus.into_iter().filter(|m| m.slot != MENU_SLOT).collect();
    for p in &placed {
        let Some((m, mpos)) = &p.menu else { continue };
        if p.slot == MENU_SLOT && !menu_ids.contains(&m.as_str()) {
            let known = if menu_ids.is_empty() { String::new() } else { format!(" (menus here: {})", menu_ids.join(", ")) };
            ctx.error(
                *mpos,
                format!("menu = \"{m}\" names no menu on this page; add [[widget]] id = \"{m}\" kind = \"menu\"{known}"),
            );
            ok = false;
        }
    }
    for m in &menus {
        let n = placed.iter().filter(|p| p.slot == MENU_SLOT && p.menu.as_ref().is_some_and(|(x, _)| *x == m.id)).count();
        if n == 0 {
            ctx.error(
                m.pos,
                format!(
                    "menu '{}' holds nothing: put Zen controls in it with slot = \"menu\" and menu = \"{}\", or remove it",
                    m.id, m.id
                ),
            );
            ok = false;
        } else if n > MAX_MENU_ITEMS {
            ctx.error(m.pos, format!("menu '{}' holds at most {MAX_MENU_ITEMS} items; it has {n}", m.id));
            ok = false;
        }
    }
    // One nav rail per page once a rail is in a band (Rosson 2026-10-06).
    if let Some(band_idx) = placed.iter().position(|p| p.kind == "nav-rail" && BAND_SLOTS.contains(&p.slot)) {
        let band_id = placed[band_idx].id.clone();
        let band = placed[band_idx].slot;
        for (j, p) in placed.iter().enumerate() {
            if p.kind != "nav-rail" || j == band_idx {
                continue;
            }
            ctx.error(
                p.pos,
                format!(
                    "the nav rail is already in the {band} band ('{band_id}'); a page draws one nav rail, so remove '{}' or move the rail back to a column",
                    p.id
                ),
            );
            ok = false;
        }
    }
    if ok {
        ctx.pos.insert("page.widgets".into(), at);
        ctx.set("page.widgets".into(), J::Array(out), at);
    }
}

/// G38 rules that need the whole page: every widget's column exists, a
/// Conversation's `agents` names an Agents widget, and a column's `widget`
/// names a widget on the page. Content widgets or columns the file leaves
/// out come from its template.
///
/// prd-zen-freeform-chrome FC24 rule 1 [FC49]: a file that places any
/// chrome replaces the template's, so it must also place both required
/// controls; the error sits at the file's first chrome widget.
fn cross_check_page(ctx: &mut Ctx, at: (usize, usize)) {
    let template = ctx
        .out
        .layer
        .get("page.template")
        .and_then(J::as_str)
        .unwrap_or(ctx.default_template)
        .to_string();
    let file_layout = ctx.out.layer.get("page.layout").cloned();
    let file_widgets = ctx.out.layer.get("page.widgets").cloned();
    if file_layout.is_none() && file_widgets.is_none() {
        return;
    }
    let tpl = match ctx.defaults {
        Some(d) => super::template_page_in(d, &template).or_else(|| super::template_page(&template)),
        None => super::template_page(&template),
    };
    let Some(tpl) = tpl else { return };
    let ncols = match &file_layout {
        Some(l) => l["columns"].as_array().map_or(0, Vec::len),
        None => tpl["layout"]["columns"].as_array().map_or(0, Vec::len),
    };
    // Every file widget with its table index (for positions).
    let file: Vec<(usize, J)> = file_widgets.as_ref().and_then(J::as_array).cloned().unwrap_or_default().into_iter().enumerate().collect();
    let is_chrome = |w: &J| w["kind"].as_str().is_some_and(is_chrome_kind);
    let file_content: Vec<(Option<usize>, J)> =
        file.iter().filter(|(_, w)| !is_chrome(w)).map(|(i, w)| (Some(*i), w.clone())).collect();
    let file_chrome: Vec<(usize, J)> = file.iter().filter(|(_, w)| is_chrome(w)).cloned().collect();
    let from_file = !file_content.is_empty();
    let widgets: Vec<(Option<usize>, J)> = if from_file {
        file_content
    } else {
        tpl["widgets"].as_array().cloned().unwrap_or_default().into_iter().map(|w| (None, w)).collect()
    };
    let layout_at = ctx.pos.get("page.layout").copied().unwrap_or(at);
    let agents_ids: Vec<String> =
        widgets.iter().filter(|(_, x)| x["kind"] == "agents").filter_map(|(_, x)| x["id"].as_str().map(str::to_string)).collect();
    // A Conversation may follow a custom widget too (UW40); whether that
    // widget asks for `agents:read` needs its manifest, so the store checks
    // it. Auto-linking stays Agents-only (`link_conversations`).
    let followable: Vec<String> = widgets
        .iter()
        .filter(|(_, x)| x["kind"] == "agents" || x["kind"] == CUSTOM_KIND)
        .filter_map(|(_, x)| x["id"].as_str().map(str::to_string))
        .collect();
    let column_error = |ctx: &mut Ctx, i: usize, id: &str, col: usize| {
        let p = ctx.pos.get(&format!("page.widget.{i}.column")).copied().unwrap_or(at);
        ctx.error(
            p,
            format!("widget '{id}' is in column {col}, but this page has {ncols} column(s) (0 to {})", ncols.saturating_sub(1)),
        );
    };
    for (idx, w) in &widgets {
        let id = w["id"].as_str().unwrap_or("?").to_string();
        let col = w["column"].as_u64().unwrap_or(0) as usize;
        let in_column = w["slot"].as_str().unwrap_or(DEFAULT_SLOT) == DEFAULT_SLOT;
        if in_column && col >= ncols {
            match idx {
                Some(i) => column_error(ctx, *i, &id, col),
                None => ctx.error(
                    layout_at,
                    format!(
                        "the template's widget '{id}' sits in column {col}, but this layout has {ncols} column(s); add [[widget]] blocks for this layout"
                    ),
                ),
            }
        }
        if w["kind"] == "conversation" {
            if let Some(i) = idx {
                let pinned = w["props"]["agent"].is_string();
                match w["props"]["agents"].as_str() {
                    Some(target) if !followable.iter().any(|a| a == target) => {
                        let p = ctx.pos.get(&format!("page.widget.{i}.props.agents")).copied().unwrap_or(at);
                        let known = if agents_ids.is_empty() { String::new() } else { format!(" ({})", agents_ids.join(", ")) };
                        ctx.error(p, format!("conversation '{id}' follows '{target}', which is not an Agents widget on this page{known}"));
                    }
                    None if !pinned && agents_ids.is_empty() => {
                        let p = ctx.pos.get(&format!("page.widget.{i}.column")).copied().unwrap_or(at);
                        ctx.error(
                            p,
                            format!(
                                "conversation '{id}' has no agent to show: add an Agents widget, or pin one with agent = \"<name or address>\""
                            ),
                        );
                    }
                    _ => {}
                }
            }
        }
    }
    // Chrome at a column edge needs that column (FC24 rule 10).
    for (i, w) in &file_chrome {
        if w["slot"].as_str().unwrap_or(DEFAULT_SLOT) != DEFAULT_SLOT {
            continue;
        }
        let col = w["column"].as_u64().unwrap_or(0) as usize;
        if col >= ncols {
            let id = w["id"].as_str().unwrap_or("?").to_string();
            column_error(ctx, *i, &id, col);
        }
    }
    // FC24 rule 1: chrome from the file must carry both required controls.
    if let Some((first, _)) = file_chrome.first() {
        let p = ctx.pos.get(&format!("page.widget.{first}")).copied().unwrap_or(at);
        for req in super::REQUIRED_CONTROLS {
            if !file_chrome.iter().any(|(_, w)| w["kind"] == *req) {
                ctx.error(p, required_chrome_error(req));
            }
        }
    }
    if let Some(l) = &file_layout {
        let ids: Vec<&str> = widgets.iter().filter_map(|(_, w)| w["id"].as_str()).collect();
        for (i, c) in l["columns"].as_array().into_iter().flatten().enumerate() {
            if let Some(w) = c["widget"].as_str() {
                if !ids.contains(&w) {
                    let p = ctx.pos.get(&format!("page.layout.column.{i}.widget")).copied().unwrap_or(layout_at);
                    ctx.error(p, format!("column {i} names widget '{w}', which is not on this page"));
                }
            }
        }
    }
}

/// Checks that need the layers below: curve names and contrast.
fn cross_check(ctx: &mut Ctx, base: &Layer) {
    let layer = ctx.out.layer.clone();
    for (key, spec) in &layer {
        let Some(name) = key.strip_prefix("animation.") else { continue };
        let Some(curve) = spec.get("curve").and_then(J::as_str) else { continue };
        let known = BUILTIN_BEZIERS.iter().any(|(n, _)| *n == curve)
            || layer.contains_key(&format!("bezier.{curve}"))
            || base.contains_key(&format!("bezier.{curve}"));
        if !known {
            let pos = ctx.pos.get(&format!("{key}#curve")).copied().unwrap_or((1, 1));
            ctx.error(pos, format!("{name}: unknown curve '{curve}'; define it in [bezier] or use one of: {}", BUILTIN_BEZIERS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")));
        }
    }
    for scheme in ["light", "dark"] {
        let get = |tok: &str| -> Option<String> {
            let k = format!("colors.{scheme}.{tok}");
            layer.get(&k).or_else(|| base.get(&k)).and_then(J::as_str).map(str::to_string)
        };
        let here = |tok: &str| layer.contains_key(&format!("colors.{scheme}.{tok}"));
        if !here("canvas") && !here("text") && !here("accent") {
            continue;
        }
        let Some(canvas) = get("canvas").as_deref().and_then(parse_color) else { continue };
        for fg in ["text", "accent"] {
            let Some(raw) = get(fg) else { continue };
            let Some(c) = parse_color(&raw) else { continue };
            let ratio = contrast(c, canvas, scheme == "dark");
            if ratio < MIN_CONTRAST {
                let at_key = if here(fg) { fg } else { "canvas" };
                let pos = ctx.pos.get(&format!("colors.{scheme}.{at_key}")).copied().unwrap_or((1, 1));
                ctx.error(
                    pos,
                    format!(
                        "colors.{scheme}: {fg} {raw} on canvas {} has contrast {ratio:.1}:1; it needs at least 3:1 so the Zen controls stay visible",
                        get("canvas").unwrap_or_default()
                    ),
                );
            }
        }
    }
}

/// Stack layers: later wins per key.
pub fn merge(layers: &[&Layer]) -> Layer {
    let mut out = Layer::new();
    for l in layers {
        for (k, v) in l.iter() {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

fn parent_of(name: &str) -> Option<&'static str> {
    ANIMATION_TREE.iter().find(|(n, _)| *n == name).and_then(|(_, p)| *p)
}

fn chain(name: &str) -> Vec<&str> {
    let mut out = vec![name];
    let mut cur = name;
    while let Some(p) = parent_of(cur) {
        out.push(p);
        cur = p;
    }
    out
}

fn ease_css(b: &[f64]) -> String {
    let f = |v: f64| {
        let s = format!("{v:.3}");
        let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        if s == "-0" { "0".to_string() } else { s }
    };
    format!("cubic-bezier({}, {}, {}, {})", f(b[0]), f(b[1]), f(b[2]), f(b[3]))
}

/// The resolved theme, chrome and motion for one Garden.
///
/// `tokens` is `{scheme, colors: {light, dark}, shape}`; `font` is
/// `{family, stack, monospace, size, lineHeight, terminal: {family, stack,
/// monospace}}`; `terminal` is `{palette: {light, dark}}`; `background` is
/// the bundle's `{image, fit, opacity}` (the store turns `image` into a
/// `data:` URL) or `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTheme {
    pub tokens: J,
    pub font: J,
    pub terminal: J,
    pub background: Option<J>,
    pub chrome: J,
    pub motion: J,
}

/// The resolved `font` object for a family name, size and line height.
pub fn font_json(family: &str, size: J, line_height: J) -> J {
    let (stack, mono) = font_family(family).unwrap_or_else(|| {
        font_family("system").unwrap_or_else(|| panic!("FONT_FAMILIES must carry 'system'"))
    });
    let term = if mono { family } else { TERMINAL_PARTNER };
    let (term_stack, term_mono) = font_family(term)
        .unwrap_or_else(|| panic!("TERMINAL_PARTNER '{TERMINAL_PARTNER}' must be in FONT_FAMILIES"));
    json!({
        "family": family,
        "stack": stack,
        "monospace": mono,
        "size": size,
        "lineHeight": line_height,
        "terminal": { "family": term, "stack": term_stack, "monospace": term_mono },
    })
}

/// Resolve `user` (zen.toml + page override, page winning) over `builtin`.
///
/// Animations follow Z22: a node takes the nearest line set by the user on
/// its chain (itself, then its parents up to `global`); only when the user
/// set none does the builtin chain apply. So a user's `global` slows every
/// animation the user didn't set, builtin leaves included. A line without a
/// style takes the nearest style on the same chain.
pub fn resolve(builtin: &Layer, user: &Layer) -> ResolvedTheme {
    let merged = merge(&[builtin, user]);
    let s = |k: &str| merged.get(k).cloned().unwrap_or(J::Null);

    let schemes = |table: &str, tokens: &[&str]| -> J {
        let mut out = Map::new();
        for scheme in ["light", "dark"] {
            let mut m = Map::new();
            for tok in tokens {
                m.insert((*tok).to_string(), s(&format!("{table}.{scheme}.{tok}")));
            }
            out.insert(scheme.to_string(), J::Object(m));
        }
        J::Object(out)
    };
    let mut shape = Map::new();
    for n in NUM_TOKENS.iter().filter(|n| n.table == "shape") {
        shape.insert(n.key.to_string(), s(&format!("shape.{}", n.key)));
    }
    let tokens = json!({
        "scheme": s("theme.scheme"),
        "colors": schemes("colors", COLOR_TOKENS),
        "shape": shape,
    });
    let family = merged.get("font.family").and_then(J::as_str).unwrap_or("system").to_string();
    let font = font_json(&family, s("font.size"), s("font.line-height"));
    let terminal = json!({ "palette": schemes("terminal", TERMINAL_TOKENS) });
    let background = merged.get("background.image").map(|image| {
        json!({
            "image": image,
            "fit": merged.get("background.fit").cloned().unwrap_or_else(|| json!("cover")),
            "opacity": merged.get("background.opacity").cloned().unwrap_or_else(|| json!(1)),
        })
    });
    let chrome = json!({
        "corners": s("chrome.corners"),
        "stoplights": s("chrome.stoplights"),
        "stoplight-offset": s("chrome.stoplight-offset"),
    });

    let mut beziers = Map::new();
    for (n, b) in BUILTIN_BEZIERS {
        beziers.insert((*n).to_string(), json!(b.iter().map(|v| num_json(*v)).collect::<Vec<_>>()));
    }
    for (k, v) in &merged {
        if let Some(n) = k.strip_prefix("bezier.") {
            beziers.insert(n.to_string(), v.clone());
        }
    }

    let mut animations = Map::new();
    for (name, _) in ANIMATION_TREE {
        let ch = chain(name);
        let find = |layer: &Layer| -> Option<(String, J)> {
            ch.iter().find_map(|n| layer.get(&format!("animation.{n}")).map(|v| (n.to_string(), v.clone())))
        };
        let (from, spec) = find(user)
            .or_else(|| find(builtin))
            .unwrap_or_else(|| ("global".into(), json!({"on": true, "speed": 3, "curve": "glide"})));
        let on = spec.get("on").and_then(J::as_bool).unwrap_or(true);
        let speed = spec.get("speed").and_then(J::as_f64).unwrap_or(0.0);
        let curve = spec.get("curve").and_then(J::as_str).unwrap_or("linear").to_string();
        let style = spec.get("style").cloned().or_else(|| {
            ch.iter().find_map(|n| {
                user.get(&format!("animation.{n}"))
                    .and_then(|v| v.get("style").cloned())
            })
            .or_else(|| {
                ch.iter().find_map(|n| {
                    builtin.get(&format!("animation.{n}")).and_then(|v| v.get("style").cloned())
                })
            })
        });
        let bez: Vec<f64> = beziers
            .get(&curve)
            .and_then(J::as_array)
            .map(|a| a.iter().filter_map(J::as_f64).collect())
            .unwrap_or_else(|| vec![0.0, 0.0, 1.0, 1.0]);
        let duration_ms = if on { (speed * 100.0).round() as i64 } else { 0 };
        animations.insert(
            (*name).to_string(),
            json!({
                "on": on,
                "speed": num_json(if on { speed } else { 0.0 }),
                "durationMs": duration_ms,
                "curve": curve,
                "bezier": bez.iter().map(|v| num_json(*v)).collect::<Vec<_>>(),
                "ease": ease_css(&bez),
                "style": style.unwrap_or(J::Null),
                "from": from,
            }),
        );
    }
    let motion = json!({
        "beziers": beziers,
        "animations": animations,
        // Z22: prefers-reduced-motion turns every Zen animation instant.
        "reducedMotion": "instant",
    });
    ResolvedTheme { tokens, font, terminal, background, chrome, motion }
}

/// 64-bit FNV-1a, hex. Stable across Rust versions.
pub fn fnv_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}
