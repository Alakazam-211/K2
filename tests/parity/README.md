# Screenshot parity harness (Style System — Checkpoint A)

Proves the P1 "Square alias" migration (`.k2/prds/prd-style-system-v1.md` §6)
changed **nothing** visually: capture the same screen matrix at two git states
(`main` vs `feat/style-system`), then pixel-diff with a **0-pixel tolerance**.

- Engine: **Playwright WebKit only** — the app ships in a WKWebView; Chromium
  rasterizes text/shadows differently and would poison a strict diff.
- No daemon required (and the user's production daemon is never touched):
  the renderer runs under `vite dev` with a fully mocked daemon (see
  "How the mock works" below). Captures are **byte-identical across runs**
  on the same source — verified against cold vite restarts.

## One-time setup

```sh
bun install                    # brings in @playwright/test, pixelmatch, pngjs
npx playwright install webkit  # downloads the WebKit browser binary (not committed)
```

## Producing the Checkpoint A evidence

The harness lives on `feat/style-system`. `main` predates it, so the harness
files are *borrowed* onto main for the "before" capture (they only read the
app through vite — they don't alter what's being screenshotted).

```sh
# 1) BEFORE — capture on main with the branch's harness borrowed in
git checkout main
git checkout feat/style-system -- tests/parity package.json bun.lock
bun install
PARITY_LABEL=before bun run parity:capture

# 2) drop the borrowed files, go to the branch (shots/ is gitignored and survives)
git reset --hard HEAD
git checkout feat/style-system
bun install

# 3) AFTER — capture on the branch
PARITY_LABEL=after bun run parity:capture

# 4) DIFF — exits non-zero on ANY pixel difference
bun run parity:diff
```

Output: `tests/parity/shots/{before,after}/<screen>.png`, per-screen PASS/FAIL
on stdout, `shots/_diff/report.json`, and a `shots/_diff/<screen>.diff.png`
heat-map for every mismatching screen.

Notes:
- Do **not** keep your own vite server running on port 5199 — each capture
  must cold-start vite so it serves the current checkout (Playwright starts
  and stops the server itself).
- `parity:diff` accepts explicit labels: `bun run parity:diff before after2`.
- Screens must match EXACTLY: a missing screen, a size change, or 1 differing
  pixel all fail the diff. There is no tolerance knob on purpose.

## Screen matrix (8 screens, 1440×900, dark, WebKit)

| Screen | Surface exercised |
|---|---|
| `01-connect-gate` | Pre-daemon full-screen "Connecting…" overlay (ConnectionGate) |
| `02-app-home` | Full app shell: TopBar (page tabs, server switcher), Sidebar empty state, main empty state |
| `03-projects-page` | Projects overlay page: ProjectNav + no-projects empty state |
| `04-feedback-page` | Feedback page: search bar, status-filter chips, list + thread empty states |
| `05-settings-general` | Settings master-detail shell + General section (rows, toggles, steppers) |
| `06-settings-terminal` | Terminal settings section (font/cursor/scrollback controls) |
| `07-settings-keybindings` | Keybindings section (key-combo table) |
| `08-settings-connections` | Connections section: K2 Connect panel, users/access, federation panes |

### Not covered (and why)

- **Terminal panes / Kessel with a live PTY** — needs a real daemon+PTY;
  output would be inherently nondeterministic. Terminal *chrome* gets its
  parity gate at Checkpoint B (P2, per-directory conversion) and via
  Rosson's daily-driving half of Checkpoint A.
- **Populated Projects dashboard / tiling grid** — mock workspaces would
  spawn terminal panes (see above). Empty states + Settings still exercise
  the overwhelming majority of tokens: globals.css variables, borders,
  buttons, inputs, toggles, tabs, list rows.
- **Settings → Timer** — renders live clock/timezone-dependent content.
- **Settings → General "K2 Server" row in `running` state** — shows PID +
  live uptime; the mock pins it to `not_installed`.
- **Modals/dialogs (Add Workspace, New Project, What's New…)** — cut for
  V1 to keep the matrix honest; add later if Checkpoint A wants more.
- **Light scheme** — Square today is dark-only; the scheme axis lands in P4.

## How the mock works (tests/parity/mock-daemon.ts)

1. **Tauri shim** — `window.__TAURI_INTERNALS__` is stubbed via
   `addInitScript`: event listens resolve, `daemon_ws_url` reports
   `not_installed` (so the app can never reach a real local daemon), version
   commands return a FIXED fake (`0.0.0-parity`) so a real version bump
   between the two git states can't show up as a diff. Unknown invokes
   REJECT loudly rather than silently succeeding with a wrong shape.
2. **Mock remote daemon** — all HTTP/WS to `127.0.0.1:45999` (nothing
   listens there) is fulfilled by Playwright route interception with canned,
   timestamp-free responses: `/boot-status` ready, whoami ok, a fresh-daemon
   default `/cli/settings/get` snapshot, `[]` for list routes, `{}` otherwise.
3. **Steering** — vite dev serves the renderer's modules by URL, so
   `import('/stores/…')` inside `page.evaluate` returns the LIVE zustand
   store singletons. The spec adds a ConnectHost for the mock address and
   `selectHost()`s it; the ConnectionGate's remote policy accepts the canned
   boot-status and mounts the real App. Page/Settings navigation drives the
   same live stores (`page-view`, `settings`) — no brittle click paths.

Determinism hardening: fixed viewport/DPR, `colorScheme: 'dark'`,
`reducedMotion: 'reduce'`, injected CSS killing all animations/transitions/
carets, `document.fonts.ready` + double-rAF + fixed settle before every shot,
and screenshots taken with `animations: 'disabled'`, `caret: 'hide'`.

Failure policy: everything fails loudly. `PARITY_LABEL` is mandatory; each
screen asserts a marker element AND that the app error boundary is NOT
showing before capturing; the diff fails on missing dirs, missing screens,
size mismatches, and any nonzero pixel count (`pixelmatch` `threshold: 0`,
`includeAA: true`).

### Why pixelmatch instead of `toHaveScreenshot`

`toHaveScreenshot` compares against baselines committed next to the spec —
one git state. This harness compares two *capture runs from two checkouts*,
so baselines-in-git is the wrong model: a standalone diff over
`shots/<label>/` keeps the two runs symmetric, produces a per-screen report
+ diff PNGs for the Checkpoint A record, and needs no snapshot-update mode
(there is nothing to "update" — a diff is simply a failure).
