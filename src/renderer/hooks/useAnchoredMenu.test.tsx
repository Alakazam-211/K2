// @vitest-environment jsdom
//
// Rosson 2026-10-04: "In home, when I select the drop-down, the options are
// cut-off because the agent section borders cuts it off." Dropdown menus are
// portalled and fixed to their trigger (`useAnchoredMenu`), so no clipping
// ancestor can cut them off: the focus-group dropdown (Agents sidebar, Zen's
// Agents view) and the Home picker (Home sidebar).

import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import FocusGroupDropdown from '@/components/Sidebar/FocusGroupDropdown'
import HomePicker from '@/components/Home/HomePicker'
import { useHomesStore } from '@/stores/homes'
import { ANCHORED_MENU_MARGIN, ZEN_ANCHORED_MENU_LAYER, anchoredMenuBox } from './useAnchoredMenu'

const OPTIONS = [
  { id: 'g-work', name: 'Work', color: '#f00' },
  { id: 'g-play', name: 'Play', color: null },
  { id: 'g-ops', name: 'Ops', color: null },
]

afterEach(() => {
  cleanup()
})

/** Give `el` a fixed rect (jsdom has no layout). */
function setRect(el: Element, r: { top: number; left: number; width: number; height: number }): void {
  const rect = { ...r, right: r.left + r.width, bottom: r.top + r.height, x: r.left, y: r.top }
  ;(el as HTMLElement).getBoundingClientRect = () => ({ ...rect, toJSON: () => rect }) as DOMRect
}

function menuEl(selector = '[data-focus-group-menu]'): HTMLElement {
  const el = document.querySelector(selector)
  if (!(el instanceof HTMLElement)) throw new Error(`no open menu ${selector}`)
  return el
}

/** A card that clips like Zen's column card (`overflow: hidden`), with the
 *  dropdown inside, on its own layer. */
function renderClipped(onChange = vi.fn(), wrap?: (n: React.ReactNode) => React.ReactNode): {
  card: HTMLElement
  trigger: HTMLElement
  onChange: ReturnType<typeof vi.fn>
} {
  const tree = (
    <div data-card="" style={{ overflow: 'hidden', height: 40, position: 'relative', zIndex: 7 }}>
      <FocusGroupDropdown options={OPTIONS} value="g-work" onChange={onChange} />
    </div>
  )
  render(<>{wrap ? wrap(tree) : tree}</>)
  const card = document.querySelector('[data-card]') as HTMLElement
  const trigger = card.querySelector('[data-focus-group-trigger]') as HTMLElement
  if (!trigger) throw new Error('no focus-group trigger')
  setRect(trigger.parentElement as HTMLElement, { top: 100, left: 20, width: 190, height: 28 })
  return { card, trigger, onChange }
}

describe('anchoredMenuBox', () => {
  const vp = { width: 1000, height: 800 }
  const opts = { gap: 2, width: 'match' as const, minWidth: 0, menuWidth: 0 }

  it('opens below the trigger at the trigger’s width', () => {
    expect(anchoredMenuBox({ top: 100, bottom: 128, left: 20, width: 190 }, vp, 200, opts)).toEqual({
      placement: 'down',
      top: 130,
      left: 20,
      width: 190,
      minWidth: 190,
    })
  })

  it('flips upward when there isn’t room below and there is more above', () => {
    expect(anchoredMenuBox({ top: 700, bottom: 728, left: 20, width: 190 }, vp, 200, opts)).toEqual({
      placement: 'up',
      bottom: 102,
      left: 20,
      width: 190,
      minWidth: 190,
    })
  })

  it('stays below when below still has more room than above', () => {
    expect(anchoredMenuBox({ top: 300, bottom: 328, left: 20, width: 190 }, vp, 600, opts).placement).toBe('down')
  })

  it('keeps the menu inside the window horizontally; `min` is at least the trigger and the floor', () => {
    const box = anchoredMenuBox({ top: 10, bottom: 30, left: 950, width: 60 }, vp, 100, {
      gap: 4,
      width: 'min',
      minWidth: 180,
      menuWidth: 120,
    })
    expect(box).toEqual({ placement: 'down', top: 34, left: 1000 - ANCHORED_MENU_MARGIN - 180, minWidth: 180 })
  })
})

