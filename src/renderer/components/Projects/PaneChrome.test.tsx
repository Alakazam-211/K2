// @vitest-environment jsdom
// The tile bar's layout: an agent tile's view picker sits in the middle
// column of three (name | picker | controls), centred while the name fits
// beside it and right after the name when it doesn't; a pane without one
// keeps the plain row. ⌘N is a quiet keycap that always shows, and keeps
// its place when the bar is short (the name truncates first); the × is the
// shared quiet close button.
import type React from 'react'
import * as fs from 'fs'
import * as path from 'path'
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { PaneChrome } from './PaneChrome'

afterEach(() => cleanup())

const bar = (): HTMLElement => document.querySelector('[data-tile-bar]') as HTMLElement
const CSS = fs.readFileSync(path.resolve(__dirname, '../../globals.css'), 'utf8')

/** Give the jsdom layout the widths a real bar would have. */
function stubWidths(widths: { bar: number; title: number; picker?: number; hint?: number; compact?: number; asleep?: number; moon?: number }): () => void {
  const proto = HTMLElement.prototype
  const saved = {
    client: Object.getOwnPropertyDescriptor(proto, 'clientWidth'),
    offset: Object.getOwnPropertyDescriptor(proto, 'offsetWidth'),
    scroll: Object.getOwnPropertyDescriptor(proto, 'scrollWidth'),
  }
  const pick = (el: HTMLElement): number => {
    if (el.hasAttribute('data-tile-bar')) return widths.bar
    if (el.hasAttribute('data-tile-title')) return widths.title
    if (el.hasAttribute('data-tile-picker') || el.hasAttribute('data-tile-picker-full')) return widths.picker ?? 0
    if (el.hasAttribute('data-tile-picker-compact')) return widths.compact ?? 0
    if (el.hasAttribute('data-tile-hint')) return widths.hint ?? 0
    if (el.hasAttribute('data-tile-asleep-word')) return widths.asleep ?? 0
    if (el.hasAttribute('data-tile-asleep-icon')) return widths.moon ?? 0
    if (el.hasAttribute('data-tile-lead')) return 16
    if (el.hasAttribute('data-tile-shortcut')) return 20
    if (el.hasAttribute('data-tile-close')) return 24
    if (el.getAttribute('data-tile-tool') === 'bell') return 24
    if (el.hasAttribute('data-tile-button')) return 24
    return 0
  }
  for (const key of ['clientWidth', 'offsetWidth', 'scrollWidth'] as const) {
    Object.defineProperty(proto, key, { configurable: true, get(this: HTMLElement) { return pick(this) } })
  }
  return () => {
    if (saved.client) Object.defineProperty(proto, 'clientWidth', saved.client)
    if (saved.offset) Object.defineProperty(proto, 'offsetWidth', saved.offset)
    if (saved.scroll) Object.defineProperty(proto, 'scrollWidth', saved.scroll)
  }
}

const agentTile = (): React.JSX.Element => (
  <PaneChrome
    title="Design System"
    shortcutNum={2}
    readOnly={false}
    focused={false}
    onClose={() => {}}
    lead={<span data-testid="lead" />}
    picker={<div data-testid="picker" />}
    tileView="thread"
  >
    <div />
  </PaneChrome>
)

