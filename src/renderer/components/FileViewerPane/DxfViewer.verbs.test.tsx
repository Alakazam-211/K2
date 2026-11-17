// @vitest-environment jsdom
//
// The DXF viewer's turning, scaling, text and block verbs, on the CAD pack's
// sampler through an in-memory drawing that answers them as the pack does:
//   - Rotate: "Rotate by" and the quarter turns in the panel, and the turn
//     handle above the selection (a ghost turns with it; Shift: 15° steps),
//     all about the selection's centre;
//   - Scale: "Scale by" a factor above 0 in the panel, and the corner
//     handles (a ghost scales the same both ways; Shift: 0.25 steps);
//   - each runs a dry run, then the commit with --expect, and shows as yours
//     with Undo; a refusal says why and the ghost goes back;
//   - a kind the verbs can't change has none of these.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'

vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onDocVersionChanged: () => () => undefined,
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { DxfViewer } from './DxfViewer'
import { fakeCadWorld, type FakeWorld } from './dxf/edit/fake-world'
import type { ModelHeader, ModelPage } from './dxf/model/types'

const FIX = resolve(__dirname, '__fixtures__')
const json = <T,>(n: string): T => JSON.parse(readFileSync(`${FIX}/${n}`, 'utf8')) as T
const HEADER = json<ModelHeader>('drawing-model/sampler-header.json')
const PAGES = {
  model: [json<ModelPage>('drawing-model/sampler-page-model.json')],
  '@h:2D': [json<ModelPage>('drawing-model/sampler-page-door.json')],
  '@h:2E': [json<ModelPage>('drawing-model/sampler-page-nest.json')],
  '@h:2F': [json<ModelPage>('drawing-model/sampler-page-dimension.json')],
}
const BRACKET = readFileSync(`${FIX}/k2-bracket.dxf`, 'utf8')
const PATH = '/ws/sampler.dxf'
const W = 800
const H = 400

let world: FakeWorld
let rectSpy: { mockRestore(): void }

beforeEach(() => {
  const rect = { x: 0, y: 0, left: 0, top: 0, width: W, height: H, right: W, bottom: H, toJSON: () => ({}) } as DOMRect
  rectSpy = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue(rect)
})
afterEach(() => {
  rectSpy.mockRestore()
  cleanup()
})

async function open(opts: { locked?: string[]; header?: ModelHeader } = {}): Promise<void> {
  world = fakeCadWorld({ header: opts.header ?? HEADER, pages: PAGES }, opts)
  renderInPrimaryRoom(<DxfViewer text={BRACKET} filePath={PATH} model={world.model} edits={world.edits} changes={world.changes} view="@h:51" />)
  await screen.findByTestId('dxf-shapes')
  await act(async () => {})
}

const sheet = () => screen.getByTestId('dxf-sheet')
const shape = (h: string) => screen.getByTestId('dxf-shapes').querySelector(`[data-h="${h}"]`)!

/** A drawing point → the screen point over it (the viewBox is fitted with "meet"). */
function client(x: number, y: number): { clientX: number; clientY: number } {
  const [vx, vy, vw, vh] = screen.getByTestId('dxf-svg').getAttribute('viewBox')!.split(' ').map(Number)
  const k = Math.max(vw / W, vh / H)
  const ox = vx + vw / 2 - (W * k) / 2
  const oy = vy + vh / 2 - (H * k) / 2
  return { clientX: (x - ox) / k, clientY: (-y - oy) / k }
}

function click(h: string, shiftKey = false): void {
  fireEvent.pointerDown(shape(h), { pointerId: 1, button: 0, shiftKey })
  fireEvent.pointerUp(sheet(), { pointerId: 1, shiftKey })
}

const xyAttr = (el: Element, a: string, b: string): [number, number] => [Number(el.getAttribute(a)), Number(el.getAttribute(b))]

