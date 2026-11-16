// The generic sample player: scenarios of steps; play, pause, step, start
// over; status for the banner. And the sample shell source built on it.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { SamplePlayer } from './sample-player'
import { SampleShellSource } from './sample-shell'
import { SHELL_SCENARIOS, shellSample } from './samples'

beforeEach(() => vi.useFakeTimers())
afterEach(() => vi.useRealTimers())

const scenarios = [
  { id: 'a', label: 'A', steps: [{ label: 'one', state: 1 }, { label: 'two', state: 2 }, { label: 'three', state: 3 }] },
  { id: 'b', label: 'B', steps: [{ label: 'only', state: 9 }] },
]

describe('SamplePlayer', () => {
  it('starts on the first step, paused when asked', () => {
    const p = new SamplePlayer({ scenarios, autoplay: false })
    expect(p.current()).toBe(1)
    expect(p.status()).toEqual({ playing: false, position: 1, length: 3, label: 'one', scenario: 'a' })
  })

  it('steps, stops at the end without looping, and starts over', () => {
    const p = new SamplePlayer({ scenarios, autoplay: false })
    const seen: number[] = []
    p.onState((s) => seen.push(s))
    p.step()
    p.step()
    p.step()
    expect(seen).toEqual([2, 3])
    expect(p.status().position).toBe(3)
    p.restart()
    expect(p.current()).toBe(1)
  })

  it('plays on a timer, and pauses', () => {
    const p = new SamplePlayer({ scenarios, autoplay: true, stepMs: 1000 })
    expect(p.status().playing).toBe(true)
    vi.advanceTimersByTime(1000)
    expect(p.current()).toBe(2)
    p.pause()
    vi.advanceTimersByTime(5000)
    expect(p.current()).toBe(2)
    p.play()
    vi.advanceTimersByTime(1000)
    expect(p.current()).toBe(3)
    vi.advanceTimersByTime(1000)
    expect(p.status().playing).toBe(false)
    p.close()
  })

  it('switches scenario from its start', () => {
    const p = new SamplePlayer({ scenarios, autoplay: false })
    p.step()
    p.setScenario('b')
    expect(p.status()).toMatchObject({ scenario: 'b', position: 1, length: 1, label: 'only' })
    expect(p.current()).toBe(9)
  })
})

describe('the sample shell source', () => {
  it('opens on “first open” for both kinds, a LiveSource of kind sample', () => {
    for (const kind of ['scene', 'timeline'] as const) {
      const src = new SampleShellSource(shellSample(kind), { autoplay: false })
      expect(src.kind).toBe('sample')
      expect(src.snapshot().tier).toBe('loading')
      expect(src.player.status().scenario).toBe('open')
      src.close()
    }
  })

  it('has the scenarios the shell needs: people, outside saves, connection', () => {
    expect(SHELL_SCENARIOS.map((s) => s.id)).toEqual(['open', 'people', 'saves', 'connection'])
    const sample = shellSample('timeline')
    const saves = sample.scenarios.find((s) => s.id === 'saves')!.steps.map((s) => s.state.outsideSave?.outcome)
    expect(saves).toEqual(['ingested', 'kept_both', 'declined', 'missing'])
    const conn = sample.scenarios.find((s) => s.id === 'connection')!.steps.map((s) => s.state.connection)
    expect(conn).toEqual(['reconnecting', 'trying', 'cant', 'offline', 'back'])
  })

  it('tells subscribers each new snapshot; retry moves a connection step on', () => {
    const src = new SampleShellSource(shellSample('scene'), { autoplay: false })
    const seen: string[] = []
    src.subscribe((s) => seen.push(s.tier))
    src.player.setScenario('connection')
    expect(src.snapshot().connection).toBe('reconnecting')
    src.retry()
    expect(src.snapshot().connection).toBe('trying')
    expect(seen.length).toBe(2)
    src.close()
  })

  it('answers a say: sent, or failed on a step that fails sends', async () => {
    const src = new SampleShellSource(shellSample('timeline'), { autoplay: false })
    src.player.setScenario('people')
    await expect(src.say({ to: 'atlas', text: 'hi', address: null })).resolves.toEqual({ ok: true })
    src.player.setScenario('connection')
    await expect(src.say({ to: 'atlas', text: 'hi', address: null })).resolves.toEqual({ ok: false })
    src.close()
  })
})
