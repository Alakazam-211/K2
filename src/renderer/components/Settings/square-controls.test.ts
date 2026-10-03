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
