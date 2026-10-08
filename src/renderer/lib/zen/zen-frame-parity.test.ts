// prd-zen-garden-sync-defaults-v1 GF1, GF4, GS46 — until the renderer reads
// `frame` from the daemon (S7, 0.45.2), every renderer constant the frame
// describes is pinned to `crates/k2-core/src/zen/frame.toml`. If either side
// moves, this fails: a frame change is a defaults change (GS45), made in
// frame.toml (and, after S7, only there).
//
// Reads source text, not modules, so no component (and nothing it imports)
// runs here.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const ROOT = resolve(__dirname, '../../../..')

function src(rel: string): string {
  return readFileSync(resolve(ROOT, rel), 'utf8')
}

type FrameValue = number | string | boolean | string[]

/** frame.toml is kept to plain tables of numbers, strings, booleans and
 *  string arrays (its header says so); anything else fails here. */
function parseFrame(text: string): Record<string, Record<string, FrameValue>> {
  const out: Record<string, Record<string, FrameValue>> = { '': {} }
  let table = ''
  for (const [i, raw] of text.split('\n').entries()) {
    const line = raw.replace(/\s+#.*$/, '').trim()
    if (line === '' || line.startsWith('#')) continue
    const t = line.match(/^\[([a-z-]+)\]$/)
    if (t) {
      table = t[1]
      out[table] = {}
      continue
    }
    const kv = line.match(/^([a-z0-9-]+)\s*=\s*(.+)$/)
    if (!kv) throw new Error(`frame.toml line ${i + 1} isn't key = value: ${raw}`)
    const v = kv[2].trim()
    let value: FrameValue
    if (v === 'true' || v === 'false') value = v === 'true'
    else if (/^-?\d+(\.\d+)?$/.test(v)) value = Number(v)
    else if (/^"[^"]*"$/.test(v)) value = v.slice(1, -1)
    else if (/^\[.*\]$/.test(v)) value = JSON.parse(v) as string[]
    else throw new Error(`frame.toml line ${i + 1}: unsupported value ${v}`)
    out[table][kv[1]] = value
  }
  return out
}

const frame = parseFrame(src('crates/k2-core/src/zen/frame.toml'))

function f(table: string, key: string): FrameValue {
  const v = frame[table]?.[key]
  if (v === undefined) throw new Error(`frame.toml has no ${table}.${key}`)
  return v
}

function has(text: string, needle: string, what: string): void {
  expect(text.includes(needle), `${what}: expected to find ${JSON.stringify(needle)}`).toBe(true)
}

describe('frame.toml pins the renderer frame (GF1)', () => {
  it('is format 1', () => {
    expect(f('', 'format')).toBe(1)
  })

  it('[glass] = lib/zen/zen-glass.ts', () => {
    const t = src('src/renderer/lib/zen/zen-glass.ts')
    has(t, `var(--zen-border) ${f('glass', 'edge-border-mix')}%, color-mix(in srgb, var(--zen-text) ${f('glass', 'edge-text-mix')}%`, 'glass edge')
    has(t, `--zen-glass-blur: ${f('glass', 'blur')};`, 'glass blur')
    has(t, `color-mix(in srgb, white ${f('glass', 'sheen-white-mix')}%, transparent)`, 'glass sheen')
    has(t, `--zen-glass-shadow: ${f('glass', 'shadow')} color-mix(in srgb, black ${f('glass', 'shadow-black-mix')}%, transparent)`, 'glass shadow')
  })

  it('[motion] = lib/zen/zen-motion.ts', () => {
    const t = src('src/renderer/lib/zen/zen-motion.ts')
    has(t, `@keyframes zen-kf-slide { from { transform: translateY(${f('motion', 'slide-px')}px) }`, 'slide')
    has(t, `scale(var(--zen-popin-from, ${f('motion', 'popin-from')}))`, 'popin')
    has(t, `50% { opacity: ${f('motion', 'pulse-opacity')} }`, 'pulse')
  })

  it('[shield] = lib/zen/zen-style-shield.ts', () => {
    const t = src('src/renderer/lib/zen/zen-style-shield.ts')
    const factor = f('shield', 'box-radius-factor') as number
    has(t, `'--radius-box': 'calc(var(--zen-radius) / ${1 / factor})'`, 'box radius')
    has(t, `'--color-scrim': 'rgba(0, 0, 0, ${f('shield', 'scrim')})'`, 'scrim')
    has(t, `color-mix(in srgb, var(--zen-accent) ${f('shield', 'agent-selection-accent-mix')}%, transparent)`, 'agent selection')
    has(t, `color-mix(in srgb, var(--zen-bubble-me-text) ${f('shield', 'me-selection-text-mix')}%, transparent)`, 'my selection')
  })

  it('[overflow] = lib/zen/zen-overflow.ts', () => {
    const t = src('src/renderer/lib/zen/zen-overflow.ts')
    const order = (f('overflow', 'order') as string[]).map((k) => `'${k}'`).join(', ')
    has(t, `export const ZEN_OVERFLOW_ORDER = [${order}] as const`, 'overflow order')
    has(t, `export const ZEN_SWITCHER_COMPACT_MAX = '${f('overflow', 'switcher-compact')}'`, 'switcher compact')
    expect(f('overflow', 'column-min-width-share')).toBe(true)
    has(t, 'return `min(${own}px, calc((100% - ${gaps} * var(--zen-gap)) * ${share}))`', 'column share rule')
  })

  it('[bands] = ZenBands.tsx and ZenTemplateControls.tsx', () => {
    const bands = src('src/renderer/components/Zen/ZenBands.tsx')
    has(bands, `const GROUP_GAP_PX = ${f('bands', 'group-gap-px')}`, 'group gap')
    has(bands, `padding: '0 ${f('bands', 'inset-px')}px'`, 'band inset')
    has(bands, `calc(var(--zen-stoplight-safe-right, 0px) + ${f('bands', 'inset-px')}px)`, 'band right inset')
    has(src('src/renderer/components/Zen/ZenTemplateControls.tsx'), `export const TEXTING_BAR_HEIGHT_PX = ${f('bands', 'bar-height-px')}`, 'bar height')
  })

  it('[columns] = ZenPage.tsx', () => {
    const t = src('src/renderer/components/Zen/ZenPage.tsx')
    has(t, `gap: 'var(--zen-${f('columns', 'gap-token')})'`, 'column gap')
    has(t, `borderRadius: 'var(--zen-${f('columns', 'radius-token')})'`, 'column radius')
  })

  it('[widgets] = the Agents and nav-rail widgets and the rail views', () => {
    const agents = src('src/renderer/components/Zen/widgets/ZenAgentsWidget.tsx')
    const av = f('widgets', 'agents-avatar-px')
    has(agents, `style={{ width: ${av}, height: ${av} }}`, 'avatar box')
    has(agents, `size={${av}}`, 'avatar size')
    has(agents, `style={{ width: ${f('widgets', 'agents-focus-group-width-px')} }}`, 'focus-group picker width')
    const rail = src('src/renderer/components/Zen/widgets/ZenNavRailWidget.tsx')
    has(rail, `export const ZEN_NAV_RAIL_WIDTH_PX = ${f('widgets', 'nav-rail-width-px')}`, 'rail width')
    has(rail, `export const ZEN_NAV_RAIL_ROW_HEIGHT_PX = ${f('widgets', 'nav-rail-row-px')}`, 'rail row')
    const views = (f('widgets', 'rail-views') as string[]).map((v) => `'${v}'`).join(', ')
    has(src('src/renderer/lib/zen/zen-window.ts'), `export const ZEN_RAIL_VIEWS: readonly ZenRailView[] = [${views}]`, 'rail views')
  })

  it('[controls] = lib/zen/zen-controls.ts (the floor the daemon clamps to, GF3)', () => {
    const t = src('src/renderer/lib/zen/zen-controls.ts')
    const target = f('controls', 'target-px')
    has(t, `const MIN_BUTTON = { width: ${target}, height: ${target} }`, 'control target')
    has(t, `export const ZEN_DRAG_MIN_WIDTH_PX = ${f('controls', 'drag-min-width-px')}`, 'drag width')
    has(t, `const MIN_DRAG = { width: ZEN_DRAG_MIN_WIDTH_PX, height: ${f('controls', 'drag-min-height-px')} }`, 'drag height')
    has(t, `const MIN_OPACITY = ${f('controls', 'min-opacity')}`, 'min opacity')
    has(src('crates/k2-core/src/zen/sync.rs'), `("target-px", ${Number(target).toFixed(1)})`, 'daemon floor = renderer target')
  })

  it('covers every key in frame.toml (nothing unpinned)', () => {
    const keys = Object.entries(frame).flatMap(([t, kv]) => Object.keys(kv).map((k) => (t ? `${t}.${k}` : k)))
    expect(keys.sort()).toEqual(
      [
        'format',
        'glass.edge-border-mix', 'glass.edge-text-mix', 'glass.sheen-white-mix', 'glass.shadow', 'glass.shadow-black-mix', 'glass.blur',
        'motion.slide-px', 'motion.popin-from', 'motion.pulse-opacity',
        'shield.box-radius-factor', 'shield.scrim', 'shield.agent-selection-accent-mix', 'shield.me-selection-text-mix',
        'overflow.order', 'overflow.switcher-compact', 'overflow.column-min-width-share',
        'bands.group-gap-px', 'bands.bar-height-px', 'bands.inset-px',
        'columns.gap-token', 'columns.radius-token',
        'widgets.agents-avatar-px', 'widgets.agents-focus-group-width-px', 'widgets.nav-rail-width-px', 'widgets.nav-rail-row-px', 'widgets.rail-views',
        'controls.target-px', 'controls.drag-min-width-px', 'controls.drag-min-height-px', 'controls.min-opacity',
      ].sort(),
    )
  })
})
