// @vitest-environment jsdom
// The desktop Thread's reply bar (Rosson 2026-11-15: "the reply object that
// floats above the activity ear or message the agent area is kinda bad
// too. It is wider than the activity ear and it's completely
// transparent"). It stands on the ear's own edges (THREAD_EAR_INSET_PX in
// from the box, as the ear and the stash), says "Replying to <name>" and the
// quoted message's first line in whole words, and × or Esc cancels it.
// Glass: the Jump to bottom pill's glass (the shared Frost, its scrim, rim
// and ink). Terminal: a solid raised surface, the control edge, cut corners.
// The stylesheets are the real ones: the Styles' and the bar's.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { THREAD_EAR_INSET_PX, THREAD_REPLY_BAR_CSS, THREAD_REPLY_BAR_GAP, ThreadReplyBar, fitReplyWords } from './ThreadReplyBar'
import { THREAD_ZEN_SCOPE_ATTR } from './chamfer'
import type { ThreadReplyTarget } from '@/lib/thread-reply'

const dir = dirname(fileURLToPath(import.meta.url))
const generated = readFileSync(join(dir, '../../styles.generated.css'), 'utf8')

const TARGET: ThreadReplyTarget = {
  id: 'm42',
  seq: 42,
  from: 'sales',
  addr: 'sales',
  text: 'Ship the release notes today\nand the rest tomorrow.',
  part: false,
}

function mount(look: { style: 'glass' | 'terminal'; scheme?: 'dark' | 'light'; frost?: 'tile' }, target: ThreadReplyTarget = TARGET) {
  const html = document.documentElement
  html.setAttribute('data-style', look.style)
  html.setAttribute('data-palette', look.style === 'glass' ? (look.scheme === 'light' ? 'veil' : 'obsidian') : 'charcoal')
  html.setAttribute('data-scheme', look.scheme ?? 'dark')
  if (look.frost) html.setAttribute('data-frost', look.frost)
  else html.removeAttribute('data-frost')
  const onCancel = vi.fn()
  const view = render(
    <div {...{ [THREAD_ZEN_SCOPE_ATTR]: '' }}>
      <div data-compose-bar="">
        <ThreadReplyBar target={target} onCancel={onCancel} />
      </div>
    </div>,
  )
  const bar = view.container.querySelector('[data-thread-reply-bar]') as HTMLElement
  return { bar, onCancel, row: view.container.querySelector('[data-thread-reply-row]') as HTMLElement }
}

let sheets: HTMLStyleElement[] = []
function withSheets(): void {
  sheets = [generated, THREAD_REPLY_BAR_CSS].map((css) => {
    const s = document.createElement('style')
    s.textContent = css
    document.head.appendChild(s)
    return s
  })
}

afterEach(() => {
  cleanup()
  for (const s of sheets) s.remove()
  sheets = []
  for (const a of ['data-style', 'data-palette', 'data-scheme', 'data-frost']) document.documentElement.removeAttribute(a)
})

describe('reply bar: words', () => {
  it('"Replying to <name>", the first line, and a named ×', () => {
    const { bar } = mount({ style: 'glass' })
    expect(bar.querySelector('[data-reply-chip-who]')?.textContent).toBe('Replying to sales')
    expect(bar.querySelector('[data-reply-chip-text]')?.textContent).toBe('Ship the release notes today')
    expect(bar.getAttribute('aria-label')).toBe('Replying to sales: Ship the release notes today')
    const x = bar.querySelector('[data-testid="thread-reply-cancel"]') as HTMLButtonElement
    expect(x.tagName).toBe('BUTTON')
    expect(x.getAttribute('aria-label')).toBe('Cancel the reply')
  })

  it('a reply to a selection says so', () => {
    const { bar } = mount({ style: 'glass' }, { ...TARGET, part: true, text: 'the rest tomorrow' })
    expect(bar.querySelector('[data-reply-chip-who]')?.textContent).toBe('Replying to part of sales')
  })

  it('the first line fits in whole words: the tail goes as a whole phrase, then "…"', () => {
    const w = (s: string): number => s.length * 7
    const line = 'Ship the release notes today and the rest tomorrow'
    expect(fitReplyWords(line, w(line), w)).toBe(line)
    // Room for "Ship the release notes…" (23 chars) but not "…today…".
    expect(fitReplyWords(line, 23 * 7 + 3, w)).toBe('Ship the release notes…')
    // Never a cut word, even with room for half of the next one.
    expect(fitReplyWords(line, 26 * 7, w)).toBe('Ship the release notes…')
    // Not even the first word: the ellipsis alone.
    expect(fitReplyWords(line, 3 * 7, w)).toBe('…')
    // Unmeasured (no room known yet): the whole line.
    expect(fitReplyWords(line, 0, w)).toBe(line)
  })
})

