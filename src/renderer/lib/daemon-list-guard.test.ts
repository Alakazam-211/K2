// Daemon list-body ratchet (research-spread-not-iterable-crash-v1).
//
// `daemonCliGet<Row[]>(…)` is a type cast, not a check: an older or odd
// server can answer a list route with an object (`{}`, `{"error":…}`, a
// wrapped `{sessions:[…]}`). Spreading that in render crashes the whole
// window ("Spread syntax requires ...iterable[Symbol.iterator] to be a
// function"). This test walks the renderer source and fails, with
// file:line, when a list-typed `/cli/*` response reaches React or zustand
// state, or a spread, without going through `asArray` or an
// `Array.isArray` check.
//
// What counts as a list-typed response (the fetchers are `daemonCliGet`,
// `daemonCliPost`, `localDaemonCliPost`, and federation's `cliGet`/`cliPost`):
//   - the type argument itself is a list (`T[]`, `Array<T>`), or
//   - a field of the type argument is a list, when the type is an inline
//     object literal or an interface / type alias declared in the renderer.
// What counts as reaching state or a spread, for a value bound from the
// fetch (`const x = await fetch<…>(…)`, `const { f } = await …`, or a
// `.then((x) => …)` parameter), within the binding's enclosing block:
//   - `setFoo(x)` / `setFoo(x.f)` (a React setter, also `x.f ?? []`),
//   - `set({ key: x })` / `set({ x })` (a zustand store write),
//   - `...x` / `...x.f`,
//   - and `setFoo(await fetch<T[]>(…))` written in one expression.
// A sink is exempt when an `Array.isArray(x)` (or `x.f`) check guards it:
// it sits inside `if (Array.isArray(x) …) …`, or after an early
// `if (!Array.isArray(x)) return`. Wrapping the fetch or the setter argument
// in `asArray(…)` never matches. It does not follow values through returns,
// props, or types imported from outside the renderer.

import { describe, it, expect } from 'vitest'
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const RENDERER = join(dirname(fileURLToPath(import.meta.url)), '..')

function walk(dir: string, out: string[]): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name)
    if (e.isDirectory()) walk(p, out)
    else if (/\.tsx?$/.test(e.name)) out.push(p)
  }
  return out
}

const SOURCE_FILES = walk(RENDERER, [])
  .map((f) => relative(RENDERER, f).split(sep).join('/'))
  .filter((f) => !/\.(ms)?test\.tsx?$/.test(f) && !f.startsWith('test-utils/'))
  .sort()

/** Source with comments blanked to spaces, newlines kept so offsets map to
 *  the real line numbers. Same crude `//` rule as the room-boundary test
 *  (only after whitespace or line start, so `'http://…'` survives). */
function read(f: string): string {
  const blank = (m: string): string => m.replace(/[^\n]/g, ' ')
  return readFileSync(join(RENDERER, f), 'utf8')
    .replace(/(^|\s)(\/\*[\s\S]*?\*\/)/g, (_m, pre: string, c: string) => pre + blank(c))
    .replace(/(^|\s)(\/\/.*)$/gm, (_m, pre: string, c: string) => pre + blank(c))
}

const FETCH = /\b(?:daemonCliGet|daemonCliPost|localDaemonCliPost|cliGet|cliPost)\s*</g

/** Index just past the `close` that balances the `open` at `at`. */
function matchClose(src: string, at: number, open: string, close: string): number {
  let depth = 0
  for (let i = at; i < src.length; i++) {
    const c = src[i]
    if (c === open) depth++
    else if (c === close) {
      depth--
      if (depth === 0) return i + 1
    }
  }
  return src.length
}

/** Generic argument starting at `<` (index `at`), angle- and brace-balanced. */
function genericArg(src: string, at: number): { text: string; end: number } {
  let depth = 0
  for (let i = at; i < src.length; i++) {
    const c = src[i]
    if (c === '<' || c === '{' || c === '(' || c === '[') depth++
    else if ((c === '>' && src[i - 1] !== '=') || c === '}' || c === ')' || c === ']') {
      depth--
      if (depth === 0) return { text: src.slice(at + 1, i).trim(), end: i + 1 }
    }
  }
  return { text: '', end: src.length }
}

