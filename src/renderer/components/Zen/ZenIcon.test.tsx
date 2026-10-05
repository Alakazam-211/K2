// @vitest-environment jsdom
//
// Rosson 2026-10-04: the Zen toggle's icon candidates (`ZenIcon`) and the
// TEMPORARY preview switch (`lib/zen/zen-icon.ts`).
//
// Asserted (fail loudly):
//   - the preview lists ripples, enso, bonsai in that order with their
//     tooltip names; preview off is the one chosen icon; a bad choice throws;
//   - every variant's on state differs from its off state, and on uses the
//     accent;
//   - motion plays when an icon turns on (an on icon mounting, or a toggle)
//     and an off icon that just mounted stays still;
//   - `prefers-reduced-motion: reduce` skips all of it: still, no ensō mask,
//     no starting transform or opacity.

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { ZenIcon } from './ZenIcon'
import { ZEN_ICON_CHOICE, ZEN_ICON_OPTIONS, ZEN_ICON_PREVIEW, zenToggleIcons, type ZenIconVariant } from '@/lib/zen/zen-icon'
import { REDUCED_MOTION_QUERY } from '@/lib/zen/zen-theme'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const VARIANTS: readonly ZenIconVariant[] = ['ripples', 'enso', 'bonsai']

let reduced = false
let restoreMatchMedia: PropertyDescriptor | undefined

beforeEach(() => {
  reduced = false
  restoreMatchMedia = Object.getOwnPropertyDescriptor(window, 'matchMedia')
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: (query: string) => ({
      matches: query === REDUCED_MOTION_QUERY ? reduced : false,
      media: query,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    }),
  })
})

afterEach(() => {
  cleanup()
  if (restoreMatchMedia) Object.defineProperty(window, 'matchMedia', restoreMatchMedia)
  else delete (window as { matchMedia?: unknown }).matchMedia
})

function svg(container: HTMLElement): SVGSVGElement {
  const el = container.querySelector('svg[data-zen-icon]')
  if (!(el instanceof SVGSVGElement)) throw new Error('no Zen icon svg')
  return el
}

/** The parts Motion animates, with their inline starting style. */
function animatedParts(container: HTMLElement): Array<{ part: string; opacity: string; transform: string }> {
  return Array.from(container.querySelectorAll('[data-zen-icon-part="ripple"], [data-zen-icon-part="pad"]')).map((p) => ({
    part: p.getAttribute('data-zen-icon-part') ?? '',
    opacity: (p as SVGElement).style.opacity,
    transform: (p as SVGElement).style.transform,
  }))
}

describe('the icon config (lib/zen/zen-icon.ts)', () => {
  it('preview: all three, in order, named for the tooltip', () => {
    expect(zenToggleIcons(true, 'enso').map((o) => [o.variant, o.label])).toEqual([
      ['ripples', 'Option 1: Stone & ripples'],
      ['enso', 'Option 2: Ensō'],
      ['bonsai', 'Option 3: Bonsai'],
    ])
  })

  it('preview off: just the chosen one', () => {
    for (const v of VARIANTS) expect(zenToggleIcons(false, v).map((o) => o.variant)).toEqual([v])
  })

  it('an unknown choice is loud', () => {
    expect(() => zenToggleIcons(false, 'koi' as ZenIconVariant)).toThrow('zen icon: unknown choice "koi"')
  })

  it('the defaults are the module constants', () => {
    expect(zenToggleIcons()).toEqual(zenToggleIcons(ZEN_ICON_PREVIEW, ZEN_ICON_CHOICE))
    expect(ZEN_ICON_OPTIONS.map((o) => o.variant)).toEqual(VARIANTS)
  })
})