describe('reply bar: cancel', () => {
  it('× cancels', () => {
    const { bar, onCancel } = mount({ style: 'glass' })
    fireEvent.click(bar.querySelector('[data-testid="thread-reply-cancel"]') as Element)
    expect(onCancel).toHaveBeenCalledTimes(1)
  })

  it('Esc cancels with the focus on ×', () => {
    const { bar, onCancel } = mount({ style: 'glass' })
    const x = bar.querySelector('[data-testid="thread-reply-cancel"]') as HTMLButtonElement
    x.focus()
    fireEvent.keyDown(x, { key: 'Escape' })
    expect(onCancel).toHaveBeenCalledTimes(1)
  })

  it('× has a focus ring in both looks', () => {
    for (const style of ['glass', 'terminal'] as const) {
      const { bar } = mount({ style })
      const x = bar.querySelector('[data-testid="thread-reply-cancel"]') as HTMLElement
      const rule = THREAD_REPLY_BAR_CSS.split('\n').find((l) => l.includes(':focus-visible') && l.includes('[data-reply-cancel]'))
      expect(rule).toBeDefined()
      expect(rule).toMatch(/outline: 2px solid var\(--zen-focus-ring/)
      expect(x.hasAttribute('data-reply-cancel')).toBe(true)
      cleanup()
    }
  })
})

describe('reply bar: on the ear', () => {
  it("stands on the ear's edges, the pill's gap above it", () => {
    const { row } = mount({ style: 'glass' })
    expect(THREAD_EAR_INSET_PX).toBe(14)
    expect(row.style.margin).toBe(`0px ${THREAD_EAR_INSET_PX}px ${THREAD_REPLY_BAR_GAP}px`)
    // A row of the bar itself, so the pill and the fade see it standing on the box.
    expect(row.parentElement?.hasAttribute('data-compose-bar')).toBe(true)
  })
})

describe('reply bar: Glass', () => {
  it("the pill's glass: computed blur on the painted element, its scrim fill, rim and ink, nothing above it cutting the backdrop off", () => {
    withSheets()
    for (const scheme of ['dark', 'light'] as const) {
      const { bar } = mount({ style: 'glass', scheme })
      expect(bar.getAttribute('data-material')).toBe('frost')
      const cs = getComputedStyle(bar)
      expect(cs.backdropFilter).toBe('blur(var(--material-blur)) saturate(var(--material-saturate))')
      expect(cs.getPropertyValue('background')).toBe('linear-gradient(var(--k2-jump-tint) 0 0), var(--k2-jump-scrim)')
      // jsdom can't compute a border colour from var(): the rim rule must match the bar.
      const rim = THREAD_REPLY_BAR_CSS.split('\n').find((l) => l.includes('border-color: var(--k2-jump-edge)'))
      expect(rim).toBeDefined()
      expect(bar.matches(rim!.slice(0, rim!.indexOf('{')).trim())).toBe(true)
      expect(cs.color).toBe('var(--k2-jump-ink)')
      expect(cs.minHeight).toBe('var(--k2-jump-h)')
      expect(bar.style.background).toBe('')
      expect(bar.style.color).toBe('')
      for (let e: Element | null = bar; e && e !== document.documentElement; e = e.parentElement) {
        const s = getComputedStyle(e)
        expect(s.opacity === '' || s.opacity === '1', `${e.tagName} opacity ${s.opacity}`).toBe(true)
        for (const prop of ['filter', 'mask-image', 'clip-path', 'mix-blend-mode', 'will-change'] as const) {
          const v = s.getPropertyValue(prop)
          expect(v === '' || v === 'none' || v === 'normal' || v === 'auto', `${e.tagName} ${prop} ${v}`).toBe(true)
        }
        if (e !== bar) expect(s.backdropFilter === '' || s.backdropFilter === 'none', `${e.tagName} backdrop-filter`).toBe(true)
      }
      // Every word in the ink (the muted grey misses 4.5:1 over your bubble).
      for (const el of Array.from(bar.querySelectorAll('[data-reply-chip-who], [data-reply-chip-text], [data-reply-cancel]'))) {
        expect(getComputedStyle(el).color, el.outerHTML).toBe('var(--k2-jump-ink)')
      }
      cleanup()
    }
  })

  it('reduced transparency: the solid tile, no scrim', () => {
    withSheets()
    const { bar } = mount({ style: 'glass', frost: 'tile' })
    const cs = getComputedStyle(bar)
    expect(cs.getPropertyValue('background')).toBe('var(--tile-raised)')
    expect(cs.backdropFilter).toBe('none')
  })
})

describe('reply bar: Terminal', () => {
  it('a solid raised surface with the control edge and cut corners, no frost', () => {
    withSheets()
    const { bar } = mount({ style: 'terminal' })
    expect(bar.hasAttribute('data-chamfer')).toBe(true)
    expect(bar.style.getPropertyValue('--ch-fill')).toBe('var(--zen-surface-raised)')
    expect(bar.style.getPropertyValue('--ch-edge')).toBe('var(--zen-control-edge, var(--zen-border))')
    expect(bar.style.getPropertyValue('--ch-tl')).toBe('5px')
    const cs = getComputedStyle(bar)
    expect(cs.backdropFilter === '' || cs.backdropFilter === 'none').toBe(true)
    expect(cs.color).toBe('var(--zen-text)')
  })
})
