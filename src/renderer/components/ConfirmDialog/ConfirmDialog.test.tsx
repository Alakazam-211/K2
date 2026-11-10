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
  useConfirmDialogStore.setState({ isOpen: false, onResolve: null })
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