describe('PaneChrome', () => {
  it('puts an agent tile’s parts in three cells, name | picker | controls', () => {
    render(agentTile())
    const b = bar()
    // Unmeasured (a test DOM): the picker starts centred.
    expect(b.getAttribute('data-tile-fit')).toBe('centred')
    const cells = Array.from(b.children) as HTMLElement[]
    expect(cells.map((c) => Object.keys(c.dataset)[0])).toEqual(['tileStart', 'tilePicker', 'tileEnd'])
    expect(cells[0].contains(screen.getByTestId('lead'))).toBe(true)
    expect(cells[0].textContent).toContain('Design System')
    expect(cells[0].className).toContain('min-w-0')
    expect(cells[1].contains(screen.getByTestId('picker'))).toBe(true)
    expect(cells[1].className).toContain('justify-self-center')
    expect(cells[2].contains(screen.getByRole('button', { name: 'Close pane' }))).toBe(true)
    expect(cells[2].querySelector('[data-tile-shortcut]')).not.toBeNull()
  })

  it('centres the picker while the name fits beside it, and lets it follow the name when not', () => {
    // jsdom has no CSS, so no gaps or padding: the name is 16 + 90 = 106, the
    // controls 20 + 24 = 44. A side column of (400 - 96) / 2 = 152 holds it.
    let restore = stubWidths({ bar: 400, title: 90, picker: 96 })
    render(agentTile())
    expect(bar().getAttribute('data-tile-fit')).toBe('centred')
    cleanup(); restore()
    // A side column of (300 - 96) / 2 = 102 < 107: the picker follows the name, nothing truncates.
    restore = stubWidths({ bar: 300, title: 90, picker: 96 })
    render(agentTile())
    expect(bar().getAttribute('data-tile-fit')).toBe('follow')
    cleanup(); restore()
    // 106 + 96 + 44 + 1 > 240: the name truncates; ⌘N keeps its place.
    restore = stubWidths({ bar: 240, title: 90, picker: 96 })
    render(agentTile())
    expect(bar().getAttribute('data-tile-fit')).toBe('follow')
    expect(bar().hasAttribute('data-tile-keys')).toBe(false)
    expect(bar().querySelector('[data-tile-shortcut]')?.textContent).toBe('⌘2')
    restore()
  })

  it('never hides an asleep agent: the word, then the moon named Asleep, while the picker moves', () => {
    const asleepTile = (): React.JSX.Element => (
      <PaneChrome title="Design System" asleep shortcutNum={2} readOnly={false} focused={false} onClose={() => {}}
        lead={<span />} picker={<div />} pickerCompact={<div />} tileView="thread">
        <div />
      </PaneChrome>
    )
    // Unmeasured: the word.
    render(asleepTile())
    expect(bar().getAttribute('data-tile-asleep')).toBe('word')
    expect(bar().querySelector('[data-tile-asleep-word]')?.textContent).toBe('asleep')
    const moon = screen.getByRole('img', { name: 'Asleep' })
    expect(moon.getAttribute('title')).toBe('Asleep')
    expect(moon.querySelector('svg')).not.toBeNull()
    cleanup()
    // jsdom has no gaps: name 16 + 90 + word 40 = 146, + 1 ≤ side (400 - 96) / 2 = 152: centred, with the word.
    let restore = stubWidths({ bar: 400, title: 90, picker: 96, compact: 30, asleep: 40, moon: 12 })
    render(asleepTile())
    expect([bar().getAttribute('data-tile-fit'), bar().getAttribute('data-tile-asleep')]).toEqual(['centred', 'word'])
    cleanup(); restore()
    // Side 132 < 147: the picker moves off centre and the word stays.
    restore = stubWidths({ bar: 360, title: 90, picker: 96, compact: 30, asleep: 40, moon: 12 })
    render(asleepTile())
    expect([bar().getAttribute('data-tile-fit'), bar().getAttribute('data-tile-asleep'), bar().getAttribute('data-tile-pick')]).toEqual(['follow', 'word', 'full'])
    cleanup(); restore()
    // 146 + 96 + 44 + 1 = 287 > 260, 106 + 12 + 96 + 44 + 1 = 259: the moon, the picker in full, ⌘N kept.
    restore = stubWidths({ bar: 260, title: 90, picker: 96, compact: 30, asleep: 40, moon: 12 })
    render(asleepTile())
    expect([bar().getAttribute('data-tile-asleep'), bar().getAttribute('data-tile-pick')]).toEqual(['icon', 'full'])
    cleanup(); restore()
    // Tighter: the picker folds, the moon stays.
    restore = stubWidths({ bar: 120, title: 90, picker: 96, compact: 30, asleep: 40, moon: 12 })
    render(asleepTile())
    expect([bar().getAttribute('data-tile-asleep'), bar().getAttribute('data-tile-pick')]).toEqual(['icon', 'compact'])
    restore()
  })

  it('collapses the asleep form not in use, the same way as the other parts', () => {
    expect(CSS).toContain('[data-tile-bar][data-tile-asleep="icon"] [data-tile-asleep-word],')
    expect(CSS).toContain('[data-tile-bar][data-tile-asleep="word"] [data-tile-asleep-icon],')
  })

  it('an awake tile draws no asleep mark', () => {
    render(agentTile())
    expect(bar().hasAttribute('data-tile-asleep')).toBe(false)
    expect(bar().querySelector('[data-tile-asleep-word], [data-tile-asleep-icon]')).toBeNull()
  })

  it('keeps the plain row for a pane with no picker, and hides its hint first', () => {
    const doc = (): React.JSX.Element => (
      <PaneChrome title="notes.md" hint="atlas" shortcutNum={5} readOnly={false} focused={false} onClose={() => {}}>
        <div />
      </PaneChrome>
    )
    // name 70, hint 30, controls 44 (no CSS gaps in jsdom): all fit in 150.
    let restore = stubWidths({ bar: 150, title: 70, hint: 30 })
    render(doc())
    const b = bar()
    expect(b.hasAttribute('data-tile-fit')).toBe(false)
    expect(b.className).toContain('flex')
    expect(b.querySelector('[data-tile-picker]')).toBeNull()
    expect(b.getAttribute('data-tile-hint')).toBe('on')
    cleanup(); restore()
    // 145 > 120: the hint goes first.
    restore = stubWidths({ bar: 120, title: 70, hint: 30 })
    render(doc())
    expect(bar().getAttribute('data-tile-hint')).toBe('off')
    cleanup(); restore()
    // 115 > 100: then the name truncates; ⌘N stays.
    restore = stubWidths({ bar: 100, title: 70, hint: 30 })
    render(doc())
    expect(bar().getAttribute('data-tile-hint')).toBe('off')
    expect(bar().querySelector('[data-tile-shortcut]')?.textContent).toBe('⌘5')
    restore()
  })

  it('has no blanket tooltip on the bar (it is still the drag handle)', () => {
    let pressed = 0
    render(
      <PaneChrome title="sales" readOnly={false} focused={false} onClose={() => {}} onHeaderMouseDown={() => { pressed++ }}>
        <div />
      </PaneChrome>,
    )
    expect(bar().hasAttribute('title')).toBe(false)
    bar().dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))
    expect(pressed).toBe(1)
    // The × never starts a drag.
    screen.getByRole('button', { name: 'Close pane' }).dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))
    expect(pressed).toBe(1)
  })

  it('uses the shared quiet × (24px, focus ring, no red) and says the agent keeps running', () => {
    let closed = 0
    render(
      <PaneChrome title="sales" readOnly={false} focused={false} onClose={() => { closed++ }} picker={<div />}>
        <div />
      </PaneChrome>,
    )
    const x = screen.getByRole('button', { name: 'Close pane' })
    expect(x.hasAttribute('data-k2-dialog-close')).toBe(true)
    expect(x.className).toContain('k2-focus')
    expect(x.className).toContain('h-6 w-6')
    expect(x.className).not.toMatch(/error|danger/)
    expect(x.getAttribute('title')).toBe('Close pane. The agent keeps running.')
    x.click()
    expect(closed).toBe(1)
  })

  it('keeps no close button for a read-only viewer', () => {
    render(
      <PaneChrome title="sales" shortcutNum={1} readOnly focused={false} onClose={() => {}}>
        <div />
      </PaneChrome>,
    )
    expect(screen.queryByRole('button', { name: 'Close pane' })).toBeNull()
    expect(bar().className).not.toContain('cursor-grab')
  })

  it('reads its type from the type roles, no raw sizes', () => {
    render(
      <PaneChrome title="notes.md" hint="atlas" shortcutNum={3} readOnly={false} focused={false} onClose={() => {}}>
        <div />
      </PaneChrome>,
    )
    const b = bar()
    expect(b.querySelector('[data-tile-title]')!.className).toContain('type-name')
    expect(b.querySelector('[data-tile-hint]')!.className).toContain('type-hint')
    expect(b.querySelector('[data-tile-shortcut]')!.className).toContain('type-hint')
    expect(b.innerHTML).not.toMatch(/text-\[\d+px\]|font-mono/)
  })

  it('⌘N keeps its name and tooltip, and always shows: plain muted text at rest, unchanged on hover', () => {
    render(agentTile())
    const keys = bar().querySelector('[data-tile-shortcut]') as HTMLElement
    expect(keys.getAttribute('title')).toBe('⌘2 focuses this pane')
    expect(keys.textContent).toBe('⌘2')
    // Plain text in the bar's muted small type: no box, edge, radius or fill.
    const cls = keys.className.split(/\s+/)
    expect(cls).toEqual(expect.arrayContaining(['type-hint', 'text-[var(--color-text-muted)]']))
    expect(cls.filter((c) => /^(border|rounded|bg-|ring|shadow|outline)/.test(c))).toEqual([])
    expect(keys.querySelector('[class*="border"], [class*="rounded"]')).toBeNull()
    // Centred between the divider and the ×: the divider leaves 4px and the
    // × button 9px before its ink, so 6px on the left and 1px on the right
    // give 10px either side.
    expect(cls).toEqual(expect.arrayContaining(['ml-1.5', 'mr-px']))
    expect(CSS).not.toMatch(/\[data-tile-shortcut\][^{}]*\{[^}]*border/)
    // Nothing hides it at rest, here or in globals.css, and nothing on hover
    // or focus changes it.
    expect(keys.className).not.toMatch(/(^|\s)(hidden|invisible|opacity-0|sr-only)(\s|$)|group-hover|hover:/)
    expect(keys.getAttribute('aria-hidden')).toBeNull()
    expect(CSS).not.toMatch(/\[data-tile-shortcut\][^{}]*\{[^}]*(opacity|visibility|display: none|max-width: 0)/)
    expect(CSS).not.toContain('data-tile-keys')
  })

  describe('an agent tile’s bell and search (TileBarTools)', () => {
    const tools = (
      <>
        <span className="flex" data-tile-tool="bell"><button type="button" data-tile-button="">bell</button></span>
        <span className="flex" data-tile-tool="search"><button type="button" data-tile-button="">search</button></span>
      </>
    )
    const tile = (): React.JSX.Element => (
      <PaneChrome
        title="Design System"
        shortcutNum={2}
        readOnly={false}
        focused={false}
        onClose={() => {}}
        onHeaderMouseDown={() => { pressed++ }}
        lead={<span />}
        picker={<div />}
        tools={tools}
        tileView="thread"
      >
        <div />
      </PaneChrome>
    )
    let pressed = 0

    it('sit at the bar’s right end, before ⌘N and the ×, and make the tile the search’s host', () => {
      render(tile())
      const end = bar().querySelector('[data-tile-end]') as HTMLElement
      const order = Array.from(end.children).map((c) => (c.hasAttribute('data-tile-shortcut') ? 'keys' : c.hasAttribute('data-tile-tools') ? 'tools' : c.querySelector('[data-tile-close]') ? 'close' : '?'))
      expect(order).toEqual(['tools', 'keys', 'close'])
      expect(document.querySelector('[data-tile]')!.hasAttribute('data-thread-floats-host')).toBe(true)
      expect(bar().getAttribute('data-tile-bell')).toBe('on')
    })

    it('never start a drag', () => {
      pressed = 0
      render(tile())
      screen.getByText('search').dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))
      expect(pressed).toBe(0)
      bar().dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))
      expect(pressed).toBe(1)
    })

    it('a pane without them is no search host and has no bell state', () => {
      render(agentTile())
      expect(document.querySelector('[data-tile]')!.hasAttribute('data-thread-floats-host')).toBe(false)
      expect(bar().hasAttribute('data-tile-bell')).toBe(false)
    })

    it('fold the bell before the name truncates, keeping ⌘N', () => {
      // No CSS in jsdom: name 16 + 90, picker 96, ⌘N 20, tools 24 + 24, × 24.
      // Following needs 106 + 96 + 20 + 48 + 24 + 1 = 295.
      let restore = stubWidths({ bar: 295, title: 90, picker: 96 })
      render(tile())
      expect(bar().getAttribute('data-tile-bell')).toBe('on')
      cleanup(); restore()
      restore = stubWidths({ bar: 294, title: 90, picker: 96 })
      render(tile())
      expect(bar().getAttribute('data-tile-bell')).toBe('off')
      expect(bar().querySelector('[data-tile-shortcut]')?.textContent).toBe('⌘2')
      restore()
      // A folded bell collapses to no width (globals.css).
      expect(CSS).toContain('[data-tile-bar][data-tile-bell="off"] [data-tile-tool="bell"],')
    })
  })

  it('folds the picker to its menu button, keeping one row, the name and ⌘N', () => {
    const folding = (): React.JSX.Element => (
      <PaneChrome
        title="Design System"
        shortcutNum={2}
        readOnly={false}
        focused={false}
        onClose={() => {}}
        lead={<span />}
        picker={<div data-testid="capsule" />}
        pickerCompact={<button type="button" aria-label="View: Thread" />}
        tileView="thread"
      >
        <div />
      </PaneChrome>
    )
    // name 106, ⌘N 20, × 24: in full 106 + 96 + 44 + 1 = 247; folded (36) 187.
    let restore = stubWidths({ bar: 247, title: 90, picker: 96, compact: 36 })
    render(folding())
    expect(bar().getAttribute('data-tile-pick')).toBe('full')
    cleanup(); restore()
    restore = stubWidths({ bar: 246, title: 90, picker: 96, compact: 36 })
    render(folding())
    expect(bar().getAttribute('data-tile-pick')).toBe('compact')
    expect(bar().querySelector('[data-tile-shortcut]')?.textContent).toBe('⌘2')
    // Both stay mounted (the capsule keeps Ctrl+`); CSS shows one.
    const cell = bar().querySelector('[data-tile-picker]') as HTMLElement
    expect(cell.querySelector('[data-tile-picker-full] [data-testid="capsule"]')).not.toBeNull()
    expect(cell.querySelector('[data-tile-picker-compact]')!.contains(screen.getByRole('button', { name: 'View: Thread' }))).toBe(true)
    restore()
    expect(CSS).toContain('[data-tile-bar][data-tile-pick="compact"] [data-tile-picker-full],')
    expect(CSS).toContain('[data-tile-bar][data-tile-pick="full"] [data-tile-picker-compact] { max-width: 0;')
    // No more own row for the picker in a narrow tile.
    expect(CSS).not.toContain('[data-tile-bar] > [data-tile-picker] { grid-column: 1 / -1')
  })

  it('a bar without a folded picker has no fold state', () => {
    render(agentTile())
    expect(bar().hasAttribute('data-tile-pick')).toBe(false)
  })
})
