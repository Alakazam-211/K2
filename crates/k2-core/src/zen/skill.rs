//! The `k2-zen` skill (prd-zen-mode-v1 Z18, T3.2; prd-zen-gardens-v1 G36).
//!
//! Generated from [`super::schema`] and the template tables so every token,
//! range, curve, preset, widget kind and widget prop the validator accepts
//! is documented; a new one without docs fails the content test. Written
//! into a workspace only when this daemon has set Zen up (Z68), so agents
//! on headless servers never see it.

use super::schema::{
    PropType, ALIGNS, ANIMATION_STYLES, ANIMATION_TREE, BACKGROUND_FITS, BACKGROUND_TYPES, BAND_WIDGET_KINDS,
    BEZIER_Y_MAX, BEZIER_Y_MIN, BLANK_TEMPLATE_ID, BUILTIN_BEZIERS, CHROME_KINDS, COLOR_TOKENS,
    COLUMN_MIN_WIDTH_MAX, CONTROL_WARNING, CORNERS, DEFAULT_ALIGN, DEFAULT_EDGE, DEFAULT_SLOT, DRAG_REGION_ERROR,
    EDGES, FILL_WIDGET_KINDS, FONT_FAMILIES, LAYOUT_KINDS, MAX_BACKGROUND_BYTES, MAX_BAND_ITEMS, MAX_BAND_WIDGETS,
    MAX_CHROME_WIDGETS, MAX_COLUMNS, MAX_EDGE_ITEMS, MAX_MENUS, MAX_MENU_ITEMS, MAX_WIDGETS, MENU_ITEM_KINDS,
    MIN_CONTRAST, NUM_TOKENS, SCHEMES, SPEED_MAX_DS, STOPLIGHTS, STOPLIGHT_OFFSET_MAX, TEMPLATE_ID,
    TEMPLATE_WIDGET_KINDS, TERMINAL_PARTNER, TERMINAL_TOKENS, TOP_SLOT, WIDGET_KINDS, WIDGET_PROPS, WIDGET_SLOTS,
};
use super::{chrome_caps, stdlib, BRIDGE_CAPS, BUILTIN_THEMES, BUILTIN_WIDGET_CAPS, DEFAULT_THEME, REQUIRED_CONTROLS};
use crate::contract::{self, gen, Exposure};

/// Frontmatter description line for the skill file.
pub const SKILL_DESCRIPTION: &str =
    "Build Zen Gardens (~/.k2/zen): k2 zen garden, place K2's built-in widgets (a whole Home or one agent) or your own sealed custom widgets (k2 zen widget), themes, tokens, font, motion, validate, reset; agents request, never grant";

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

fn prop_type(t: PropType) -> String {
    match t {
        PropType::Bool => "true or false".into(),
        PropType::Text => "text".into(),
        PropType::OneOf(xs) => format!("one of {}", ticks(xs)),
        PropType::SubsetOf(xs) => format!("a list from {}", ticks(xs)),
    }
}

