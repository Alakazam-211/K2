// @vitest-environment jsdom
// The monitor and transport on the timeline's sample (T4): the layout
// (monitor, transport, say row, then the lanes full width under the panel),
// Play on the picture's centre line, every monitor state with its words and
// actions, details, the watching keys, and the scrub bar.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { onYourPlayhead, sampleSnapshot } from '@/lib/live-view/timeline/sample-source'
import { STATES, WATCH } from '@/lib/live-view/timeline/scenarios'
import { frameRect } from './Monitor'
import { MONITOR_CSS } from './monitorCss'
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
})

async function mount(width = 1440): Promise<void> {
  render(<TimelineSample width={width} />)
  await act(async () => {})
}
const step = (n: number): void => {
  for (let i = 0; i < n; i++) fireEvent.click(screen.getByTestId('lv-sample-step'))
}
function pick(id: string): void {
  fireEvent.click(within(screen.getByTestId('lv-sample-scenarios')).getByRole('button'))
  fireEvent.click(document.querySelector(`[data-setting-dropdown-menu] button[data-value="${id}"]`) as HTMLElement)
}
const MS = onYourPlayhead(STATES, 'states')
const at = (label: string): number => MS.steps.findIndex((s) => s.label === label)
/** The states scenario at a step, by its label. */
function states(label: string): void {
  pick('states')
  step(at(label))
}
const mon = (): HTMLElement => screen.getByTestId('lv-monitor')
const note = (): string | null => screen.queryByTestId('lv-mon-note')?.textContent ?? null
const who = (): string => screen.getByTestId('lv-transport-who').textContent ?? ''
const counter = (): string => screen.getByTestId('lv-counter').textContent ?? ''
const key = (k: string, el: Element = mon()): void => {
  fireEvent.keyDown(el, { key: k })
}

describe('the layout, as the mock', () => {
  it('monitor, transport, say row, then the lanes; the lanes run full width under the side panel', async () => {
    await mount()
    const hero = screen.getByTestId('lv-hero')
    const order = Array.from(hero.querySelectorAll('[data-testid="lv-monitor"], [data-testid="lv-transport"], [data-testid="lv-say"], [data-testid="lv-lanes"]')).map((e) => e.getAttribute('data-testid'))
    expect(order).toEqual(['lv-monitor', 'lv-transport', 'lv-say', 'lv-lanes'])
    const upper = screen.getByTestId('lv-upper')
    expect(upper.contains(screen.getByTestId('lv-panel'))).toBe(true)
    expect(upper.contains(screen.getByTestId('lv-say'))).toBe(true)
    expect(upper.contains(screen.getByTestId('lv-lanes'))).toBe(false)
    expect(screen.getByTestId('lv-lanes').parentElement).toBe(hero)
    // one say row and one panel: the frame holds neither
    expect(screen.getAllByTestId('lv-say')).toHaveLength(1)
    expect(screen.getAllByTestId('lv-panel')).toHaveLength(1)
  })

  it('View and zoom live in the transport row', async () => {
    await mount()
    const right = screen.getByTestId('lv-transport-right')
    for (const id of ['lv-view-btn', 'lv-zoom-in', 'lv-zoom-out', 'lv-zoom-fit', 'lv-loop', 'lv-mode', 'lv-keys-hint']) expect(right.contains(screen.getByTestId(id)), id).toBe(true)
  })
})

describe('Play on the picture’s centre line', () => {
  it('the picture is centred in its well at every size', () => {
    for (const [W, H] of [
      [1180, 420],
      [760, 300],
      [500, 600],
      [1555, 389],
    ]) {
      const r = frameRect(W, H)
      expect(Math.abs(r.x + r.w / 2 - W / 2)).toBeLessThanOrEqual(0.5)
      expect(Math.abs(r.w / r.h - 16 / 9)).toBeLessThan(0.02)
    }
  })

  it('the transport row has equal sides, and Play is the middle of a mirrored group, in the monitor’s own column', async () => {
    await mount()
    expect(MONITOR_CSS).toMatch(/\.lv-tl-trow \{[^}]*grid-template-columns: minmax\(0, 1fr\) auto minmax\(0, 1fr\)/)
    const row = screen.getByTestId('lv-transport-row')
    expect(Array.from(row.children).map((c) => c.className.split(' ')[0])).toEqual(['lv-tl-tleft', 'lv-tl-tbtns', 'lv-tl-tright'])
    const btns = Array.from(screen.getByTestId('lv-transport-buttons').children)
    const mid = btns.indexOf(screen.getByTestId('lv-play'))
    expect(mid).toBe((btns.length - 1) / 2)
    for (let i = 0; i < mid; i++) expect(btns[i].getAttribute('data-size')).toBe(btns[btns.length - 1 - i].getAttribute('data-size'))
    expect(screen.getByTestId('lv-monitor').parentElement).toBe(screen.getByTestId('lv-transport').parentElement)
  })
})

