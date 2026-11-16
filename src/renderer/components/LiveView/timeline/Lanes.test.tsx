// @vitest-environment jsdom
// The lanes and the ruler on the timeline's sample (T3): lane headers (V2,
// V1, A1, A2, names, the muted glyph focusable with its name), clips with
// whole-name labels and their marks, the ruler (timecodes by the density
// rule, other people's tags, markers), lane heights from the View menu
// (saved for this tab, with its line), zoom and fit by keys, the narrow
// window, and a long edit (small lanes, the overview, only a window drawn).

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { shortNames } from '@/lib/live-view/timeline/labels'
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

/** The banner's scenarios are a grouped menu (more than five): open it and pick one. */
function pick(id: string): void {
  fireEvent.click(within(screen.getByTestId('lv-sample-scenarios')).getByRole('button'))
  fireEvent.click(document.querySelector(`[data-setting-dropdown-menu] button[data-value="${id}"]`) as HTMLElement)
}
const px = (el: HTMLElement, k: 'left' | 'width' | 'height'): number => parseFloat(el.style[k])

describe('the lanes', () => {
  it('stack V2 and V1 over A1 and A2, each header with its label and name; the muted track says so with a focusable glyph', async () => {
    await mount()
    const heads = ['V2', 'V1', 'A1', 'A2'].map((l) => screen.getByTestId(`lv-lane-head-${l}`))
    expect(heads.map((h) => h.textContent)).toEqual(['V2B-roll and titles', 'V1Interviews', 'A1Dialogue', 'A2Music'])
    const flag = screen.getByTestId('lv-lane-flag-A2')
    expect(flag.getAttribute('aria-label')).toBe('Muted in the file')
    expect(flag.getAttribute('tabindex')).toBe('0')
    expect(flag.hasAttribute('data-k2-focus')).toBe(true)
    expect(flag.getAttribute('title')).toContain('plays silent')
    // medium lanes: 58 px video, 48 px audio (each with its 1 px rule; the split above A1)
    expect(['V2', 'V1', 'A1', 'A2'].map((l) => px(screen.getByTestId(`lv-lane-${l}`), 'height'))).toEqual([59, 59, 52, 49])
  })

  it('every clip’s label is its full name or one of its short names, and the full name is its accessible name', async () => {
    await mount()
    const clips = Array.from(document.querySelectorAll<HTMLElement>('.lv-tl-clip'))
    expect(clips.length).toBeGreaterThan(10)
    let named = 0
    for (const c of clips) {
      const full = c.dataset.full as string
      expect(c.getAttribute('aria-label')!.startsWith(full)).toBe(true)
      const name = c.querySelector('.lv-tl-name')
      if (!name) continue
      named++
      expect(shortNames(full)).toContain(name.textContent)
      expect(name.textContent).not.toMatch(/…/)
    }
    expect(named).toBeGreaterThan(5)
  })

  it('a hold, a change and a refusal mark their clips with an outline and a glyph, never words', async () => {
    await mount()
    step(2) // Atlas holds
    const held = document.querySelector<HTMLElement>('.lv-tl-clip[data-k="@item:2"]')!
    expect(held.hasAttribute('data-held')).toBe(true)
    expect(held.querySelector('.lv-tl-lock')).not.toBeNull()
    expect(held.getAttribute('aria-label')).toContain('held by Atlas')
    step(3) // trim
    const trimmed = document.querySelector<HTMLElement>('.lv-tl-clip[data-k="@item:2"]')!
    expect(trimmed.hasAttribute('data-changed')).toBe(true)
    expect(trimmed.querySelector('.lv-tl-dot')).not.toBeNull()
    expect(trimmed.querySelector('.lv-tl-band')!.textContent).not.toMatch(/changed/i)
    step(9) // refused
    const refused = document.querySelector<HTMLElement>('.lv-tl-clip[data-k="@item:2"]')!
    expect(refused.hasAttribute('data-refused')).toBe(true)
    expect(refused.querySelector('.lv-tl-stop')).not.toBeNull()
    expect(refused.getAttribute('aria-label')).toContain('Atlas couldn’t trim Interview B: Brook is holding it')
  })
})

