// @vitest-environment jsdom
// Follow on the scene (S5), with the shell's follow state machine driving
// it as useLiveShell does: the picture shows whom you follow's view;
// choosing a view yourself looks away (Back to them); the sample's scripted
// follows; when they stop, "Atlas stopped. Stopped following." with Back
// to my place and Follow again; when they leave, "Sam left the file.
// Stopped following."; the say row points at the view on screen in K2
// Render's grammar; a chip for whoever looks through another view follows
// them; two views; the turntable strip.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { useReducer } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { SCENARIOS } from '@/lib/live-view/scene/scenarios-more'
import { simulate, TURN } from '@/lib/live-view/scene/turn'
import { stillCamera } from '@/lib/live-view/scene/project'
import { standInCamera } from '@/lib/live-view/scene/stand-in/camera'
import { sampleBounds } from '@/lib/live-view/scene/stand-in/shapes'
import { SHAPE_ASPECT, stillHeight, type PictureSource } from '@/lib/live-view/scene/picture-source'
import { parseView } from '@/lib/live-view/scene/view-address'
import type { SimState } from '@/lib/live-view/scene/sim'
import { sceneSnapshot } from '@/lib/live-view/scene/sample-source'
import { FOLLOW_IDLE, followReducer, type FollowAction, type FollowState } from '@/lib/live-view/shared/follow'
import type { SayAt } from '../shell/shell-config'
import { SceneStage } from './SceneStage'

const sc = (id: string) => SCENARIOS.find((s) => s.id === id)!
const idx = (id: string, label: string): number => {
  const i = sc(id).steps.findIndex((x) => x.label === label)
  expect(i, `${id}: ${label}`).toBeGreaterThanOrEqual(0)
  return i
}

function cameraSource(S: () => SimState): PictureSource {
  return {
    kind: 'sample',
    still: (req) => Promise.resolve({ rev: req.rev, view: req.view, shape: req.shape, width: req.width, height: stillHeight(req.width, req.shape), image: { kind: 'rgba', data: new Uint8ClampedArray(4) }, camera: stillCamera(standInCamera(S().revDocs[req.rev] ?? S().D, req.view, SHAPE_ASPECT[req.shape])), ids: null, renderMs: 1 }),
    bounds: sampleBounds,
    close: () => undefined,
  }
}

let said: (SayAt | null)[] = []
let dispatched: FollowAction[] = []
let current: SimState
let state: FollowState = FOLLOW_IDLE

/** The stage with the shell's follow reducer, as useLiveShell runs it. */
function Host({ S, source, initial }: { S: SimState; source: PictureSource; initial: FollowState }): React.JSX.Element {
  const people = sceneSnapshot(S, sc('turn'), 0).people
  const [f, d] = useReducer((s: FollowState, a: FollowAction) => followReducer(s, a, people), initial)
  state = f
  const dispatch = (a: FollowAction): void => {
    dispatched.push(a)
    d(a)
  }
  // The shell sees whom you follow leave.
  const gone = (f.kind === 'following' || f.kind === 'away') && !people.some((p) => p.id === f.who)
  if (gone) queueMicrotask(() => d({ type: 'left', who: f.who, name: 'Sam', now: Date.now() }))
  return <SceneStage S={S} control={{ retryPicture: () => undefined }} source={source} narrow={false} size={{ width: 1100, height: 700 }} offscreen={[]} onJump={() => undefined} follow={{ id: f.kind === 'following' ? f.who : null, state: f, dispatch }} setSayAt={(a) => said.push(a)} people={people} />
}

function mount(id: string, step: number, initial: FollowState = FOLLOW_IDLE) {
  current = simulate(sc(id), step)
  const source = cameraSource(() => current)
  const r = render(<Host S={current} source={source} initial={initial} />)
  return {
    ...r,
    step: (n: number) => {
      current = simulate(sc(id), n)
      r.rerender(<Host S={current} source={source} initial={initial} />)
    },
  }
}
const stage = () => screen.getByTestId('lv-scene-stage')
const settle = () => act(async () => undefined)

