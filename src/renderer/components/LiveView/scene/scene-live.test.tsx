// @vitest-environment jsdom
// The scene tab on a real file: only when the daemon serves it live (its
// core on: live-doc-core `scene`), the shell over the real source on a
// scripted docs socket replaying the BLEND pack's golden. Real people, real
// changes, no Sample data banner; a file the daemon doesn't serve keeps the
// sample, banner and all; the connection reconnects and says so.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { act, cleanup, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { LiveClose, LiveServerFrame, LiveTransport, LiveWelcome } from '@/lib/docs-live-transport'
import { stateFor } from '@/lib/live-doc-core'
import { render as renderChip } from '@testing-library/react'
import { StateChip } from '../shell/StateChip'
import { backoffMs, keepConnected } from '@/lib/live-view/shared/live-connect'
import type { ServerScope } from '@/kessel/server-scope'

vi.mock('@/components/Room/RoomContext', () => ({ useOptionalRoom: () => null }))
const { default: SceneLive } = await import('./SceneLive')

type Step = { step: string; watch: Record<string, unknown>[] | null }
const STEPS = (JSON.parse(readFileSync(join(__dirname, '..', '..', '..', '..', '..', 'tests', 'fixtures', 'blend-watch', 'scene_turn.json'), 'utf8')) as { steps: Step[] }).steps
const member = (handle: string, name: string, kind: string, colour: string, client: number) => ({ handle, name, colour, kind, role: 'viewer', away: false, cursor: null, rights: 'edit', devices: [{ client, cursor: null }] })
const ROSTER = { kind: 'snapshot', version: 1, members: [member('ada', 'Ada', 'person', '#2563eb', 1), member('atlas', 'Atlas', 'agent', '#d9480f', 2)], watching: 0 }
const WELCOME = { root: '', version: 0, client: 1, self: 'ada', rights: 'edit', features: ['keyed_watch', 'view_address', 'presence'] } as unknown as LiveWelcome

/** A scripted docs socket: connect resolves with the welcome; the test pushes frames and closes. */
function scripted(welcome: LiveWelcome = WELCOME): LiveTransport & { push: (f: unknown) => void; drop: (c: LiveClose) => void; connects: number } {
  const frames = new Set<(f: LiveServerFrame) => void>()
  const closes = new Set<(c: LiveClose) => void>()
  const t = {
    connects: 0,
    connect: async () => {
      t.connects++
      return welcome
    },
    send: () => undefined,
    onFrame: (cb: (f: LiveServerFrame) => void) => (frames.add(cb), () => frames.delete(cb)),
    onClose: (cb: (c: LiveClose) => void) => (closes.add(cb), () => closes.delete(cb)),
    close: () => undefined,
    push: (f: unknown) => {
      for (const cb of [...frames]) cb(f as LiveServerFrame)
    },
    drop: (c: LiveClose) => {
      for (const cb of [...closes]) cb(c)
    },
  }
  return t as unknown as LiveTransport & { push: (f: unknown) => void; drop: (c: LiveClose) => void; connects: number }
}

beforeEach(() => {
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null)
})
afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('the daemon’s word', () => {
  it('liveView false with on true still opens the real source (liveView only means a live text editor)', () => {
    expect(stateFor({ core: 'blend', on: true, liveView: false })).toEqual({ status: 'scene' })
    expect(stateFor({ core: 'blend', on: true, liveView: true })).toEqual({ status: 'scene' })
  })

  it('a .blend whose core is on is a live scene; off, the sample', () => {
    expect(stateFor({ core: 'blend', on: true, liveView: false })).toEqual({ status: 'scene' })
    expect(stateFor({ core: 'blend', on: false, liveView: false })).toEqual({ status: 'off' })
  })
})

describe('the scene tab on a real file', () => {
  it('real people and a change, and no Sample data banner', async () => {
    const t = scripted()
    render(<SceneLive scope={{} as ServerScope} filePath="/ws/scenes/set.blend" width={1440} createTransport={() => t} />)
    await waitFor(() => expect(t.connects).toBe(1))
    await act(async () => {
      t.push({ type: 'roster_event', event: ROSTER })
      for (const f of STEPS[0].watch!) t.push(f)
      for (const f of STEPS.find((s) => s.step === 'place Table')!.watch!) t.push(f)
    })
    expect(screen.queryByTestId('lv-sample-banner')).toBeNull()
    expect(screen.getByTestId('lv-shell-header').textContent).toContain('set')
    expect(screen.getByRole('group', { name: /Who’s here/ }).textContent).toContain('A')
    expect(screen.getByTestId('lv-scene-section-activity').textContent).toContain('Atlas moved and turned Table')
    // the outliner is the file's: its collections and objects
    expect(screen.getByRole('tree', { name: /Collections and objects/ }).textContent).toContain('Table')
    // only real people: the sample's Brook and Sam are nowhere
    expect(document.body.textContent).not.toMatch(/Brook|Sam\b/)
  })
})

