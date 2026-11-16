// @vitest-environment jsdom
// Every remaining state of the timeline tab (T6), with the mock's words and
// actions: opening, no clips yet, files that can't be live, preparing video
// on the lanes, the four outside saves (edits paused hatch the lanes), a
// refusal, a hold gone outside, letting go, and the connection.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
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
const stage = (): HTMLElement | null => screen.queryByTestId('lv-mon-stage')
const clip = (name: string): HTMLElement => document.querySelector<HTMLElement>(`.lv-tl-clip[data-full="${name}"]`) as HTMLElement

describe('no edit to show', () => {
  it('opening: the parts as they arrive, step N of M', async () => {
    await mount()
    pick('open')
    expect(stage()?.dataset.kind).toBe('opening')
    expect(stage()?.textContent).toMatch(/^Opening rough-cutThe lanes fill in as each part arrives; the picture comes once the clips are in\. Step 1 of \d+\./)
    expect(within(stage()!).getByText('Connecting')).toBeTruthy()
  })

  it('no clips yet: Ask Atlas puts you in the say row; the first clip takes the card away', async () => {
    await mount()
    pick('empty')
    expect(stage()?.textContent).toContain('No clips on this timeline yet')
    expect(stage()?.textContent).toContain('When Atlas adds clips, the picture shows here.')
    fireEvent.click(within(stage()!).getByRole('button', { name: 'Ask Atlas' }))
    expect(document.activeElement).toBe(screen.getByTestId('lv-say-field'))
    step(1)
    expect(stage()).toBeNull()
  })

  it('a file that can’t be live: why, Open saved versions and Ask Atlas, for each reason', async () => {
    await mount()
    pick('errors')
    const titles: string[] = []
    for (let i = 0; i < 4; i++) {
      const c = stage()!
      expect(c.getAttribute('role')).toBe('alert')
      expect(within(c).getByRole('button', { name: 'Open saved versions' })).toBeTruthy()
      expect(within(c).getByRole('button', { name: 'Ask Atlas' })).toBeTruthy()
      titles.push(c.querySelector('b')!.textContent!)
      if (i < 3) step(1)
    }
    expect(titles).toEqual(['This file holds 3 timelines', 'Two clips overlap on V2', 'Some times fall between frames', 'Too large to watch live, for now'])
  })
})

describe('preparing video on the lanes', () => {
  it('clips that aren’t ready are plain; the ones nobody can preview carry the glyph and light up when shown', async () => {
    await mount()
    pick('prep')
    expect(clip('B-roll 05').querySelector('[data-testid="lv-clip-noprev"]')).not.toBeNull()
    expect(clip('B-roll 05').getAttribute('aria-label')).toContain('can’t be previewed')
    expect(clip('B-roll 05').hasAttribute('data-hl')).toBe(false)
    step(1)
    expect(clip('B-roll 05').hasAttribute('data-hl')).toBe(true)
    expect(clip('B-roll 08').hasAttribute('data-hl')).toBe(true)
    step(1)
    expect(document.querySelectorAll('.lv-tl-clip[data-fresh-in]').length).toBeGreaterThan(0)
  })
})

describe('outside saves', () => {
  it('taken in: the notice says so, Dismiss', async () => {
    await mount()
    pick('save-ingested')
    step(1)
    const n = screen.getByTestId('lv-notice')
    expect(n.textContent).toContain('Saved outside K2')
    expect(within(n).getByRole('button', { name: 'Dismiss' })).toBeTruthy()
  })

  it('kept both: Open the copy and History', async () => {
    await mount()
    pick('save-both')
    step(1)
    const n = screen.getByTestId('lv-notice')
    expect(within(n).getByRole('button', { name: 'Open the copy' })).toBeTruthy()
    expect(within(n).getByRole('button', { name: 'History' })).toBeTruthy()
  })

  it('declined: edits pause, the lanes hatch, the say row still sends', async () => {
    await mount()
    pick('save-declined')
    expect(screen.getByTestId('lv-lanes').hasAttribute('data-paused')).toBe(false)
    step(1)
    expect(screen.getByTestId('lv-notice').textContent).toContain('can’t be live any more')
    expect(screen.getByTestId('lv-state').textContent).toBe('Edits paused')
    expect(screen.getByTestId('lv-lanes').hasAttribute('data-paused')).toBe(true)
    expect(screen.getByTestId('lv-say-field').getAttribute('placeholder')).toMatch(/^Edits paused; you can still message /)
  })

  it('missing: the chip says File missing, and Save it again', async () => {
    await mount()
    pick('save-missing')
    step(1)
    expect(screen.getByTestId('lv-state').textContent).toBe('File missing')
    expect(within(screen.getByTestId('lv-notice')).getByRole('button', { name: 'Save it again' })).toBeTruthy()
  })
})

describe('holds and refusals', () => {
  it('refused: the notice names who holds it, the clip carries the stop glyph', async () => {
    await mount()
    pick('refuse')
    step(1)
    const n = screen.getByTestId('lv-notice')
    expect(n.textContent).toContain('Brook couldn’t move B-roll 03: Atlas is holding it.')
    expect(within(n).getByRole('button', { name: 'Ask Atlas' })).toBeTruthy()
    expect(clip('B-roll 03').hasAttribute('data-refused')).toBe(true)
  })

  it('a hold ended outside K2: the header and Watching say why', async () => {
    await mount()
    pick('holdgone')
    step(1)
    expect(screen.getByTestId('lv-sentence').textContent).toContain('Brook’s hold on Music ended: the track was renamed outside K2')
  })

  it('let go: Watching says so and the clips it held fade their outline', async () => {
    await mount()
    pick('watch')
    step(12)
    expect(screen.getByTestId('lv-person-agent:atlas').textContent).toContain('let go of its hold just now')
    expect(document.querySelectorAll('.lv-tl-clip[data-released]').length).toBeGreaterThan(0)
  })
})

describe('the connection', () => {
  it('reconnecting keeps the view and turns the say row off; can’t reconnect offers Retry', async () => {
    await mount()
    pick('connection')
    step(1)
    expect(screen.getByTestId('lv-notice').textContent).toContain('Reconnecting…')
    expect((screen.getByTestId('lv-say-field') as HTMLInputElement).disabled).toBe(true)
    expect(screen.getByTestId('lv-monitor').getAttribute('data-state')).not.toBe('none')
    step(4)
    expect(within(screen.getByTestId('lv-notice')).getByRole('button', { name: 'Retry' })).toBeTruthy()
  })
})
