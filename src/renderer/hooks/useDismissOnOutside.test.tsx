// @vitest-environment jsdom
import { useRef } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { useDismissOnOutside } from './useDismissOnOutside'

function Harness({ open, close }: { open: boolean; close: () => void }): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  useDismissOnOutside(ref, open, close)
  return (
    <div>
      <div ref={ref} data-testid="inside" />
      <div data-testid="outside" />
    </div>
  )
}

afterEach(cleanup)

describe('useDismissOnOutside', () => {
  it('closes on a click outside, not inside', () => {
    const close = vi.fn()
    render(<Harness open close={close} />)
    fireEvent.mouseDown(screen.getByTestId('inside'))
    expect(close).not.toHaveBeenCalled()
    fireEvent.mouseDown(screen.getByTestId('outside'))
    expect(close).toHaveBeenCalledOnce()
  })

  it('closes on Escape', () => {
    const close = vi.fn()
    render(<Harness open close={close} />)
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(close).toHaveBeenCalledOnce()
  })

  it('does nothing while closed', () => {
    const close = vi.fn()
    render(<Harness open={false} close={close} />)
    fireEvent.mouseDown(screen.getByTestId('outside'))
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(close).not.toHaveBeenCalled()
  })
})

describe('useDismissOnOutside with several elements', () => {
  function Two({ close }: { close: () => void }): React.JSX.Element {
    const a = useRef<HTMLDivElement>(null)
    const b = useRef<HTMLDivElement>(null)
    useDismissOnOutside([a, b], true, close)
    return (
      <div>
        <div ref={a} data-testid="a" />
        <div ref={b} data-testid="b" />
        <div data-testid="c" />
      </div>
    )
  }

  it('treats each as inside', () => {
    const close = vi.fn()
    render(<Two close={close} />)
    fireEvent.mouseDown(screen.getByTestId('a'))
    fireEvent.mouseDown(screen.getByTestId('b'))
    expect(close).not.toHaveBeenCalled()
    fireEvent.mouseDown(screen.getByTestId('c'))
    expect(close).toHaveBeenCalledOnce()
  })
})
