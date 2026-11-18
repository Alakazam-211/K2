// @vitest-environment jsdom
//
// Editing a drawing in the DXF viewer, on the CAD pack's sampler through an
// in-memory drawing that answers the edit verbs as the pack does:
//   - click, Shift-click, a window and a crossing box, Esc; the outline in
//     your own accent; the panel beside the drawing;
//   - drag to move (a ghost, Shift's lock), a dry run then the commit with
//     --expect, your change shown at once with Undo, the drawing read again;
//   - a refusal snaps the ghost back and says why in plain words;
//   - an attribute's value changed in the panel, checked as you type; a
//     text's value shown read only;
//   - an agent's hold: marked, and asked about once before an edit, never
//     edited over silently;
//   - view only (DWG) and the viewer's own reading (no handles): no edit controls;
//   - Undo: only on your own latest change, gone once a newer version arrives;
//     a landed undo shows as yours; a refusal is calm; with no change feed,
//     someone else's version reads the drawing again.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'

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

import type { DocVersionChangedEvent } from '@/stores/session-events'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { DxfViewer } from './DxfViewer'
import { fakeCadWorld, type FakeWorld } from './dxf/edit/fake-world'
import type { Hold } from './dxf/edit/edit-source'
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

/** Opens framed on the walls by the door (at the whole drawing's fit, a few pixels span them all). */
async function open(opts: { holds?: Hold[]; locked?: string[]; header?: ModelHeader; writerOff?: string; readOnly?: string; names?: Record<string, string>; you?: string } = {}): Promise<void> {
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

function drag(from: [number, number], to: [number, number], o: { shiftKey?: boolean; target?: Element } = {}): void {
  const a = client(...from)
  const b = client(...to)
  fireEvent.pointerDown(o.target ?? sheet(), { pointerId: 1, button: 0, ...a })
  fireEvent.pointerMove(sheet(), { pointerId: 1, ...b, shiftKey: o.shiftKey })
  fireEvent.pointerUp(sheet(), { pointerId: 1, ...b, shiftKey: o.shiftKey })
}

function click(h: string, shiftKey = false): void {
  fireEvent.pointerDown(shape(h), { pointerId: 1, button: 0, shiftKey })
  fireEvent.pointerUp(sheet(), { pointerId: 1, shiftKey })
}

const selected = () => screen.queryAllByTestId('dxf-selected').map((r) => r.getAttribute('data-h')).sort()

describe('selecting', () => {
  it('click picks one, with the panel beside the drawing; the outline is your own accent', async () => {
    await open()
    expect(screen.queryByTestId('dxf-edit-panel')).toBeNull()
    expect(screen.getByTestId('dxf-edit-hint').textContent).toMatch(/Hold Space and drag to pan/)
    click('@h:41')
    expect(selected()).toEqual(['@h:41'])
    expect(screen.getByTestId('dxf-selected').getAttribute('stroke')).toBe('var(--color-accent)')
    expect(screen.getByTestId('dxf-edit-count').textContent).toBe('1 selected')
    expect(screen.getByTestId('dxf-edit-kind').textContent).toBe('Line')
    expect(screen.getByTestId('dxf-edit-one').textContent).toContain('A-WALL')
    expect(screen.getByTestId('dxf-edit-one').textContent).toContain('41')
    expect(screen.queryByTestId('dxf-edit-hint')).toBeNull()
  })

  it('a line under the pointer wins over the text whose box it crosses', async () => {
    await open()
    // The browser reports the text as under the pointer (its box covers the line).
    fireEvent.pointerDown(shape('@h:5C'), { pointerId: 1, button: 0, ...client(5, 2) })
    fireEvent.pointerUp(sheet(), { pointerId: 1, ...client(5, 2) })
    expect(selected()).toEqual(['@h:41'])
    // Away from any line, the text itself.
    fireEvent.pointerDown(shape('@h:5C'), { pointerId: 1, button: 0, ...client(12, 3) })
    fireEvent.pointerUp(sheet(), { pointerId: 1, ...client(12, 3) })
    expect(selected()).toEqual(['@h:5C'])
  })

  it('a block’s attribute text picks its block', async () => {
    await open()
    fireEvent.pointerDown(shape('@h:62'), { pointerId: 1, button: 0 })
    fireEvent.pointerUp(sheet(), { pointerId: 1 })
    expect(selected()).toEqual(['@h:61'])
  })

  it('Shift-click adds and removes; Esc clears', async () => {
    await open()
    click('@h:41')
    click('@h:A00', true)
    expect(selected()).toEqual(['@h:41', '@h:A00'])
    expect(screen.getByTestId('dxf-edit-kinds').textContent).toContain('2 lines')
    click('@h:41', true)
    expect(selected()).toEqual(['@h:A00'])
    fireEvent.keyDown(sheet(), { key: 'Escape' })
    expect(selected()).toEqual([])
    expect(screen.queryByTestId('dxf-edit-panel')).toBeNull()
  })

  it('left to right is a window (wholly inside), right to left a crossing (touching)', async () => {
    await open()
    drag([-1, -0.5], [11, 2.5])
    expect(selected()).toEqual(['@h:41', '@h:61', '@h:A00', '@h:A01'])
    drag([5.5, -0.5], [4.5, 2.5])
    expect(selected()).toEqual(expect.arrayContaining(['@h:41', '@h:A00', '@h:A01', '@h:51', '@h:5C']))
    expect(selected()).not.toContain('@h:4B')
  })

  it('a click on empty sheet clears; hidden and frozen entities are never picked', async () => {
    await open()
    click('@h:41')
    drag([500, 500], [500, 500])
    expect(selected()).toEqual([])
    drag([-1, 5.5], [11, 7.5])
    expect(selected()).toEqual([])
  })
})

/** A version from any writer, as the server announces it (`by`: its author's pid, when known). */
async function arrive(version: number, by?: string): Promise<void> {
  await act(async () => {
    for (const fn of bus.handlers) fn({ kind: 'doc_version_changed', projectId: 'p', workspace: '/ws', rel: 'sampler.dxf', version, ...(by ? { by } : {}) })
    await new Promise((r) => setTimeout(r, 0))
  })
}

/** Select the three walls and move them 5 to the right with Move by. */
async function moveWalls(): Promise<void> {
  click('@h:41')
  click('@h:A00', true)
  click('@h:A01', true)
  fireEvent.change(screen.getByTestId('dxf-move-dx'), { target: { value: '5' } })
  await act(async () => {
    fireEvent.click(screen.getByTestId('dxf-move-go'))
  })
  await screen.findByTestId('dxf-change-undo')
}

describe('undo', () => {
  it('your own latest change has Undo; it sends the head and its digest, and the undo shows as yours with no Undo', async () => {
    await open()
    await moveWalls()
    await waitFor(() => expect(Number(shape('@h:41').getAttribute('x1'))).toBeCloseTo(5, 6))
    const digest = world.edits.calls[1][world.edits.calls[1].indexOf('--expect') + 1]
    const revert = vi.spyOn(world.changes, 'revert')
    await act(async () => {
      fireEvent.click(screen.getByTestId('dxf-change-undo'))
    })
    expect(revert).toHaveBeenCalledWith(PATH, 8, digest)
    await waitFor(() => expect(screen.getByTestId('dxf-change-words').textContent).toBe('You undid 3 lines on A-WALL'))
    expect(screen.queryByTestId('dxf-change-undo')).toBeNull()
    // The drawing is read again: the walls are back.
    await waitFor(() => expect(Number(shape('@h:41').getAttribute('x1'))).toBeCloseTo(0, 6))
  })

  it('Undo is gone once a newer version arrives, from anyone', async () => {
    await open()
    await moveWalls()
    world.agentMove(['@h:42'], 0, 1)
    await arrive(9)
    expect(screen.queryByTestId('dxf-change-undo')).toBeNull()
  })

  it('someone changed it after: the refusal is calm (muted, not red), and Undo goes', async () => {
    await open()
    await moveWalls()
    // Someone else's version that hasn't reached this viewer yet.
    world.agentMove(['@h:42'], 0, 1)
    await act(async () => {
      fireEvent.click(screen.getByTestId('dxf-change-undo'))
    })
    const words = await screen.findByText('Someone changed the drawing after this, so it can’t be undone.')
    expect(words.getAttribute('data-muted')).toBe('')
    expect(words.className).toContain('--color-text-secondary')
    expect(document.querySelector('[class*="text-[var(--color-status-error"], [class*="text-[var(--color-danger"]')).toBeNull()
    expect(screen.queryByTestId('dxf-change-undo')).toBeNull()
  })

  it('no change feed: a version’s author pid is named from who’s on the file', async () => {
    await open({ names: { 'agent:w_atlas': 'Atlas' } })
    world.agentMove(['@h:41'], 0, 3)
    await arrive(8, 'agent:w_atlas')
    await waitFor(() => expect(screen.getByTestId('dxf-change-words').textContent).toBe('Atlas changed the drawing'))
    expect(screen.getByTestId('dxf-change-words').getAttribute('data-muted')).toBe('')
  })

  it('your own pid: no line (your own highlight covers it)', async () => {
    await open({ names: { 'person:me': 'Rosa' }, you: 'person:me' })
    world.agentMove(['@h:41'], 0, 3)
    await arrive(8, 'person:me')
    await waitFor(() => expect(Number(shape('@h:41').getAttribute('y1'))).toBeCloseTo(5, 6))
    expect(screen.queryByTestId('dxf-change-note')).toBeNull()
  })

  it('a pid not on the file: "The drawing was changed"', async () => {
    await open({ names: { 'agent:w_atlas': 'Atlas' } })
    world.agentMove(['@h:41'], 0, 3)
    await arrive(8, 'agent:w_gone')
    await waitFor(() => expect(screen.getByTestId('dxf-change-words').textContent).toBe('The drawing was changed'))
  })

  it('no change feed: someone else’s version reads the drawing again, with no outlines', async () => {
    await open()
    world.agentMove(['@h:41'], 0, 3)
    // No author on the version: read again, silently.
    await arrive(8)
    await waitFor(() => expect(Number(shape('@h:41').getAttribute('y1'))).toBeCloseTo(5, 6))
    expect(screen.queryAllByTestId('dxf-change-mark')).toHaveLength(0)
    expect(screen.queryByTestId('dxf-change-note')).toBeNull()
  })
})

describe('moving', () => {
  it('drag: a ghost follows, Shift locks to one axis; release runs a dry run, then the commit with --expect', async () => {
    await open()
    click('@h:41')
    const a = client(5, 2)
    fireEvent.pointerDown(shape('@h:41'), { pointerId: 1, button: 0, ...a })
    const b = client(25, 4)
    fireEvent.pointerMove(sheet(), { pointerId: 1, ...b, shiftKey: true })
    const ghost = screen.getByTestId('dxf-ghost')
    const [gx, gy] = ghost.getAttribute('data-d')!.split(' ').map(Number)
    expect(gx).toBeCloseTo(20, 0)
    expect(gy).toBe(0)
    expect(ghost.querySelector('line')).not.toBeNull()
    await act(async () => {
      fireEvent.pointerUp(sheet(), { pointerId: 1, ...b, shiftKey: true })
    })
    await waitFor(() => expect(world.edits.calls).toHaveLength(2))
    const [dry, real] = world.edits.calls
    expect(dry.slice(0, 4)).toEqual(['entity', 'move', '@h:41', '--by'])
    expect(dry[4]).toMatch(/^20(\.\d+)?,0$/)
    expect(dry.slice(5)).toEqual(['--unit', 'mm', '--base', '7', '--json', '--dry-run'])
    expect(real.slice(0, 7)).toEqual(dry.slice(0, 7))
    expect(real.slice(7, 11)).toEqual(['--base', '7', '--json', '--expect'])
    expect(real[11]).toMatch(/^sha256:/)
    // Shown at once as yours, with Undo; then the drawing is read again at the new version.
    const note = await screen.findByTestId('dxf-change-note')
    expect(note.textContent).toContain('You moved a line on A-WALL')
    expect(screen.getByTestId('dxf-change-undo')).toBeTruthy()
    expect(screen.getAllByTestId('dxf-change-mark').every((m) => m.getAttribute('data-self') === '')).toBe(true)
    await waitFor(() => expect(Number(shape('@h:41').getAttribute('x1'))).toBeCloseTo(20, 0))
    expect(selected()).toEqual(['@h:41'])
  })

  it('Move by, in the drawing’s units', async () => {
    await open()
    click('@h:41')
    click('@h:A00', true)
    expect(screen.getByTestId('dxf-move-by').textContent).toContain('in mm')
    fireEvent.change(screen.getByTestId('dxf-move-dx'), { target: { value: '2.5' } })
    fireEvent.change(screen.getByTestId('dxf-move-dy'), { target: { value: '-1' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('dxf-move-go'))
    })
    await waitFor(() => expect(world.edits.calls).toHaveLength(2))
    expect(world.edits.calls[1].slice(0, 8)).toEqual(['entity', 'move', '@h:41', '@h:A00', '--by', '2.5,-1', '--unit', 'mm'])
    expect((await screen.findByTestId('dxf-change-note')).textContent).toContain('You moved 2 lines on A-WALL')
  })

  it('a refusal snaps the ghost back and says why, never the code', async () => {
    await open({ locked: ['A-WALL'] })
    click('@h:41')
    const a = client(5, 2)
    fireEvent.pointerDown(shape('@h:41'), { pointerId: 1, button: 0, ...a })
    fireEvent.pointerMove(sheet(), { pointerId: 1, ...client(15, 2) })
    await act(async () => {
      fireEvent.pointerUp(sheet(), { pointerId: 1, ...client(15, 2) })
    })
    const msg = await screen.findByTestId('dxf-edit-message')
    expect(msg.textContent).toBe('Not moved. A-WALL is locked. Unlock it in your CAD app, then try again.')
    expect(world.edits.calls).toHaveLength(1)
    expect(screen.getByTestId('dxf-ghost').getAttribute('data-d')).toBe('0 0')
    expect(screen.queryByTestId('dxf-change-note')).toBeNull()
  })

  it('kinds the verbs can’t change say why, and have no Move', async () => {
    await open()
    click('@h:68')
    expect(screen.getByTestId('dxf-edit-cant').textContent).toContain('Hatches can’t be changed here yet.')
    expect(screen.queryByTestId('dxf-move-by')).toBeNull()
    const a = client(5, -25)
    fireEvent.pointerDown(shape('@h:68'), { pointerId: 1, button: 0, ...a })
    fireEvent.pointerMove(sheet(), { pointerId: 1, ...client(15, -25) })
    fireEvent.pointerUp(sheet(), { pointerId: 1, ...client(15, -25) })
    expect(screen.queryByTestId('dxf-ghost')).toBeNull()
    expect(world.edits.calls).toHaveLength(0)
  })
})

describe('relabelling', () => {
  it('a block reference’s attributes are rows; Enter saves through attrib set', async () => {
    await open()
    click('@h:61')
    expect(screen.getByTestId('dxf-edit-kind').textContent).toBe('Block')
    expect(screen.getByTestId('dxf-edit-name').textContent).toBe('DOOR')
    const input = screen.getByTestId('dxf-attrib-input') as HTMLInputElement
    expect(input.value).toBe('D-01')
    fireEvent.change(input, { target: { value: 'D-02' } })
    await act(async () => {
      fireEvent.keyDown(input, { key: 'Enter' })
    })
    await waitFor(() => expect(world.edits.calls).toHaveLength(2))
    expect(world.edits.calls[1].slice(0, 7)).toEqual(['attrib', 'set', '@h:61', '--tag', 'NO', '--value', 'D-02'])
    expect((await screen.findByTestId('dxf-change-note')).textContent).toContain('You changed a label in a block on A-WALL')
  })

  it('the value is checked as you type, and a bad one isn’t sent', async () => {
    await open()
    click('@h:61')
    const input = screen.getByTestId('dxf-attrib-input')
    fireEvent.change(input, { target: { value: 'x'.repeat(256) } })
    expect(screen.getByTestId('dxf-attrib-problem').textContent).toBe('Too long: 256 bytes, and a label holds 255.')
    expect(input.getAttribute('aria-invalid')).toBe('true')
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(world.edits.calls).toHaveLength(0)
  })

  it('a multi-line text’s value is shown, not editable here yet', async () => {
    await open()
    click('@h:60')
    expect(screen.getByTestId('dxf-edit-text').textContent).toContain('Multi-line text: later.')
    expect(screen.queryByTestId('dxf-text-input')).toBeNull()
    expect(screen.queryByTestId('dxf-attrib-input')).toBeNull()
  })
})

describe('holds', () => {
  const atlas: Hold = { pid: 'p_atlas', name: 'Atlas', note: 'moving the doors', keys: ['@h:41'] }

  it('holds are read again when a claim is taken or ends, with no polling', async () => {
    await open()
    expect(screen.queryAllByTestId('dxf-hold-mark')).toHaveLength(0)
    world.edits.setHolds([atlas])
    await act(async () => {
      world.edits.emitHoldsChanged()
    })
    await waitFor(() => expect(screen.getAllByTestId('dxf-hold-mark').map((m) => m.getAttribute('data-h'))).toEqual(['@h:41']))
    world.edits.setHolds([])
    await act(async () => {
      world.edits.emitHoldsChanged()
    })
    await waitFor(() => expect(screen.queryAllByTestId('dxf-hold-mark')).toHaveLength(0))
  })

  it('an agent’s hold is marked on its entities, and named in the panel', async () => {
    await open({ holds: [atlas] })
    await waitFor(() => expect(screen.getAllByTestId('dxf-hold-mark').map((m) => m.getAttribute('data-h'))).toEqual(['@h:41']))
    click('@h:41')
    expect(screen.getByTestId('dxf-edit-held').textContent).toBe('Atlas is working on this: moving the doors.')
  })

  it('an edit over a hold asks once, naming who and what for; Cancel sends nothing', async () => {
    await open({ holds: [atlas] })
    click('@h:41')
    fireEvent.change(screen.getByTestId('dxf-move-dx'), { target: { value: '5' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('dxf-move-go'))
    })
    const ask = await screen.findByTestId('dxf-hold-ask')
    expect(ask.textContent).toContain('Atlas is working on this: moving the doors. Move anyway?')
    expect(world.edits.calls).toHaveLength(0)
    await act(async () => {
      fireEvent.click(screen.getByTestId('dxf-hold-ask-no'))
    })
    expect(screen.queryByTestId('dxf-hold-ask')).toBeNull()
    expect(world.edits.calls).toHaveLength(0)
  })

  it('Move anyway runs the edit (a person isn’t refused by a hold)', async () => {
    await open({ holds: [atlas] })
    click('@h:41')
    fireEvent.change(screen.getByTestId('dxf-move-dx'), { target: { value: '5' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('dxf-move-go'))
    })
    await act(async () => {
      fireEvent.click(await screen.findByTestId('dxf-hold-ask-yes'))
    })
    await waitFor(() => expect(world.edits.calls).toHaveLength(2))
    expect((await screen.findByTestId('dxf-change-note')).textContent).toContain('You moved a line')
  })
})

describe('when the server can’t save drawing edits', () => {
  const HINT = 'K2 can’t save drawing edits yet; this file opens read-only.'

  it('the first try turns the tab view only, with the server’s words as given, and the controls don’t come back', async () => {
    await open({ writerOff: HINT })
    click('@h:41')
    fireEvent.change(screen.getByTestId('dxf-move-dx'), { target: { value: '5' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('dxf-move-go'))
    })
    expect((await screen.findByTestId('dxf-view-only')).textContent).toBe(HINT)
    // The reason shows once, in the header line; the panel says only "View only", and nothing is in the error colour.
    expect(screen.queryByTestId('dxf-edit-message')).toBeNull()
    expect(screen.getByTestId('dxf-edit-view-only').textContent).toBe('View only')
    expect(screen.getByTestId('file-dxf').textContent!.split(HINT)).toHaveLength(2)
    expect(document.querySelector('[class*="text-[var(--color-status-error"], [class*="text-[var(--color-danger"]')).toBeNull()
    // The dry run was the only call: nothing was written.
    expect(world.edits.calls).toHaveLength(1)
    expect(world.edits.calls[0]).toContain('--dry-run')
    expect(screen.queryByTestId('dxf-move-by')).toBeNull()
    // The same server, opened again this session: view only from the start.
    cleanup()
    renderInPrimaryRoom(<DxfViewer text={BRACKET} filePath="/ws/other.dxf" model={world.model} edits={world.edits} changes={world.changes} view="@h:51" />)
    await screen.findByTestId('dxf-shapes')
    expect((await screen.findByTestId('dxf-view-only')).textContent).toBe(HINT)
    click('@h:41')
    expect(screen.queryByTestId('dxf-move-by')).toBeNull()
  })

  it('the host’s writer, the header’s first key: the drawing reads as ever, and false is view only', async () => {
    const { writer: _w, ...rest } = HEADER as ModelHeader & { writer?: boolean }
    await open({ header: { writer: false, ...rest } as ModelHeader })
    expect(Object.keys((await world.model.header())!)[0]).toBe('writer')
    expect(screen.getByTestId('dxf-shapes').getAttribute('data-source')).toBe('model')
    expect(screen.getByTestId('dxf-stats').textContent).toMatch(/^49 entities/)
    expect(screen.getByTestId('dxf-view-only').textContent).toBe('This server can’t save drawing edits yet; this file opens read-only.')
    click('@h:41')
    expect(screen.queryByTestId('dxf-move-by')).toBeNull()
  })

  it('docs/cores’ reason wins over the header’s writer: false, as the server’s own words', async () => {
    await open({ header: { writer: false, ...HEADER } as ModelHeader, readOnly: 'Drawings open read-only until the edit side is on.' })
    expect((await screen.findByTestId('dxf-view-only')).textContent).toBe('Drawings open read-only until the edit side is on.')
  })

  it('the host’s header says writer: false: view only up front, in its words, with no Move or attribute controls', async () => {
    await open({ header: { ...HEADER, writer: false, read_only_reason: 'Drawing edits are off on this server.' } })
    expect(screen.getByTestId('dxf-view-only').textContent).toBe('Drawing edits are off on this server.')
    click('@h:61')
    expect(screen.queryByTestId('dxf-move-by')).toBeNull()
    expect(screen.queryByTestId('dxf-attrib-input')).toBeNull()
    expect(world.edits.calls).toHaveLength(0)
  })

  it('docs/cores gives a read-only reason: view only up front, in its words', async () => {
    await open({ readOnly: 'This server opens drawings read-only.' })
    expect((await screen.findByTestId('dxf-view-only')).textContent).toBe('This server opens drawings read-only.')
    click('@h:61')
    expect(screen.queryByTestId('dxf-move-by')).toBeNull()
    expect(screen.queryByTestId('dxf-attrib-input')).toBeNull()
  })

  it('neither says so: the edit controls show', async () => {
    await open({ header: { ...HEADER, writer: true } })
    click('@h:61')
    expect(screen.queryByTestId('dxf-view-only')).toBeNull()
    expect(screen.getByTestId('dxf-move-by')).toBeTruthy()
    expect(screen.getByTestId('dxf-attrib-input')).toBeTruthy()
  })
})

describe('when editing isn’t on', () => {
  it('a DWG drawing: view only, the header says why, no edit controls', async () => {
    await open({ header: { ...HEADER, format: 'dwg' } })
    expect(screen.getByTestId('dxf-view-only').textContent).toMatch(/^View only: DWG drawings can’t be changed yet\./)
    click('@h:41')
    expect(screen.getByTestId('dxf-edit-count').textContent).toBe('1 selected')
    expect(screen.queryByTestId('dxf-move-by')).toBeNull()
    click('@h:61')
    expect(screen.queryByTestId('dxf-attrib-input')).toBeNull()
    drag([5, 2], [15, 2], { target: shape('@h:61') })
    expect(screen.queryByTestId('dxf-ghost')).toBeNull()
    expect(world.edits.calls).toHaveLength(0)
  })

  it('before DXF 2000: the header says to save it again', async () => {
    await open({ header: { ...HEADER, acadver: 'AC1014' } })
    expect(screen.getByTestId('dxf-view-only').textContent).toMatch(/saved before DXF 2000/)
  })

  it('the viewer’s own reading (no model, so no handles): no selection, no panel, the left button pans', async () => {
    world = fakeCadWorld({ header: HEADER, pages: PAGES })
    renderInPrimaryRoom(<DxfViewer text={BRACKET} filePath={PATH} model={null} edits={world.edits} changes={null} />)
    const svg = await screen.findByTestId('dxf-svg')
    await screen.findByTestId('dxf-shapes')
    expect(screen.queryByTestId('dxf-edit-hint')).toBeNull()
    expect(screen.queryByTestId('dxf-view-only')).toBeNull()
    const before = svg.getAttribute('viewBox')
    fireEvent.pointerDown(sheet(), { pointerId: 1, button: 0, clientX: 100, clientY: 100 })
    fireEvent.pointerMove(sheet(), { pointerId: 1, clientX: 160, clientY: 100 })
    fireEvent.pointerUp(sheet(), { pointerId: 1, clientX: 160, clientY: 100 })
    expect(svg.getAttribute('viewBox')).not.toBe(before)
    expect(screen.queryByTestId('dxf-edit-panel')).toBeNull()
    expect(screen.queryByTestId('dxf-edit-marks')).toBeNull()
  })
})
