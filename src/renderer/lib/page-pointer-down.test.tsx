// @vitest-environment jsdom
// A click on a native browser page (macOS underlay) closes K2's open menus
// and popovers exactly as an outside click does. Fail loud: real menus.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'
import { useEffect, useState } from 'react'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { outsidePointerDown } from './page-pointer-down'

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }))

afterEach(() => cleanup())

function AnchoredMenu({ onClose }: { onClose: () => void }): React.JSX.Element {
  const menu = useAnchoredMenu<HTMLButtonElement>({ open: true, onClose })
  return (
    <>
      <button ref={menu.anchorRef}>open</button>
      {menu.portal(
        <div ref={menu.menuRef} style={menu.style} data-testid="menu">
          item
        </div>,
      )}
    </>
  )
}

/** A popover that closes on an outside pointerdown, the other common way. */
function Popover({ onClose }: { onClose: () => void }): React.JSX.Element {
  const [el, setEl] = useState<HTMLDivElement | null>(null)
  useEffect(() => {
    const onDown = (e: PointerEvent): void => {
      if (el && !el.contains(e.target as Node)) onClose()
    }
    document.addEventListener('pointerdown', onDown)
    return () => document.removeEventListener('pointerdown', onDown)
  }, [el, onClose])
  return <div ref={setEl}>picker</div>
}

describe('page pointer-down', () => {
  it('closes an anchored menu (mousedown outside)', () => {
    const onClose = vi.fn()
    render(<AnchoredMenu onClose={onClose} />)
    expect(onClose).not.toHaveBeenCalled()
    act(() => outsidePointerDown())
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it('closes a popover that listens for pointerdown', () => {
    const onClose = vi.fn()
    render(<Popover onClose={onClose} />)
    act(() => outsidePointerDown())
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it('is an ordinary outside click: a click inside a menu would not close it', () => {
    const onClose = vi.fn()
    const view = render(<AnchoredMenu onClose={onClose} />)
    act(() => outsidePointerDown(view.getByTestId('menu')))
    expect(onClose).not.toHaveBeenCalled()
  })
})
