// Contrast floors for the shared controls, in the four shipped looks
// (Terminal: charcoal, paper; Glass: obsidian, veil). Palette colours are
// read from the style packages and measured with the WCAG formula; the CSS
// rules are read from globals.css and the generated styles, comments removed.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

const repo = join(__dirname, '..', '..', '..', '..')
const strip = (css: string): string => css.replace(/\/\*[\s\S]*?\*\//g, ' ')
const globals = strip(readFileSync(join(repo, 'src/renderer/globals.css'), 'utf8'))
const generated = strip(readFileSync(join(repo, 'src/renderer/styles.generated.css'), 'utf8'))

const LOOKS = [
  ['terminal', 'charcoal'],
  ['terminal', 'paper'],
  ['glass', 'obsidian'],
  ['glass', 'veil'],
] as const

type Rgb = [number, number, number]

function palette(style: string, id: string): Record<string, string> {
  const json = JSON.parse(readFileSync(join(repo, 'styles', style, 'palettes', `${id}.json`), 'utf8'))
  return Object.fromEntries(Object.entries(json.color as Record<string, { $value: string }>).map(([k, v]) => [k, v.$value]))
}

function hex(value: string): Rgb {
  const m = /^#([0-9a-f]{6})$/i.exec(value)
  if (!m) throw new Error(`not a #rrggbb colour: ${value}`)
  return [0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16)) as Rgb
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

/** The body of the first rule whose selector is exactly `sel`. */
function rule(css: string, sel: string): string {
  const at = css.indexOf(`${sel} {`)
  expect(at, `no rule for ${sel}`).toBeGreaterThanOrEqual(0)
  return css.slice(at, css.indexOf('}', at))
}

describe('Slider', () => {
  it('draws the unfilled track in the control edge', () => {
    expect(rule(globals, '.k2-slider::-webkit-slider-runnable-track')).toMatch(
      /linear-gradient\(to right, var\(--color-accent\) var\(--k2-slider-pct, 0%\), var\(--color-control-edge\) /,
    )
  })

  it.each(LOOKS)('the control edge holds 3:1 on a card and a raised control (%s %s)', (style, id) => {
    const p = palette(style, id)
    for (const ground of ['bg-surface', 'bg-elevated', 'bg']) {
      expect(contrast(hex(p['control-edge']), hex(p[ground])), `${id} control-edge on ${ground}`).toBeGreaterThanOrEqual(3)
    }
  })

  it('a white Glass thumb on a white Veil card carries the control edge', () => {
    const body = rule(generated, '[data-style="glass"][data-palette="veil"] .k2-slider:not([data-zen-root] *)::-webkit-slider-thumb')
    expect(body).toContain('0 0 0 1px var(--color-control-edge)')
  })
})

/** The colour a palette slot paints, for the few forms the shipped palettes
 *  use: #rrggbb, var() of another slot or Glass's accent, and
 *  color-mix(in srgb, <colour> N%, black|white). */
function resolve(style: string, id: string, value: string): Rgb {
  const p = palette(style, id)
  const v = value.trim()
  if (v.startsWith('#')) return hex(v)
  if (v === 'black') return [0, 0, 0]
  if (v === 'white') return [255, 255, 255]
  if (v === 'var(--glass-accent)') {
    const overrides = readFileSync(join(repo, 'styles', style, 'overrides.css'), 'utf8')
    return hex(/--glass-accent:\s*(#[0-9a-f]{6})/i.exec(overrides)![1])
  }
  const ref = /^var\(--color-([a-z-]+)\)$/.exec(v)
  if (ref) return resolve(style, id, p[ref[1]])
  const m = /^color-mix\(in srgb, (.+) (\d+)%, (black|white)\)$/.exec(v)
  if (!m) throw new Error(`can't resolve ${value}`)
  const [a, b, t] = [resolve(style, id, m[1]), resolve(style, id, m[3]), Number(m[2]) / 100]
  return a.map((c, i) => c * t + b[i] * (1 - t)) as Rgb
}

const over = (top: Rgb, ground: Rgb, alpha: number): Rgb => top.map((c, i) => c * alpha + ground[i] * (1 - alpha)) as Rgb

describe('Filled accent controls', () => {
  it.each(LOOKS)('the label reads at 4.5:1 on the accent fill, also at a 90%% hover (%s %s)', (style, id) => {
    const p = palette(style, id)
    const fill = resolve(style, id, 'var(--color-accent-fill)')
    const label = hex(p['on-accent'])
    expect(contrast(label, fill)).toBeGreaterThanOrEqual(4.5)
    for (const ground of ['bg-surface', 'bg-elevated']) {
      const g = hex(p[ground])
      expect(contrast(over(label, g, 0.9), over(fill, g, 0.9)), `${id} hover on ${ground}`).toBeGreaterThanOrEqual(4.5)
    }
  })

  it.each(LOOKS)('hovering the accent Button visibly changes the fill (1.3:1) and the label still reads at 4.5:1 (%s %s)', (style, id) => {
    const p = palette(style, id)
    const fill = resolve(style, id, 'var(--color-accent-fill)')
    const hover = resolve(style, id, p['accent-fill-hover'])
    const label = hex(p['on-accent'])
    expect(contrast(fill, hover), `${id} rest against hover`).toBeGreaterThanOrEqual(1.3)
    expect(contrast(label, hover), `${id} label on hover`).toBeGreaterThanOrEqual(4.5)
    // Press goes a little further than hover, the same way.
    const press = hover.map((c) => c * 0.9) as Rgb
    expect(contrast(label, press), `${id} label on press`).toBeGreaterThanOrEqual(4.5)
  })

  it('the accent Button hovers and presses on the hover slot', () => {
    const cls = /\n  accent:\s*'([^']+)'/.exec(readFileSync(join(__dirname, 'Button.tsx'), 'utf8'))![1]
    expect(cls).toContain('hover:bg-[var(--color-accent-fill-hover)] hover:border-[var(--color-accent-edge,var(--color-accent-fill-hover))]')
    expect(cls).toContain('active:bg-[color-mix(in_srgb,var(--color-accent-fill-hover)_90%,black)] active:border-[var(--color-accent-edge,color-mix(in_srgb,var(--color-accent-fill-hover)_90%,black))]')
  })

  it('the Button, the selected segment and Glass\'s segment thumb paint the fill', () => {
    expect(readFileSync(join(__dirname, 'Button.tsx'), 'utf8')).toContain("'bg-[var(--color-accent-fill)] border border-[var(--color-accent-edge,var(--color-accent-fill))] text-[var(--color-on-accent)]")
    expect(rule(globals, '.k2-segment-thumb')).toContain('background: var(--color-accent-fill)')
    expect(rule(generated, '[data-style="glass"] .k2-segment-thumb:not([data-zen-root] *)')).toContain('background: var(--color-accent-fill);')
  })
})

describe('Segmented in Glass', () => {
  it('is one design in both palettes: edged track, secondary labels, the accent fill under a white label', () => {
    const track = rule(generated, '[data-style="glass"] .k2-segmented[data-material="frost"]:not([data-zen-root] *)')
    expect(track).toContain('border: 1px solid var(--color-control-edge)')
    // Clear, so the track is the card: a Frost wash takes the chosen segment under 3:1 on Obsidian.
    expect(track).toContain('background: transparent')
    expect(rule(generated, '[data-style="glass"] .k2-segment:not([data-zen-root] *)')).toContain('color: var(--color-text-secondary)')
    expect(rule(generated, '[data-style="glass"] .k2-segment[aria-checked="true"]:not([data-zen-root] *)')).toContain('color: var(--color-on-accent)')
    // No palette draws its own thumb or labels.
    expect(generated).not.toMatch(/\[data-palette="veil"\] \.k2-segment(-thumb)?[:[]/)
  })

  it.each([['obsidian'], ['veil']])('the edge holds 3:1 on the card and the labels 4.5:1 on the track (%s)', (id) => {
    const p = palette('glass', id)
    const card = hex(p['bg-surface'])
    const track = card
    expect(contrast(hex(p['control-edge']), card), `${id} edge on card`).toBeGreaterThanOrEqual(3)
    expect(contrast(hex(p['control-edge']), hex(p['bg-elevated'])), `${id} edge on a raised card`).toBeGreaterThanOrEqual(3)
    expect(contrast(hex(p['text-secondary']), track), `${id} label on track`).toBeGreaterThanOrEqual(4.5)
    const fill = resolve('glass', id, 'var(--color-accent-fill)')
    expect(contrast(hex(p['on-accent']), fill), `${id} selected label`).toBeGreaterThanOrEqual(4.5)
    expect(contrast(fill, track), `${id} selected segment on the track`).toBeGreaterThanOrEqual(3)
  })
})

describe('Quiet destructive button', () => {
  const variant = (): string => /'danger-quiet':\s*'([^']+)'/.exec(readFileSync(join(__dirname, 'Button.tsx'), 'utf8'))![1]

  it('rests as the quiet outline with an error label, and fills red only on hover and press', () => {
    const cls = variant()
    expect(cls).toContain('bg-transparent border border-[var(--color-control-edge)] text-[var(--color-status-error-text)]')
    expect(cls).toContain('hover:bg-[var(--color-danger-muted)]')
    expect(cls).toContain('active:bg-[var(--color-danger-active)]')
    expect(cls).not.toMatch(/(^|\s)bg-\[var\(--color-danger/)
  })

  it.each(LOOKS)('the error label reads at 4.5:1 on a card, and white at 4.5:1 on the hover and press fills (%s %s)', (style, id) => {
    const p = palette(style, id)
    for (const ground of ['bg-surface', 'bg-elevated']) {
      expect(contrast(hex(p['status-error-text']), hex(p[ground])), `${id} label on ${ground}`).toBeGreaterThanOrEqual(4.5)
    }
    for (const fill of ['danger-muted', 'danger-active']) {
      expect(contrast(hex(p['on-accent']), hex(p[fill])), `${id} white on ${fill}`).toBeGreaterThanOrEqual(4.5)
    }
  })
})

describe('Information text on every ground', () => {
  const GROUNDS = ['bg', 'bg-surface', 'bg-elevated', 'bg-inset', 'bg-stripe', 'bg-canvas']

  it.each(LOOKS)('muted text and ok text (the (active) mark) read at 4.5:1 (%s %s)', (style, id) => {
    const p = palette(style, id)
    for (const slot of ['text-muted', 'status-ok-soft']) {
      for (const ground of GROUNDS) {
        expect(contrast(hex(p[slot]), hex(p[ground])), `${id} ${slot} on ${ground}`).toBeGreaterThanOrEqual(4.5)
      }
    }
  })
})

describe('Toggle', () => {
  it('a disabled switch in Glass light greys its knob and drops the lift, as the dark looks show it', () => {
    const body = rule(generated, '[data-style="glass"][data-palette="veil"] .k2-switch:disabled .k2-switch-knob:not([data-zen-root] *)')
    expect(body).toContain('background-color: color-mix(in srgb, var(--color-control-edge) 50%, var(--color-bg-surface))')
    expect(body).toContain('box-shadow: none')
  })
})

describe('Keyboard focus', () => {
  it('outside Terminal, buttons, fields, dropdown triggers, checkboxes and radios get a 2px ring in the readable accent', () => {
    const body = rule(
      globals,
      'html:not([data-style="terminal"]) :is(.k2-focus, .k2-checkbox, .k2-radio):focus-visible:not([data-zen-root] *, [data-k2-thread-zen] *)',
    )
    expect(body).toContain('outline: 2px solid var(--color-accent-text)')
    expect(body).toContain('outline-offset: 2px')
  })

  it('the shared Button, Input and the Settings dropdown trigger opt in', () => {
    for (const file of [join(__dirname, 'Button.tsx'), join(__dirname, 'Input.tsx'), join(repo, 'src/renderer/components/Settings/controls/SettingControls.tsx')]) {
      expect(readFileSync(file, 'utf8'), file).toMatch(/['`]k2-focus /)
    }
  })

  it('switch, segment, slider thumb, menu items and dropdown options ring in the readable accent', () => {
    expect(rule(globals, '.k2-switch:focus-visible')).toContain('outline: 2px solid var(--color-accent-text)')
    expect(rule(globals, '.k2-segment:focus-visible')).toContain('outline: 2px solid var(--color-accent-text)')
    expect(rule(generated, '[data-style="glass"] .k2-slider:focus-visible:not([data-zen-root] *)::-webkit-slider-thumb')).toContain('0 0 0 2px var(--color-accent-text)')
    expect(readFileSync(join(__dirname, 'OverflowMenu.tsx'), 'utf8')).toContain('focus-visible:outline-[var(--color-accent-text)]')
    expect(readFileSync(join(repo, 'src/renderer/components/Settings/controls/SettingControls.tsx'), 'utf8')).toContain(
      'focus:outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:-outline-offset-2 focus-visible:outline-[var(--color-accent-text)]',
    )
  })

  it('in Terminal a focused text field (Input, SearchField) draws the cut buttons\' 2px ring, not just a border recolour', () => {
    const body = rule(
      globals,
      'html[data-style="terminal"] input[data-k2-chamfer]:not([type="checkbox"], [type="radio"], [type="range"]):focus-visible:not([data-zen-root] *, [data-k2-thread-zen] *)',
    )
    // 1px border + 1px inset shadow on the sides, a 2px band on each cut.
    expect(body).toContain('--k2-ch-bw: 2px')
    expect(body).toContain('--k2-ch-edge: var(--color-accent-text)')
    expect(body).toContain('border-color: var(--color-accent-text)')
    expect(body).toContain('box-shadow: inset 0 0 0 1px var(--color-accent-text)')
    // The corner bands read their width from --k2-ch-bw.
    expect(globals).toContain('--k2-ch-w: var(--k2-ch-bw, 1px)')
    // Both shared fields are cut inputs, so both get it.
    expect(readFileSync(join(__dirname, 'Input.tsx'), 'utf8')).toContain('data-k2-chamfer="control"')
    expect(readFileSync(join(__dirname, 'SearchField.tsx'), 'utf8')).toContain('data-k2-chamfer="control"')
  })

  it('in Terminal a cut text box (<Textarea>) draws the same edge bands and the same focus ring as a field', () => {
    // The same rules as the cut <input>, the text box listed first in each.
    for (const state of ['', ':focus']) {
      expect(globals).toContain(
        `html[data-style="terminal"] :where(textarea[data-k2-chamfer]${state}:not([data-zen-root] *, [data-k2-thread-zen] *)),\nhtml[data-style="terminal"] :where(input[data-k2-chamfer]${state}:not([data-zen-root] *, [data-k2-thread-zen] *))`,
      )
    }
    expect(globals).toContain(
      'html[data-style="terminal"] textarea[data-k2-chamfer]:focus-visible:not([data-zen-root] *, [data-k2-thread-zen] *),\nhtml[data-style="terminal"] input[data-k2-chamfer]:not([type="checkbox"], [type="radio"], [type="range"]):focus-visible',
    )
    // It has no pseudo-elements, so the ::before corner band skips it.
    expect(globals).toContain(':where([data-k2-chamfer]:not(input, textarea):not([data-zen-root] *, [data-k2-thread-zen] *))::before')
    const src = readFileSync(join(__dirname, 'Input.tsx'), 'utf8')
    expect(src).toMatch(/<textarea ref=\{ref\} data-k2-chamfer="control"/)
    expect(src).toContain("'k2-focus no-drag w-full border border-[var(--color-control-edge)]")
  })

  it('the shared field sets its text at the control size, so a caller\'s type-control is not undercut by a raw size', () => {
    const src = readFileSync(join(__dirname, 'Input.tsx'), 'utf8')
    expect(src).toContain("'type-control px-2 py-1 outline-none'")
    expect(src).not.toContain('text-[11px]')
  })

  it('rows and tabs that opt in with data-k2-focus ring inside, in the theme\'s shape', () => {
    const sel = '[data-k2-focus]:focus-visible:not([data-zen-root] *, [data-k2-thread-zen] *)::after'
    const round = rule(globals, sel)
    expect(round).toContain('box-shadow: inset 0 0 0 2px var(--color-accent-text)')
    expect(round).toContain('border-radius: var(--radius-selector)')
    expect(round).toContain('inset: 0')
    const cut = rule(globals, `html[data-style="terminal"] ${sel}`)
    expect(cut).toContain('--k2-ch-cut: var(--chamfer-control)')
    expect(cut).toContain('border-radius: 0')
    expect(cut).toContain('background: var(--color-accent-text)')
    expect(cut).toMatch(/clip-path: polygon\(evenodd, /)
  })

  it('in Terminal a filled button rings in the accent with a white inner line; the others ring in the accent alone', () => {
    const button = readFileSync(join(__dirname, 'Button.tsx'), 'utf8')
    for (const v of ['accent', 'danger', "'danger-muted'"]) {
      const cls = new RegExp(`\\n  ${v}:\\s*'([^']+)'`).exec(button)![1]
      expect(cls, v).toContain('[--k2-ch-focus-inner:var(--color-on-accent)]')
      expect(cls, v).not.toContain('[--k2-ch-focus:')
    }
    for (const v of ['default', 'ghost', 'quiet', "'danger-quiet'"]) {
      const cls = new RegExp(`\\n  ${v}:\\s*'([^']+)'`).exec(button)![1]
      expect(cls, v).not.toContain('--k2-ch-focus')
    }
    const sel = 'html[data-style="terminal"] :where(button[data-k2-chamfer]:focus-visible:not([data-zen-root] *, [data-k2-thread-zen] *))'
    expect(rule(globals, `${sel}::after`)).toContain('background: var(--k2-ch-focus, var(--color-accent-text))')
    const inner = rule(globals, `${sel}::before`)
    expect(inner).toContain('--k2-ch-fr: 3px')
    expect(inner).toContain('background: var(--k2-ch-focus-inner, transparent)')
    expect(inner).toMatch(/clip-path: polygon\(evenodd, /)
  })

  it.each([['charcoal'], ['paper']])('Terminal %s: every Button variant\'s ring holds 3:1 on the page and on the button', (id) => {
    const p = palette('terminal', id)
    const ring = resolve('terminal', id, 'var(--color-accent)')
    const white = hex(p['on-accent'])
    for (const ground of ['bg', 'bg-surface', 'bg-elevated']) {
      expect(contrast(ring, hex(p[ground])), `${id} ring on ${ground}`).toBeGreaterThanOrEqual(3)
    }
    // Filled: the white line sits between the ring and the fill.
    for (const fill of ['var(--color-accent-fill)', 'var(--color-danger)', 'var(--color-danger-muted)']) {
      const f = resolve('terminal', id, fill)
      expect(contrast(white, f), `${id} inner line on ${fill}`).toBeGreaterThanOrEqual(3)
      expect(contrast(white, ring), `${id} inner line against the ring`).toBeGreaterThanOrEqual(3)
    }
    // Outlined: the ring sits on the button's own face (default is raised, the rest are clear).
    for (const face of ['bg-elevated', 'bg-surface', 'bg']) {
      expect(contrast(ring, hex(p[face])), `${id} ring on a ${face} button`).toBeGreaterThanOrEqual(3)
    }
  })

  it.each(LOOKS)('the readable accent clears 3:1 as a ring on a card and a raised control (%s %s)', (style, id) => {
    const p = palette(style, id)
    // Veil darkens it in overrides.css (80% accent, 20% black); elsewhere it is the accent.
    const accent = resolve(style, id, 'var(--color-accent)')
    const ring = id === 'veil' ? (accent.map((c) => c * 0.8) as Rgb) : accent
    for (const ground of ['bg-surface', 'bg-elevated', 'bg']) {
      expect(contrast(ring, hex(p[ground])), `${id} ring on ${ground}`).toBeGreaterThanOrEqual(3)
    }
  })
})

describe('Code in a sentence', () => {
  it('outside Terminal, a command inside a hint or row steps down to the sentence\'s visual size', () => {
    const body = rule(
      globals,
      'html:not([data-style="terminal"]) :is(.type-hint, .type-row, .type-control, .type-name) :is(code, .font-mono):not([class*="type-"]):not([data-zen-root] *)',
    )
    expect(body).toContain('font-size: 0.9em')
  })
})
