// @vitest-environment jsdom
// The Jump to bottom pill in Glass is K2 Companion's glass pill (Rosson
// 2026-11-15, "following the same glass theme that mobile has"): the shared
// Frost material (the phone's .zen-glass: 10% white over a 28px blur at 180%
// saturation, a 0.5px white rim, a 1px light catch), a full pill, 13px words
// at 32px tall with 14px sides, the "↓" glyph, the same quiet pill for every
// state, the phone's entrance (a 6px rise from 96% over 220ms) and a 140ms
// fade out. Its fill sits over a scrim and its rim is a little brighter, so
// the words keep 4.5:1 and the edge 3:1 over the messages (THREAD_JUMP_PILL_CSS).
// Terminal keeps its chamfered pill. Rules are matched against the real
// element (jsdom's `matches`), so a selector that misses the pill fails here.
import { afterEach, describe, expect, it } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  THREAD_JUMP_PILL_ANIMATING,
  THREAD_JUMP_PILL_CSS,
  THREAD_JUMP_PILL_WIDTH_MS,
  ThreadJumpPill,
  type ThreadJumpPillProps,
} from './ThreadJumpPill'
import { THREAD_ZEN_SCOPE_ATTR } from './chamfer'

const dir = dirname(fileURLToPath(import.meta.url))
const globals = readFileSync(join(dir, '../../globals.css'), 'utf8')
const glassTokens = JSON.parse(readFileSync(join(dir, '../../../../styles/glass/tokens.json'), 'utf8'))
const obsidian = JSON.parse(readFileSync(join(dir, '../../../../styles/glass/palettes/obsidian.json'), 'utf8'))

/** One rule's selectors and body from a stylesheet's text, outside @media. */
interface Rule {
  selectors: string[]
  body: string
}

function splitSelectors(list: string): string[] {
  const out: string[] = []
  let depth = 0
  let cur = ''
  for (const ch of list) {
    if (ch === '(') depth++
    if (ch === ')') depth--
    if (ch === ',' && depth === 0) {
      out.push(cur)
      cur = ''
    } else cur += ch
  }
  out.push(cur)
  return out.map((s) => s.trim()).filter(Boolean)
}

/** Top-level rules (not inside an at-rule), and the at-rules' inner rules. */
function parse(css: string): { top: Rule[]; media: Record<string, Rule[]> } {
  const top: Rule[] = []
  const media: Record<string, Rule[]> = {}
  let i = 0
  const read = (until: number, into: Rule[]): void => {
    while (i < until) {
      const open = css.indexOf('{', i)
      if (open < 0 || open >= until) break
      const head = css.slice(i, open).trim()
      if (head.startsWith('@')) {
        let depth = 1
        let j = open + 1
        while (depth > 0) {
          if (css[j] === '{') depth++
          if (css[j] === '}') depth--
          j++
        }
        const inner: Rule[] = []
        i = open + 1
        read(j - 1, inner)
        media[head] = [...(media[head] ?? []), ...inner]
        i = j
        continue
      }
      const close = css.indexOf('}', open)
      into.push({ selectors: splitSelectors(head), body: css.slice(open + 1, close) })
      i = close + 1
    }
  }
  read(css.length, top)
  return { top, media }
}

const pillCss = parse(THREAD_JUMP_PILL_CSS)

/** The declarations every top-level rule matching `el` gives `prop`, in order. */
function declared(el: Element, prop: string, rules: Rule[] = pillCss.top): string[] {
  const out: string[] = []
  for (const r of rules) {
    if (!r.selectors.some((s) => el.matches(s))) continue
    for (const d of r.body.split(';')) {
      const at = d.indexOf(':')
      if (at < 0) continue
      if (d.slice(0, at).trim() === prop) out.push(d.slice(at + 1).trim())
    }
  }
  return out
}

/** A `--name: value` declaration in a globals.css block for exactly `selector`
 *  (the last one wins, as in the cascade). */