describe('keeping it connected', () => {
  it('a retryable drop says reconnecting and dials again after the backoff; declined is the source’s to word', async () => {
    expect([backoffMs(0), backoffMs(1), backoffMs(10)]).toEqual([1000, 2000, 30000])
    const t = scripted()
    const timers: (() => void)[] = []
    const src = { attach: vi.fn(), reconnected: vi.fn(), setConnection: vi.fn(), refusedAtOpen: vi.fn(), onRetryAsked: vi.fn(), close: vi.fn() }
    keepConnected(t, src, { schedule: (fn) => (timers.push(fn), () => undefined) })
    await waitFor(() => expect(src.attach).toHaveBeenCalledTimes(1))
    t.drop({ reason: 'network', retry: true } as LiveClose)
    expect(src.setConnection).toHaveBeenCalledWith('reconnecting')
    await act(async () => timers.shift()!())
    await waitFor(() => expect(src.reconnected).toHaveBeenCalledTimes(1))
    t.drop({ reason: 'declined', retry: false, code: 'live_version', message: 'a version the pack doesn’t read' } as LiveClose)
    expect(src.refusedAtOpen).toHaveBeenCalledWith(expect.objectContaining({ id: 'live_version' }))
  })
})

describe('a .blend this release can’t save (the BLEND writer off)', () => {
  it('the header says Read only from the start, the why in its tooltip; People promises no holds; Follow and Say stay', async () => {
    const t = scripted({ ...WELCOME, rights: 'read', read_only_reason: 'K2 can’t save .blend edits yet; this file opens read-only.' } as unknown as LiveWelcome)
    render(<SceneLive scope={{} as ServerScope} filePath="/ws/scenes/set.blend" width={1440} createTransport={() => t} />)
    await waitFor(() => expect(t.connects).toBe(1))
    await act(async () => {
      t.push({ type: 'roster_event', event: ROSTER })
      for (const f of STEPS[0].watch!) t.push(f)
    })
    const chip = screen.getByTestId('lv-state')
    expect(chip.getAttribute('data-state')).toBe('read-only-file')
    expect(chip.textContent).toBe('Read only')
    expect(chip.getAttribute('title')).toBe('Read only: K2 can’t save .blend edits yet; this file opens read-only.')
    expect(screen.getByTestId('lv-scene-head-people').textContent).toContain('People')
    expect(screen.getByTestId('lv-scene-head-people').textContent).not.toContain('holds')
    expect(screen.getAllByRole('button', { name: /Follow Atlas/ }).length).toBeGreaterThan(0)
  })

  it('a short why rides after the word (the timeline’s .fcpxml); a long one only in the tooltip', () => {
    const a = renderChip(<StateChip state="read-only-file" why="K2 writes back .otio only" />)
    expect(a.container.textContent).toBe('Read only: K2 writes back .otio only')
    a.unmount()
    const b = renderChip(<StateChip state="read-only-file" why="K2 can’t save .blend edits yet; this file opens read-only." />)
    expect(b.container.textContent).toBe('Read only')
  })
})

describe('the BLEND writer on', () => {
  it('write and claims in the welcome: no read-only chip, and the section is “People and holds”', async () => {
    const t = scripted({ ...WELCOME, features: ['keyed_watch', 'view_address', 'presence', 'claims'] } as unknown as LiveWelcome)
    render(<SceneLive scope={{} as ServerScope} filePath="/ws/scenes/set.blend" width={1440} createTransport={() => t} />)
    await waitFor(() => expect(t.connects).toBe(1))
    await act(async () => {
      t.push({ type: 'roster_event', event: ROSTER })
      for (const f of STEPS[0].watch!) t.push(f)
    })
    expect(screen.getByTestId('lv-state').getAttribute('data-state')).not.toBe('read-only-file')
    expect(screen.getByTestId('lv-scene-head-people').textContent).toContain('People and holds')
  })
})

describe('a file refused at open', () => {
  it('the stage holds only the card; the panel keeps Activity and People, with no Outliner or Properties to mislead', async () => {
    const t = scripted()
    render(<SceneLive scope={{} as ServerScope} filePath="/ws/scenes/set.blend" width={1440} createTransport={() => t} />)
    await waitFor(() => expect(t.connects).toBe(1))
    await act(async () => {
      t.push({ type: 'watch_refused', error: { id: 'MPC-E407', message: 'live editing is off: … (format 5.02) … (live_version)', details: { code: 'live_version' } } })
    })
    expect(screen.getByTestId('lv-scene-card').textContent).toContain('This file is from a newer Blender.')
    expect(screen.queryByRole('button', { name: /Change view/ })).toBeNull()
    expect(document.querySelector('.lv-keys')).toBeNull()
    expect(document.querySelector('.lv-infob')).toBeNull()
    expect(screen.queryByTestId('lv-scene-section-outliner')).toBeNull()
    expect(screen.queryByTestId('lv-scene-section-props')).toBeNull()
    expect(screen.getByTestId('lv-scene-section-activity')).toBeTruthy()
    expect(screen.getByTestId('lv-scene-section-people')).toBeTruthy()
    expect(document.body.textContent).not.toContain('No objects yet')
  })
})
