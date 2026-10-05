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

## The model

- **Zen is a window mode.** On/off lives in the renderer per window
  (`k2.zen.window.v1.<label>`). The daemon knows nothing about windows.
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
    "widgets": [ Widget, … ],
    "controls": [                  // always the template's; a Garden file can't change them
      { "kind": "garden-switcher", "placement": "top-left" },
      { "kind": "drag-region", "placement": "top" },
      { "kind": "zen-toggle", "placement": "top-right" },
      { "kind": "add-agent", "placement": "widget-bottom-left", "widget": "agents" }   // texting only
    ]
  },
  "theme": { … }, "themes": [ … ], "chrome": { … }, "motion": { … },     // see Themes
  "errors":   [{ "file": "gardens/g-3f9a12c0.toml", "line": 7, "col": 1, "message": "…" }],
  "warnings": [ … same shape … ],
  "lastGoodAt": "2026-10-04T18:00:00Z",
  "sources": { "zen.toml": "file", "gardens/g-3f9a12c0.toml": "snapshot:…", "themes/default/theme.toml": "default" }
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
| widgets | `agents` (column 0, `home-picker: true`), `conversation` (column 1, `agents: "agents"`), `nav-rail` (id `nav`, column 0) | `garden-empty` (column 0) |
| controls | `garden-switcher`, `drag-region`, `zen-toggle`, `add-agent` | `garden-switcher`, `drag-region`, `zen-toggle` |

**Required controls** (G24; Rosson, 2026-10-04: exactly two):
`zen-toggle`, `garden-switcher`. Every template declares both, plus
`drag-region`, which K2 binds for window drag but never checks.
`home-switcher` and `home-option` are gone from Zen. The renderer binds `garden-option` (with its Garden id) the way it
bound `home-option`. Template controls get the caps `agents:add` and
`gardens:manage` (G29); widgets never get `gardens:manage`.

### `Widget`

```jsonc
{
  "id": "agents",                  // unique on the page
  "kind": "agents",                // agents | conversation | nav-rail | garden-empty
  "column": 0,
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
| `agents` | `status` | list of `working`, `idle`, `needs-you` | all three | which live statuses show |
| `conversation` | `agents` | string (a widget id) | the page's first `agents` widget (column order) | follow that widget's picked agent; **always sent** unless `agent` is set |
| `conversation` | `agent` | string | unset | pin one agent's conversation (name or address), no list needed |
| `conversation` | `home` | string | unset | with `agent`: the Home to look it up in (id or name) |
| `conversation` | `compose` | bool | `true` | the compose box |
| `conversation` | `attachments` | bool | `true` | attachments in the box |
| `conversation` | `load-older` | bool | `true` | load older on scroll |

`nav-rail` has no props (Rosson, 2026-10-04). It is a thin (44px), icon-only
rail drawn at the **left edge of its column, outside the column's box**; it
takes no share of the box. Top to bottom: **My Home** (current: the Garden
itself), **Agents**, **Projects**, **Tickets** (with the top bar's waiting
badge), each with the top bar's page name as its tooltip. Agents, Projects
and Tickets leave Zen in this window and open that page (`app.open`), the
same page store the top bar uses. Only `k2.texting@1` places it; a Garden
file may place it like any built-in widget.

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

1–3 columns, at most 12 widgets. A `caps` key anywhere is an error;
`[[control]]` is a warning and ignored; an unknown kind or prop is an error
at its line; a widget's `column` must exist in the effective layout; a
`conversation`'s `agents` must name an `agents` widget on the page; a
`conversation` with neither `agents` nor `agent` needs an `agents` widget to
follow. Theme tables restyle that Garden only.

## Themes (Omarchy additions, 2026-10-04)

`theme` changed shape for theme bundles. It is no longer
`{scheme, colors, type, shape}`. `chrome` and `motion` stay top level.

```jsonc
"theme": {
  "name": "default",            // the active theme for this Garden
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
"themes": [ { "name": "default", "builtin": true, "user": false, "summary": "…", "active": true }, … ]
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
  1. K2's built-in `default` theme;
  2. K2's built-in copy of the active theme;
  3. `themes/<active>/theme.toml`;
  4. `zen.toml`;
  5. `gardens/<id>.toml`.

  The built-ins are `default`, `paper` and `midnight`. They are embedded in
  the daemon and read-only. `~/.k2/zen` holds only the user's changes, and
  the `zen.toml` stub is empty. Page templates are the same: the built-in
  `k2.texting@1` and `k2.blank@1` are the defaults, and a Garden file holds
  only its changes (theme tables, and optionally its layout of built-in
  widgets).
- **Errors.** A theme file with errors keeps its last good version, the
  same way as `zen.toml`. The errors come back in `errors` with
  `file: "themes/<name>/theme.toml"`. If a theme pick points to a deleted
  theme, `default` is shown and a warning is added for `active.json`.

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
key left out gets K2's default, which is the built-in `themes/default.toml`;
`zen-theme-engine.test.ts` checks that the two match.

```jsonc
"theme": {
  "name": "default", "builtin": true, "user": false,
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
"themes": [{ "name": "default", "builtin": true, "user": false, "active": true }, …],
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
3. **New Garden.** The switcher sends `{name}` only (the daemon's default
   template, `blank`) and reads `garden` from the answer; the window switches
   to it at once and the list is re-read on the `zen_changed` that follows.
   Names are checked case-insensitively before posting, and a 409
   `garden_exists` shows "You already have a Garden called “<name>”.".
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
   `gardens.create/rename/delete` (`gardens:manage`, template controls
   only), `homes.list`/`agents.home`/`agents.setHome`/`agents.local`
   (`agents:read`), `compose.draft` (`thread:post`), `app.open(page)` /
   `app.badges()` / `app.subscribe(fn)` (`app:navigate`: `page` is `home`,
   `agents`, `projects` or `tickets`; `home` does nothing); `homes.select`
   is gone.
9. **Placement** (Rosson, 2026-10-04). One top band for both templates and
   safe mode: the Garden switcher top left, the drag strip, then the
   top-right cluster — K2's theme control immediately left of the Zen
   toggle, which sits in the window's top-right corner (where the top bar's
   Zen toggle is outside Zen; clear of Windows' controls via
   `--zen-stoplight-safe-right`). No footer under any column. **Add agent**
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
