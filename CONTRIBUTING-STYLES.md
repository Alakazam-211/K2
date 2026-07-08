# Contributing a Style to K2

K2's entire visual language — colors, corner radii, borders/rings, shadows, glass
materials, spacing, motion, and terminal colors — is driven by **Style packages**:
schema-validated data folders under [`styles/`](styles/). The app ships Square
(the original look), Liquid Glass, and Bezel; community Styles arrive as pull
requests adding a folder here. No component code required, and none accepted —
if a look can't be expressed through the contract, open an issue about extending
the contract instead.

## Two ways to contribute

1. **A palette for an existing Style** (~70 lines of JSON): add
   `styles/<style>/palettes/<your-palette>.json`. Every color slot is required —
   copy an existing palette and re-color it. Include the 16 terminal ANSI colors;
   they're half of what makes a K2 palette feel intentional.
2. **A whole new Style**: add `styles/<your-style>/` with `style.json` (manifest),
   `tokens.json` (shape/material/layout/motion), at least one palette, and — only
   if tokens genuinely can't express something — a bounded `overrides.css`.

Read [`styles/README.md`](styles/README.md) for the folder anatomy and
[`styles/style.schema.json`](styles/style.schema.json) for every slot's meaning.

## The rules the build enforces (locally: `bun run styles:build`)

- **Every slot is required.** A missing slot is a hard build failure, never a
  silent fallback — a Style can't half-inherit another Style's look.
- **`overrides.css` is bounded**: every selector scoped under
  `[data-style="<your-id>"]`, no `!important`, no `@import`, no `url()` (no
  remote assets, ever).
- **Terminals stay opaque.** The `bg` color slot must be a solid color; translucent
  materials may only touch chrome surfaces (`data-surface` roles `surface` /
  `elevated` / `stripe`), and reduced-transparency fallbacks are expected.
- **Gap discipline**: if your rings/shadows render outside the element
  (spread rings), set `gap.*` large enough to clear them, and keep
  `divider.width ≈ 2 × gap.tile` so pane-divider dragging still works.

## Author it live

Run the app from source (`bun run dev`) with `bun run styles:watch` in a second
terminal: every save of a style file regenerates the outputs and hot-reloads
into the running app — your style shows up as a card in Settings → Styles, with
live palette hover-previews, while you edit its JSON.

## Submitting

1. `bun run styles:build` — regenerates `src/renderer/styles.generated.{css,ts}`;
   commit the regenerated files with your package.
2. `bun run typecheck:web && bun run styles:lint` — both must pass.
3. Screenshot what you made (the Settings→Styles preview is a good subject) and
   put it in the PR description. CI also captures a screen matrix per style
   configuration and attaches it to the PR as an artifact.
4. One Style or palette per PR.

Squares stays pixel-frozen — PRs that alter Square/Charcoal values will be
declined (that's the app's reference rendering, verified by a zero-threshold
screenshot diff).

Maintainers review for visual coherence, contrast/readability (both schemes if
you claim both), terminal color quality, and policy compliance. Styles we love
may be promoted to first-party.
