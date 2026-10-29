// @vitest-environment jsdom
// Fail loud: the palette (Select) toggle and its colour, the tray's Send all
// and per-tile Send, the pop-up's overlay claim (which freezes the page) and its keys,
// the tray's actions, the web stub, and the "Show the Design tab" switch.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliPost: vi.fn(async () => ({ ok: true })) }))
const PRIMARY = { id: 'primary', isPrimary: true, isRemote: false } as unknown as import('@/kessel/server-scope').ServerScope
vi.mock('@/stores/projects', async () => {
  const { create } = await import('zustand')
  return {
    useProjectsStore: create<{ projects: { id: string; path: string; designModeEnabled?: number }[] }>(() => ({
      projects: [{ id: 'p1', path: '/w/docs', designModeEnabled: 1 }],
    })),
  }
})

import { daemonCliPost } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'
import { overlayClaims, resetBrowserOverlaysForTest } from '@/lib/browser-overlay'
import { DesignPaletteButton } from './DesignToolbar'
import { NotePopover, sendNowChord } from './NotePopover'
import { NoteTray } from './NoteTray'
import { DesignWebStub } from './DesignWebStub'
import { DesignModeSettingRow } from './DesignModeSettingRow'
import type { DesignNoteDraft, NoteElement } from './types'

class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', ResizeObserverStub)
  resetBrowserOverlaysForTest()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

const element: NoteElement = { selector: 'button.cta', tag: 'button', classes: ['cta'], text: 'Buy', html: '' }

describe('DesignPaletteButton', () => {
  it('is the Select toggle: accent while on, the normal icon colour while off', () => {
    const onToggle = vi.fn()
    const { rerender } = render(<DesignPaletteButton armed onToggle={onToggle} />)
    const btn = screen.getByRole('button', { name: 'Select an element' })
    expect(btn.getAttribute('aria-pressed')).toBe('true')
    expect(btn.className).toContain('text-[var(--color-accent)]')
    expect(btn.querySelector('[data-k2-icon="palette"]')).toBeTruthy()
    fireEvent.click(btn)
    expect(onToggle).toHaveBeenCalledTimes(1)
    rerender(<DesignPaletteButton armed={false} onToggle={onToggle} />)
    const off = screen.getByRole('button', { name: 'Select an element' })
    expect(off.getAttribute('aria-pressed')).toBe('false')
    expect(off.className).not.toContain('text-[var(--color-accent)]')
    expect(off.className).toContain('text-[var(--color-text-secondary)]')
  })
})

