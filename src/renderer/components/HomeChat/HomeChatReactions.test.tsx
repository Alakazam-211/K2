// @vitest-environment jsdom
// Reactions and the action bar on a Home chat message (K2 Design pass):
//   1. Chips: the emoji and count, a ring when yours, a tip naming who.
//      A toggle is a small crisp change (the width holds); a 28px hit
//      target; a new chip row grows in and a chip that goes fades out, on
//      the fast motion token, with none of it under reduced motion.
//   2. The picker: six quick emoji, a search and a grid of emoji, all by
//      keyboard; Esc closes it and focus goes back to the React button. It
//      opens by the React button inside the window, placed before it shows,
//      and keeps one size, so typing never moves it.
// (The hover row on a message: HomeChatHoverRow.test.tsx.)

import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import {
  HomeChatReactionChips,
  HomeChatReactionPicker,
  PICKER_COLUMNS,
  REACTION_CHIP_PX,
  REACTION_HIT_PX,
  PICKER_GRID_PX,
  pickerMove,
  pickerPlace,
  reactionTipTop,
  reactionTipText,
  reactorsLine,
} from './HomeChatReactions'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  delete (HTMLElement.prototype as { animate?: unknown }).animate
})

/** Web Animations for jsdom: records each call; `finish()` ends them all. */
function mockAnimate(): { calls: { el: HTMLElement; frames: Keyframe[]; opts: KeyframeAnimationOptions }[]; finish: () => void } {
  const calls: { el: HTMLElement; frames: Keyframe[]; opts: KeyframeAnimationOptions }[] = []
  const anims: { onfinish: (() => void) | null }[] = []
  ;(HTMLElement.prototype as { animate?: unknown }).animate = function (this: HTMLElement, frames: Keyframe[], opts: KeyframeAnimationOptions) {
    calls.push({ el: this, frames, opts })
    const a = { onfinish: null as (() => void) | null, cancel: () => undefined }
    anims.push(a)
    return a
  }
  return {
    calls,
    finish: () => {
      for (const a of anims.splice(0)) a.onfinish?.()
    },
  }
}

function mockReducedMotion(reduce: boolean): void {
  vi.stubGlobal(
    'matchMedia',
    vi.fn((q: string) => ({ matches: reduce && q.includes('reduce'), media: q, addEventListener: () => undefined, removeEventListener: () => undefined })),
  )
}

const face = (chip: Element): HTMLElement => chip.querySelector('[data-reaction-face]') as HTMLElement
const px = (v: string): number => Number.parseFloat(v || '0')

describe('who reacted', () => {
  it('names you first, joins with "and", and folds a long list', () => {
    expect(reactorsLine([])).toBe('')
    expect(reactorsLine(['Ana'])).toBe('Ana')
    expect(reactorsLine(['Ana', 'You'])).toBe('You and Ana')
    expect(reactorsLine(['Ana', 'You', 'Ben'])).toBe('You, Ana and Ben')
    expect(reactorsLine(['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H'])).toBe('A, B, C, D, E and 3 others')
    expect(reactorsLine(['A', 'B', 'C', 'D', 'E', 'F', 'G'])).toBe('A, B, C, D, E and 2 others')
  })

  it('the tip says who, or that it is still asking', () => {
    expect(reactionTipText(undefined, '👍')).toBe('Finding who reacted with 👍…')
    expect(reactionTipText(['Ana', 'You'], '👍')).toBe('You and Ana reacted with 👍')
    expect(reactionTipText([], '👍')).toBe('Reacted with 👍')
  })
})

