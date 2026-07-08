# PRD — Style System V2: Runtime-Loaded Styles

**Date:** 2026-07-08 · **Owner:** Rosson · **Status:** SPEC ONLY — build after Styles V1 ships (~0.40.38)
**Audience:** written so a junior developer can build this without having lived through V1.
**V1 references:** `.k2/prds/prd-style-system-v1.md` (the shipped system) · `.k2/notes/design-system-skins-research.md` (research SSOT) · `CONTRIBUTING-STYLES.md` (the PR contribution path this V2 complements, not replaces)

---

## 1. Summary

V1 Styles are **compiled into the app**: `styles/<id>/` JSON packages → `scripts/build-styles.mjs` → `src/renderer/styles.generated.{css,ts}` → bundled at build time. Users can only get new styles by app update or by building from source.

V2 adds **runtime-loaded local styles**: the app reads style packages from a folder on the user's machine (`~/.k2/styles/`), validates them with the *same rules CI enforces*, injects their CSS at runtime, and shows them in Settings→Styles under a "Local" section. Editing a local style file updates the running app live.

**Why:** terminal users theme everything (iTerm/alacritty/nvim are all runtime files). The PR path is the *curation* funnel (reviewed, shipped to everyone); runtime loading is the *tinkerer* funnel (instant, personal, no toolchain). Both use the identical package format, so a local style that turns out great can be PR'd verbatim.

