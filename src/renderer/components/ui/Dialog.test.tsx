// @vitest-environment jsdom
// The shared dialog frame cancels cleanly, once: the × (labelled
// "Close (Esc)"), Esc and a press outside the frame (its scrim or anywhere
// else) each call `onCancel` exactly once; a press inside does not. With two
// frames open only the top one cancels. Without `onCancel` the frame stays
// presentational (no ×, no key or press handling).

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { createPortal } from 'react-dom'
import { DialogFrame, DialogScrim, useDialogCancel, type DialogCancelOptions } from '@/components/ui'

vi.mock('@/lib/browser-overlay', () => ({ useBrowserOverlay: () => {} }))

afterEach(cleanup)

function One({ onCancel, id = 'a' }: { onCancel?: () => void; id?: string }): React.JSX.Element {
  return (
    <>
      <DialogScrim data-testid={`scrim-${id}`} />
      <DialogFrame data-testid={`frame-${id}`} onCancel={onCancel}>
        <p data-testid={`body-${id}`}>Body</p>
        <input data-testid={`input-${id}`} />
        {/* A menu the dialog portals to <body> (an option list, a picker). */}
        {createPortal(<div role="listbox" data-testid={`menu-${id}`}>Option</div>, document.body)}
      </DialogFrame>
    </>
  )
}

