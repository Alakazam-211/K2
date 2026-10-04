# Zen v1: what the renderer (S4) reads from the daemon (S1)

The S4 renderer was built in parallel with the S1 daemon routes. This page
records exactly what the renderer sends and parses, so the two can be checked
against each other. Source of truth: `.k2/prds/prd-zen-mode-v1.md` Z9–Z16 and
the vs-live amendments Z58–Z62. The renderer side is in
`src/renderer/lib/zen/zen-api.ts` and `src/renderer/lib/zen/zen-page.ts`.

Every Zen config request goes to this computer's daemon through
`scopeForHost('local')` (owner token). None goes through `primaryScope()`.

## Requests

| Call | When | Body or query |
|---|---|---|
| `POST /cli/zen/page/ensure` | Before the first `get` for a Home in this app session | `{"homeId": "<uuid>", "name": "<Home name>"}` |
| `GET /cli/zen/get` | Zen opens for a Home, Try again, and every `zen_changed` | `?home=<uuid>` |
| `POST /cli/zen/homes/sync` | A Home is created, renamed or deleted, and some Home has Zen on | `{"homes": [{"id": "<uuid>", "name": "<name>"}, …]}`, the full list in Home order |

**Flag 1, `homes/sync` body.** The PRD names the route (Z11) but not its body.
The renderer sends the full list shown above. The daemon writes `homes.json`
from it and moves the page files of Homes that are gone into `.history/`.

**Flag 2, `homes/sync` must not create the folder.** The renderer only calls it
once `k2.zen.homes.v1` has a Home turned on. Even so, the daemon should treat
a sync with no `~/.k2/zen/` folder as a no-op. Z8 says only `page/ensure`
creates the folder.

## `GET /cli/zen/get` answer

What the PRD fixes (Z15):

```
{ ok, schema, version, page: { template, layout, widgets, controls },
  theme, chrome, motion, errors, warnings, lastGoodAt }
```

**Flag 3, inner shapes.** Z10 doesn't spell out the inner shapes. The
renderer reads them as follows and is lenient about the rest:

```jsonc
{
  "ok": true,                      // not read; a page present is what counts
  "schema": 1,                     // any other value: "this K2 reads Zen schema 1" (safe mode)
  "version": "b3f…",               // string; changes whenever the resolved result changes
  "page": {
    "template": "k2.texting@1",
    "layout": {
      "kind": "columns",
      "split": [34, 66],           // percent per column
      "minWidths": [240, 360]      // px per column; `min_widths` also accepted
    },
    "widgets": [
      { "id": "agents", "kind": "agents", "column": 0, "props": {},
        "caps": ["agents:read", "presence:read"], "source": "builtin" },
      { "id": "conversation", "kind": "conversation", "column": 1, "props": {},
        "caps": ["agents:read", "presence:read", "thread:read", "thread:post"], "source": "builtin" }
    ],
    "controls": ["zen-toggle", "home-switcher", "drag-region"]   // strings, or objects with `kind`
  },
  "theme": { … }, "chrome": { … }, "motion": { … },             // passed whole to the S5 theme engine
  "errors":   [{ "file": "zen.toml", "line": 12, "col": 3, "message": "unknown color 'acent'" }],
  "warnings": [ … same shape … ],
  "lastGoodAt": "2026-10-04T18:00:00Z"
}
```

Fallbacks:

- `widgets`: `type` is accepted for `kind`, and `col` for `column`.
- A missing `layout` or `widgets` falls back to the built-in `k2.texting@1`.
- A missing `controls` does **not** fall back. A page that declares none fails
  the "declared" check, and the window goes into safe mode.

**Errors with a last good version.** When a file has errors, `get` should
answer **200** with the last good `page` plus `errors`. The `ok` field is not
read. A non-2xx status, or a body with no `page`, puts the window into safe
mode:

- No answer at all: "Can't reach K2 on this computer".
- An answer that isn't a page: "K2 on this computer sent a Zen page this app
  can't read".

## Themes (Omarchy additions, 2026-10-04)

`theme` changed shape for theme bundles. It is no longer
`{scheme, colors, type, shape}`. `chrome` and `motion` stay top level.

