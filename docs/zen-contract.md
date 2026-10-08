# Zen Gardens: what the renderer reads from the daemon

This page records exactly what the daemon serves for Zen and what the
renderer sends, so the two halves can be built in parallel and checked
against each other. Source of truth: `.k2/prds/prd-zen-gardens-v1.md`
(G1–G39, Rosson's answers of 2026-10-04) with its vs-live amendments
G40–G66, over `.k2/prds/prd-zen-mode-v1.md` (Z9–Z16, Z58–Z62). Daemon side:
`crates/k2-daemon/src/zen_routes.rs` and `crates/k2-core/src/zen/`. Renderer
side: `src/renderer/lib/zen/`.

Every Zen request goes to **this computer's** daemon through
`scopeForHost('local')` with its owner token. None goes through
`primaryScope()`. Any other token (a Connect login of any role, an app pass,
an agent passport) gets 403 `{error: "zen_local_only"}`. Bodies over 64 KB
get 413. A GET on a POST route is 405.

Feature key: `/boot-status` `features` carries **`zen-gardens-v1`** (G19).
The renderer requires it from the local scope; without it, Zen shows safe
mode with "K2 on this computer is older than this app. Update it to use
Gardens." `zen-v1` stays and means the theme routes exist.

**`zen-chrome-v1`** (prd-zen-freeform-chrome FC29) means the page answer
carries `page.chrome`, `page.bands`, `page.edges` and `page.menus`, and a
Garden file may place K2's controls (see "Zen controls" below). A renderer
talking to a daemon without it builds the template's chrome itself (FC32).

## The model

- **Zen is a window mode.** On/off lives in the renderer per window
  (`k2.zen.window.v1.<label>`, with the Garden and the nav rail's view).
  The daemon knows nothing about windows.
- **A Garden** is a personal page on this computer. The list lives in
  `~/.k2/zen/gardens.json` (daemon-written: ids, names, order, templates);
  each Garden's page is `~/.k2/zen/gardens/<id>.toml`.
- **Ids** are `g-` + 8 lowercase hex (`g-3f9a12c0`). They never change:
  rename and reorder keep them. Any request field or query named `garden`
  takes an **id or a name** (a name matches case-insensitively and exactly;
  an id wins).
- **Index** is **1-based** everywhere (the first Garden is `index: 1`;
  ⌘⌥1 is Garden 1).
- **Templates:** `k2.texting@1` (Garden 1: Agents beside Conversation)
  and `k2.blank@1` (Garden 2 and every new Garden: the empty-Garden widget).
- **First Gardens** (Rosson, 2026-10-04): `setup` on an empty list makes
  **Garden 1** (`k2.texting@1`) and **Garden 2** (`k2.blank@1`, nothing in
  it but the empty-Garden widget: ask your agents to build it). Only an
  empty list seeds, so running `setup` again makes nothing, and a deleted
  Garden 2 never comes back.
- **No migration.** Per-Home Zen never shipped (Rosson, 2026-10-04). A folder
  left from that era (`pages/`, `homes.json`) is ignored, and it counts as not
  set up until `setup` adds Garden 1 and Garden 2.

## Requests

| Call | When | Body or query | Answer |
|---|---|---|---|
| `GET /cli/zen/gardens` | Zen turns on in a window; every local `zen_changed` | none | `{ok, setUp, gardens: [Garden]}`. **Always 200**: `setUp: false, gardens: []` when Zen isn't set up, so "not set up" differs from "down" |
| `POST /cli/zen/setup` | Turning Zen on when `gardens` said `setUp: false` (once) | `{}` | `{ok, path, createdFolder, createdDefault, migrated: null, gardens: [Garden], changed}`. `createdDefault`: this call made Garden 1 and Garden 2. Idempotent. The only route that creates `~/.k2/zen` |
| `GET /cli/zen/get` | Showing a Garden; Try again; every `zen_changed` | `?garden=<id|name>` (omit for the first Garden) | the page answer below |
| `POST /cli/zen/garden/new` | **+ New Garden** in the switcher | `{name, template?: "blank"\|"texting", seedHome?, at?}` | `{ok, garden: Garden, file, path, changed}` |
| `POST /cli/zen/garden/rename` | CLI and agents (Q6) | `{garden, name}` | `{ok, garden: Garden, changed}`; the same name again is `changed: false` and emits nothing |
| `POST /cli/zen/garden/reorder` | CLI and agents | `{garden, to}` (`to` is 1-based) | `{ok, gardens: [Garden], changed}` |
| `POST /cli/zen/garden/template` | **Start with the default** on an empty Garden; `k2 zen garden template` | `{garden, template: "texting"\|"blank", force?}` | `{ok, garden: Garden, template, file, path, snapshot, replaced: [key], changed}`. Turns the Garden into that template's stub with the **same id, name and place**; the list entry names the template. The old file is kept in `.history/gardens/<id>.toml/` first (`snapshot`). A Garden already on the template with nothing of its own is `changed: false` (nothing written, no event). A file with its own top-level keys (layout, widgets, theme tables) is 409 `garden_has_changes` unless `force`; `replaced` lists what `force` replaced |
| `POST /cli/zen/garden/delete` | CLI and agents | `{garden}` | `{ok, deleted: <id>, name, snapshot, gardens: [Garden], changed}` |
| `POST /cli/zen/theme/set` · `next` · `prev` · `new`, `GET /cli/zen/theme/list` | theme picker and keys | see Themes | |
| `GET /cli/zen/status` | CLI | none | `{ok, setUp, path, gardens: [Garden], watching, message}` |
| `GET /cli/zen/validate`, `GET /cli/zen/history`, `POST /cli/zen/reset`, `POST /cli/zen/reload`, `GET /cli/zen/doctor` | CLI | `garden=` / `{garden}`, `file=`, `theme=` | for the CLI |

`Garden` (one entry of every list above):

```jsonc
{
  "id": "g-3f9a12c0",
  "name": "Launch room",            // 1–60 characters, unique without regard to case
  "index": 2,                       // 1-based position
  "template": "k2.blank@1",         // what it was made with; its file may name another
  "hasFile": true,                  // gardens/<id>.toml exists
  "createdAt": "2026-10-04T18:00:00Z",
  "theme": "paper",                 // its own theme pick, or null (follows the computer)
  "seedHome": "<home id>"           // only when garden/new was given one (G27)
}
```

### Errors

| Status | `error` | When |
|---|---|---|
| 404 | `zen_not_set_up` | any route but `gardens`, `status`, `doctor` and `setup` before setup. `message`: "Zen isn't set up on this computer. Turn it on with the Zen toggle in the K2 app's top bar." `garden/new` never sets Zen up |
| 404 | `unknown_garden` | `{garden, gardens: [ids], message}`: no Garden with that id or name |
| 409 | `garden_exists` | `garden/new` or `garden/rename` onto a name in use (case aside). `message`: "You already have a Garden called “Notes”." |
| 409 | `last_garden` | `garden/delete` of the only Garden. `message`: "That's your last Garden." |
| 409 | `garden_has_changes` | `garden/template` without `force` onto a Garden whose file sets its own layout, widgets or theme tables (or doesn't parse). `{garden, keys: [top-level key], message}`. Nothing is written |
| 400 | `bad_request` | a name that is empty or over 60 characters, `at`/`to` outside the list, an unknown `template`, an unknown body field (bodies are strict), or the old `home` key/query |
| 404 / 409 | `unknown_theme` / `theme_exists` | as before (Themes) |

**Removed** (G13): `POST /cli/zen/page/ensure` and `POST /cli/zen/homes/sync`.
They have no policy rows. A POST to either is refused by the top-level
method guard (405, like any path that isn't a POST route); a GET is 404
`{error: "unknown zen route"}`. A `home=` query or `home` body field on any
Zen route is 400, never silently ignored.

**Delete** moves the page file into `.history/gardens/<id>.toml/` (the
`snapshot` name) with a `deleted.json` record of the list entry, and drops
the Garden's theme pick. Nothing is removed without a copy in history.
`GET /cli/zen/history?garden=<deleted id>` still lists it (`deleted: true`).
A restore verb is later (Q8).

## `GET /cli/zen/get` answer

```jsonc
{
  "ok": true,                      // not read; a page present is what counts
  "schema": 1,                     // any other value: "this K2 reads Zen schema 1" (safe mode)
  "version": "b3f…",               // changes whenever anything below changes (Garden name included)
  "garden": { "id": "g-3f9a12c0", "name": "Launch room", "index": 2 },   // replaces v1's `home`
  "page": {
    "template": "k2.texting@1",
    "layout": {
      "kind": "columns",
      "split": [34, 66],           // percent per column; adds up to 100
      "minWidths": [240, 360],     // px per column
      "columns": [{ "size": 34, "min-width": 240, "widget": "agents" }, …]   // `widget`: first widget in it
    },
    "widgets": [ Widget, … ],      // CONTENT widgets only: never a chrome kind (FC28)
    "controls": [                  // the template's list as is while the chrome is the template's
      { "kind": "garden-switcher", "placement": "top-left" },
      { "kind": "drag-region", "placement": "top" },
      { "kind": "zen-toggle", "placement": "top-right" },
      { "kind": "add-agent", "placement": "widget-bottom-left", "widget": "agents" }   // texting only
    ],
    "chrome": {                    // K2's controls (FC29); zen-chrome-v1
      "from": "template",          // "template" | "garden" (the Garden file placed its own)
      "items": [ ChromeItem, … ]
    },
    "bands": {                     // ids (content and chrome) in draw order; null = no such band
      "top":    { "start": ["garden-switcher"], "center": [], "end": ["usage", "theme-picker", "zen-toggle"] },
      "bottom": null
    },
    "edges": [                     // column edges that hold something, by column, top before bottom
      { "column": 1, "edge": "bottom", "start": [], "center": [], "end": ["zen-toggle"] }
    ],
    "menus": { "more": ["garden-switcher", "theme-picker", "zen-toggle"] }   // menu id → item ids, file order
  },
  "theme": { … }, "themes": [ … ], "chrome": { … }, "motion": { … },     // see Themes
  "errors":   [{ "file": "gardens/g-3f9a12c0.toml", "line": 7, "col": 1, "message": "…" }],
  "warnings": [ … same shape … ],
  "lastGoodAt": "2026-10-04T18:00:00Z",
  "sources": { "zen.toml": "file", "gardens/g-3f9a12c0.toml": "snapshot:…", "themes/basic/theme.toml": "default" }
}
```

**The daemon honours the page's template** (G40). The page is the Garden
file's `template` (else the template the Garden was made with), then the
file's `[layout]` and `[[widget]]` in place of the template's (G38). A file
with errors keeps its last good version: `get` answers **200** with the last
good `page` plus `errors`. A non-2xx status, or a body with no `page`, puts
the window into safe mode.

### Templates

| | `k2.texting@1` (Garden 1) | `k2.blank@1` (Garden 2, new Gardens) |
|---|---|---|
| layout | 2 columns, `[34, 66]`, min `[240, 360]` | 1 column, `[100]`, min `[320]` |
| widgets | `agents` (column 0, `home-picker: true`), `conversation` (column 1, `agents: "agents"`), `nav-rail` (id `nav`, column 0) | `garden-empty` (column 0; caps `agents:read`, `thread:read`, `thread:post`, `gardens:template`) |
| controls | `garden-switcher`, `drag-region`, `zen-toggle`, `add-agent` | `garden-switcher`, `drag-region`, `zen-toggle` |
| chrome (FC30) | top band: `garden-switcher` (start); `usage`, `theme-picker`, `zen-toggle` (end) | the same |

**Required controls** (G24; Rosson, 2026-10-04: exactly two):
`zen-toggle`, `garden-switcher`. Every template places both as chrome and
declares both in `controls`, plus `drag-region`, which K2 binds for window
drag but never checks. A Garden file that places any chrome must place
both (FC24), so no page can drop one. Safe mode always draws the
template's chrome without usage and theme (FC22).
`home-switcher` and `home-option` are gone from Zen. The renderer binds `garden-option` (with its Garden id) the way it
bound `home-option`. Template controls get the caps `agents:add` and
`gardens:manage` (G29); widgets never get `gardens:manage`. The built-in
`garden-empty` widget alone gets `gardens:template` (Start with the
default on its own Garden).

### `Widget`

```jsonc
{
  "id": "agents",                  // unique on the page
  "kind": "agents",                // agents | conversation | nav-rail | garden-empty
  "slot": "column",                // "column" (default) | "top" | "bottom" (the bands); a string, more values later
  "column": 0,                     // absent for a band widget
  "edge": "bottom",                // only when the file set it: a row item at that column edge
  "align": "end",                  // only when the file set it: start | center | end
  "props": { … },                  // EVERY prop is present: the daemon fills K2's defaults
  "caps": ["agents:read", "agents:add", "presence:read"],   // K2's, by kind; a file can't name caps
  "source": "builtin"
}
```

Caps by kind: `agents` → `agents:read, agents:add, presence:read`;
`conversation` → `agents:read, presence:read, thread:read, thread:post`;
`garden-empty` → `agents:read, thread:read, thread:post`;
`nav-rail` → `app:navigate`.

Props (Rosson, answer 5: a widget shows **a whole Home** or **one agent
filtered from a Home**):

| kind | prop | type | default | meaning |
|---|---|---|---|---|
| `agents` | `mode` | `"home"` \| `"agent"` | `"agent"` when `agent` is set, else `"home"` (always sent) | whole Home, or one agent |
| `agents` | `home` | string | unset | the Home to show, by id **or name**; unset = the Garden's `seedHome`, else the window's selected Home (G27) |
| `agents` | `agent` | string | unset | in `mode: "agent"`, the one agent's name or address in that Home |
| `agents` | `home-picker` | bool | `false` (texting: `true`) | the widget's own Home picker; never moves the Home page |
| `agents` | `order` | `"home"` | `"home"` | row order |
| `agents` | `server-tag` | bool | `true` | tag rows from another server |
| `agents` | `preview` | bool | `true` | last message under each row |
| `agents` | `status` | list of `working`, `idle`, `needs-you` | all three | which live statuses show (`monitoring` shows with `working`, `unverifiable` with `idle`; a row's `activity` is one of `working`, `monitoring`, `needs-you`, `unverifiable`, `idle`, prd-daemon-activity-and-thread-working-v1 Q13) |
| `conversation` | `agents` | string (a widget id) | the page's first `agents` widget (column order) | follow that widget's picked agent; **always sent** unless `agent` is set |
| `conversation` | `agent` | string | unset | pin one agent's conversation (name or address), no list needed |
| `conversation` | `home` | string | unset | with `agent`: the Home to look it up in (id or name) |
| `conversation` | `compose` | bool | `true` | the compose box |
| `conversation` | `attachments` | bool | `true` | attachments in the box |
| `conversation` | `load-older` | bool | `true` | load older on scroll |

`nav-rail` has one prop, `orientation` (`"row"` | `"column"`, always sent):
`"row"` when the rail is in the top band (the only orientation allowed
there), else `"column"` unless the file sets `"row"` (Rosson, 2026-10-06).
In a column it is a thin (44px), icon-only rail drawn at the **left edge of
its column, outside the column's box**; it takes no share of the box (a
`"row"` rail in a column runs across the top of the column instead). With
`slot = "top"` it is a **row of four icons in the top band, immediately
right of the Garden switcher**, 36px high with 30px buttons; the glass pill
slides sideways, and the current view and Tickets badge work the same. Top to bottom: **My Home** (current: the Garden
itself), **Agents**, **Projects**, **Tickets** (with the top bar's waiting
badge), each with the top bar's page name as its tooltip. Each switches the
Garden's **view** in this window, inside Zen (`app.open`; Rosson,
2026-10-04): Zen stays on and the app page under Zen never changes. The view
shown is the rail's current item, and it is remembered per window in
`k2.zen.window.v1.<label>` (`view`: `home`, `agents`, `projects` or
`tickets`; missing means `home`). The views are drawn by the renderer from
the Garden's own page (`lib/zen/zen-rail-views.ts`); the template's
controls never change, so a view switch can't fail the required-controls
check:

- **My Home**: the Garden's page.
- **Agents**: the same page, but every `agents` widget lists this server's
  workspaces the way the app's Agents page does (pinned first; with focus
  groups on, the active group's and the ungrouped ones), with the app's
  focus-group dropdown in place of the Home picker when focus groups are
  on, and no Add agent row.
- **Projects**: "Coming soon — or build a new one yourself!", where the
  link is the empty Garden's Ask my agent.
- **Tickets**: the Tickets page's board (list + item, HTML brief) in a
  liquid glass look, chat only (no terminal).

Only `k2.texting@1` places it (column 0, `"column"`); a Garden file may
place it like any built-in widget, in a column or in the top band. A Garden
without a rail ignores the view.

`garden-empty` has no props. It shows "This Garden is empty." / "Ask your
agents to add things to this Garden." and **Ask my agent** (G28) until the
Garden's file declares `[[widget]]`.

### What a Garden file may declare (G38)

```toml
schema = 1
template = "k2.blank@1"            # or "k2.texting@1"
[layout]
kind = "columns"
[[layout.column]]
size = 40                          # percent; the sizes add up to 100
min-width = 240                    # px, 0–800 (default 0)
[[layout.column]]
size = 60
[[widget]]
id = "work"
kind = "agents"                    # agents | conversation | nav-rail (garden-empty only from the template)
column = 0
[widget.props]
home = "Work"
agent = "cortana"                  # one agent → mode "agent"
```

A widget may sit in the **top band** instead of a column (Rosson,
2026-10-06):

```toml
[[widget]]
kind = "nav-rail"
slot = "top"                       # "column" (default) | "top"
```

- `slot` is `"column"` (the default: needs `column = n`), `"top"` or
  `"bottom"` (the page's bands), or `"menu"` (a Zen control inside a menu;
  see below). It is a string so later slots are new values, not a new key;
  any other value is an error.
- Content kinds in the schema's one allowlist, `BAND_WIDGET_KINDS`
  (`nav-rail` today), and every chrome kind fit a band; any other kind in a
  band is an error naming both lists. At most 2 content widgets per band
  (`MAX_BAND_WIDGETS`) and 8 items in all (`MAX_BAND_ITEMS`).
- A band widget has no `column` (an error if set), and its `id` may be
  left out: it is then its kind (`"nav-rail"`).
- **One rail per page:** with a `nav-rail` in a band, any other `nav-rail`
  on the page (column or band) is an error at its line.
- A band widget is a content widget, so it is part of the file's content
  list, which replaces the template's: on the texting template, declare
  `agents` and `conversation` too. It does NOT replace the template's
  chrome: with the template's chrome, a band widget sits after the Garden
  switcher in `start` (or before the template's end group in `end`), so the
  toggle keeps its corner.

### Zen controls: chrome widgets (prd-zen-freeform-chrome, zen-chrome-v1)

K2's controls are widgets too. A Garden file places them with `[[widget]]`:

```toml
[[widget]]
id     = "x"            # optional outside a column's body (defaults to the kind)
kind   = "garden-switcher" | "zen-toggle" | "usage" | "theme-picker" | "menu"
slot   = "column" (default) | "top" | "bottom" | "menu"
column = 0              # slot = "column" only
edge   = "top" | "bottom"           # slot = "column" only (default "top")
align  = "start" | "center" | "end" # bands and edges (default "start")
menu   = "more"         # slot = "menu" only: the menu widget's id
[widget.props]          # menu only: icon = "dots" | "bars" | "zen", label = "…" (≤ 24)
```

- **Two groups, two replace rules (FC3).** Content (`agents`,
  `conversation`, `nav-rail`) and chrome are separate. Declaring any
  content widget replaces the template's content; declaring any chrome
  widget replaces **all** of the template's chrome (`chrome.from:
  "garden"`), and chrome you don't place is not shown. A file with only
  one group keeps the template's other group.
- **Required:** a file that places any chrome must place `zen-toggle` and
  `garden-switcher`, each exactly once, directly (a band or a column edge)
  or as a top-level item of a menu. Otherwise validate fails at the first
  chrome widget's line: "This file places Zen controls, so it replaces the
  template's. It must also place a zen-toggle (the way out): …".
- **Where:** a band item sits in `start`, `center` or `end` of its band.
  A chrome item with `slot = "column"` sits at that column's `edge`
  (a row; the column must exist). A `nav-rail` with an `edge` (or
  `orientation = "row"`) is a row at that edge. `agents` and
  `conversation` fill their column and take no `edge` or `align`.
- **Order** is file order inside each group; in `end` the **last** item
  sits in the corner. There is no `order` key.
- **Menus:** `kind = "menu"` sits in a band or at a column edge and holds
  the items whose `slot = "menu"` and `menu = "<its id>"`: only
  `zen-toggle`, `garden-switcher`, `theme-picker`, `usage`. No menu inside a
  menu, no content in a menu, 1–6 items each, at most 3 menus. `slot =
  "menu"` without `menu`, or a `menu` naming no menu, is an error; so is
  `menu` on any other slot.
- **Limits** (counted after each kind is known): 12 content widgets, 10
  chrome widgets, 8 items per band (2 content), 4 per column edge. Each
  chrome kind at most once, except `menu`.
- **Never placed:** `kind = "drag-region"` is an error ("K2 makes the empty
  space in every band drag the window; there is nothing to place.").
- **Caps:** a file never names them. `garden-switcher` gets
  `gardens:manage`; the others get none (FC31).
- **The user guide** (FC33–FC35): `k2 zen guide [topic] [--json]` teaches
  this grammar in static pages of at most 80 lines, with no daemon needed;
  `k2 zen guide example <name> --toml` prints whole Garden files to pipe
  into `gardens/<id>.toml`. `tests/cli/zen_guide.sh` validates every example
  against a headless daemon, and `zen_core.rs` (`fc_t17_*`, `fc_t18_*`)
  checks the examples and the pages against these tables, so a grammar
  change the guide doesn't follow fails a test.

`ChromeItem` (every key K2 fills):

```jsonc
{ "id": "zen-toggle", "kind": "zen-toggle", "slot": "bottom", "align": "end", "props": {}, "caps": [] }
{ "id": "zen-toggle", "kind": "zen-toggle", "slot": "column", "column": 1, "edge": "bottom", "align": "end", "props": {}, "caps": [] }
{ "id": "zen-toggle", "kind": "zen-toggle", "slot": "menu", "menu": "more", "props": {}, "caps": [] }
{ "id": "more", "kind": "menu", "slot": "top", "align": "end", "props": { "icon": "dots", "label": "More" }, "caps": [] }
```

`bands`, `edges` and `menus` list ids in draw order, computed by the
daemon, so the renderer only draws. With the template's chrome,
`controls` is the template's list as is. With the Garden's, `controls`
lists `garden-switcher` and `zen-toggle` with placements `"<band>-<align>"`
(`"bottom-start"`), `"column-<n>-<edge>-<align>"` (`"column-1-bottom-end"`)
or `"menu:<id>"`, then `drag-region` (`"bands"`), and `add-agent` when the
page has an Agents widget. Older apps read only each control's `kind` and
ignore `chrome`, `bands`, `edges` and `menus`, so they keep drawing their
own top band with both required controls (FC32). A file whose `widget`
key holds only chrome still counts as the file's own changes, so `k2 zen
garden template` needs `--force` (FC49).

1–3 columns, at most 12 content widgets. A `caps` key anywhere is an error;
`[[control]]` is a warning and ignored ("[[control]] is ignored: place
controls with [[widget]] kind = \"zen-toggle\" (see k2 zen guide bands)");
an unknown kind or prop is an error
at its line; a widget's `column` must exist in the effective layout; a
`conversation`'s `agents` must name an `agents` widget on the page; a
`conversation` with neither `agents` nor `agent` needs an `agents` widget to
follow. Theme tables restyle that Garden only.

## Themes (Omarchy additions, 2026-10-04)

`theme` changed shape for theme bundles. It is no longer
`{scheme, colors, type, shape}`. `chrome` and `motion` stay top level.

```jsonc
"theme": {
  "name": "basic",              // the active theme for this Garden (its id, lower case)
  "label": "Basic",             // what people see (Rosson, 2026-10-04): a built-in's label, else the id with a capital
  "builtin": true,              // shipped inside K2 (read-only)
  "user": false,                // ~/.k2/zen/themes/<name>/theme.toml exists (for a built-in: an override)
  "scope": "global",            // "garden" when the Garden has its own pick
  "tokens": {
    "scheme": "auto",           // auto | light | dark
    "colors": { "light": { "canvas": "#…", …, "idle": "#…", … }, "dark": { … } },  // `idle`, no `unread` (decision 9)
    "shape": { "radius": 14, "bubble-radius": 18, "gap": 12, "list-width": 300 }
  },
  "font": {                     // one font for the whole Zen page AND its terminals
    "family": "system", "stack": "-apple-system, …", "monospace": false,
    "size": 14, "lineHeight": 1.45,
    "terminal": { "family": "meslo", "stack": "\"MesloLGM Nerd Font\", …", "monospace": true }
  },
  "terminal": { "palette": { "light": { "foreground", "background", "cursor", "cursor-text",
                "selection", "black" … "bright-white" }, "dark": { … } } },
  "background": {               // only when the theme names an image
    "dataUrl": "data:image/png;base64,…", "mime": "image/png", "bytes": 12345,
    "file": "background.png", "fit": "cover", "opacity": 1, "lastGood": false
  }
},
"themes": [ { "name": "basic", "label": "Basic", "builtin": true, "user": false, "summary": "…", "active": true }, … ]
```

- **Font.** `font.stack` is the CSS `font-family` for the Zen root.
  Terminals in Zen use `font.terminal.stack`. That is the same family when
  `font.monospace` is true. A proportional family (system, rounded, serif)
  pairs with `meslo` in terminals, because a terminal grid needs
  fixed-width glyphs. Families are system stacks or fonts the app already
  bundles: meslo, jetbrains-mono, fira-code and lilex. Nothing loads from
  the network.
- **Background: served as a `data:` URL.** The app CSP allows
  `img-src 'self' asset: data: blob:` and has no `http://127.0.0.1`. A
  daemon image route would therefore be blocked, and it would also need the
  owner token in the URL. The locked frame CSP (`lib/frame-csp.ts`) allows
  `img-src data: blob:`, so v2 widgets in a frame can use the same URL.
  Images are capped:
  - types: `.png`, `.jpg`/`.jpeg`, `.webp` and `.gif`. No SVG.
  - size: 2 MB (2 097 152 bytes).
  - content: the bytes must match the extension. Links are refused.

  A bad image is reported in `errors` with the file and line of
  `image =`. The theme's last good image is then served with
  `lastGood: true`. The renderer should paint `dataUrl` under the Zen root
  with `fit` (`cover|contain|tile|center`) and `opacity`.
- **Layering.** For one Garden the stack is, lowest first:
  1. K2's built-in `basic` theme;
  2. K2's built-in copy of the active theme;
  3. `themes/<active>/theme.toml`;
  4. `zen.toml`;
  5. `gardens/<id>.toml`.

  The built-ins are `basic`, `paper` and `midnight` (Rosson, 2026-10-04:
  `basic` was called `default`; a saved pick of `default` in an existing
  `active.json` reads as `basic`, quietly, unless the person has a theme of
  their own called `default`). They are embedded in
  the daemon and read-only. `~/.k2/zen` holds only the user's changes, and
  the `zen.toml` stub is empty. Page templates are the same: the built-in
  `k2.texting@1` and `k2.blank@1` are the defaults, and a Garden file holds
  only its changes (theme tables, and optionally its layout of built-in
  widgets).
- **Errors.** A theme file with errors keeps its last good version, the
  same way as `zen.toml`. The errors come back in `errors` with
  `file: "themes/<name>/theme.toml"`. If a theme pick points to a deleted
  theme, `basic` is shown and a warning is added for `active.json`.

| Call | Body | Answer |
|---|---|---|
| `GET /cli/zen/theme/list` | `?garden=<id|name>` | `{active, scope, global, garden, gardenTheme, missing, themes}` |
| `POST /cli/zen/theme/set` | `{name}`, or `{name, garden}` for one Garden, or `{garden, clear: true}` to drop the Garden's pick | `{ok, theme, scope, garden, changed}` |
| `POST /cli/zen/theme/next` and `/prev` | `{}` or `{garden}` | same as set; cycles `themes` in order and wraps |
| `POST /cli/zen/theme/new` | `{name, from?}` | `{ok, name, file, path, from, copiedImage, changed}`; 409 `theme_exists` when the theme is already there |
| `POST /cli/zen/reset` | `{theme}` (as well as `file` and `garden`) | removes an override of a built-in theme (`restored: "builtin"`) |

An unknown name gets 404 `{error: "unknown_theme", theme, themes, message}`
and changes nothing. Each switch emits one `zen_changed`, so the renderer
re-`get`s. Cycle order: built-ins first in K2's order, then the user's
themes by name. The renderer calls next and prev (Flag 7).

## `theme`, `chrome`, `motion`: what the S5 engine reads

Source: `src/renderer/lib/zen/zen-theme-engine.ts`, `zen-chrome.ts`,
`zen-motion.ts`, `zen-tokens.ts` and `zen-page.ts`. The renderer reads the
daemon's `theme` shape from the section above. Only the names below are
read. Any other key is ignored, logged once, and never reaches CSS. A value
that is present but doesn't parse keeps the last good value for that key. A
key left out gets K2's default, which is the built-in `themes/basic.toml`;
`zen-theme-engine.test.ts` checks that the two match.

```jsonc
"theme": {
  "name": "basic", "builtin": true, "user": false,
  "scope": "global",                   // "garden": a switch carries the Garden (Flag 7)
  "tokens": {
    "scheme": "auto",                  // auto | light | dark
    "colors": { "light": { "canvas": "#faf7f2", … }, "dark": { … } },   // 16 tokens each
    "shape":  { "radius": 14, "bubble-radius": 18, "gap": 12, "list-width": 300 }
  },
  "font": {
    "family": "system",                // a FONT_FAMILIES name; the CSS stack comes from K2's own table
    "size": 14, "lineHeight": 1.45,
    "terminal": { "family": "meslo" }  // used when it is a fixed-width family K2 knows
  },                                   // `stack` and `monospace` are accepted and not read
  "terminal": { "palette": { "light": { "foreground": "#…", "cursor-text": "#…", "bright-black": "#…", … },
                             "dark":  { … } } },
  "background": { "dataUrl": "data:image/png;base64,…", "fit": "cover", "opacity": 1 }  // optional
},
"themes": [{ "name": "basic", "label": "Basic", "builtin": true, "user": false, "active": true }, …],   // the picker shows `label` (else the id with a capital) and sends `name`
"chrome": { "corners": "system", "stoplights": "round", "stoplight-offset": [0, 0] },
"motion": { "animations": { "<name>": { "on": true, "speed": 3, "bezier": [0.22, 1, 0.36, 1], "style": "popin 92%" }, … } }
```

- **Font.** The renderer writes the stack for `font.family` from
  `ZEN_FONT_TABLE`, which is a byte-for-byte copy of the daemon's
  `FONT_FAMILIES`. The daemon's `stack` text is never used as CSS.
  Terminals use `font.terminal.family` when it names a fixed-width family.
  Otherwise they pair a proportional family with `meslo`.
- **Terminal palette.** The renderer reads the active scheme's table. The
  daemon's TOML-style keys map to xterm `ITheme` names: `cursor-text`
  becomes `cursorAccent`, and `bright-black` becomes `brightBlack`. Unknown
  keys in either scheme are rejected.
- **Older shapes**, still read because they cost nothing:
  - tokens flat on `theme`;
  - a `tokens.type` table, `{family, size, line-height}`;
  - `font` as a bare name, or `MesloLGM Nerd Font` / `JetBrains Mono`;
  - a flat palette, xterm-style keys, or 16 ANSI colours as an array;
  - `background` as a bare data URL string.
- **`themes`.** The renderer reads `name`, `builtin` and `user`. If
  `theme.name` is missing, the entry with `active: true` names the active
  theme.

**Flag 5, background.** Only a base64 `data:image/(png|jpeg|webp|gif|avif)`
`dataUrl` is drawn. The app CSP allows `data:` and `blob:` images only, and
Zen loads nothing remote. The image is painted under the page, over the
root's canvas colour:
- `fit` is one of `cover`, `contain`, `tile` and `center`. Any other value
  becomes `cover`.
- `opacity` is 0–1, default 1. The canvas shows through the rest.
- `lastGood` is not read, because the daemon already reports the error.
- Reduced transparency drops the image.

**Flag 6, `motion`.** The renderer rebuilds each curve from the four
`bezier` numbers and the duration from `speed` (tenths of a second). The
`ease`, `durationMs` and `curve` strings are not read.

**Flag 7, theme switch routes.** All three calls go to the local daemon:
- The picker calls `POST /cli/zen/theme/set {name}`.
- ⌃⌘. (Ctrl+Alt+.) calls `POST /cli/zen/theme/next {}`.
- ⌃⌘⇧. (Ctrl+Alt+Shift+.) calls `POST /cli/zen/theme/prev {}`.

When `theme.scope` is `"garden"`, each body also carries `garden: <Garden id>`,
so the switch changes what this Garden shows and its own pick stays its own. The
daemon owns the order and the wrap. The renderer then re-reads `get`, and
`zen_changed` follows the switch as well.

## Event

`zen_changed` is an app-class kind with **no payload** (`{"kind":"zen_changed"}`):
the app bus reaches every Connect login on this daemon, and Garden names are
personal (G15). The renderer re-reads `GET /cli/zen/gardens` and
`GET /cli/zen/get` on each event (two local requests).

Exactly **one** event per effective change, whoever made it:
- `garden/new`, `garden/rename` (a real rename), `garden/reorder` (a real
  move), `garden/delete`;
- a save of `gardens/<id>.toml`, `zen.toml` or a theme file (the watcher,
  250 ms debounce; a burst is one event);
- a theme switch, `reset`, `setup` when it changes anything.

A no-op (the same name, the same position, a save with the same bytes, a
write to `grants.json`) emits nothing. `gardens.json` is not watched:
only the `garden/*` routes write it, and they announce their own change. The fingerprint
covers the list (ids, names, order, templates), every live layer and its
diagnostics.

The renderer holds its own app socket to the local daemon
(`subscribeToActiveState(scopeForHost('local'))`) while Zen is on screen,
whichever server the window is on.

## Custom widgets (zen-widgets-v1)

Source of truth: `.k2/prds/prd-zen-user-widgets-v2.md` (UW1–UW72, UWA1–UWA15,
UWB1–UWB30, Rosson's answers in §12–§14.5, the day-0 interfaces in §15).
A custom widget is the agent's own HTML, CSS and JS in
`~/.k2/zen/widgets/<name>/`, placed with `[[widget]] kind = "custom"`, and run
in a sealed frame. `/boot-status` `features` carries **`zen-widgets-v1`** when
this daemon serves the routes below. An older local daemon answers 404
`unknown zen route` on `widget/bundle`; the renderer then draws "K2 on this
computer is older than this app. Update it to run custom widgets." in the
widget's box (UW36, UW63).

### Verbs, caps and errors: the verb catalog

The verbs a widget may call, their caps, `portable`/`local`, the feature key
that brought each one, their argument and answer shapes, and the shared error
codes live in **one** place: `crates/k2-core/src/contract/catalog.json`
(UWA1). Nothing here copies them. Read them as:

- `k2 zen guide api`: every widget helper with its cap, reach, feature, a
  one-line example and its errors (no daemon needed);
- `sdk/generated/k2.d.ts`: the frame's `k2` object as TypeScript types;
- `src/renderer/lib/zen/zen-verbs.generated.ts` (`ZEN_VERBS`,
  `ZEN_CUSTOM_VERBS`) and `src/renderer/lib/k2-caps.generated.ts` (`K2_CAPS`
  with each cap's Settings label and plain sentence).

All of them are written by `cargo run -p k2-core --bin contract-gen` and never
edited by hand; `cargo test -p k2-core --lib contract::` and the vitest twin
`lib/contract/contract-generated.test.ts` fail on drift. A widget verb's
answer is the **guest projection** (UWA6): closed shapes, and no field named in
the catalog's `bannedFields` (`path`, `sessionId`, `conversation_id`,
`hostKey`, `token`, …). The catalog test checks every `http` binding against
`ROUTES` (`crates/k2-daemon/tests/contract_routes.rs`).

### Names

- A **user widget** is a folder name (the theme-name rule: lower-case letters,
  digits, `-`, `_`, up to 40).
- A **built-in widget** is `k2:<name>@<n>` (`k2:diary@1`), compiled into
  k2-core (`crates/k2-core/src/zen/builtin_widgets/`), never a folder, and
  immutable per version: better code ships as `@2`. A Garden file names the
  version; `k2 zen widget new my-diary --from k2:diary` copies the latest into
  a user folder to edit.
- A **catalog Garden** is a template `k2.<short>@<n>` from
  `crates/k2-core/src/zen/garden-catalog/<short>-<n>.toml`. The Diary
  (`k2.diary@1`, widget `k2:diary@1`) is the first. Catalog Gardens are only
  added when the person picks one in New Garden; never appended to an
  existing list (R5 as changed).
- **The Diary** (Rosson 2026-10-08) is a haunted journal: one page per
  agent on this computer (`<handle>::local` rows only), turned by dragging
  or clicking a page corner or with ←/→ and PageUp/PageDown. Writing on a
  page posts to that agent's Thread; the reply bleeds back in handwriting.
  Its page has no chrome but one ⋯ `menu` (top right) holding the Garden
  switcher and the Zen toggle, so the required-controls check reads the
  menu's button (FC25); ⌃⌘Z still leaves Zen.

### The widget in `GET /cli/zen/get`

A custom placement is a content widget in `page.widgets`
(`ZenCustomWidgetPayload` in `src/renderer/lib/zen/zen-custom-types.ts`):

```json
{ "id": "arcade", "kind": "custom", "widget": "agent-arcade", "column": 0,
  "props": { "home": "Work", "config": {} },
  "caps": ["agents:read"], "requested": ["agents:read", "thread:post"],
  "source": "user", "name": "Agent Arcade", "description": "…",
  "reasons": { "thread:post": "…" }, "libs": ["three@0.170"],
  "hash": "<sha256 of the bundle without nonces>",
  "state": "ok", "errors": [], "warnings": [],
  "origin": "local", "paused": null }
```

- **No permissions** (Rosson's 0.45.1 smoke, 2026-10-08: "If the widget
  exists, it should be able to interact with agents"). `caps` = requested ∩
  widget caps: a widget in your own Garden gets every Garden-safe cap it
  asks for, with no review card, grant, scope picker or Sending switch. The
  renderer intersects again and forces `source: "user"`.
- `origin`: `local` (a folder under `~/.k2/zen/widgets/` or a built-in
  `k2:`). The seam for v4's widgets imported from other people: any other
  origin gets no caps and never runs until a review exists
  (`zenWidgetMayRun`); the renderer reads an unknown origin as `other`.
- `paused`: `{at, reason: "runaway"}` while the runaway guard has posting
  paused (see Limits), else `null`.
- **Reach.** A widget's rows are the Garden's reach, resolved on this device
  (`zenWidgetReachRows`): this computer's agents plus every row on the
  person's Homes (the rooms and servers they connected in Home), at most 8
  live servers. A saved server on no Home is outside it (`not_bound`).
  Remote rows use the person's own Connect login, so every server's role
  floors still apply.
- `state`: `ok`, `errors` (a newer edit has errors; the last good bundle is
  served), `broken` (errors and no last good bundle).

### Routes

All on the local daemon under `/cli/zen/*`, owner token only, Member policy
rows, 64 KB request bodies, a GET on a POST route is 405.

| Route | Body / query | Answer |
|---|---|---|
| `GET widgets` | — | `{ok, widgets: [{name, title, description, caps, hash, state, errors, warnings, placements: [{garden, gardenName, placement, paused}]}]}` |
| `GET widget/bundle` | `?widget=<name or k2:…@n>` | `{ok, widget, hash, nonce, html, bytes}`; 404 `unknown_widget`; 409 `widget_broken` |
| `POST widget/new` | `{name, from?}` (`hello`, `arcade`, `k2:diary`) | `{ok, name, path, files, changed}`; 409 `widget_exists`; 404 `zen_not_set_up` |
| `POST widget/pause` | `{garden, placement, reason: "runaway"}` | `{ok, paused, changed}`; the renderer's runaway guard tripped. Taking power away: anything Zen accepts |
| `POST widget/resume` | `{garden, placement}` | `{ok, paused: null, resumed, changed}`; the person's Resume click: owner token only (a passport, Connect login, API key or app pass gets 403 `owner_only`) |
| `GET templates` | — | `[{id, short, label, description, section, newUsers}]` |

**No grants** (2026-10-08). The first Zen v2 build's grant routes
(`widget/grant`, `widget/revoke`, `widget/sending`, `GET widget/grants`) are
gone (404), and `garden/new` takes no `grant` (400 on the field). Table
`zen_widget_grants` (migration 0138) stays in the schema so existing
databases keep their migration list, but nothing reads or writes it, and
`~/.k2/zen-grant.key` is no longer made or read. The runaway pause is held
in the daemon's memory (a daemon restart clears it, like the renderer's
post counters). `grants.json` stays refused as a `reset`/`validate` target.
`k2 zen widget grant` and `revoke` only print that widgets need no
permissions. Each widget post writes one `zen.widget.post` audit line
(widget, Garden, to, length; never the text), and pause/resume write
`zen.widget.pause` / `zen.widget.resume`.

`validate`, `history` and `reset` take `widget=` (GET) / `{widget}` (POST).
`doctor` gains a `widgets` check.

### The frame protocol

One `MessageChannel` per placement (UW15). On the frame's `load` the host
posts, with one port:

```json
{ "k2": "hello", "v": 1, "caps": ["agents:read"], "features": ["zen-v1", "…"],
  "widget": { "id": "arcade", "name": "Agent Arcade", "garden": "g-test0001" },
  "config": {}, "motion": { "reduced": false } }
```

Then, over the port only:

| Frame → host | Host → frame |
|---|---|
| `{id, verb, args}` | `{id, ok: true, value}` or `{id, ok: false, error: {code, message, cap?, room?, feature?}}` |
| `{sub, verb, args}` | `{sub, value}` per push, or `{sub, error: {code, message, …}}` once when K2 refuses it (the subscription is over; the runtime calls the widget's `onError`, the function after `cb`, or reports it to K2 as an uncaught error) |
| `{unsub}` | — |
| `{ready: true}` (once drawn) | `{ping}` every 5 s after ready |
| `{pong}` | — |
| `{error: {message, stack?}}` | — |
| `{chord}` (forwarded keys) | — |

The frame's first script is `sdk/generated/k2-frame.js` (the shared runtime
`sdk/k2-runtime.js` joined with the catalog's widget verb table as
`K2_CONTRACT`), nonced with the bundle's nonce and outside the widget's code
budget; at most 16 KB.

### Limits (UW8, UW29, UWB8, UWB9, UWB13)

Code (HTML + JS + CSS after inlining) 256 KB; each asset 1 MB, all assets
2 MB; the bundle 3 MB; libraries (`requires.libs`, inlined after `k2.js`) up
to 8 MB per widget, outside those. 6 custom widgets per page. Per widget: 60
calls a second (burst 120), 16 live subscriptions, 64 KB per message, 8 live
servers. Posts are text only, up to 4,000 characters, never files or
secrets; the runaway guard (more than 120 posts in 10 minutes, or 20
identical texts to one agent in 10 minutes) pauses posting (`sending_off`;
the frame keeps running) with a small "Paused: too many posts. Resume"
notice on the widget, in every window, until the person clicks Resume.

### Events

No new event kind: a widget save, a pause and a resume each emit exactly
one payload-free `zen_changed` (UW12, UW37).

## Garden sync with K2's defaults (zen-sync-v1)

prd-zen-garden-sync-defaults-v1. A Garden file still holds only its
changes; what it sits on is now per Garden. Each Garden's **page** (the
template, the widget prop defaults, the frame) and **theme** (the built-in
layers under its theme) is either **synced** (follows K2's improvements) or
its **own copy** (resolves on an archived set of K2's defaults, so no K2
update changes it). The Garden file is never rewritten. A daemon without
`zen-sync-v1` in `/boot-status` has none of this: no switches, no card.

| Call | Who | Body or query | Answer |
|---|---|---|---|
| `GET /cli/zen/sync` | Settings → Gardens; `k2 zen sync` | none | `{ok, liveDefaults, k2Version, previousDefaults, migratedAt, gardens: [SyncRow]}` |
| `POST /cli/zen/garden/sync` | the person only (Settings, CLI) | `{garden, part: "page"\|"theme"\|"both", sync: bool}` · `{garden, undo: true}` · `{garden, keep: "previous", part?}` | `{ok, garden: SyncRow, changed, announced}`. A change emits ONE `zen_changed`. GET → 405 |
| `GET /cli/zen/news` | the What's new side card; `k2 zen news` | none | `{ok, items: [NewsItem], copiesWithNewerDefault, liveDefaults, previousDefaults}`, unseen only, newest first |
| `POST /cli/zen/news/seen` | closing What's new; the card's button | `{ids: [id]}` or `{all: true}` | `{ok, seen: [id]}`. GET → 405 |
| `GET /cli/zen/get?garden=<id>&preview=page\|theme\|both` | Settings' Preview in Zen | as `get` | the Garden resolved as if those parts were synced; `sync.preview` names them. Writes nothing |

Every route is owner-token only, like all of `/cli/zen/*`: an agent
passport, a Connect login or an app pass gets 403 `zen_local_only`. Agents
edit Garden files and suggest the switch to the person.

`SyncRow`: `{id, name, index, page: Part, theme: Part, ownChanges: [key] |
null (file doesn't parse), ownChangesError, undo: {parts, at} | null,
keepPrevious: {page, theme}, themeName, themeLabel, themeBuiltin,
themeBase: {name, label}, gardenTheme, damaged: [{defaults, message}]}`.
`Part`: `{mode: "synced"|"copy", defaults?: "d-<16 hex>", since?, reason?,
newerDefault, k2Version?}`; `newerDefault` is true for a copy whose synced
look would differ.

`NewsItem`: `{kind: "catalog", id: "catalog:<short>", short, label,
description, template, version}` or `{kind: "update", id:
"update:<liveFp>:<garden>", garden, gardenName, part, previous, at,
keepPrevious}`. Skip kinds you don't know.

`GET /cli/zen/get` adds two top-level fields:

- `sync`: `{page: {mode, defaults?, since?, newerDefault}, theme: {…},
  preview?}`;
- `frame`: the frame values (GF1, `crates/k2-core/src/zen/frame.toml`) of
  the page's defaults, with the control-check thresholds clamped to K2's
  floor. The renderer reads it from 0.45.2 (S7); until then its constants
  are pinned to frame.toml by `zen-frame-parity.test.ts`.

`version` covers both. A synced page whose file names `k2.<short>@<n>`
shows that family's newest shipped version; an own copy shows exactly the
id it names, from its set. Anything a set doesn't have resolves live.

Files (daemon-written; never edit, never watched): `sync.json`,
`news.json`, `.defaults/<fp>.json` (one archived set per K2 release that
changed a default; never pruned) and a per-Garden mirror
`.history/gardens/<id>.toml/sync.json` that travels with delete. The
one-time upgrade pass (the first Zen read after the first boot of 0.45.1)
makes every Garden whose file sets anything its own copy, page and theme,
and leaves every untouched Garden synced; it writes no Garden file.

## Contract changes from the PRD text (flagged for the renderer)

The PRD's route contract is followed; these points were open in it or are
additions, decided here:
1. `index` is **1-based** in every answer.
2. Each `Garden` carries `theme` (its own pick or `null`), for the CLI's
   `*` and a switcher that wants it.
3. `setup` also answers `createdDefault`, `path` and `changed`; `migrated`
   is always `null` (no migration, Rosson).
4. `garden/new` also answers `file`, `path`, `changed`; `garden/delete` also
   answers `name`, `gardens`, `changed`; `garden/rename` answers
   `{ok, garden, changed}`; `garden/reorder` answers `{ok, gardens, changed}`.
5. The removed POST routes are 405 (method guard), not 404; a GET is 404.
6. Widget props for answer 5 (not in the PRD's G38 list): `agents.mode`,
   `agents.agent`, `conversation.agent`, `conversation.home`. The daemon
   sends **every** prop with K2's defaults filled, `agents.mode` derived,
   and `conversation.agents` linked to the first Agents widget.
7. `theme/list` answers `garden`/`gardenTheme` (was `home`/`homeTheme`); a
   theme switch answers `garden` (was `home`); `theme.scope` is
   `"garden"`/`"global"`.
8. `history` entries for Garden files carry `garden` and `deleted`.
9. `unknown_garden` also carries `garden` (what was asked for).

## Renderer notes (2026-10-04)

What the renderer (`src/renderer/lib/zen/`, `components/Zen/`) does with
this contract, where it reads more loosely or decides something:

1. **Feature key.** `scopeForHost('local').serverSupports` reports every
   feature as supported, so it can't tell an older daemon. The renderer
   treats `{error: "unknown zen route"}` (404) on `GET /cli/zen/gardens` as
   "K2 on this computer is older than this app. Update it to use Gardens."
   Keep that 404 for unknown Zen GET routes.
2. **Setup.** After `POST /cli/zen/setup` the renderer re-reads
   `GET /cli/zen/gardens` instead of using `setup`'s `gardens`.
3. **New Garden** (Rosson, 2026-10-04; one modal since 2026-10-08). "+ New
   Garden" in the switcher (or a `menu` holding it) closes the menu and
   opens the New Garden modal (`ZenNewGardenModal`): a name field and the
   templates as cards with a small sketch, the catalog first (the Diary
   leads), then **Start with the default** (`template: "texting"`, Garden
   1's layout) and **Start empty and ask my agent** (`template: "blank"`;
   the new Garden's empty-Garden widget then opens Ask my agent by itself,
   once). Pick a card, name it, Create: one step. The name follows the pick
   until typed ("Diary", "Garden 4"). There is nothing to agree to: a
   catalog Garden's widgets work the moment it's created (no permissions).
   It sends `{name, template}` and reads `garden` from the answer; the window switches to it at
   once and the list is re-read on the `zen_changed` that follows. Names are
   checked case-insensitively before the create, and a 409 `garden_exists`
   stays in the modal with "You already have a Garden called “<name>”.".
   Settings → Gardens' "+ New Garden" and its Garden catalog cards open the
   same modal.
3a. **Start with the default** (Rosson, 2026-10-04). The empty-Garden widget
   shows it next to Ask my agent only while the Garden is empty (the blank
   template with only its `garden-empty` widget). It sends
   `garden/template {garden: <this Garden>, template: "texting"}` with no
   confirmation (nothing is lost); on 409 `garden_has_changes` it asks once
   and resends with `force: true`. The page switches on the `zen_changed`
   that follows, like any other change.
4. **`unknown_garden`** on `get` re-reads the list; the window moves to the
   first Garden. A window's stored Garden that the list lacks is replaced by
   the first one.
5. **Layout.** `split` / `minWidths` are read; when `split` is missing the
   `columns` tables are read instead (`size`, `min-width`).
6. **Props.** `agents`: `mode`, `home`, `agent`, `home-picker`,
   `server-tag`, `preview`, `status`. `conversation`: `agents`, `agent` +
   `home` (pinned, opens itself), `compose`, `attachments`, `load-older`.
   `order` is not read (rows are always in Home order). Props are read
   loosely: a missing one gets the default in the table above.
7. **The Agents widget's Home** is per Garden and widget in localStorage
   `k2.zen.gardenHomes.v1` (`{version:1, picks:{"<gardenId>/<widgetId>":
   "<homeId>"}}`): its pick, else `home`, else the Garden's `seedHome`, else
   the window's selected Home at first show (then kept). It never moves the
   Home page.
8. **Bridge verbs** `gardens.list/current/switch` (no cap),
   `gardens.create(name, template?, {ask})`/`rename`/`delete`
   (`gardens:manage`, template controls only), `gardens.empty()` and
   `gardens.useTemplate(template, {force})` (`gardens:template`, the
   `garden-empty` widget only; it acts on the Garden on screen, never
   another), `homes.list`/`agents.home`/`agents.setHome`/`agents.local`
   (`agents:read`), `focusGroups.get/set/subscribe` (`agents:read`, the
   Agents view's dropdown; `set` never switches workspaces),
   `compose.draft` (`thread:post`), `app.open(page)` / `app.current()` /
   `app.subscribeCurrent(fn)` / `app.badges()` / `app.subscribe(fn)`
   (`app:navigate`: `page` is `home`, `agents`, `projects` or `tickets`,
   the rail's view in this window); `homes.select` is gone.
9. **Placement** (Rosson, 2026-10-04; prd-zen-freeform-chrome). The
   template's chrome (and safe mode's) is one top band: the Garden switcher
   top left, the drag strip, then the top-right cluster — K2's usage tool
   (the top bar's subscription usage chip and menu), then its theme
   control, immediately left of the Zen toggle, which sits in the window's
   top-right corner (clear of Windows' controls via
   `--zen-stoplight-safe-right`). A Garden file may move every one of them
   (`page.chrome`, `page.bands`, `page.edges`, `page.menus`; see "Zen
   controls"). The renderer draws all of it from those fields
   (`components/Zen/ZenBands.tsx`; chrome by kind in `zen-registry.tsx`);
   a daemon that sends no `page.chrome` gets the template's chrome built
   from the renderer's own built-in page (FC32). Safe mode always draws
   the template's chrome without usage and theme (FC22).
   - **Bands.** The top band (drawn only when it holds something) keeps
     the window buttons' insets; with nothing in it the renderer draws
     the **title strip**, an empty drag strip as tall as the window
     buttons (`--zen-stoplight-safe-top`). The bottom band is drawn only
     when it holds something. K2 binds `drag-region` on each band's (and
     the title strip's) own element; each band keeps at least 120 px of
     empty space between its `start`, `center` and `end` groups
     (`ZEN_DRAG_MIN_WIDTH_PX`). A run of content widgets in a row sits in
     one `ZenBandSlot` (`components/Zen/ZenBand.tsx`), which shrinks and
     clips first.
   - **Column edges** are rows above and below a column's box (where a
     row rail always sat); not drag areas. In a one-column rail view
     (Projects, Tickets) every column-edge item moves to column 0's same
     edge and alignment (FC48).
   - **Menus** (`kind = "menu"`, `components/Zen/widgets/ZenMenu.tsx`): a
     button (bound `zen-menu` with its id) that opens a K2 menu toward the
     page; Gardens are listed inline with + New Garden, Exit Zen Mode is a
     bound `zen-toggle` row, Theme › and Usage › swap to a sub-panel.
     Keyboard per FC19.
   - **Dropdowns** (the switcher's, the theme list, menus) open toward the
     page through `useAnchoredMenu` (down from the top, up from the
     bottom), portalled into the Zen root as K2 overlays. The usage menu
     is the app's `UsageButton`, flipped by Zen's CSS only (FC51).
   - **Narrow windows** (FC27): column min widths scale down to fit; a
     crowded row hides usage, then the theme control, then truncates the
     switcher's name; required controls and menu buttons never shrink.
   No footer under any column. **Add agent**
   is the Agents widget's last row (its bottom-left corner), shown in
   whole-Home mode when the widget holds `agents:add`; it opens the picker
   for that widget's own Home.
10. **The required-controls check** (Z27/Z28) checks the two required
    controls only. The switcher's wiring rule: after the trigger is
    activated, every Garden must get a bound `garden-option` within 1 s. It
    passes the moment that happens (closing the menu inside the second is
    fine), and an activation while the options are already bound (the click
    that closes the menu) passes at once. K2 never cancels Enter/Space on the
    trigger. A resize runs the check once, 300 ms after resizing stops.
    It reads where the page put each control first (`page.chrome`, FC25):
    a control inside menu M is checked through M's button (visible, can
    take focus, not `[inert]` / `aria-hidden`), never itself, so a closed
    menu is not a missing control. After M's button is activated, every
    required control M holds must bind within 1 s (each menu has its own
    timer); an activation while they are bound passes at once. Problems:
    `undeclared`, `missing`, `not-wired`, `invisible`, `no-keyboard`; the
    cause names the menu ("The Zen toggle’s menu (More) isn’t visible.").
    An open K2 menu is a K2 overlay, never "covering" a control.
11. **Focus after a pick** (Rosson, 2026-10-04). A click on an Agents row,
    or adding an agent through Add agent (which closes the picker and opens
    that agent), puts the caret in the conversation's "Message <agent>" box
    once it can be typed in. Nothing else moves focus (first render, a
    remote update, ⌘1–9, a Garden switch); a pick waits at most 15 s and
    never takes focus from another field the person started typing in.
12. **macOS stoplights in Zen** (Rosson, 2026-10-04) sit 8px further right
    and 8px further down than the Style's (before the theme's
    `stoplight-offset`); `--zen-stoplight-safe-*` include it, so the Garden
    switcher starts at 91px and the top band (at least 52px) stays below
    them. Leaving Zen restores the Style's position. Linux and Windows are
    unchanged.
