// @vitest-environment jsdom
//
// Linetypes and lineweights on the drawing-model path. Dash lengths are in
// drawing units, so they grow and shrink with the zoom; the viewer sets the
// screen pixels per drawing unit on the drawing as it zooms, and each dashed
// line's pattern is a CSS expression over it. These tests evaluate those
// expressions at a given zoom, so the maths is checked as the browser runs it.
// Lineweights are screen pixels at 96 dpi and don't follow the zoom.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { cleanup, render } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { ESTIMATED_METRICS } from '../text-metrics'
import { lineCtx, lineweightPx, patternOf, setPxPerUnit, strokeOf, insertLineVars, PX_PER_UNIT } from './lines'
import { loadModel } from './load'
import { fixtureDrawingSource } from './source'
import type { ModelHeader, ModelLinetype, ModelPage, ModelStyle } from './types'

const FIX = resolve(__dirname, '../../__fixtures__/drawing-model')
const json = <T,>(n: string): T => JSON.parse(readFileSync(`${FIX}/${n}.json`, 'utf8')) as T
const HEADER = json<ModelHeader>('sampler-header')
const MODEL = json<ModelPage>('sampler-page-model')
const DOOR = json<ModelPage>('sampler-page-door')
const NEST = json<ModelPage>('sampler-page-nest')
const DIM = json<ModelPage>('sampler-page-dimension')
const LT = (name: string): ModelLinetype => HEADER.linetypes!.find((l) => l.name === name)!
const withSettings = (s: Partial<NonNullable<ModelHeader['settings']>>): ModelHeader => ({ ...HEADER, settings: { ...HEADER.settings, ...s } })

afterEach(() => cleanup())

/**
 * A CSS value (calc, clamp, var with fallbacks, px) evaluated as the browser
 * would, with the given custom properties. Returns the list of numbers (px).
 */