/// The skill body (markdown, no frontmatter).
pub fn generate_k2_zen_skill() -> String {
    let mut s = String::new();
    s.push_str(
        "# K2 Zen Gardens\n\n\
Zen Mode is a mode of a K2 desktop window. In Zen the window shows a **Garden**:\n\
a personal page that lives on THIS computer. The human can have as many Gardens\n\
as they like and switches between them with the Garden switcher (or Cmd+Option+1-9).\n\
Zen starts with two Gardens. **Garden 1** is the texting page: a Home's agents\n\
beside the conversation with the one picked. **Garden 2** is empty: it's there\n\
for the human to ask you to build it. Garden 2 and every new Garden start\n\
**empty**, with \"Ask your agents to add things to this Garden\", an Ask my\n\
agent button and **Start with the default** (which turns that Garden into the\n\
texting page). + New Garden opens one window: a name and the ready-made Gardens as\n\
cards (the Diary, start with the default, or start empty and ask you).\n\n\
When the human asks you to add something to a Garden, you edit that Garden's\n\
file. A message that starts **\"In my Zen Garden ... (id g-...)\"** means: edit\n\
`~/.k2/zen/gardens/<id>.toml`, then run `k2 zen validate --garden <id>`.\n\n\
## Gardens\n\n\
- `k2 zen garden list [--json]`: index, id, name, template; `*` marks a Garden with its own theme.\n\
- `k2 zen garden new <name> [--texting] [--at <n>]`: a new Garden (empty, or `--texting` for the\n\
  texting page) at position n (default: the end). Prints its id and file.\n\
- `k2 zen garden rename <garden> <name>`: names are 1 to 60 characters and unique (case aside).\n\
- `k2 zen garden reorder <garden> <position>`: positions are 1-based.\n\
- `k2 zen garden template <garden> texting|blank [--force]`: turn a Garden into the texting\n\
  page (the default) or an empty page; same id, name and place. The old file is kept in\n\
  `.history/`. A file with its own layout, widgets or theme tables needs `--force`.\n\
- `k2 zen garden delete <garden>`: the page moves into `.history/` (never lost); the last Garden can't be deleted.\n\n\
`<garden>` is an id (`g-3f9a12c0`) or a name. Ids never change, so renaming or\n\
reordering never touches the file. Only the human turns Zen on (the Zen toggle in\n\
the app's top bar); until then every `k2 zen` verb exits 3.\n\n\
## Files (`~/.k2/zen/`, owned by this computer's K2)\n\n\
- `gardens/<id>.toml`: one Garden's page: its template, its layout of K2's built-in\n\
  widgets, and theme tables that restyle this Garden only. **This is the file you edit.**\n\
- `zen.toml`: the human's theme changes for every Garden, on top of the active theme.\n\
- `themes/<name>/theme.toml`: a theme bundle (colours, font, terminal palette,\n\
  optional background image next to it).\n\
- `gardens.json`: the Garden list. K2 writes it. **Never write gardens.json**; use `k2 zen garden`.\n\
- `active.json`: the active theme. K2 writes it. **Never write active.json**; use `k2 zen theme`.\n\
- `.history/`: the last 20 good versions of each file, and deleted Gardens. K2 writes it. **Never write .history/.**\n\
- `widgets/<name>/`: your custom widgets (see Custom widgets). **You edit these too.**\n\
- `sync.json`, `news.json`, `.defaults/`: which Gardens sync with K2's defaults, which\n\
  Garden news was seen, and K2's defaults that own-copy Gardens sit on. K2 writes\n\
  them. **Never write sync.json, news.json or .defaults/**; read them with\n\
  `k2 zen sync` and `k2 zen news`.\n\
- Widgets in your own Gardens need no permissions: a widget can talk to your agents\n\
  right away. **Never write grants.json** or `pins.json` (K2 never reads them).\n\n\
## Synced with K2's defaults, or its own copy\n\n\
Each Garden's page (template, widget prop defaults, the frame) and theme (the\n\
built-in layers under its theme) either sync with K2's defaults, following K2's\n\
improvements, or are the Garden's own copy that no K2 update changes.\n\
Editing a Garden's file never changes whether it syncs. A synced Garden follows\n\
K2's improvements under your edits; to keep it exactly as it is, the person turns\n\
sync off in Settings → Gardens (you can't: `k2 zen sync <garden> on|off` answers an\n\
agent with 403 `zen_local_only`). Suggest it to the person when it matters.\n\
`k2 zen sync` lists each Garden's state; `k2 zen sync <garden> preview` prints it as\n\
if synced; `k2 zen news` lists new catalog Gardens. `k2 zen guide sync` has more.\n\n\
## Workflow\n\n\
1. `k2 zen garden list` to find the Garden (or make one with `k2 zen garden new`).\n\
2. Edit `~/.k2/zen/gardens/<id>.toml`. Every Zen window reloads on save.\n\
3. Run `k2 zen validate --garden <id>` before you tell the human it's done. It prints\n\
   `file:line:col: message` and exits 1 on any error. A file with errors is NOT\n\
   shown: Zen keeps the last good version and shows the error line.\n\
4. Undo with `k2 zen history --garden <id>` then `k2 zen reset --garden <id> --to <snapshot>`,\n\
   or `k2 zen reset --garden <id>` for the Garden's empty stub.\n\
5. `k2 zen reload` re-reads now; `k2 zen doctor` checks the setup; `k2 zen path`\n\
   prints the folder.\n\n\
`k2 zen guide` is the user guide, and it needs no daemon: short pages (start with\n\
`k2 zen guide gardens`; then `files`, `widgets`, `bands`, `required`, `menus`, `themes`,\n\
`sync`, `examples`, `undo`, `safe-mode`) and whole Garden files you can pipe into a Garden:\n\
`k2 zen guide example --list`, then\n\
`k2 zen guide example <name> --toml > ~/.k2/zen/gardens/<id>.toml`.\n\n\
`k2 zen` talks only to the K2 on this computer (each person's Gardens live on\n\
their own computer), so only an agent on this computer can edit them.\n\n\
## A Garden page\n\n",
    );
    s.push_str(&format!(
        "```toml\n\
schema = 1\n\
template = \"{BLANK_TEMPLATE_ID}\"   # or \"{TEMPLATE_ID}\"\n\
\n\
[layout]\n\
kind = \"columns\"\n\
[[layout.column]]\n\
size = 40          # percent of the width; the sizes add up to 100\n\
min-width = 240    # px\n\
[[layout.column]]\n\
size = 60\n\
min-width = 360\n\
\n\
[[widget]]\n\
id = \"work\"\n\
kind = \"agents\"\n\
column = 0\n\
[widget.props]\n\
home = \"Work\"      # the whole Home\n\
\n\
[[widget]]\n\
id = \"talk\"\n\
kind = \"conversation\"\n\
column = 1\n\
[widget.props]\n\
agents = \"work\"    # shows the agent picked in the \"work\" widget\n\
\n\
[colors.light]     # theme tables restyle this Garden only\n\
accent = \"#065f46\"\n\
```\n\n\
Templates: `{TEMPLATE_ID}` (the texting page: Agents beside Conversation) and\n\
`{BLANK_TEMPLATE_ID}` (empty). The template gives the page its layout, its widgets\n\
and its controls until the file declares its own.\n\n\
- `[layout]`: `kind` is {}; 1 to {MAX_COLUMNS} `[[layout.column]]`, each with `size`\n\
  (percent, the sizes add up to 100) and `min-width` (px, 0 to {}). `canvas = \"full\"`\n\
  gives the page the whole window: the top band floats over the columns, which lose\n\
  their margin and glass box, and the theme's background shows behind everything\n\
  (the Diary); the default is `framed`.\n\
- `[[widget]]`: `id` (letters, digits, - and _; unique on the page), `kind`, `column` (0 is\n\
  the first; it must exist in the layout), and optional `[widget.props]`. At most {MAX_WIDGETS}\n\
  content widgets and {MAX_CHROME_WIDGETS} Zen controls.\n\
- Two groups, two replace rules: declaring any **content** widget (the built-in widgets\n\
  below) replaces the template's content widgets; declaring any **Zen control** (see\n\
  Controls, bands and menus) replaces ALL of the template's controls. A file with only\n\
  content widgets keeps the template's controls, and the other way round.\n\
- `slot`: {} (default `{DEFAULT_SLOT}`). `slot = \"{TOP_SLOT}\"` or `\"bottom\"` puts the widget in the\n\
  page's top or bottom band instead of a column; `slot = \"menu\"` puts a Zen control in a menu.\n\
  Content kinds that fit a band: {}, at most {MAX_BAND_WIDGETS} per band. A band or menu widget has\n\
  no `column`, and its `id` may be left out (it is then the kind).\n\
- A page never names caps: built-in widgets get K2's caps (`caps` is an error).\n\
- `[[control]]` is warned and ignored: \"{CONTROL_WARNING}\".\n\
- Unknown kinds, props and keys are errors at their line.\n\n",
        ticks(LAYOUT_KINDS),
        fmt_num(COLUMN_MIN_WIDTH_MAX),
        ticks(WIDGET_SLOTS),
        ticks(BAND_WIDGET_KINDS),
    ));
    s.push_str(&format!(
        "### The nav rail in the top band\n\n\
To show the nav rail as a row of icons in the top band, right of the Garden\n\
switcher, instead of a strip down a column:\n\n\
```toml\n\
[[widget]]\n\
kind = \"nav-rail\"\n\
slot = \"{TOP_SLOT}\"\n\
```\n\n\
The rail is a content widget, so it replaces the template's widgets (on the texting page\n\
list the others too: `agents` in column 0, `conversation` in column 1) and keeps the\n\
template's controls. A page with the rail in a band draws one rail: another `nav-rail`\n\
on the page is an error. Its `orientation` is `row` there (the only one that fits); in a\n\
column it is `column` unless you set `orientation = \"row\"` or an `edge`.\n\n"
    ));
    s.push_str(&controls_section());
    s.push_str("### Built-in widgets\n\n");
    for (kind, what) in WIDGET_KINDS {
        let caps = BUILTIN_WIDGET_CAPS.iter().find(|(k, _)| k == kind).map(|(_, c)| ticks(c)).unwrap_or_default();
        s.push_str(&format!("**`{kind}`**: {what}. K2 grants it {caps}.\n\n"));
        for p in WIDGET_PROPS.iter().filter(|p| p.kind == *kind) {
            let default = p.default.map(|d| format!(" Default `{d}`.")).unwrap_or_default();
            s.push_str(&format!("- `{}` ({}): {}.{default}\n", p.name, prop_type(p.ty), p.doc));
        }
        s.push('\n');
    }
    for (kind, what) in TEMPLATE_WIDGET_KINDS {
        s.push_str(&format!("`{kind}` is {what}. Only the template places it; a Garden file can't.\n\n"));
    }
    s.push_str(
        "### A whole Home, or one agent\n\n\
- **Whole Home:** `kind = \"agents\"` with `home = \"<Home name or id>\"`: every agent of that\n\
  Home, with a Conversation beside it (`agents = \"<that widget's id>\"`). Add\n\
  `home-picker = true` to let the human switch Homes inside the widget.\n\
- **One agent:** `kind = \"agents\"` with `agent = \"<name or address>\"` (and `home` to say\n\
  which Home it is in): only that agent's row. Or skip the list and pin the\n\
  conversation itself: `kind = \"conversation\"` with `agent = \"<name>\"` and `home`.\n\
- Picking a Home in a Garden never changes the Home page.\n\n",
    );

    s.push_str("## Themes, defaults and your changes\n\n");
    s.push_str(
        "K2's themes are built into the app and read-only. `~/.k2/zen/` holds only the\n\
human's changes, layered on top. App updates never touch those files. For one\n\
Garden the stack is: K2's `basic` theme, then K2's copy of the active theme,\n\
then `themes/<active>/theme.toml`, then `zen.toml`, then `gardens/<id>.toml`.\n\
A later file wins key by key; anything a file leaves out comes from below.\n\n\
One theme is active for the computer, and a Garden may pick its own. K2 keeps\n\
that pick; change it only with `k2 zen theme`.\n\n\
Built-in themes: ",
    );
    s.push_str(
        &BUILTIN_THEMES
            .iter()
            .map(|t| format!("`{}` ({})", t.name, t.summary))
            .collect::<Vec<_>>()
            .join(", "),
    );
    s.push_str(&format!(
        ". Every other theme sits on top of `{DEFAULT_THEME}`.\n\n\
- `k2 zen theme list` lists the themes (built in, and the human's own).\n\
- `k2 zen theme next` / `k2 zen theme prev` cycle them; `k2 zen theme set <name>` picks one.\n\
  Add `--garden <name|id>` for one Garden; `k2 zen theme set --garden <name> --clear` drops a Garden's pick.\n\
- `k2 zen theme new <name> [--from <theme>]` starts a theme bundle in\n\
  `~/.k2/zen/themes/<name>/` from a copy of a theme (default: K2's theme of that\n\
  name, else `{DEFAULT_THEME}`). Using a built-in's name makes an override of it.\n\
- `k2 zen reset --theme <name>` clears it: an override is removed (K2's theme\n\
  shows again); a theme only the human has goes back to a copy of `{DEFAULT_THEME}`.\n\n\
To make a theme: `k2 zen theme new <name>`, edit `~/.k2/zen/themes/<name>/theme.toml`,\n\
add a background image next to it if asked, run `k2 zen validate`, then\n\
`k2 zen theme set <name>` (or `--garden <id>` for one Garden).\n\n"
    ));
    s.push_str(
        "## Rules every file follows\n\n\
- The first line is `schema = 1`. This K2 reads Zen schema 1 only.\n\
- Unknown keys are errors, with the line and a \"did you mean\".\n\
- No raw CSS, no selectors, no HTML, no fonts from the network.\n\
- `[layout]`, `[[widget]]` and `template` belong only in `gardens/<id>.toml`.\n\
- Agents never write grants.json, gardens.json, active.json, sync.json, news.json,\n\
  .defaults/ or .history/: K2 owns them.\n\n",
    );

    s.push_str("## Tokens\n\n### `[theme]`\n\n");
    s.push_str(&format!("- `scheme`: {} (auto follows the computer).\n\n", ticks(SCHEMES)));

    s.push_str("### `[colors.light]` and `[colors.dark]`\n\n");
    s.push_str("Each is a colour: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(r, g, b)` or `rgba(r, g, b, a)`. Each token becomes `--zen-<token>`.\n\n");
    for t in COLOR_TOKENS {
        s.push_str(&format!("- `{t}`\n"));
    }
    s.push_str(&format!(
        "\n`text` and `accent` must reach {}:1 contrast against `canvas` in each scheme, so a theme can't hide the Zen controls.\n\n",
        fmt_num(MIN_CONTRAST)
    ));

    s.push_str("### `[font]` (one font for Zen and its terminals)\n\n");
    s.push_str("- `family`: one of these (system stacks and fonts K2 already bundles; never fonts from the network):\n");
    for (name, _, mono) in FONT_FAMILIES {
        s.push_str(&format!("  - `{name}`{}\n", if *mono { " (monospace)" } else { "" }));
    }
    s.push_str(&format!(
        "  The family applies to the whole Zen page and the agent terminals shown in it.\n  Terminals need fixed-width letters, so a proportional family uses `{TERMINAL_PARTNER}` inside terminals.\n"
    ));
    for n in NUM_TOKENS.iter().filter(|n| n.table == "font") {
        s.push_str(&format!("- `{}`: {} to {}\n", n.key, fmt_num(n.min), fmt_num(n.max)));
    }
    s.push_str("- `[type]` is the old name of `[font]` and is an error.\n");
    s.push_str("\n### `[shape]` (px)\n\n");
    for n in NUM_TOKENS.iter().filter(|n| n.table == "shape") {
        s.push_str(&format!("- `{}`: {} to {}\n", n.key, fmt_num(n.min), fmt_num(n.max)));
    }

    s.push_str("\n### `[terminal.light]` and `[terminal.dark]`\n\n");
    s.push_str("The palette of the agent terminals shown in Zen. Each is a colour, like `[colors.*]`:\n\n");
    for t in TERMINAL_TOKENS {
        s.push_str(&format!("- `{t}`\n"));
    }
    s.push_str("\n### `[background]` (theme bundles only)\n\n");
    s.push_str(&format!(
        "Only in `themes/<name>/theme.toml`; anywhere else it is an error.\n\n\
- `image`: a file name in the theme's own folder (no folders, not hidden), one of {}. No SVG.\n  At most {} bytes (2 MB); the bytes must match the extension; links are refused.\n\
- `fit`: {}\n",
        BACKGROUND_TYPES.iter().map(|(e, _)| format!("`.{e}`")).collect::<Vec<_>>().join(", "),
        MAX_BACKGROUND_BYTES,
        ticks(BACKGROUND_FITS),
    ));
    for n in NUM_TOKENS.iter().filter(|n| n.table == "background") {
        s.push_str(&format!("- `{}`: {} to {}\n", n.key, fmt_num(n.min), fmt_num(n.max)));
    }
    s.push_str("\nA bad image is reported with its file and line, and Zen keeps the theme's last good image.\n");
    s.push_str("\n### `[chrome]` (the window itself)\n\n");
    s.push_str(&format!("- `corners`: {} (macOS window corners).\n", ticks(CORNERS)));
    s.push_str(&format!(
        "- `stoplights`: {} (the window buttons; `hidden` hides them while the theme shows: ⌃⌘Z or the page's Zen toggle leaves Zen, and ⌘W / ⌘M or the menu bar still close and minimize the window).\n",
        ticks(STOPLIGHTS)
    ));
    s.push_str(&format!(
        "- `stoplight-offset = [x, y]`: move the buttons right and down, 0 to {} px each.\n\n",
        fmt_num(STOPLIGHT_OFFSET_MAX)
    ));
    s.push_str(
        "What chrome can't do: K2 can't recolor, resize, reorder or move the window\n\
buttons to the right (only shape, nudge, or `hidden`).\n\
Corners are system or square only. The window is never transparent: no glass,\n\
no blur, no desktop showing through. Linux and Windows have no native chrome to\n\
theme; K2 draws their window controls and menu button in a corner Zen can't cover.\n\n",
    );

    s.push_str("## Motion (Hyprland style)\n\n### `[bezier]`\n\n");
    s.push_str(&format!(
        "`name = [x1, y1, x2, y2]`. x is 0 to 1; y is {} to {} (outside 0–1 overshoots). Names start with a letter. Built in: {}.\n\n",
        fmt_num(BEZIER_Y_MIN),
        fmt_num(BEZIER_Y_MAX),
        BUILTIN_BEZIERS
            .iter()
            .map(|(n, b)| format!("`{n}` [{}]", b.iter().map(|v| fmt_num(*v)).collect::<Vec<_>>().join(", ")))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    s.push_str("### `[animation]`\n\n");
    s.push_str(&format!(
        "`name = [on, speed, curve, style]`. `on` is 1 or 0 (`[0]` alone turns it off). `speed` is tenths of a second, 0 to {}. `curve` is a builtin or a `[bezier]` name. `style` is optional: {}, written like `\"popin 92%\"`. Each line becomes `--zen-anim-<name>-duration` and `--zen-anim-<name>-ease`.\n\n",
        fmt_num(SPEED_MAX_DS),
        ANIMATION_STYLES
            .iter()
            .map(|s| if *s == "popin" { "`popin <n>%`".to_string() } else { format!("`{s}`") })
            .collect::<Vec<_>>()
            .join(", ")
    ));
    s.push_str("The tree (a child takes its parent's line until you set it):\n\n");
    for (name, parent) in ANIMATION_TREE {
        match parent {
            Some(p) => s.push_str(&format!("- `{name}` (inherits `{p}`)\n")),
            None => s.push_str(&format!("- `{name}` (the root)\n")),
        }
    }
    s.push_str(
        "\nA line you set in `zen.toml` beats K2's defaults for every child you didn't\n\
set, so `global = [1, 8, \"glide\"]` slows everything you haven't tuned.\n\
When the computer asks for reduced motion, every Zen animation is instant.\n\n",
    );

    s.push_str("## Controls\n\nEvery Garden page carries the required controls, which K2 binds and checks itself: ");
    s.push_str(&ticks(REQUIRED_CONTROLS));
    s.push_str(
        " (the Zen toggle and the Garden switcher). The template puts the switcher top\n\
left and the toggle top right. A Garden file may move them (Controls, bands and menus),\n\
but it can never drop one: a file that places any control must place both, and\n\
validate refuses it otherwise. If one is hidden, covered or unusable, Zen drops to\n\
safe mode, which draws the template's controls. The human can always leave with Exit\n\
Zen Mode (Ctrl+Cmd+Z on macOS, Ctrl+Alt+Z on Linux and Windows).\n\n",
    );
    s.push_str(&bridge_section());
    s.push_str(
        "Each `agents.list()` / `agents.subscribe(cb)` row has `activity`: what the agent's\n\
server says it is doing (the daemon decides; K2 never guesses). `working`;\n\
`monitoring` (the turn is done, only background work is left); `needs-you` (the\n\
daemon's `waiting`: a permission prompt or a question in the terminal);\n\
`unverifiable` (nothing heard for 30 minutes while the session is open; never\n\
treat it as done); `idle`; or `null` when that server can't say. The Agents\n\
widget's `status` prop keeps three values: `monitoring` shows with `working`, and\n\
`unverifiable` with `idle`. A custom widget's rows also say `unreachable` when\n\
that agent's server is offline.\n\n\
Built-in widgets get their caps from K2. A custom widget asks for caps in its\n\
manifest and gets every one it asks for at once: widgets in your own Gardens\n\
need no permissions.\n\n",
    );
    s.push_str(&custom_widgets_section());
    s.push_str(&whats_available_section());
    s
}

/// "Bridge verbs and caps" (UW42, UWA11): every verb, its cap and its one
/// line come from the verb catalog, never a hand-written list.
fn bridge_section() -> String {
    let c = contract::catalog();
    let mut s = format!(
        "## Bridge verbs and caps\n\n\
Every widget reaches K2 only through the Zen bridge; each verb needs a cap\n\
the widget has (a built-in's from K2, a custom widget's from its manifest).\n\
Caps: {}.\n\n\
This list comes from K2's verb catalog (version {}). A verb marked\n\
**custom widgets** is one a custom widget may call too (see Custom widgets);\n\
the rest belong to K2's own widgets and controls.\n\n",
        ticks(BRIDGE_CAPS),
        c.catalog_version,
    );
    let rows = gen::bridge_rows(c);
    let mut caps: Vec<Option<&str>> = c.caps.iter().map(|cap| Some(cap.name.as_str())).collect();
    caps.push(None);
    for cap in caps {
        let group: Vec<_> = rows.iter().copied().filter(|v| v.cap.as_deref() == cap).collect();
        if group.is_empty() {
            continue;
        }
        match cap {
            Some(name) => s.push_str(&format!("**`{name}`**\n\n")),
            None => s.push_str("**no cap**\n\n"),
        }
        for v in group {
            let custom = if v.exposed_to(Exposure::Widget) { " **custom widgets**" } else { "" };
            s.push_str(&format!("- `{}`: {}{custom}\n", gen::signature(v), v.doc));
        }
        s.push('\n');
    }
    s
}

/// "Custom widgets" (UW42 as amended: UWA11, UWB2–UWB13, UWB21–UWB24).
fn custom_widgets_section() -> String {
    let c = contract::catalog();
    let mut s = String::from(
        "## Custom widgets (your own HTML, CSS and JS)\n\n\
When the built-in widgets can't do what the human asks (\"make my agents look like a\n\
game\", \"a diary\", \"a globe of my servers\"), write a **custom widget**: a small web\n\
page in a folder, placed in a Garden. K2 runs it **sealed**: no network, no storage,\n\
no popups, no files, no K2 internals. It talks to K2 only through the `k2` object,\n\
with the caps its manifest asks for. **Widgets can talk to your agents right away:**\n\
no review, no permission dialog, nothing for the human to allow.\n\n\
### The folder\n\n\
`~/.k2/zen/widgets/<name>/`, one flat folder (`<name>`: lower-case letters, digits,\n\
`-` and `_`, up to 40; subfolders are ignored). K2 reads `manifest.json`, the entry\n\
HTML, `*.js`, `*.css`, and `*.png *.jpg *.jpeg *.webp *.gif *.woff2` (no SVG). K2\n\
inlines it all into one page; `k2.asset(\"cat.png\")` gives a file as a `data:` URL.\n\n\
```json\n\
{\n  \"schema\": 1,\n  \"name\": \"Agent Arcade\",\n  \"description\": \"Your agents as little characters.\",\n  \"entry\": \"index.html\",\n  \"caps\": [\"agents:read\", \"thread:read\", \"thread:post\"],\n  \"reasons\": {\"thread:post\": \"so you can talk to a character from the game\"},\n  \"requires\": {\"libs\": [\"three@0.170\"]}\n}\n\
```\n\n\
- `schema` 1; `name` 1–40 characters (shown to the human as the widget's own words);\n\
  `description` up to 200; `entry` defaults to `index.html`.\n\
- `caps`: only ",
    );
    s.push_str(&ticks(&c.caps_exposed_to(Exposure::Widget)));
    s.push_str(
        ". Any other cap is an error. An empty list is fine (a clock).\n\
- `reasons`: one line (up to 140) per cap, in your words. K2 shows its own sentence too.\n\
- `requires.libs`: libraries from K2's standard library (see What's available),\n\
  inlined before your scripts. `id` is optional and must equal the folder name.\n\
- `version`, `license`, `bindings` are accepted and ignored for now. `net` and `secrets`\n\
  are errors: outside data comes in a later version.\n\n\
### Placing it\n\n\
```toml\n\
[[widget]]\n\
id = \"arcade\"\n\
kind = \"custom\"\n\
widget = \"agent-arcade\"   # the folder, or a built-in like \"k2:diary@1\"\n\
column = 0\n\
[widget.props]\n\
home = \"Work\"             # optional props your code reads (k2.widget, k2.config)\n\
```\n\n\
`custom` is a content widget that fills its column (never a band, edge or menu). At most\n\
6 custom widgets per page. `config` (a small table of strings, numbers and booleans)\n\
reaches the widget as `k2.config`. A conversation widget can follow it:\n\
`agents = \"arcade\"` on a `conversation` widget shows what it opens with\n\
`k2.conversation.open(address)`.\n\n\
### No permissions: it works right away\n\n\
A widget in your own Garden can use every cap its manifest asks for the moment it's\n\
placed: no review card, no grant, no scope to pick, no sending switch. It reaches the\n\
agents on this computer plus every agent on the human's Homes (the rooms and servers\n\
they connected in Home); any other address is `not_bound`, and every server still\n\
checks the human's role. Never write grants.json; there is nothing to grant. K2 keeps\n\
a few invisible rails: the sealed frame, an audit line per post, and a runaway guard:\n\
more than 120 posts in 10 minutes, or 20 identical texts to one agent, pauses posting\n\
(`sending_off`) with a small \"Paused: too many posts. Resume\" notice on the widget\n\
until the human clicks Resume. The widget keeps running meanwhile.\n\n\
### The `k2` object\n\n\
Run `k2 zen guide api` for every helper with an example and its errors. In short, by\n\
cap:\n\n",
    );
    for (cap, rows) in gen::widget_groups(c) {
        match cap.and_then(|n| c.cap(n)) {
            Some(row) => s.push_str(&format!(
                "**`{}`**: \"{}\"\n\n",
                row.name,
                row.sentence.as_deref().unwrap_or(&row.label).replace("{where}", "<scope>")
            )),
            None => s.push_str("**no cap**\n\n"),
        }
        for v in rows {
            let reach = match v.reach {
                contract::Reach::Portable => "portable",
                contract::Reach::Local => "local",
            };
            s.push_str(&format!("- `k2.{}` ({reach}): {}\n", gen::signature(v), v.doc));
        }
        s.push('\n');
    }
    s.push_str(
        "Also: `k2.call(verb, ...args)`, `k2.subscribe(verb, ...args, cb)` (alias `k2.on`),\n\
`k2.connected`, `k2.can(verb)`, `k2.ready()` (call once you have drawn), `k2.config`,\n\
`k2.widget`, `k2.asset(name)`, `k2.motion.reduced`.\n\n\
**`await k2.connected` before `k2.can`, `k2.config`, `k2.widget` or `k2.motion`.** K2\n\
connects the frame after your top-level script runs; until then `k2.can()` is false and\n\
the others are empty, so a widget that checks them at startup draws as if it had no\n\
access. Calls and subscribes wait for the connection on their own. `k2.connected`\n\
rejects (K2Error `failed`, \"not connected\") when K2 never connects the frame:\n\n\
```js\n\
k2.connected.then(() => {\n\
  if (k2.can('agents.subscribe')) k2.agents.subscribe(draw)\n\
  k2.ready()\n\
}, () => showNote('K2 didn’t start this widget.'))\n\
```\n\n\
Every call returns a Promise; every subscribe\n\
returns its unsubscribe. A refused call rejects with a `K2Error` (`.code`: ",
    );
    s.push_str(&c.errors.iter().map(|e| format!("`{}`", e.code)).collect::<Vec<_>>().join(", "));
    s.push_str(
        ").\n\n\
### Rules\n\n\
- **`textContent` for anything an agent wrote, never `innerHTML`.** Agent text can hold markup.\n\
- No inline handlers (`onclick=`): they don't run. Use `addEventListener` in a script file.\n\
- No `fetch`, `XMLHttpRequest`, `WebSocket`, `localStorage`, `eval`, `new Function`,\n\
  `<iframe>` or `import … from`: they're blocked, or validate refuses them.\n\
- Follow the Garden's look: the `--zen-*` CSS variables are set in the frame, and\n\
  `k2.theme.changed(cb)` fires when they change. Respect `k2.motion.reduced`.\n\n\
### Workflow\n\n\
1. `k2 zen widget new <name> --from k2:diary` (or `--from hello`, `--from arcade`) copies a\n\
   working example into `~/.k2/zen/widgets/<name>/`.\n\
2. Edit the folder; place it in a Garden (`kind = \"custom\"`, `widget = \"<name>\"`).\n\
3. `k2 zen validate --widget <name>` and `k2 zen validate --garden <id>` before you say\n\
   it's done. A folder with errors keeps serving its last good version.\n\
4. Tell the human it's ready: it talks to their agents as soon as it's placed.\n\
5. `k2 zen widget list` shows each widget's state and placements; `k2 zen history --widget\n\
   <name>` and `k2 zen reset --widget <name> --to <utc>` undo a bad edit. To stop a\n\
   widget, take its `[[widget]]` out of the Garden file.\n\n\
**The Diary** (`k2:diary@1`, the Diary Garden in New Garden) is the canonical example:\n\
a haunted journal with one page per agent on this computer. The human turns the page\n\
(drags or clicks a corner, or uses the arrow keys) to write to another agent; their ink\n\
sinks in and the reply bleeds back in handwriting. It keeps to this computer's agents,\n\
and its Garden keeps only one ⋯ menu floating over the page (the Garden switcher and\n\
the Zen toggle) and hides the window's stoplights (`stoplights = \"hidden\"`).\n\
Copy it with `k2 zen widget new my-diary --from k2:diary`; a built-in widget itself\n\
never changes.\n\n\
Files K2 owns here too: **never write grants.json** or `pins.json`.\n\n",
    );
    s
}

/// "What's available" (UWB25): the standard library, the limits and the
/// examples, from `zen-lib.json` and the Diary.
fn whats_available_section() -> String {
    let m = stdlib::zen_lib_manifest();
    let mut s = String::from("## What's available to a custom widget\n\n### Libraries (inside K2, pinned, offline)\n\n");
    if m.libs.is_empty() {
        s.push_str("This K2 ships no libraries yet; write plain JavaScript.\n\n");
    } else {
        s.push_str(
            "Name them in the manifest's `requires.libs` as `\"<id>@<version>\"` (a version prefix\n\
that ends at a dot works: `\"three@0.170\"`). Each defines a global.\n\n",
        );
        for l in &m.libs {
            let how = match l.source {
                stdlib::LibSource::Bundled => "bundled",
                stdlib::LibSource::Download => "downloaded on first use, blocked under air-gap",
            };
            let notes = l.notes.as_deref().map(|n| format!(" {n}")).unwrap_or_default();
            s.push_str(&format!(
                "- `{}` ({}, {}): global `{}`, {}, {}.{notes}\n",
                l.key(),
                l.title,
                l.license,
                l.global,
                how,
                kb(l.total_bytes())
            ));
        }
        s.push('\n');
    }
    s.push_str(&format!(
        "Anything else: put the file in the widget's folder (it counts toward the code limit),\n\
or name a pinned CDN file as `{{\"url\": …, \"integrity\": \"sha256-…\"}}` from {}; K2's daemon\n\
fetches and checks it, the frame never touches the network, and air-gap blocks it.\n\n\
### Limits\n\n\
- Code (HTML + JS + CSS after inlining): 256 KB. Each asset 1 MB; all assets 2 MB; the\n\
  bundled page 3 MB. Libraries: up to {} per widget, outside those limits.\n\
- 6 custom widgets per page. Per widget: 60 calls a second, 16 live subscriptions,\n\
  64 KB per message, 8 live servers.\n\
- WebGL, canvas, wasm and blob workers work. No network, storage, popups or dialogs.\n\n\
### Examples to copy\n\n\
- `k2:diary` (the Diary): a full widget with draggable page turns, handwriting and reduced motion.\n\
- `hello`: a clock and a greeting; the smallest widget.\n\
- `arcade`: agents as characters; click one to talk.\n\n",
        m.cdn_hosts.join(", "),
        kb(m.max_bytes_per_widget),
    ));
    s
}

fn kb(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{} MB", (bytes as f64 / (1024.0 * 1024.0) * 10.0).round() / 10.0)
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

/// "Controls, bands and menus" (prd-zen-freeform-chrome FC38): the chrome
/// grammar, generated from the schema tables, with two examples.
fn controls_section() -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "### Controls, bands and menus\n\n\
Before moving controls, read `k2 zen guide bands` and `k2 zen guide required`.\n\n\
K2's controls are widgets too (**Zen controls**). Place them with `[[widget]]` like any\n\
widget. The two required ones are {}: a file that places any Zen control replaces ALL of\n\
the template's and must place both, once each, somewhere the human can reach in one\n\
click. Everything else is optional; a control you don't place isn't shown.\n\n\
- **Where:** `slot = \"top\"` or `\"bottom\"` is the page's top or bottom band (full width).\n\
  `slot = \"column\"` with `column = n` puts a control at that column's edge: `edge` is\n\
  {} (default `{DEFAULT_EDGE}`). `slot = \"menu\"` with `menu = \"<menu id>\"` puts it inside a `menu`.\n\
  Note the difference: `slot = \"top\"` is the page's top band; `slot = \"column\"` with\n\
  `edge = \"top\"` is the top of one column.\n\
- **`align`:** {} (default `{DEFAULT_ALIGN}`): the left end, the middle or the right end of a band\n\
  or column edge. Inside each group items keep file order, and in an `end` group the\n\
  LAST item in the file sits in the corner.\n\
- **Menus** hold only {}: no menu inside a menu, no content widget.\n\
  A menu holds 1 to {MAX_MENU_ITEMS} items; at most {MAX_MENUS} menus on a page. `slot = \"menu\"` always needs `menu`.\n\
- **Limits:** at most {MAX_BAND_ITEMS} items per band, {MAX_EDGE_ITEMS} per column edge. Each Zen\n\
  control goes on the page once, except `menu`.\n\
- {} take no `edge` or `align`: they fill their column.\n\
- Never place `drag-region`: {DRAG_REGION_ERROR} With no top band K2 keeps an empty,\n\
  draggable strip at the top.\n\n",
        ticks(REQUIRED_CONTROLS),
        ticks(EDGES),
        ticks(ALIGNS),
        ticks(MENU_ITEM_KINDS),
        ticks(FILL_WIDGET_KINDS),
    ));
    s.push_str("Zen control kinds:\n\n");
    for (kind, what) in CHROME_KINDS {
        let caps = chrome_caps(kind);
        let granted = if caps.is_empty() { "no caps".to_string() } else { ticks(caps) };
        s.push_str(&format!("**`{kind}`**: {what}. K2 grants it {granted}.\n\n"));
        for p in WIDGET_PROPS.iter().filter(|p| p.kind == *kind) {
            let default = p.default.map(|d| format!(" Default `{d}`.")).unwrap_or_default();
            s.push_str(&format!("- `{}` ({}): {}.{default}\n", p.name, prop_type(p.ty), p.doc));
        }
        if WIDGET_PROPS.iter().any(|p| p.kind == *kind) {
            s.push('\n');
        }
    }
    s.push_str(
        "`bottom-bar`: the Garden switcher bottom left, the theme and the Zen toggle bottom\n\
right, usage not shown, no top band. The template's content widgets stay:\n\n\
```toml\n\
schema = 1\n\
\n\
[[widget]]\n\
kind = \"garden-switcher\"\n\
slot = \"bottom\"\n\
\n\
[[widget]]\n\
kind = \"theme-picker\"\n\
slot = \"bottom\"\n\
align = \"end\"\n\
\n\
[[widget]]\n\
kind = \"zen-toggle\"\n\
slot = \"bottom\"\n\
align = \"end\"\n\
```\n\n\
`menu-both`: both required controls and the theme in one menu, top right:\n\n\
```toml\n\
schema = 1\n\
\n\
[[widget]]\n\
id = \"more\"\n\
kind = \"menu\"\n\
slot = \"top\"\n\
align = \"end\"\n\
[widget.props]\n\
icon = \"dots\"\n\
label = \"More\"\n\
\n\
[[widget]]\n\
kind = \"garden-switcher\"\n\
slot = \"menu\"\n\
menu = \"more\"\n\
\n\
[[widget]]\n\
kind = \"theme-picker\"\n\
slot = \"menu\"\n\
menu = \"more\"\n\
\n\
[[widget]]\n\
kind = \"zen-toggle\"\n\
slot = \"menu\"\n\
menu = \"more\"\n\
```\n\n\
A file that places only a `garden-switcher` fails validate: it replaced the template's\n\
controls, so it must also place a `zen-toggle` (the way out).\n\n",
    );
    s
}