beforeEach(() => {
  said = []
  dispatched = []
  state = FOLLOW_IDLE
  const h = document.documentElement
  h.setAttribute('data-style', 'glass')
  h.setAttribute('data-palette', 'obsidian')
  h.setAttribute('data-scheme', 'dark')
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null)
})
afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('following on the picture', () => {
  it('the sample’s Follow Atlas: the picture is the view Atlas looks through', async () => {
    mount('follow', idx('follow', 'follow Atlas'))
    await settle()
    expect(dispatched).toContainEqual({ type: 'follow', who: 'agent:atlas' })
    expect(stage().getAttribute('data-following')).toBe('agent:atlas')
    expect(stage().getAttribute('data-view')).toBe('@obj:Table')
  })

  it('choosing a view yourself looks away; the shell then offers Back to them', async () => {
    mount('follow', idx('follow', 'say something'), { kind: 'following', who: 'agent:atlas' })
    await settle()
    fireEvent.keyDown(stage(), { key: 'Home' })
    await settle()
    expect(dispatched).toContainEqual({ type: 'look-away' })
    expect(state.kind).toBe('away')
    expect(stage().getAttribute('data-view')).toBe('@scene:main')
  })

  it('the say row points at the view on screen, in K2 Render’s grammar', async () => {
    mount('follow', idx('follow', 'say something'), { kind: 'following', who: 'agent:atlas' })
    await settle()
    const last = said.filter(Boolean).pop()!
    expect(last).toMatchObject({ label: 'Table, framed', address: '@obj:Table' })
    expect(parseView(last.address)).not.toBeNull()
  })

  it('when Atlas’s turn ends: “Atlas stopped. Stopped following.”, Back to my place and Follow again', async () => {
    const end = TURN.steps.findIndex((s) => s.events.some((e) => e.kind === 'turn_end'))
    current = simulate(TURN, end)
    const source = cameraSource(() => current)
    render(<Host S={current} source={source} initial={{ kind: 'following', who: 'agent:atlas' }} />)
    await settle()
    const fin = screen.getByTestId('lv-finished')
    expect(within(fin).getByText('Atlas stopped. Stopped following.')).toBeTruthy()
    expect(state.kind).toBe('idle')
    expect(within(fin).getByRole('button', { name: 'Back to my place' })).toBeTruthy()
    // The two ways on stay together as a pair (they wrap under the sentence as one).
    const pair = within(fin).getByRole('group', { name: 'After following' })
    expect(within(pair).getAllByRole('button').map((b) => b.textContent)).toEqual(['Back to my place', 'Follow again'])
    fireEvent.click(within(fin).getByRole('button', { name: 'Follow again' }))
    await settle()
    expect(state).toEqual({ kind: 'following', who: 'agent:atlas' })
    expect(screen.queryByTestId('lv-finished')).toBeNull()
  })

  it('when Sam leaves: “Sam left the file. Stopped following.”, Back to my place, no Follow again', async () => {
    mount('follow', idx('follow', 'Sam leaves'), { kind: 'following', who: 'person:sam' })
    await settle()
    await settle()
    const fin = screen.getByTestId('lv-finished')
    expect(within(fin).getByText('Sam left the file. Stopped following.')).toBeTruthy()
    expect(within(fin).getByRole('button', { name: 'Back to my place' })).toBeTruthy()
    expect(within(fin).queryByRole('button', { name: 'Follow again' })).toBeNull()
    // You keep the view Sam had.
    expect(stage().getAttribute('data-view')).toBe('@cam:Camera.001')
  })
})

describe('people on other views', () => {
  it('a chip by the picker for whoever looks through another view; it follows them', async () => {
    mount('who', 0)
    await settle()
    const chip = screen.getByRole('button', { name: 'Brook is looking through Camera 2. Follow Brook' })
    expect(chip.hasAttribute('data-k2-focus')).toBe(true)
    fireEvent.click(chip)
    await settle()
    expect(state).toEqual({ kind: 'following', who: 'agent:brook' })
    expect(stage().getAttribute('data-view')).toBe('@cam:Camera.001')
  })
})

describe('two views', () => {
  it('the second follows the agent’s view under its own picker, one above the other at this size', async () => {
    mount('two', 0)
    await settle()
    expect(stage().getAttribute('data-layout')).toBe('stacked')
    const second = screen.getByRole('button', { name: /^Second view: Atlas’s view/ })
    expect(second).toBeTruthy()
    expect(document.querySelector('.lv-pane[data-pane="1"] .lv-plabel')?.textContent).toBe('Table, framed')
    // The second view's still covers its half: no stage shows beside it.
    const frame = document.querySelector<HTMLElement>('.lv-pane[data-pane="1"] .lv-frame')!
    expect(frame.hasAttribute('data-cover')).toBe(true)
    expect(parseFloat(frame.style.left)).toBeLessThanOrEqual(0)
    expect(parseFloat(frame.style.left) + parseFloat(frame.style.width)).toBeGreaterThanOrEqual(1100 - 0.5)
    fireEvent.click(screen.getByRole('button', { name: 'Show one view' }))
    await settle()
    expect(stage().getAttribute('data-layout')).toBe('one')
  })
})

describe('labels and the menus', () => {
  it('while the views menu lies over the picture, its labels wait', async () => {
    mount('keys', 0)
    await settle()
    expect(screen.getByRole('menu', { name: 'Views' })).toBeTruthy()
    expect(screen.queryAllByTestId('lv-tag')).toEqual([])
  })
})

describe('the turntable', () => {
  it('eight stills at the picture’s foot; a step with the keys turns it', async () => {
    mount('turntable', 0)
    await settle()
    const strip = screen.getByRole('slider', { name: /Turntable: 8 still pictures/ })
    expect(strip.getAttribute('aria-valuenow')).toBe('1')
    fireEvent.keyDown(strip, { key: 'ArrowRight' })
    await settle()
    expect(stage().getAttribute('data-view')).toBe('@turntable:1')
    expect(screen.getByText('Still 2 of 8 · drag to turn')).toBeTruthy()
  })
})
