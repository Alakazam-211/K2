# K2 Style System ("Skins") — Research Report (SSOT)

**Date:** 2026-07-06 · **Status:** Research complete, nothing built. PRD to follow on Rosson's go.
**Method:** 4 parallel research agents — zite.com CSS forensics, K2 renderer styling audit, multi-skin architecture + community-theme ecosystems, Liquid Glass web implementation.
**Vision (Rosson):** K2 (public GH repo) gets a **modular Style system** with 3 first-party styles — **Square** (current), **Liquid Glass**, and the **zite.com layered-ring style** — where community members can PR a new Style, and each Style ships its own **palette options** as sub-variants.

---

## 1. The zite.com answer (Rosson's direct question)

**What zite.com is:** the AI no-code business-app builder from the Fillout team (renamed April 2025). Next.js/Tailwind; their full compiled CSS was statically fetchable — the analysis below quotes their real shipped rules.

**The "double outline with a tiny brighter gap" is stacked zero-blur `box-shadow` spread rings — no `border`, no `outline`.** The site uses effectively ZERO border utilities; every edge on the page is a shadow ring. The flagship class (hero screenshots + white cards):

```css
.shadow-marketing-outline { --tw-shadow:
  0 12px 12px -6px #00000005, 0 6px 6px -3px #00000005,   /* soft elevation ramp */
  0 3px 3px -1.5px #00000005, 0 1px 1px -.5px #0000000a,
  0 0 0 1px #0000000f,   /* inner hairline — 1px black @6%  */
  0 0 0 3px #fffc,       /* THE GAP — 2px ring of white @80% (the brighter line) */
  0 0 0 4px #0000000d; } /* outer hairline — 1px black @5%  */
```

