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
use super::{chrome_caps, BRIDGE_CAPS, BUILTIN_THEMES, BUILTIN_WIDGET_CAPS, DEFAULT_THEME, REQUIRED_CONTROLS};

/// Frontmatter description line for the skill file.
pub const SKILL_DESCRIPTION: &str =
    "Build Zen Gardens (~/.k2/zen): k2 zen garden, place K2's built-in widgets (a whole Home or one agent), themes, tokens, font, motion, validate, reset; agents request, never grant";

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
texting page). + New Garden asks the human: start with the default, or start\n\
empty and ask you.\n\n\
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
- `grants.json`: widget permissions (Zen v2). Only the K2 app writes it, when the\n\
  human clicks Allow. **Never write grants.json**, never ask the human to paste\n\
  into it, and never edit it to give yourself or a widget a permission. You may\n\
  request a permission by telling the human what and why; the human grants it in\n\
  the app. Built-in widgets get their caps from K2; nothing is grantable yet.\n\n\
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
`examples`, `undo`, `safe-mode`) and whole Garden files you can pipe into a Garden:\n\
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
  (percent, the sizes add up to 100) and `min-width` (px, 0 to {}).\n\
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
- Agents never write grants.json, gardens.json, active.json or .history/: K2 owns them.\n\n",
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
    s.push_str(&format!("- `stoplights`: {} (the macOS window buttons).\n", ticks(STOPLIGHTS)));
    s.push_str(&format!(
        "- `stoplight-offset = [x, y]`: move the buttons right and down, 0 to {} px each.\n\n",
        fmt_num(STOPLIGHT_OFFSET_MAX)
    ));
    s.push_str(
        "What chrome can't do: macOS doesn't let K2 hide, recolor, resize, reorder or\n\
move the window buttons to the right (`stoplights = \"hidden\"` is an error).\n\
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
Zen Mode (Ctrl+Cmd+Z on macOS, Ctrl+Alt+Z on Linux and Windows).\n\n\
## Bridge verbs and caps\n\n\
Every widget reaches K2 only through the Zen bridge; each verb needs a cap\n\
the widget is granted. Caps: ",
    );
    s.push_str(&ticks(BRIDGE_CAPS));
    s.push_str(
        ".\n\n\
- `agents.list()`, `agents.subscribe(cb)`, `agents.home()`, `agents.setHome(id)`, `agents.local()`, `homes.list()`, `conversation.open(address)`, `conversation.close()`: `agents:read`\n\
- `agents.add({anchor, toggle})`: `agents:add` (opens K2's Add agent picker for the widget's Home; the human picks)\n\
- `presence.get(address)`, `presence.subscribe(cb)`: `presence:read`\n\
- `thread.read(address, {beforeSeq, limit})`, `thread.subscribe(address, cb)`: `thread:read`\n\
- `thread.post(address, text)`, `thread.answer(address, cardId, choice)`, `thread.void(address, cardId)`, `compose.draft(address, text)`: `thread:post`\n\
- `gardens.create(name, template?)`, `gardens.rename(id, name)`, `gardens.delete(id)`: `gardens:manage` (the Garden switcher only, never a widget)\n\
- `gardens.useTemplate(template, {force})`: `gardens:template` (the `garden-empty` widget's Start with the default; only the Garden it is on)\n\
- `focusGroups.get()`, `focusGroups.set(id)`, `focusGroups.subscribe(cb)`: `agents:read` (the app's focus groups, for the Agents view)\n\
- `app.open(page)` (`home`, `agents`, `projects` or `tickets`: switches the Garden's view in this window, inside Zen; Zen stays on), `app.current()`, `app.subscribeCurrent(fn)`, `app.badges()`, `app.subscribe(fn)`: `app:navigate` (the `nav-rail` widget)\n\
- `gardens.list()`, `gardens.current()`, `gardens.switch(id)`, `zen.exit()`, `controls.bind(kind, element, gardenId?)`, `theme.get()`: no cap\n\n\
Built-in widgets get their caps from K2. In Zen v2, a user widget asks for caps in\n\
its manifest and the human grants them in the K2 app. Agents request caps; they\n\
never grant them.\n",
    );
    s
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
