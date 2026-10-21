// 0.46.3 Gardens "Light blur" (Garden settings → Appearance, this computer):
// the Glass theme's progressive blur draws two backdrop layers per stack
// instead of four. Off by default — today's look.
//
// Perf note (what this file measures): one floating conversation has two
// stacks (top bar + behind the message box). Default: 8 live
// `backdrop-filter` layers, 88px of summed blur radius (2+6+12+24 per
// stack). Light blur: 4 layers, 40px (4+16 per stack). jsdom can't time the
// GPU's backdrop pass, so the measure is the work the compositor is handed:
// live blur layers and their summed radius, per conversation, asserted here.
// Backdrop-blur cost grows with the number of layers and with the radius,
// so Light blur roughly halves the layers and cuts the radius by ~55%.

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import {
  ZEN_MATERIALS_CSS,
  ZEN_PROGRESSIVE_BLUR_LAYERS,
  ZEN_PROGRESSIVE_BLUR_LAYERS_LIGHT,
  ZenMaterialsContext,
  zenProgressiveBlurLayers,
} from '@/lib/zen/zen-materials'
import { ZEN_LIGHT_BLUR_DEFAULT, ZEN_LIGHT_BLUR_KEY, readZenLightBlurStored, useZenLightBlurStore } from '@/lib/zen/zen-light-blur'
import { ZenProgressiveBlur } from './ZenProgressiveBlur'

const ROOT = '[data-zen-root][data-zen-materials="mobile"]'

/** One floating conversation's two blur stacks. */
function Conversation({ lightBlur, reduced = false }: { lightBlur?: boolean; reduced?: boolean }): React.JSX.Element {
  return (
    <div data-zen-root="" data-zen-materials="mobile">
      <ZenMaterialsContext.Provider value={{ materials: 'mobile', reducedTransparency: reduced, lightBlur }}>
        <ZenProgressiveBlur edge="header" />
        <ZenProgressiveBlur edge="footer" />
      </ZenMaterialsContext.Provider>
    </div>
  )
}

/** Live blur layers on the page and their summed radius (px), read from the
 *  CSS rule each rendered layer gets. */
function measure(): { layers: number; radius: number } {
  const spans = Array.from(document.querySelectorAll<HTMLElement>('[class^="zen-blur-layer-"]'))
  let radius = 0
  for (const s of spans) {
    const edge = s.parentElement?.getAttribute('data-zen-progressive-blur')
    const sel = `${ROOT} .zen-${edge}-blur > .${s.className} {`
    const at = ZEN_MATERIALS_CSS.indexOf(sel)
    if (at < 0) throw new Error(`no CSS rule for ${sel}`)
    const body = ZEN_MATERIALS_CSS.slice(at, ZEN_MATERIALS_CSS.indexOf('}', at))
    const m = body.match(/backdrop-filter: blur\((\d+)px\)/)
    if (!m) throw new Error(`no blur in ${body}`)
    radius += Number(m[1])
  }
  return { layers: spans.length, radius }
}

beforeEach(() => {
  localStorage.clear()
  useZenLightBlurStore.getState().reload()
})
afterEach(() => {
  cleanup()
  localStorage.clear()
})

describe('Light blur: before and after', () => {
  it('default (off) is today’s look: 8 layers, 88px per floating conversation', () => {
    render(<Conversation />)
    expect(measure()).toEqual({ layers: 8, radius: 88 })
    for (const edge of ['header', 'footer']) {
      const stack = document.querySelector(`[data-zen-progressive-blur="${edge}"]`)
      expect(Array.from(stack?.children ?? []).map((c) => c.className)).toEqual([
        'zen-blur-layer-1',
        'zen-blur-layer-2',
        'zen-blur-layer-3',
        'zen-blur-layer-4',
        'zen-blur-tint',
      ])
    }
  })

  it('on: 4 layers, 40px per floating conversation (the phone’s 4px + 16px per stack)', () => {
    render(<Conversation lightBlur />)
    expect(measure()).toEqual({ layers: 4, radius: 40 })
    for (const edge of ['header', 'footer']) {
      const stack = document.querySelector(`[data-zen-progressive-blur="${edge}"]`)
      expect(stack?.getAttribute('data-zen-blur-layers')).toBe('2')
      expect(Array.from(stack?.children ?? []).map((c) => c.className)).toEqual([
        'zen-blur-layer-light-1',
        'zen-blur-layer-light-2',
        'zen-blur-tint',
      ])
    }
  })

  it('reduced transparency wins: no layers either way', () => {
    render(<Conversation lightBlur reduced />)
    expect(measure()).toEqual({ layers: 0, radius: 0 })
    expect(ZEN_MATERIALS_CSS).toContain('.zen-progressive-blur > [class^="zen-blur-layer-"] { display: none; }')
  })

  it('the light layers keep the stack’s direction and overlap (no bands)', () => {
    expect(zenProgressiveBlurLayers(false)).toBe(ZEN_PROGRESSIVE_BLUR_LAYERS)
    expect(ZEN_PROGRESSIVE_BLUR_LAYERS_LIGHT.map(([px]) => px)).toEqual([4, 16])
    const [[, , firstTo], [, secondFrom]] = ZEN_PROGRESSIVE_BLUR_LAYERS_LIGHT
    expect(secondFrom).toBeLessThan(firstTo)
    for (const [edge, dir] of [
      ['header', 'to top'],
      ['footer', 'to bottom'],
    ] as const) {
      ZEN_PROGRESSIVE_BLUR_LAYERS_LIGHT.forEach(([px, from, to], i) => {
        const sel = `${ROOT} .zen-${edge}-blur > .zen-blur-layer-light-${i + 1} {`
        const at = ZEN_MATERIALS_CSS.indexOf(sel)
        expect(at, sel).toBeGreaterThan(0)
        const body = ZEN_MATERIALS_CSS.slice(at, ZEN_MATERIALS_CSS.indexOf('}', at))
        expect(body).toContain(`backdrop-filter: blur(${px}px);`)
        expect(body).toContain(`mask-image: linear-gradient(${dir}, transparent ${from}%, #000 ${to}%);`)
      })
    }
  })
})

describe('Light blur setting (this computer)', () => {
  it('is off by default and remembered in localStorage', () => {
    expect(ZEN_LIGHT_BLUR_DEFAULT).toBe(false)
    expect(useZenLightBlurStore.getState().on).toBe(false)
    useZenLightBlurStore.getState().setOn(true)
    expect(localStorage.getItem(ZEN_LIGHT_BLUR_KEY)).toBe('1')
    expect(readZenLightBlurStored()).toBe(true)
    useZenLightBlurStore.getState().setOn(false)
    expect(localStorage.getItem(ZEN_LIGHT_BLUR_KEY)).toBeNull()
    expect(useZenLightBlurStore.getState().on).toBe(false)
  })
})