**Performance stance (already analyzed, don't re-litigate):** zero per-frame cost — WebKit doesn't care where CSS came from. Startup cost is single-digit ms. The only real perf lever is what *effects* a style uses (backdrop-filter), which V1's contract already governs (material gating, `surface.solid` rule). See pre-mortem §6.3 and §6.4 for the two real risks: first-paint flash and pathological overrides.

### Goals
1. Drop a valid style folder into `~/.k2/styles/<id>/` → it appears in Settings→Styles (labeled **Local**) without restarting the app.
2. Live authoring: saving any file in a loaded local style re-validates and re-applies it in place.
3. Local styles are validated at load with the same policy as CI; invalid packages are *skipped with a visible, specific error* — never a crash, never a silent half-load.
4. An active local style survives app relaunch without a flash of the default style.
5. Cross-machine sync degrades gracefully: if the daemon-synced style id isn't present on this machine, fall back to Square without error spam.

### Non-goals (V2)
- No marketplace / `k2 style install <url>` (V3; see §8).
- No runtime loading of *palettes into first-party styles* (a local palette requires wrapping it in a local style folder; relaxing this is V3).
- No remote-host styles: styles are **per-client view state** (see `project_canonical_vs_per_client_view_state` memory) — they load from the client machine, never from a connected daemon.
- No sandboxing beyond CSS policy: styles remain CSS+JSON data. If a future proposal wants JS in styles, that is a different (and much scarier) PRD.

---

## 2. Background — how V1 works (read this before touching anything)

The pipeline you are extending:

```
styles/<id>/style.json          manifest: name, capabilities, palettes, dials
styles/<id>/tokens.json         non-color slots (radius/gap/ring/shadow/material/motion/font)
styles/<id>/palettes/<p>.json   ~50 required color slots + terminal ANSI-16
styles/<id>/overrides.css       optional, policy-bounded CSS escape hatch
        │
        ▼  scripts/build-styles.mjs  (validation + emission — THE reference implementation)
src/renderer/styles.generated.css   :root fallback + [data-style]/[data-palette]/[data-gaps] blocks
src/renderer/styles.generated.ts    typed registry: STYLES: StyleMeta[] (incl. terminal colors as
        │                           0xRRGGBB ints for Kessel, swatches, dials, gapPresets)
        ▼
src/renderer/stores/style.ts        applies selection → stamps <html> data-* attrs, localStorage
                                    mirror (k2.style/k2.palette/k2.scheme/k2.gaps + per-scheme),
                                    dial custom properties, macOS traffic-light inset (invoke)
src/renderer/index.html             inline pre-paint bootstrap reads the mirror → no flash
src/renderer/stores/settings.ts     daemon persistence (AppSettings.style, typed Rust struct)
StylesSection.tsx                   the picker (reads STYLES from the registry)
KesselConfigProvider (App.tsx)      terminal colors ← activePalette.terminal (registry ints)
```

Key invariants V1 established (violating any of these is a bug, not a choice):
- **Every color/token slot is required**; missing slot = loud failure, never fallback-inheritance.
- **`overrides.css` policy:** every selector scoped under `[data-style="<id>"]`; no `!important`, no `@import`, no `url()`. Comments are exempt from the policy scan.
- **Terminals never frost**: material CSS may target `data-surface` roles `surface|elevated|stripe` only; `bg`/`canvas`/`inset` stay opaque.
- **Terminal palette values must be `#rrggbb`** (Kessel needs integers).
- Ring/shadow slot values are **never the keyword `none`** — they compose into one `box-shadow` list in the `<Surface>` primitive; the no-op is `0 0 #0000`.

---

## 3. UX spec

**Folder:** `~/.k2/styles/<style-id>/` — same layout as `styles/<id>/` in the repo, byte-compatible. (`.k2` is the existing K2 home dir; do not invent a new location.)

**Settings→Styles:** local styles render in the same master list under a `LOCAL` group header, each card badged `Local`. Everything else behaves identically: palettes, schemes, density, dials, hover-preview.

**Load errors:** an invalid local package renders a muted, non-clickable card: name = folder name, caption = the first validation error verbatim (e.g. `palettes/night.json: missing required slot color.bg-hover`). Fix the file → card becomes live (the watcher revalidates). Errors also go to the console with the `[styles]` prefix. Never a dialog, never a toast storm, never a crash.

**Live reload:** while the app runs, changes inside `~/.k2/styles/` re-validate and re-apply within ~300ms. If the *active* style changed, restamp it (same pipeline as switching). If it became invalid, **keep the last-good CSS applied** and show the error state in the picker — do not yank the user to Square mid-edit.

**Deletion:** removing the active style's folder falls back to Square (with mirror + daemon write, so it sticks) and logs why.

**CLI (thin, optional if time allows):** `k2 style list` (bundled + local + validity), `k2 style validate <path>` (run the validator against a folder, print CI-identical output). Follows the gated-CLI conventions of `k2 agent`/`k2 feedback`.

---

## 4. Architecture

### 4.1 Shared resolver module — the load-bearing refactor

`scripts/build-styles.mjs` is today the ONLY implementation of validation + CSS/registry emission. V2 must not fork that logic. Extract it:

```
src/shared/style-compiler/          (plain TS, no DOM, no Node APIs in core)
  validate.ts     REQUIRED_*_SLOTS tables, manifest/tokens/palette/dial validation,
                  overrides.css policy scan  → returns {ok} | {errors: string[]}
                  (library code returns errors; ONLY the CLI wrapper calls process.exit)
  emit.ts         buildCss(pkg): string  ·  buildMeta(pkg): StyleMeta (incl. hexToInt terminal
                  conversion, swatches, gapPresets → [data-gaps] blocks, dial metadata)
  types.ts        the package-shape types (StyleMeta lives here now; styles.generated.ts
                  re-exports it so existing imports don't churn)
```

`build-styles.mjs` becomes a thin Node wrapper (read files → validate → emit → write, plus `--check`/`--watch`). The renderer imports the same module for runtime loading. **Acceptance: after the refactor, `styles.generated.{css,ts}` are byte-identical to before it.**

### 4.2 Runtime loader (renderer)

New `src/renderer/lib/local-styles.ts`:
1. Read `~/.k2/styles/*/` via the Tauri fs plugin (`@tauri-apps/plugin-fs`, already a dependency). Scope: add the styles dir to the fs allowlist in `src-tauri/capabilities/` — grant read-only, exactly this subtree, nothing wider.
2. For each folder: read files → `validate()` → `emit()` → `{meta, css} | {error}`.
3. Inject CSS: ONE `<style id="k2-local-styles">` element, replaced wholesale on every reload (never append-accumulate — see pre-mortem 6.7).
4. Registry: a zustand store `useLocalStylesStore` holding `LocalStyleEntry[] = {meta, css} | {id, error}`. A selector `allStyles()` = bundled `STYLES` + valid local metas. **StylesSection and the style store's resolution switch from importing `STYLES` directly to `allStyles()`** — this is the second load-bearing seam; grep every `STYLES` import (`styles.generated`) and route it through the combined accessor (except tests that intentionally pin the bundled set).
5. Watch: `watchImmediate` from plugin-fs on the styles dir, debounced 250ms → full re-scan (packages are a few KB; don't build incremental logic).

### 4.3 Collision + precedence rules (decide now, not in code review)

- A local style whose `id` collides with a bundled one is **skipped with an error** ("id 'square' is reserved by a bundled style"). No shadowing — shadowing would let a local file silently redefine the default look and break the parity guarantee.
- Local-vs-local duplicate ids: first alphabetically wins, second gets the error card.

### 4.4 Persistence, sync, and fallback

- Selection persistence is unchanged (daemon `AppSettings.style` + localStorage mirror). A local style's id simply appears as the value.
- **Resolution fallback:** `resolveStyleSelection` (in `lib/style-resolve.ts`) currently assumes the style id exists. Add: unknown id → resolve as Square/charcoal, and surface `unknownStyleId` in the resolved result so the picker can show "synced style 'foo' isn't installed on this machine" instead of nothing. DO NOT write the fallback back to the daemon automatically (the style may exist on the user's other machine — clobbering their choice from the machine that lacks it is the bug, not the fix).

### 4.5 Pre-paint cache (kills the relaunch flash)

If the active style is local, its CSS isn't in the bundle, and the `index.html` bootstrap stamps `data-style="mystyle"` against rules that don't exist yet → one frame of unstyled-ish render (`:root` Square fallback), then a pop when the loader injects.

Fix: after every successful local-style application, write the compiled CSS string to `localStorage['k2.localStyleCss.<id>']` (a few KB — fine). The `index.html` bootstrap, *only when* `k2.style` names a non-bundled id (add `k2.styleIsLocal = '1'` to the mirror to avoid embedding the bundled-id list in the bootstrap), reads that key and injects a `<style>` before first paint. The runtime loader later replaces it with freshly-validated CSS. Cache staleness is therefore bounded to one frame-set, not a session.

### 4.6 What explicitly does NOT change

Kessel bridge (meta already carries terminal ints), dials (metadata-driven), traffic-light inset (reads computed `--inset-window`), gap presets, scheme resolution, the daemon's Rust `StyleSettings` (it stores strings; local ids are just strings). If you find yourself editing any of these beyond the `STYLES` → `allStyles()` reroute, stop and re-read this PRD.

---

## 5. Build slices (each lands green: `typecheck:web` 0 · full vitest · `styles:check` · `styles:lint` · parity 9/9)

| # | Slice | Acceptance |
|---|---|---|
| S1 | **Extract `src/shared/style-compiler/`**; `build-styles.mjs` becomes a wrapper | Generated outputs byte-identical before/after (diff them in CI fashion); wrapper keeps `--check`/`--watch` behavior; unit tests move/extend to the library (validation error cases, emission snapshots) |
| S2 | **Loader + injection + combined registry** (no watcher yet; manual load at boot) | Folder in `~/.k2/styles/` appears in picker after relaunch; selecting it styles the whole app incl. terminals; invalid folder shows the error card with the exact validator message |
| S3 | **Watcher + live reload + last-good semantics** | Edit → visible in ≤1s; break the JSON mid-edit → app keeps last-good, card shows error; fix → recovers. Delete active style's folder → falls back to Square, persisted |
| S4 | **Pre-paint cache** | Relaunch with an active local style: zero visible flash (verify with a slowed screen recording, and a Playwright first-frame screenshot if the qa harness can catch it) |
| S5 | **Sync fallback** | Simulate daemon returning an unknown style id: app renders Square + picker notice; daemon value NOT overwritten; installing the missing style and re-fetching settings applies it |
| S6 | **Picker polish + docs** | LOCAL group header, Local badge, error cards; CONTRIBUTING-STYLES.md gains the "use it locally without forking" section; styles/README pointer |
| S7 | *(stretch)* `k2 style list` / `k2 style validate` | CLI output matches validator verbatim; gated like other agent CLIs |

Estimated total: 1–2 focused sessions. S1 is the riskiest (touches the reference implementation); S4 is the fiddliest (pre-paint).

---

## 6. PRE-MORTEM — "it's six months later and V2 caused a mess; what happened?"

Each entry: the failure → the guard. These are ranked by how likely they are to actually happen.

1. **The validator forked.** Someone patched validation in the runtime path (or the build script) without the other; a style passes CI but fails at runtime, or vice versa. → S1's whole point: ONE implementation in `src/shared/style-compiler/`, and a CI test that runs the library against every `styles/` package AND a fixtures dir of deliberately-broken packages, asserting exact error strings. If you ever see `REQUIRED_COLOR_SLOTS` defined twice in the repo, that is the fire alarm.
2. **A malicious/sloppy overrides.css exfiltrates or breaks the app.** CSS can't run JS, but `url()` can beacon to a remote host and creative selectors can restyle *other* styles or the error UI itself. → The policy scan (scoped-selector check, no url/@import/!important) runs at load, same as CI; the scan operates on comment-stripped source. Also: the injected `<style>` element must come BEFORE the app's own late-mounted style elements (CodeMirror's style-mod) in document order so locals can't win specificity wars by position. Add a fixtures test with a hostile package (unscoped selector, `url(//evil)`, `[data-style="square"]` scoping) — all three must be rejected.
3. **First-paint flash regressions.** Someone changes the mirror keys, or the bootstrap, and active-local-style relaunches start flashing again — nobody notices for weeks because everyone tests with bundled styles. → S4's acceptance includes an automated first-frame check; keep `k2.styleIsLocal` in the mirror-writer unit test; the qa-styles spec gains a local-style config.
4. **Pathological styles are slow and users blame K2.** Forty frosted surfaces at blur(40px). → At load, count `backdrop-filter` occurrences in overrides + material.blur value; if over the V1 heuristics (>4 surfaces targeted or blur >20px), still load it but tag the card "may reduce performance" and log specifics. Do NOT hard-reject — it's the user's machine — but never let it be mysterious.
5. **The `STYLES` reroute missed a consumer.** Some component still imports the bundled registry directly, so local styles work everywhere except (say) the Kessel bridge — terminals silently keep Square colors under a local style. → After S2, `grep -rn "from '@/styles.generated'"` — every hit outside the compiler/regeneration machinery and `allStyles()` itself must justify itself in the PR description. Add an integration test: select a local style, assert `KesselConfigProvider` receives its terminal ints.
6. **fs-scope creep.** The Tauri capability granted for `~/.k2/styles/` quietly widens to all of `~/.k2` (which contains tokens/secrets/session state) because someone found the narrow scope inconvenient. → The capability file gets a comment marking it security-reviewed; CI grep asserts the scope string is exactly the styles subtree. Reading anything else via this grant is a rejected PR.
7. **Style-element accumulation / ordering drift.** Live reload appends new `<style>` tags instead of replacing, or replaces by recreating at the END of `<head>`, changing cascade order vs the bundled sheet across a session; symptoms are "styles look different after editing for a while" — hell to debug. → One element, stable id, `textContent` replacement (never remove+append), created once at a deterministic position (immediately after the bundled stylesheet link). Assert element count == 1 in the loader's tests.
8. **Watcher event storms.** Editors write via rename+truncate; a single save can fire 3–6 events; a git checkout inside `~/.k2/styles` fires hundreds. Naive handling = repeated full re-scans, error-card flicker, wasted CPU. → 250ms debounce + coalesce to ONE re-scan; re-scan is idempotent and cheap (KBs). Never re-scan per-event.
9. **Bad UX on validation errors makes authors give up.** "Style invalid" with no line info. → Error strings must carry file + slot path (the V1 validator already formats `<file>: missing required slot color.bg-hover` — preserve that shape through the library refactor; it's part of the API contract, tested in fixtures).
10. **Sync clobber.** The unknown-id fallback writes Square back to the daemon "to fix the mismatch," destroying the user's choice made on their other machine. → §4.4 explicitly forbids it; unit test: unknown id → NO `settingsUpdate` call.
11. **Someone regenerates `styles.generated.ts` with local styles included** (running the compiler against the wrong dir), shipping a user's personal style in a commit. → The build wrapper only ever reads repo `styles/`; the runtime loader only ever reads `~/.k2/styles/`; neither path is configurable. `styles:check` in CI would catch the diff anyway — do not add a flag that lets these cross.
12. **V1 gotchas resurface** (for the junior dev — these each cost real debugging time in V1):
    - Tailwind v4 colors are **oklch**; `/N` opacity compiles to `color-mix(in oklab, …)`. Comparing hand-computed hex against rendered output can be one channel off. The parity harness runs at threshold 0 and WILL catch it.
    - `box-shadow` lists reject the keyword `none` as a member — no-op is `0 0 #0000`.
    - `<Surface>`'s composed `[box-shadow:…]` arbitrary property clobbers Tailwind `ring-*`/`shadow-*` utilities on the same element (CSS order). Don't mix.
    - The `role` prop name is taken by DOM ARIA — Surface uses `role2`.
    - JSDoc in `src/shared/types.ts` claiming the settings store keeps unknown JSON keys is FALSE; `AppSettings` is a typed Rust struct that drops unknowns.
    - The parity/qa harness mock daemon must echo the seeded style back from `/cli/settings/get*` (glob needs the `*` — the real fetch appends `?token=`), or the read-back reverts your test's style mid-run.
    - Playwright/vite tests bind port 5199 — two harness runs can't overlap.
    - `tabs.test.ts` has a pre-existing order-dependent flake (passes in isolation) — don't burn an afternoon on it.

---

## 7. Test plan (summarized; details per-slice above)

- **Unit:** style-compiler fixtures (valid, each-missing-slot, hostile overrides, dial range errors) with exact-string error assertions; loader store (collision, precedence, last-good, deletion); resolve fallback (unknown id, no daemon write).
- **Integration (vitest + jsdom):** combined registry feeds picker + Kessel selector; style-element singleton invariant.
- **QA harness:** add a `local-style` config to `tests/parity/qa-styles.spec.ts` that seeds a fixture package (the spec can write it to a temp HOME) — screenshot matrix + first-frame flash check.
- **Parity:** unchanged 9/9 requirement — local-styles machinery must be invisible when no local styles exist.

## 8. V3 seeds (explicitly deferred)

`k2 style install <git-url>` · palette-only local additions to bundled styles · style sharing via K2 Connect/federation · in-app style editor UI (AIFileEditor precedent) · Windows/Linux path handling if K2 desktop lands there (use the platform config-dir API from day one — don't hardcode `~/.k2` in the loader, use the same home-resolution helper the daemon uses).
