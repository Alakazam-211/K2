// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { dockAppearance, parseCssColor, sameAppearance, snapRect } from './dockGeometry'

describe('snapRect', () => {
  it('rounds edges to device pixels at 2x (half points survive)', () => {
    expect(snapRect({ x: 240.3, y: 72.74, width: 600.4, height: 400.2 }, 2)).toEqual({
      x: 240.5,
      y: 72.5,
      width: 600,
      height: 400.5,
    })
  })

  it('keeps the right edge where x + width lands, not x and width rounded alone', () => {
    const r = snapRect({ x: 100.4, y: 0, width: 50.4, height: 10 }, 1)
    expect(r.x + r.width).toBe(151)
    expect(r.width).toBe(51)
  })

  it('two adjacent docks share their edge at 1.5x', () => {
    const a = snapRect({ x: 0, y: 0, width: 333.3, height: 10 }, 1.5)
    const b = snapRect({ x: 333.3, y: 0, width: 200, height: 10 }, 1.5)
    expect(a.x + a.width).toBe(b.x)
  })

  it('a bad devicePixelRatio falls back to 1', () => {
    expect(snapRect({ x: 1.4, y: 1.6, width: 2, height: 2 }, 0)).toEqual({ x: 1, y: 2, width: 2, height: 2 })
  })
})

describe('parseCssColor', () => {
  it('reads the computed forms WebKit hands back', () => {
    expect(parseCssColor('rgb(18, 18, 20)')).toEqual([18, 18, 20, 255])
    expect(parseCssColor('rgba(255, 255, 255, 0.5)')).toEqual([255, 255, 255, 128])
    expect(parseCssColor('rgb(10 20 30 / 50%)')).toEqual([10, 20, 30, 128])
    expect(parseCssColor('color(srgb 1 0.5 0)')).toEqual([255, 128, 0, 255])
    expect(parseCssColor('color(srgb 0 0 0 / 0.25)')).toEqual([0, 0, 0, 64])
  })

  it('anything else is null (leave the engine default)', () => {
    expect(parseCssColor('')).toBeNull()
    expect(parseCssColor('oklch(0.7 0.1 200)')).toBeNull()
    expect(parseCssColor('transparent')).toBeNull()
  })
})

describe('dockAppearance', () => {
  afterEach(() => {
    vi.restoreAllMocks()
    document.body.innerHTML = ''
  })

  function box(left: number, top: number, width: number, height: number): DOMRect {
    return {
      x: left,
      y: top,
      left,
      top,
      width,
      height,
      right: left + width,
      bottom: top + height,
      toJSON() {
        return {}
      },
    } as DOMRect
  }

  it('rounds the dock corners that sit on a rounded pane corner, minus its border', () => {
    document.body.innerHTML =
      '<div id="pane" style="border-top-left-radius: 12px; border-top-right-radius: 12px; border-bottom-right-radius: 12px; border-bottom-left-radius: 12px; border-left-width: 1px; border-right-width: 1px; border-top-width: 1px; border-bottom-width: 1px; border-style: solid"><div id="bar"></div><div id="dock" style="background-color: rgb(20, 21, 22)"></div></div>'
    const pane = document.getElementById('pane') as HTMLElement
    const dock = document.getElementById('dock') as HTMLElement
    vi.spyOn(pane, 'getBoundingClientRect').mockReturnValue(box(100, 50, 602, 402))
    // Below a 36px chrome bar: only the bottom corners touch the pane's.
    vi.spyOn(dock, 'getBoundingClientRect').mockReturnValue(box(101, 87, 600, 364))
    const a = dockAppearance(dock)
    expect(a.background).toEqual([20, 21, 22, 255])
    expect(a.cornerRadius).toBe(11)
    expect(a.corners).toEqual([false, false, true, true])
  })

  it('square when the nearest rounded ancestor does not share a corner, or none is rounded', () => {
    document.body.innerHTML = '<div id="pane" style="border-top-left-radius: 12px; border-top-right-radius: 12px; border-bottom-right-radius: 12px; border-bottom-left-radius: 12px"><div id="dock"></div></div>'
    const pane = document.getElementById('pane') as HTMLElement
    const dock = document.getElementById('dock') as HTMLElement
    vi.spyOn(pane, 'getBoundingClientRect').mockReturnValue(box(0, 0, 800, 600))
    vi.spyOn(dock, 'getBoundingClientRect').mockReturnValue(box(200, 100, 300, 200))
    expect(dockAppearance(dock)).toEqual({ background: null, cornerRadius: 0, corners: [false, false, false, false] })

    document.body.innerHTML = '<div><div id="dock2"></div></div>'
    expect(dockAppearance(document.getElementById('dock2') as HTMLElement).cornerRadius).toBe(0)
  })

  it('sameAppearance compares by value; nothing sent yet is never the same', () => {
    const a = { background: null, cornerRadius: 0, corners: [false, false, false, false] as [boolean, boolean, boolean, boolean] }
    expect(sameAppearance(null, a)).toBe(false)
    expect(sameAppearance({ ...a }, a)).toBe(true)
    expect(sameAppearance({ ...a, cornerRadius: 4 }, a)).toBe(false)
  })
})