describe('reaction chips', () => {
  const reactions = [
    { emoji: '👍', count: 2, me: true },
    { emoji: '🎉', count: 1, me: false },
  ]

  it('show the emoji and count; yours is pressed and ringed in the accent', () => {
    render(<HomeChatReactionChips reactions={reactions} mine={false} onToggle={vi.fn()} whoFor={async () => []} />)
    const chips = screen.getAllByTestId('home-chat-reaction')
    expect(chips.map((c) => c.textContent)).toEqual(['👍2', '🎉1'])
    expect(chips[0].getAttribute('aria-pressed')).toBe('true')
    expect(chips[0].hasAttribute('data-me')).toBe(true)
    expect(face(chips[0]).style.border).toContain('var(--zen-focus-ring)')
    expect(face(chips[1]).style.border).toContain('var(--zen-control-edge)')
    // A chip in each look: the Thread's chamfer in Terminal, a pill in Glass.
    expect(face(chips[0]).hasAttribute('data-chamfer')).toBe(true)
    expect(face(chips[0]).className).toContain('rounded-[var(--radius-pill)]')
    expect(chips[0].className).toContain('rounded-[var(--radius-pill)]')
  })

  it('each chip is a 28px hit target around the smaller chip you see', () => {
    render(<HomeChatReactionChips reactions={reactions} mine={false} onToggle={vi.fn()} whoFor={async () => []} />)
    const chip = screen.getAllByTestId('home-chat-reaction')[0]
    expect(REACTION_HIT_PX).toBeGreaterThanOrEqual(28)
    expect(px(chip.style.height)).toBe(REACTION_HIT_PX)
    expect(px(face(chip).style.height)).toBe(REACTION_CHIP_PX)
    // The chips' hit areas meet: no dead gap between them, and the row keeps
    // its small visual gap from the hit area's own inset.
    expect(screen.getByTestId('home-chat-reactions').style.gap).toBe('0px')
  })

  it('toggling yours is a small change: the width holds (edge, inset and a count of up to two digits)', () => {
    const one = { emoji: '👍', count: 9, me: false }
    const { rerender } = render(<HomeChatReactionChips reactions={[one]} mine={false} onToggle={vi.fn()} whoFor={async () => []} />)
    const across = (): number => {
      const f = face(screen.getByTestId('home-chat-reaction'))
      // (A var() colour leaves the longhands empty here: read the shorthand.)
      return px(/([\d.]+)px/.exec(f.style.border)?.[1] ?? '') * 2 + px(f.style.paddingLeft) + px(f.style.paddingRight)
    }
    const theirs = across()
    rerender(<HomeChatReactionChips reactions={[{ ...one, count: 10, me: true }]} mine={false} onToggle={vi.fn()} whoFor={async () => []} />)
    expect(across()).toBe(theirs)
    const count = screen.getByTestId('home-chat-reaction').querySelector('[data-reaction-count]') as HTMLElement
    expect(count.style.minWidth).toBe('2ch')
    expect(face(screen.getByTestId('home-chat-reaction')).style.fontVariantNumeric).toBe('tabular-nums')
  })

  it('the fill and ring change on the fast token, at once under reduced motion', () => {
    mockReducedMotion(false)
    const { unmount } = render(<HomeChatReactionChips reactions={reactions} mine={false} onToggle={vi.fn()} whoFor={async () => []} />)
    expect(face(screen.getAllByTestId('home-chat-reaction')[0]).style.transition).toContain('background-color var(--motion-duration-fast)')
    unmount()
    mockReducedMotion(true)
    render(<HomeChatReactionChips reactions={reactions} mine={false} onToggle={vi.fn()} whoFor={async () => []} />)
    expect(face(screen.getAllByTestId('home-chat-reaction')[0]).style.transition).toBe('none')
  })

  it('a first reaction grows the chip row in (no jump below); a row already there just draws', () => {
    const anim = mockAnimate()
    const { rerender } = render(<HomeChatReactionChips reactions={[]} mine={false} onToggle={vi.fn()} whoFor={async () => []} />)
    expect(screen.queryByTestId('home-chat-reactions')).toBeNull()
    rerender(<HomeChatReactionChips reactions={[{ emoji: '👍', count: 1, me: true }]} mine={false} onToggle={vi.fn()} whoFor={async () => []} />)
    const box = screen.getByTestId('home-chat-reactions')
    const grow = anim.calls.find((c) => c.el === box)
    expect(grow?.frames[0]).toMatchObject({ height: '0px' })
    expect(typeof grow?.opts.duration).toBe('number')
    // Mounted with reactions (scrolled into view): no motion.
    cleanup()
    anim.calls.length = 0
    render(<HomeChatReactionChips reactions={reactions} mine={false} onToggle={vi.fn()} whoFor={async () => []} />)
    expect(anim.calls).toHaveLength(0)
  })

  it('a chip that goes fades out first (not clickable meanwhile); the last one folds the row', () => {
    const anim = mockAnimate()
    const onToggle = vi.fn()
    const { rerender } = render(<HomeChatReactionChips reactions={reactions} mine={false} onToggle={onToggle} whoFor={async () => []} />)
    rerender(<HomeChatReactionChips reactions={[reactions[0]]} mine={false} onToggle={onToggle} whoFor={async () => []} />)
    const leaving = document.querySelector('[data-leaving]') as HTMLElement
    expect(leaving.getAttribute('data-emoji')).toBe('🎉')
    expect(leaving.getAttribute('aria-hidden')).toBe('true')
    expect(leaving.tabIndex).toBe(-1)
    fireEvent.click(leaving)
    expect(onToggle).not.toHaveBeenCalled()
    expect(anim.calls.find((c) => c.el === leaving)?.frames.at(-1)).toMatchObject({ opacity: 0 })
    // It holds its place until the fade ends.
    expect(screen.getAllByTestId('home-chat-reaction').map((c) => c.getAttribute('data-emoji'))).toEqual(['👍', '🎉'])
    act(() => anim.finish())
    expect(screen.getAllByTestId('home-chat-reaction').map((c) => c.getAttribute('data-emoji'))).toEqual(['👍'])
    // The last chip: the row folds as it fades, then goes.
    rerender(<HomeChatReactionChips reactions={[]} mine={false} onToggle={onToggle} whoFor={async () => []} />)
    const box = screen.getByTestId('home-chat-reactions')
    expect(anim.calls.find((c) => c.el === box)?.frames.at(-1)).toMatchObject({ height: '0px' })
    act(() => anim.finish())
    expect(screen.queryByTestId('home-chat-reactions')).toBeNull()
  })

  it('under reduced motion a chip just goes, and a row just appears', () => {
    mockReducedMotion(true)
    const anim = mockAnimate()
    const { rerender } = render(<HomeChatReactionChips reactions={[]} mine={false} onToggle={vi.fn()} whoFor={async () => []} />)
    rerender(<HomeChatReactionChips reactions={reactions} mine={false} onToggle={vi.fn()} whoFor={async () => []} />)
    rerender(<HomeChatReactionChips reactions={[reactions[0]]} mine={false} onToggle={vi.fn()} whoFor={async () => []} />)
    expect(screen.getAllByTestId('home-chat-reaction')).toHaveLength(1)
    expect(anim.calls).toHaveLength(0)
  })

  it('hover shows a tip listing who reacted, and the label names them too', async () => {
    const whoFor = vi.fn(async () => ['Ana Ruiz', 'You'])
    render(<HomeChatReactionChips reactions={reactions} mine={false} onToggle={vi.fn()} whoFor={whoFor} />)
    const chip = screen.getAllByTestId('home-chat-reaction')[0]
    await act(async () => {
      fireEvent.mouseEnter(chip)
    })
    expect(whoFor).toHaveBeenCalledWith('👍')
    expect(screen.getByTestId('home-chat-reaction-tip').textContent).toBe('You and Ana Ruiz reacted with 👍')
    expect(chip.getAttribute('aria-label')).toBe('👍 2: You and Ana Ruiz')
    await act(async () => {
      fireEvent.mouseLeave(chip)
    })
    expect(screen.queryByTestId('home-chat-reaction-tip')).toBeNull()
  })

  it('the tip goes below the chip, off the message text; above only when there is no room below', () => {
    const chip = { top: 300, bottom: 322 }
    expect(reactionTipTop(chip, 24, 900)).toBe(328)
    // No room under it: above.
    expect(reactionTipTop({ top: 860, bottom: 882 }, 24, 900)).toBe(830)
    // No room either way: kept 8px inside the window.
    expect(reactionTipTop({ top: 10, bottom: 32 }, 30, 60)).toBe(22)
  })

  it('the tip is floating chrome: Frost in Glass, cut in Terminal', async () => {
    render(<HomeChatReactionChips reactions={reactions} mine={false} onToggle={vi.fn()} whoFor={async () => ['Ana Ruiz']} />)
    await act(async () => {
      fireEvent.mouseEnter(screen.getAllByTestId('home-chat-reaction')[0])
    })
    const tip = screen.getByTestId('home-chat-reaction-tip')
    expect(tip.getAttribute('data-material')).toBe('frost')
    expect(tip.getAttribute('data-float')).toBe('popover')
    expect(tip.getAttribute('data-k2-chamfer')).toBe('chip')
  })

  it('a tap toggles yours; read-only chips do nothing', () => {
    const onToggle = vi.fn()
    const { rerender } = render(<HomeChatReactionChips reactions={reactions} mine={false} onToggle={onToggle} whoFor={async () => []} />)
    fireEvent.click(screen.getAllByTestId('home-chat-reaction')[1])
    expect(onToggle).toHaveBeenCalledWith('🎉')
    onToggle.mockClear()
    rerender(<HomeChatReactionChips reactions={reactions} mine={false} onToggle={onToggle} whoFor={async () => []} disabled />)
    fireEvent.click(screen.getAllByTestId('home-chat-reaction')[1])
    expect(onToggle).not.toHaveBeenCalled()
  })
})

