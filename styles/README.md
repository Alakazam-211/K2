# K2 Styles

A **Style** is a data package that defines K2's entire visual language — colors, corner radii, border/ring treatment, shadows, materials, spacing/gaps, motion, fonts, and terminal colors. The app's components read only the CSS custom properties these packages emit, so a new Style changes how K2 looks without touching a single component.

## Layout

```
styles/
  style.schema.json        # the contract (JSON Schema) — all packages validate against it
  <id>/
    style.json             # manifest: name, version, capabilities, default palette
    tokens.json            # non-color slots: radius, gap, ring, shadow, material, motion, font…
    palettes/
      <palette-id>.json    # color + terminal slots; a Style ships one or more palettes
```

## Build

```
bun run styles:build   # regenerates src/renderer/styles.generated.css — commit the result
bun run styles:check   # fails if the committed output is stale (CI)
```

The generated file defines `:root` fallbacks (the default Style) plus one block per
`[data-style]` / `[data-style][data-palette]` combination. `<html>` is stamped with
`data-style` / `data-palette` / `data-scheme` before first paint by an inline script
in `src/renderer/index.html`.

Every slot in the contract is **required** — the build fails loudly on a missing slot
so a Style can never silently inherit half of another Style's look.

### Token names

A slot `group.key` emits `--group-key`. Tailwind v4 owns `--text-*` and `--font-mono`, so a
slot must never emit those: the type scale is `type.*` (`--type-xs`), and the code font is
`font.code` (`--font-code`). `density.scale` multiplies Tailwind's `--spacing`, so every
spacing utility follows it. Inside a Garden, `ZEN_STYLE_SHIELD` must map every new slot
(`zen-style-shield.test.ts` fails otherwise).

## Lint ratchet

`bun run styles:lint` counts colors and shapes (raw `rounded-*`, `shadow-*`, `backdrop-blur*`,
inline shape literals) that bypass the tokens, per file, against the two maps in
`lint-baseline.json`. Comments and `components/Zen/` aren't counted. A file may only go down;
after a cleanup, run `node scripts/check-style-lint.mjs --update-baseline`.

## Contributing a Style or palette

Community contributions arrive as PRs adding a folder here (or a single palette file
inside an existing Style). Full policy, CI screenshot pipeline, and review criteria
land with the community pipeline phase — see `.k2/prds/prd-style-system-v1.md` (P7).
Until then, the contract may still change without notice.
