// @vitest-environment jsdom
// Fail loud: the arbiter decides when a native Browser page gives way.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'
import { useRef, useState } from 'react'
import {
  BrowserOverlayClaim,
  beginBrowserMotion,
  claimBrowserOverlay,
  claimCovers,
  coveringClaims,
  overlayClaims,
  rectsOverlap,
  resetBrowserOverlaysForTest,
  useBrowserCovered,
  useBrowserOverlay,
  type OverlayRect,
} from './browser-overlay'

const PANE: OverlayRect = { x: 400, y: 40, width: 600, height: 500 }

function rectOf(r: OverlayRect): DOMRect {
  return {
    ...r,
    top: r.y,
    left: r.x,
    right: r.x + r.width,
    bottom: r.y + r.height,
    toJSON() {
      return {}
    },
  } as DOMRect
}

/** Give elements tagged data-rect="x,y,w,h" that box. */
function stubRects(): void {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const spec = this.getAttribute('data-rect')
    if (!spec) return rectOf({ x: 0, y: 0, width: 0, height: 0 })
    const [x, y, width, height] = spec.split(',').map(Number)
    return rectOf({ x, y, width, height })
  })
}

class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

beforeEach(() => {
  resetBrowserOverlaysForTest()
  globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver
  stubRects()
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  resetBrowserOverlaysForTest()
})

describe('rectsOverlap', () => {
  it('needs a positive-area overlap; touching edges do not cover', () => {
    expect(rectsOverlap(PANE, { x: 990, y: 100, width: 50, height: 50 })).toBe(true)
    expect(rectsOverlap(PANE, { x: 1000, y: 100, width: 50, height: 50 })).toBe(false)
    expect(rectsOverlap(PANE, { x: 100, y: 100, width: 300, height: 50 })).toBe(false)
    expect(rectsOverlap(PANE, { x: 100, y: 0, width: 2000, height: 40 })).toBe(false)
    expect(rectsOverlap(PANE, { x: 500, y: 539, width: 10, height: 10 })).toBe(true)
  })

  it('an empty rect covers nothing', () => {
    expect(rectsOverlap(PANE, { x: 500, y: 100, width: 0, height: 50 })).toBe(false)
    expect(rectsOverlap({ x: 0, y: 0, width: 0, height: 0 }, PANE)).toBe(false)
  })
})

describe('claims', () => {
  it('a menu on the far side leaves the page live; one over it covers', () => {
    const far = claimBrowserOverlay({ kind: 'menu', area: { x: 10, y: 40, width: 200, height: 300 } })
    expect(coveringClaims(overlayClaims(), PANE, null)).toEqual([])
    far.update({ x: 380, y: 40, width: 200, height: 300 })
    expect(coveringClaims(overlayClaims(), PANE, null).map((c) => c.id)).toEqual([far.id])
    far.release()
    expect(overlayClaims()).toEqual([])
  })

  it('a fullscreen claim covers every pane; release is idempotent', () => {
    const end = beginBrowserMotion('drag')
    expect(coveringClaims(overlayClaims(), PANE, null)).toHaveLength(1)
    expect(overlayClaims()[0].kind).toBe('drag')
    end()
    end()
    expect(overlayClaims()).toEqual([])
  })

  it('nested overlays: closing the inner menu leaves the dialog covering', () => {
    const dialog = claimBrowserOverlay({ kind: 'modal', area: 'fullscreen' })
    const menu = claimBrowserOverlay({ kind: 'menu', area: { x: 500, y: 100, width: 100, height: 100 } })
    expect(coveringClaims(overlayClaims(), PANE, null)).toHaveLength(2)
    menu.release()
    expect(coveringClaims(overlayClaims(), PANE, null).map((c) => c.kind)).toEqual(['modal'])
    dialog.release()
    expect(coveringClaims(overlayClaims(), PANE, null)).toEqual([])
  })

  it('a Browser pane inside the overlay root is not covered by that overlay', () => {
    const root = document.createElement('div')
    const dock = document.createElement('div')
    root.appendChild(dock)
    const own = claimBrowserOverlay({ kind: 'modal', area: 'fullscreen', element: root })
    const ownClaim = overlayClaims().find((c) => c.id === own.id)
    expect(ownClaim).toBeDefined()
    expect(claimCovers(ownClaim as NonNullable<typeof ownClaim>, PANE, dock)).toBe(false)
    expect(claimCovers(ownClaim as NonNullable<typeof ownClaim>, PANE, document.createElement('div'))).toBe(true)
    const other = claimBrowserOverlay({ kind: 'menu', area: 'fullscreen' })
    expect(coveringClaims(overlayClaims(), PANE, dock).map((c) => c.id)).toEqual([other.id])
  })

  it('an unmeasured pane (display:none) is never covered', () => {
    claimBrowserOverlay({ kind: 'modal', area: 'fullscreen' })
    expect(coveringClaims(overlayClaims(), null, null)).toEqual([])
    expect(coveringClaims(overlayClaims(), { x: 0, y: 0, width: 0, height: 0 }, null)).toEqual([])
  })

  it('an unchanged area does not notify', () => {
    const h = claimBrowserOverlay({ kind: 'menu', area: { x: 1, y: 1, width: 10, height: 10 } })
    const before = overlayClaims()
    h.update({ x: 1.2, y: 1, width: 10, height: 10.3 })
    expect(overlayClaims()).toBe(before)
    h.update({ x: 5, y: 1, width: 10, height: 10 })
    expect(overlayClaims()).not.toBe(before)
  })
})