function token(selector: string, name: string): string {
  let found: string | null = null
  for (let at = globals.indexOf(`\n${selector} {`); at >= 0; at = globals.indexOf(`\n${selector} {`, at + 1)) {
    const body = globals.slice(at, globals.indexOf('}', at))
    const m = body.match(new RegExp(`${name}:\\s*([^;]+);`))
    if (m) found = m[1].trim()
  }
  if (found === null) throw new Error(`missing ${name} in ${selector}`)
  return found
}

const BASE: ThreadJumpPillProps = { shown: true, unseen: 0, sent: false, reducedMotion: false, onJump: () => {}, bottom: 64 }

function mount(look: { style: 'glass' | 'terminal'; scheme?: 'dark' | 'light'; frost?: 'tile' }, props: Partial<ThreadJumpPillProps> = {}) {
  const html = document.documentElement
  html.setAttribute('data-style', look.style)
  html.setAttribute('data-palette', look.style === 'glass' ? (look.scheme === 'light' ? 'veil' : 'obsidian') : 'charcoal')
  html.setAttribute('data-scheme', look.scheme ?? 'dark')
  if (look.frost) html.setAttribute('data-frost', look.frost)
  else html.removeAttribute('data-frost')
  const wrap = (p: ThreadJumpPillProps) => (
    <div {...{ [THREAD_ZEN_SCOPE_ATTR]: '' }}>
      <ThreadJumpPill {...p} />
    </div>
  )
  const view = render(wrap({ ...BASE, ...props }))
  const pill = (): HTMLButtonElement => view.container.querySelector('[data-testid="thread-jump-pill"]') as HTMLButtonElement
  return { pill, rerender: (p: Partial<ThreadJumpPillProps>) => view.rerender(wrap({ ...BASE, ...props, ...p })) }
}

afterEach(() => {
  cleanup()
  for (const a of ['data-style', 'data-palette', 'data-scheme', 'data-frost']) document.documentElement.removeAttribute(a)
})