So: **dark 1px line → bright 2px white "mat" → faint 1px outer line**, all following the border-radius (which classic `outline` can't). On their dark section it inverts — bright hairlines, gap painted in the section background, plus a brand-yellow glow ring (`0 0 0 4px #212124, 0 0 0 5px #ffc70014`). Focus states swap the stack (darker hairline + soft 3px halo — the Stripe pattern).

**Name of the aesthetic:** the **layered-ring / bezel style** — the current premium-SaaS refinement of flat design (photo-mat borders + tactile keycap buttons). Same family: **Vercel/Geist** (border-first hairlines as shadow tokens), **Stripe** (the `0 0 0 1px` + halo focus ring), **Attio** (live-verified same outer-halo card rings), Tailwind's own `ring`/`ring-offset` utilities institutionalize the gap-ring idea. NOT neobrutalism, not Teenage-Engineering skeuomorphism — no textures, no hard offset shadows.

**Full recipe (7 patterns), palette (#FFC700 accent, warm grays, Inter + BL Melody), tactile button treatment, and dark-inversion rules** are in the agent report; the distilled CSS recipe is reproduced in §6.3. Vibe in one line: *paper-white surfaces machined rather than drawn, every panel sitting in a lit bezel, buttons like backlit keycaps.*

**The K2-relevant insight:** this entire style is expressible as **one multi-layer shadow token per surface role**. That's the cleanest possible proof the skin contract needs a layered-shadow slot — flat skins set it to a plain hairline, the bezel skin sets the triple ring, glass sets glow — zero component changes.

## 2. Where K2 is today (audit)

**Favorable:** Tailwind v4 CSS-first; ONE global stylesheet (`src/renderer/globals.css`); the app is **~95% color-tokenized already** — 8 semantic `--color-*` vars in `:root` funnel ~2,900 component class references (30:1 semantic vs hardcoded). A color-only reskin is nearly free today. The CodeMirror editor already has a working user-theme system (JSON in `~/.k2so/themes/`, daemon routes `themes/list|create-template|delete`) — plumbing precedent for the skin store.

**The five real obstacles:**
1. **Shape is not tokenized at all** — the Square style is literal `border-radius: 0` + `border`/`shadow-*` utility strings across 178 component files + globals.css. No `--radius`, `--border-width`, `--blur`, `--shadow-*` tokens exist.
2. **No shared UI-primitives layer** — no `components/ui/`, no shadcn/Radix/headless libs; every button/dialog/input is copy-pasted Tailwind strings. The single largest structural blocker; skins need primitives (or at least token-funneled class recipes) to have one place to express shape.
3. **Kessel terminal is a parallel color universe** — integer `0xRRGGBB` `KesselColorsConfig` (Tango ANSI defaults), painter theme, and the iTerm-style **seam-color edge-fill** that paints pane remainders with the TUI's majority bg — a translucent terminal bg would fight it. Terminal theming needs a CSS-var ↔ 0xRRGGBB bridge and terminal ANSI must be part of each palette.
4. **Liquid glass needs Rust work** — window is `transparent: false`, no vibrancy crate; `macOSPrivateApi` is on only for the overlay titlebar. Real behind-window glass = `transparent: true` + `window-vibrancy` + translucent body/mosaic backgrounds.
5. **Hardcoded islands** — ~28 arbitrary-hex component uses, a swath of literal hex in globals.css (markdown/diff/status dots/mosaic/scrollbars), and **4 phantom tokens referenced but never defined** (`--color-bg-hover`, `--color-accent-hover`, `--color-bg-primary`, `--color-bg-secondary`) silently resolving to nothing (fix regardless).

Also relevant: entire UI body is **monospace** (MesloLGM Nerd Font) — a Style could legitimately introduce a sans UI face as part of its identity (zite style wants Inter-like; glass wants SF-adjacent). Icons are hand-authored inline SVGs (no library) — they inherit `currentColor`, so they theme for free.

## 3. Recommended architecture (the modular Style system)

### 3.1 Concept (confirmed with Rosson 2026-07-06)
- **Style (skin)** = a folder conforming to a **versioned contract** (published JSON Schema, Zed-style). It owns everything expensive: shape language, materials, elevation, density, motion, structural capabilities.
- **Palettes nest inside the Style** — each Style ships ≥1 palette variant re-binding only the color slots (incl. terminal ANSI-16). Two contribution tiers: full Style PR (high bar) or palette-only PR into an existing Style (30-line file, where most community energy goes — Warp/Alacritty precedent).
- Community styles land in a registry folder via PR; loved ones get **promoted to first-party at zero cost** because first-party styles use the identical contract from day one.

### 3.2 Token tiers
1. **Primitives** (`.tokens.json`, W3C DTCG 2025.10 format — first stable version shipped Oct 2025): oklch color ramps, dimension/duration/easing scales, shadow layers. Note: DTCG theming lives in the separate Resolver Module which is still a draft — use the format now, treat resolvers as directional.
2. **Semantic contract (THE Style API):** ~40 color slots (bg/fg-pair naming) + the non-color slots the three styles actually differ in:
   - `radius.box | field | selector` (daisyUI's role-scoped model — better than one knob: Square wants 0 everywhere; bezel wants 8/12px; glass wants capsule)
   - `border.width`, `ring` (**multi-layer shadow slot** — Square: hairline; zite: triple ring; glass: edge-glow), `ring.focus`
   - `shadow.1..5` (elevation sets), `material.blur | saturate | surface-opacity | tint` (glass), **`surface.solid` (MANDATORY opaque token — anything hosting WebGL/terminals always uses it)**
   - `density.field | selector` (Radix `scaling` precedent), `motion.duration.* | ease.*` (glass = springy ~300ms; square/bezel = 100–150ms)
   - `font.ui | display` (optional per-style face), terminal `ansi.0..15 | cursor | selection | seam-fallback`
3. **Component tokens** only where a component must deviate.

### 3.3 Tailwind v4 mechanics
The canonical multi-theme pattern (shadcn's v4 recipe + Simon Vrachliotis):
```css
@theme inline {                    /* utilities read the indirection var */
  --color-surface: var(--surface);
  --radius-field: var(--skin-radius-field);
  --shadow-ring: var(--skin-ring);
}
:root, [data-skin="square"][data-palette="charcoal"] { --surface:#141414; --skin-radius-field:0px; --skin-ring:0 0 0 1px var(--border); }
[data-skin="bezel"][data-palette="paper"]            { --skin-radius-field:8px; --skin-ring:0 0 0 1px rgb(0 0 0/.06), 0 0 0 3px rgb(255 255 255/.8), 0 0 0 4px rgb(0 0 0/.05); }
```
- **`@theme inline` for anything skin-switched** (plain `@theme` emits resolved values into `:root` and the override loses); `[data-skin]` blocks in `@layer base`; two orthogonal attributes `data-skin` + `data-palette` (+ `data-scheme` reserved for light/dark within a palette).
- Glass surfaces gated by `[data-skin="glass"] .surface { backdrop-filter: ... }` so other skins pay zero backdrop-root cost (a `blur(0)` filter still forces the render pass — never ship it unconditionally).
- 90% of skin expression = CSS + the two pseudo-element "free layers"; the rare structural need (glass backdrop slot in window chrome/panels) via a `<Surface>` primitive reading a `SkinProvider` capability flag. **Never per-skin JSX forks** — community skins must be expressible in CSS+tokens alone.

### 3.4 Switching + persistence
- `data-skin`/`data-palette` stamped on `<html>` by a synchronous inline head script reading a localStorage mirror (no first-paint flash — Tauri controls index.html, so this is airtight); canonical persistence in daemon settings (mirror the editor-theme plumbing); Tauri event broadcast for multi-window. Set the Tauri window background color per-skin to kill white-flash at window creation.
- Root var swap = one-off whole-tree recalc — fine for explicit switches; keep `calc()` chains off switched vars shallow.
- Scheme: **dark-only V1 per style is acceptable** (Warp launched dark-only) but `data-scheme` is in the contract from day one; don't trust `prefers-color-scheme` in Tauri (documented propagation bugs) — derive from the window theme API when light variants arrive.
- Terminal bridge: on skin/palette switch, push the palette's ANSI set into `KesselColorsConfig` programmatically (CSS vars → 0xRRGGBB); OSC-4 runtime overrides from TUIs still win (existing behavior).

### 3.5 Contribution contract (phase 2, designed now)
Community Style = folder: `skin.json` manifest (name, author, semver, schemaVersion, `capabilities: {backdrop, schemes}`, screenshot) + palette token files + optional **bounded** `overrides.css`. CI on PR: JSON-Schema validation, all-required-slots check, banned-CSS lint (**no remote assets, no `!important`, selector allowlist** — Obsidian's policies verbatim), and an auto-generated **Playwright screenshot matrix (style × palette × key screens) rendered on WebKit** posted to the PR as the review gallery. Review = judge the gallery, not audit CSS.

## 4. Liquid Glass — feasibility verdict

**Feasible with a "frosted, not refracted" ceiling.** What defines the real material (Apple WWDC25): lensing/refraction at edges, adaptive tint sampling, gel morphing, and the layering law — **glass lives only on the floating chrome layer, never the content layer, never glass-on-glass**.

- **Works in WKWebView:** `backdrop-filter: blur() saturate() brightness()` + gradient tint + 1px inner light ring + layered inset sheen shadows — reads convincingly as "regular" glass.
- **Does NOT work in WKWebView:** SVG displacement-map refraction as backdrop-filter (Chromium-only). Skip it or fake edges with a static inner band. (Every liquid-glass React lib says the same: displacement invisible in Safari.)
- **The K2 rule that falls out of Apple's own layering law:** terminals are the content layer → **stay opaque (`surface.solid`)** — which conveniently avoids the pathological case (backdrop blur over a constantly-repainting WebGL canvas re-runs the blur every canvas frame; documented Safari perf cliff). Glass goes on tab bars, sidebars, palettes, modals. Cap ~4 concurrent glass surfaces, blur ≤ ~20px, `isolation`/`contain`, and beware filter ancestors breaking `position: fixed`.
- **Behind-window glass:** CSS can never sample the desktop (documented Tauri limitation). Real vibrancy = `transparent: true` + `tauri-apps/window-vibrancy` (`UnderWindowBackground`/`HudWindow`/`Sidebar` materials) with `html, body { background: transparent }`. A `tauri-plugin-liquid-glass` exists wrapping macOS 26's native NSGlassEffectView but it's **private API** — fallback-only, note App Store risk. Also: documented Tauri artifacts combining window transparency with CSS backdrop-filter (#6876, #12804) — **the glass skin should be in-app glass over app layers first; window vibrancy is a separate, later, opt-in enhancement.**
- **Tint opacity is the master dial** (Apple shipped→retracted→re-tuned transparency across iOS 26 betas, reportedly ending at a user slider) — expose it as a per-user setting within the glass style.
- **Accessibility:** `prefers-reduced-transparency` → opaque fallback set (contract capability), reduce-motion kills morph springs.

## 5. Migration plan (proven pattern, low risk)

1. **Alias phase** — define the full semantic contract with values bound to today's exact rendering (Square style = the contract's first implementation). Visual no-op, verified by screenshots. Fix the 4 phantom tokens + ~28 hex islands + globals.css literals as part of this.
2. **Primitives pass** — introduce `components/ui/` (Button, Input, Dialog, Surface, Menu, Tabs…) absorbing the copy-pasted class strings; highest-traffic components first. This is the big lift (178 files) but each conversion is mechanical.
3. **Codemod-by-mapping-table** (`bg-[#1a1a1a]` → `bg-surface`, `rounded-*`→role tokens) — no off-the-shelf tool; scripted find/replace + human review.
4. **Lint ratchet** — ban arbitrary values/raw palette utilities (eslint-plugin-tailwindcss `no-arbitrary-value` has rocky v4 support; evaluate oxlint-tailwindcss or a small custom denylist rule — MetaMask/Atlassian ship exactly this). Warn → error per directory.
5. **Build Liquid Glass second** as the contract stress test (exercises material + structure + motion at once), **bezel third** (exercises the ring slots — cheap once the shadow-slot exists).
6. **Visual regression:** Playwright screenshot matrix over story/screen IDs × skin × palette, **on WebKit** (backdrop-filter renders differently per engine). Chromatic Modes if we ever adopt Storybook.

## 6. The three first-party styles (spec sketches)

### 6.1 Square (current, default)
Radius 0 everywhere; 1px `--color-border` hairlines; shadows only on floating surfaces; compact density; monospace UI; today's 8-color charcoal palette as `charcoal`, room for community palettes immediately. Zero visual change from today — it IS the alias phase.

### 6.2 Liquid Glass
Chrome layer (tab bars, sidebar, palettes, modals, status) = regular-glass recipe: `backdrop-filter: blur(18px) saturate(180%)` + dark tint `rgba(20,20,25,.35)` + 1px edge highlight `rgba(255,255,255,.18)` + sheen insets + soft ambient shadow. Content layer (terminals, editors, file trees' scroll bodies) = `surface.solid`. Capsule radii (box 16–20px, field 10–12px), springy motion tokens, tint-opacity user dial, reduced-transparency fallback. Phase-2 enhancement: window vibrancy behind it all.

### 6.3 Bezel (the zite-style — working name)
Dark-first adaptation of the layered-ring recipe (zite is light-first; K2 is a dark app — use their **dark-section inversion** as the primary treatment):
```css
--skin-ring:            /* panels/cards */
  0 -1px 0 0 rgb(255 255 255 / .06),   /* top edge light */
  0 0 0 1px rgb(255 255 255 / .08),    /* inner hairline */
  0 2px 4px rgb(0 0 0 / .32),          /* drop */
  0 0 0 3px var(--surface-deep),       /* the GAP — painted in bg */
  0 0 0 4px rgb(255 255 255 / .08);    /* outer hairline */
--skin-ring-focus: 0 0 0 1px var(--accent), 0 1px 3px rgb(0 0 0/.4), 0 0 0 3px color-mix(in oklch, var(--accent) 25%, transparent);
```
Radius 8px fields / 12px boxes; tactile accent buttons (inset top-glow + dark inner ring + bottom shade on hover); geometric elevation ramps at 2–8% opacity; optional accent glow ring on active surfaces (their amber trick, in K2's accent color). Palettes: `graphite` (K2 charcoal + blue accent), plus a `zite-tribute` (warm grays + `#FFC700`) as the demonstration second palette.

## 7. Open decisions (for the PRD)

1. **Naming**: "Styles" (user-facing) vs "Skins" (internal)? Third style's name ("Bezel"? "Machined"? "Frame")?
2. **Primitives strategy**: hand-rolled `components/ui/` (matches hand-rolled codebase) vs adopting shadcn structure as scaffolding (we own the code either way). Recommend hand-rolled, shadcn-inspired naming.
3. **Persistence scope**: per-app (recommended V1) vs per-window; does skin choice sync across a user's clients via daemon settings (probably yes — it's already where settings live)?
4. **UI font as a style dimension**: allow styles to swap the monospace UI body font? (Bezel/glass arguably want a sans; Square keeps mono identity.) Recommendation: yes, `font.ui` slot with mono default.
5. **Window vibrancy** (Rust, `transparent: true`) in glass V1 or V1.5? Recommend V1.5 — documented Tauri transparency+backdrop-filter artifacts make in-app glass the safe first ship.
6. **Community registry timing**: contract + CI from day one (cheap), accept external PRs from day one or after the three first-party styles stabilize?
7. **Editor + terminal theme unification**: fold the existing CodeMirror theme system into the Style/palette contract, or keep it independent (a Style could *suggest* an editor theme)?

## 8. Key sources

Zite forensics: [zite.com](https://www.zite.com) + compiled CSS (fetched; local copies in scratchpad) · [Fillout→Zite rename](https://www.fillout.com/blog/the-next-chapter) · [CSS-Tricks Stacked Borders](https://css-tricks.com/stacked-borders/) · [shadcn.io Vercel/Geist](https://www.shadcn.io/design/vercel)
Architecture: [DTCG 2025.10 spec](https://www.designtokens.org/tr/2025.10/format/) · [daisyUI themes](https://daisyui.com/docs/themes/) · [shadcn theming](https://ui.shadcn.com/docs/theming) + [tailwind-v4 recipe](https://ui.shadcn.com/docs/tailwind-v4) · [Radix Themes overview](https://www.radix-ui.com/themes/docs/theme/overview) · [Tailwind v4 multi-theme (simonswiss)](https://simonswiss.com/posts/tailwind-v4-multi-theme/) · [Obsidian theme guidelines](https://docs.obsidian.md/Themes/App+themes/Theme+guidelines) · [Zed themes](https://zed.dev/docs/extensions/themes) · [Warp themes repo](https://github.com/warpdotdev/themes) · [Style Dictionary](https://styledictionary.com)
Liquid Glass: [WWDC25 #219](https://developer.apple.com/videos/play/wwdc2025/219/) · [HIG Materials](https://developer.apple.com/design/human-interface-guidelines/materials) · [kube.io Liquid Glass CSS/SVG](https://kube.io/blog/liquid-glass-css-svg/) · [kevinbism pure-CSS](https://github.com/kevinbism/liquid-glass-effect) · [rdev/liquid-glass-react](https://github.com/rdev/liquid-glass-react) · [window-vibrancy](https://github.com/tauri-apps/window-vibrancy) · [tauri-plugin-liquid-glass](https://github.com/hkandala/tauri-plugin-liquid-glass) · [Josh Comeau backdrop-filter](https://www.joshwcomeau.com/css/backdrop-filter/) · Tauri issues [#2827](https://github.com/tauri-apps/tauri/issues/2827), [#6876](https://github.com/tauri-apps/tauri/issues/6876)
Repo anchors: `src/renderer/globals.css` (8 tokens, :24-33) · `kessel/config.ts:52-79,164-179` (terminal colors) · `kessel-term/seamColor.ts` · `lib/editor-themes.ts` + `stores/custom-themes.ts` (theme plumbing precedent) · `src-tauri/tauri.conf.json:12-26` (transparent: false)

**Unconfirmed items:** WWDC-2026 transparency slider (single source); community glass budget numbers (≤4 layers / ≤20px blur — heuristics, not Apple-official); `prefers-reduced-transparency` current Safari status; eslint-plugin-tailwindcss v4 maturity (evaluate oxlint-tailwindcss at build time); Linear/Vercel ring claims rest on documented design systems (Attio live-verified).
