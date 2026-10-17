// @vitest-environment jsdom
// Fail loud: the palette (Select) toggle and its colour, Send disabled on an empty
// tray, the pop-up's overlay claim (which freezes the page) and its keys,
// the tray's actions, the web stub, and the Design mode switch.
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
import { DesignPaletteButton, DesignToolbar } from './DesignToolbar'
import { NotePopover } from './NotePopover'
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

describe('DesignToolbar', () => {
  function toolbar(over: Partial<Parameters<typeof DesignToolbar>[0]> = {}) {
    const props = {
      noteCount: 0,
      sending: false,
      canRetry: false,
      onSend: vi.fn(),
      onRetry: vi.fn(),
      ...over,
    }
    const r = render(<DesignToolbar {...props} />)
    return { ...r, props }
  }

  it('has no Select button and never mentions an S key', () => {
    toolbar({ armed: true })
    expect(screen.queryByRole('button', { name: /select/i })).toBeNull()
    const bar = screen.getByTestId('design-toolbar')
    expect(bar.textContent).not.toMatch(/\bS\b/)
    expect(bar.querySelector('kbd')).toBeNull()
  })

  it('Send is disabled with no notes and enabled with some', () => {
    const { props, rerender } = toolbar()
    expect((screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement).disabled).toBe(true)
    rerender(<DesignToolbar {...props} noteCount={2} />)
    const send = screen.getByRole('button', { name: 'Send' }) as HTMLButtonElement
    expect(send.disabled).toBe(false)
    expect(screen.getByTestId('design-note-count').textContent).toBe('2 notes')
    fireEvent.click(send)
    expect(props.onSend).toHaveBeenCalledTimes(1)
  })

  it('shows Retry instead of Send when an item is undelivered', () => {
    const { props } = toolbar({ noteCount: 1, canRetry: true })
    expect(screen.queryByRole('button', { name: 'Send' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(props.onRetry).toHaveBeenCalledTimes(1)
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

  it('⌘↩ sends now with the chosen intent; plain Return does not', () => {
    const onAdd = vi.fn()
    const onSendNow = vi.fn()
    render(<NotePopover open element={element} anchor={anchor} container={container} onAdd={onAdd} onSendNow={onSendNow} onCancel={vi.fn()} />)
    const box = screen.getByRole('textbox')
    // Nothing typed and an empty tray: nothing to send.
    fireEvent.keyDown(box, { key: 'Enter', metaKey: true })
    expect(onSendNow).not.toHaveBeenCalled()
    fireEvent.change(box, { target: { value: '  make it primary  ' } })
    fireEvent.click(screen.getByRole('radio', { name: 'Question' }))
    fireEvent.keyDown(box, { key: 'Enter' })
    expect(onSendNow).not.toHaveBeenCalled()
    fireEvent.keyDown(box, { key: 'Enter', metaKey: true })
    expect(onSendNow).toHaveBeenCalledWith('make it primary', 'question')
    expect(onAdd).not.toHaveBeenCalled()
  })

  it('Ctrl+Enter sends now too; with nothing typed it sends the tray', () => {
    const onSendNow = vi.fn()
    render(<NotePopover open element={element} anchor={anchor} container={container} onAdd={vi.fn()} onSendNow={onSendNow} trayCount={2} onCancel={vi.fn()} />)
    expect(screen.getByTestId('design-popover-hint').textContent).toContain('the 2 notes')
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter', ctrlKey: true })
    expect(onSendNow).toHaveBeenCalledWith('', 'change')
  })

  it('the Add button adds; a full tray turns Add off with the hint', () => {
    const onAdd = vi.fn()
    const { rerender } = render(<NotePopover open element={element} anchor={anchor} container={container} onAdd={onAdd} onCancel={vi.fn()} />)
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'x' } })
    fireEvent.click(screen.getByRole('button', { name: 'Add' }))
    expect(onAdd.mock.calls).toEqual([['x', 'change']])
    rerender(<NotePopover open element={element} anchor={anchor} container={container} onAdd={onAdd} addDisabledReason="Send these first; up to 6 notes" onCancel={vi.fn()} />)
    const add = screen.getByRole('button', { name: 'Add' }) as HTMLButtonElement
    expect(add.disabled).toBe(true)
    expect(screen.getByTestId('design-popover-hint').textContent).toContain('Send these first; up to 6 notes')
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

  it('numbers notes, deletes, clears, sends, and collapses', () => {
    const props = tray()
    expect(screen.getAllByRole('listitem')).toHaveLength(2)
    fireEvent.click(screen.getByRole('button', { name: 'Delete note 2' }))
    expect(props.onRemove).toHaveBeenCalledWith('b')
    fireEvent.click(screen.getByRole('button', { name: 'Clear' }))
    expect(props.onClear).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    expect(props.onSend).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole('button', { name: /Notes \(2\)/ }))
    expect(screen.queryAllByRole('listitem')).toHaveLength(0)
  })

  it('edits a note with ⌘Enter', () => {
    const props = tray()
    fireEvent.click(screen.getByRole('button', { name: 'Edit note 1' }))
    const box = screen.getByRole('textbox', { name: 'Edit note 1' })
    fireEvent.change(box, { target: { value: 'first, bigger' } })
    fireEvent.keyDown(box, { key: 'Enter', metaKey: true })
    expect(props.onEdit).toHaveBeenCalledWith('a', 'first, bigger')
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
  it('turns Design mode off for the workspace and updates the projects store', async () => {
    render(<DesignModeSettingRow scope={PRIMARY} project={{ id: 'p1', path: '/w/docs', designModeEnabled: 1 }} />)
    const sw = screen.getByRole('switch', { name: 'Design mode' })
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
    expect(screen.getByRole('switch', { name: 'Design mode' }).getAttribute('aria-checked')).toBe('true')
  })
})
