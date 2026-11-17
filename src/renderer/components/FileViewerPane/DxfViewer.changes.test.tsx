// @vitest-environment jsdom
//
// The DXF viewer's live change highlight: when a new version of the file
// arrives, the viewer asks what changed (from = to − 1), outlines it in each
// author's presence colour for about 2 s, then fades it (at once under
// reduced motion), and says who changed what with Show and Undo. It also
// opens framed to a view address.

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

const FIX = resolve(__dirname, '__fixtures__')
const PLAN = readFileSync(`${FIX}/dxf-change-plan.dxf`, 'utf8')
const change = (name: string): unknown => JSON.parse(readFileSync(`${FIX}/cad-change/${name}.json`, 'utf8'))
const PATH = '/ws/drawings/plan.dxf'

function emit(version: number): void {
  for (const fn of bus.handlers) fn({ kind: 'doc_version_changed', projectId: 'p', workspace: '/ws', rel: 'drawings/plan.dxf', version })
}

async function open(source: ReturnType<typeof fixtureCadChangeSource>, view?: string): Promise<SVGSVGElement> {
  renderInPrimaryRoom(<DxfViewer text={PLAN} filePath={PATH} changes={source} view={view} />)
  const svg = (await screen.findByTestId('dxf-svg')) as unknown as SVGSVGElement
  vi.useFakeTimers()
  return svg
}

async function arrive(version: number): Promise<void> {
  await act(async () => {
    emit(version)
    await vi.advanceTimersByTimeAsync(0)
  })
}

const marks = () => screen.queryAllByTestId('dxf-change-mark')
const viewBox = (svg: SVGSVGElement) => svg.getAttribute('viewBox')!.split(' ').map(Number)

beforeEach(() => bus.handlers.clear())
afterEach(() => {
  vi.useRealTimers()
  cleanup()
  vi.unstubAllGlobals()
})

