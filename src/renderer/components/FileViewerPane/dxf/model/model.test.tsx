// @vitest-environment jsdom
//
// The drawing viewer on the CAD pack's drawing model: a header, pages of
// entities with their styles resolved, and block definitions fetched once
// per digest and instanced. Fixtures: the pack's own sampler (header, the
// model page and the DOOR block page) in __fixtures__/drawing-model/.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { cleanup, render } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { ESTIMATED_METRICS } from '../text-metrics'
import { viewBounds } from '../view-address'
import { loadModel, type ModelDrawing } from './load'
import { insertVars, paintOf } from './paint'
import { fixtureDrawingSource } from './source'
import { wrapRuns } from './text'
import type { ModelHeader, ModelPage, ModelRow } from './types'

const FIX = resolve(__dirname, '../../__fixtures__/drawing-model')
const json = <T,>(n: string): T => JSON.parse(readFileSync(`${FIX}/${n}.json`, 'utf8')) as T
const HEADER = json<ModelHeader>('sampler-header')
const MODEL = json<ModelPage>('sampler-page-model')
const DOOR = json<ModelPage>('sampler-page-door')
const row = (h: string): ModelRow => MODEL.entities.find((e) => e.h === h)!
const source = () => fixtureDrawingSource({ header: HEADER, pages: { model: [MODEL], '@h:2D': [DOOR] } })

async function drawn(): Promise<{ d: ModelDrawing; svg: SVGSVGElement }> {
  const d = (await loadModel(source(), ESTIMATED_METRICS))!
  const { container } = render(<svg>{d.shapes}</svg>)
  return { d, svg: container.querySelector('svg')! }
}
const el = (svg: SVGSVGElement, h: string): Element => svg.querySelector(`[data-h="${h}"]`)!

afterEach(() => cleanup())

describe('paint: styles come resolved from the model', () => {
  it('uses the model’s rgb as it is, with no palette of its own', () => {
    const s = row('@h:43').style!
    expect(paintOf(s).stroke).toBe(`rgb(${(s.rgb as number[]).join(' ')})`)
  })

  it('fg: true is the theme’s foreground, whatever its rgb', () => {
    expect(paintOf(row('@h:42').style!).stroke).toBe('currentColor')
  })

  it('"insert" takes the referencing INSERT’s value, through nested inserts', () => {
    const inBlock = DOOR.entities[0].style!
    expect(paintOf(inBlock).stroke).toBe('var(--k2i-c, currentColor)')
    expect(paintOf(inBlock).opacity).toBe('var(--k2i-a, 1)')
    // The INSERT hands its own resolved colour to its block…
    const ins = row('@h:61').style!
    expect(insertVars(ins)['--k2i-c']).toBe(`rgb(${(ins.rgb as number[]).join(' ')})`)
    // …and an INSERT whose own colour is "insert" (a nested one) hands nothing, so the outer value flows through.
    expect(insertVars(inBlock)['--k2i-c']).toBeUndefined()
    expect(insertVars({ ...ins, fg: true })['--k2i-c']).toBe('currentColor')
  })

  it('alpha below 1 is drawn as opacity', () => {
    expect(paintOf(row('@h:41').style!).opacity).toBeCloseTo((row('@h:41').style!.alpha as number), 6)
  })
})

describe('loadModel', () => {
  it('fetches each block definition once per digest, across versions', async () => {
    const src = source()
    await loadModel(src, ESTIMATED_METRICS)
    await loadModel(src, ESTIMATED_METRICS)
    // DOOR is placed twice (one an array); NEST and the dimension's block aren't in the fixtures.
    expect(src.calls.block.filter((h) => h === '@h:2D')).toHaveLength(1)
    // A new digest is a new definition.
    const changed = { ...HEADER, blocks: HEADER.blocks.map((b) => (b.handle === '@h:2D' ? { ...b, digest: 'changed' } : b)) }
    src.setHeader(changed)
    await loadModel(src, ESTIMATED_METRICS)
    expect(src.calls.block.filter((h) => h === '@h:2D')).toHaveLength(2)
  })

  it('a missing route (no header) is null, so the viewer keeps its own reading', async () => {
    expect(await loadModel(fixtureDrawingSource({ header: null, pages: {} }), ESTIMATED_METRICS)).toBeNull()
  })

  it('counts what isn’t drawn, with plain words for why (attribute templates aren’t counted)', async () => {
    const d = (await loadModel(source(), ESTIMATED_METRICS))!
    expect(d.notDrawn.count).toBe(3)
    expect(d.notDrawn.lines).toEqual(['2 kinds this viewer doesn’t draw yet: 3DSOLID, ACME_GADGET', '1 spline whose weights can’t be drawn'])
  })

  it('boxes and handles come from the model, for the change highlight and view addresses', async () => {
    const d = (await loadModel(source(), ESTIMATED_METRICS))!
    expect(d.index.handles['4B']).toEqual({ minX: 18, minY: 18, maxX: 22, maxY: 22 })
    expect(viewBounds('@h:4B', d)).toEqual({ minX: 18, minY: 18, maxX: 22, maxY: 22 })
    expect(d.index.layerState.OFF).toEqual({ frozen: false, off: true })
    expect(d.index.layerState.FROZEN).toEqual({ frozen: true, off: false })
  })
})