describe('the ruler', () => {
  it('labels timecodes at least 110 px apart and shows the monitor’s timecode big in its corner', async () => {
    await mount()
    const labels = screen.getAllByTestId('lv-ruler-label')
    expect(labels.length).toBeGreaterThan(2)
    for (const l of labels) expect(l.textContent).toMatch(/^\d\d:\d\d:\d\d:\d\d$/)
    const xs = labels.map((l) => px(l, 'left')).sort((a, b) => a - b)
    for (let i = 1; i < xs.length; i++) expect(xs[i] - xs[i - 1]).toBeGreaterThanOrEqual(110)
    expect(screen.getByTestId('lv-counter').textContent).toMatch(/^\d\d:\d\d:\d\d:\d\d$/)
  })

  it('other people’s playheads are name tags in the shared tag style; yours is the monitor’s line, with no tag; tags never overlap', async () => {
    await mount()
    step(4) // Sam comes to the same spot as Atlas
    const tags = screen.getAllByTestId('lv-ruler-tag')
    const people = tags.filter((t) => t.hasAttribute('data-person')).map((t) => t.textContent)
    expect(people).toEqual(expect.arrayContaining(['Sam', 'Atlas']))
    expect(people).not.toContain('You')
    expect(screen.getByTestId('lv-monitor-line')).toBeTruthy()
    const boxes = tags.map((t) => [px(t, 'left'), px(t, 'left') + px(t, 'width')] as const).sort((a, b) => a[0] - b[0])
    for (let i = 1; i < boxes.length; i++) expect(boxes[i][0]).toBeGreaterThanOrEqual(boxes[i - 1][1])
  })

  it('a marker shows as a flag, its label the whole name with a capital or none', async () => {
    await mount()
    const flags = screen.getAllByTestId('lv-ruler-flag')
    expect(flags.length).toBeGreaterThan(0)
    for (const t of screen.getAllByTestId('lv-ruler-tag').filter((x) => x.hasAttribute('data-marker'))) expect(t.textContent![0]).toBe(t.textContent![0].toUpperCase())
  })
})

describe('the View menu', () => {
  it('holds three labelled radio groups; a choice changes the lanes and says it’s saved for this tab', async () => {
    await mount()
    fireEvent.click(screen.getByTestId('lv-view-btn'))
    const menu = screen.getByRole('dialog', { name: 'View' })
    const groups = within(menu).getAllByRole('group')
    expect(groups.map((g) => g.querySelector('legend')!.textContent)).toEqual(['Lane height', 'Pictures on clips', 'While playing'])
    expect(within(groups[0]).getAllByRole('radio').map((r) => (r as HTMLInputElement).checked)).toEqual([false, true, false])
    for (const r of within(menu).getAllByRole('radio')) expect(r.className).toContain('k2-radio')
    // the chosen radio has the focus when it opens
    expect(document.activeElement).toBe(within(groups[0]).getByRole('radio', { name: 'Medium' }))
    fireEvent.click(within(groups[0]).getByRole('radio', { name: 'Large' }))
    expect(screen.getByTestId('lv-view-done').textContent).toBe('Lane height: Large. Saved for this tab.')
    expect(screen.getByTestId('lv-lanes').getAttribute('data-size')).toBe('L')
    expect(px(screen.getByTestId('lv-lane-V2'), 'height')).toBe(93)
    expect(screen.getByTestId('lv-lane-head-V2').textContent).toContain('13 clips')
    fireEvent.click(within(groups[1]).getByRole('radio', { name: 'None' }))
    expect(screen.getByTestId('lv-lanes').getAttribute('data-pic')).toBe('off')
  })

  it('Esc closes it and puts the focus back on View', async () => {
    await mount()
    const btn = screen.getByTestId('lv-view-btn')
    fireEvent.click(btn)
    expect(btn.getAttribute('aria-expanded')).toBe('true')
    fireEvent.keyDown(screen.getByRole('dialog', { name: 'View' }), { key: 'Escape' })
    expect(screen.queryByRole('dialog', { name: 'View' })).toBeNull()
    expect(document.activeElement).toBe(btn)
  })

  it('the choice is kept for the tab when it opens again', async () => {
    await mount()
    fireEvent.click(screen.getByTestId('lv-view-btn'))
    fireEvent.click(screen.getByRole('radio', { name: 'Small' }))
    cleanup()
    await mount()
    expect(screen.getByTestId('lv-lanes').getAttribute('data-size')).toBe('S')
    // back to medium for the tests after this one (the choice outlives the mount)
    fireEvent.click(screen.getByTestId('lv-view-btn'))
    fireEvent.click(screen.getByRole('radio', { name: 'Medium' }))
    fireEvent.click(screen.getByRole('radio', { name: 'Automatic' }))
    expect(screen.getByTestId('lv-lanes').getAttribute('data-pic')).toBe('auto')
  })
})