describe('every monitor state, with its words and actions', () => {
  it('paused on the nearest picture; the label, Sample data and details on the picture', async () => {
    await mount()
    states('still')
    expect(mon().getAttribute('data-state')).toBe('paused')
    expect(note()).toMatch(/^Nearest picture: \d\.\d s before$/)
    expect(screen.getByTestId('lv-mon-label').textContent).toBe('V1Interview C')
    expect(screen.getByTestId('lv-mon-sample').textContent).toBe('Sample data')
    expect(screen.getByTestId('lv-mon-info').getAttribute('aria-label')).toBe('Details')
    expect(who()).toBe('Your playhead')
  })

  it('loading, playing, seeking, ready, waiting', async () => {
    await mount()
    states('loading')
    expect(mon().getAttribute('data-state')).toBe('loading')
    expect(note()).toBe('Loading…')
    step(1) // playing
    expect(mon().getAttribute('data-state')).toBe('playing')
    expect(screen.getByTestId('lv-play').getAttribute('aria-label')).toBe('Pause')
    expect(note()).toBeNull()
    step(2) // seeking
    expect(mon().getAttribute('data-state')).toBe('seeking')
    expect(note()).toBe('Seeking…')
    step(1) // sharp: the exact frame
    expect(mon().getAttribute('data-state')).toBe('paused')
    expect(mon().hasAttribute('data-exact')).toBe(true)
    expect(note()).toBeNull()
    step(1) // waiting
    expect(mon().getAttribute('data-state')).toBe('waiting')
    expect(note()).toBe('Waiting for video…')
  })

  it('stills only: the mode word reads Stills and says why; Play is off', async () => {
    await mount()
    states('stills only')
    expect(mon().getAttribute('data-state')).toBe('stills')
    const mode = screen.getByTestId('lv-mode')
    expect(mode.textContent).toBe('Stills')
    expect(mode.getAttribute('title')).toBe('This computer can’t play the preview video, so you see stills.')
    expect((screen.getByTestId('lv-play') as HTMLButtonElement).disabled).toBe(true)
  })

  it('the cards: not a video, no preview, offline, nest, and the gap, each with Skip to the next clip', async () => {
    await mount()
    states('not a video')
    let card = screen.getByTestId('lv-mon-card')
    expect(card.textContent).toContain('Map still')
    expect(card.textContent).toContain('Image')
    step(1)
    card = screen.getByTestId('lv-mon-card')
    expect(mon().getAttribute('data-state')).toBe('no_preview')
    expect(card.textContent).toContain('Interview CThis clip can’t be previewed.')
    fireEvent.click(within(card).getByRole('button', { name: 'Skip to the next clip' }))
    expect(who()).toMatch(/^Skipped to /)
    expect(mon().getAttribute('data-state')).not.toBe('no_preview')
    step(2) // offline
    expect(screen.getByTestId('lv-mon-card').textContent).toContain('This clip’s media isn’t available right now.')
    step(1) // nest
    expect(screen.getByTestId('lv-mon-card').textContent).toContain('A nested timeline: the preview shows cuts at the top level only.')
    step(2) // gap
    const gap = screen.getByTestId('lv-mon-gap')
    expect(gap.textContent).toContain('Gap: nothing on the video tracks here')
    expect(within(gap).getByRole('button', { name: 'Skip to the next clip' })).toBeTruthy()
  })

  it('the end: “End of the edit” with Play from the start, which plays from the start', async () => {
    await mount()
    states('end')
    expect(mon().getAttribute('data-state')).toBe('ended')
    expect(note()).toContain('End of the edit')
    expect(who()).toBe('Your playhead · end of the edit')
    expect(screen.getByTestId('lv-play').getAttribute('title')).toBe('Play from the start (Space)')
    fireEvent.click(screen.getByTestId('lv-mon-restart'))
    expect(mon().getAttribute('data-state')).toBe('playing')
    expect(screen.getByTestId('lv-play').getAttribute('aria-label')).toBe('Pause')
  })

  it('preparing video: the count, Play waits and says why, the clips that can’t be previewed', async () => {
    await mount()
    pick('prep')
    expect(mon().getAttribute('data-state')).toBe('preparing')
    const prep = screen.getByTestId('lv-mon-prep')
    expect(prep.textContent).toMatch(/Preparing video · 3 of \d+ previewable clips ready/)
    const play = screen.getByTestId('lv-play') as HTMLButtonElement
    expect(play.disabled).toBe(true)
    expect(play.getAttribute('title')).toBe('Playback starts when the next part of the video is ready')
    fireEvent.click(screen.getByTestId('lv-mon-cant'))
    expect(within(prep).getByRole('button', { name: 'B-roll 05' })).toBeTruthy()
    step(5)
    expect(screen.getByTestId('lv-mon-prep').textContent).toContain('Video ready')
  })

  it('details: the numbers behind the picture; Esc closes and goes back to the ⓘ; a sheet at 800', async () => {
    await mount()
    states('no preview')
    fireEvent.click(screen.getByTestId('lv-mon-info'))
    const det = screen.getByTestId('lv-mon-det')
    expect(det.getAttribute('role')).toBe('dialog')
    // Veil's frost puts a frost box back in the flow unless it carries the absolute class
    expect(det.classList.contains('absolute')).toBe(true)
    const terms = Array.from(det.querySelectorAll('dt')).map((d) => d.textContent)
    expect(terms).toEqual(expect.arrayContaining(['File', 'Version', 'Frame rate', 'Preview', 'Source file', 'Why', 'Code']))
    expect(det.textContent).toContain('not_on_this_host')
    expect(det.hasAttribute('data-sheet')).toBe(false)
    expect(mon().contains(det)).toBe(true)
    fireEvent.keyDown(det, { key: 'Escape' })
    expect(screen.queryByTestId('lv-mon-det')).toBeNull()
    expect(document.activeElement).toBe(screen.getByTestId('lv-mon-info'))
    cleanup()
    await mount(800)
    states('still')
    fireEvent.click(screen.getByTestId('lv-mon-info'))
    const sheet = screen.getByTestId('lv-mon-det')
    expect(sheet.hasAttribute('data-sheet')).toBe(true)
    expect(screen.getByTestId('lv-upper').contains(sheet)).toBe(true)
    expect(mon().contains(sheet)).toBe(false)
  })
})

