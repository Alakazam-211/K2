// Settings uses K2's square controls. A native <input type="checkbox|radio">
// paints the browser's rounded control, so this source walk fails with
// file:line on any that is not SquareCheckbox / SquareRadio. The only
// exception is a custom-drawn square: an `sr-only` peer input whose visible
// box is a sibling span.
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { describe, expect, it } from 'vitest'

function tsxFiles(dir: string): string[] {
  const out: string[] = []
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) out.push(...tsxFiles(p))
    else if (name.endsWith('.tsx') && !name.includes('.test.')) out.push(p)
  }
  return out
}

const ROOT = __dirname
const FILES = tsxFiles(ROOT)

/** Every native checkbox/radio in `src`, as `file:line native <type>`. */
function nativeChoiceInputs(file: string, src: string): string[] {
  const hits: string[] = []
  // Up to the self-closing `/>`: a bare `>` also ends `=>` in handlers.
  const re = /<input\b[\s\S]*?\/>/g
  for (const m of src.matchAll(re)) {
    const type = /\btype=["'{]+(checkbox|radio)\b/.exec(m[0])
    if (!type) continue
    if (/\bsr-only\b/.test(m[0])) continue
    const line = src.slice(0, m.index).split('\n').length
    hits.push(`${file}:${line} native ${type[1]}`)
  }
  return hits
}

describe('Settings square controls', () => {
  it('walks the Settings sources', () => {
    expect(FILES.length).toBeGreaterThan(10)
    expect(FILES.some((f) => f.endsWith('WakeSchedulerSection.tsx'))).toBe(true)
  })

  it('the guard catches a native checkbox and radio, and skips sr-only squares', () => {
    const sample = [
      '<input type="checkbox" checked={a} />',
      '<input\n  type="radio"\n  name="x"\n/>',
      '<input type="checkbox" className="peer sr-only" />',
      '<SquareCheckbox checked={b} />',
    ].join('\n')
    expect(nativeChoiceInputs('sample.tsx', sample)).toEqual([
      'sample.tsx:1 native checkbox',
      'sample.tsx:2 native radio',
    ])
  })

  it('has no native rounded checkbox or radio', () => {
    const hits = FILES.flatMap((f) => nativeChoiceInputs(relative(ROOT, f), readFileSync(f, 'utf8')))
    expect(hits).toEqual([])
  })

  it('Wake Scheduler "Also on battery" is a SquareCheckbox', () => {
    const src = readFileSync(join(ROOT, 'sections', 'WakeSchedulerSection.tsx'), 'utf8')
    const at = src.indexOf('Also on battery')
    expect(at).toBeGreaterThan(-1)
    const before = src.slice(0, at)
    const lastSquare = before.lastIndexOf('<SquareCheckbox')
    expect(lastSquare).toBeGreaterThan(-1)
    expect(before.slice(lastSquare)).toContain('handleSetWake(true, e.target.checked)')
    expect(before.lastIndexOf('<input')).toBeLessThan(lastSquare)
  })
})

// Rosson 2026-10-18: a raw radio anywhere in the app's components paints
// the browser's round control. Every `type="radio"` must be the shared
// SquareRadio (components/ui/SquareRadio.tsx), the one file allowed to
// spell it. Tests are skipped; no sr-only exception for radios.
const COMPONENTS = join(__dirname, '..')

/** Every raw radio under `src/renderer/components`, as `file:line`. */
function rawRadios(): string[] {
  const allowed = join(COMPONENTS, 'ui', 'SquareRadio.tsx')
  const hits: string[] = []
  for (const f of tsxFiles(COMPONENTS)) {
    if (f === allowed) continue
    const src = readFileSync(f, 'utf8')
    for (const m of src.matchAll(/<input\b[\s\S]*?\/>/g)) {
      if (!/\btype=["'{]+radio\b/.test(m[0])) continue
      hits.push(`${relative(COMPONENTS, f)}:${src.slice(0, m.index).split('\n').length}`)
    }
  }
  return hits
}

describe('square radios across the app', () => {
  it('walks every component source, including the Owner account prompt and the Clone dialog', () => {
    const files = tsxFiles(COMPONENTS).map((f) => relative(COMPONENTS, f))
    expect(files).toEqual(expect.arrayContaining([join('OwnerAccountPrompt', 'OwnerAccountPrompt.tsx'), join('CloneToDialog', 'CloneOptionsBlocks.tsx')]))
  })

  it('has no raw type="radio" outside ui/SquareRadio.tsx', () => {
    expect(rawRadios()).toEqual([])
  })
})