describe('DxfViewer: a new version is highlighted', () => {
  it('asks for that version’s change, outlines it in the author’s colour, and says who changed what', async () => {
    const source = fixtureCadChangeSource({ '42': change('move') })
    await open(source)
    await arrive(42)
    expect(source.calls.change).toEqual([[PATH, 41, 42]])
    const m = marks()
    expect(m.map((e) => [e.getAttribute('data-box'), e.getAttribute('data-was'), e.getAttribute('stroke')])).toEqual([
      ['0 0 100 0', 'true', 'var(--color-presence-1)'],
      ['10 0 110 0', 'false', 'var(--color-presence-1)'],
    ])
    // The line from where it was to where it went.
    expect(screen.getByTestId('dxf-change-link').getAttribute('x1')).toBe('50')
    expect(screen.getByTestId('dxf-change-note').textContent).toContain('Atlas moved a line on A-WALL')
  })

  it('holds about 2 s, then fades on the motion token and is removed; the note stays', async () => {
    await open(fixtureCadChangeSource({ '42': change('move') }))
    await arrive(42)
    const layer = screen.getByTestId('dxf-change-layer')
    expect(layer.getAttribute('data-phase')).toBe('on')
    await act(async () => void (await vi.advanceTimersByTimeAsync(1999)))
    expect(layer.getAttribute('data-phase')).toBe('on')
    await act(async () => void (await vi.advanceTimersByTimeAsync(1)))
    expect(layer.getAttribute('data-phase')).toBe('fading')
    expect(layer.getAttribute('style')).toContain('var(--motion-duration-slow)')
    await act(async () => void (await vi.advanceTimersByTimeAsync(400)))
    expect(screen.queryByTestId('dxf-change-layer')).toBeNull()
    expect(screen.getByTestId('dxf-change-note')).toBeTruthy()
  })

  it('under reduced motion: shown at once, removed at once, never fading', async () => {
    vi.stubGlobal('matchMedia', (q: string) => ({ matches: /reduce/.test(q), media: q, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {} }))
    await open(fixtureCadChangeSource({ '42': change('move') }))
    await arrive(42)
    expect(screen.getByTestId('dxf-change-layer').getAttribute('data-phase')).toBe('on')
    await act(async () => void (await vi.advanceTimersByTimeAsync(2000)))
    expect(screen.queryByTestId('dxf-change-layer')).toBeNull()
  })

  it('someone else’s change has no Undo: only your own latest change can be undone', async () => {
    await open(fixtureCadChangeSource({ '42': change('move') }))
    await arrive(42)
    expect(screen.getByTestId('dxf-change-note').textContent).toContain('Atlas moved a line on A-WALL')
    expect(screen.queryByTestId('dxf-change-undo')).toBeNull()
  })

  it('no change feed: the drawing is read again, with a quiet line naming who when the version says, and no outlines', async () => {
    const source = fixtureCadChangeSource({})
    await open(source)
    await act(async () => {
      for (const fn of bus.handlers) fn({ kind: 'doc_version_changed', projectId: 'p', workspace: '/ws', rel: 'drawings/plan.dxf', version: 42, by: { name: 'Atlas' } } as DocVersionChangedEvent)
      await vi.advanceTimersByTimeAsync(0)
    })
    const words = screen.getByTestId('dxf-change-words')
    expect(words.textContent).toBe('Atlas changed the drawing')
    expect(words.getAttribute('data-muted')).toBe('')
    expect(marks()).toHaveLength(0)
    expect(screen.queryByTestId('dxf-change-undo')).toBeNull()
    expect(screen.queryByTestId('dxf-change-show')).toBeNull()
    // A version that doesn't say who: read again, no line.
    fireEvent.click(screen.getByTestId('dxf-change-dismiss'))
    await arrive(43)
    expect(screen.queryByTestId('dxf-change-note')).toBeNull()
  })

  it('versions in a row are one note: the range from the first, "Atlas made 3 changes"', async () => {
    const source = fixtureCadChangeSource({ '50-51': change('move'), '50-52': change('three-in-a-row') })
    await open(source)
    await arrive(51)
    await arrive(52)
    expect(source.calls.change).toEqual([
      [PATH, 50, 51],
      [PATH, 50, 52],
    ])
    expect(screen.getByTestId('dxf-change-note').textContent).toContain('Atlas made 3 changes')
  })

  it('two authors: each outline in its author’s colour; the note names both, with no Undo', async () => {
    await open(fixtureCadChangeSource({ '50': change('two-authors') }))
    await arrive(50)
    const after = marks().filter((e) => e.getAttribute('data-was') === 'false')
    expect(after.map((e) => e.getAttribute('stroke'))).toEqual(['var(--color-presence-1)', 'var(--color-presence-1)', 'var(--color-presence-3)', 'var(--color-presence-1)'])
    // The approximate text box is dashed.
    expect(after[2].getAttribute('stroke-dasharray')).toBeTruthy()
    expect(screen.getByTestId('dxf-change-note').textContent).toContain('Atlas and Sam made 4 changes')
    expect(screen.queryByTestId('dxf-change-undo')).toBeNull()
  })

  it('doubt: quiet outlines for rows paired across an outside save; refused: no outlines', async () => {
    await open(fixtureCadChangeSource({ '54': change('doubt'), '55': change('refused') }))
    await arrive(54)
    expect(marks().filter((e) => e.getAttribute('data-was') === 'false').map((e) => e.getAttribute('data-quiet'))).toEqual(['true', 'false'])
    expect(screen.getByTestId('dxf-change-note').textContent).toContain('Changed by an outside save')
    await act(async () => void (await vi.advanceTimersByTimeAsync(20_000)))
    await arrive(55)
    expect(marks()).toEqual([])
    expect(screen.getByTestId('dxf-change-note').textContent).toContain('Changed by an outside save')
  })

  it('a block change outlines each placement', async () => {
    await open(fixtureCadChangeSource({ '45': change('block') }))
    await arrive(45)
    expect(marks().map((e) => e.getAttribute('data-address'))).toEqual(['@h:3F1', '@h:3F2'])
  })

  it('Show frames what changed', async () => {
    const svg = await open(fixtureCadChangeSource({ '43': change('add') }))
    await arrive(43)
    fireEvent.click(screen.getByTestId('dxf-change-show'))
    const want = fitBox({ minX: 40, minY: 40, maxX: 60, maxY: 60 })
    viewBox(svg).forEach((v, i) => expect(v).toBeCloseTo([want.x, want.y, want.w, want.h][i], 6))
  })

  it('no change from the server (no route yet): no outline, no note, as before', async () => {
    const source = fixtureCadChangeSource({})
    await open(source)
    await arrive(42)
    expect(source.calls.change).toHaveLength(1)
    expect(marks()).toEqual([])
    expect(screen.queryByTestId('dxf-change-note')).toBeNull()
  })
})

describe('DxfViewer: opening at a view address', () => {
  it('frames one entity by handle; Fit still shows the whole drawing', async () => {
    const svg = await open(fixtureCadChangeSource({}), '@h:2C0')
    const want = fitBox({ minX: 40, minY: 40, maxX: 60, maxY: 60 })
    viewBox(svg).forEach((v, i) => expect(v).toBeCloseTo([want.x, want.y, want.w, want.h][i], 6))
    fireEvent.click(screen.getByTestId('dxf-fit'))
    expect(viewBox(svg)[2]).toBeGreaterThan(100)
  })

  it('frames a layer, or a set of entities', async () => {
    const svg = await open(fixtureCadChangeSource({}), '@h:2C0,@h:2B1')
    const want = fitBox({ minX: 10, minY: 0, maxX: 110, maxY: 60 })
    viewBox(svg).forEach((v, i) => expect(v).toBeCloseTo([want.x, want.y, want.w, want.h][i], 6))
  })
})