function Menu({ open, rect }: { open: boolean; rect: string }): React.JSX.Element | null {
  const ref = useRef<HTMLDivElement>(null)
  useBrowserOverlay({ active: open, ref, kind: 'menu' })
  if (!open) return null
  return <div ref={ref} data-rect={rect} />
}

function Dock({ onCovered }: { onCovered: (v: boolean) => void }): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  onCovered(useBrowserCovered(ref))
  return <div ref={ref} data-rect="400,40,600,500" />
}

describe('useBrowserOverlay / useBrowserCovered', () => {
  it('claims while open, follows the box, and unregisters on close and on unmount', () => {
    const seen: boolean[] = []
    const record = (v: boolean): void => {
      seen.push(v)
    }
    const view = render(
      <>
        <Dock onCovered={record} />
        <Menu open={false} rect="450,60,100,100" />
      </>,
    )
    expect(overlayClaims()).toHaveLength(0)
    expect(seen.at(-1)).toBe(false)

    view.rerender(
      <>
        <Dock onCovered={record} />
        <Menu open rect="450,60,100,100" />
      </>,
    )
    expect(overlayClaims()).toHaveLength(1)
    expect(seen.at(-1)).toBe(true)

    // Closed: released.
    view.rerender(
      <>
        <Dock onCovered={record} />
        <Menu open={false} rect="450,60,100,100" />
      </>,
    )
    expect(overlayClaims()).toHaveLength(0)
    expect(seen.at(-1)).toBe(false)

    // A menu over the sidebar does not cover the dock.
    view.rerender(
      <>
        <Dock onCovered={record} />
        <Menu open rect="10,60,100,100" />
      </>,
    )
    expect(overlayClaims()).toHaveLength(1)
    expect(seen.at(-1)).toBe(false)

    // Unmounting the open menu releases its claim.
    view.rerender(<Dock onCovered={record} />)
    expect(overlayClaims()).toHaveLength(0)
  })

  it('<BrowserOverlayClaim /> claims the window while mounted', () => {
    let covered = false
    function Toggle(): React.JSX.Element {
      const [on, setOn] = useState(true)
      ;(globalThis as { __toggle?: () => void }).__toggle = () => setOn(false)
      return <>{on && <BrowserOverlayClaim />}</>
    }
    render(
      <>
        <Dock onCovered={(v) => (covered = v)} />
        <Toggle />
      </>,
    )
    expect(overlayClaims().map((c) => c.area)).toEqual(['fullscreen'])
    expect(covered).toBe(true)
    act(() => (globalThis as { __toggle?: () => void }).__toggle?.())
    expect(overlayClaims()).toHaveLength(0)
    expect(covered).toBe(false)
  })

  it('a ref that is not attached claims the whole window (over-hide is the safe bias)', () => {
    function Detached(): null {
      const ref = useRef<HTMLDivElement>(null)
      useBrowserOverlay({ active: true, ref, kind: 'popover' })
      return null
    }
    render(<Detached />)
    expect(overlayClaims().map((c) => c.area)).toEqual(['fullscreen'])
  })
})
