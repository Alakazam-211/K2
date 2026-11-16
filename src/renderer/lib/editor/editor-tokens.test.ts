// The editor's palette tokens (build-editors.md E1a): syntax, presence, find
// matches and the selection. Every palette carries them (the build requires
// them, so a custom look can never drop one), and in the four shipped looks
// each one reads on the editor surface and the code well, measured with the
// WCAG formula on the palette files themselves.
import { readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

const repo = join(__dirname, '..', '..', '..', '..')

type Rgb = [number, number, number]

const SYNTAX_SLOTS = ['keyword', 'string', 'number', 'comment', 'function', 'type', 'property', 'punctuation'].map((k) => `syntax-${k}`)
const PRESENCE_SLOTS = ['presence-1', 'presence-2', 'presence-3', 'presence-4']
const MARK_SLOTS = ['find-match', 'find-match-current', 'selection']
const EDITOR_SLOTS = [...SYNTAX_SLOTS, ...PRESENCE_SLOTS, ...MARK_SLOTS]

function palette(style: string, id: string): Record<string, string> {
  const json = JSON.parse(readFileSync(join(repo, 'styles', style, 'palettes', `${id}.json`), 'utf8'))
  return Object.fromEntries(Object.entries(json.color as Record<string, { $value: string }>).map(([k, v]) => [k, v.$value]))
}

function parse(value: string): [number, number, number, number] {
  let m = /^#([0-9a-f]{6})$/i.exec(value)
  if (m) return [...[0, 2, 4].map((i) => parseInt(m![1].slice(i, i + 2), 16)), 1] as [number, number, number, number]
  m = /^rgba?\((\d+),\s*(\d+),\s*(\d+)(?:,\s*([\d.]+))?\)$/.exec(value)
  if (m) return [Number(m[1]), Number(m[2]), Number(m[3]), m[4] === undefined ? 1 : Number(m[4])]
  throw new Error(`not a #rrggbb or rgba() colour: ${value}`)
}

/** A colour as painted over a solid ground (opaque colours are themselves). */
function on(value: string, ground: Rgb): Rgb {
  const [r, g, b, a] = parse(value)
  return [r * a + ground[0] * (1 - a), g * a + ground[1] * (1 - a), b * a + ground[2] * (1 - a)]
}

function luminance([r, g, b]: Rgb): number {
  const lin = (v: number): number => {
    const s = v / 255
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

function contrast(a: Rgb, b: Rgb): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return (hi + 0.05) / (lo + 0.05)
}

const LOOKS = [
  ['terminal', 'charcoal'],
  ['terminal', 'paper'],
  ['glass', 'obsidian'],
  ['glass', 'veil'],
] as const

describe('Editor palette tokens', () => {
  it('every palette of every style defines them', () => {
    for (const style of readdirSync(join(repo, 'styles'), { withFileTypes: true }).filter((d) => d.isDirectory())) {
      for (const file of readdirSync(join(repo, 'styles', style.name, 'palettes'))) {
        const p = palette(style.name, file.replace(/\.json$/, ''))
        const missing = EDITOR_SLOTS.filter((s) => !(s in p))
        expect([style.name, file, missing]).toEqual([style.name, file, []])
      }
    }
  })

  it('the build requires them, so a look can never drop one', () => {
    const src = readFileSync(join(repo, 'scripts', 'build-styles.mjs'), 'utf8')
    for (const slot of EDITOR_SLOTS) expect(src, slot).toContain(`'${slot}'`)
  })

  it('the compiled styles carry them for each shipped look', () => {
    const css = readFileSync(join(repo, 'src', 'renderer', 'styles.generated.css'), 'utf8')
    for (const [style, id] of LOOKS) {
      const block = css.split(`[data-style="${style}"][data-palette="${id}"] {`)[1]?.split('}')[0] ?? ''
      for (const slot of EDITOR_SLOTS) expect(block, `${style}/${id} --color-${slot}`).toContain(`--color-${slot}:`)
    }
  })

  it.each(LOOKS)('%s %s: syntax colours read at 4.5:1 on the editor, the pane tile and the code well', (style, id) => {
    const p = palette(style, id)
    const grounds = ['bg', 'bg-surface', 'bg-inset'].map((g) => [g, on(p[g], on(p.bg, [0, 0, 0]))] as const)
    for (const slot of SYNTAX_SLOTS) {
      for (const [g, ground] of grounds) {
        expect(contrast(on(p[slot], ground), ground), `${id} ${slot} on ${g}`).toBeGreaterThanOrEqual(4.5)
      }
    }
  })

  it.each(LOOKS)('%s %s: presence colours (carets, flags) reach 3:1 on the editor and the tile', (style, id) => {
    const p = palette(style, id)
    for (const slot of PRESENCE_SLOTS) {
      for (const g of ['bg', 'bg-surface']) {
        const ground = on(p[g], [0, 0, 0])
        expect(contrast(on(p[slot], ground), ground), `${id} ${slot} on ${g}`).toBeGreaterThanOrEqual(3)
      }
    }
  })

  it.each(LOOKS)('%s %s: text stays at 4.5:1 on find matches and the selection, and each mark shows', (style, id) => {
    const p = palette(style, id)
    const ground = on(p.bg, [0, 0, 0])
    for (const slot of MARK_SLOTS) {
      const mark = on(p[slot], ground)
      expect(contrast(on(p['text-primary'], mark), mark), `${id} text on ${slot}`).toBeGreaterThanOrEqual(4.5)
      expect(contrast(mark, ground), `${id} ${slot} against the editor`).toBeGreaterThanOrEqual(1.1)
    }
    // Find selects the current match, on the current line: the match is drawn
    // over the selection over the active-line band, and text still reads.
    const stacked = on(p['find-match-current'], on(p.selection, on(p['wash-1'], ground)))
    expect(contrast(on(p['text-primary'], stacked), stacked), `${id} text on the current match, selected, on the active line`).toBeGreaterThanOrEqual(4.5)
    // The current match stands apart from the other matches.
    expect(contrast(on(p['find-match-current'], ground), on(p['find-match'], ground)), `${id} current vs other matches`).toBeGreaterThan(1.05)
  })

  it.each(LOOKS)('%s %s: line numbers (text-muted) read at 4.5:1 in the gutter', (style, id) => {
    const p = palette(style, id)
    const ground = on(p.bg, [0, 0, 0])
    expect(contrast(on(p['text-muted'], ground), ground)).toBeGreaterThanOrEqual(4.5)
  })
})