fn ticks(xs: &[&str]) -> String {
    xs.iter().map(|x| format!("`{x}`")).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TUW5.2 (UWB2): skill v12 documents custom widgets from the catalog.
    #[test]
    fn skill_documents_every_custom_verb_with_its_cap() {
        assert!(crate::skills::version::SKILL_VERSION_ZEN > 11, "custom widgets need a new skill version");
        let body = generate_k2_zen_skill();
        let c = contract::catalog();
        for v in c.verbs_exposed_to(Exposure::Widget) {
            let line = format!("- `k2.{}`", gen::signature(v));
            assert!(body.contains(&line), "skill must list {line}");
            if let Some(cap) = &v.cap {
                // The verb's line sits under its cap's heading.
                let head = body.find(&format!("**`{cap}`**: \"")).unwrap_or_else(|| panic!("no heading for {cap}"));
                let at = body.find(&line).expect("line present");
                assert!(at > head, "{} is listed under {cap}", v.verb);
            }
        }
        for v in gen::bridge_rows(c) {
            assert!(body.contains(&format!("- `{}`: {}", gen::signature(v), v.doc)), "bridge list has {}", v.verb);
        }
        for cap in c.caps_exposed_to(Exposure::Widget) {
            assert!(body.contains(&format!("**`{cap}`**: \"")), "the cap {cap} with K2's sentence");
        }
        for must in [
            "**Widgets can talk to your agents right away:**",
            "### No permissions: it works right away",
            "Never write grants.json",
            "Paused: too many posts. Resume",
            "Run `k2 zen guide api`",
            "`k2 zen widget new <name> --from k2:diary`",
            "`k2 zen validate --widget <name>`",
            "## What's available to a custom widget",
            "**never write grants.json** or `pins.json`",
            "textContent",
        ] {
            assert!(body.contains(must), "skill v12 must say {must:?}");
        }
        assert!(!body.contains("nothing is grantable yet"), "the v2 placeholder line is gone");
        assert!(!body.contains("only the app writes it"), "grants live in the daemon now (UWB4)");
        // Rosson 2026-10-08: no permissions. Nothing tells an agent to send
        // the human to a review, a grant or a sending switch.
        for gone in ["click Review", "click **Review**", "K2 draws its own review card", "zen-grant.key", "widget revoke", "Sending is on", "human grants"] {
            assert!(!body.contains(gone), "skill v12 must not say {gone:?}");
        }
    }

    #[test]
    fn whats_available_lists_every_library() {
        let body = generate_k2_zen_skill();
        for l in &stdlib::zen_lib_manifest().libs {
            assert!(body.contains(&format!("`{}`", l.key())), "what's available lists {}", l.key());
        }
        for host in &stdlib::zen_lib_manifest().cdn_hosts {
            assert!(body.contains(host.as_str()), "{host}");
        }
    }
}
