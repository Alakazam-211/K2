#!/usr/bin/env node
// Compiles Style packages (styles/<id>/) into src/renderer/styles.generated.css.
//
//   node scripts/build-styles.mjs          # build
//   node scripts/build-styles.mjs --check  # verify the committed output is current (CI)
//
// Emits, in order:
//   :root                                → the default style's default palette (pre-stamp fallback)
//   [data-style="<id>"]                  → the style's non-color tokens
//   [data-style="<id>"][data-palette=…]  → each palette's color + terminal slots
//
// Every slot below is REQUIRED in every style/palette — a missing slot is a
// hard build failure, never a silent fallback.

import { readdirSync, readFileSync, writeFileSync, statSync } from 'node:fs'
import { join, dirname, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const STYLES_DIR = join(ROOT, 'styles')
const OUT_FILE = join(ROOT, 'src', 'renderer', 'styles.generated.css')
const DEFAULT_STYLE = 'square'

const REQUIRED_TOKEN_SLOTS = {
  radius: ['box', 'field', 'selector'],
  gap: ['pane', 'tile', 'section'],
  inset: ['window'],
  divider: ['width'],
  ring: ['surface', 'field', 'focus'],
  shadow: ['1', '2', '3', '4', '5'],
  material: ['blur', 'saturate', 'tint', 'tint-opacity'],
  density: ['scale'],
  motion: ['duration-fast', 'duration-medium', 'duration-slow', 'ease-standard'],
  font: ['ui', 'display'],
}

const REQUIRED_COLOR_SLOTS = [
  'bg', 'bg-surface', 'bg-elevated', 'bg-inset', 'bg-stripe', 'bg-canvas', 'bg-hover',
  'border', 'border-strong',
  'text-primary', 'text-secondary', 'text-muted',
  'accent', 'accent-hover', 'accent-soft', 'on-accent',
  'status-working', 'status-error', 'status-ok', 'status-warn',
  'diff-add-text', 'diff-remove-text', 'diff-modified-border',
  'code-inline', 'danger-hover',
  'overlay-soft-bg', 'overlay-soft-border',
  'scrollbar-thumb', 'scrollbar-thumb-hover', 'scrollbar-thumb-strong', 'scrollbar-thumb-strong-hover',
]

const REQUIRED_TERMINAL_SLOTS = ['fg', 'bg', 'cursor', 'selection', 'ansi']

function fail(msg) {
  console.error(`build-styles: ERROR: ${msg}`)
  process.exit(1)
}

function readJson(path) {
  let raw
  try {
    raw = readFileSync(path, 'utf8')
  } catch (e) {
    fail(`cannot read ${relative(ROOT, path)}: ${e.message}`)
  }
  try {
    return JSON.parse(raw)
  } catch (e) {
    fail(`${relative(ROOT, path)} is not valid JSON: ${e.message}`)
  }
}

function tokenValue(file, group, key, token) {
  if (token === undefined) fail(`${file}: missing required slot ${group}.${key}`)
  if (typeof token !== 'object' || token.$value === undefined)
    fail(`${file}: slot ${group}.${key} must be a { "$value": ... } token`)
  return token.$value
}

function emitDecl(lines, name, value) {
  lines.push(`  ${name}: ${value};`)
}

function buildTokenDecls(file, tokens) {
  const lines = []
  for (const [group, keys] of Object.entries(REQUIRED_TOKEN_SLOTS)) {
    if (!tokens[group]) fail(`${file}: missing required token group "${group}"`)
    for (const key of keys) {
      emitDecl(lines, `--${group}-${key}`, tokenValue(file, group, key, tokens[group][key]))
    }
    for (const extra of Object.keys(tokens[group])) {
      if (extra.startsWith('$')) continue
      if (!keys.includes(extra)) emitDecl(lines, `--${group}-${extra}`, tokenValue(file, group, extra, tokens[group][extra]))
    }
  }
  return lines
}

function buildPaletteDecls(file, palette) {
  const lines = []
  if (!palette.color) fail(`${file}: missing "color" group`)
  for (const key of REQUIRED_COLOR_SLOTS) {
    emitDecl(lines, `--color-${key}`, tokenValue(file, 'color', key, palette.color[key]))
  }
  for (const extra of Object.keys(palette.color)) {
    if (extra.startsWith('$')) continue
    if (!REQUIRED_COLOR_SLOTS.includes(extra)) emitDecl(lines, `--color-${extra}`, tokenValue(file, 'color', extra, palette.color[extra]))
  }
  if (!palette.terminal) fail(`${file}: missing "terminal" group`)
  for (const key of REQUIRED_TERMINAL_SLOTS) {
    if (palette.terminal[key] === undefined) fail(`${file}: missing required slot terminal.${key}`)
  }
  emitDecl(lines, '--term-fg', palette.terminal.fg.$value)
  emitDecl(lines, '--term-bg', palette.terminal.bg.$value)
  emitDecl(lines, '--term-cursor', palette.terminal.cursor.$value)
  emitDecl(lines, '--term-selection', palette.terminal.selection.$value)
  const ansi = palette.terminal.ansi
  if (!Array.isArray(ansi) || ansi.length !== 16) fail(`${file}: terminal.ansi must be exactly 16 colors`)
  ansi.forEach((hex, i) => {
    if (!/^#[0-9a-fA-F]{6}$/.test(hex)) fail(`${file}: terminal.ansi[${i}] "${hex}" is not #rrggbb`)
    emitDecl(lines, `--term-ansi-${i}`, hex)
  })
  return lines
}

function loadStyle(id) {
  const dir = join(STYLES_DIR, id)
  const manifestFile = relative(ROOT, join(dir, 'style.json'))
  const manifest = readJson(join(dir, 'style.json'))
  for (const key of ['id', 'name', 'author', 'version', 'schemaVersion', 'defaultPalette', 'capabilities']) {
    if (manifest[key] === undefined) fail(`${manifestFile}: missing required field "${key}"`)
  }
  if (manifest.id !== id) fail(`${manifestFile}: id "${manifest.id}" must match folder name "${id}"`)
  if (manifest.schemaVersion !== 1) fail(`${manifestFile}: unsupported schemaVersion ${manifest.schemaVersion}`)

  const tokensFile = relative(ROOT, join(dir, 'tokens.json'))
  const tokenDecls = buildTokenDecls(tokensFile, readJson(join(dir, 'tokens.json')))

  const palettesDir = join(dir, 'palettes')
  let paletteFiles
  try {
    paletteFiles = readdirSync(palettesDir).filter((f) => f.endsWith('.json')).sort()
  } catch {
    fail(`${relative(ROOT, palettesDir)}: style "${id}" has no palettes/ directory`)
  }
  if (paletteFiles.length === 0) fail(`style "${id}" has no palettes`)

  const palettes = paletteFiles.map((f) => {
    const file = relative(ROOT, join(palettesDir, f))
    const palette = readJson(join(palettesDir, f))
    for (const key of ['id', 'name', 'schemes']) {
      if (palette[key] === undefined) fail(`${file}: missing required field "${key}"`)
    }
    if (`${palette.id}.json` !== f) fail(`${file}: palette id "${palette.id}" must match filename`)
    return { ...palette, decls: buildPaletteDecls(file, palette) }
  })

  if (!palettes.some((p) => p.id === manifest.defaultPalette))
    fail(`${manifestFile}: defaultPalette "${manifest.defaultPalette}" not found in palettes/`)

  return { manifest, tokenDecls, palettes }
}

// ── Build ────────────────────────────────────────────────────────────

const styleIds = readdirSync(STYLES_DIR)
  .filter((f) => statSync(join(STYLES_DIR, f)).isDirectory())
  .sort()
if (!styleIds.includes(DEFAULT_STYLE)) fail(`default style "${DEFAULT_STYLE}" not found in styles/`)

const styles = styleIds.map(loadStyle)
const out = []

out.push('/* AUTO-GENERATED by scripts/build-styles.mjs — DO NOT EDIT BY HAND.')
out.push(' * Source of truth: styles/<id>/{style.json,tokens.json,palettes/*.json}.')
out.push(' * Rebuild with: bun run styles:build (and commit the result). */')
out.push('')

// :root fallback = default style + its default palette, so everything renders
// correctly even before (or without) the data-style stamp on <html>.
const def = styles.find((s) => s.manifest.id === DEFAULT_STYLE)
const defPalette = def.palettes.find((p) => p.id === def.manifest.defaultPalette)
out.push(`/* Fallback: ${def.manifest.name} / ${defPalette.name} */`)
out.push(':root {')
out.push(...def.tokenDecls, ...defPalette.decls)
out.push('}')

for (const style of styles) {
  out.push('')
  out.push(`/* ── Style: ${style.manifest.name} (${style.manifest.id}) v${style.manifest.version} ── */`)
  out.push(`[data-style="${style.manifest.id}"] {`)
  out.push(...style.tokenDecls)
  out.push('}')
  for (const palette of style.palettes) {
    out.push(`[data-style="${style.manifest.id}"][data-palette="${palette.id}"] {`)
    out.push(...palette.decls)
    out.push('}')
  }
}
out.push('')

const css = out.join('\n')

if (process.argv.includes('--check')) {
  let current = ''
  try {
    current = readFileSync(OUT_FILE, 'utf8')
  } catch {
    fail(`${relative(ROOT, OUT_FILE)} does not exist — run: bun run styles:build`)
  }
  if (current !== css) fail(`${relative(ROOT, OUT_FILE)} is stale — run: bun run styles:build and commit`)
  console.log(`build-styles: ${relative(ROOT, OUT_FILE)} is current (${styles.length} style(s))`)
} else {
  writeFileSync(OUT_FILE, css)
  const paletteCount = styles.reduce((n, s) => n + s.palettes.length, 0)
  console.log(`build-styles: wrote ${relative(ROOT, OUT_FILE)} (${styles.length} style(s), ${paletteCount} palette(s))`)
}
