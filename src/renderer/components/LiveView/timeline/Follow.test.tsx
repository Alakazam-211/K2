// @vitest-environment jsdom
// Follow on the timeline (T5), on the watched edit: Follow Atlas puts
// Atlas's playhead in the monitor; any watching key, a scroll of the lanes
// or a loop looks away (your playhead starts where Atlas's was) and Back to
// Atlas returns; a change's bracket on the ruler loops it; when Atlas stops
// or Sam leaves, following ends with the editors' finished words; and the
// say row points at the clip under the playhead.

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { SampleShellSource } from '@/lib/live-view/shared/sample-shell'
import { modelOf } from '@/lib/live-view/timeline/fold'
import { headOf } from '@/lib/live-view/timeline/playheads'
import { sampleSnapshot } from '@/lib/live-view/timeline/sample-source'
import { WATCH } from '@/lib/live-view/timeline/scenarios'
import { nowOf } from '@/lib/live-view/timeline/snapshot'
import { ATLAS } from '@/lib/live-view/timeline/world'
import TimelineSample from './TimelineSample'

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true })
  try {
    localStorage.clear()
  } catch {
    /* none */
  }
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.restoreAllMocks()
})

async function mount(width = 1440): Promise<void> {
  render(<TimelineSample width={width} />)
  await act(async () => {})
}
const at = (label: string): number => WATCH.steps.findIndex((s) => s.label === label)
/** Play the watched edit up to a step, one step at a time (the shell keeps whom you follow between steps). */
const upTo = (label: string): void => {
  for (let i = 0; i < at(label); i++) fireEvent.click(screen.getByTestId('lv-sample-step'))
}
const who = (): string => screen.getByTestId('lv-transport-who').textContent ?? ''
const counter = (): string => screen.getByTestId('lv-counter').textContent ?? ''
const atlasTc = (label: string): string => {
  const snap = sampleSnapshot(WATCH, at(label))
  const S = snap.timeline.state
  const M = modelOf(S.B)
  return M.tcF(headOf(M, S.peers[ATLAS], nowOf(S))!.f)
}

describe('Follow on the timeline', () => {
  it('Follow Atlas: the monitor shows Atlas’s playhead, and the header says whom you follow', async () => {
    await mount()
    upTo('follow Atlas')
    expect(screen.getByTestId('lv-follow').textContent).toContain('Following Atlas')
    expect(who()).toBe('Atlas’s playhead')
    expect(counter()).toBe(atlasTc('follow Atlas'))
  })

  it('a watching key looks away from where Atlas was; Back to Atlas returns', async () => {
    await mount()
    upTo('follow Atlas')
    fireEvent.keyDown(screen.getByTestId('lv-monitor'), { key: 'ArrowRight' })
    expect(who()).toBe('Your playhead')
    const back = screen.getByTestId('lv-back')
    expect(back.textContent).toContain('Back to Atlas')
    expect(screen.getByTestId('lv-transport-right').contains(back)).toBe(true)
    expect(counter()).not.toBe(atlasTc('follow Atlas'))
    fireEvent.click(back)
    expect(who()).toBe('Atlas’s playhead')
    expect(counter()).toBe(atlasTc('follow Atlas'))
  })

  it('a scroll of your own on the lanes stops following', async () => {
    await mount()
    upTo('follow Atlas')
    fireEvent.wheel(screen.getByTestId('lv-lanes-scroll').querySelector('[data-scroll-edges-list]') as HTMLElement, { deltaX: 120 })
    expect(screen.getByTestId('lv-back').textContent).toContain('Back to Atlas')
    expect(who()).toBe('Your playhead')
  })

  it('a change’s bracket on the ruler loops it, leaving Follow; Back to Atlas ends the loop', async () => {
    await mount()
    upTo('tile in')
    expect(who()).toBe('Atlas’s playhead')
    const brackets = screen.getAllByTestId('lv-bracket')
    fireEvent.click(brackets[brackets.length - 1])
    expect(who()).toBe('Looping Atlas’s trim · Esc to stop')
    expect(screen.getByTestId('lv-ruler-loop')).toBeTruthy()
    fireEvent.click(screen.getByTestId('lv-back'))
    expect(who()).toBe('Atlas’s playhead')
    expect(screen.queryByTestId('lv-ruler-loop')).toBeNull()
  })

  it('the sample’s scripted loop and its Back', async () => {
    await mount()
    upTo('loop it')
    expect(who()).toBe('Looping Atlas’s trim · Esc to stop')
    expect(screen.getByTestId('lv-back').textContent).toContain('Back to Atlas')
  })

  it('when Atlas stops: “Atlas stopped. Stopped following.”, Back to my place and Follow again', async () => {
    await mount()
    upTo('Atlas lets go')
    expect(screen.getByTestId('lv-finished').textContent).toBe('Atlas stopped. Stopped following.')
    // you stay where Atlas was
    expect(counter()).toBe(atlasTc('Atlas lets go'))
    expect(screen.getByTestId('lv-follow').textContent).toContain('Follow Atlas')
    expect(screen.getByRole('button', { name: 'Back to my place' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Follow again' }))
    expect(screen.queryByTestId('lv-finished')).toBeNull()
    expect(who()).toBe('Atlas’s playhead')
  })

  it('at 800 the finished line and its buttons take their short words, their names whole', async () => {
    await mount(800)
    upTo('Atlas lets go')
    expect(screen.getByTestId('lv-finished').textContent).toBe('Atlas stopped.')
    expect(screen.getByTestId('lv-finished').getAttribute('title')).toBe('Atlas stopped. Stopped following.')
    expect(screen.getByRole('button', { name: 'Back to my place' }).textContent).toBe('Back')
    expect(screen.getByRole('button', { name: 'Follow again' }).textContent).toBe('Follow')
  })

  it('Back to my place puts your playhead where it was before you followed', async () => {
    await mount()
    upTo('Atlas holds')
    const mine = counter()
    upTo('Atlas lets go') // continues from step 2: follow at 3 … stop at 12
    fireEvent.click(screen.getByRole('button', { name: 'Back to my place' }))
    expect(who()).toBe('Your playhead')
    expect(counter()).toBe(mine)
  })

  it('when Sam leaves: “Sam left the file. Stopped following.”, and no Follow again', async () => {
    await mount()
    upTo('Sam left')
    expect(screen.getByTestId('lv-finished').textContent).toBe('Sam left the file. Stopped following.')
    expect(screen.getByRole('button', { name: 'Back to my place' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Follow again' })).toBeNull()
  })

  it('the say row points at the clip under the playhead, and the message carries its address and time', async () => {
    const say = vi.spyOn(SampleShellSource.prototype, 'say')
    await mount()
    upTo('back to Atlas')
    const chip = screen.getByTestId('lv-say-chip')
    expect(chip.textContent).toBe(screen.getByTestId('lv-mon-label').textContent?.replace(/^V\d/, ''))
    fireEvent.change(screen.getByTestId('lv-say-field'), { target: { value: 'Tighter here?' } })
    fireEvent.keyDown(screen.getByTestId('lv-say-field'), { key: 'Enter' })
    expect(say).toHaveBeenCalledWith(expect.objectContaining({ to: ATLAS, text: 'Tighter here?', address: expect.stringMatching(/^@item:\d+$/), time: { at: expect.stringMatching(/^\d+$/), in: '@d-seq:1' } }))
  })
})
