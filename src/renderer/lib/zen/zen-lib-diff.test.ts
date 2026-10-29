// @vitest-environment jsdom
//
// k2-diff@1 (Workbench B6, catalog v40): the real checked-in file runs in
// jsdom, as K2 loads it in a widget frame (and highlight.js@11 after it for
// the colour case). Diff correctness, hunks, the view's states, the tree,
// and the security rule: a file's text is only ever text.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { beforeAll, describe, expect, it, vi } from 'vitest'
import { ZEN_LIB_MANIFEST, zenLibResolve } from './zen-lib-loader'

const LIB_DIR = resolve(__dirname, '../../zen-lib')

function load(key: string): string {
  const at = key.lastIndexOf('@')
  const [id, version] = [key.slice(0, at), key.slice(at + 1)]
  const e = ZEN_LIB_MANIFEST.libs.find((l) => l.id === id && l.version === version)
  if (!e) throw new Error(`no row ${key}`)
  return e.files.map((f) => readFileSync(resolve(LIB_DIR, key, f.name), 'utf8')).join('\n;\n')
}

type Line = { type: 'ctx' | 'add' | 'del'; old: number | null; new: number | null; text: string; noNewline?: boolean }
type Side = { text: string | null; exists: boolean }
type Diff = {
  version: string
  lines(a: string | null, b: string | null, o?: { ignoreWhitespace?: boolean }): Line[] & { whole?: boolean }
  hunks(l: Line[], o?: { context?: number }): Array<{ oldStart: number; oldLines: number; newStart: number; newLines: number; lines: Line[] }>
  stats(l: Line[]): { additions: number; deletions: number }
  render(
    el: Element,
    d: { file: string; oldFile?: string | null; old: Side; new: Side; binary?: boolean; tooLarge?: boolean },
    o?: Record<string, unknown>,
  ): { destroy(): void; lines: Line[] }
  tree(
    el: Element,
    files: Array<{ file: string; status: string; additions: number | null; deletions: number | null }>,
    o?: { selected?: string; onSelect?(f: string): void },
  ): { select(f: string): void; destroy(): void }
}

let K2Diff: Diff

beforeAll(() => {
  const r = zenLibResolve(['k2-diff@1'])
  expect(r.findings.filter((f) => f.error)).toEqual([])
  expect(r.keys.map(String)).toEqual(['k2-diff@1.0.0'])
  ;(0, eval)(load('k2-diff@1.0.0'))
  K2Diff = (window as unknown as { K2Diff: Diff }).K2Diff
  expect(K2Diff.version).toBe('1.0.0')
})

const kinds = (ls: Line[]): string => ls.map((l) => (l.type === 'ctx' ? ' ' : l.type === 'add' ? '+' : '-') + l.text).join('|')

