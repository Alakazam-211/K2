// @vitest-environment jsdom
// Reactions and the action bar on a Home chat message (K2 Design pass):
//   1. Chips: the emoji and count, a ring when yours, a tip naming who.
//   2. The picker: six quick emoji and a search, all by keyboard; Esc
//      closes it and focus goes back to the React button.
//   3. The action bar: React (an icon), Reply, More; beside the bubble on
//      the open side, so it never covers the words.
//   4. A jumped-to message gets a wash, not a flash.

import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import {
  HomeChatReactionChips,
  HomeChatReactionPicker,
  PICKER_COLUMNS,
  pickerMove,
  reactionTipText,
  reactorsLine,
} from './HomeChatReactions'
import { HomeChatMessageRow, actionBarPlace, type HomeChatRowActions } from './HomeChatMessageRow'
import type { HomeChatMessage } from '@/lib/home-chat/api'

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

describe('the action bar', () => {
  it('sits beside the bubble on the open side, level with its top edge', () => {
    // Theirs: to the right of the bubble.
    expect(actionBarPlace({ width: 800 }, { left: 0, right: 300, top: 20 }, 90, false)).toEqual({ top: 12, inset: 306, beside: true })
    // Yours: to its left (inset from the row's right edge).
    expect(actionBarPlace({ width: 800 }, { left: 500, right: 800, top: 0 }, 90, true)).toEqual({ top: -8, inset: 306, beside: true })
  })

  it('a bubble with no room beside it gets the bar over its outer top corner', () => {
    const p = actionBarPlace({ width: 400 }, { left: 0, right: 360, top: 20 }, 90, false)
    expect(p.beside).toBe(false)
    expect(p.inset).toBe(40)
    // Wholly above the bubble's words (the bubble's padding is under its last 4px).
    expect(p.top).toBeLessThan(20)
  })

  const msg: HomeChatMessage = { id: '5', conv: '7', authorId: '2', body: 'hello', createdAt: 1, seq: 5, reactions: [], replyTo: null, deleted: false, nonce: null }
  const actions: HomeChatRowActions = { react: vi.fn(), reply: vi.fn(), jump: vi.fn(), who: async () => [] }
  const row = (o: Partial<{ flash: boolean; readOnly: boolean }> = {}) => (
    <HomeChatMessageRow m={msg} mine={false} author="Ana" timeLabel="now" still runStart tail reply={null} flash={o.flash ?? false} readOnly={o.readOnly ?? false} actions={actions} />
  )

  it('React is an icon button (no text glyph); the bar is a toolbar of React, Reply, More', () => {
    render(row())
    const react = screen.getByTestId('home-chat-react')
    expect(react.querySelector('svg')).not.toBeNull()
    expect(react.textContent).toBe('')
    const bar = screen.getByRole('toolbar', { name: 'Message actions' })
    expect([...bar.querySelectorAll('button')].map((b) => b.getAttribute('aria-label'))).toEqual(['React', 'Reply', 'More'])
    // Shown on hover and on keyboard focus inside the message.
    const holder = bar.parentElement!
    expect(holder.className).toContain('group-hover:opacity-100')
    expect(holder.className).toContain('focus-within:opacity-100')
  })

  it('React opens the picker; Esc closes it and puts focus back on React', async () => {
    render(row())
    await act(async () => {
      fireEvent.click(screen.getByTestId('home-chat-react'))
    })
    expect(screen.getByTestId('home-chat-reaction-picker')).toBeTruthy()
    await act(async () => {
      fireEvent.keyDown(screen.getByTestId('home-chat-emoji-search'), { key: 'Escape' })
    })
    expect(screen.queryByTestId('home-chat-reaction-picker')).toBeNull()
    expect(document.activeElement).toBe(screen.getByTestId('home-chat-react'))
  })

  it('a jumped-to message gets a soft accent wash', () => {
    const { rerender, container } = render(row())
    expect(container.querySelector('[data-home-chat-wash]')).toBeNull()
    rerender(row({ flash: true }))
    const wash = container.querySelector<HTMLElement>('[data-home-chat-wash]')!
    expect(wash.style.background).toContain('var(--zen-accent) 18%')
  })
})