describe('rotating', () => {
  it('Rotate by, about the selection’s centre: a dry run, then the commit, shown as yours', async () => {
    await open()
    click('@h:41')
    click('@h:A00', true)
    expect(screen.getByTestId('dxf-rotate-by').textContent).toContain('anticlockwise')
    fireEvent.change(screen.getByTestId('dxf-rotate-deg'), { target: { value: '30' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('dxf-rotate-go'))
    })
    await waitFor(() => expect(world.edits.calls).toHaveLength(2))
    const [dry, real] = world.edits.calls
    // The box around both lines is 0,0 to 10,2: its centre is 5,1.
    expect(dry).toEqual(['entity', 'rotate', '@h:41', '@h:A00', '--by', '30', '--about', '5,1', '--unit', 'mm', '--base', '7', '--json', '--dry-run'])
    expect(real.slice(0, 13)).toEqual(dry.slice(0, 13))
    expect(real[13]).toBe('--expect')
    expect((await screen.findByTestId('dxf-change-note')).textContent).toContain('You rotated 2 lines on A-WALL')
    expect(screen.getByTestId('dxf-change-undo')).toBeTruthy()
  })

  it('90° left and right are quarter turns either way', async () => {
    await open()
    click('@h:41')
    await act(async () => {
      fireEvent.click(screen.getByTestId('dxf-rotate-right'))
    })
    await waitFor(() => expect(world.edits.calls).toHaveLength(2))
    expect(world.edits.calls[0].slice(0, 7)).toEqual(['entity', 'rotate', '@h:41', '--by', '270', '--about', '5,2'])
  })

  it('the turn handle above the box: a ghost turns as you drag, Shift snaps to 15°, release rotates about the centre', async () => {
    await open()
    click('@h:41')
    const grip = screen.getByTestId('dxf-handle-turn')
    const [hx, hy] = xyAttr(grip, 'cx', 'cy')
    const r = hy - 2
    // A quarter turn anticlockwise and a little more: from above the centre to its left.
    const to: [number, number] = [5 - r * Math.cos(0.1), 2 - r * Math.sin(0.1)]
    fireEvent.pointerDown(grip, { pointerId: 1, button: 0, ...client(hx, hy) })
    fireEvent.pointerMove(sheet(), { pointerId: 1, ...client(...to) })
    const ghost = screen.getByTestId('dxf-ghost')
    expect(Number(ghost.getAttribute('data-turn'))).toBeCloseTo(95.7, 0)
    expect(ghost.getAttribute('transform')).toMatch(/^rotate\(95\.7 5 2\)$/)
    fireEvent.pointerMove(sheet(), { pointerId: 1, ...client(...to), shiftKey: true })
    expect(ghost.getAttribute('data-turn')).toBe('90')
    await act(async () => {
      fireEvent.pointerUp(sheet(), { pointerId: 1, ...client(...to), shiftKey: true })
    })
    await waitFor(() => expect(world.edits.calls).toHaveLength(2))
    expect(world.edits.calls[1].slice(0, 9)).toEqual(['entity', 'rotate', '@h:41', '--by', '90', '--about', '5,2', '--unit', 'mm'])
    // Read again: the line now stands upright through the centre.
    await waitFor(() => expect(Number(shape('@h:41').getAttribute('x1'))).toBeCloseTo(5, 6))
    expect(Number(shape('@h:41').getAttribute('x2'))).toBeCloseTo(5, 6)
  })

  it('a refused turn says why, and the ghost goes back', async () => {
    await open({ locked: ['A-WALL'] })
    click('@h:41')
    const grip = screen.getByTestId('dxf-handle-turn')
    const [hx, hy] = xyAttr(grip, 'cx', 'cy')
    fireEvent.pointerDown(grip, { pointerId: 1, button: 0, ...client(hx, hy) })
    fireEvent.pointerMove(sheet(), { pointerId: 1, ...client(-20, 2), shiftKey: true })
    await act(async () => {
      fireEvent.pointerUp(sheet(), { pointerId: 1, ...client(-20, 2), shiftKey: true })
    })
    expect((await screen.findByTestId('dxf-edit-message')).textContent).toBe('Not rotated. A-WALL is locked. Unlock it in your CAD app, then try again.')
    expect(screen.getByTestId('dxf-ghost').getAttribute('data-turn')).toBe('0')
    expect(world.edits.calls).toHaveLength(1)
  })

  it('kinds the verbs can’t change have no Rotate by and no handles', async () => {
    await open()
    click('@h:68')
    expect(screen.getByTestId('dxf-edit-cant').textContent).toContain('Hatches can’t be changed here yet.')
    expect(screen.queryByTestId('dxf-rotate-by')).toBeNull()
    expect(screen.queryByTestId('dxf-handles')).toBeNull()
  })
})