describe('NotePopover', () => {
  const anchor = { x: 100, y: 100, width: 50, height: 20 }
  const container = { x: 0, y: 0, width: 1000, height: 800 }

  it('claims the overlay while open (the page freezes) and releases on close', () => {
    const onAdd = vi.fn()
    const onCancel = vi.fn()
    const { rerender } = render(
      <NotePopover open element={element} anchor={anchor} container={container} onAdd={onAdd} onCancel={onCancel} />,
    )
    expect(overlayClaims()).toHaveLength(1)
    expect(overlayClaims()[0].kind).toBe('popover')
    expect(screen.getByTestId('design-element-chip').textContent).toBe('<button.cta>')
    rerender(<NotePopover open={false} element={element} anchor={anchor} container={container} onAdd={onAdd} onCancel={onCancel} />)
    expect(overlayClaims()).toHaveLength(0)
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('unmounting releases the claim (no stuck frozen page)', () => {
    const { unmount } = render(
      <NotePopover open element={element} anchor={anchor} container={container} onAdd={vi.fn()} onCancel={vi.fn()} />,
    )
    expect(overlayClaims()).toHaveLength(1)
    unmount()
    expect(overlayClaims()).toHaveLength(0)
  })

  it('is a New Note tile with Change | Question tabs', () => {
    render(<NotePopover open element={element} anchor={anchor} container={container} onAdd={vi.fn()} onSendNow={vi.fn()} onCancel={vi.fn()} />)
    expect(screen.getByTestId('design-new-note-header').textContent).toContain('New Note')
    const tabs = screen.getAllByRole('tab')
    expect(tabs.map((t) => t.textContent)).toEqual(['Change', 'Question'])
    expect(screen.getByRole('tablist', { name: 'Kind of note' })).toBeTruthy()
    expect(tabs[0].getAttribute('aria-selected')).toBe('true')
    fireEvent.keyDown(tabs[0], { key: 'ArrowRight' })
    expect(screen.getByRole('tab', { name: 'Question' }).getAttribute('aria-selected')).toBe('true')
    fireEvent.keyDown(screen.getByRole('tab', { name: 'Question' }), { key: 'ArrowRight' })
    expect(screen.getByRole('tab', { name: 'Change' }).getAttribute('aria-selected')).toBe('true')
  })

  it('⌘↩ is Send now: only this note, with the chosen kind; plain Return does not', () => {
    const onAdd = vi.fn()
    const onSendNow = vi.fn()
    render(<NotePopover open element={element} anchor={anchor} container={container} onAdd={onAdd} onSendNow={onSendNow} onCancel={vi.fn()} />)
    const box = screen.getByRole('textbox')
    fireEvent.keyDown(box, { key: 'Enter', metaKey: true })
    expect(onSendNow).not.toHaveBeenCalled()
    fireEvent.change(box, { target: { value: '  make it primary  ' } })
    fireEvent.click(screen.getByRole('tab', { name: 'Question' }))
    fireEvent.keyDown(box, { key: 'Enter' })
    expect(onSendNow).not.toHaveBeenCalled()
    fireEvent.keyDown(box, { key: 'Enter', metaKey: true })
    expect(onSendNow).toHaveBeenCalledWith('make it primary', 'question')
    fireEvent.keyDown(box, { key: 'Enter', ctrlKey: true })
    expect(onSendNow).toHaveBeenCalledTimes(2)
    fireEvent.click(screen.getByRole('button', { name: 'Send now' }))
    expect(onSendNow).toHaveBeenCalledTimes(3)
    expect(onAdd).not.toHaveBeenCalled()
  })

  it('Send now shows the chord in the system font: ⌘↩ on macOS, Ctrl+Enter elsewhere', () => {
    expect(sendNowChord(true)).toBe('⌘↩')
    expect(sendNowChord(false)).toBe('Ctrl+Enter')
    render(<NotePopover open element={element} anchor={anchor} container={container} onAdd={vi.fn()} onSendNow={vi.fn()} onCancel={vi.fn()} />)
    const kbd = screen.getByRole('button', { name: 'Send now' }).querySelector('[data-testid="design-shortcut"]') as HTMLElement
    expect(kbd.tagName).toBe('KBD')
    expect(kbd.style.fontFamily).toContain('-apple-system')
    expect(kbd.className).not.toContain('font-mono')
    expect(['⌘↩', 'Ctrl+Enter']).toContain(kbd.textContent)
  })

  it('Add pins the note; a refused Add shows the notice and keeps the editor', () => {
    const onAdd = vi.fn()
    const { rerender } = render(<NotePopover open element={element} anchor={anchor} container={container} onAdd={onAdd} onCancel={vi.fn()} />)
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'x' } })
    const add = screen.getByRole('button', { name: 'Add' }) as HTMLButtonElement
    expect(add.disabled).toBe(false)
    fireEvent.click(add)
    expect(onAdd.mock.calls).toEqual([['x', 'change']])
    rerender(<NotePopover open element={element} anchor={anchor} container={container} onAdd={onAdd} notice="You've hit the limit of 9 notes. Send some first." onCancel={vi.fn()} />)
    expect(screen.getByTestId('design-popover-notice').textContent).toBe("You've hit the limit of 9 notes. Send some first.")
    expect((screen.getByRole('button', { name: 'Add' }) as HTMLButtonElement).disabled).toBe(false)
  })

  it('Esc cancels', () => {
    const onCancel = vi.fn()
    render(<NotePopover open element={element} anchor={anchor} container={container} onAdd={vi.fn()} onCancel={onCancel} />)
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Escape' })
    expect(onCancel).toHaveBeenCalledTimes(1)
  })

  it('sits below the element when it fits', () => {
    render(<NotePopover open element={element} anchor={anchor} container={container} onAdd={vi.fn()} onCancel={vi.fn()} />)
    expect(screen.getByRole('dialog').getAttribute('data-side')).toBe('bottom')
  })
})

