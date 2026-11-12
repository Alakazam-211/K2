# ── k2 zen guide (prd-zen-freeform-chrome FC33, FC34, FC59–FC66) ─────
# A user guide in short static pages, daemon-optional like `k2 study`
# (the connection gate skips `k2 zen guide`). Each page is at most 80
# lines. `k2 zen guide example <name> --toml` prints a whole Garden file,
# ready for `> ~/.k2/zen/gardens/<id>.toml`. tests/cli/zen_guide.sh writes
# every example into a scratch Garden and runs `k2 zen validate`; the
# zen_core Rust test checks every example with the daemon's own schema and
# that the pages name every slot, align, edge and control kind.

_ZEN_GUIDE_TOPICS="gardens files widgets bands required menus phone themes sync examples undo safe-mode api services libs"
_ZEN_GUIDE_EXAMPLES="rail-top quiet-top bottom-bar menu-both texting-chrome menu-bottom-right column-corner"

_zen_guide_usage() {
    cat <<'EOF'
Usage: k2 zen guide [<topic>] [--json]
       k2 zen guide example [<name> [--toml]] [--json]
       k2 zen guide example --list

Short pages on building Zen Gardens. Works with no daemon running.

Topics:
  gardens  files  widgets  bands  required  menus  phone  themes  sync
  examples  undo  safe-mode  api  services  libs

  k2 zen guide                          the index (start here)
  k2 zen guide <topic>                  one page
  k2 zen guide <topic> --json           {"id","title","body"}
  k2 zen guide example                  the example list
  k2 zen guide example <name>           one example with notes
  k2 zen guide example <name> --toml    the file only, ready to pipe:
      k2 zen guide example bottom-bar --toml > ~/.k2/zen/gardens/<id>.toml
  k2 zen guide example --list           bare example names
  k2 zen guide example board-lite [--toml | --script]
                                        a widget kept in its data, and the
                                        agent's script beside it
EOF
}

_zen_guide_title() {
    case "$1" in
        index) echo "Building a Zen Garden" ;;
        gardens) echo "What a Garden is" ;;
        files) echo "The Zen folder" ;;
        widgets) echo "Widgets and the two replace rules" ;;
        bands) echo "Bands, column edges, align and drag" ;;
        required) echo "The two buttons every page keeps" ;;
        menus) echo "Menus" ;;
        phone) echo "A Garden's mobile layout" ;;
        themes) echo "Themes" ;;
        sync) echo "Synced with K2's defaults, or its own copy" ;;
        examples) echo "Examples you can pipe" ;;
        undo) echo "Check, look back, put it back" ;;
        safe-mode) echo "Safe mode and the ways out" ;;
        api) echo "The k2 object in a custom widget" ;;
        libs) echo "K2's shared libraries for widgets" ;;
        services) echo "A widget's data, on this computer only" ;;
        board-lite) echo "Sticky notes in a widget's data" ;;
        rail-top) echo "The nav rail in the top band" ;;
        quiet-top) echo "The top band without usage" ;;
        bottom-bar) echo "Controls in a bottom band" ;;
        menu-both) echo "Both buttons in one menu" ;;
        texting-chrome) echo "The template's controls, written out" ;;
        menu-bottom-right) echo "One menu in the bottom-right corner" ;;
        column-corner) echo "The toggle at a column's corner" ;;
        *) echo "" ;;
    esac
}

_zen_guide_is_in() {
    local want="$1" t
    shift
    for t in $*; do
        [ "$t" = "$want" ] && return 0
    done
    return 1
}

_zen_guide_index() {
    cat <<'EOF'
k2 zen guide — building a Zen Garden

A Garden is a personal page on this computer, shown when a K2 window is in
Zen Mode. Its file is ~/.k2/zen/gardens/<id>.toml. Edit the file, run
k2 zen validate --garden <id>, and every Zen window reloads.

Start here: k2 zen guide gardens

  gardens     What a Garden is, ids, the switcher, Cmd+Option+1-9
  files       The Zen folder: what you edit, what you never write
  widgets     Content widgets, Zen controls and the two replace rules
  bands       Where things go: bands, column edges, align, corners, drag
  required    The two buttons every page keeps; what validate refuses
  menus       A menu button that holds controls
  phone       A Garden's mobile layout: screens, heights, phone settings
  themes      Colours, font, shape, motion; k2 zen theme
  sync        Synced with K2's defaults, or the Garden's own copy
  examples    Whole Garden files you can pipe into a Garden
  undo        validate, history, reset, reload, doctor; troubleshooting
  safe-mode   Every safe-mode line, its fix, and the ways out
  api         The k2 object in a custom widget: every helper, by cap
  services    A widget's data: k2.store, revs, live changes, grants
  libs        K2's shared libraries: which to pick, lines, sizes

  k2 zen guide <topic> [--json]
  k2 zen guide example <name> [--toml]

This guide works with no daemon running. Every widget prop and theme token
is in the k2-zen skill.
EOF
}

