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
