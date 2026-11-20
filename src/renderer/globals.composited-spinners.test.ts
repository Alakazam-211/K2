// The app's always-on spinners (the braille spinner and the working grid)
// can be on screen dozens of times at once. If their keyframes animate
// anything but `transform` or `opacity`, the browser can't hand them to the
// compositor and redoes style, layout and paint every frame for each one.
// These checks read globals.css itself, so a later edit can't quietly bring
// a `content` (or colour, size, position …) animation back.

import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const dir = dirname(fileURLToPath(import.meta.url))
const css = readFileSync(join(dir, 'globals.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')

type Block = { prelude: string; body: string }

/** The `{ … }` blocks at one nesting level of `src`, with what precedes each. */
function blocksOf(src: string): Block[] {
  const out: Block[] = []
  let i = 0
  let start = 0
  while (i < src.length) {
    const ch = src[i]
    if (ch === ';') {
      start = i + 1
    } else if (ch === '{') {
      let depth = 1
      let j = i + 1
      while (j < src.length && depth > 0) {
        if (src[j] === '{') depth++
        else if (src[j] === '}') depth--
        j++
      }
      out.push({ prelude: src.slice(start, i).trim(), body: src.slice(i + 1, j - 1) })
      i = j
      start = j
      continue
    } else if (ch === '}') {
      start = i + 1
    }
    i++
  }
  return out
}

/** Every style rule (also inside @media / @supports) and every @keyframes. */
function walk(src: string, rules: Block[], keyframes: Map<string, string>): void {
  for (const b of blocksOf(src)) {
    const kf = /^@(?:-webkit-)?keyframes\s+([\w-]+)$/.exec(b.prelude)
    if (kf) keyframes.set(kf[1], b.body)
    else if (b.prelude.startsWith('@')) walk(b.body, rules, keyframes)
    else rules.push(b)
  }
}

const rules: Block[] = []
const keyframes = new Map<string, string>()
walk(css, rules, keyframes)

function declarations(body: string): Array<[string, string]> {
  return body
    .split(';')
    .map((d) => d.trim())
    .filter(Boolean)
    .map((d) => {
      const at = d.indexOf(':')
      return [d.slice(0, at).trim().toLowerCase(), d.slice(at + 1).trim()] as [string, string]
    })
}

/** The keyframes the rules matching `selector` run. */
function animationsFor(selector: RegExp): Set<string> {
  const names = new Set<string>()
  for (const r of rules) {
    if (!selector.test(r.prelude)) continue
    for (const [prop, value] of declarations(r.body)) {
      if (prop !== 'animation' && prop !== 'animation-name') continue
      for (const token of value.split(/[\s,]+/)) if (keyframes.has(token)) names.add(token)
    }
  }
  return names
}

/** The properties a @keyframes block sets, across all its steps. */
function animatedProps(name: string): string[] {
  const props = new Set<string>()
  for (const step of blocksOf(keyframes.get(name) ?? '')) {
    for (const [prop] of declarations(step.body)) {
      // A per-step timing function is allowed: it says how to move, not what.
      if (prop !== 'animation-timing-function') props.add(prop)
    }
  }
  return [...props].sort()
}

const COMPOSITED = new Set(['opacity', 'transform'])

describe.each([
  ['the braille spinner', /\.braille-spinner/],
  ['the working grid', /\.agent-working|\[data-agent-working\]/],
])('%s animates only transform and opacity', (_label, selector) => {
  const names = [...animationsFor(selector)]

  it('runs at least one keyframes animation', () => {
    expect(names.length).toBeGreaterThan(0)
  })

  it.each(names.length ? names : ['(none)'])('@keyframes %s', (name) => {
    const props = animatedProps(name)
    expect(props.length).toBeGreaterThan(0)
    expect(props.filter((p) => !COMPOSITED.has(p))).toEqual([])
  })
})

describe('both spinners pause while the window is hidden', () => {
  // `installDocHiddenAttr` (lib/doc-hidden.ts) sets `data-doc-hidden` on
  // <html> while the document is hidden.
  const paused = rules.filter(
    (r) =>
      /html\[data-doc-hidden\]/.test(r.prelude) &&
      declarations(r.body).some(([p, v]) => p === 'animation-play-state' && v === 'paused'),
  )

  it.each([
    ['the braille spinner', /\.braille-spinner/],
    ['the working grid', /\.agent-working/],
  ])('%s', (_label, selector) => {
    expect(paused.some((r) => selector.test(r.prelude))).toBe(true)
  })
})