describe('DialogFrame onCancel', () => {
  it('the × is labelled Close (Esc) and cancels once', () => {
    const onCancel = vi.fn()
    render(<One onCancel={onCancel} />)
    const close = screen.getByRole('button', { name: 'Close (Esc)' })
    expect(close.getAttribute('title')).toBe('Close (Esc)')
    fireEvent.pointerDown(close)
    fireEvent.click(close)
    expect(onCancel).toHaveBeenCalledTimes(1)
  })

  it('Esc cancels once, wherever focus is', () => {
    const onCancel = vi.fn()
    render(<One onCancel={onCancel} />)
    fireEvent.keyDown(document.body, { key: 'Escape' })
    expect(onCancel).toHaveBeenCalledTimes(1)
    const input = screen.getByTestId('input-a')
    input.focus()
    fireEvent.keyDown(input, { key: 'Escape' })
    expect(onCancel).toHaveBeenCalledTimes(2)
  })

  it('a press on the scrim cancels once', () => {
    const onCancel = vi.fn()
    render(<One onCancel={onCancel} />)
    fireEvent.pointerDown(screen.getByTestId('scrim-a'))
    expect(onCancel).toHaveBeenCalledTimes(1)
  })

  it('a press anywhere outside the frame cancels', () => {
    const onCancel = vi.fn()
    render(
      <>
        <button type="button" data-testid="page">Page</button>
        <DialogFrame onCancel={onCancel}>Body</DialogFrame>
      </>,
    )
    fireEvent.pointerDown(screen.getByTestId('page'))
    expect(onCancel).toHaveBeenCalledTimes(1)
  })

  it('a press inside the frame, or in a menu it portals, does not cancel', () => {
    const onCancel = vi.fn()
    render(<One onCancel={onCancel} />)
    fireEvent.pointerDown(screen.getByTestId('body-a'))
    fireEvent.pointerDown(screen.getByTestId('input-a'))
    fireEvent.pointerDown(screen.getByTestId('menu-a'))
    fireEvent.click(screen.getByTestId('body-a'))
    expect(onCancel).not.toHaveBeenCalled()
  })

  it('other keys do nothing', () => {
    const onCancel = vi.fn()
    render(<One onCancel={onCancel} />)
    fireEvent.keyDown(document.body, { key: 'Enter' })
    fireEvent.keyDown(document.body, { key: 'a' })
    expect(onCancel).not.toHaveBeenCalled()
  })

  it('with two frames open only the top one cancels', () => {
    const under = vi.fn()
    const top = vi.fn()
    const view = render(<One id="a" onCancel={under} />)
    view.rerender(
      <>
        <One id="a" onCancel={under} />
        <One id="b" onCancel={top} />
      </>,
    )
    fireEvent.keyDown(document.body, { key: 'Escape' })
    expect(top).toHaveBeenCalledTimes(1)
    expect(under).not.toHaveBeenCalled()
    // A press on the lower dialog is outside the top one: only the top cancels.
    fireEvent.pointerDown(screen.getByTestId('body-a'))
    expect(top).toHaveBeenCalledTimes(2)
    expect(under).not.toHaveBeenCalled()
    // The top one closes: the lower one is on top again.
    view.rerender(<One id="a" onCancel={under} />)
    fireEvent.keyDown(document.body, { key: 'Escape' })
    expect(under).toHaveBeenCalledTimes(1)
    expect(top).toHaveBeenCalledTimes(2)
  })

  it('held (a change in flight): Esc and presses do nothing, and stay off the dialog under it', () => {
    const under = vi.fn()
    const top = vi.fn()
    const view = render(<One id="a" onCancel={under} />)
    const both = (held: boolean): React.JSX.Element => (
      <>
        <One id="a" onCancel={under} />
        <DialogScrim data-testid="scrim-b" />
        <DialogFrame onCancel={top} cancelHeld={held}>Saving…</DialogFrame>
      </>
    )
    view.rerender(both(true))
    expect((screen.getAllByRole('button', { name: 'Close (Esc)' }).at(-1) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.keyDown(document.body, { key: 'Escape' })
    fireEvent.pointerDown(screen.getByTestId('scrim-b'))
    expect(top).not.toHaveBeenCalled()
    expect(under).not.toHaveBeenCalled()
    // Released: it is still the top dialog.
    view.rerender(both(false))
    fireEvent.keyDown(document.body, { key: 'Escape' })
    expect(top).toHaveBeenCalledTimes(1)
    expect(under).not.toHaveBeenCalled()
  })

  it('Esc leaves an open drop-down or menu that has focus alone', () => {
    const onCancel = vi.fn()
    render(
      <DialogFrame onCancel={onCancel}>
        <button type="button" aria-haspopup="menu" aria-expanded="true" data-testid="trigger">
          Theme
        </button>
        <div role="menu">
          <button type="button" role="menuitem" data-testid="item">
            Dark
          </button>
        </div>
      </DialogFrame>,
    )
    fireEvent.keyDown(screen.getByTestId('trigger'), { key: 'Escape' })
    fireEvent.keyDown(screen.getByTestId('item'), { key: 'Escape' })
    expect(onCancel).not.toHaveBeenCalled()
    fireEvent.keyDown(document.body, { key: 'Escape' })
    expect(onCancel).toHaveBeenCalledTimes(1)
  })

  it('without onCancel the frame has no × and ignores Esc and presses', () => {
    render(
      <>
        <DialogScrim data-testid="scrim" />
        <DialogFrame data-testid="frame">Body</DialogFrame>
      </>,
    )
    expect(screen.queryByRole('button', { name: 'Close (Esc)' })).toBeNull()
    const esc = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
    document.body.dispatchEvent(esc)
    expect(esc.defaultPrevented).toBe(false)
  })
})

function Custom({ onCancel, options }: { onCancel: () => void; options: DialogCancelOptions }): React.JSX.Element {
  const cancel = useDialogCancel(onCancel, false, options)
  return (
    <>
      <button type="button" data-testid="outside">Page</button>
      <div {...cancel} data-testid="box">
        Box
      </div>
    </>
  )
}

describe('useDialogCancel options', () => {
  it('escape: a first step (clear a search) runs instead of cancelling while it says so', () => {
    const onCancel = vi.fn()
    let query = 'font'
    const escape = vi.fn(() => {
      if (!query) return false
      query = ''
      return true
    })
    render(<Custom onCancel={onCancel} options={{ escape }} />)
    fireEvent.keyDown(document.body, { key: 'Escape' })
    expect(query).toBe('')
    expect(onCancel).not.toHaveBeenCalled()
    fireEvent.keyDown(document.body, { key: 'Escape' })
    expect(onCancel).toHaveBeenCalledTimes(1)
    // The × and a press outside skip the first step.
    fireEvent.pointerDown(screen.getByTestId('outside'))
    expect(onCancel).toHaveBeenCalledTimes(2)
  })

  it('pressOutside false: a press outside does nothing; Esc still cancels', () => {
    const onCancel = vi.fn()
    render(<Custom onCancel={onCancel} options={{ pressOutside: false }} />)
    fireEvent.pointerDown(screen.getByTestId('outside'))
    expect(onCancel).not.toHaveBeenCalled()
    fireEvent.keyDown(document.body, { key: 'Escape' })
    expect(onCancel).toHaveBeenCalledTimes(1)
  })
})
