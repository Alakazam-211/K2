//! The `k2-zen` skill (prd-zen-mode-v1 Z18, T3.2).
//!
//! Generated from [`super::schema`] so every token, range, curve and preset
//! the validator accepts is documented; a new token without docs fails the
//! content test. Written into a workspace only when this daemon has a
//! `~/.k2/zen/` folder (Z68), so agents on headless servers never see it.

use super::schema::{
    ANIMATION_STYLES, ANIMATION_TREE, BEZIER_Y_MAX, BEZIER_Y_MIN, BUILTIN_BEZIERS, COLOR_TOKENS,
    CORNERS, FAMILIES, MIN_CONTRAST, NUM_TOKENS, SCHEMES, SPEED_MAX_DS, STOPLIGHTS,
    STOPLIGHT_OFFSET_MAX, TEMPLATE_ID, V2_WARNING,
};
use super::{BRIDGE_CAPS, REQUIRED_CONTROLS};

/// Frontmatter description line for the skill file.
pub const SKILL_DESCRIPTION: &str =
    "Restyle Zen Mode (~/.k2/zen): tokens, motion, chrome, validate, reset; agents request, never grant";

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// The skill body (markdown, no frontmatter).
pub fn generate_k2_zen_skill() -> String {
    let mut s = String::new();
    s.push_str(
        "# K2 Zen\n\n\
Zen Mode is a second face for a Home in the K2 desktop app: the Home's agents\n\
in a list, the conversation with one agent, and a box to message it. Its look\n\
comes from files on THIS computer that you may edit when the human asks you\n\
to restyle Zen.\n\n\
## Files (`~/.k2/zen/`, owned by this computer's K2)\n\n\
- `zen.toml`: the theme for every Home. Edit this.\n\
- `pages/<home-id>.toml`: one page per Home. `schema = 1`, `template = \"",
    );
    s.push_str(TEMPLATE_ID);
    s.push_str(
        "\"`,\n  then any theme table below to override `zen.toml` for that Home only.\n\
- `homes.json`: Home id to name. K2 writes it. **Never write homes.json.**\n\
- `.history/`: the last 20 good versions of each file. K2 writes it. **Never write .history/.**\n\
- `grants.json`: widget permissions (Zen v2). Only the K2 app writes it, when the\n\
  human clicks Allow. **Never write grants.json**, never ask the human to paste\n\
  into it, and never edit it to give yourself or a widget a permission. You may\n\
  request a permission by telling the human what and why; the human grants it in\n\
  the app. Zen v1 has no grantable widgets: K2 ignores grants.json entirely.\n\n\
## Workflow\n\n\
1. Edit `~/.k2/zen/zen.toml` (or a page). Every Zen window reloads on save.\n\
2. Run `k2 zen validate` before you tell the human it's done. It prints\n\
   `file:line:col: message` and exits 1 on any error. A file with errors\n\
   is NOT shown: Zen keeps the last good version and shows the error line.\n\
3. Undo with `k2 zen history` then `k2 zen reset --to <snapshot>`, or\n\
   `k2 zen reset` for the default theme. Add `--home <name|id>` for a page.\n\
4. `k2 zen reload` re-reads now; `k2 zen doctor` checks the setup;\n\
   `k2 zen pages` lists the Homes; `k2 zen path` prints the folder.\n\n\
`k2 zen` talks only to the K2 on this computer (each person's Zen lives on\n\
their own computer). Zen isn't set up until the human turns it on from Home.\n\n\
## Rules every file follows\n\n\
- The first line is `schema = 1`. This K2 reads Zen schema 1 only.\n\
- Unknown keys are errors, with the line and a \"did you mean\".\n\
- No raw CSS, no selectors, no fonts from the network.\n\
- Agents never write grants.json, homes.json or .history/: K2 owns them.\n",
    );
    s.push_str(&format!("- `[layout]` and `[[widget]]` are warned and ignored: \"{V2_WARNING}\".\n\n"));

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

    s.push_str("### `[type]`\n\n");
    s.push_str(&format!("- `family`: {} (system stacks and bundled fonts).\n", ticks(FAMILIES)));
    for n in NUM_TOKENS.iter().filter(|n| n.table == "type") {
        s.push_str(&format!("- `{}`: {} to {}\n", n.key, fmt_num(n.min), fmt_num(n.max)));
    }
    s.push_str("\n### `[shape]` (px)\n\n");
    for n in NUM_TOKENS.iter().filter(|n| n.table == "shape") {
        s.push_str(&format!("- `{}`: {} to {}\n", n.key, fmt_num(n.min), fmt_num(n.max)));
    }

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

    s.push_str("## The page (template ");
    s.push_str(TEMPLATE_ID);
    s.push_str(
        ")\n\nZen v1 ships one page: the Agents widget (the Home's agents, in Home order, each\n\
with its live status: working, idle or needs you, plus its last message) beside the\n\
Conversation widget (that agent's Thread and a box to message it). Layout and\n\
widgets are fixed in v1; you change only the theme. The page always carries the\n\
required controls, which K2 binds and checks itself: ",
    );
    s.push_str(&ticks(REQUIRED_CONTROLS));
    s.push_str(
        ". If one is hidden, covered or unusable, Zen drops to safe mode.\n\
The human can always leave with Exit Zen Mode (Ctrl+Cmd+Z on macOS,\n\
Ctrl+Alt+Z on Linux and Windows).\n\n\
## Bridge verbs and caps\n\n\
Every widget reaches K2 only through the Zen bridge; each verb needs a cap\n\
the widget declares. Caps: ",
    );
    s.push_str(&ticks(BRIDGE_CAPS));
    s.push_str(
        ".\n\n\
- `agents.list()`, `agents.subscribe(cb)`, `conversation.open(address)`, `conversation.close()`: `agents:read`\n\
- `presence.get(address)`, `presence.subscribe(cb)`: `presence:read`\n\
- `thread.read(address, {beforeSeq, limit})`, `thread.subscribe(address, cb)`: `thread:read`\n\
- `thread.post(address, text)`, `thread.answer(address, cardId, choice)`, `thread.void(address, cardId)`: `thread:post`\n\
- `homes.list()`, `homes.select(id)`, `zen.exit()`, `controls.bind(kind, element, homeId?)`, `theme.get()`: no cap\n\n\
Built-in widgets get their caps from K2. In Zen v2, a user widget asks for caps in\n\
its manifest and the human grants them in the K2 app. Agents request caps; they\n\
never grant them.\n",
    );
    s
}

fn ticks(xs: &[&str]) -> String {
    xs.iter().map(|x| format!("`{x}`")).collect::<Vec<_>>().join(", ")
}