describe('ZenIcon', () => {
  it.each(VARIANTS)('%s: on differs from off, and on uses the accent', (variant) => {
    reduced = true
    const off = render(<ZenIcon variant={variant} on={false} accent="#c0ffee" />)
    const offSvg = svg(off.container)
    expect([offSvg.getAttribute('data-zen-icon'), offSvg.getAttribute('data-zen-icon-on')]).toEqual([variant, 'false'])
    expect(offSvg.innerHTML).not.toContain('#c0ffee')
    const offHtml = offSvg.innerHTML
    cleanup()
    const on = render(<ZenIcon variant={variant} on accent="#c0ffee" />)
    const onSvg = svg(on.container)
    expect(onSvg.getAttribute('data-zen-icon-on')).toBe('true')
    expect(onSvg.innerHTML).toContain('#c0ffee')
    expect(onSvg.innerHTML).not.toBe(offHtml)
  })

  it.each(VARIANTS)('%s: a 24 viewBox in currentColor at the given size', (variant) => {
    const { container } = render(<ZenIcon variant={variant} on={false} size={16} />)
    const el = svg(container)
    expect([el.getAttribute('viewBox'), el.getAttribute('width'), el.getAttribute('height')]).toEqual(['0 0 24 24', '16', '16'])
    expect(el.innerHTML).toContain('currentColor')
  })

  it.each(VARIANTS)('%s: an off icon that just mounted stays still', (variant) => {
    const { container } = render(<ZenIcon variant={variant} on={false} />)
    expect(svg(container).getAttribute('data-zen-icon-motion')).toBe('still')
    expect(container.querySelector('mask')).toBeNull()
    for (const p of animatedParts(container)) expect(p.opacity === '' || p.opacity === '1').toBe(true)
  })

  it('turning on plays: the ripples ease outward from small and clear', () => {
    const r = render(<ZenIcon variant="ripples" on={false} />)
    r.rerender(<ZenIcon variant="ripples" on />)
    expect(svg(r.container).getAttribute('data-zen-icon-motion')).toBe('play')
    const parts = animatedParts(r.container)
    expect(parts.map((p) => p.part)).toEqual(['ripple', 'ripple'])
    expect(parts.map((p) => p.opacity)).toEqual(['0', '0'])
    expect(parts.every((p) => p.transform.includes('scale(0.72)'))).toBe(true)
  })

  it('an on icon mounting plays (that is how entering Zen looks): the ensō draws in', () => {
    const { container } = render(<ZenIcon variant="enso" on />)
    expect(svg(container).getAttribute('data-zen-icon-motion')).toBe('play')
    const mask = container.querySelector('mask')
    if (!mask) throw new Error('no ensō draw mask')
    const brush = container.querySelector('[data-zen-icon-part="enso"]')
    expect(brush?.getAttribute('mask')).toBe(`url(#${mask.id})`)
    const draw = mask.querySelector('[data-zen-icon-part="enso-draw"]')
    // pathLength 0: Motion draws it as a zero-length dash.
    expect(draw?.getAttribute('pathLength')).toBe('1')
    expect(draw?.getAttribute('stroke-dasharray')).toMatch(/^0(px)? 1(px)?$/)
  })

  it('the bonsai’s pads pop with a sway when it turns on', () => {
    const { container } = render(<ZenIcon variant="bonsai" on />)
    expect(svg(container).getAttribute('data-zen-icon-motion')).toBe('play')
    const parts = animatedParts(container)
    expect(parts.map((p) => p.part)).toEqual(['pad', 'pad', 'pad'])
    expect(parts.every((p) => p.transform.includes('scale(0.7)') && p.transform.includes('rotate('))).toBe(true)
  })

  it.each(VARIANTS)('%s: reduced motion skips all of it, on mount and on toggle', (variant) => {
    reduced = true
    const r = render(<ZenIcon variant={variant} on />)
    const check = (): void => {
      expect(svg(r.container).getAttribute('data-zen-icon-motion')).toBe('still')
      expect(r.container.querySelector('mask')).toBeNull()
      expect(r.container.querySelector('[data-zen-icon-part="enso"]')?.getAttribute('mask') ?? null).toBeNull()
      for (const p of animatedParts(r.container)) {
        expect(p.opacity === '' || p.opacity === '1').toBe(true)
        expect(p.transform === '' || p.transform === 'none').toBe(true)
      }
    }
    check()
    r.rerender(<ZenIcon variant={variant} on={false} />)
    check()
    r.rerender(<ZenIcon variant={variant} on />)
    check()
  })
})