describe('scaling', () => {
  it('Scale by a factor about the selection’s centre; 0 or less says why and can’t be sent', async () => {
    await open()
    click('@h:41')
    click('@h:A00', true)
    fireEvent.change(screen.getByTestId('dxf-scale-factor'), { target: { value: '0' } })
    expect(screen.getByTestId('dxf-scale-problem').textContent).toBe('Use a factor above 0.')
    expect((screen.getByTestId('dxf-scale-go') as HTMLButtonElement).disabled).toBe(true)
    fireEvent.change(screen.getByTestId('dxf-scale-factor'), { target: { value: '2' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('dxf-scale-go'))
    })
    await waitFor(() => expect(world.edits.calls).toHaveLength(2))
    expect(world.edits.calls[0]).toEqual(['entity', 'scale', '@h:41', '@h:A00', '--factor', '2', '--about', '5,1', '--unit', 'mm', '--base', '7', '--json', '--dry-run'])
    expect((await screen.findByTestId('dxf-change-note')).textContent).toContain('You scaled 2 lines on A-WALL')
    // Read again: the lower line, twice as long about 5,1.
    await waitFor(() => expect(Number(shape('@h:A00').getAttribute('x1'))).toBeCloseTo(-5, 6))
    expect(Number(shape('@h:A00').getAttribute('x2'))).toBeCloseTo(15, 6)
  })

  it('a corner handle: a ghost scales the same both ways as you drag, Shift snaps to 0.25 steps', async () => {
    await open()
    click('@h:41')
    const corners = screen.getAllByTestId('dxf-handle-size')
    expect(corners).toHaveLength(4)
    const top = corners[2]
    const at: [number, number] = [Number(top.getAttribute('x')) + Number(top.getAttribute('width')) / 2, Number(top.getAttribute('y')) + Number(top.getAttribute('height')) / 2]
    // From the corner straight out from the centre (5,2), to 1.6 times as far.
    const to: [number, number] = [5 + (at[0] - 5) * 1.6, 2 + (at[1] - 2) * 1.6]
    fireEvent.pointerDown(top, { pointerId: 1, button: 0, ...client(...at) })
    fireEvent.pointerMove(sheet(), { pointerId: 1, ...client(...to) })
    const ghost = screen.getByTestId('dxf-ghost')
    expect(Number(ghost.getAttribute('data-scale'))).toBeCloseTo(1.6, 1)
    expect(ghost.getAttribute('transform')).toMatch(/^translate\(5 2\) scale\(1\.[56]\d*\) translate\(-5 -2\)$/)
    fireEvent.pointerMove(sheet(), { pointerId: 1, ...client(...to), shiftKey: true })
    expect(ghost.getAttribute('data-scale')).toBe('1.5')
    await act(async () => {
      fireEvent.pointerUp(sheet(), { pointerId: 1, ...client(...to), shiftKey: true })
    })
    await waitFor(() => expect(world.edits.calls).toHaveLength(2))
    expect(world.edits.calls[1].slice(0, 9)).toEqual(['entity', 'scale', '@h:41', '--factor', '1.5', '--about', '5,2', '--unit', 'mm'])
  })

  it('kinds the verbs can’t change have no Scale by', async () => {
    await open()
    click('@h:6B')
    expect(screen.getByTestId('dxf-edit-cant').textContent).toContain('Dimensions can’t be changed here yet.')
    expect(screen.queryByTestId('dxf-scale-by')).toBeNull()
    expect(screen.queryByTestId('dxf-handle-size')).toBeNull()
  })
})
