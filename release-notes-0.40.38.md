# K2 0.40.38 — "Make K2 yours: Styles"

The modular Style System: K2's entire visual language — colors, radii, borders/rings,
shadows, glass materials, spacing, motion, terminal colors — is now schema-validated
data, switchable at runtime.

## Styles

- **Settings → Styles** (new page): master-detail picker with style cards, palette
  swatches (including terminal ANSI-16 strips), Light/Dark/**Auto** scheme control
  (Auto follows the OS appearance live), density presets, style dials, and a live
  mini-K2 preview. Hover to preview, click to commit, Esc-safe.
- **Three first-party Styles, each with dark + light palettes:**
  - **Square** — the classic look, rebuilt on the contract (pixel-parity verified by a
    zero-threshold screenshot harness). Palettes: Charcoal, **Paper** (warm paper-and-ink
    light). Density: Compact/Regular/Spacious (floating tiles with draggable seams).
  - **Liquid Glass** *(experimental preview)* — frosted translucent chrome over an
    ambient canvas; terminals stay opaque; reduced-transparency and no-backdrop-filter
    fallbacks. Palettes: Obsidian, **Veil**. Includes a **Frost** dial (blur 0–30px).
  - **Bezel** *(experimental preview)* — the layered-ring aesthetic: hairline → bright
    gap → outer line, keycap controls. Palettes: Graphite, **Porcelain** (the light
    original of the ring technique).
- **Terminals are part of the theme:** every palette carries fg/bg/cursor/selection +
  ANSI-16; live terminals repaint on switch (KesselConfigProvider bridge).
- **macOS traffic lights follow the style:** floating-chrome styles inset the window
  buttons with the UI (new `set_traffic_light_inset` command; re-asserted on resize).
- Style choice persists in daemon settings (`AppSettings.style`, new typed struct) with
  a localStorage mirror for flash-free first paint; cross-window sync via `sync:settings`.

## Under the hood

- **Style contract**: `styles/<id>/` packages (manifest + tokens + palettes + bounded
  `overrides.css`) compiled by `scripts/build-styles.mjs` into generated CSS + a typed
  registry. Every slot required; missing slots fail the build loudly.
- **UI primitives** (`components/ui/`): Surface/Button/Input/Toggle/Callout/Dialog —
  the only layer that spells out shape slots. ~600 hardcoded colors migrated to
  contract slots across parity-gated waves.
- **Style lint ratchet** (`bun run styles:lint`): per-file raw-color budgets that can
  only shrink; CI-enforced.
- **Community styles**: `CONTRIBUTING-STYLES.md` + a `styles` CI workflow (schema
  validation, policy lint, per-style screenshot matrix attached to PRs). Live authoring
  via `bun run styles:watch` + vite HMR.
- **Parity guarantee**: Square/Charcoal is byte-identical to 0.40.37 on all content
  screens (Playwright WebKit harness, threshold 0); Settings screens differ only by the
  new nav item.

## Docs

- `CONTRIBUTING-STYLES.md`, `styles/README.md`, README "Styles" section.
- Future-work PRDs with builder pre-mortems: runtime-loaded styles V2, email server.