```jsonc
"theme": {
  "name": "default",            // the active theme for this Home
  "builtin": true,              // shipped inside K2 (read-only)
  "user": false,                // ~/.k2/zen/themes/<name>/theme.toml exists (for a built-in: an override)
  "scope": "global",            // "home" when the Home has its own pick
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
- **Layering.** For one Home the stack is, lowest first:
  1. K2's built-in `default` theme;
  2. K2's built-in copy of the active theme;
  3. `themes/<active>/theme.toml`;
  4. `zen.toml`;
  5. `pages/<home>.toml`.

  The built-ins are `default`, `paper` and `midnight`. They are embedded in
  the daemon and read-only. `~/.k2/zen` holds only the user's changes, and
  the `zen.toml` stub is empty. The page template is the same: the built-in
  `k2.texting@1` is the default, and a page file holds only overrides.
- **Errors.** A theme file with errors keeps its last good version, the
  same way as `zen.toml`. The errors come back in `errors` with
  `file: "themes/<name>/theme.toml"`. If a theme pick points to a deleted
  theme, `default` is shown and a warning is added for `active.json`.

| Call | Body | Answer |
|---|---|---|
| `GET /cli/zen/theme/list` | `?home=<id|name>` | `{active, scope, global, home, homeTheme, missing, themes}` |
| `POST /cli/zen/theme/set` | `{name}`, or `{name, home}` for one Home, or `{home, clear: true}` to drop the Home's pick | `{ok, theme, scope, home, changed}` |
| `POST /cli/zen/theme/next` and `/prev` | `{}` or `{home}` | same as set; cycles `themes` in order and wraps |
| `POST /cli/zen/theme/new` | `{name, from?}` | `{ok, name, file, path, from, copiedImage, changed}`; 409 `theme_exists` when the theme is already there |
| `POST /cli/zen/reset` | `{theme}` (as well as `file` and `home`) | removes an override of a built-in theme (`restored: "builtin"`) |

An unknown name gets 404 `{error: "unknown_theme", theme, themes, message}`
and changes nothing. Each switch emits one `zen_changed`, so the renderer
re-`get`s. Cycle order: built-ins first in K2's order, then the user's
themes by name. The renderer may cycle on the client over `themes`, or
call next and prev.

## `theme`, `chrome`, `motion`: what the S5 engine reads

Source: `src/renderer/lib/zen/zen-theme-engine.ts`, `zen-chrome.ts`,
`zen-motion.ts`, `zen-tokens.ts`. Only the names below are read; any other
key is ignored (logged once) and never reaches CSS. A value that is present
but doesn't parse keeps the last good value for that key; a key left out is
K2's default (`default-zen.toml`).

```jsonc
"theme": {
  "name": "tokyo-night",                 // shown as the active theme
  "tokens": {                            // or these four keys flat on `theme` (S1 today)
    "scheme": "auto",                    // auto | light | dark
    "colors": { "light": { "canvas": "#faf7f2", … }, "dark": { … } },   // 16 tokens each
    "type":   { "family": "system", "size": 14, "line-height": 1.45 },
    "shape":  { "radius": 14, "bubble-radius": 18, "gap": 12, "list-width": 300 }
  },
  "font": "JetBrains Mono",              // system | rounded | serif | mono | MesloLGM Nerd Font | JetBrains Mono
  "terminal": { "palette": {             // flat, {light, dark}, or 16 ANSI colours as an array
    "foreground": "#…", "background": "#…", "cursor": "#…", "cursorAccent": "#…", "selection": "#…",
    "black": "#…", … "brightWhite": "#…" } },
  "background": { "data": "data:image/png;base64,…", "dim": 0.8 }      // optional
},
"themes": [{ "name": "k2-light", "builtin": true }, { "name": "mine", "builtin": false }],
"chrome": { "corners": "system", "stoplights": "round", "stoplight-offset": [0, 0] },
"motion": { "animations": { "<name>": { "on": true, "speed": 3, "bezier": [0.22, 1, 0.36, 1], "style": "popin 92%" }, … } }
```

**Flag 5, background.** Only a base64 `data:image/(png|jpeg|webp|gif|avif)`
URL is drawn, in `data` or `url`. The app CSP allows `data:` and `blob:`
images only, and Zen loads nothing remote. A daemon-served URL would need a
fetch-to-blob path; send `data` instead. `dim` (0.5–0.95, default 0.8) is the
canvas scrim over the image for readability. Reduced transparency drops it.

**Flag 6, `motion`.** The renderer rebuilds each curve from the four
`bezier` numbers and the duration from `speed` (tenths of a second). The
`ease`, `durationMs` and `curve` strings are not read.

**Flag 7, theme switch route.** The picker and the cycle keys call
`POST /cli/zen/theme/set {"name": "<theme>"}` on the local daemon, then
re-read `get`. Next and previous are computed from the `themes` order, so
the renderer needs no `next` or `prev` route. `zen_changed` should follow a
switch like any other change.

## Event

`zen_changed` is an app-class kind with no payload. The renderer adds it to
`src/shared/session-event-kinds.json` and its TypeScript route table.

**Flag 4, merge order.** The Rust `session_event_kinds_match_shared_registry`
test fails until the daemon has `SessionEvent::ZenChanged {}` (Z58). Land S1
first, or cherry-pick the two together. If S1 also adds the JSON line, keep
one copy when merging.

The renderer holds its own app socket to the local daemon
(`subscribeToActiveState(scopeForHost('local'))`) while Zen is on screen. It
does this whichever server the window is on.

## Not used by S4

- `GET /cli/zen/validate`, `/cli/zen/history`, and `POST /cli/zen/reload` and
  `/reset` are for the CLI.
- `/boot-status` `zen-v1`: the local scope always reports every feature as
  supported. If the routes are missing (a 404), the window goes into safe mode
  with "Can't reach K2 on this computer".