describe('k2-diff@1: lines', () => {
  it('marks added, deleted and unchanged lines with old and new numbers', () => {
    const ls = K2Diff.lines('a\nb\nc\n', 'a\nB\nc\nd\n')
    expect(kinds(ls)).toBe(' a|-b|+B| c|+d')
    expect(ls.map((l) => [l.old, l.new])).toEqual([
      [1, 1],
      [2, null],
      [null, 2],
      [3, 3],
      [null, 4],
    ])
    expect(K2Diff.stats(ls)).toEqual({ additions: 2, deletions: 1 })
  })

  it('handles empty and missing sides', () => {
    expect(kinds(K2Diff.lines('', 'x\ny\n'))).toBe('+x|+y')
    expect(kinds(K2Diff.lines('x\n', null))).toBe('-x')
    expect(K2Diff.lines('', '')).toEqual([])
    expect(K2Diff.lines('same\n', 'same\n').every((l) => l.type === 'ctx')).toBe(true)
  })

  it('a last line losing or gaining its newline is a change, marked', () => {
    const ls = K2Diff.lines('a\nb', 'a\nb\n')
    expect(kinds(ls)).toBe(' a|-b|+b')
    expect(ls[1].noNewline).toBe(true)
    expect(ls[2].noNewline).toBeUndefined()
  })

  it('ignoreWhitespace hides indentation-only changes', () => {
    expect(K2Diff.stats(K2Diff.lines('if (x) {\n  y()\n}\n', 'if (x) {\n    y()\n}\n', { ignoreWhitespace: true }))).toEqual({
      additions: 0,
      deletions: 0,
    })
    expect(K2Diff.stats(K2Diff.lines('  y()\n', '    y()\n'))).toEqual({ additions: 1, deletions: 1 })
  })

  it('finds a minimal edit inside a big file quickly', () => {
    const a = Array.from({ length: 20_000 }, (_, i) => `line ${i}`)
    const b = a.slice()
    b[10_000] = 'changed'
    b.splice(15_000, 0, 'inserted')
    const t = performance.now()
    const ls = K2Diff.lines(a.join('\n') + '\n', b.join('\n') + '\n')
    expect(performance.now() - t).toBeLessThan(2_000)
    expect(K2Diff.stats(ls)).toEqual({ additions: 2, deletions: 1 })
    expect(ls.whole).toBeUndefined()
  })

  it('too many edits to compare show the middle as replaced, in bounded time', () => {
    const a = Array.from({ length: 8_000 }, (_, i) => `a${i}`).join('\n')
    const b = Array.from({ length: 8_000 }, (_, i) => `b${i}`).join('\n')
    const t = performance.now()
    const ls = K2Diff.lines(a, b)
    expect(performance.now() - t).toBeLessThan(5_000)
    expect(ls.whole).toBe(true)
    expect(K2Diff.stats(ls)).toEqual({ additions: 8_000, deletions: 8_000 })
  })
})

describe('k2-diff@1: hunks', () => {
  const a = Array.from({ length: 30 }, (_, i) => `l${i + 1}`)
  it('keeps context lines around each change and merges near ones', () => {
    const b = a.slice()
    b[4] = 'X'
    b[6] = 'Y'
    b[24] = 'Z'
    const hs = K2Diff.hunks(K2Diff.lines(a.join('\n'), b.join('\n')), { context: 2 })
    expect(hs.length).toBe(2)
    expect([hs[0].oldStart, hs[0].oldLines, hs[0].newStart, hs[0].newLines]).toEqual([3, 7, 3, 7])
    expect([hs[1].oldStart, hs[1].oldLines]).toEqual([23, 5])
    expect(hs[1].lines.filter((l) => l.type !== 'ctx').map((l) => l.text)).toEqual(['l25', 'Z'])
  })
  it('defaults to 3 lines of context, none for an unchanged file', () => {
    const b = a.slice()
    b[14] = 'mid'
    const [h] = K2Diff.hunks(K2Diff.lines(a.join('\n'), b.join('\n')))
    expect(h.oldStart).toBe(12)
    expect(h.oldLines).toBe(7)
    expect(K2Diff.hunks(K2Diff.lines('x', 'x'))).toEqual([])
  })
})

const side = (text: string | null, exists = true): Side => ({ text, exists })

