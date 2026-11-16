// @vitest-environment jsdom

// The live views' side panel slides like the app's drawers (PanelMotion).
// Pinned here:
//   - beside the hero it is the drawers' own AnimatedDrawer: opening and
//     closing animate its width with the drawer motion (DRAWER_TRANSITION),
//     and closing keeps it until the slide ends;
//   - the narrow sheet slides in and out on the same duration;
//   - reduced motion: no transition, the panel appears and goes at once;
//   - closing with focus inside the panel puts focus on the panel toggle.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { useState } from 'react'
import { DRAWER_ANIM_MS, DRAWER_TRANSITION } from '@/components/Layout/drawer-motion'
import { LiveTabFrame } from './LiveTabFrame'

function motion(reduce: boolean): void {
  vi.stubGlobal('matchMedia', (q: string) => ({ matches: reduce && q.includes('reduce'), media: q, addEventListener: () => undefined, removeEventListener: () => undefined, addListener: () => undefined, removeListener: () => undefined }))
}

function Frame({ narrow = false, mode = 'reflow' as 'sheet' | 'reflow' }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  return (
    <LiveTabFrame
      header={
        <button type="button" data-testid="lv-panel-toggle" onClick={() => setOpen((o) => !o)}>
          Panel
        </button>
      }
      hero={<div>hero</div>}
      panel={
        <button type="button" data-testid="inside">
          Inside
        </button>
      }
      panelOpen={open}
      panelMode={mode}
      narrow={narrow}
      sample={false}
      onClosePanel={() => setOpen(false)}
    />
  )
}

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('the side panel slides like the drawers', () => {
  it('opens and closes with the drawer motion, kept while it slides out', () => {
    motion(false)
    vi.useFakeTimers()
    render(<Frame />)
    expect(screen.queryByTestId('lv-panel')).toBeNull()
    fireEvent.click(screen.getByTestId('lv-panel-toggle'))
    const drawer = screen.getByTestId('lv-panel').closest('[data-drawer-side]') as HTMLElement
    expect(drawer.dataset.drawerSide).toBe('right')
    expect(drawer.style.transition).toBe(DRAWER_TRANSITION)
    expect(drawer.dataset.drawerAnim).toBe('opening')
    act(() => vi.advanceTimersByTime(DRAWER_ANIM_MS + 100))
    expect(drawer.style.transition).toBe('')
    fireEvent.click(screen.getByTestId('lv-panel-toggle'))
    expect((screen.getByTestId('lv-panel').closest('[data-drawer-side]') as HTMLElement).dataset.drawerAnim).toBe('closing')
    act(() => vi.advanceTimersByTime(DRAWER_ANIM_MS + 100))
    expect(screen.queryByTestId('lv-panel')).toBeNull()
  })

  it('the narrow sheet slides in and out on the same duration', () => {
    motion(false)
    vi.useFakeTimers()
    render(<Frame narrow mode="sheet" />)
    fireEvent.click(screen.getByTestId('lv-panel-toggle'))
    const panel = screen.getByTestId('lv-panel')
    expect(panel.hasAttribute('data-sheet')).toBe(true)
    expect((panel.parentElement as HTMLElement).dataset.lvSheetAnim).toBe('in')
    fireEvent.click(screen.getByTestId('lv-panel-toggle'))
    expect((screen.getByTestId('lv-panel').parentElement as HTMLElement).dataset.lvSheetAnim).toBe('out')
    act(() => vi.advanceTimersByTime(DRAWER_ANIM_MS))
    expect(screen.queryByTestId('lv-panel')).toBeNull()
  })

  it('reduced motion: no transition; it appears and goes at once', () => {
    motion(true)
    render(<Frame />)
    fireEvent.click(screen.getByTestId('lv-panel-toggle'))
    const drawer = screen.getByTestId('lv-panel').closest('[data-drawer-side]') as HTMLElement
    expect(drawer.style.transition).toBe('')
    expect(drawer.dataset.drawerAnim).toBeUndefined()
    fireEvent.click(screen.getByTestId('lv-panel-toggle'))
    expect(screen.queryByTestId('lv-panel')).toBeNull()
  })

  it('closing with focus inside puts focus on the panel toggle', () => {
    motion(true)
    render(
      <div className="lv-shell">
        <Frame narrow mode="reflow" />
      </div>,
    )
    fireEvent.click(screen.getByTestId('lv-panel-toggle'))
    const close = screen.getByRole('button', { name: 'Close the side panel' })
    close.focus()
    fireEvent.click(close)
    expect(document.activeElement).toBe(screen.getByTestId('lv-panel-toggle'))
  })
})