function evalCss(value: string, vars: Record<string, string>): number[] {
  const sub = (s: string): string => {
    let out = s
    for (let n = 0; n < 20 && out.includes('var('); n++) {
      out = out.replace(/var\((--[\w-]+)(?:,\s*((?:[^()]|\([^()]*\))*))?\)/g, (_m, name: string, fb?: string) => {
        const v = vars[name] ?? fb
        if (v === undefined) throw new Error(`unset ${name} in ${value}`)
        return `(${v})`
      })
    }
    return out
  }
  // Split the dash list at top-level spaces between terms.
  const terms: string[] = []
  let depth = 0
  let cur = ''
  for (const ch of value.trim()) {
    if (ch === '(') depth++
    if (ch === ')') depth--
    if (ch === ' ' && depth === 0) {
      if (cur) terms.push(cur)
      cur = ''
    } else cur += ch
  }
  if (cur) terms.push(cur)
  return terms.map((t) => {
    const js = sub(t).replace(/calc\(/g, '(').replace(/clamp\(/g, 'K(').replace(/(\d)px/g, '$1')
    return Function('K', `return ${js}`)((lo: number, v: number, hi: number) => Math.min(hi, Math.max(lo, v))) as number
  })
}

/** An entity's dash list in screen px at `ppu` screen px per drawing unit (and a placement scale). */
function dashesAt(style: ModelStyle, header: ModelHeader, ppu: number, extra: Record<string, string> = {}): number[] | null {
  const s = strokeOf(style, lineCtx(header)) as Record<string, string>
  if (!s.strokeDasharray) return null
  return evalCss(s.strokeDasharray, { [PX_PER_UNIT]: String(ppu), ...extra, ...(s['--k2-u'] ? { '--k2-u': sub1(s['--k2-u'], { [PX_PER_UNIT]: String(ppu), ...extra }) } : {}) })
}
const sub1 = (v: string, vars: Record<string, string>): string => String(evalCss(v, vars)[0])

const style = (over: Partial<ModelStyle>): ModelStyle => ({ rgb: [0, 0, 255], fg: false, alpha: 1, ltype: 'DASHED', lt_scale: 1, lw_mm: 0.25, ...over })

describe('patternOf: a linetype as alternating dash and gap lengths', () => {
  it('DASHED: a dash then a gap, in drawing units', () => {
    expect(patternOf(LT('DASHED'))).toEqual({ runs: [{ len: 0.5, dot: false }, { len: 0.25, dot: false }], min: 0.25 })
  })

  it('a dot is a zero-length dash, kept apart so it can draw as a point', () => {
    expect(patternOf(LT('DOT'))).toEqual({ runs: [{ len: 0, dot: true }, { len: 0.2, dot: false }], min: 0.2 })
  })

  it('continuous, empty and gapless patterns draw as plain lines', () => {
    expect(patternOf(LT('CONTINUOUS'))).toBeNull()
    expect(patternOf(undefined)).toBeNull()
    expect(patternOf({ name: 'SOLIDISH', length: 1, dashes: [1], complex: [] })).toBeNull()
  })

  it('a pattern that starts with a gap gets a zero dash in front (not a dot); one ending on a dash stays even', () => {
    expect(patternOf({ name: 'G', length: 1.5, dashes: [-0.5, 1], complex: [] })).toEqual({
      runs: [{ len: 0, dot: false }, { len: 0.5, dot: false }, { len: 1, dot: false }, { len: 0, dot: false }],
      min: 0.5,
    })
  })

  it('runs of the same kind join (CENTER-like: long dash, gap, short dash, gap)', () => {
    expect(patternOf({ name: 'C', length: 2, dashes: [1.25, -0.25, 0.25, -0.25], complex: [] })!.runs.map((r) => r.len)).toEqual([1.25, 0.25, 0.25, 0.25])
    expect(patternOf({ name: 'J', length: 2, dashes: [0.5, 0.5, -0.25, -0.25], complex: [] })!.runs.map((r) => r.len)).toEqual([1, 0.5])
  })
})

describe('dash maths at a zoom', () => {
  it('lengths are drawing units × the entity’s linetype scale × screen px per unit', () => {
    // The model's lt_scale already carries $LTSCALE (own scale × $LTSCALE): it is used as it is.
    expect(dashesAt(style({ lt_scale: 2 }), HEADER, 10)).toEqual([10, 5])
    expect(dashesAt(style({ lt_scale: 1 }), HEADER, 10)).toEqual([5, 2.5])
    expect(dashesAt(style({ lt_scale: 2 }), HEADER, 40)).toEqual([40, 20])
  })

  it('a placement’s scale grows the dashes of its block’s entities', () => {
    expect(dashesAt(style({ lt_scale: 2 }), HEADER, 10, { '--k2i-s': '3' })).toEqual([30, 15])
  })

  it('a dot draws one lineweight long (1 px with lineweights off)', () => {
    expect(dashesAt(style({ ltype: 'DOT', lt_scale: 2 }), HEADER, 10)).toEqual([1, 4])
    const on = withSettings({ lwdisplay: true })
    expect(dashesAt(style({ ltype: 'DOT', lt_scale: 2, lw_mm: 0.5 }), on, 10)![0]).toBeCloseTo(1.89, 2)
  })

  it('dense: when the shortest dash or gap is under a screen pixel, the line draws continuous', () => {
    // DASHED at scale 2: the gap is 0.5 units. 2 px per unit → a 1 px gap still shows…
    expect(dashesAt(style({ lt_scale: 2 }), HEADER, 2.01)!.every((v) => v > 0)).toBe(true)
    // …under it, every gap closes.
    const dense = dashesAt(style({ lt_scale: 2 }), HEADER, 1.5)!
    expect(dense[0]).toBeGreaterThan(0)
    expect(dense.filter((_, i) => i % 2 === 1)).toEqual([0])
    // Zoomed far out, the same.
    expect(dashesAt(style({ lt_scale: 2 }), HEADER, 0.01)!.filter((_, i) => i % 2 === 1)).toEqual([0])
  })

  it('before the viewer has measured the zoom, dashed lines draw continuous', () => {
    const s = strokeOf(style({ lt_scale: 2 }), lineCtx(HEADER)) as Record<string, string>
    const u = sub1(s['--k2-u'], {})
    expect(evalCss(s.strokeDasharray, { '--k2-u': u }).filter((_, i) => i % 2 === 1)).toEqual([0])
  })

  it('continuous lines carry no dash at all', () => {
    expect(dashesAt(style({ ltype: 'CONTINUOUS' }), HEADER, 10)).toBeNull()
    expect(dashesAt(style({ ltype: 'NO-SUCH-TYPE' }), HEADER, 10)).toBeNull()
  })

  it('"insert" takes the placement’s pattern, which the placement works out at its own scale', () => {
    const s = strokeOf(style({ ltype: 'insert' }), lineCtx(HEADER)) as Record<string, string>
    expect(s.strokeDasharray).toBe('var(--k2i-d, none)')
    const vars = insertLineVars(style({ lt_scale: 2 }), lineCtx(HEADER), [3, 0, 0, 3, 0, 0], false)
    expect(vars['--k2i-s']).toBe('3')
    const d = vars['--k2i-d'].replace(/var\(--k2i-s, 1\)/g, vars['--k2i-s'])
    expect(evalCss(d, { [PX_PER_UNIT]: '10' })).toEqual([30, 15])
    // A continuous placement hands "none"; a nested placement whose own type is "insert" hands nothing.
    expect(insertLineVars(style({ ltype: 'CONTINUOUS' }), lineCtx(HEADER), [1, 0, 0, 1, 0, 0], false)['--k2i-d']).toBe('none')
    expect(insertLineVars(style({ ltype: 'insert' }), lineCtx(HEADER), [1, 0, 0, 1, 0, 0], true)['--k2i-d']).toBeUndefined()
  })

  it('a nested placement’s scale multiplies its outer placement’s', () => {
    const outer = insertLineVars(style({}), lineCtx(HEADER), [0, 2, -2, 0, 0, 0], false)
    expect(outer).toMatchObject({ '--k2i-s': '2', '--k2i-s0': '2' })
    const inner = insertLineVars(style({}), lineCtx(HEADER), [1.5, 0, 0, 1.5, 0, 0], true)
    expect(evalCss(inner['--k2i-s'], { '--k2i-s0': outer['--k2i-s0'] })).toEqual([3])
  })
})

describe('lineweights: hundredths of a mm to screen px at 96 dpi', () => {
  const on = lineCtx(withSettings({ lwdisplay: true }))

  it('mm / 25.4 × 96, at least 1 px', () => {
    expect(lineweightPx(0.5, on.settings)).toBeCloseTo(1.89, 2)
    expect(lineweightPx(1, on.settings)).toBeCloseTo(3.78, 2)
    expect(lineweightPx(0.13, on.settings)).toBe(1)
    expect(lineweightPx(0, on.settings)).toBe(1)
  })

  it('Default (or anything unresolved) uses $LWDEFAULT, else 0.25 mm', () => {
    expect(lineweightPx(-3, lineCtx(withSettings({ lwdisplay: true, lwdefault_mm: 1 })).settings)).toBeCloseTo(3.78, 2)
    expect(lineweightPx(undefined, lineCtx(withSettings({ lwdisplay: true, lwdefault_mm: 1 })).settings)).toBeCloseTo(3.78, 2)
    expect(lineweightPx(-3, lineCtx(withSettings({ lwdisplay: true, lwdefault_mm: undefined })).settings)).toBe(1)
  })

  it('$LWDISPLAY off or absent: everything at 1 px, as AutoCAD shows it', () => {
    expect(lineweightPx(2, lineCtx(withSettings({ lwdisplay: false })).settings)).toBeNull()
    expect(lineweightPx(2, lineCtx({ ...HEADER, settings: undefined }).settings)).toBeNull()
    expect((strokeOf(style({ ltype: 'CONTINUOUS', lw_mm: 2 }), lineCtx(HEADER)) as Record<string, string>).strokeWidth).toBeUndefined()
  })

  it('an entity draws at its weight; ByLayer resolves through its layer, ByBlock through its placement', () => {
    expect((strokeOf(style({ ltype: 'CONTINUOUS', lw_mm: 1 }), on) as Record<string, string>).strokeWidth).toBe('3.78px')
    expect((strokeOf(style({ ltype: 'CONTINUOUS', lw_mm: -1 }), on, 'A-WALL') as Record<string, string>).strokeWidth).toBe('1.32px')
    expect((strokeOf(style({ ltype: 'CONTINUOUS', lw_mm: 'insert' }), on) as Record<string, string>).strokeWidth).toBe('var(--k2i-w, 1px)')
    expect(insertLineVars(style({ ltype: 'CONTINUOUS', lw_mm: 0.5 }), on, [1, 0, 0, 1, 0, 0], false)['--k2i-w']).toBe('1.89px')
    expect(insertLineVars(style({ ltype: 'CONTINUOUS', lw_mm: 'insert' }), on, [1, 0, 0, 1, 0, 0], true)['--k2i-w']).toBeUndefined()
    // Lineweights off: a placement hands no weight either.
    expect(insertLineVars(style({ ltype: 'CONTINUOUS', lw_mm: 0.5 }), lineCtx(HEADER), [1, 0, 0, 1, 0, 0], false)['--k2i-w']).toBeUndefined()
  })
})

describe('setPxPerUnit: the viewer tells the drawing its zoom', () => {
  it('screen px per drawing unit, from the view and the pane (meet-fitted)', () => {
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg')
    const pane = { getBoundingClientRect: () => ({ width: 800, height: 400 }) } as unknown as Element
    setPxPerUnit(svg, pane, { w: 100, h: 100 })
    // 100 units fit the 400 px height: 4 px per unit.
    expect(svg.style.getPropertyValue(PX_PER_UNIT)).toBe('4')
    setPxPerUnit(svg, pane, { w: 400, h: 50 })
    expect(svg.style.getPropertyValue(PX_PER_UNIT)).toBe('2')
  })

  it('an unmeasured pane leaves it unset', () => {
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg')
    setPxPerUnit(svg, { getBoundingClientRect: () => ({ width: 0, height: 0 }) } as unknown as Element, { w: 1, h: 1 })
    expect(svg.style.getPropertyValue(PX_PER_UNIT)).toBe('')
  })
})

describe('the sampler drawn with its linetypes and lineweights', () => {
  const src = (h: ModelHeader) => fixtureDrawingSource({ header: h, pages: { model: [MODEL], '@h:2D': [DOOR], '@h:2E': [NEST], '@h:2F': [DIM] } })
  const draw = async (h: ModelHeader) => {
    const d = (await loadModel(src(h), ESTIMATED_METRICS))!
    const { container } = render(<svg>{d.shapes}</svg>)
    return container.querySelector('svg')!
  }
  const styleOf = (svg: SVGSVGElement, h: string): string => svg.querySelector(`[data-h="${h}"]`)!.getAttribute('style') ?? ''

  it('dashed lines, arcs and polylines carry their pattern; continuous ones don’t', async () => {
    const svg = await draw(HEADER)
    for (const h of ['@h:41', '@h:4C', '@h:51', '@h:52', '@h:A00', '@h:47']) expect(styleOf(svg, h)).toContain('stroke-dasharray')
    expect(styleOf(svg, '@h:43')).not.toContain('stroke-dasharray')
    // A polyline is one path, so its pattern runs on across its vertices.
    expect(svg.querySelectorAll('[data-h="@h:51"]')).toHaveLength(1)
    expect(svg.querySelector('[data-h="@h:51"]')!.getAttribute('d')!.match(/M/g)).toHaveLength(1)
  })

  it('the sampler has $LWDISPLAY off: no line is drawn wider than 1 px', async () => {
    const svg = await draw(HEADER)
    expect(svg.innerHTML).not.toContain('stroke-width:')
  })

  it('with $LWDISPLAY on, the 0.5 mm line is wider than the 0.35 mm ones, and a block’s ByBlock line takes its placement’s weight', async () => {
    const svg = await draw(withSettings({ lwdisplay: true }))
    expect(styleOf(svg, '@h:A01')).toContain('stroke-width: 1.89px')
    expect(styleOf(svg, '@h:A00')).toContain('stroke-width: 1.32px')
    const use = svg.querySelector('use[data-h="@h:61"]') as SVGUseElement
    expect(use.style.getPropertyValue('--k2i-w')).toBe('1.32px')
    const line0 = svg.querySelector('defs g[data-block="@h:2D"] [data-h="@h:35"]')!
    expect(line0.getAttribute('style')).toContain('var(--k2i-w, 1px)')
    expect(line0.getAttribute('style')).toContain('var(--k2i-d, none)')
  })
})