describe('Jump to bottom pill: Glass is the phone glass pill', () => {
  it('the desktop Frost is the phone .zen-glass, value for value', () => {
    expect(glassTokens.material.blur.$value).toBe('28px')
    expect(glassTokens.material.saturate.$value).toBe('180%')
    expect(glassTokens.radius.pill.$value).toBe('999px')
    expect(glassTokens.type.lg.$value).toBe('13px')
    expect(glassTokens.motion['ease-standard'].$value).toBe('cubic-bezier(0.22, 1, 0.36, 1)')
    expect(obsidian.color['frost-fill'].$value).toBe('rgba(255, 255, 255, 0.10)')
    expect(obsidian.color['frost-edge'].$value).toBe('rgba(255, 255, 255, 0.22)')
    expect(obsidian.color['frost-sheen'].$value).toBe('rgba(255, 255, 255, 0.28)')
  })

  it('carries the Frost material in a pill shape, with no inline paint to cover it', () => {
    for (const unseen of [0, 3]) {
      const { pill } = mount({ style: 'glass' }, { unseen })
      const p = pill()
      expect(p.getAttribute('data-material')).toBe('frost')
      expect(p.getAttribute('data-frost-shape')).toBe('pill')
      // Inline paint would beat the shared Frost rule: the colours ride variables.
      expect(p.style.background).toBe('')
      expect(p.style.backgroundColor).toBe('')
      expect(p.style.border).toBe('')
      expect(p.style.borderColor).toBe('')
      expect(p.style.color).toBe('')
      expect(p.style.getPropertyValue('--jump-fill')).not.toBe('')
      cleanup()
    }
  })

  it("Glass: the phone's frost over a scrim, its rim, ink, size and type, in every state", () => {
    for (const [unseen, sent] of [[0, false], [1, false], [3, false], [0, true]] as const) {
      const { pill } = mount({ style: 'glass' }, { unseen, sent })
      const p = pill()
      expect(declared(p, 'background').at(-1)).toBe('linear-gradient(var(--k2-jump-tint) 0 0), var(--k2-jump-scrim)')
      expect(declared(p, 'border-color').at(-1)).toBe('var(--k2-jump-edge)')
      expect(declared(p, 'color').at(-1)).toBe('var(--k2-jump-ink)')
      expect(declared(p, 'min-height').at(-1)).toBe('var(--k2-jump-h)')
      expect(declared(p, 'font-size').at(-1)).toBe('var(--k2-jump-type)')
      expect(declared(p, 'font-weight').at(-1)).toBe('400')
      const inner = p.querySelector('[data-jump-inner]') as Element
      expect(declared(inner, 'padding').at(-1)).toBe('0 var(--k2-jump-pad)')
      cleanup()
    }
  })

  it('the tokens: the phone values, a scrim and a rim for contrast, light ink on Veil', () => {
    const dark = 'html[data-style="glass"]'
    expect(token(dark, '--k2-jump-tint')).toBe('var(--color-frost-fill)')
    expect(token(dark, '--k2-jump-scrim')).toBe('rgba(0, 0, 0, 0.6)')
    expect(token(dark, '--k2-jump-edge')).toBe('rgba(255, 255, 255, 0.32)')
    expect(token(dark, '--k2-jump-ink')).toBe('#ffffff')
    expect(token(dark, '--k2-jump-h')).toBe('32px')
    expect(token(dark, '--k2-jump-pad')).toBe('14px')
    expect(token(dark, '--k2-jump-type')).toBe('var(--type-lg)')
    const light = 'html[data-style="glass"][data-scheme="light"]'
    expect(token(light, '--k2-jump-scrim')).toBe('transparent')
    expect(token(light, '--k2-jump-edge')).toBe('rgba(0, 0, 0, 0.45)')
    expect(token(light, '--k2-jump-ink')).toBe('var(--color-text-primary)')
  })

  it('the computed backdrop blur is on the element that paints the fill, and nothing above it cuts the backdrop off', () => {
    // The real stylesheets: the Styles' (the shared Frost rule) and the pill's.
    const sheets = [readFileSync(join(dir, '../../styles.generated.css'), 'utf8'), THREAD_JUMP_PILL_CSS].map((css) => {
      const s = document.createElement('style')
      s.textContent = css
      document.head.appendChild(s)
      return s
    })
    try {
      for (const scheme of ['dark', 'light'] as const) {
        const { pill } = mount({ style: 'glass', scheme }, { unseen: 3 })
        const p = pill()
        const cs = getComputedStyle(p)
        expect(cs.backdropFilter).toBe('blur(var(--material-blur)) saturate(var(--material-saturate))')
        // The same box paints the tint and the scrim (no pseudo-element, no child layer).
        expect(cs.getPropertyValue('background')).toBe('linear-gradient(var(--k2-jump-tint) 0 0), var(--k2-jump-scrim)')
        expect(p.querySelector('[data-material]')).toBeNull()
        // A backdrop root between the pill and the page would blur nothing but itself.
        for (let e: Element | null = p; e && e !== document.documentElement; e = e.parentElement) {
          const s = getComputedStyle(e)
          expect(s.opacity === '' || s.opacity === '1', `${e.tagName} opacity ${s.opacity}`).toBe(true)
          for (const prop of ['filter', 'mask-image', 'clip-path', 'mix-blend-mode', 'will-change'] as const) {
            const v = s.getPropertyValue(prop)
            expect(v === '' || v === 'none' || v === 'normal' || v === 'auto', `${e.tagName} ${prop} ${v}`).toBe(true)
          }
          if (e !== p) expect(s.backdropFilter === '' || s.backdropFilter === 'none', `${e.tagName} backdrop-filter`).toBe(true)
        }
        cleanup()
      }
    } finally {
      for (const s of sheets) s.remove()
    }
  })

  it('reduced transparency (Frost falls back to a tile): no scrim fill, the Glass ink and size stay', () => {
    const { pill } = mount({ style: 'glass', frost: 'tile' }, { unseen: 2 })
    const p = pill()
    expect(declared(p, 'background')).not.toContain('linear-gradient(var(--k2-jump-tint) 0 0), var(--k2-jump-scrim)')
    expect(declared(p, 'color').at(-1)).toBe('var(--k2-jump-ink)')
    expect(declared(p, 'min-height').at(-1)).toBe('var(--k2-jump-h)')
  })

  it('Terminal keeps its own pill: none of the Glass rules apply', () => {
    const { pill } = mount({ style: 'terminal' }, { unseen: 2 })
    const p = pill()
    expect(declared(p, 'min-height')).toEqual([])
    expect(declared(p, 'background')).toEqual(['var(--jump-fill)'])
    expect(declared(p, 'color')).toEqual(['var(--jump-ink)'])
    // The accent count, as before.
    expect(p.style.getPropertyValue('--jump-fill')).toBe('var(--zen-accent-fill, var(--zen-accent))')
    expect(p.querySelector('[data-k2-icon="arrow-down"]')).not.toBeNull()
    const glyph = p.querySelector('[data-jump-glyph]') as Element
    expect(declared(glyph, 'display').at(-1)).toBe('none')
  })
})

