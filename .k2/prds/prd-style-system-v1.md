# PRD — K2 Style System V1 ("Styles")

**Date:** 2026-07-06 · **Owner:** Rosson · **Status:** IN BUILD — P1 started 2026-07-06 on `feat/style-system` (ST6: nothing touches main until Checkpoint A passes)
**Research SSOT:** `.k2/notes/design-system-skins-research.md` (zite forensics, styling audit, architecture, Liquid Glass feasibility — all claims there are cited)

---

## 1. Vision

K2 (a public GitHub repo) gets a **modular Style system**. A Style is a data package — not code — that defines the app's entire visual language: colors, corner radii, border/ring treatment, shadows, glass material, spacing/gaps, motion, and terminal colors. Three first-party Styles ship the vision:

1. **Square** — K2's current look, rebuilt *on* the system (pixel-identical).
2. **Liquid Glass** — Apple-style frosted translucency on the chrome layer.
3. **Bezel** — the zite.com layered-ring aesthetic (stacked shadow rings: dark hairline → bright gap → faint outer line).

Each Style ships its own **palette options** (nested color variants, incl. terminal ANSI-16). Because Styles are schema-validated data, community members can **submit a new Style or palette by PR**; CI screenshots it; we judge the gallery and can promote favorites to first-party.

### 1.1 Decisions locked by Rosson

| # | Decision |
|---|---|
| ST1 | Modular system: Styles are PR-able packages conforming to a versioned contract; palettes nest inside Styles (two contribution tiers). |
| ST2 | **Build order: Square-on-the-contract FIRST, as an explicit STOP-AND-TEST milestone** (Checkpoint A, §6). Only after Rosson verifies K2 looks/feels unchanged does customization work begin. |
| ST3 | **Spacing/gaps are a first-class contract dimension** (Rosson 2026-07-06): Square implicitly assumes flush, everything-touching layout; other Styles need growable gaps between tiles/panes/sections with a canvas layer visible behind. Gap tokens ship in the contract from day one, set to flush/zero for Square. |
| ST4 | Engine ceilings accepted: Glass is "frosted, not refracted" (WebKit has no refraction path); terminals always opaque; behind-window vibrancy deferred to V1.5. |
| ST5 | **Styles get their own discrete Settings page** (Rosson 2026-07-06): a "Styles" item in the Settings nav, not a subsection of an existing page. Selection model is three-level: **Style → palette (theme within the style) → scheme (Light / Dark / Auto)**. The scheme *machinery* (`data-scheme` axis + Auto following the OS appearance signal) ships in V1 even though launch palettes are dark — pieces in place, ready for new themes. |
| ST6 | **All work happens on a long-lived feature branch** (`feat/style-system`), never directly on main, so the concurrent 0.40.31 Linux arc is untouched. Merges to main only at checkpoint boundaries after they pass. |

## 2. Goals / Non-goals

