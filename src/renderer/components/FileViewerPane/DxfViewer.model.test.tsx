// @vitest-environment jsdom
//
// The DXF viewer on the CAD pack's drawing model: when the server serves the
// model it draws from it (the same viewer DWG will use); when it doesn't,
// the viewer reads the DXF text itself, exactly as before. Either way the
// stats line, view addresses and the change highlight work.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen } from '@testing-library/react'
import type { DocVersionChangedEvent } from '@/stores/session-events'

const bus = vi.hoisted(() => ({ handlers: new Set<(e: DocVersionChangedEvent) => void>() }))
vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onDocVersionChanged: (_scope: unknown, fn: (e: DocVersionChangedEvent) => void) => {
    bus.handlers.add(fn)
    return () => void bus.handlers.delete(fn)
  },
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { DxfViewer, fitBox } from './DxfViewer'
import { fixtureCadChangeSource } from './dxf/cad-change-source'
import { fixtureDrawingSource } from './dxf/model/source'
import type { ModelHeader, ModelPage } from './dxf/model/types'

const FIX = resolve(__dirname, '__fixtures__')
const json = <T,>(n: string): T => JSON.parse(readFileSync(`${FIX}/${n}`, 'utf8')) as T
const HEADER = json<ModelHeader>('drawing-model/sampler-header.json')
const MODEL = json<ModelPage>('drawing-model/sampler-page-model.json')
const DOOR = json<ModelPage>('drawing-model/sampler-page-door.json')
const NEST = json<ModelPage>('drawing-model/sampler-page-nest.json')
const DIM = json<ModelPage>('drawing-model/sampler-page-dimension.json')
const BRACKET = readFileSync(`${FIX}/k2-bracket.dxf`, 'utf8')
const PATH = '/ws/sampler.dxf'
const model = () => fixtureDrawingSource({ header: HEADER, pages: { model: [MODEL], '@h:2D': [DOOR], '@h:2E': [NEST], '@h:2F': [DIM] } })

beforeEach(() => bus.handlers.clear())
afterEach(() => {
  vi.useRealTimers()
  cleanup()
})