describe('the picker', () => {
  const SIZE = { width: 236, height: 250 }
  const btn = { top: 300, bottom: 326, left: 400, right: 426 }

  it('opens above the React button, lined up with it; below when there is no room above', () => {
    expect(pickerPlace(btn, SIZE, { width: 1200, height: 900 }, 'start')).toEqual({ left: 400, top: 300 - 250 - 6 })
    expect(pickerPlace(btn, SIZE, { width: 1200, height: 900 }, 'end')).toEqual({ left: 426 - 236, top: 44 })
    const high = { top: 120, bottom: 146, left: 400, right: 426 }
    expect(pickerPlace(high, SIZE, { width: 1200, height: 900 }, 'start').top).toBe(146 + 6)
  })

  it('stays inside the window: clamped at its edges, both ways', () => {
    const p = pickerPlace({ top: 120, bottom: 146, left: 1150, right: 1176 }, SIZE, { width: 1200, height: 300 }, 'start')
    expect(p.left).toBe(1200 - 236 - 8)
    expect(p.top).toBeGreaterThanOrEqual(8)
    expect(p.top + SIZE.height).toBeLessThanOrEqual(300 - 8 + 0.5)
    expect(pickerPlace({ top: 300, bottom: 326, left: 2, right: 28 }, SIZE, { width: 1200, height: 900 }, 'end').left).toBe(8)
  })

  it('is placed before it shows, and typing never moves or resizes it', () => {
    // Its height follows what it holds (as a browser's would).
    const h = vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (this: HTMLElement) {
      return this.getAttribute('data-testid') === 'home-chat-reaction-picker' ? 120 + 4 * this.querySelectorAll('button').length : 0
    })
    const w = vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockImplementation(() => 236)
    const anchor = document.createElement('button')
    anchor.getBoundingClientRect = () => DOMRect.fromRect({ x: 400, y: 600, width: 26, height: 26 })
    document.body.append(anchor)
    try {
      render(<HomeChatReactionPicker anchor={anchor} onPick={vi.fn()} onClose={vi.fn()} />)
      const box = screen.getByTestId('home-chat-reaction-picker')
      const holder = box.closest('[data-k2-chamfer-float="menu"]') as HTMLElement
      expect(holder.style.visibility).toBe('visible')
      const at = { left: holder.style.left, top: holder.style.top }
      expect(px(at.top)).toBeLessThan(600)
      // The grid has one fixed height, whatever it shows.
      const grid = screen.getByTestId('home-chat-emoji-results')
      expect(grid.style.height).toBe(`${PICKER_GRID_PX}px`)
      fireEvent.change(screen.getByTestId('home-chat-emoji-search'), { target: { value: 'heart' } })
      expect({ left: holder.style.left, top: holder.style.top }).toEqual(at)
      expect(screen.getByTestId('home-chat-emoji-results').style.height).toBe(`${PICKER_GRID_PX}px`)
      fireEvent.change(screen.getByTestId('home-chat-emoji-search'), { target: { value: 'zzzz' } })
      expect({ left: holder.style.left, top: holder.style.top }).toEqual(at)
      expect(screen.getByRole('status').textContent).toBe('No emoji match')
    } finally {
      anchor.remove()
      h.mockRestore()
      w.mockRestore()
    }
  })

  it('with no search it shows every emoji to browse; Enter in the empty search picks nothing', () => {
    const onPick = vi.fn()
    render(<HomeChatReactionPicker anchor={null} onPick={onPick} onClose={vi.fn()} />)
    const grid = screen.getByTestId('home-chat-emoji-results')
    expect(grid.getAttribute('aria-label')).toBe('All emoji')
    expect(grid.querySelectorAll('button').length).toBeGreaterThan(100)
    fireEvent.keyDown(screen.getByTestId('home-chat-emoji-search'), { key: 'Enter' })
    expect(onPick).not.toHaveBeenCalled()
    fireEvent.keyDown(screen.getByTestId('home-chat-emoji-search'), { key: 'ArrowDown' })
    expect(document.activeElement).toBe(grid.querySelector('button'))
  })

  it('keys move between the quick row, the search and the results', () => {
    const counts = { quick: 6, results: 14 }
    expect(pickerMove({ zone: 'search', index: 0 }, 'ArrowUp', counts)).toEqual({ zone: 'quick', index: 0 })
    expect(pickerMove({ zone: 'search', index: 0 }, 'ArrowDown', counts)).toEqual({ zone: 'results', index: 0 })
    expect(pickerMove({ zone: 'search', index: 0 }, 'ArrowDown', { quick: 6, results: 0 })).toBeNull()
    expect(pickerMove({ zone: 'quick', index: 5 }, 'ArrowRight', counts)).toEqual({ zone: 'quick', index: 0 })
    expect(pickerMove({ zone: 'quick', index: 0 }, 'ArrowLeft', counts)).toEqual({ zone: 'quick', index: 5 })
    expect(pickerMove({ zone: 'quick', index: 3 }, 'ArrowDown', counts)).toEqual({ zone: 'search', index: 0 })
    expect(pickerMove({ zone: 'results', index: 2 }, 'ArrowUp', counts)).toEqual({ zone: 'search', index: 0 })
    expect(pickerMove({ zone: 'results', index: 2 }, 'ArrowDown', counts)).toEqual({ zone: 'results', index: 2 + PICKER_COLUMNS })
    expect(pickerMove({ zone: 'results', index: 8 }, 'ArrowUp', counts)).toEqual({ zone: 'results', index: 2 })
    expect(pickerMove({ zone: 'results', index: 13 }, 'ArrowDown', counts)).toBeNull()
    expect(pickerMove({ zone: 'results', index: 4 }, 'End', counts)).toEqual({ zone: 'results', index: 13 })
  })

  it('is the app menu shape, with six quick emoji, and focus starts in the search', () => {
    render(<HomeChatReactionPicker anchor={null} onPick={vi.fn()} onClose={vi.fn()} />)
    const box = screen.getByTestId('home-chat-reaction-picker')
    expect(box.hasAttribute('data-k2-menu')).toBe(true)
    expect(box.getAttribute('data-k2-chamfer')).toBe('surface')
    expect(box.closest('[data-k2-chamfer-float="menu"]')).not.toBeNull()
    expect(screen.getByTestId('home-chat-quick-reactions').querySelectorAll('button')).toHaveLength(6)
    expect(document.activeElement).toBe(screen.getByTestId('home-chat-emoji-search'))
  })

  it('↑ reaches the quick row, → moves, Enter picks; it closes and focus goes back', async () => {
    const anchor = document.createElement('button')
    document.body.append(anchor)
    const onPick = vi.fn()
    const onClose = vi.fn()
    render(<HomeChatReactionPicker anchor={anchor} onPick={onPick} onClose={onClose} />)
    fireEvent.keyDown(screen.getByTestId('home-chat-emoji-search'), { key: 'ArrowUp' })
    const quick = screen.getByTestId('home-chat-quick-reactions').querySelectorAll('button')
    expect(document.activeElement).toBe(quick[0])
    fireEvent.keyDown(quick[0], { key: 'ArrowRight' })
    expect(document.activeElement).toBe(quick[1])
    fireEvent.click(quick[1])
    expect(onPick).toHaveBeenCalledWith(quick[1].getAttribute('data-emoji'))
    expect(onClose).toHaveBeenCalled()
    expect(document.activeElement).toBe(anchor)
    anchor.remove()
  })

  it('search finds emoji; ↓ enters the results; Enter in the search picks the first', () => {
    const onPick = vi.fn()
    render(<HomeChatReactionPicker anchor={null} onPick={onPick} onClose={vi.fn()} />)
    const search = screen.getByTestId('home-chat-emoji-search')
    fireEvent.change(search, { target: { value: 'heart' } })
    const results = screen.getByTestId('home-chat-emoji-results').querySelectorAll('button')
    expect(results.length).toBeGreaterThan(0)
    fireEvent.keyDown(search, { key: 'ArrowDown' })
    expect(document.activeElement).toBe(results[0])
    fireEvent.keyDown(search, { key: 'Enter' })
    expect(onPick).toHaveBeenCalledWith(results[0].getAttribute('data-emoji'))
  })

  it('Esc closes it from anywhere inside, and goes no further', () => {
    const onClose = vi.fn()
    const outer = vi.fn()
    document.addEventListener('keydown', outer)
    render(<HomeChatReactionPicker anchor={null} onPick={vi.fn()} onClose={onClose} />)
    fireEvent.keyDown(screen.getByTestId('home-chat-quick-reactions').querySelector('button')!, { key: 'Escape' })
    expect(onClose).toHaveBeenCalledTimes(1)
    expect(outer).not.toHaveBeenCalled()
    document.removeEventListener('keydown', outer)
  })
})