describe('k2-diff@1: render', () => {
  it('draws a unified diff with numbers, signs and a fold for long unchanged runs', () => {
    const a = Array.from({ length: 40 }, (_, i) => `row ${i + 1}`)
    const b = a.slice()
    b[19] = 'row twenty'
    const el = document.createElement('div')
    const v = K2Diff.render(el, { file: 'notes.txt', old: side(a.join('\n') + '\n'), new: side(b.join('\n') + '\n') })
    expect(el.querySelector('.k2d-file')?.textContent).toBe('notes.txt')
    expect(el.querySelectorAll('.k2d-row.k2d-add').length).toBe(1)
    expect(el.querySelectorAll('.k2d-row.k2d-del').length).toBe(1)
    expect(el.querySelectorAll('.k2d-row').length).toBe(8)
    const gaps = Array.from(el.querySelectorAll('button.k2d-gap')).map((b) => b.textContent)
    expect(gaps).toEqual(['Expand 16 lines', 'Expand 17 lines'])
    ;(el.querySelector('button.k2d-gap') as HTMLButtonElement).click()
    expect(el.querySelectorAll('.k2d-row').length).toBe(24)
    expect(el.querySelectorAll('button.k2d-gap').length).toBe(1)
    v.destroy()
    expect(el.childNodes.length).toBe(0)
  })

  it('split view pairs a deleted line with its replacement on one row', () => {
    const el = document.createElement('div')
    K2Diff.render(el, { file: 'a.txt', old: side('one\ntwo\n'), new: side('one\nTWO\nthree\n') }, { view: 'split' })
    expect(el.querySelector('.k2d')?.classList.contains('k2d-split')).toBe(true)
    const rows = Array.from(el.querySelectorAll('.k2d-row'))
    expect(rows.length).toBe(3)
    const cells = (r: Element): string[] => Array.from(r.querySelectorAll('.k2d-c')).map((c) => c.textContent ?? '')
    expect(cells(rows[1])).toEqual(['two', 'TWO'])
    expect(cells(rows[2])).toEqual(['', 'three'])
  })

  it('says binary, too large, new, deleted and no-change in plain words', () => {
    const msg = (d: Parameters<Diff['render']>[1], o?: Record<string, unknown>): string => {
      const el = document.createElement('div')
      K2Diff.render(el, d, o)
      return el.textContent ?? ''
    }
    expect(msg({ file: 'x.png', old: side(null), new: side(null), binary: true })).toContain('Binary file')
    expect(msg({ file: 'big.sql', old: side(null), new: side(null), tooLarge: true })).toContain('too large to show here')
    expect(msg({ file: 'n.ts', old: side(null, false), new: side('a\n') })).toContain('New file')
    expect(msg({ file: 'gone.ts', old: side('a\n'), new: side(null, false) })).toContain('Deleted')
    expect(msg({ file: 's.ts', old: side('a\n'), new: side('a\n') })).toContain('No changes.')
    expect(msg({ file: 's.ts', old: side(' a\n'), new: side('a\n') }, { ignoreWhitespace: true })).toContain('Only whitespace changed.')
    expect(msg({ file: 'e.ts', old: side('a'), new: side('b') })).toContain('No newline at end of file')
  })

  it('a line that looks like HTML stays text: nothing in it runs', () => {
    const evil = '<img src=x onerror="window.__pwned=1"><script>window.__pwned=2</script>'
    const el = document.createElement('div')
    K2Diff.render(el, { file: '<b>x</b>.html', old: side('safe\n'), new: side(evil + '\n') })
    expect(el.querySelector('img, script, b')).toBeNull()
    expect(Array.from(el.querySelectorAll('*')).some((n) => Array.from(n.attributes).some((a) => /^on/i.test(a.name)))).toBe(false)
    expect(el.querySelector('.k2d-row.k2d-add .k2d-c')?.textContent).toBe(evil)
    expect(el.querySelector('.k2d-file')?.textContent).toBe('<b>x</b>.html')
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined()
  })

  it('onLine adds a note button per line that names the side and line', () => {
    const onLine = vi.fn()
    const el = document.createElement('div')
    K2Diff.render(el, { file: 'a.ts', old: side('keep\nold\n'), new: side('keep\nnew\n') }, { onLine })
    const btns = Array.from(el.querySelectorAll('button.k2d-addnote')) as HTMLButtonElement[]
    expect(btns.map((b) => b.getAttribute('aria-label'))).toEqual(['Add a note on line 1', 'Add a note on line 2', 'Add a note on line 2'])
    btns[1].click()
    btns[2].click()
    expect(onLine.mock.calls.map((c) => c[0])).toEqual([
      { file: 'a.ts', side: 'old', line: 2, text: 'old' },
      { file: 'a.ts', side: 'new', line: 2, text: 'new' },
    ])
  })

  it('colours code with highlight.js when the widget loads it, still as text nodes', () => {
    ;(0, eval)(load(zenLibResolve(['highlight.js@11']).keys.map(String).find((k) => k.startsWith('highlight.js@')) as string))
    const el = document.createElement('div')
    K2Diff.render(el, { file: 'm.ts', old: side('const a = 1\n'), new: side('const a = "<b>2</b>" // note\n') })
    const add = el.querySelector('.k2d-row.k2d-add .k2d-c') as HTMLElement
    expect(add.textContent).toBe('const a = "<b>2</b>" // note')
    expect(add.querySelector('.hljs-keyword')?.textContent).toBe('const')
    expect(add.querySelector('.hljs-string')?.textContent).toBe('"<b>2</b>"')
    expect(add.querySelector('b')).toBeNull()
    const plain = document.createElement('div')
    K2Diff.render(plain, { file: 'm.ts', old: side('a\n'), new: side('const b\n') }, { highlight: false })
    expect(plain.querySelector('[class^="hljs-"]')).toBeNull()
  })
})

