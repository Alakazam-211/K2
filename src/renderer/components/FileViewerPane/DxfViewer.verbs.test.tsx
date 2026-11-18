// @vitest-environment jsdom
//
// The DXF viewer's turning, scaling, text and block verbs, on the CAD pack's
// sampler through an in-memory drawing that answers them as the pack does:
//   - Rotate: "Rotate by" and the quarter turns in the panel, and the turn
//     handle above the selection (a ghost turns with it; Shift: 15° steps),
//     all about the selection's centre;
//   - Scale: "Scale by" a factor above 0 in the panel, and the corner
//     handles (a ghost scales the same both ways; Shift: 0.25 steps);
//   - Text: a single-line text's value set in the panel with a label's
//     checks; multi-line text read only;
//   - Insert block: the toolbar's action or the panel's with nothing
//     selected, a picker, a ghost that follows the pointer, a click, the
//     labels form, Insert; the new block is selected; refusals in words;
//   - two people changing different labels of one block from the same
//     version both land; the same label refuses the second;
//   - Undo (your own latest change) takes back each of these;
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

async function open(opts: { locked?: string[]; header?: ModelHeader; proxyBlocks?: string[]; pages?: typeof PAGES } = {}): Promise<void> {
  world = fakeCadWorld({ header: opts.header ?? HEADER, pages: opts.pages ?? PAGES }, opts)
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
    expect(world.edits.calls[0].slice(0, 7)).toEqual(['entity', 'rotate', '@h:41', '--by', '-90', '--about', '5,2'])
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

  it('a row the model says is on a locked layer names that layer, with no Rotate by', async () => {
    const model = structuredClone(PAGES.model[0])
    model.entities = model.entities.map((r) => (r.h === '@h:41' ? { ...r, edit: 'locked-layer' } : r))
    await open({ pages: { ...PAGES, model: [model] } })
    click('@h:41')
    expect(screen.getByTestId('dxf-edit-cant').textContent).toContain('It’s on A-WALL, which is locked. Unlock it in your CAD app first.')
    expect(screen.queryByTestId('dxf-rotate-by')).toBeNull()
  })

  it('a row marked read-only has no edit controls; its reason shows unless the header already says view only (DWG)', async () => {
    const model = structuredClone(PAGES.model[0])
    model.entities = model.entities.map((r) => ({ ...r, edit: 'read-only' }))
    const pages = { ...PAGES, model: [model] }
    // A DWG: the header line says it once; the panel adds no reason.
    await open({ header: { ...HEADER, format: 'dwg' }, pages })
    expect(screen.getByTestId('dxf-view-only').textContent).toMatch(/DWG/)
    click('@h:41')
    expect(screen.getByTestId('dxf-edit-view-only').textContent).toBe('View only')
    expect(screen.queryByTestId('dxf-edit-cant')).toBeNull()
    expect(screen.queryByTestId('dxf-move-by')).toBeNull()
    expect(screen.queryByTestId('dxf-rotate-by')).toBeNull()
    expect(screen.queryByTestId('dxf-handles')).toBeNull()
    cleanup()
    // Marked read-only without the header saying so: the row's own reason, and still no controls.
    await open({ pages })
    click('@h:41')
    expect(screen.getByTestId('dxf-edit-cant').textContent).toContain('This drawing is read only here.')
    expect(screen.queryByTestId('dxf-move-by')).toBeNull()
    expect(screen.queryByTestId('dxf-scale-by')).toBeNull()
    expect(screen.queryByTestId('dxf-handles')).toBeNull()
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

  it('a press on the shape itself, even right by a corner handle, moves it', async () => {
    await open()
    click('@h:41')
    // The line's end is about 7 pixels from its box's corner handle.
    fireEvent.pointerDown(shape('@h:41'), { pointerId: 1, button: 0, ...client(10, 2) })
    fireEvent.pointerMove(sheet(), { pointerId: 1, ...client(14, 2) })
    const ghost = screen.getByTestId('dxf-ghost')
    expect(ghost.getAttribute('data-scale')).toBeNull()
    expect(Number(ghost.getAttribute('data-d')!.split(' ')[0])).toBeCloseTo(4, 0)
    fireEvent.keyDown(sheet(), { key: 'Escape' })
  })

  it('kinds the verbs can’t change have no Scale by (and no Move by or Rotate by)', async () => {
    await open()
    click('@h:6B')
    expect(screen.getByTestId('dxf-edit-cant').textContent).toContain('Dimensions can’t be changed here yet.')
    expect(screen.queryByTestId('dxf-scale-by')).toBeNull()
    expect(screen.queryByTestId('dxf-move-by')).toBeNull()
    expect(screen.queryByTestId('dxf-rotate-by')).toBeNull()
    expect(screen.queryByTestId('dxf-handle-size')).toBeNull()
  })
})

describe('text', () => {
  it('a single-line text’s value is editable in the panel: Enter runs text set, shown as yours', async () => {
    await open()
    click('@h:5C')
    const input = screen.getByTestId('dxf-text-input') as HTMLInputElement
    expect(input.value).toBe('ROOM 101')
    fireEvent.change(input, { target: { value: 'ROOM 102' } })
    await act(async () => {
      fireEvent.keyDown(input, { key: 'Enter' })
    })
    await waitFor(() => expect(world.edits.calls).toHaveLength(2))
    expect(world.edits.calls[0]).toEqual(['text', 'set', '@h:5C', '--value', 'ROOM 102', '--base', '7', '--json', '--dry-run'])
    expect((await screen.findByTestId('dxf-change-note')).textContent).toContain('You edited text on ANNO')
    await waitFor(() => expect((screen.getByTestId('dxf-text-input') as HTMLInputElement).value).toBe('ROOM 102'))
  })

  it('the same checks as a label’s, in a text’s words; nothing is sent while one fails', async () => {
    await open()
    click('@h:5C')
    const input = screen.getByTestId('dxf-text-input')
    fireEvent.change(input, { target: { value: 'a'.repeat(256) } })
    expect(screen.getByTestId('dxf-text-problem').textContent).toBe('Too long: 256 bytes, and a line of text holds 255.')
    fireEvent.change(input, { target: { value: 'tab\there' } })
    expect(screen.getByTestId('dxf-text-problem').textContent).toBe('Remove the hidden character, such as a tab.')
    fireEvent.change(input, { target: { value: '--json' } })
    expect(screen.getByTestId('dxf-text-problem').textContent).toBe('Text can’t start with “--” yet.')
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(world.edits.calls).toHaveLength(0)
  })

  it('multi-line text stays read only', async () => {
    await open()
    click('@h:60')
    expect(screen.getByTestId('dxf-edit-text').textContent).toContain('Multi-line text: later.')
    expect(screen.queryByTestId('dxf-text-input')).toBeNull()
  })
})

describe('inserting a block', () => {
  /** The vendored pack's sampler header (the current layer, each block's flags and label definitions), with DOOR's definitions as given. */
  const doorDefs = (attdefs: ModelHeader['blocks'][number]['attdefs']): ModelHeader => ({
    ...HEADER,
    blocks: HEADER.blocks.map((b) => (b.name === 'DOOR' ? { ...b, attdefs } : b)),
  })
  /** The header as a pack before v0.4.0 wrote it: no block flags, no label definitions, no current layer. */
  const { current_layer: _cl, ...WITHOUT_LAYER } = HEADER
  const OLDER: ModelHeader = { ...WITHOUT_LAYER, blocks: HEADER.blocks.map(({ dynamic: _d, annotative: _a, attdefs: _t, ...b }) => b) }
  const press = (x: number, y: number) => {
    fireEvent.pointerDown(sheet(), { pointerId: 1, button: 0, ...client(x, y) })
    fireEvent.pointerUp(sheet(), { pointerId: 1, ...client(x, y) })
  }

  it('picker, then a ghost that follows the pointer, a click, the labels form, Insert; the new block is selected', async () => {
    await open()
    // With nothing selected the panel offers it; the toolbar too.
    expect(screen.getByTestId('dxf-edit-empty').textContent).toContain('Insert block…')
    expect(screen.getByTestId('dxf-insert-block').textContent).toBe('Insert block')
    fireEvent.click(screen.getByTestId('dxf-insert-open'))
    const choices = screen.getAllByTestId('dxf-insert-choice')
    expect(choices.map((c) => c.textContent)).toEqual(['DOOR', 'NEST'])
    fireEvent.click(choices[0])
    expect(screen.getByTestId('dxf-edit-hint').textContent).toContain('Click to place DOOR')
    const ghost = screen.getByTestId('dxf-ghost')
    expect(ghost.getAttribute('data-block')).toBe('@h:2D')
    expect(ghost.style.visibility).toBe('hidden')
    fireEvent.pointerMove(sheet(), { pointerId: 1, ...client(30, 5) })
    expect(ghost.style.visibility).toBe('')
    expect(ghost.getAttribute('data-d')).toBe('30 5')
    expect(ghost.querySelector('use')?.getAttribute('href')).toMatch(/^#.+-b-2D$/)
    press(30, 5)
    expect(world.edits.calls).toHaveLength(0)
    expect(screen.getByTestId('dxf-insert-at').textContent).toBe('At 30, 5 mm')
    // The drawing's current layer (@h:19), by name.
    expect(screen.getByTestId('dxf-insert-layer').textContent).toBe('Goes on layer 0')
    // DOOR's one label, by its prompt, with its default.
    const input = screen.getByTestId('dxf-insert-input') as HTMLInputElement
    expect(screen.getByTestId('dxf-insert-label').textContent).toBe('Door number')
    expect(input.value).toBe('D-00')
    fireEvent.change(input, { target: { value: 'D-07' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('dxf-insert-go'))
    })
    await waitFor(() => expect(world.edits.calls).toHaveLength(2))
    expect(world.edits.calls[0]).toEqual(['block', 'insert', '--block', '@h:2D', '--at', '30,5', '--unit', 'mm', '--attr', 'NO=D-07', '--base', '7', '--json', '--dry-run'])
    expect(world.edits.calls[1][13]).toBe('--expect')
    expect((await screen.findByTestId('dxf-change-note')).textContent).toContain('You inserted a block on 0')
    // Read again: the new reference is selected, with its label.
    await waitFor(() => expect(screen.queryAllByTestId('dxf-selected').map((r) => r.getAttribute('data-h'))).toEqual(['@h:C01']))
    expect(screen.getByTestId('dxf-edit-name').textContent).toBe('DOOR')
    expect((screen.getByTestId('dxf-attrib-input') as HTMLInputElement).value).toBe('D-07')
    expect(screen.queryByTestId('dxf-insert-panel')).toBeNull()
  })

  it('a block with no labels is placed on the click', async () => {
    await open()
    fireEvent.click(screen.getByTestId('dxf-insert-block'))
    fireEvent.click(screen.getAllByTestId('dxf-insert-choice')[1])
    fireEvent.pointerMove(sheet(), { pointerId: 1, ...client(40, 0) })
    await act(async () => {
      press(40, 0)
    })
    await waitFor(() => expect(world.edits.calls).toHaveLength(2))
    expect(world.edits.calls[0].slice(0, 8)).toEqual(['block', 'insert', '--block', '@h:2E', '--at', '40,0', '--unit', 'mm'])
    await waitFor(() => expect(screen.queryAllByTestId('dxf-selected').map((r) => r.getAttribute('data-h'))).toEqual(['@h:C01']))
  })

  it('only the labels that aren’t constant are asked for, each by its prompt, in file order', async () => {
    const defs = [
      { tag: 'NO', default: 'D-00', constant: false, prompt: 'Door number' },
      { tag: 'MAKER', default: 'ACME', constant: true, prompt: 'Maker' },
      { tag: 'FIRE', default: 'FD30', constant: false, prompt: 'Fire rating' },
    ]
    await open({ header: doorDefs(defs) })
    fireEvent.click(screen.getByTestId('dxf-insert-block'))
    fireEvent.click(screen.getAllByTestId('dxf-insert-choice')[0])
    press(30, 5)
    expect(screen.getAllByTestId('dxf-insert-label').map((l) => l.textContent)).toEqual(['Door number', 'Fire rating'])
    expect(screen.getAllByTestId('dxf-insert-input').map((i) => (i as HTMLInputElement).value)).toEqual(['D-00', 'FD30'])
  })

  it('no layer line when the drawing has no current layer', async () => {
    await open({ header: { ...HEADER, current_layer: null } })
    fireEvent.click(screen.getByTestId('dxf-insert-block'))
    fireEvent.click(screen.getAllByTestId('dxf-insert-choice')[0])
    press(30, 5)
    expect(screen.getByTestId('dxf-insert-form')).toBeTruthy()
    expect(screen.queryByTestId('dxf-insert-layer')).toBeNull()
  })

  it('labels are checked as you type; without definitions (a pack before v0.4.0), a label is sent only when filled in and no layer line shows', async () => {
    await open({ header: OLDER })
    fireEvent.click(screen.getByTestId('dxf-insert-block'))
    fireEvent.click(screen.getAllByTestId('dxf-insert-choice')[0])
    press(30, 5)
    const input = screen.getByTestId('dxf-insert-input') as HTMLInputElement
    expect(input.value).toBe('')
    expect(input.placeholder).toBe('The block’s default')
    expect(screen.getByTestId('dxf-insert-label').textContent).toBe('NO')
    expect(screen.queryByTestId('dxf-insert-layer')).toBeNull()
    fireEvent.change(input, { target: { value: '--x' } })
    expect((screen.getByTestId('dxf-insert-go') as HTMLButtonElement).disabled).toBe(true)
    fireEvent.change(input, { target: { value: '' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('dxf-insert-go'))
    })
    await waitFor(() => expect(world.edits.calls).toHaveLength(2))
    expect(world.edits.calls[0].slice(0, 9)).toEqual(['block', 'insert', '--block', '@h:2D', '--at', '30,5', '--unit', 'mm', '--base'])
  })

  it('a refusal says why in plain words and keeps the form', async () => {
    await open({ proxyBlocks: ['DOOR'] })
    fireEvent.click(screen.getByTestId('dxf-insert-block'))
    fireEvent.click(screen.getAllByTestId('dxf-insert-choice')[0])
    press(30, 5)
    await act(async () => {
      fireEvent.click(screen.getByTestId('dxf-insert-go'))
    })
    expect((await screen.findByTestId('dxf-edit-message')).textContent).toBe('Not inserted. That block holds an object made by another app. Insert it in that app.')
    expect(screen.getByTestId('dxf-insert-form')).toBeTruthy()
    expect(world.edits.calls).toHaveLength(1)
  })

  it('Esc stops placing: the ghost goes and the panel is back', async () => {
    await open()
    fireEvent.click(screen.getByTestId('dxf-insert-block'))
    fireEvent.click(screen.getAllByTestId('dxf-insert-choice')[0])
    fireEvent.pointerMove(sheet(), { pointerId: 1, ...client(30, 5) })
    expect(screen.getByTestId('dxf-ghost')).toBeTruthy()
    fireEvent.keyDown(sheet(), { key: 'Escape' })
    expect(screen.queryByTestId('dxf-ghost')).toBeNull()
    expect(screen.queryByTestId('dxf-insert-panel')).toBeNull()
    expect(screen.getByTestId('dxf-edit-empty')).toBeTruthy()
    expect(world.edits.calls).toHaveLength(0)
  })

  it('view only: no Insert block anywhere', async () => {
    await open({ header: { ...HEADER, format: 'dwg' } })
    expect(screen.queryByTestId('dxf-insert-block')).toBeNull()
    expect(screen.queryByTestId('dxf-edit-empty')).toBeNull()
  })
})

describe('labels of one block, changed by two people', () => {
  /** The DOOR at @h:61 with a second label, SIZE. */
  const twoLabels = (): typeof PAGES => {
    const model = structuredClone(PAGES.model[0])
    const door = model.entities.find((r) => r.h === '@h:61')!
    const attribs = door.geom!.attribs as { h: string; tag: string; value: string }[]
    door.geom = { ...door.geom, attribs: [...attribs, { ...attribs[0], h: '@h:B00', tag: 'SIZE', value: '900' }] }
    return { ...PAGES, model: [model] }
  }
  const relabel = async (tag: string, value: string) => {
    const input = screen.getAllByTestId('dxf-attrib-row').find((r) => r.getAttribute('data-tag') === tag)!.querySelector('input')!
    fireEvent.change(input, { target: { value } })
    await act(async () => {
      fireEvent.keyDown(input, { key: 'Enter' })
    })
  }

  it('different labels both land, from the same version', async () => {
    await open({ pages: twoLabels() })
    click('@h:61')
    // Someone else sets SIZE while this view still shows version 7.
    world.agentRun(['attrib', 'set', '@h:61', '--tag', 'SIZE', '--value', '1000'])
    await relabel('NO', 'D-02')
    await waitFor(() => expect(world.edits.calls).toHaveLength(2))
    expect(world.edits.calls[1].slice(0, 9)).toEqual(['attrib', 'set', '@h:61', '--tag', 'NO', '--value', 'D-02', '--base', '7'])
    expect((await screen.findByTestId('dxf-change-note')).textContent).toContain('You changed a label in a block')
    expect(screen.queryByTestId('dxf-edit-message')).toBeNull()
  })

  it('the same label refuses the second, in plain words, and reads the drawing again', async () => {
    await open({ pages: twoLabels() })
    click('@h:61')
    world.agentRun(['attrib', 'set', '@h:61', '--tag', 'NO', '--value', 'D-09'])
    await relabel('NO', 'D-02')
    expect((await screen.findByTestId('dxf-edit-message')).textContent).toBe('Not changed. Someone changed this since you looked. The drawing has been refreshed.')
    expect(world.edits.calls).toHaveLength(1)
  })
})

describe('undo', () => {
  const undo = async () => {
    const digest = world.edits.calls[1][world.edits.calls[1].indexOf('--expect') + 1]
    const revert = vi.spyOn(world.changes, 'revert')
    await act(async () => {
      fireEvent.click(await screen.findByTestId('dxf-change-undo'))
    })
    expect(revert).toHaveBeenCalledWith(PATH, 8, digest)
  }
  const x1 = (h: string) => Number(shape(h).getAttribute('x1'))

  it('takes back a rotate and a scale', async () => {
    for (const go of [() => fireEvent.click(screen.getByTestId('dxf-rotate-left')), () => {
      fireEvent.change(screen.getByTestId('dxf-scale-factor'), { target: { value: '3' } })
      fireEvent.click(screen.getByTestId('dxf-scale-go'))
    }]) {
      await open()
      click('@h:41')
      await act(async () => go())
      await waitFor(() => expect(x1('@h:41')).not.toBeCloseTo(0, 6))
      await undo()
      await waitFor(() => expect(screen.getByTestId('dxf-change-words').textContent).toBe('You undid a line on A-WALL'))
      await waitFor(() => expect(x1('@h:41')).toBeCloseTo(0, 6))
      cleanup()
    }
  })

  it('takes back a text’s new value', async () => {
    await open()
    click('@h:5C')
    const input = screen.getByTestId('dxf-text-input')
    fireEvent.change(input, { target: { value: 'ROOM 102' } })
    await act(async () => {
      fireEvent.keyDown(input, { key: 'Enter' })
    })
    await waitFor(() => expect((screen.getByTestId('dxf-text-input') as HTMLInputElement).value).toBe('ROOM 102'))
    await undo()
    await waitFor(() => expect((screen.getByTestId('dxf-text-input') as HTMLInputElement).value).toBe('ROOM 101'))
  })

  it('takes back a placed block: it’s gone again', async () => {
    await open()
    fireEvent.click(screen.getByTestId('dxf-insert-block'))
    fireEvent.click(screen.getAllByTestId('dxf-insert-choice')[1])
    await act(async () => {
      fireEvent.pointerDown(sheet(), { pointerId: 1, button: 0, ...client(40, 0) })
      fireEvent.pointerUp(sheet(), { pointerId: 1, ...client(40, 0) })
    })
    await waitFor(() => expect(screen.getByTestId('dxf-shapes').querySelector('[data-h="@h:C01"]')).not.toBeNull())
    await undo()
    await waitFor(() => expect(screen.getByTestId('dxf-shapes').querySelector('[data-h="@h:C01"]')).toBeNull())
    expect(screen.getByTestId('dxf-change-words').textContent).toBe('You undid a block on 0')
  })
})