**Goals (V1)**
1. The Style contract (versioned JSON Schema) + all K2 surfaces reading only contract slots.
2. Square as the reference implementation — **zero visual change**, verified.
3. Runtime switching (no restart, no flash), Settings→Appearance picker, per-app persistence via daemon.
4. Liquid Glass + Bezel as first-party Styles proving the contract can express radically different languages.
5. Spacing elasticity: gaps/density adjustable per Style (and demonstrably working on Square as a variant).
6. Contribution pipeline: schema validation + screenshot CI on PRs (even if we don't advertise it loudly yet).

**Non-goals (V1)**
- Light palettes for Glass/Bezel (the `data-scheme` axis + Auto detection ship in V1 per ST5, proven by a Square light palette in P4 — but designing light variants of the other Styles is follow-on work). 
- Behind-window vibrancy / OS transparency (V1.5 — documented Tauri artifacts, research §4).
- A theme marketplace UI in-app (the repo folder + gallery is the marketplace for now).
- Editor (CodeMirror) theme unification — Styles may *suggest* an editor theme; the existing editor-theme system stays independent in V1.
- Per-window style overrides (per-app only in V1).

## 3. The Style contract (the product's core artifact)

A Style = folder under `styles/` (first-party) or `community-styles/`:

```
styles/square/
  style.json            # manifest: name, author, version, schemaVersion,
                        # capabilities {backdrop, gaps, schemes}, screenshot
  tokens.json           # the non-color slots (shape/material/layout/motion)
  palettes/
    charcoal.json       # default palette (colors + terminal ANSI + editor suggestion)
    <more>.json
  overrides.css         # optional, bounded (lint-enforced: no remote assets,
                        # no !important, selector allowlist)
```

**Slot groups** (full schema in-repo; ~70 slots):

| Group | Slots (abridged) | Square | Glass | Bezel |
|---|---|---|---|---|
| Color | ~40 bg/fg-pair slots + accent + status | today's 8, expanded | translucent tints | warm darks |
| Radius | `radius.box / field / selector` | 0 / 0 / 0 | 18 / 11 / full | 12 / 8 / 6 |
| Ring | `ring.surface / field / focus` (multi-layer shadow strings) | 1px hairline | 1px edge-light + sheen | triple ring (hairline/bright gap/outer) |
| Elevation | `shadow.1..5` | none/flat + floats | ambient glow set | geometric 2–8% ramps |
| Material | `material.blur / saturate / tint / tint-opacity` + **`surface.solid`** (mandatory opaque) | n/a (blur 0, gated off) | 18px / 180% / dial | n/a |
| **Layout (ST3)** | `gap.pane / tile / section`, `inset.window`, `divider.width`, `bg.canvas` | **0 / 0 / 0, inset 0, divider 1px** (flush) | 10 / 12 / 16px, inset 10px, divider 0 (floating cards on canvas) | 6 / 8 / 12px, divider 0 (ringed cards) |
| Density | `density.field / selector` (control heights/padding scale) | compact | regular | compact-plus |
| Motion | `motion.duration.* / ease.*` | 100–150ms linear | springy ~300ms | 150ms ease-out |
| Type | `font.ui / display` (optional) | mono (Meslo) | mono default, sans optional | mono default, sans optional |
| Terminal | `ansi.0..15, cursor, selection, seam-fallback` | Tango (today) | per-palette | per-palette |

**The ST3 layout group — what it means structurally.** Today the mosaic panes, tab strips, and sidebar butt against each other with 1px dividers. The contract makes that a *choice*: when `gap.pane > 0`, panes render as rounded, ringed cards floating on `bg.canvas`, dividers disappear, and drag-divider hit-areas widen to the gap. The tiling grid, TabBar, PaneGroupView, and Sidebar read these tokens instead of assuming flush. For Square all values are 0/flush — **which is exactly why Checkpoint A can verify a pixel-identical app while the machinery underneath becomes elastic.** Interaction notes: Kessel's seam-color edge-fill keys off the terminal pane's own background and is unaffected by outer gaps; the browser-pane arc's webview bounds-bridge follows pane rects, so gaps just flow into the rects it already tracks.

## 4. Architecture (summary — full detail in research SSOT §3)

- **Tailwind v4 `@theme inline`** maps semantic vars → utilities; `[data-skin]` + `[data-palette]` blocks in `@layer base` bind slots per Style; attributes stamped on `<html>` by a synchronous inline head script (no first-paint flash); canonical persistence in daemon settings (mirrors the editor-theme plumbing); Tauri event for multi-window; per-skin Tauri window background color (kills white flash).
- Components consume **semantic slots only** — enforced by a lint ratchet (no arbitrary hex/radius/palette utilities), warn→error per directory.
- Glass effects gated by `[data-skin="glass"]` selectors so other Styles pay zero backdrop-filter cost; **`surface.solid` is mandatory in every Style and is the only background allowed on WebGL-hosting panes.**
- Structural needs (glass backdrop layer) via a `<Surface>` primitive reading a capability flag — never per-Style JSX forks (community Styles must be expressible in CSS+tokens alone).
- Terminal bridge: switch pushes the active palette's ANSI set into `KesselColorsConfig` (CSS var → 0xRRGGBB); runtime OSC-4 from TUIs still wins.
- Token files in W3C DTCG 2025.10 format; build step (Style Dictionary v4) emits `skins.generated.css` per (style × palette).

## 5. UI (ST5)

**Settings→Styles** — a discrete page with its own item in the Settings nav (master-detail, same pattern as Settings→Projects):

- **Left (master):** Style cards with preview thumbnails — Square, Liquid Glass, Bezel, plus any community styles present.
- **Right (detail), three levels top-to-bottom:**
  1. **Palette picker** — the selected Style's nested themes as swatch cards.
  2. **Scheme control** — segmented **Light / Dark / Auto**. Auto follows the OS appearance signal (`prefers-color-scheme` + Tauri theme events — on macOS this already tracks the user's day/night auto-appearance, so we get night/day detection for free instead of hand-rolling a clock). A palette declares which schemes it supports (`schemes: ["dark"]` etc.); unsupported options render disabled with a tooltip, so dark-only launch palettes degrade gracefully.
  3. **Style dials** — whatever the Style advertises (Glass: tint-opacity slider; any Style with `capabilities.gaps`: compact/regular/spacious), plus a live preview strip (button/input/card/tab rendered in-place).

Switch applies instantly (no restart, no flash); Esc reverts within the preview flow. Resolved selection = (style, palette, scheme) persisted in daemon settings.

## 6. Build phases & checkpoints (ST2)

| Phase | Contents | Gate |
|---|---|---|
| **P1 — Contract + Square alias** | Schema + token files; globals.css rebuilt on slots; the ~28 hex islands, globals.css literals, and 4 phantom tokens migrated; **layout/gap tokens wired into mosaic/TabBar/PaneGroupView/Sidebar at flush values**; screenshot harness (Playwright, WebKit) capturing the parity matrix | **CHECKPOINT A — STOP AND TEST (Rosson):** K2 on the contract must be visually indistinguishable from today. Automated before/after pixel diff across key screens + Rosson daily-driving it. **No further phase starts until this passes.** |
| **P2 — Primitives** | `components/ui/` (Button, Input, Dialog, Surface, Menu, Tabs, Toast…); convert the 178 components directory-by-directory (subagent worktrees, cherry-picked), parity screenshots per directory | Checkpoint B: parity holds on the high-traffic surfaces (TabBar, Settings, Projects dashboard, terminal chrome) |
| **P3 — Switching + picker** | data-skin/palette/**scheme** stamping, head script, **Auto scheme listener (OS appearance)**, daemon persistence, terminal ANSI bridge, **Settings→Styles page (ST5)** | Square↔Square-variant switch works live |
| **P4 — Square elasticity proof (Rosson's "then customize")** | Ship a second Square palette + a "Square (Spacious)" gap variant exercising ST3 knobs end-to-end; **the second palette is a Square *light* palette, proving the scheme axis + Auto end-to-end** | **CHECKPOINT C (Rosson):** gaps grow/shrink correctly everywhere; light/dark/auto switch correct; feel-test |
| **P5 — Liquid Glass** | Full Style: chrome-layer glass, capsule radii, springy motion, tint dial, reduced-transparency fallback | Checkpoint D: feel-test + perf check alongside busy terminals |
| **P6 — Bezel** | Ring slots + tactile buttons + dark-inversion treatment; `graphite` + `zite-tribute` palettes | Feel-test |
| **P7 — Community pipeline** | Publish schema; CI: validation + policy lint + screenshot-matrix bot on PRs; CONTRIBUTING-STYLES.md | Dry-run: submit a test community palette via PR |
| **P8 — Docs/release** | WHATS_NEW, glossary, README gallery | Release |

P1 is deliberately the largest *de-risking* step and the smallest *visible* step: everything after it is customization on safe rails. Standing build discipline applies (daemon-first where applicable, subagent worktrees cherry-picked, no prod reloads, release.sh, signed-bundle launch tests).

**Execution reorder (2026-07-07, Rosson's compressed test window):** P3 (switching + Settings→Styles) and P4 (Paper light palette + gap presets) build BEFORE the bulk of P2, because they're what Rosson tests and they ride the existing var layer (~95% color-tokenized) without needing primitives. P2 proceeds in parallel/afterward as subagent waves: first a hex-in-TSX sweep (light-palette readiness), then primitives + directory conversion. Glass/Bezel (P5/P6) still require P2. Checkpoints A/C collapse into one combined Rosson session: daily-drive parity + switching + elasticity + light, all at once.

**Branch strategy (ST6):** everything lands on `feat/style-system`, branched from main at kickoff. Subagent worktree commits cherry-pick onto *the branch*, not main. The branch rebases on main periodically to stay current with the 0.40.31 Linux arc, and merges to main only after a checkpoint passes (earliest: Checkpoint A).

## 7. Risks

| Risk | Mitigation |
|---|---|
| Parity drift in P1/P2 (the "did we miss a corner" risk) | Alias-by-construction + automated pixel diffs + per-directory conversion + Checkpoint A/B human gates |
| Glass perf next to WebGL terminals | `surface.solid` rule (glass never over canvases), ≤4 glass surfaces, blur ≤20px, WebKit screenshot/perf checks in P5 |
| Gap tokens destabilize drag/divider/drop-zone math | Gap-aware hit-areas land in P1 at zero (no behavior change), exercised in P4 before any Style depends on them |
| Community CSS safety | Bounded overrides + Obsidian policies (no remote assets/!important) + selector allowlist lint + schema CI |
| Lint tooling maturity on Tailwind v4 | Evaluate oxlint-tailwindcss vs custom ESLint denylist at P1; the rule set is small either way |

## 8. Constraints from the concurrent 0.40.31 arc (platform team, 2026-07-07) — READ BEFORE P1/P2 CONVERSION WORK

The 0.40.31 "K2 on Linux" arc is churning main while this branch lives. Four hard constraints:

1. **Deprecated-surface exclusion list.** The **Review Queue** and **Agent Ops** surfaces (hidden since cd636df) are being **deleted from main in 0.40.31** — components, routes, TopBar buttons, dead CSS, and the state threaded through App.tsx. Do NOT migrate, tokenize, or screenshot these components in P1/P2; exclude them from the 178-component conversion count and the parity matrix. Their deep-links (Feedback page, ProjectChat) are being re-pointed to the ⌘J switcher — if you touch those files, preserve the ⌘J wiring, not the old navigation.
2. **TS-cleanup sequencing.** Main currently carries ~66 pre-existing TS errors in ~22 renderer files. 0.40.31 drives that to **zero** and then adds a `typecheck:web` CI gate (plus Rust `-D warnings`). Consequences for this branch: (a) after the gate lands, `feat/style-system` cannot merge unless it typechecks clean — rebase promptly once warning-zero lands on main; (b) until then, introduce **no new** TS errors (capture a baseline diff per directory you convert, same discipline as the deletion crews); (c) don't "fix" pre-existing errors in files you convert unless trivial — the warning-zero crew is sweeping them centrally and parallel fixes create rebase conflicts.
3. **webkitgtk (Linux) blur fallback is now a launch requirement, not a nice-to-have.** 0.40.31 ships K2 Desktop on Linux (webkitgtk). `backdrop-filter` support/perf on webkitgtk is materially worse than macOS WKWebView. Liquid Glass must therefore ship with a **capability-detected reduced-transparency fallback** (solid `surface.solid` rendering when blur is unavailable or expensive) — detect at runtime, don't OS-sniff. Note the Playwright **WebKit** screenshot harness is *not* webkitgtk; add a Linux (webkitgtk) render smoke to the P5 gate before calling Glass done.
4. **Rebase cadence (ST6 addendum).** Rebase `feat/style-system` on main at minimum: (a) right after the Review Queue/Agent Ops deletion lands, (b) right after TS-zero + CI gates land, and (c) immediately before running Checkpoint A pixel diffs — the parity baseline must be *current main*, or the diff proves nothing.

## 9. Open decisions — SETTLED at P1 kickoff (2026-07-06, per recommendations; Rosson may override)

1. User-facing name: **Styles**; third style ships as **Bezel** (rename is a one-line manifest change if a better name lands).
2. Primitives: **hand-rolled**, shadcn-inspired (no scaffolding dependency; K2's components are too bespoke for drop-in shadcn).
3. `font.ui` as a Style dimension: **yes**, mono (Meslo) default — Square keeps its identity.
4. Style choice **syncs via daemon settings** (canonical), with localStorage mirror for the pre-connect first paint.
5. Community PRs: **accepted after P6 stabilizes**; contract + CI still built in P7 so the door opens the moment we're ready.
