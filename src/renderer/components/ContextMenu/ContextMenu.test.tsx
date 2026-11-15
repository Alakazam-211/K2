// @vitest-environment jsdom
//
// The shared right-click menu's surface: cut like K2's other menus in
// Terminal, Frost in Glass (the shared row menu, OverflowMenu, is Frost).
import { afterEach, describe, expect, it } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import ContextMenu from './ContextMenu'
import { useContextMenuStore } from '@/stores/context-menu'

afterEach(() => {
  act(() => useContextMenuStore.getState().close())
  cleanup()
})

function open(): void {
  act(() => {
    void useContextMenuStore.getState().show(40, 40, [
      { id: 'settings', label: 'Project settings' },
      { id: 'sep', label: '', type: 'separator' },
      { id: 'more', label: 'Move to', submenu: [{ id: 'top', label: 'Top' }] },
    ])
  })
}

describe('the right-click menu', () => {
  it('is a cut box in a drop-shadow wrapper (Terminal) and Frost (Glass)', () => {
    render(<ContextMenu />)
    open()
    const box = document.querySelector<HTMLElement>('[data-context-menu]')
    expect(box).not.toBeNull()
    expect(box?.getAttribute('data-k2-chamfer')).toBe('surface')
    expect(box?.getAttribute('data-material')).toBe('frost')
    expect(box?.getAttribute('data-float')).toBe('menu')
    expect(box?.hasAttribute('data-floating')).toBe(false)
    expect(box?.parentElement?.getAttribute('data-k2-chamfer-float')).toBe('menu')
    expect(box?.parentElement?.style.position).toBe('fixed')
    expect(screen.getByRole('button', { name: 'Project settings' })).toBeTruthy()
  })

  it('the clear look (the Home nav’s agent menu) marks the box; it closes back to the default', () => {
    render(<ContextMenu />)
    act(() => {
      void useContextMenuStore.getState().show(40, 40, [{ id: 'pin', label: 'Pin' }], { look: 'clear' })
    })
    const box = document.querySelector<HTMLElement>('[data-context-menu]')
    expect(box?.hasAttribute('data-float-clear')).toBe(true)
    expect(box?.getAttribute('data-material')).toBe('frost')
    expect(box?.getAttribute('data-k2-chamfer')).toBe('surface')
    act(() => useContextMenuStore.getState().close())
    open()
    expect(document.querySelector('[data-context-menu]')?.hasAttribute('data-float-clear')).toBe(false)
  })

  it('a submenu is the same surface', () => {
    render(<ContextMenu />)
    open()
    fireEvent.click(screen.getByRole('button', { name: /Move to/ }))
    const sub = document.querySelector<HTMLElement>('[data-context-submenu]')
    expect(sub).not.toBeNull()
    expect(sub?.getAttribute('role')).toBe('menu')
    expect(sub?.getAttribute('data-k2-chamfer')).toBe('surface')
    expect(sub?.getAttribute('data-material')).toBe('frost')
    expect(sub?.parentElement?.getAttribute('data-k2-chamfer-float')).toBe('menu')
    expect(screen.getByRole('button', { name: 'Top' })).toBeTruthy()
  })

  it('a danger row is quiet at rest and red when lit (as OverflowMenu draws one)', () => {
    render(<ContextMenu />)
    act(() => {
      void useContextMenuStore.getState().show(40, 40, [
        { id: 'rename', label: 'Rename' },
        { id: 'delete', label: 'Delete…', danger: true },
      ])
    })
    const del = screen.getByRole('button', { name: 'Delete…' })
    expect(del.style.color).toBe('var(--color-text-secondary)')
    fireEvent.mouseEnter(del)
    expect(del.style.color).toBe('var(--color-status-error-soft)')
    // A plain row lit is not red.
    const rename = screen.getByRole('button', { name: 'Rename' })
    fireEvent.mouseEnter(rename)
    expect(rename.style.color).toBe('var(--color-text-secondary)')
  })
})