describe('Jump to bottom pill: the words', () => {
  it('"Jump to bottom ↓", "New message ↓", "3 new messages ↓", "Sent ↓", with the glyph in Glass', () => {
    const cases = [
      { unseen: 0, sent: false, text: 'Jump to bottom', label: 'Jump to bottom' },
      { unseen: 1, sent: false, text: 'New message', label: 'New message, jump to bottom' },
      { unseen: 3, sent: false, text: '3 new messages', label: '3 new messages, jump to bottom' },
      { unseen: 0, sent: true, text: 'Sent', label: 'Sent, jump to bottom' },
    ]
    for (const c of cases) {
      const { pill } = mount({ style: 'glass' }, { unseen: c.unseen, sent: c.sent })
      const p = pill()
      expect(p.querySelector('[data-jump-text]')?.textContent).toBe(c.text)
      expect(p.getAttribute('aria-label')).toBe(c.label)
      const glyph = p.querySelector('[data-jump-glyph]') as Element
      expect(glyph.textContent).toBe('↓')
      expect(glyph.getAttribute('aria-hidden')).toBe('true')
      // Glass shows the glyph, not the icon.
      expect(declared(glyph, 'display').at(-1)).toBe('inline')
      expect(declared(p.querySelector('[data-k2-icon="arrow-down"]') as Element, 'display').at(-1)).toBe('none')
      cleanup()
    }
  })
})

describe('Jump to bottom pill: motion', () => {
  it("Glass enters as the phone's pill does and fades out in 140ms", () => {
    const { pill, rerender } = mount({ style: 'glass' }, { shown: false })
    expect(declared(pill(), 'transform').at(-1)).toBe('translate(-50%, 6px) scale(0.96)')
    expect(declared(pill(), 'transition').at(-1)).toMatch(/^opacity 140ms ease-out, /)
    rerender({ shown: true })
    expect(declared(pill(), 'transform').at(-1)).toBe('translate(-50%, 0) scale(1)')
    expect(declared(pill(), 'transition').at(-1)).toMatch(
      new RegExp(`^opacity ${THREAD_JUMP_PILL_WIDTH_MS}ms var\\(--motion-ease-standard, cubic-bezier\\(0\\.22, 1, 0\\.36, 1\\)\\), transform ${THREAD_JUMP_PILL_WIDTH_MS}ms`),
    )
  })

  it('reduced motion: no width animation, and no entrance, exit or glide in any look', () => {
    const { pill, rerender } = mount({ style: 'glass' }, { reducedMotion: true })
    act(() => rerender({ unseen: 1 }))
    expect(pill().classList.contains(THREAD_JUMP_PILL_ANIMATING)).toBe(false)
    act(() => rerender({ unseen: 2 }))
    expect(pill().classList.contains(THREAD_JUMP_PILL_ANIMATING)).toBe(false)
    const reduced = pillCss.media['@media (prefers-reduced-motion: reduce)']
    expect(reduced).toBeDefined()
    const p = pill()
    expect(declared(p, 'transition', reduced)).toEqual(['none !important'])
    p.classList.add(THREAD_JUMP_PILL_ANIMATING)
    expect(declared(p.querySelector('[data-jump-clip]') as Element, 'transition', reduced)).toEqual(['none !important'])
  })

  it('with motion, new words glide the width', () => {
    const { pill, rerender } = mount({ style: 'glass' })
    act(() => rerender({ unseen: 1 }))
    expect(pill().classList.contains(THREAD_JUMP_PILL_ANIMATING)).toBe(true)
  })
})