describe('FocusGroupDropdown in a clipping card', () => {
  it('renders the menu outside the card (portal, fixed), at the trigger’s width, on a layer above the card', () => {
    const { card, trigger } = renderClipped()
    act(() => {
      fireEvent.click(trigger)
    })
    const menu = menuEl()
    expect(card.contains(menu)).toBe(false)
    expect(menu.parentElement).toBe(document.body)
    expect(menu.style.position).toBe('fixed')
    expect(menu.style.top).toBe('130px')
    expect(menu.style.left).toBe('20px')
    expect(menu.style.width).toBe('190px')
    expect(Number(menu.style.zIndex)).toBeGreaterThanOrEqual(400)
    expect(menu.getAttribute('data-placement')).toBe('down')
    expect(trigger.getAttribute('aria-expanded')).toBe('true')
    // The search box takes the keys.
    expect(document.activeElement).toBe(menu.querySelector('input'))
  })

  it('a pick works and closes the menu', () => {
    const { trigger, onChange } = renderClipped()
    act(() => {
      fireEvent.click(trigger)
    })
    const play = Array.from(menuEl().querySelectorAll('button')).find((b) => b.textContent?.includes('Play'))
    if (!play) throw new Error('no Play option')
    // A press inside the menu is not an outside click.
    act(() => {
      fireEvent.mouseDown(play)
      fireEvent.click(play)
    })
    expect(onChange.mock.calls).toEqual([['g-play']])
    expect(document.querySelector('[data-focus-group-menu]')).toBeNull()
  })

  it('keyboard: ArrowDown + Enter picks; Escape closes', () => {
    const { trigger, onChange } = renderClipped()
    act(() => {
      fireEvent.click(trigger)
    })
    const input = menuEl().querySelector('input') as HTMLInputElement
    act(() => {
      fireEvent.keyDown(input, { key: 'ArrowDown' })
    })
    act(() => {
      fireEvent.keyDown(input, { key: 'Enter' })
    })
    expect(onChange.mock.calls).toEqual([['g-play']])
    expect(document.querySelector('[data-focus-group-menu]')).toBeNull()

    act(() => {
      fireEvent.click(trigger)
    })
    expect(document.querySelector('[data-focus-group-menu]')).not.toBeNull()
    act(() => {
      fireEvent.keyDown(menuEl().querySelector('input') as HTMLInputElement, { key: 'Escape' })
    })
    expect(document.querySelector('[data-focus-group-menu]')).toBeNull()
  })

  it('an outside click closes it; the trigger toggles it', () => {
    const { trigger, onChange } = renderClipped()
    act(() => {
      fireEvent.click(trigger)
    })
    act(() => {
      fireEvent.mouseDown(document.body)
    })
    expect(document.querySelector('[data-focus-group-menu]')).toBeNull()
    act(() => {
      fireEvent.click(trigger)
    })
    expect(document.querySelector('[data-focus-group-menu]')).not.toBeNull()
    act(() => {
      fireEvent.mouseDown(trigger)
      fireEvent.click(trigger)
    })
    expect(document.querySelector('[data-focus-group-menu]')).toBeNull()
    expect(onChange).not.toHaveBeenCalled()
  })

  it('flips upward near the bottom of the window', () => {
    const { trigger } = renderClipped()
    setRect(trigger.parentElement as HTMLElement, { top: window.innerHeight - 40, left: 20, width: 190, height: 28 })
    act(() => {
      fireEvent.click(trigger)
    })
    const menu = menuEl()
    expect(menu.getAttribute('data-placement')).toBe('up')
    expect(menu.style.top).toBe('')
    expect(menu.style.bottom).toBe('42px')
  })

  it('follows the trigger on scroll and resize', () => {
    const { trigger } = renderClipped()
    act(() => {
      fireEvent.click(trigger)
    })
    expect(menuEl().style.top).toBe('130px')
    setRect(trigger.parentElement as HTMLElement, { top: 60, left: 30, width: 190, height: 28 })
    act(() => {
      fireEvent.scroll(window)
    })
    expect(menuEl().style.top).toBe('90px')
    expect(menuEl().style.left).toBe('30px')
    setRect(trigger.parentElement as HTMLElement, { top: 80, left: 30, width: 220, height: 28 })
    act(() => {
      window.dispatchEvent(new Event('resize'))
    })
    expect(menuEl().style.top).toBe('110px')
    expect(menuEl().style.width).toBe('220px')
  })

  it('inside Zen: portalled into the Zen root (its theme tokens), above Zen’s layers', () => {
    const { card, trigger } = renderClipped(vi.fn(), (n) => (
      <div data-zen-root="" style={{ ['--zen-surface' as string]: '#123456', position: 'fixed', zIndex: 150 }}>
        <div style={{ position: 'relative', zIndex: 1 }}>{n}</div>
      </div>
    ))
    act(() => {
      fireEvent.click(trigger)
    })
    const menu = menuEl()
    const root = document.querySelector('[data-zen-root]')
    expect(card.contains(menu)).toBe(false)
    expect(menu.parentElement).toBe(root)
    expect(Number(menu.style.zIndex)).toBe(ZEN_ANCHORED_MENU_LAYER)
    // Above the window-control cluster (3) and the Add agent picker (30).
    expect(ZEN_ANCHORED_MENU_LAYER).toBeGreaterThan(30)
  })
})

