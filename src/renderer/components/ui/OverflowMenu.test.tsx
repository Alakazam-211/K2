// @vitest-environment jsdom
// The row "…" menu: opens on the first enabled item, arrows skip disabled
// items, a disabled item says why and does nothing, and Esc closes it without
// also reaching Settings (which leaves an Esc alone once it's been claimed).
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'

vi.mock('@/lib/browser-overlay', () => ({ useBrowserOverlay: () => undefined }))

import { OverflowMenu, type OverflowMenuItem } from './OverflowMenu'

afterEach(cleanup)

function setup(): { items: OverflowMenuItem[]; trigger: HTMLElement } {
  const items: OverflowMenuItem[] = [
    { id: 'pair', label: 'Pair', disabled: true, hint: 'Turn on federation first', onSelect: vi.fn() },
    { id: 'restart', label: 'Restart', onSelect: vi.fn() },
    { id: 'edit', label: 'Edit', group: true, onSelect: vi.fn() },
  ]
  render(<OverflowMenu items={items} label="More actions for z3" />)
  return { items, trigger: screen.getByRole('button', { name: 'More actions for z3' }) }
}

describe('OverflowMenu', () => {
  it('is closed until asked, then focuses the first enabled item', () => {
    const { trigger } = setup()
    expect(screen.queryByRole('menu')).toBeNull()
    fireEvent.click(trigger)
    expect(trigger.getAttribute('aria-expanded')).toBe('true')
    expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'Restart' }))
  })

  it('shows why an item is disabled, and picking it does nothing', () => {
    const { items, trigger } = setup()
    fireEvent.click(trigger)
    expect(screen.getByText('Turn on federation first')).toBeTruthy()
    fireEvent.click(screen.getByRole('menuitem', { name: /Pair/ }))
    expect(items[0].onSelect).not.toHaveBeenCalled()
    expect(screen.getByRole('menu')).toBeTruthy()
  })

  it('arrows move between enabled items and wrap', () => {
    const { trigger } = setup()
    fireEvent.click(trigger)
    const menu = screen.getByRole('menu')
    fireEvent.keyDown(menu, { key: 'ArrowDown' })
    expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'Edit' }))
    fireEvent.keyDown(menu, { key: 'ArrowDown' })
    expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: 'Restart' }))
  })

  it('picking an item runs it, closes, and returns focus to the button', () => {
    const { items, trigger } = setup()
    fireEvent.click(trigger)
    fireEvent.click(screen.getByRole('menuitem', { name: 'Restart' }))
    expect(items[1].onSelect).toHaveBeenCalledTimes(1)
    expect(screen.queryByRole('menu')).toBeNull()
    expect(document.activeElement).toBe(trigger)
  })

  it('Esc closes and claims the key, so Settings stays open', () => {
    const { trigger } = setup()
    fireEvent.click(trigger)
    const notPrevented = fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' })
    expect(notPrevented).toBe(false)
    expect(screen.queryByRole('menu')).toBeNull()
    expect(document.activeElement).toBe(trigger)
  })
})