const LIST_TYPE = /^(?:(?:Readonly)?Array<[\s\S]*>|ReadonlyArray<[\s\S]*>|[\s\S]*\[\])(?:\s*\|\s*(?:null|undefined))*$/
function isListType(t: string): boolean {
  return LIST_TYPE.test(t.trim())
}

/** Top-level `name?: Type` members of an object-type body (no braces). */
function listFields(body: string): string[] {
  const out: string[] = []
  let depth = 0
  let start = 0
  const parts: string[] = []
  for (let i = 0; i <= body.length; i++) {
    const c = body[i]
    if (c === '{' || c === '<' || c === '(' || c === '[') depth++
    else if ((c === '>' && body[i - 1] !== '=') || c === '}' || c === ')' || c === ']') depth--
    if (i === body.length || (depth === 0 && (c === ';' || c === ',' || c === '\n'))) {
      parts.push(body.slice(start, i))
      start = i + 1
    }
  }
  for (const p of parts) {
    const m = /^\s*(?:readonly\s+)?(\w+)\??\s*:\s*([\s\S]+?)\s*$/.exec(p)
    if (m && isListType(m[2])) out.push(m[1])
  }
  return out
}

/** Interface / object type alias bodies declared anywhere in the renderer. */
const TYPE_BODIES = new Map<string, Map<string, string>>()
for (const f of SOURCE_FILES) {
  const src = read(f)
  const re = /\b(?:interface\s+(\w+)(?:<[^>]*>)?(?:\s+extends\s+[^{]+)?\s*|type\s+(\w+)(?:<[^>]*>)?\s*=\s*)\{/g
  let m: RegExpExecArray | null
  while ((m = re.exec(src))) {
    const name = m[1] ?? m[2]
    const open = m.index + m[0].length - 1
    const body = src.slice(open + 1, matchClose(src, open, '{', '}') - 1)
    let byFile = TYPE_BODIES.get(name)
    if (!byFile) TYPE_BODIES.set(name, (byFile = new Map()))
    byFile.set(f, body)
  }
}

/** List fields of a response type: inline literal, or a named type (same
 *  file first, else the one renderer-wide definition when it is unique). */
function responseListFields(type: string, file: string): string[] {
  const t = type.trim()
  if (t.startsWith('{')) return listFields(t.slice(1, matchClose(t, 0, '{', '}') - 1))
  const name = /^(?:Partial<)?(\w+)/.exec(t)?.[1]
  if (!name) return []
  const defs = TYPE_BODIES.get(name)
  if (!defs) return []
  const body = defs.get(file) ?? (defs.size === 1 ? [...defs.values()][0] : undefined)
  return body === undefined ? [] : listFields(body)
}

const esc = (s: string): string => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

/** Text of the block enclosing `at` (from `at` to the brace that closes it). */
function enclosingBlockEnd(src: string, at: number): number {
  let depth = 0
  for (let i = at; i < src.length; i++) {
    const c = src[i]
    if (c === '{') depth++
    else if (c === '}') {
      depth--
      if (depth < 0) return i
    }
  }
  return src.length
}

/** Ranges of `region` an `Array.isArray(expr)` check actually guards:
 *  the body of `if (Array.isArray(expr) …)` (no `||`), or everything after
 *  `if (!Array.isArray(expr)) return|throw|continue|break`. A check that
 *  only runs after the sink (the ChatHistory N5 shape) guards nothing. */
function guardedRanges(region: string, e: string): Array<[number, number]> {
  const out: Array<[number, number]> = []
  const isArr = `Array\\.isArray\\(\\s*${e}\\s*\\)`
  const ifRe = /\bif\s*\(/g
  let m: RegExpExecArray | null
  while ((m = ifRe.exec(region))) {
    const open = m.index + m[0].length - 1
    const close = matchClose(region, open, '(', ')')
    const cond = region.slice(open + 1, close - 1)
    const rest = region.slice(close)
    if (new RegExp(`!\\s*${isArr}`).test(cond)) {
      if (/^\s*\{?\s*(?:return|throw|continue|break)\b/.test(rest)) out.push([close, region.length])
    } else if (new RegExp(isArr).test(cond) && !cond.includes('||')) {
      const body = /^\s*\{/.exec(rest)
      if (body) {
        const start = close + body[0].length - 1
        out.push([start, matchClose(region, start, '{', '}')])
      } else {
        const stmt = /^[^;\n]*/.exec(rest)
        out.push([close, close + (stmt ? stmt[0].length : 0)])
      }
    }
  }
  return out
}

/** Unguarded sinks of `expr` (a variable or `var.field`) in `region`.
 *  `spread: false` checks setters only (a whole body may be spread when
 *  its list fields are then replaced with asArray, as fetchProjectGroupShow does). */
function sinks(region: string, expr: string, spread = true): number[] {
  const e = esc(expr).replace('\\.', '\\??\\.')
  const guarded = guardedRanges(region, e)
  const patterns = [
    new RegExp(`\\bset[A-Z]\\w*\\(\\s*${e}\\s*(?:\\?\\?[^,)]*)?[,)]`, 'g'),
    // `set({ a: x })`, `set({ a, x })`, and shorthand first: `set({ x, a })`.
    new RegExp(`\\bset\\(\\s*\\{(?:[^}]*?:\\s*${e}|(?:[^}]*?,)?\\s*${e})\\s*(?:\\?\\?[^,}]*)?[,}]`, 'g'),
    ...(spread ? [new RegExp(`\\.\\.\\.\\s*${e}\\b(?!\\s*\\??\\.)`, 'g')] : []),
  ]
  const hits: number[] = []
  for (const re of patterns) {
    let m: RegExpExecArray | null
    while ((m = re.exec(region))) {
      const at = m.index
      if (!guarded.some(([a, b]) => at >= a && at < b)) hits.push(at)
    }
  }
  return hits
}

function lineOf(src: string, at: number): number {
  return src.slice(0, at).split('\n').length
}

function scan(file: string): string[] {
  return scanSource(file, read(file))
}

function scanSource(file: string, src: string): string[] {
  const found = new Set<string>()
  const report = (at: number, what: string): void => {
    found.add(`${file}:${lineOf(src, at)}: ${what}`)
  }
  let m: RegExpExecArray | null
  FETCH.lastIndex = 0
  while ((m = FETCH.exec(src))) {
    const lt = m.index + m[0].length - 1
    const { text: type, end } = genericArg(src, lt)
    const list = isListType(type)
    const fields = list ? [] : responseListFields(type, file)
    if (!list && fields.length === 0) continue

    // Look back for how the result is bound.
    const before = src.slice(Math.max(0, m.index - 200), m.index)
    const direct = /\bset[A-Z]\w*\(\s*await\s+$|\bset\(\s*\{[^}]*:\s*await\s+$/.exec(before)
    if (direct && list) report(m.index, `setter fed straight from a ${type} fetch`)

    const bound = /\b(?:const|let)\s+(\w+|\{[^}]*\})\s*(?::[^=]+)?=\s*await\s+$/.exec(before)
    const names: Array<{ expr: string; at: number; whole?: boolean }> = []
    let regionStart = m.index
    let regionEnd = 0
    if (bound) {
      regionEnd = enclosingBlockEnd(src, m.index)
      if (bound[1].startsWith('{')) {
        for (const part of bound[1].slice(1, -1).split(',')) {
          const [field, alias] = part.split(':').map((s) => s.trim())
          if (field && fields.includes(field)) names.push({ expr: alias || field, at: m.index })
        }
      } else if (list) {
        names.push({ expr: bound[1], at: m.index, whole: true })
      } else {
        for (const f of fields) names.push({ expr: `${bound[1]}.${f}`, at: m.index })
        // The whole body into state carries its list fields in unchecked
        // (0.43.0: `set({ doc })` with `SubscriptionDoc.harnesses`).
        names.push({ expr: bound[1], at: m.index, whole: true })
      }
    } else {
      // `fetch<…>(…).then((x) => …)`: the callback is the region.
      const callEnd = src[end] === '(' ? matchClose(src, end, '(', ')') : end
      const then = /^\s*\.then\(\s*(?:async\s*)?\(?\s*(\w+)/.exec(src.slice(callEnd))
      if (then) {
        const thenOpen = src.indexOf('(', callEnd)
        regionStart = thenOpen
        regionEnd = matchClose(src, thenOpen, '(', ')')
        if (list) names.push({ expr: then[1], at: thenOpen })
        else for (const f of fields) names.push({ expr: `${then[1]}.${f}`, at: thenOpen })
      }
    }
    const region = src.slice(regionStart, regionEnd)
    for (const n of names) {
      for (const hit of sinks(region, n.expr, !n.whole)) {
        report(regionStart + hit, `\`${n.expr}\` from a \`${type.replace(/\s+/g, ' ')}\` fetch reaches state or a spread without asArray / Array.isArray`)
      }
    }
  }
  return [...found]
}

describe('daemon list bodies never reach state or a spread unchecked', () => {
  it('every list-typed /cli/* response goes through asArray or Array.isArray first', () => {
    const offenders = SOURCE_FILES.flatMap(scan)
    expect(offenders).toEqual([])
  })

  // The scanner itself: each shape it is meant to catch, and the guarded
  // forms it must leave alone. If these drift, the ratchet above is blind.
  it('flags the shapes it claims to and passes the guarded forms', () => {
    const probe = (body: string): number => {
      const src = body
      let n = 0
      FETCH.lastIndex = 0
      let m: RegExpExecArray | null
      while ((m = FETCH.exec(src))) {
        const lt = m.index + m[0].length - 1
        const { text } = genericArg(src, lt)
        if (isListType(text)) n++
      }
      return n
    }
    expect(probe('daemonCliGet<Row[]>(s, "x")')).toBe(1)
    expect(probe('daemonCliGet<Array<{ a: string }>>(s, "x")')).toBe(1)
    expect(probe('daemonCliGet<{ rows: Row[] }>(s, "x")')).toBe(0)
    expect(listFields(' rows: Row[]\n next_cursor: number | null\n days?: Day[] | null ')).toEqual(['rows', 'days'])
    expect(sinks('{ setRows(list) }', 'list')).toHaveLength(1)
    expect(sinks('{ set({ presets: result }) }', 'result')).toHaveLength(1)
    expect(sinks('{ return [...prev, ...page.rows] }', 'page.rows')).toHaveLength(1)
    expect(sinks('{ setRows(page.rows ?? []) }', 'page.rows')).toHaveLength(1)
    expect(sinks('{ setRows(asArray(list)) }', 'list')).toHaveLength(0)
    expect(sinks('{ if (!Array.isArray(list)) return; setRows(list) }', 'list')).toHaveLength(0)
    expect(sinks('{ if (Array.isArray(list)) { setRows(list) } }', 'list')).toHaveLength(0)
    expect(sinks('{ if (Array.isArray(list) && ok) setRows(list) }', 'list')).toHaveLength(0)
    // A check after the sink (or on another branch) does not guard it.
    expect(sinks('{ setRows(list); if (Array.isArray(list)) restamp(list) }', 'list')).toHaveLength(1)
    expect(sinks('{ if (Array.isArray(list) || x) setRows(list) }', 'list')).toHaveLength(1)
    expect(sinks('{ setRows(Array.isArray(list) ? list : []) }', 'list')).toHaveLength(0)
    expect(sinks('{ setCount(list.length) }', 'list')).toHaveLength(0)
    // A whole body with list fields into state (0.43.0 `set({ doc })`).
    expect(sinks('{ set({ doc, error: null }) }', 'doc', false)).toHaveLength(1)
    expect(sinks('{ setReport(report) }', 'report', false)).toHaveLength(1)
    expect(sinks('{ return { ...show, members: asArray(show.members) } }', 'show', false)).toHaveLength(0)
    const old0430 = [
      "const doc = await daemonCliGet<SubscriptionDoc>(primaryScope(), 'usage/subscriptions')",
      'if (epoch === loadEpoch) set({ doc, error: null })',
    ].join('\n')
    expect(scanSource('stores/subscription-usage.ts', `{ ${old0430} }`)).toHaveLength(1)
  })
})
