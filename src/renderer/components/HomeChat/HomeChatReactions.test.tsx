// @vitest-environment jsdom
// Reactions and the action bar on a Home chat message (K2 Design pass):
//   1. Chips: the emoji and count, a ring when yours, a tip naming who.
//   2. The picker: six quick emoji and a search, all by keyboard; Esc
//      closes it and focus goes back to the React button.
// (The hover row on a message: HomeChatHoverRow.test.tsx.)

import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import {
  HomeChatReactionChips,
  HomeChatReactionPicker,
  PICKER_COLUMNS,
  pickerMove,
  reactionTipTop,
  reactionTipText,
  reactorsLine,
} from './HomeChatReactions'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

afterEach(() => cleanup())

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
    expect(chips[0].style.border).toContain('var(--zen-focus-ring)')
    expect(chips[1].style.border).toContain('var(--zen-control-edge)')
    // A chip in each look: the Thread's chamfer in Terminal, a pill in Glass.
    expect(chips[0].hasAttribute('data-chamfer')).toBe(true)
    expect(chips[0].className).toContain('rounded-[var(--radius-pill)]')
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