describe('DxfViewer on the drawing model', () => {
  it('draws from the model when the server serves it', async () => {
    renderInPrimaryRoom(<DxfViewer text={BRACKET} filePath={PATH} model={model()} changes={null} />)
    const shapes = await screen.findByTestId('dxf-shapes')
    expect(shapes.getAttribute('data-source')).toBe('model')
    expect(shapes.querySelector('[data-h="@h:4B"]')!.tagName).toBe('circle')
  })

  it('says how many items aren’t drawn, and why in plain words on hover or focus', async () => {
    renderInPrimaryRoom(<DxfViewer text={BRACKET} filePath={PATH} model={model()} changes={null} />)
    const note = await screen.findByTestId('dxf-not-drawn')
    expect(note.textContent).toContain('3 items not drawn')
    const tip = screen.getByTestId('dxf-not-drawn-why')
    expect(tip.getAttribute('role')).toBe('tooltip')
    expect(tip.textContent).toContain('3DSOLID, ACME_GADGET')
    expect(tip.textContent).not.toContain('MULTILEADER')
    expect(tip.textContent).toContain('spline whose weights can’t be drawn')
    expect(note.getAttribute('aria-describedby')).toBe(tip.id)
  })

  it('lists paper layouts as coming next', async () => {
    renderInPrimaryRoom(<DxfViewer text={BRACKET} filePath={PATH} model={model()} changes={null} />)
    expect((await screen.findByTestId('dxf-layouts')).textContent).toContain('Layout1 coming next')
  })

  it('no model route: the viewer reads the DXF text itself, as before', async () => {
    const none = fixtureDrawingSource({ header: null, pages: {} })
    renderInPrimaryRoom(<DxfViewer text={BRACKET} filePath={PATH} model={none} changes={null} />)
    const shapes = await screen.findByTestId('dxf-shapes')
    expect(shapes.getAttribute('data-source')).toBeNull()
    expect(screen.getByTestId('dxf-stats').textContent).toMatch(/^17 entities/)
    expect(none.calls.header).toBe(1)
  })

  it('dashes follow the zoom: the drawing is told its screen px per drawing unit as it zooms', async () => {
    const rect = { x: 0, y: 0, left: 0, top: 0, width: 800, height: 400, right: 800, bottom: 400, toJSON: () => ({}) } as DOMRect
    const spy = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue(rect)
    try {
      renderInPrimaryRoom(<DxfViewer text={BRACKET} filePath={PATH} model={model()} changes={null} />)
      const svg = (await screen.findByTestId('dxf-svg')) as unknown as SVGSVGElement
      await screen.findByTestId('dxf-shapes')
      const ppu = (): number => Number(svg.style.getPropertyValue('--k2-ppu'))
      const [, , w, h] = svg.getAttribute('viewBox')!.split(' ').map(Number)
      expect(ppu()).toBeCloseTo(1 / Math.max(w / 800, h / 400), 4)
      const before = ppu()
      fireEvent.wheel(svg.parentElement!, { deltaY: -200, clientX: 400, clientY: 200 })
      expect(ppu()).toBeGreaterThan(before)
      // The viewer's own reading has no linetypes: nothing to tell.
      cleanup()
      renderInPrimaryRoom(<DxfViewer text={BRACKET} filePath={PATH} model={null} changes={null} />)
      const own = (await screen.findByTestId('dxf-svg')) as unknown as SVGSVGElement
      await screen.findByTestId('dxf-shapes')
      expect(own.style.getPropertyValue('--k2-ppu')).toBe('')
    } finally {
      spy.mockRestore()
    }
  })

  it('view addresses frame the model’s boxes', async () => {
    // The dense hatch's box, as the model gives it (a box under 2% of the drawing would grow to that).
    renderInPrimaryRoom(<DxfViewer text={BRACKET} filePath={PATH} model={model()} changes={null} view="@h:6A" />)
    const svg = await screen.findByTestId('dxf-svg')
    await screen.findByTestId('dxf-shapes')
    const [x0, y0, x1, y1] = MODEL.entities.find((e) => e.h === '@h:6A')!.bbox!
    const want = fitBox({ minX: x0, minY: y0, maxX: x1, maxY: y1 })
    svg.getAttribute('viewBox')!.split(' ').map(Number).forEach((v, i) => expect(v).toBeCloseTo([want.x, want.y, want.w, want.h][i], 6))
  })

  it('a moved text is highlighted, and framed, on the model’s own text box (the change and the drawing share one box)', async () => {
    const text = MODEL.entities.find((e) => e.h === '@h:5C')!
    const box = text.bbox!
    const moved = {
      schema: 'cad.change/1', format: 'dxf', from: 6, to: 7, pairing: 'kept',
      versions: [{ version: 7, by: { pid: 'p_atlas', name: 'Atlas', kind: 'agent' }, verb: 'entity move' }],
      entities: [{ address: '@h:5C', op: 'changed', kind: 'TEXT', layer: text.layer, owner: 'model', fields: ['geom'], bbox_before: [box[0] - 5, box[1], box[2] - 5, box[3]], bbox_after: box, bbox_q: 'approx', version: 7, by: 'p_atlas', identity: null }],
      tables: [], blocks: [],
    }
    renderInPrimaryRoom(<DxfViewer text={BRACKET} filePath={PATH} model={model()} changes={fixtureCadChangeSource({ '7': moved })} view="@h:5C" />)
    const svg = await screen.findByTestId('dxf-svg')
    await screen.findByTestId('dxf-shapes')
    // The view address frames the model's text box, about its centre (a small box grows to 2% of the drawing)…
    const [x, y, w, h] = svg.getAttribute('viewBox')!.split(' ').map(Number)
    expect(x + w / 2).toBeCloseTo((box[0] + box[2]) / 2, 6)
    expect(-(y + h / 2)).toBeCloseTo((box[1] + box[3]) / 2, 6)
    expect(w).toBeGreaterThanOrEqual(box[2] - box[0])
    await act(async () => {
      for (const fn of bus.handlers) fn({ kind: 'doc_version_changed', projectId: 'p', workspace: '/ws', rel: 'sampler.dxf', version: 7 })
      await new Promise((r) => setTimeout(r, 0))
    })
    // …and the highlight sits on the same box.
    const after = (await screen.findAllByTestId('dxf-change-mark')).find((m) => m.getAttribute('data-was') === 'false')!
    expect(after.getAttribute('data-box')).toBe(box.join(' '))
  })

  it('the change highlight outlines a block’s placements on the model’s boxes', async () => {
    const block = {
      schema: 'cad.change/1', format: 'dxf', from: 6, to: 7, pairing: 'kept',
      versions: [{ version: 7, by: { pid: 'p_atlas', name: 'Atlas', kind: 'agent' }, verb: 'block set' }],
      entities: [], tables: [], blocks: [{ address: '@h:2D', name: 'DOOR', op: 'changed', affected_inserts: ['@h:61'], affected_more: 0 }],
    }
    renderInPrimaryRoom(<DxfViewer text={BRACKET} filePath={PATH} model={model()} changes={fixtureCadChangeSource({ '7': block })} />)
    await screen.findByTestId('dxf-shapes')
    await act(async () => {
      for (const fn of bus.handlers) fn({ kind: 'doc_version_changed', projectId: 'p', workspace: '/ws', rel: 'sampler.dxf', version: 7 })
      await new Promise((r) => setTimeout(r, 0))
    })
    const mark = await screen.findByTestId('dxf-change-mark')
    expect(mark.getAttribute('data-box')).toBe(MODEL.entities.find((e) => e.h === '@h:61')!.bbox!.join(' '))
    fireEvent.click(screen.getByTestId('dxf-change-dismiss'))
  })
})
