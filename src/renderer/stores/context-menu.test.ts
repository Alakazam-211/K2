// @vitest-environment jsdom
// Chrome zoom (html { zoom }) vs position:fixed. Fail loud if show()
// stores raw clientX/Y or if gBCR size is mixed with CSS left.

import { afterEach, describe, expect, it } from 'vitest'
import { clientToCssPx, useContextMenuStore } from './context-menu'

afterEach(() => {
  delete window.__k2soZoom
  useContextMenuStore.getState().close()
})

describe('clientToCssPx', () => {
  it('divides by __k2soZoom: client 300,150 at 1.5 → CSS 200,100', () => {
    window.__k2soZoom = 1.5
    expect(clientToCssPx(300)).toBe(200)
    expect(clientToCssPx(150)).toBe(100)
  })

  it('zoom 1 is identity', () => {
    window.__k2soZoom = 1
    expect(clientToCssPx(300)).toBe(300)
    expect(clientToCssPx(150)).toBe(150)
  })

  it('unset zoom is identity', () => {
    expect(clientToCssPx(300)).toBe(300)
  })

  it('guards z > 0 (0 / negative do not divide)', () => {
    window.__k2soZoom = 0
    expect(clientToCssPx(300)).toBe(300)
    window.__k2soZoom = -1
    expect(clientToCssPx(300)).toBe(300)
  })
})

describe('useContextMenuStore.show chrome zoom', () => {
  it('stores client 300,150 as CSS 200,100 at zoom 1.5', () => {
    window.__k2soZoom = 1.5
    void useContextMenuStore.getState().show(300, 150, [{ id: 'a', label: 'A' }])
    const s = useContextMenuStore.getState()
    expect(s.x).toBe(200)
    expect(s.y).toBe(100)
    expect(s.isOpen).toBe(true)
  })

  it('zoom 1 is identity for stored left/top', () => {
    window.__k2soZoom = 1
    void useContextMenuStore.getState().show(300, 150, [{ id: 'a', label: 'A' }])
    const s = useContextMenuStore.getState()
    expect(s.x).toBe(300)
    expect(s.y).toBe(150)
  })
})

describe('viewport clamp must not mix CSS left with post-zoom gBCR', () => {
  it('converts gBCR width before comparing to innerWidth', () => {
    window.__k2soZoom = 1.5
    const cssLeft = 200
    const cssWidth = 180
    const gBCRWidth = cssWidth * 1.5
    const vw = 400
    expect(cssLeft + gBCRWidth > vw).toBe(true)
    expect(cssLeft + clientToCssPx(gBCRWidth) > vw).toBe(false)
    expect(clientToCssPx(gBCRWidth)).toBe(cssWidth)
  })
})
