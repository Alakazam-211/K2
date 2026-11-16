// @vitest-environment jsdom
// The model picker's look against the real stylesheets (K2 Design's polish
// round on the Thread /model picker, 2026-11-16):
// - Glass: the popover is the standard menu Frost, as every other menu (a 97%
//   fill over a 48px blur), and nothing above it cuts the backdrop off.
// - "Find a model" focused: one ring. In Glass the field keeps the control
//   edge and the shared focus ring (2px of the accent as text) is the only
//   accent; in Terminal the cut field's own 2px ring is, with no outline.
// - The footer is one short line.
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { ModelPicker, MODEL_PICKER_WORDS } from './ModelPicker'
import { modelPickerRows } from '@/lib/model-choices'

const dir = dirname(fileURLToPath(import.meta.url))
const SHEETS = ['../../styles.generated.css', '../../globals.css']
let styles: HTMLStyleElement[] = []

/** jsdom drops a colour property whose value is a var(), so the edge and ring
 *  tokens become stand-in colours before the sheets load; the cascade
 *  (selectors, specificity, order) is the real one. */
const STAND_IN: Record<string, string> = {
  'control-edge': 'rgb(1, 2, 3)',
  accent: 'rgb(4, 5, 6)',
  'accent-text': 'rgb(7, 8, 9)',
}
const EDGE = STAND_IN['control-edge']
const ACCENT_TEXT = STAND_IN['accent-text']
function standIns(css: string): string {
  return css.replace(/var\(--color-([a-z-]+)\)/g, (all, name: string) => STAND_IN[name] ?? all)
}

beforeAll(() => {
  styles = SHEETS.map((f) => {
    const s = document.createElement('style')
    s.textContent = standIns(readFileSync(join(dir, f), 'utf8'))
    document.head.appendChild(s)
    return s
  })
  // Both sheets parsed: a sheet jsdom can't read would pass every check here.
  for (const s of styles) expect(s.sheet?.cssRules.length ?? 0).toBeGreaterThan(100)
})
afterAll(() => {
  for (const s of styles) s.remove()
})

type Look = { style: 'glass' | 'terminal'; scheme: 'dark' | 'light' }
const LOOKS: Look[] = [
  { style: 'glass', scheme: 'dark' },
  { style: 'glass', scheme: 'light' },
  { style: 'terminal', scheme: 'dark' },
  { style: 'terminal', scheme: 'light' },
]
const PALETTE = { glass: { dark: 'obsidian', light: 'veil' }, terminal: { dark: 'charcoal', light: 'paper' } } as const

function mount(look: Look) {
  const html = document.documentElement
  html.setAttribute('data-style', look.style)
  html.setAttribute('data-scheme', look.scheme)
  html.setAttribute('data-palette', PALETTE[look.style][look.scheme])
  const rows = (q: string) => modelPickerRows({ harness: 'claude', mode: 'thread', serverDefault: 'opus', query: q })
  render(
    <ModelPicker
      anchor={null}
      rows={rows}
      checked="haiku"
      state="ready"
      footer={MODEL_PICKER_WORDS.threadFooter}
      label="Model for this chat"
      onPick={() => {}}
      onClose={() => {}}
    />,
  )
  return {
    popover: screen.getByTestId('model-picker'),
    search: screen.getByTestId('model-picker-search') as HTMLInputElement,
  }
}

afterEach(() => {
  cleanup()
  for (const a of ['data-style', 'data-scheme', 'data-palette']) document.documentElement.removeAttribute(a)
})

describe('model picker: the menu Frost in Glass', () => {
  it('the popover is the standard menu Frost, and nothing above it cuts the backdrop off', () => {
    for (const scheme of ['dark', 'light'] as const) {
      const { popover } = mount({ style: 'glass', scheme })
      const cs = getComputedStyle(popover)
      expect(cs.backdropFilter).toBe('blur(48px) saturate(180%)')
      expect(cs.getPropertyValue('background')).toBe(scheme === 'dark' ? 'rgba(32, 32, 36, 0.97)' : 'rgba(255, 255, 255, 0.97)')
      expect(popover.getAttribute('data-material')).toBe('frost')
      expect(popover.getAttribute('data-float')).toBe('menu')
      // The standard menu, not the clear one meant for menus over the nav list.
      expect(popover.hasAttribute('data-float-clear')).toBe(false)
      // No inline paint over the material.
      expect(popover.style.background).toBe('')
      expect(popover.style.backgroundColor).toBe('')
      // A backdrop root between the popover and the page would blur nothing but itself.
      for (let e: Element | null = popover.parentElement; e && e !== document.documentElement; e = e.parentElement) {
        const s = getComputedStyle(e)
        expect(s.opacity === '' || s.opacity === '1', `${e.tagName} opacity ${s.opacity}`).toBe(true)
        for (const prop of ['filter', 'backdrop-filter', 'mask-image', 'clip-path', 'mix-blend-mode', 'will-change'] as const) {
          const v = s.getPropertyValue(prop)
          expect(v === '' || v === 'none' || v === 'normal' || v === 'auto', `${e.tagName} ${prop} ${v}`).toBe(true)
        }
        const t = s.getPropertyValue('transform')
        expect(t === '' || t === 'none', `${e.tagName} transform ${t}`).toBe(true)
      }
      cleanup()
    }
  })
})

describe('model picker: one focus ring on "Find a model"', () => {
  it('Glass: the field keeps the control edge; the shared ring is the only accent', () => {
    for (const scheme of ['dark', 'light'] as const) {
      const { search } = mount({ style: 'glass', scheme })
      expect(document.activeElement).toBe(search)
      expect(search.matches(':focus-visible')).toBe(true)
      const cs = getComputedStyle(search)
      for (const side of ['Top', 'Right', 'Bottom', 'Left'] as const) {
        expect(cs[`border${side}Color`], side).toBe(EDGE)
        expect(cs[`border${side}Width`], side).toBe('1px')
      }
      expect(cs.outline).toBe(`2px solid ${ACCENT_TEXT}`)
      expect(cs.outlineOffset).toBe('2px')
      cleanup()
    }
  })

  it('Terminal: the cut field draws its one 2px ring, with no outline beside it', () => {
    for (const scheme of ['dark', 'light'] as const) {
      const { search } = mount({ style: 'terminal', scheme })
      expect(document.activeElement).toBe(search)
      const cs = getComputedStyle(search)
      expect(cs.borderTopColor).toBe(ACCENT_TEXT)
      expect(cs.boxShadow).toBe(`inset 0 0 0 1px ${ACCENT_TEXT}`)
      expect(cs.outlineStyle === '' || cs.outlineStyle === 'none', `outline ${cs.outline}`).toBe(true)
      cleanup()
    }
  })
})

describe('model picker: the footer', () => {
  it('is one short line in every look', () => {
    expect(MODEL_PICKER_WORDS.threadFooter).toBe('This chat only, from your next message.')
    for (const look of LOOKS) {
      mount(look)
      expect(screen.getByTestId('model-picker-footer').textContent).toBe('This chat only, from your next message.')
      cleanup()
    }
  })
})