describe('zoom, fit and the keyboard', () => {
  it('the lanes take the focus; = zooms in, - out, and \\ fits the edit', async () => {
    await mount()
    const lanes = screen.getByTestId('lv-lanes')
    expect(lanes.getAttribute('tabindex')).toBe('0')
    expect(lanes.hasAttribute('data-k2-focus')).toBe(true)
    const width = (): number => px(screen.getByTestId('lv-ruler-body'), 'width')
    const w0 = width()
    lanes.focus()
    fireEvent.keyDown(lanes, { key: '=' })
    expect(width()).toBeGreaterThan(w0 * 1.4)
    fireEvent.keyDown(lanes, { key: '-' })
    expect(width()).toBeCloseTo(w0, 0)
    fireEvent.keyDown(lanes, { key: '\\' })
    // fit: the whole edit in the window, so the lanes are only as wide as the window
    expect(width()).toBeLessThan(w0)
    fireEvent.click(screen.getByTestId('lv-zoom-in'))
    expect(width()).toBeGreaterThan(w0 / 2)
  })

  it('every control in the lanes’ row is a real button with a name', async () => {
    await mount()
    for (const id of ['lv-zoom-out', 'lv-zoom-in', 'lv-zoom-fit', 'lv-view-btn']) {
      const b = screen.getByTestId(id)
      expect(b.tagName).toBe('BUTTON')
      expect(b.getAttribute('aria-label') || b.textContent).toBeTruthy()
    }
  })
})

describe('narrow, and a long edit', () => {
  it('at 800 px the lane heads keep their label and clip count, the name in the tooltip; the counter leaves the ruler', async () => {
    await mount(800)
    const v1 = screen.getByTestId('lv-lane-head-V1')
    expect(v1.hasAttribute('data-narrow')).toBe(true)
    expect(v1.textContent).toBe('V18 clips')
    expect(v1.getAttribute('title')).toContain('Interviews')
    expect(screen.queryByTestId('lv-counter')).toBeNull()
  })

  it('a long edit opens with small lanes and the overview; zoomed in, it draws only a window of its clips', async () => {
    await mount()
    pick('large')
    expect(screen.getByTestId('lv-lanes').getAttribute('data-size')).toBe('S')
    expect(screen.getByTestId('lv-overview')).toBeTruthy()
    // it opens fitted (every clip a sliver); the next step zooms in to Atlas's lane
    step(1)
    const drawn = document.querySelectorAll('.lv-tl-clip').length
    expect(drawn).toBeGreaterThan(0)
    expect(drawn).toBeLessThan(300)
  })

  it('the pinned file’s lanes read as the pack labels them', async () => {
    await mount()
    pick('golden')
    expect(screen.getByTestId('lv-lane-head-V1').textContent).toContain('V1')
    expect(document.querySelectorAll('.lv-tl-clip').length).toBeGreaterThan(0)
  })
})