describe('ModelShapes', () => {
  it('instances a block from its definition: one definition, a placement per INSERT and array cell', async () => {
    const { svg } = await drawn()
    const defs = svg.querySelectorAll('defs [data-block="@h:2D"]')
    expect(defs).toHaveLength(1)
    // One INSERT plus a 2 × 3 array.
    expect(svg.querySelectorAll('use[data-block="@h:2D"]')).toHaveLength(7)
    // The block's layer-0 line draws in the INSERT's colour; its A-WALL line in its own.
    const [line0, , wall] = Array.from(defs[0].children)
    // A variable goes through style (presentation attributes don't read variables everywhere).
    expect(line0.getAttribute('style')).toContain('var(--k2i-c, currentColor)')
    expect(line0.getAttribute('style')).toContain('var(--k2i-v, visible)')
    expect(wall.getAttribute('stroke')).toBe(`rgb(${(DOOR.entities[2].style!.rgb as number[]).join(' ')})`)
    const use = svg.querySelector('use[data-h="@h:61"]') as SVGUseElement
    expect(use.style.getPropertyValue('--k2i-c')).toBe(`rgb(${(row('@h:61').style!.rgb as number[]).join(' ')})`)
  })

  it('draws a spline as its tessellated path', async () => {
    const { svg } = await drawn()
    const d = el(svg, '@h:58').getAttribute('d')!
    expect(d.match(/[ML]/g)).toHaveLength((row('@h:58').geom as { path: unknown[] }).path.length)
  })

  it('a dense hatch tiles its pattern inside its boundary instead of drawing every line', async () => {
    const { svg } = await drawn()
    const hatch = el(svg, '@h:6A')
    const fill = hatch.getAttribute('fill')!
    expect(fill).toMatch(/^url\(#/)
    expect(svg.querySelector(`pattern#${fill.slice(5, -1)}`)).not.toBeNull()
    expect(hatch.getAttribute('fill-rule')).toBe('evenodd')
    // A pattern hatch with its lines draws them; a solid one fills.
    expect(el(svg, '@h:69').querySelectorAll('path[data-part="lines"]')).toHaveLength(1)
    expect(el(svg, '@h:68').getAttribute('fill')).not.toBe('none')
  })

  it('places TEXT at the model’s anchor and alignment, with capitals at the stated height', async () => {
    const { svg } = await drawn()
    const centred = el(svg, '@h:5D')
    expect(centred.getAttribute('transform')).toBe('translate(20 10) scale(1 -1)')
    expect(centred.getAttribute('text-anchor')).toBe('middle')
    expect(Number(centred.getAttribute('font-size'))).toBeCloseTo(2 / 0.7, 9)
    expect(centred.querySelector('tspan')!.getAttribute('y')).toBe('1')
    // Fit: from the first point, along the run, stretched to its length.
    const fit = el(svg, '@h:5E')
    expect(fit.getAttribute('transform')).toBe('translate(0 20) scale(1 -1) rotate(-45)')
    expect(Number(fit.querySelector('tspan')!.getAttribute('textLength'))).toBeCloseTo(Math.hypot(10, 10), 9)
    // Runs keep their decoration.
    expect(el(svg, '@h:5F').querySelector('tspan[text-decoration="underline"]')!.textContent).toBe('UNDER')
  })

  it('MTEXT: paragraphs, wrapped at the reference width, with runs that keep bold and colour', async () => {
    const { svg } = await drawn()
    const m = el(svg, '@h:60')
    expect(m.querySelector('tspan[font-weight="700"]')!.textContent).toBe('Bold')
    // A run's own tspan (a line's tspan holds its runs).
    const red = Array.from(m.querySelectorAll('tspan tspan')).find((t) => t.textContent === 'red')!
    expect(red.getAttribute('fill')).toBe('rgb(255 0 0)')
    // Each line steps down by its tallest run, so a line of 5-unit runs never overlaps the next.
    const ys = Array.from(m.children).map((t) => Number(t.getAttribute('y')))
    for (let i = 1; i < ys.length; i++) expect(ys[i] - ys[i - 1]).toBeGreaterThanOrEqual((5 * 5) / 3 - 1e-9)
  })

  it('leaves hidden and not-drawn rows out', async () => {
    const { svg } = await drawn()
    for (const h of ['@h:44', '@h:45', '@h:46', '@h:5B', '@h:6E', '@h:6F']) expect(svg.querySelector(`[data-h="${h}"]`)).toBeNull()
  })
})

describe('wrapRuns', () => {
  const adv = (s: string) => s.length // one unit per character at size 1
  it('breaks between words at the width, keeping each run’s style', () => {
    const lines = wrapRuns([{ text: 'aaa bbb ' }, { text: 'ccc ddd', bold: true }], 8, 1, adv)
    expect(lines.map((l) => l.map((r) => r.text).join(''))).toEqual(['aaa bbb ', 'ccc ddd'])
    expect(lines[1][0].bold).toBe(true)
  })
  it('no width: one line', () => {
    expect(wrapRuns([{ text: 'aaa bbb ccc' }], 0, 1, adv)).toHaveLength(1)
  })
})