describe('k2-diff@1: tree', () => {
  const files = [
    { file: 'src/lib/a.ts', status: 'modified', additions: 3, deletions: 1 },
    { file: 'src/lib/b.ts', status: 'added', additions: 9, deletions: 0 },
    { file: 'README.md', status: 'deleted', additions: 0, deletions: 4 },
    { file: 'docs/x <img onerror=1>.md', status: 'untracked', additions: null, deletions: null },
  ]
  it('folds one-child folder chains, shows status letters and counts, selects', () => {
    const onSelect = vi.fn()
    const el = document.createElement('div')
    const t = K2Diff.tree(el, files, { selected: 'README.md', onSelect })
    const names = Array.from(el.querySelectorAll('.k2d-tn')).map((n) => n.textContent)
    expect(names).toEqual(['docs', 'x <img onerror=1>.md', 'src/lib', 'a.ts', 'b.ts', 'README.md'])
    expect(el.querySelector('img')).toBeNull()
    const sel = el.querySelector('[aria-selected="true"]')
    expect(sel?.textContent).toContain('README.md')
    expect(sel?.querySelector('.k2d-tst')?.textContent).toBe('D')
    const a = Array.from(el.querySelectorAll('button.k2d-t')).find((b) => b.textContent?.includes('a.ts')) as HTMLButtonElement
    expect(a.textContent).toContain('+3')
    expect(a.textContent).toContain('−1')
    a.click()
    expect(onSelect).toHaveBeenCalledWith('src/lib/a.ts')
    expect(a.getAttribute('aria-selected')).toBe('true')
    t.select('src/lib/b.ts')
    expect(a.getAttribute('aria-selected')).toBe('false')
    const folder = Array.from(el.querySelectorAll('button.k2d-t')).find((b) => b.textContent?.includes('src/lib')) as HTMLButtonElement
    folder.click()
    expect(folder.getAttribute('aria-expanded')).toBe('false')
    t.destroy()
    expect(el.childNodes.length).toBe(0)
  })
})

describe('k2-diff@1: shipped', () => {
  it('is small, K2 licensed, and in NOTICE', () => {
    const row = ZEN_LIB_MANIFEST.libs.find((l) => l.id === 'k2-diff')
    expect(row?.license).toBe('FSL-1.1-Apache-2.0')
    expect(row?.files[0].bytes).toBeLessThanOrEqual(25 * 1024)
    expect(readFileSync(resolve(__dirname, '../../../../NOTICE.md'), 'utf8')).toContain('(`k2-diff`) | 1.0.0 | FSL-1.1-Apache-2.0')
  })
})