describe('Home picker (Home sidebar)', () => {
  it('portals out of the sidebar’s scroller; a pick selects the Home; outside click closes', () => {
    const first = useHomesStore.getState().homes[0]
    const second = useHomesStore.getState().createHome('Anchored test')
    if (!second) throw new Error('could not create a Home')
    act(() => useHomesStore.getState().selectHome(first.id))
    try {
      render(
        <div data-sidebar="" style={{ overflowY: 'auto', height: 120 }}>
          <HomePicker />
        </div>,
      )
      const sidebar = document.querySelector('[data-sidebar]') as HTMLElement
      const button = sidebar.querySelector('button[title="Pick a Home"]') as HTMLElement
      setRect(button.parentElement as HTMLElement, { top: 50, left: 12, width: 230, height: 30 })
      act(() => {
        fireEvent.click(button)
      })
      const menu = menuEl('[data-home-picker-menu]')
      expect(sidebar.contains(menu)).toBe(false)
      expect(menu.style.position).toBe('fixed')
      expect(menu.style.top).toBe('82px')
      expect(menu.style.width).toBe('230px')
      // Every row is clickable and looks it.
      for (const b of Array.from(menu.querySelectorAll('button:not(:disabled)'))) {
        expect(b.classList.contains('cursor-pointer'), b.textContent ?? '').toBe(true)
      }
      const pick = Array.from(menu.querySelectorAll('[role="menuitemradio"]')).find((b) =>
        b.textContent?.includes('Anchored test'),
      )
      if (!(pick instanceof HTMLElement)) throw new Error('no Anchored test Home in the menu')
      act(() => {
        fireEvent.mouseDown(pick)
        fireEvent.click(pick)
      })
      expect(useHomesStore.getState().selectedId).toBe(second)
      expect(document.querySelector('[data-home-picker-menu]')).toBeNull()

      // Rename mode lives in the same portalled menu; typing there doesn't close it.
      act(() => {
        fireEvent.click(button)
      })
      const rename = Array.from(menuEl('[data-home-picker-menu]').querySelectorAll('button')).find((b) =>
        b.textContent?.startsWith('Rename'),
      )
      if (!rename) throw new Error('no Rename')
      act(() => {
        fireEvent.mouseDown(rename)
        fireEvent.click(rename)
      })
      const input = menuEl('[data-home-picker-menu]').querySelector('input') as HTMLInputElement
      act(() => {
        fireEvent.mouseDown(input)
      })
      expect(document.querySelector('[data-home-picker-menu]')).not.toBeNull()
      act(() => {
        fireEvent.mouseDown(document.body)
      })
      expect(document.querySelector('[data-home-picker-menu]')).toBeNull()
    } finally {
      act(() => useHomesStore.getState().deleteHome(second))
    }
  })
})