describe('the watching keys', () => {
  it('← → step a frame, Space plays and pauses, L again for 2×, K stops, Home and End', async () => {
    await mount()
    states('sharp')
    const M = sampleSnapshot(MS, at('sharp')).timeline.model
    const c0 = counter()
    key('ArrowRight')
    expect(counter()).not.toBe(c0)
    key('ArrowLeft')
    expect(counter()).toBe(c0)
    key(' ')
    expect(screen.getByTestId('lv-play').getAttribute('aria-label')).toBe('Pause')
    key(' ')
    expect(screen.getByTestId('lv-play').getAttribute('aria-label')).toBe('Play')
    key('l')
    key('l')
    expect(who()).toBe('Your playhead · 2×')
    key('k')
    expect(who()).toBe('Your playhead')
    key('Home')
    expect(counter()).toBe(M.tcF(0))
    key('End')
    expect(counter()).toBe(M.tcF(Math.ceil(M.endF) - 1))
  })

  it('keys in the say field are the field’s; the transport’s buttons do the same as the keys', async () => {
    await mount()
    states('sharp')
    const c0 = counter()
    key('ArrowRight', screen.getByTestId('lv-say-field'))
    expect(counter()).toBe(c0)
    fireEvent.click(screen.getByTestId('lv-frame-fwd'))
    expect(counter()).not.toBe(c0)
    fireEvent.click(screen.getByTestId('lv-frame-back'))
    expect(counter()).toBe(c0)
  })

  it('↑ ↓ go to the agent’s edits; the loop plays the latest change and Esc stops it', async () => {
    await mount()
    step(8) // Atlas has edited
    const before = counter()
    key('ArrowDown')
    key('ArrowUp')
    expect(counter()).not.toBe(before)
    fireEvent.click(screen.getByTestId('lv-loop'))
    expect(who()).toMatch(/^Looping Atlas’s \w+ · Esc to stop$/)
    expect(screen.getByTestId('lv-scrub-loop')).toBeTruthy()
    key('Escape')
    expect(who()).toBe('Your playhead')
  })
})

describe('the scrub bar', () => {
  it('shows the whole edit: changes, markers, others’ playheads and the monitor’s; a click moves your playhead there', async () => {
    await mount()
    step(8)
    const M = sampleSnapshot(WATCH, 8).timeline.model
    const bar = screen.getByTestId('lv-scrub')
    expect(bar.querySelectorAll('[data-testid="lv-scrub-change"]').length).toBeGreaterThan(0)
    expect(bar.querySelectorAll('[data-testid="lv-scrub-marker"]').length).toBeGreaterThan(0)
    expect(bar.querySelector('[data-testid^="lv-scrub-head-"]')).not.toBeNull()
    bar.getBoundingClientRect = () => ({ left: 0, top: 0, right: 1000, bottom: 14, width: 1000, height: 14, x: 0, y: 0, toJSON: () => ({}) })
    fireEvent.mouseDown(bar, { clientX: 500 })
    const f = Math.round(M.endF / 2)
    expect(counter()).toBe(M.tcF(f))
    expect(bar.getAttribute('aria-valuenow')).toBe(String(f))
    expect(parseFloat(screen.getByTestId('lv-scrub-now').style.left)).toBeCloseTo((100 * f) / M.endF, 2)
  })
})