_zen_guide_page_gardens() {
    cat <<'EOF'
k2 zen guide gardens — what a Garden is

Zen Mode is a mode of one K2 desktop window. In Zen the window shows a
Garden: a personal page that lives on this computer. Nobody else on the
server sees it.

THE FIRST TWO
  Garden 1   the texting page: a Home's agents beside a conversation.
  Garden 2   empty: "Ask your agents to add things to this Garden", with
             Ask my agent and Start with the default.
  Every new Garden starts empty too (k2 zen garden new <name> --texting
  starts it as the texting page).

IDS AND NAMES
  Each Garden has an id like g-3f9a12c0. Ids never change, so renaming or
  reordering never touches the file. Wherever k2 zen takes <garden>, an id
  or a name works.

  k2 zen garden list                    index, id, name, template
  k2 zen garden new <name> [--texting] [--at <n>]
  k2 zen garden rename <garden> <name>
  k2 zen garden reorder <garden> <position>       (1-based)
  k2 zen garden template <garden> texting|blank [--force]
  k2 zen garden delete <garden>         kept in .history/; the last stays

SWITCHING
  The Garden switcher shows this Garden's name and opens the Garden list
  and + New Garden. Cmd+Option+1-9 (Super+Alt+1-9 on Linux) switch Gardens
  even when no switcher is on screen.

SYNC
  Each Garden's page and theme either sync with K2's defaults (they get
  K2's improvements) or are its own copy (K2 updates never change it).
  Editing the file never changes which; the person does, in Settings →
  Gardens. k2 zen sync lists them; k2 zen guide sync.

WHEN THE HUMAN ASKS
  "In my Zen Garden ... (id g-...)" means: edit
  ~/.k2/zen/gardens/<id>.toml, then run k2 zen validate --garden <id>.
  Only the human turns Zen on. Until Zen is set up, every k2 zen verb
  that reads the folder exits 3.

NEXT
  k2 zen guide files · widgets · bands · examples
EOF
}

_zen_guide_page_files() {
    cat <<'EOF'
k2 zen guide files — the Zen folder (~/.k2/zen)

It belongs to this computer's K2. k2 zen path prints it.

YOU EDIT
  gardens/<id>.toml          one Garden's page: its template, layout,
                             widgets, and theme tables for it alone
  zen.toml                   theme changes for every Garden
  themes/<name>/theme.toml   a theme bundle (and its background image)
  widgets/<name>/            a custom widget (k2 zen guide api)

NEVER WRITE (K2 writes them)
  gardens.json   the Garden list: use k2 zen garden
  active.json    the active theme: use k2 zen theme
  .history/      the last 20 good versions of each file
  sync.json      which Gardens sync with K2's defaults: k2 zen sync
  news.json      which Garden news was seen: k2 zen news
  .defaults/     K2's defaults that own-copy Gardens sit on
  grants.json    K2 never reads it: widgets in your own Gardens need no
                 permissions. Also never pins.json.

LAYERS (a later file wins, key by key)
  1. K2's basic theme
  2. K2's copy of the active theme
  3. themes/<active>/theme.toml
  4. zen.toml
  5. gardens/<id>.toml

A GARDEN FILE
  schema = 1                     always the first line
  template = "k2.texting@1"      or "k2.blank@1"; without it the Garden
                                 keeps the template it was made with
  [layout]                       kind = "columns"
  [[layout.column]]              1 to 3: size (percent, adds up to 100)
                                 and min-width (px); collapsible = true
                                 folds it to a tab (collapsed = true
                                 starts folded, label = "Chat"); never a
                                 column with zen-toggle, garden-switcher
                                 or nav-rail, and one stays open.
                                 Widgets stacked in a column are separate
                                 tiles; joined = true keeps one panel
  [[widget]]                     see k2 zen guide widgets and bands
  [[phone.screen]]               a mobile layout: see k2 zen guide phone
  [colors.light]  [font]  ...    see k2 zen guide themes

The template gives the page its layout, its content widgets and its Zen
controls until the file declares its own.

RULES
  Unknown keys are errors at their line, with a "did you mean".
  No raw CSS, no selectors, no HTML, no fonts from the network.
  [layout], [[widget]], [[phone.screen]] and template go only in gardens/<id>.toml.
  A file with errors is not shown: Zen keeps the last good version.
EOF
}

_zen_guide_page_widgets() {
    cat <<'EOF'
k2 zen guide widgets — what you place, and the two replace rules

Everything on a Garden page is a [[widget]]. Widgets come in two groups,
and each group replaces only the template's own group:

  1. Any CONTENT widget (agents, conversation, nav-rail) replaces the
     template's content widgets. The template's Zen controls stay. A
     nav-rail with slot = "top" keeps the switcher and the toggle.
  2. Any ZEN CONTROL (garden-switcher, zen-toggle, usage, theme-picker,
     menu) replaces ALL of the template's controls. A control you don't
     place is not shown, so the file must place the two required ones.

WRONG FILE (don't pipe it): only a switcher, under schema = 1
    [[widget]]
    kind = "garden-switcher"
    slot = "bottom"
k2 zen validate prints (line 3 is that widget's first key):
  gardens/<id>.toml:3:1: This file places Zen controls, so it replaces the template's. It must also place a zen-toggle (the way out)
Fix: place a zen-toggle too (k2 zen guide required).

CONTENT WIDGETS (in a column they need id, kind and column = n)
  agents        a Home's agents with live status and their last message:
                home = "<Home>" for the whole Home, or agent = "<name>"
                for one agent (mode follows); home-picker, order,
                server-tag, preview, status, pins,
                pin-limit, settings, recent-first, unread;
                or list-from = "<custom widget id>" (mode list): just
                the agents that widget lists with k2.agents.show([...]).
                The person drags rows (or Alt+Up/Down) and pins or
                unpins agents in it, per Garden on this computer; their
                order beats the widget's. Agents can read it with
                k2 zen agents-order, never set it.
  conversation  one agent's Thread and a message box: agents = "<id of
                an Agents widget>" to follow it, or agent = "<name>"
                (with home) to pin one, or pick = ["sales", "ops"] (2 to
                8) to switch between a few from its header, no list
                needed, or pick = "all" for any agent (a search field;
                with agents = "<custom widget id>" the two share one
                pick); compose, attachments, load-older,
                activity-drawer, mute
  nav-rail      My Home, Agents, Projects, Tickets: a strip at the left
                of its column, or a row in a band or at a column edge;
                orientation = "row" | "column"

ZEN CONTROLS (K2 draws them; outside a column's body the id may be left
out, and is then the kind)
  garden-switcher   required: the way to your other Gardens
  zen-toggle        required: the way out of Zen
  usage             optional: the subscription usage chip and its menu
  theme-picker      optional: the theme control
  menu              optional: a button that holds other Zen controls

LIMITS
  12 content widgets and 10 Zen controls per page. Each Zen control at
  most once, except menu. A page never names caps: K2 grants them.

ON A PHONE (k2 zen guide phone has the layout)
  k2.view is 'phone' in K2 Companion, else 'desktop'; await k2.connected first.
  k2.viewport is the visible area in CSS px, or null. k2.setHeight(px) asks for a
  height (auto-height widgets only need it to fit by hand). k2.onVisible(fn) hears
  true/false as the frame leaves or comes back: stop timers and re-reads on false.
  k2.save.* on a phone is the phone's own copy, so panning never moves the desktop.
  Handle touch yourself: matchMedia('(pointer: coarse)'), touch-action, pointers. A
  phone never drops files, so keep a pick button. Use k2-png@1, not canvas.toBlob.

Where each one may go: k2 zen guide bands. Every prop: the k2-zen skill.
EOF
}

_zen_guide_page_bands() {
    cat <<'EOF'
k2 zen guide bands — where things go

THE PLACES (slot = "column" | "top" | "bottom" | "menu")
  top band      a full-width row above the columns        slot = "top"
  bottom band   a full-width row below the columns        slot = "bottom"
  column edge   a short row at the top or bottom of one column:
                slot = "column" (the default), column = n,
                edge = "top" | "bottom" (default "top")
  menu          inside a menu button: slot = "menu", menu = "<menu id>"

TOP BAND OR TOP OF A COLUMN? Two different places:

  slot = "top"                       slot = "column"
                                     column = 1
                                     edge = "top"

  the page's top band: full width,   the top of column 1 only, inside
  above every column                 that column's box

ALIGN, ORDER AND CORNERS
  align = "start" | "center" | "end" (default "start"): the left end, the
  middle or the right end of a band or a column edge. Inside each group,
  items keep file order. In an "end" group the LAST item in the file sits
  in the corner. There is no order key.
  The bottom-right corner: slot = "bottom", align = "end", last in file.

WHAT FITS WHERE
  bands         every Zen control, and nav-rail (drawn as a row)
  column edge   every Zen control, and a nav-rail with an edge or
                orientation = "row"
  menu          zen-toggle, garden-switcher, theme-picker, usage
  agents and conversation fill their column: no edge, no align.
  A band item or a menu item has no column.

BANDS EXIST ONLY WHEN THEY HOLD SOMETHING
  With no top band, K2 keeps a thin, empty title strip as tall as the
  window buttons, so the window still drags and the macOS buttons never
  sit on your page. Nothing can be placed in it. With no bottom band,
  none is drawn. Columns always start below the top band or title strip.

DRAG
  Never place drag-region. The empty space in every band IS the drag
  area. validate refuses kind = "drag-region" with:
    K2 makes the empty space in every band drag the window; there is nothing to place.
  Each band keeps at least 120 px of empty space. Column edges don't drag.

LIMITS
  8 items per band (at most 2 content widgets), 4 per column edge.

Examples: k2 zen guide examples. The required pair: k2 zen guide required.
EOF
}

_zen_guide_page_required() {
    cat <<'EOF'
k2 zen guide required — the two buttons every page keeps

THE RULE
  zen-toggle        the way out of Zen
  garden-switcher   the way to your other Gardens
  A file that places ANY Zen control replaces all of the template's, so
  it must place both of these, each exactly once. Everything else
  (usage, theme-picker, menu) is optional.

REACHABLE MEANS
  1. Placed once: in a band, at a column edge, or as a top-level item of
     a menu that sits in a band or at a column edge.
  2. One click away: its own button is on screen, or the menu button that
     holds it is. Never two clicks: no menu inside a menu.
  3. On screen: shown, not faded below 30% opacity, at least 24x24 px,
     inside the window, clear of the window buttons, not covered.
  4. Keyboard: Tab reaches the button; a menu opens with Enter or Space.
  5. Wired: using it does its job within 1 second.
  The app checks this. Twice unreachable in a row: safe mode
  (k2 zen guide safe-mode).

FAILING FILE (don't pipe it): only a switcher
    schema = 1
    [[widget]]
    kind = "garden-switcher"
    slot = "top"
k2 zen validate prints:
  gardens/<id>.toml:3:1: This file places Zen controls, so it replaces the template's. It must also place a zen-toggle (the way out)
and exits 1. Zen keeps showing the last good version. Fix: add
    [[widget]]
    kind = "zen-toggle"
    slot = "top"
    align = "end"

WHAT VALIDATE REFUSES (an error at its line)
  - Zen controls without a zen-toggle, or without a garden-switcher.
  - A Zen control placed twice (only menu may repeat).
  - kind = "drag-region" (K2 does the drag; k2 zen guide bands).
  - slot = "menu" without menu = "<id>"; a menu = that names no menu; a
    menu = on any other slot.
  - A menu inside a menu; a content widget in a menu; a menu holding
    nothing, or more than 6 items.
  - align in a menu or on agents or conversation; edge outside
    slot = "column", or on agents or conversation.
  - Over the limits: 12 content widgets, 10 Zen controls, 8 items per
    band, 4 per column edge, 3 menus.
  - A kind that doesn't fit its slot; a column the layout doesn't have.
  - caps anywhere.

Validate doesn't judge pixels. The app keeps the required pair on
screen: in a crowded band, usage hides first, then the theme control.
EOF
}

_zen_guide_page_menus() {
    cat <<'EOF'
k2 zen guide menus — a button that holds controls

A menu is a button K2 draws in a band or at a column edge. It holds the
Zen controls that name it, in file order. A fragment (not a whole file):

    [[widget]]
    id = "more"
    kind = "menu"
    slot = "top"
    align = "end"
    [widget.props]
    icon = "dots"       # "dots" (the default), "bars" or "zen"
    label = "More"      # up to 24 characters: shown, tooltip, screen reader

    [[widget]]
    kind = "zen-toggle"
    slot = "menu"       # always both keys: slot = "menu" AND menu = "<id>"
    menu = "more"

WHAT IT HOLDS
  zen-toggle, garden-switcher, theme-picker and usage. Never a menu (no
  menu inside a menu) and never a content widget. 1 to 6 items each; at
  most 3 menus on a page. A menu without an id gets the id "menu".

IN THE OPEN MENU
  garden-switcher   a Gardens section: every Garden with its
                    Cmd+Option+N, then + New Garden
  zen-toggle        Exit Zen Mode (Ctrl+Cmd+Z)
  theme-picker      Theme: <name>, which opens the theme list, with Back
  usage             Usage, which opens the usage panel, with Back
  K2 adds separators between sections.

HOW IT OPENS
  Click, or Enter, Space or Down on the focused button. Never on hover.
  It opens toward the page: down from the top band or a top edge, up from
  the bottom band or a bottom edge, and flips when there's no room.
  It closes on a pick, Esc, Tab, or a click outside.

KEYBOARD
  Up and Down move and wrap; Home and End jump; Enter and Space pick.
  In a sub-panel, Left or Esc goes back. Esc at the top closes the menu
  and puts focus back on its button. Tab closes it and moves on.

A required control in a menu is reachable when the menu button is on
screen (k2 zen guide required). Whole files: k2 zen guide example
menu-both, and k2 zen guide example menu-bottom-right.
EOF
}

_zen_guide_page_phone() {
    cat <<'EOF'
k2 zen guide phone — a Garden's mobile layout

Every Garden shows on a phone in K2 Companion. With no [[phone.screen]] it uses
the default layout: one screen per column, every widget. Add them to choose the
screens, leave widgets out and give a custom widget its own phone settings.
Nothing changes on the computer. There's no preview: look in K2 Companion.

Every device lists the same Gardens. You can't hide a Garden on a phone or show
one only there. A widget you leave out just doesn't load on a phone.

THE TABLE (in the Garden's file, beside [layout] and [[widget]])
    [[phone.screen]]
    label = "Fleet"             # optional: 1 to 24 characters, names its tab
    canvas = "full"             # optional: "full", or "framed" (the default)

    [[phone.screen.widget]]
    id = "fleet"                # a widget on this page
    height = "fill"             # optional: "fill", "auto" or px, 160 to 4000
    [phone.screen.widget.config]
    bubble = false              # optional, custom widgets only

    [[phone.screen]]
    label = "Talk"

    [[phone.screen.widget]]
    id = "agents"

    [[phone.screen.widget]]
    id = "talk"
    height = 420

A phone layout has 1 to 12 screens. A widget sits on one screen. Controls (the
switcher, the toggle, menus) aren't listed: the phone draws its own.

WRITE AND CHECK
  k2 zen garden put <id> --from page.toml     needs the Garden's list, like any
                                              edit; a bad [phone] writes nothing
  k2 zen validate --garden <id> [--from page.toml]
                                              the same rules, with file:line:col

HEIGHT
  fill   fills the screen. The widget must be alone on it. The default when alone.
  auto   starts at 60% of the screen and fits the widget's content. The default
         in a stack.
  420    that many px: at least 160, at most 3x the visible height.
  A screen with several widgets scrolls, so they may add up past the screen.
  With no layout: fill when the widget is alone in its column, auto otherwise.
  canvas = "full" gives the widget the whole screen, with the phone's own controls
  floating over it. It holds one widget. As on a desktop, it takes touches under
  those controls with k2.canvas.setHitRegions(rects).

LISTING A CONVERSATION
  A Conversation follows its Agents widget: list both, or validate warns.

CONFIG
  Merged over the desktop's [widget.props.config], key by key: write only what
  differs. Sources stay bound that way. A key can be overridden but not removed.
  The merged config is at most 4 KB. Only custom widgets take one. It reaches the
  widget as k2.config. A key does nothing unless the widget's code reads it.

SAVES
  On a phone k2.save.* writes the phone's own copy, shared by the person's phones.
  A key the phone hasn't set reads the desktop's value. save.remove drops the
  phone's copy, so the desktop's shows again. Nothing a phone does changes the
  desktop's saves. k2.store is the widget's data: one for every device. K2 doesn't
  choose between k2.config and a saved value: the widget's code does.

IN THE WIDGET (k2 zen guide widgets has the rest)
  k2.view === 'phone', k2.viewport, k2.setHeight(px), k2.onVisible(fn)

SOURCES
  k2.sources.* work on a phone for what the merged config binds, on this server
  only: another server's answers server_not_saved. Refresh: once per 15 s.

ON A TEMPLATE
  A file with only [phone] keeps the template's page. If a K2 update takes a widget
  off the template, the phone leaves it out, with a warning, and the file stays
  good. Against your own [[widget]] blocks a wrong id is an error.

k2 zen widget show <name> lists the sources each placement binds, a phone's too.
EOF
}

_zen_guide_page_themes() {
    cat <<'EOF'
k2 zen guide themes — colours, font, shape, motion

One theme is active for this computer, and each Garden may pick its own.
K2's themes are built in and read-only: basic, paper, midnight, haunted
(the Diary's candlelit study, a full-window background), glass (Garden 1's
default: K2 Companion's look).

  k2 zen theme list [--garden <g>]
  k2 zen theme next | prev [--garden <g>]
  k2 zen theme set <name> [--garden <g>]
  k2 zen theme set --garden <g> --clear      follow the computer again
  k2 zen theme new <name> [--from <theme>]   a bundle in themes/<name>/
  k2 zen reset --theme <name>                drop your override

In the app, Ctrl+Cmd+. and Ctrl+Cmd+Shift+. cycle themes (Ctrl+Alt+. on
Linux), even on a page that places no theme-picker.

THEME TABLES (zen.toml, a theme bundle, or one Garden's file)
  [theme]                       scheme = "auto" | "light" | "dark"
  [colors.light] [colors.dark]  canvas, surface, text, accent, ... each a
                                colour; text and accent need 3:1 contrast
                                against canvas
  [font]                        family (system, rounded, serif, mono,
                                meslo, ...), size, line-height
  [shape]                       radius, bubble-radius, gap, list-width,
                                drawer-radius (optional)
  [terminal.light] [terminal.dark]   the agent terminals' palette
  [chrome]                      the window itself: corners, stoplights,
                                stoplight-offset (not the Zen controls)
  [bezier] [animation]          motion, Hyprland style
  [background]                  only in themes/<name>/theme.toml: image,
                                fit, opacity

MAKE A THEME
  k2 zen theme new sunset
  edit ~/.k2/zen/themes/sunset/theme.toml
  k2 zen validate --theme sunset
  k2 zen theme set sunset              (or --garden <g> for one Garden)

ONE GARDEN ONLY
  Put theme tables straight into that Garden's file:
    [colors.light]
    accent = "#065f46"

Every token and its range: the k2-zen skill.
EOF
}

_zen_guide_page_examples() {
    cat <<'EOF'
k2 zen guide examples — whole Garden files you can pipe

Each example is a whole Garden file. It names template = "k2.texting@1",
so it shows Agents beside a Conversation on any Garden.

  rail-top            the nav rail as a row in the top band; the template's
                      controls stay (it does NOT remove the switcher)
  quiet-top           the template's top band without usage
  bottom-bar          switcher bottom left; theme and toggle bottom right;
                      no usage, no top band
  menu-both           switcher, theme and toggle in one menu, top right
  texting-chrome      the template's controls written out: a start point
  menu-bottom-right   one menu in the bottom-right corner holds them all
  column-corner       the toggle at the bottom-right of the conversation
                      column (edge, not a band)

  k2 zen guide example <name>           the file with notes
  k2 zen guide example <name> --toml    the file only
  k2 zen guide example --list           bare names

TRY ONE
  k2 zen garden list                    find the Garden's id
  k2 zen guide example bottom-bar --toml > ~/.k2/zen/gardens/<id>.toml
  k2 zen validate --garden <id>

Piping replaces the whole file. To keep your own widgets, copy only the
Zen-control [[widget]] tables into your file instead. To go back:
k2 zen guide undo.

A WIDGET EXAMPLE
  k2 zen guide example board-lite   sticky notes kept in the widget's data,
                                    with the script an agent uses beside it
EOF
}

_zen_guide_page_sync() {
    cat <<'EOF'
k2 zen guide sync — synced with K2's defaults, or its own copy

A Garden file holds only its changes. Everything else comes from K2's
defaults: the template's layout, widgets and controls, every widget prop
the file doesn't set, the built-in theme under its theme, and the frame
(sizes, gaps, glass) the app draws.

TWO PARTS, TWO STATES
  page    the template, prop defaults and frame
  theme   the built-in theme layers under the Garden's theme
  synced     follows K2's improvements. Your edits sit on top.
  own copy   K2 keeps the defaults this Garden sat on; no K2 update
             changes it. The Garden file is never rewritten.

WHO DECIDES
  The setting is the truth. Editing a Garden's file never changes it.
  Only the person turns sync on or off, in Settings → Gardens ("Keep in
  sync with future template updates": page and theme together) or k2 zen
  sync <garden> on|off. An agent gets 403 zen_local_only: suggest it to
  the person instead.
  Start with the default (k2 zen garden template) and k2 zen reset
  --garden <id> turn page sync on; reset --to <snapshot> leaves it.
  New Gardens start synced.

LOOK
  k2 zen sync                       every Garden: page and theme state,
                                    own changes, newer default
  k2 zen sync <garden> preview      the Garden as if synced (no write)
  k2 zen news                       new catalog Gardens; defaults a
                                    synced Garden picked up

CHANGE (the person)
  k2 zen sync <garden> on|off [--page|--theme]
  k2 zen sync <garden> undo         the state before the last change
  k2 zen sync <garden> keep-previous [--page|--theme]
                                    after an update changed a synced
                                    Garden: back to the previous defaults
  k2 zen news seen <id>... | --all

WHAT STILL CHANGES FOR EVERY GARDEN
  What widgets show and do, security and accessibility fixes, K2's
  safety UI (safe mode, pause notices, banners), and everything outside
  the Garden. New options appear only when a file asks for them.

LOCK (the person)
  "Lock garden" in Settings → Gardens: agents can't change it. K2 puts
  back edits to its file (kept in .history/gardens/<id>.toml/refused/)
  and refuses k2 zen changes to it with exit 3. Its layout, widgets and
  widget code stay as they were; themes still change it, and K2's
  template updates still reach it while sync is on.

FILES (K2 writes them; never edit)
  sync.json, news.json, .defaults/<fp>.json, .locks/<id>/, and a copy of
  each Garden's state in .history/gardens/<id>.toml/sync.json.
  k2 zen doctor checks them.
EOF
}

_zen_guide_page_undo() {
    cat <<'EOF'
k2 zen guide undo — check, look back, put it back

CHECK
  k2 zen validate --garden <id>   file:line:col: message for each error,
                                  then exit 1. A file with errors is NOT
                                  shown: Zen keeps the last good version.
  k2 zen validate                 every file
  k2 zen reload                   re-read now (K2 also watches the folder)
  k2 zen doctor                   folder, Garden list, templates, watcher

LOOK BACK
  k2 zen history --garden <id>    the last 20 good versions, newest
                                  first (deleted Gardens too)

PUT IT BACK
  k2 zen reset --garden <id> --to <snapshot>   one version from history
  k2 zen reset --garden <id>                   the Garden's stub
  k2 zen reset --theme <name>                  drop a theme override
  k2 zen reset                                 zen.toml back to empty
  Every reset keeps the current file as a snapshot first.

START OVER FROM A TEMPLATE
  k2 zen garden template <garden> texting|blank [--force]
  A file with its own layout, widgets (Zen controls count) or theme
  tables needs --force. The old file is kept in .history/. It also turns
  the Garden's page sync on (k2 zen sync <garden> undo puts it back).

KEEP IT EXACTLY AS IT IS
  Ask the person to turn sync off in Settings → Gardens: then no K2
  update changes the Garden. k2 zen guide sync.

TROUBLESHOOTING
  validate is clean, but the open app still shows the old top strip:
    the app is older than K2 on this computer (the daemon). An older app
    ignores where a Garden puts its controls and keeps drawing its own
    top band, with both required controls. Update the app
    (k2 update --app). Don't edit K2's source to force it.
  validate is clean, but nothing changed:
    run k2 zen reload, and check that you edited the Garden on screen
    (k2 zen garden list).
  The window went to safe mode: k2 zen guide safe-mode.
EOF
}

_zen_guide_page_safe_mode() {
    cat <<'EOF'
k2 zen guide safe-mode — what each line means, and the ways out

Safe mode draws the built-in texting page with the template's Garden
switcher and Zen toggle, and ignores this Garden's file. Your files are
untouched. The banner says "Zen is in safe mode. Your files are
untouched.", then one of the lines below, with Try again and Exit Zen.

THE LINES
  You held Shift while turning Zen on.
      You asked for it. Turn Zen off and on again without Shift.
  The Zen toggle isn't on the page.
      Same for The Garden switcher. Place it: k2 zen guide required.
  The Zen toggle isn't declared by the page.
      The page answer doesn't list it. Run k2 zen doctor; if the
      Garden's file looks right, update K2 on this computer.
  The Zen toggle isn't visible.
      Hidden, faded, under 24x24 px, off screen, under the window
      buttons, or covered. Move it, or put it in a menu.
  The Garden switcher isn't wired.
      K2 couldn't confirm it works (its list didn't open within 1
      second). Try again; if it repeats, file a ticket.
  The page crashed: <message>
      Try again. If it repeats, validate the file, then reset it.
  Can't reach K2 on this computer.
      The daemon isn't running. Start K2, then Try again.
  K2 on this computer sent a Zen page this app can't read: <message>
      The app and K2 disagree. Update both.
  K2 on this computer is older than this app. Update it to use Gardens.
      Update K2 on this computer.
  When the control sits in a menu, the line is about the menu's button.

THE WAYS OUT (they never depend on the page)
  Ctrl+Cmd+Z on macOS, Ctrl+Alt+Z on Linux.
  View > Exit Zen Mode on macOS; Menu > Exit Zen Mode in K2's window
  buttons on Linux.
  Exit Zen in the safe-mode banner.
  Hold Shift while turning Zen on: safe mode, whatever the file says.
  Cmd+Option+1-9 (Super+Alt+1-9 on Linux) switch Gardens with no switcher.
  From a terminal: k2 zen reset --garden <id> restores the Garden's stub.

Safe mode lasts until Try again, a change to the files, or leaving Zen.
EOF
}

# `k2 zen guide api` (prd-zen-user-widgets-v2 UWA11) is generated from the
# verb catalog by contract-gen. Nobody edits the fenced block by hand.
# >>> generated by contract-gen: zen guide api (do not edit; run `cargo run -p k2-core --bin contract-gen`)
_zen_guide_page_api() {
    cat <<'EOF'
k2 zen guide api — the k2 object in a custom widget

A custom widget is a sealed page in a Garden: no network, no storage, no
files. It reaches K2 only through window.k2. Each call needs a cap its
manifest.json asks for; a widget in your own Garden gets every one it asks
for at once (no review). From K2's verb catalog, version 55.

TEN EXAMPLES
  const off = k2.agents.subscribe((rows) => draw(rows))   // live, in scope
  el.textContent = row.label                              // never innerHTML
  await k2.conversation.open(row.address)  // the Conversation beside follows
  k2.thread.subscribe(row.address, (v) => drawPage(v.items, v.turn))
  const page = await k2.thread.read(row.address, {beforeSeq: first.seq})
  await k2.thread.post(row.address, 'How is it going?')
  await k2.connected; if (k2.can('thread.post')) showSendButton()
  k2.theme.changed(({theme}) => paint(theme.vars))
  img.src = k2.asset('cat.png')                   // a file in the folder
  k2.ready()                                     // once, after first draw

EVERY HELPER, BY CAP (portable: the same call works in an App later;
local: Garden only). Each line: helper, reach, feature, example, [errors]
no cap
  k2.gardens.list()                                                                                                local    zen-gardens-v1         const gardens = await k2.gardens.list()
  k2.gardens.current()                                                                                             local    zen-gardens-v1         const here = await k2.gardens.current()
  k2.gardens.switch(id)                                                                                            local    zen-gardens-v1         await k2.gardens.switch(gardens[1].id)  [rate_limited]
  k2.theme.get()                                                                                                   local    zen-v1                 const {theme, motion} = await k2.theme.get()
  k2.theme.changed(cb)                                                                                             local    zen-widgets-v1         k2.theme.changed(({theme}) => paint(theme))
  k2.canvas.setHitRegions(rects)                                                                                   local    zen-widgets-v1         await k2.canvas.setHitRegions([bar.getBoundingClientRect()])  [too_large]
  k2.window.startDrag()                                                                                            local    zen-widgets-v1         grip.addEventListener('mousedown', () => k2.window.startDrag())
  k2.layout.get()                                                                                                  local    zen-widgets-v1         const {columns} = await k2.layout.get()  [rate_limited]
  k2.layout.set({collapsed, column})                                                                               local    zen-widgets-v1         await k2.layout.set({column: 2, collapsed: false})  [bad_args rate_limited]
  k2.layout.toggle(column)                                                                                         local    zen-widgets-v1         chat.addEventListener('click', () => k2.layout.toggle(2))  [bad_args rate_limited]
  k2.layout.subscribe(cb)                                                                                          local    zen-widgets-v1         k2.layout.subscribe(({columns}) => drawTabs(columns))  [rate_limited]
  k2.save.get(key)                                                                                                 local    garden-data-v1         const best = await k2.save.get('high-score')  [bad_key rate_limited]
  k2.save.set(key, value)                                                                                          local    garden-data-v1         await k2.save.set('high-score', 42)  [bad_key value_too_large space_full store_full rate_limited]
  k2.save.remove(key)                                                                                              local    garden-data-v1         await k2.save.remove('high-score')  [bad_key rate_limited]
  k2.save.keys({prefix})                                                                                           local    garden-data-v1         const keys = await k2.save.keys()  [rate_limited]
  k2.viewer.get()                                                                                                  local    garden-data-v1         const {name} = await k2.viewer()
  k2.app.rooms({app})                                                                                              portable guests-rooms-v1        const {ceiling, granted} = await k2.asApp().app.rooms()  [app_pass_required pass_required pass_invalid pass_wrong_link usage not_found stale_widget author_unknown]
  k2.app.self({app})                                                                                               portable app-self-v1            const me = await k2.asApp().app.self()  [pass_required pass_invalid pass_wrong_link not_exposed not_found usage]
  k2.app.usage({app, hours})                                                                                       portable app-self-v1            const {verbs, quotas} = await k2.asApp().app.usage({hours: 24})  [pass_required pass_invalid pass_wrong_link not_exposed not_found usage]
  k2.widget.self()                                                                                                 local    app-self-v1            const me = await k2.widget.self()  [stale_widget rate_limited]
  k2.app.resources()                                                                                               local    app-resources-v1       const {disks, top} = await k2.app.resources()  [stale_widget rate_limited]
agents:read (See agents)
  k2.agents.list()                                                                                                 portable zen-v1                 const rows = await k2.agents.list()
  k2.agents.subscribe(cb)                                                                                          portable zen-v1                 const off = k2.agents.subscribe((rows) => draw(rows))
  k2.conversation.open(address)                                                                                    local    zen-v1                 await k2.conversation.open(row.address)  [not_bound]
  k2.conversation.close()                                                                                          local    zen-v1                 await k2.conversation.close()
  k2.agents.show(addresses)                                                                                        local    zen-widgets-v1         await k2.agents.show(rows.filter(onBoard).map((r) => r.address))  [not_bound bad_args rate_limited]
  k2.agents.openTerminal(address)                                                                                  local    workbench-v1           await k2.agents.openTerminal(row.address)  [not_bound bad_args rate_limited]
presence:read (See who is looking)
  k2.presence.get(address)                                                                                         local    zen-v1                 const people = await k2.presence.get(row.address)  [not_bound]
  k2.presence.subscribe(address, cb)                                                                               local    zen-v1                 const off = k2.presence.subscribe(row.address, (people) => draw(people))  [not_bound]
thread:read (Read the Thread)
  k2.thread.read(address, {afterSeq, beforeSeq, limit})                                                            portable zen-v1                 const page = await k2.thread.read(row.address, {limit: 50})  [not_bound]
  k2.thread.subscribe(address, cb)                                                                                 portable zen-v1                 const off = k2.thread.subscribe(row.address, (view) => draw(view))  [not_bound rate_limited]
  k2.thread.inbox(cb)                                                                                              portable thread-stream-v1       const off = k2.thread.inbox((m) => bump(m.address, m))  [rate_limited]
  k2.thread.markRead(address, {seq})                                                                               local    thread-read-v1         await k2.thread.markRead(row.address, {seq: last.seq})  [not_bound]
  k2.thread.attachment(address, docId, n, {mode, px})                                                              local    thread-images-v1       const t = await k2.thread.attachment(row.address, item.id, 0, {mode: 'thumb'})  [not_bound too_large]
  k2.thread.open(address, {docId, line, n, rel})                                                                   local    thread-attachments-v1  await k2.thread.open(row.address, {docId: item.id, n: 0})  [not_bound]
  k2.thread.links(address, candidates)                                                                             local    thread-attachments-v1  const {links} = await k2.thread.links(row.address, ['src/app.ts:12'])  [not_bound]
  k2.thread.catchup({limit, threads})                                                                              portable inbox-catchup-v1       const {threads} = await k2.thread.catchup({threads: [{addr: 'sales', afterSeq: 41}]})  [pass_required pass_invalid pass_wrong_link skin_room usage not_found not_bound rate_limited]
  k2.thread.unread()                                                                                               portable app-thread-parity-v1   const {total, byConversation} = await k2.thread.unread()  [pass_required pass_invalid pass_wrong_link skin_room usage not_found not_bound]
  k2.thread.mute({addr, muted, until})                                                                             portable app-read-state-v1      await k2.thread.mute({addr: 'sales', muted: true})  [pass_required pass_invalid pass_wrong_link skin_room not_bound usage not_found paused]
thread:post (Post to the Thread)
  k2.thread.pickFiles(address)                                                                                     local    thread-attachments-v1  const {files} = await k2.thread.pickFiles(row.address)  [not_bound]
  k2.thread.post(address, text, {attachments})                                                                     portable zen-v1                 await k2.thread.post(row.address, 'How is it going?')  [not_bound sending_off rate_limited too_large usage]
  k2.thread.answer(address, cardId, choice?)                                                                       portable zen-v1                 await k2.thread.answer(row.address, card.id, 'Yes')  [not_bound sending_off rate_limited card_person_only]
  k2.compose.draft(address, text)                                                                                  local    zen-v1                 await k2.compose.draft(row.address, 'Ask for a status')  [not_bound]
  k2.conversation.setContext(text)                                                                                 local    thread-context-v1      await k2.conversation.setContext('Board: ' + board.name)  [needs_editor not_bound bad_args rate_limited]
sources:read (Read data sources)
  k2.sources.list()                                                                                                portable sources-v1             const rows = await k2.sources.list()
  k2.sources.read(address)                                                                                         portable sources-v1             const v = await k2.sources.read(k2.config.source)  [not_bound server_not_saved unreachable removed]
  k2.sources.history(address, field, {fromAt, toAt})                                                               portable sources-v1             const h = await k2.sources.history(k2.config.source, 'count')  [not_bound server_not_saved unreachable removed no_history]
  k2.sources.subscribe(address, cb)                                                                                portable sources-v1             const off = k2.sources.subscribe(k2.config.source, (v) => draw(v))  [not_bound server_not_saved unreachable removed rate_limited]
  k2.sources.refresh(address)                                                                                      portable sources-v1             await k2.sources.refresh(k2.config.source)  [not_bound server_not_saved unreachable removed rate_limited]
  k2.sources.ask(address, itemKey?)                                                                                local    sources-v1             await k2.sources.ask(k2.config.source, tile.key)  [not_bound retired]
links:open (Open links in your browser)
  k2.links.open(url)                                                                                               local    zen-widgets-v1         const {ok} = await k2.open('https://example.com/report')  [bad_args rate_limited]
downloads:save (Save files to your Downloads)
  k2.thread.download(address, docId, n)                                                                            local    zen-files-v1           const {saved, name} = await k2.thread.download(row.address, item.id, n)  +thread:read  [not_bound not_granted not_found attachment_secret bad_args rate_limited]
  k2.downloads.begin(name, {mime})                                                                                 local    zen-files-v1           k2.download('Board.png', async () => await drawPng())  [bad_args rate_limited]
  k2.downloads.save(name, data, {mime, ticket})                                                                    local    zen-files-v1           const {saved, name} = await k2.download('board.json', JSON.stringify(doc))  [not_granted bad_args too_large rate_limited blob_missing]
uploads:pick (Take files you pick or drop)
  k2.uploads.pick({accept, max, multiple})                                                                         local    zen-files-v1           const {files} = await k2.pick({accept: ['image/*'], multiple: true})  [not_granted author_unknown stale_widget rate_limited bad_args too_large space_full store_full paused busy]
  k2.uploads.dropped(cb)                                                                                           local    zen-files-v1           k2.onDrop(({files, x, y}) => place(files, x, y))
files:read (Read files (also shows heartbeat instructions))
  k2.files.list({relPath, showHidden, workspace})                                                                  portable app-api-v1             const entries = await k2.files.list({workspace, relPath: '.'})  [pass_required pass_invalid pass_wrong_link skin_room usage not_bound author_unknown]
  k2.files.read({relPath, workspace})                                                                              portable app-api-v1             const {content} = await k2.files.read({workspace, relPath: 'notes.md'})  [pass_required pass_invalid pass_wrong_link skin_room usage not_bound author_unknown]
  k2.files.readBinary({relPath, workspace})                                                                        portable app-api-v1             const {base64} = await k2.files.readBinary({workspace, relPath})  [pass_required pass_invalid pass_wrong_link skin_room usage not_bound author_unknown]
tickets:read (Read tickets and get live ticket updates)
  k2.tickets.list({all, project, status})                                                                          portable app-api-v1             const {items} = await k2.tickets.list({project: 'sales'})  [pass_required pass_invalid pass_wrong_link skin_room usage not_bound author_unknown]
  k2.tickets.show({brief, id})                                                                                     portable app-api-v1             const t = await k2.tickets.show({id})  [pass_required pass_invalid pass_wrong_link skin_room usage not_bound author_unknown not_found]
  k2.tickets.attachment({commentId, id, len, mode, n, offset, px})                                                 portable ticket-attachments-v1  const t = await k2.tickets.attachment({id, commentId: c.id, n: 0, mode: 'thumb'})  [pass_required pass_invalid pass_wrong_link skin_room usage not_bound author_unknown not_found]
tickets:post (Open, answer, assign and set status on tickets)
  k2.tickets.create({agentName, assignees, attachments, body, briefHtml, kind, options, priority, project, title}) portable app-api-v1             await k2.tickets.create({project: 'sales', title: 'Quote for J1042?'})  [pass_required pass_invalid pass_wrong_link skin_room usage too_large not_bound author_unknown paused]
  k2.tickets.comment({attachments, body, id, optionPick})                                                          portable app-api-v1             await k2.tickets.comment({id, body: 'Looks right'})  [pass_required pass_invalid pass_wrong_link skin_room usage not_bound author_unknown not_found paused]
  k2.tickets.resolve({id, status})                                                                                 portable app-api-v1             await k2.tickets.resolve({id})  [pass_required pass_invalid pass_wrong_link skin_room usage not_bound author_unknown not_found paused]
store:read (Read the workspace store)
  k2.store.get(key, {from})                                                                                        local    garden-data-v1         const e = await k2.store.get('obj/7')  [not_bound not_granted author_unknown stale_widget rate_limited bad_key]
  k2.store.list({after, from, limit, prefix})                                                                      local    garden-data-v1         const {items, next} = await k2.store.list({prefix: 'obj/'})  [not_bound not_granted author_unknown stale_widget rate_limited bad_key]
  k2.store.changes({after, from, prefix, since})                                                                   local    garden-data-v1         const page = await k2.store.changes({since: 41})  [not_bound not_granted author_unknown stale_widget rate_limited resync bad_key]
  k2.store.history({after, before, from, key, limit, prefix})                                                      local    garden-history-v1      const {items, next} = await k2.store.history({prefix: 'board/b1/', limit: 200})  [not_bound not_granted author_unknown stale_widget rate_limited bad_key bad_args]
  k2.blobs.get(ref, {from, px})                                                                                    local    garden-blobs-v1        img.src = await k2.blobs.url(obj.src, {px: 256})  [not_granted author_unknown stale_widget rate_limited bad_args not_bound blob_missing]
  k2.blobs.info(ref)                                                                                               local    garden-blobs-v1        const {size, refs} = await k2.blobs.info(ref)  [not_granted author_unknown stale_widget rate_limited blob_missing]
  k2.blobs.list({after, limit})                                                                                    local    garden-blobs-v1        const {items, bytes, quota} = await k2.blobs.list()  [not_granted author_unknown stale_widget rate_limited]
  k2.store.grantees()                                                                                              local    garden-data-v1         const {items} = await k2.store.grantees()  [not_granted author_unknown stale_widget rate_limited]
store:write (Change dump-table rows)
  k2.store.put(key, value, {ifRev})                                                                                local    garden-data-v1         const {rev} = await k2.store.put('obj/7', note, {ifRev: e.rev})  [not_granted author_unknown stale_widget rate_limited bad_key conflict value_too_large space_full store_full paused]
  k2.store.patch(key, partial, {ifRev})                                                                            local    garden-data-v1         const {rev} = await k2.store.patch('obj/7', {done: true}, {ifRev: e.rev})  [not_granted author_unknown stale_widget rate_limited bad_key conflict value_too_large space_full store_full paused]
  k2.store.delete(key, {ifRev})                                                                                    local    garden-data-v1         await k2.store.delete('obj/7')  [not_granted author_unknown stale_widget rate_limited bad_key conflict paused]
  k2.store.batch(ops)                                                                                              local    garden-data-v1         await k2.store.batch([{op: 'put', key: 'obj/1', value: a}, {op: 'patch', key: 'obj/3', value: {x: 4}}, {op: 'delete', key: 'obj/2'}])  [not_granted author_unknown stale_widget rate_limited bad_key conflict value_too_large too_large space_full store_full paused]
  k2.store.txBegin()                                                                                               local    garden-data-v1         const tx = k2.store.tx()  [not_granted author_unknown stale_widget rate_limited busy]
  k2.store.txAdd(txId, ops)                                                                                        local    garden-data-v1         await tx.add(ops)  [not_granted author_unknown stale_widget rate_limited bad_tx tx_expired bad_key value_too_large too_large]
  k2.store.txCommit(txId)                                                                                          local    garden-data-v1         const {rev} = await tx.commit()  [not_granted author_unknown stale_widget rate_limited bad_tx tx_expired conflict value_too_large space_full store_full paused]
  k2.store.txAbort(txId)                                                                                           local    garden-data-v1         await tx.abort()
  k2.store.restore({at, dryRun, except, prefixes, snapshot, toRev})                                                local    garden-history-v1      await k2.store.restore({at: yesterday, prefixes: ['board/b1/'], except: ['board/b1/versions/']})  [not_granted author_unknown stale_widget rate_limited bad_args bad_key no_history blob_missing space_full store_full paused]
  k2.store.snapshot({except, label, prefixes})                                                                     local    garden-history-v1      const {ref, rev} = await k2.store.snapshot({prefixes: ['board/b1/'], except: ['board/b1/versions/'], label})  [not_granted author_unknown stale_widget rate_limited bad_args bad_key too_large space_full store_full paused]
  k2.blobs.put(data, {mime, name})                                                                                 local    garden-blobs-v1        const {ref} = await k2.blobs.put(file); await k2.store.patch('obj/7', {src: ref})  [not_granted author_unknown stale_widget rate_limited bad_args too_large space_full store_full paused busy]
guests:rooms (Set its own guests' rooms (an owner grant per App, never a guest's))
  k2.guests.rooms({app, username})                                                                                 portable guests-rooms-v1        const g = await k2.asApp().guests.rooms({username: 'dana'})  [app_pass_required pass_required pass_invalid pass_wrong_link grant_required usage not_found rate_limited stale_widget author_unknown]
  k2.guests.setRooms({app, rooms, username})                                                                       portable guests-rooms-v1        await k2.asApp().guests.setRooms({username: 'dana', rooms: ['acme']})  [app_pass_required pass_required pass_invalid pass_wrong_link grant_required usage not_found outside_app_rooms above_role has_role rate_limited stale_widget author_unknown confirm_required declined paused]
  k2.guests.list({app, cursor, limit})                                                                             portable guests-rooms-v1        const {guests, nextCursor} = await k2.asApp().guests.list({limit: 200})  [app_pass_required pass_required pass_invalid pass_wrong_link grant_required usage not_found rate_limited stale_widget author_unknown]
activity:read (See when the agent is working)
  k2.thread.onActivity(address, cb)                                                                                portable widget-activity-v1     const off = k2.thread.onActivity(row.address, ({turn}) => turn ? drawWorking(turn) : hideWorking())  +thread:read  [not_bound rate_limited]
  k2.activity.snapshot({workspace})                                                                                portable app-api-v1             const {status} = await k2.activity.snapshot({workspace})  [pass_required pass_invalid pass_wrong_link skin_room usage not_bound author_unknown]
room:members (See who has access to the room)
  k2.room.members({app, workspace})                                                                                portable room-members-v1        const {members} = await k2.room.members({workspace})  [pass_required pass_invalid pass_wrong_link skin_room not_bound not_found usage]
git:read (See the Workbench's git)
  k2.work.list()                                                                                                   local    workbench-v1           const tasks = await k2.work.list()  [author_unknown needs_editor stale_widget rate_limited]
  k2.git.snapshot({address, base})                                                                                 local    workbench-v1           const s = await k2.git.snapshot({address: row.address})  [not_bound not_a_repo author_unknown needs_editor stale_widget rate_limited]
  k2.git.diffFile({address, base, file, oldFile})                                                                  local    workbench-v1           const d = await k2.git.diffFile({address: row.address, file: 'src/app.ts', base: 'main'})  [not_bound not_a_repo author_unknown needs_editor stale_widget rate_limited not_found]
  k2.git.log({address, base, limit})                                                                               local    workbench-v1           const {commits} = await k2.git.log({address: row.address})  [not_bound not_a_repo author_unknown needs_editor stale_widget rate_limited]
git:notes (Ask an agent for changes)
  k2.git.note({address, notes, send})                                                                              local    workbench-v1           await k2.git.note({address: row.address, notes: [{file: 'src/a.ts', line: 12, body: 'Keep the old cookie'}]})  [not_bound bad_args author_unknown needs_editor stale_widget rate_limited paused declined]
compute:read (See test runs on compute)
  k2.compute.jobs({address, limit})                                                                                local    workbench-v1           const jobs = await k2.compute.jobs({address: row.address})  [not_bound not_a_repo author_unknown needs_editor stale_widget rate_limited]
compute:nodes (See compute node load)
  k2.compute.nodes()                                                                                               local    compute-nodes-v1       const {nodes} = await k2.compute.nodes()  [needs_editor stale_widget rate_limited]
agents:start (Start, land and discard worktree agents)
  k2.agents.start({base, brief, harness, name, workspace})                                                         local    workbench-v1           const t = await k2.agents.start({workspace: 'k2::local', name: 'fix-login', brief})  [not_bound bad_args author_unknown needs_editor stale_widget rate_limited paused declined confirm_required]
  k2.git.land({address})                                                                                           local    workbench-v1           const r = await k2.git.land({address: row.address})  +git:read  [not_bound bad_args author_unknown needs_editor stale_widget rate_limited paused declined conflict confirm_required]
  k2.agents.settle({address, discard})                                                                             local    workbench-v1           await k2.agents.settle({address: row.address, discard: true})  +git:read  [not_bound bad_args author_unknown needs_editor stale_widget rate_limited paused declined conflict confirm_required]
  k2.agents.end({address})                                                                                         local    workbench-v1           const {ended} = await k2.agents.end({address: row.address})  [not_bound bad_args author_unknown needs_editor stale_widget rate_limited paused declined sidecar_attached confirm_required]
  k2.agents.archive({address})                                                                                     local    workbench-v1           await k2.agents.archive({address: row.address})  [not_bound bad_args author_unknown needs_editor stale_widget rate_limited paused sidecar_live worktree_sidecar]
agents:stop (Stop an agent's turn)
  k2.agents.stop({address})                                                                                        local    workbench-v1           await k2.agents.stop({address: row.address})  [not_bound bad_args author_unknown needs_editor stale_widget rate_limited paused declined]

Every call may also fail with cap_not_granted, verb_unavailable, failed.

ALSO ON k2
  k2.call(verb, ...args), k2.subscribe(verb, ...args, cb) (alias k2.on).
  await k2.connected first: until then k2.can is false and k2.config,
  k2.widget, k2.motion are empty (calls wait on their own). k2.can(verb)
  is advice; K2 checks every call. k2.ready() once drawn. k2.asset(name).
  A refused call rejects with K2Error (.code .message .verb .cap .room
  .feature .last). Pass onError after cb to hear a refused subscription.

DATA (this computer): k2.store.subscribe(prefix, cb) = the state, then
  each rev from any writer; k2.store.tx() add/commit = up to 1,000 ops as
  one rev; putChunked/getChunked to 1 MiB; k2.viewer(). sources:read drops
  store:write. More: k2 zen guide services.

WHOLE CANVAS ([layout] canvas = "full"): K2's top band (about 52 px)
  drags the window. k2.canvas.setHitRegions(rects) cuts holes in it (CSS
  px rects of your viewport, el.getBoundingClientRect(); [] clears; 32 at
  most; K2's controls win). k2.window.startDrag() in a mousedown drags it.

COLUMNS AND LISTS: k2.layout.set/toggle fold a collapsible column only
  within 5 s of a click in your widget (kept per Garden). An Agents widget
  with list-from = "<your id>" draws what k2.agents.show([...]) lists.

RULES: textContent for anything an agent wrote, never innerHTML; no
  onclick= (addEventListener in a script file). Check with k2 zen validate
  --widget <name>. Posting pauses after 120 posts (or 20 identical ones to
  one agent) in 10 minutes, until the person clicks Resume (sending_off).

Types: sdk/generated/k2.d.ts. Caps in K2's words: the k2-zen skill.
EOF
}
# <<< generated

# `k2 zen guide libs` (0.46.5, prd-widget-size-and-shared-libs-v1 L9) is
# generated from src/shared/zen-lib.json by contract-gen. Nobody edits the
# fenced block by hand.
# >>> generated by contract-gen: zen guide libs (do not edit; run `cargo run -p k2-core --bin contract-gen`)
_zen_guide_page_libs() {
    cat <<'EOF'
k2 zen guide libs — K2's shared libraries for custom widgets

K2 ships these with the app. A widget names them in manifest.json and never
copies them: K2 inlines each one as a nonced classic script after k2.js and
before your own scripts, so your code calls its global (uPlot, Chart, ...).
They don't count toward the code limit (1 MB; a warning above 512 KB) or the
page limit (3.5 MB); a widget may load up to 8 MB of them. Every library
runs inside the widget's sealed frame, with the widget's caps and no more.

ASK FOR A LINE, NOT A PATCH
  "requires": { "libs": ["uplot@1", "chart.js@4"] }
  A line is the major (uplot@1), or 0.minor below 1.0 (three@0.186). K2
  updates patches inside a line, so the widget keeps working.
  One file from a CDN: {"url": "https://cdn.jsdelivr.net/npm/x@1.2.3/x.min.js",
  "integrity": "sha384-..."}, an exact version, from one of
  cdnjs.cloudflare.com, cdn.jsdelivr.net, unpkg.com.
  The list is part of the version you sign, like your code.

WHICH TO PICK
  lines, sparklines   uplot@1          52 KB
  bar, pie, doughnut  chart.js@4       204 KB
  big dashboards      echarts@6        1.1 MB
  your own charts     d3@7             274 KB
  markdown (safe)     k2-markdown@1    7 KB
  code highlighting   highlight.js@11  129 KB
  diffs               k2-diff@1        23 KB
  maths               katex@0.18       630 KB
  3D                  three@0.186      959 KB
  2D games            kaplay@3001      185 KB
  UI components       preact@10        17 KB
  a help button       k2-help@1        14 KB
  a Thread panel      k2-thread@1      54 KB
  PNG from a canvas   k2-png@1         4 KB
  dates, numbers      no library: the browser's Intl.DateTimeFormat,
                      Intl.RelativeTimeFormat and Intl.NumberFormat

Never rely on canvas.toDataURL or canvas.toBlob in a Garden: on WebKit (the
Mac app, iPhone, iPad) they come back empty in the sealed frame. Read pixels
with getImageData, or require k2-png@1 (K2Png.toBlob(canvas)).

EVERY LIBRARY: request, global, licence, size
  three@0.186           THREE            MIT                  959 KB
  globe.gl@2            Globe            MIT                  1.8 MB
  cannon-es@0.20        CANNON           MIT                  121 KB
  pixi.js@8             PIXI             MIT                  824 KB
  phaser@4              Phaser           MIT                  1.3 MB
  kaplay@3001           kaplay           MIT                  185 KB
  matter-js@0.20        Matter           MIT                   82 KB
  d3@7                  d3               ISC                  274 KB
  chart.js@4            Chart            MIT                  204 KB
  echarts@6             echarts          Apache-2.0           1.1 MB
  uplot@1               uPlot            MIT                   52 KB
  animejs@4             anime            MIT                  115 KB
  lottie-web@5          lottie           MIT                  299 KB
  perfect-freehand@1    PerfectFreehand  MIT                    5 KB
  roughjs@4             rough            MIT                   27 KB
  howler@2              Howler           MIT                   35 KB
  tone@15               Tone             MIT                  337 KB
  leaflet@1             L                BSD-2-Clause         171 KB
  marked@18             marked           MIT                   46 KB
  dompurify@3           DOMPurify        Apache-2.0            29 KB
  highlight.js@11       hljs             BSD-3-Clause         129 KB
  katex@0.18            katex            MIT                  630 KB
  preact@10             preact           MIT AND Apache-2.0    17 KB
  lit@3                 Lit              BSD-3-Clause          23 KB
  font-caveat@5         K2Fonts          OFL-1.1              182 KB
  babylonjs@8           BABYLON          Apache-2.0           7.5 MB  download
  plotly.js-dist-min@4  Plotly           MIT                  4.6 MB  download
  k2-help@1             K2Help           MIT                   14 KB
  k2-activity@1         K2Activity       MIT                   15 KB
  k2-thread@1           K2Thread         MIT                   54 KB
  k2-markdown@1         K2Markdown       MIT                    7 KB
  k2-png@1              K2Png            MIT                    4 KB
  k2-diff@1             K2Diff           MIT                   23 KB

"download": fetched once by this computer's K2 on first use, checked, then
kept (refused when the computer is air-gapped). Notes on each library are in
the k2-zen skill. Check a widget with k2 zen validate --widget <name>: it
prints the size line and says when a file looks like a copied library.
EOF
}
# <<< generated

