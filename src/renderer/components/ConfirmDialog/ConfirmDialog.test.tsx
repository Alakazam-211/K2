// @vitest-environment jsdom
// The app's confirm prompt cancels cleanly: the × (Close (Esc)), Esc and a
// press outside each resolve `confirm()` to false, once, and close it.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import ConfirmDialog from './ConfirmDialog'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

vi.mock('@/lib/browser-overlay', () => ({ useBrowserOverlay: () => {} }))

afterEach(() => {
  cleanup()
  useConfirmDialogStore.setState({ isOpen: false, onResolve: null, choices: null, onChoose: null, safeDefault: false, cancelLabel: 'Cancel' })
})

function ask(): { pending: Promise<boolean> } {
  render(
    <>
      <button type="button" data-testid="page">Page</button>
      <ConfirmDialog />
    </>,
  )
  let pending!: Promise<boolean>
  act(() => {
    pending = useConfirmDialogStore.getState().confirm({ title: 'Remove it?', message: 'It goes to the Trash.', confirmLabel: 'Remove', destructive: true })
  })
  screen.getByText('Remove it?')
  return { pending }
}

function expectClosed(): void {
  expect(useConfirmDialogStore.getState().isOpen).toBe(false)
  expect(screen.queryByText('Remove it?')).toBeNull()
}

describe('ConfirmDialog cancels to false', () => {
  it('the ×', async () => {
    const { pending } = ask()
    const close = screen.getByRole('button', { name: 'Close (Esc)' })
    expect(close.getAttribute('title')).toBe('Close (Esc)')
    fireEvent.click(close)
    // Closed at once (fails here, not on a timeout, when nothing cancels).
    expect(useConfirmDialogStore.getState().isOpen).toBe(false)
    expect(await pending).toBe(false)
    expectClosed()
  })

  it('Esc', async () => {
    const { pending } = ask()
    fireEvent.keyDown(document.body, { key: 'Escape' })
    // Closed at once (fails here, not on a timeout, when nothing cancels).
    expect(useConfirmDialogStore.getState().isOpen).toBe(false)
    expect(await pending).toBe(false)
    expectClosed()
  })

  it('a press outside the dialog', async () => {
    const { pending } = ask()
    fireEvent.pointerDown(screen.getByTestId('page'))
    // Closed at once (fails here, not on a timeout, when nothing cancels).
    expect(useConfirmDialogStore.getState().isOpen).toBe(false)
    expect(await pending).toBe(false)
    expectClosed()
  })

  it('Confirm still resolves true (a press inside never cancels)', async () => {
    const { pending } = ask()
    const confirm = screen.getByRole('button', { name: 'Remove' })
    fireEvent.pointerDown(confirm)
    fireEvent.click(confirm)
    expect(await pending).toBe(true)
    expectClosed()
  })
})

describe('ConfirmDialog with two answers (choose)', () => {
  function choose(): { pending: Promise<string | null> } {
    render(<ConfirmDialog />)
    let pending!: Promise<string | null>
    act(() => {
      pending = useConfirmDialogStore.getState().choose({
        title: 'Same person?',
        message: 'Pick one.',
        choices: [
          { id: 'different', label: 'Different person' },
          { id: 'same', label: 'Same person', primary: true },
        ],
      })
    })
    screen.getByText('Same person?')
    return { pending }
  }

  it('shows Cancel and both answers, in order', () => {
    choose()
    const labels = screen.getAllByRole('button').map((b) => b.textContent).filter((t) => t && t !== '')
    expect(labels.slice(-3)).toEqual(['Cancel', 'Different person', 'Same person'])
  })

  it('each answer resolves its id', async () => {
    const { pending } = choose()
    fireEvent.click(screen.getByText('Different person'))
    await expect(pending).resolves.toBe('different')
    expect(useConfirmDialogStore.getState().isOpen).toBe(false)
  })

  it('Enter gives the primary answer', async () => {
    const { pending } = choose()
    fireEvent.keyDown(window, { key: 'Enter' })
    await expect(pending).resolves.toBe('same')
  })

  it('Cancel, Esc and the × resolve null, never an answer', async () => {
    let p = choose().pending
    fireEvent.click(screen.getByText('Cancel'))
    await expect(p).resolves.toBeNull()
    cleanup()
    p = choose().pending
    fireEvent.keyDown(document, { key: 'Escape' })
    await expect(p).resolves.toBeNull()
    cleanup()
    p = choose().pending
    fireEvent.click(screen.getByRole('button', { name: 'Close (Esc)' }))
    await expect(p).resolves.toBeNull()
  })

  it('a plain confirm is unchanged: one confirm button, true on confirm', async () => {
    const { pending } = ask()
    expect(document.querySelector('[data-confirm-choice]')).toBeNull()
    fireEvent.click(screen.getByText('Remove'))
    await expect(pending).resolves.toBe(true)
  })
})

describe('ConfirmDialog with a safe default', () => {
  it('names its no-op (Keep it), focuses it, and Enter keeps things as they are', async () => {
    render(<ConfirmDialog />)
    let pending!: Promise<boolean>
    act(() => {
      pending = useConfirmDialogStore.getState().confirm({ title: 'Delete #general?', message: 'Gone for posting.', confirmLabel: 'Delete', cancelLabel: 'Keep it', destructive: true, safeDefault: true })
    })
    const keep = screen.getByText('Keep it')
    expect(document.activeElement).toBe(keep)
    // Enter goes to the focused button (the browser's own press), never the window's confirm.
    fireEvent.keyDown(window, { key: 'Enter' })
    expect(useConfirmDialogStore.getState().isOpen).toBe(true)
    fireEvent.click(keep)
    await expect(pending).resolves.toBe(false)
  })
})