describe('NoteTray', () => {
  const notes: DesignNoteDraft[] = [
    { id: 'a', comment: 'first', intent: 'change', element, url: 'http://x/', title: '' },
    { id: 'b', comment: 'second', intent: 'question', url: 'http://x/', title: '' },
  ]
  function tray(over: Partial<Parameters<typeof NoteTray>[0]> = {}) {
    const props = {
      notes,
      sending: false,
      error: null,
      canRetry: false,
      onEdit: vi.fn(),
      onRemove: vi.fn(),
      onClear: vi.fn(),
      onSend: vi.fn(),
      onRetry: vi.fn(),
      ...over,
    }
    render(<NoteTray {...props} />)
    return props
  }

  it('renders nothing with no notes and no error', () => {
    tray({ notes: [] })
    expect(screen.queryByTestId('design-tray')).toBeNull()
  })

  it('tiles: number and text on one row, edit pencil and trash icons, in a 3-wide grid', () => {
    tray()
    const grid = screen.getByTestId('design-tile-grid')
    expect(grid.className).toContain('grid-cols-3')
    const tiles = grid.querySelectorAll('[data-design-tile]')
    expect(tiles).toHaveLength(2)
    const first = tiles[0] as HTMLElement
    expect(first.querySelector('[data-testid="design-tile-n"]')?.textContent).toBe('1')
    expect(first.querySelector('[data-testid="design-tile-text"]')?.textContent).toBe('first')
    // One row: number, text and the two buttons are siblings in a flex row.
    expect(first.className).toContain('flex')
    expect(first.className).toContain('items-center')
    expect(first.querySelector('[data-k2-icon="pencil"]')).toBeTruthy()
    expect(first.querySelector('[data-k2-icon="trash-2"]')).toBeTruthy()
    // The count tag, nav style; the limit is never shown.
    expect(screen.getByTestId('design-note-count').textContent).toBe('2')
    expect(screen.getByTestId('design-note-count').hasAttribute('data-nav-count-tag')).toBe(true)
    expect(screen.getByTestId('design-tray').textContent).not.toContain('9')
    // The tile shows its kind.
    expect(first.querySelector('[data-testid="design-tile-kind"]')?.getAttribute('data-kind')).toBe('change')
  })

  it('a tile has its own Send between the pencil and the trash', () => {
    const onSendOne = vi.fn()
    tray({ onSendOne })
    const tile = screen.getByTestId('design-tile-grid').querySelectorAll('[data-design-tile]')[1] as HTMLElement
    const buttons = Array.from(tile.querySelectorAll('button')).map((b) => b.getAttribute('aria-label'))
    expect(buttons).toEqual(['Edit note 2', 'Send note 2', 'Delete note 2'])
    const send = screen.getByRole('button', { name: 'Send note 2' })
    expect(send.getAttribute('title')).toBe('Send this note')
    expect(send.querySelector('[data-k2-icon="arrow-up"]')).toBeTruthy()
    fireEvent.click(send)
    expect(onSendOne).toHaveBeenCalledWith('b')
  })

  it('Send all sits in the nav-style notes header, centred, with no shortcut glyphs', () => {
    const props = tray()
    const header = screen.getByTestId('design-tray-header')
    expect(header.className).toContain('items-center')
    expect(header.className).toContain('py-1.5')
    expect(header.querySelector('[data-nav-section-label]')?.textContent).toBe('Notes')
    expect(header.querySelector('[data-nav-section-chevron]')).toBeTruthy()
    const sendAll = screen.getByRole('button', { name: 'Send all' })
    expect(header.contains(sendAll)).toBe(true)
    expect(sendAll.querySelector('kbd')).toBeNull()
    expect(sendAll.textContent).toBe('Send all')
    fireEvent.click(sendAll)
    expect(props.onSend).toHaveBeenCalledTimes(1)
  })

  it('deletes, clears, sends, and collapses', () => {
    const props = tray()
    fireEvent.click(screen.getByRole('button', { name: 'Delete note 2' }))
    expect(props.onRemove).toHaveBeenCalledWith('b')
    fireEvent.click(screen.getByRole('button', { name: 'Clear' }))
    expect(props.onClear).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole('button', { name: 'Send all' }))
    expect(props.onSend).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole('button', { name: 'Notes, 2' }))
    expect(screen.queryByTestId('design-tile-grid')).toBeNull()
    expect(document.querySelector('[data-nav-section-chevron="collapsed"]')).toBeTruthy()
  })

  it('edits inline: Enter saves, Esc cancels', () => {
    const props = tray()
    fireEvent.click(screen.getByRole('button', { name: 'Edit note 1' }))
    const box = screen.getByRole('textbox', { name: 'Edit note 1' })
    fireEvent.change(box, { target: { value: 'first, bigger' } })
    fireEvent.keyDown(box, { key: 'Enter' })
    expect(props.onEdit).toHaveBeenCalledWith('a', 'first, bigger')
    expect(screen.queryByRole('textbox', { name: 'Edit note 1' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Edit note 2' }))
    const box2 = screen.getByRole('textbox', { name: 'Edit note 2' })
    fireEvent.change(box2, { target: { value: 'never saved' } })
    fireEvent.keyDown(box2, { key: 'Escape' })
    expect(props.onEdit).toHaveBeenCalledTimes(1)
  })

  it('nine tiles fill the 3 × 3 grid with no limit text', () => {
    const nine: DesignNoteDraft[] = Array.from({ length: 9 }, (_, i) => ({ id: `n${i}`, comment: `note ${i}`, intent: 'change', url: 'http://x/', title: '' }))
    tray({ notes: nine })
    expect(screen.getByTestId('design-tile-grid').querySelectorAll('[data-design-tile]')).toHaveLength(9)
    expect(screen.queryByTestId('design-tray-full')).toBeNull()
    expect(screen.getByTestId('design-note-count').textContent).toBe('9')
  })

  it('locked notes cannot be edited; the error line and Retry show', () => {
    const props = tray({ canRetry: true, lockedNoteIds: ['a'], error: 'agent did not get it' })
    expect((screen.getByRole('button', { name: 'Edit note 1' }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByRole('alert').textContent).toBe('agent did not get it')
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(props.onRetry).toHaveBeenCalledTimes(1)
  })
})

describe('DesignWebStub', () => {
  it('says Design needs the desktop app and links the Thread', () => {
    const onOpenThread = vi.fn()
    render(<DesignWebStub onOpenThread={onOpenThread} />)
    expect(screen.getByText('Design needs the desktop app')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Open the Thread' }))
    expect(onOpenThread).toHaveBeenCalledTimes(1)
  })
})

describe('DesignModeSettingRow', () => {
  it('turns "Show the Design tab" off for the workspace and updates the projects store', async () => {
    render(<DesignModeSettingRow scope={PRIMARY} project={{ id: 'p1', path: '/w/docs', designModeEnabled: 1 }} />)
    const sw = screen.getByRole('switch', { name: 'Show the Design tab' })
    expect(sw.getAttribute('aria-checked')).toBe('true')
    await act(async () => {
      fireEvent.click(sw)
    })
    expect(vi.mocked(daemonCliPost)).toHaveBeenCalledWith(
      expect.objectContaining({ isPrimary: true }),
      'workspace/set',
      { project: '/w/docs', fields: { design_mode_enabled: '0' } },
    )
    expect(useProjectsStore.getState().projects[0].designModeEnabled).toBe(0)
  })

  it('a missing value reads as on', () => {
    render(<DesignModeSettingRow scope={PRIMARY} project={{ id: 'p2', path: '/w/x' }} />)
    expect(screen.getByRole('switch', { name: 'Show the Design tab' }).getAttribute('aria-checked')).toBe('true')
  })
})
