// The JavaScript side of the vectors. `node vectors/check.mjs` checks the `canon` and `rev` layers with
// no code from this repo's crate: canonical bytes, their sha256, revisions, every JSON refusal, and
// the ids a refusal names. A guard computed in
// JavaScript and in Rust must match, byte for byte. It also shows what a JavaScript reader has to do
// that JSON.parse won't: number literals and duplicate keys need checking on the text.
import { readFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const dir = dirname(fileURLToPath(import.meta.url))
const MAX_DIGITS = 17
const MAX_LITERAL = 64

class Refused extends Error {
  constructor(code) { super(code); this.code = code }
}

// A decimal literal as [negative, significant digits, exponent], with value 0.DIGITS x 10^exponent;
// null for zero. Significant digits run from the first non-zero digit to the last.
function decimal(text) {
  const m = /^(-?)(\d+)(?:\.(\d+))?(?:[eE]([+-]?\d+))?$/.exec(text)
  const joined = m[2] + (m[3] || '')
  const kept = joined.replace(/^0+/, '')
  const digits = kept.replace(/0+$/, '')
  if (!digits) return null
  return [m[1] === '-', digits, BigInt(m[4] || 0) + BigInt(m[2].length) - BigInt(joined.length - kept.length)]
}

// The number rule. Magnitude is judged on the literal, since 9007199254740993 parses to 2^53.
function numberValue(lit) {
  if (lit.length > MAX_LITERAL) throw new Refused('limit_number_literal')
  const d = decimal(lit)
  if (d === null) return 0
  const [, digits, exp] = d
  if (exp > 16n || (exp === 16n && digits > '9007199254740992')) {
    throw new Refused('json_number_range')
  }
  if (digits.length > MAX_DIGITS) throw new Refused('json_number_inexact')
  const v = Number(lit)
  if (v === 0) throw new Refused('json_number_inexact')
  return v
}

// A strict parser returning canonical text directly.
function canonical(text, limits) {
  let i = 0
  let depth = 0
  let values = 0
  const syntax = () => new Refused('json_syntax')
  const ws = () => { while (' \t\n\r'.includes(text[i]) && i < text.length) i++ }
  function string() {
    i++
    let out = ''
    for (;;) {
      if (i >= text.length) throw syntax()
      const c = text[i]
      if (c === '"') { i++; return out }
      if (c < ' ') throw syntax()
      if (c !== '\\') { out += c; i++; continue }
      i++
      const e = text[i++]
      if (e === 'u') {
        const hex4 = () => {
          const h = text.slice(i, i + 4)
          if (!/^[0-9a-fA-F]{4}$/.test(h)) throw syntax()
          i += 4
          return parseInt(h, 16)
        }
        const u = hex4()
        if (u >= 0xDC00 && u <= 0xDFFF) throw new Refused('json_surrogate')
        if (u >= 0xD800 && u <= 0xDBFF) {
          if (text[i] !== '\\' || text[i + 1] !== 'u') throw new Refused('json_surrogate')
          i += 2
          const low = hex4()
          if (low < 0xDC00 || low > 0xDFFF) throw new Refused('json_surrogate')
          out += String.fromCharCode(u, low)
        } else out += String.fromCharCode(u)
      } else {
        const map = { '"': '"', '\\': '\\', '/': '/', b: '\b', f: '\f', n: '\n', r: '\r', t: '\t' }
        if (!(e in map)) throw syntax()
        out += map[e]
      }
    }
  }
  function value() {
    if (++values > (limits.values ?? 20000000)) throw new Refused('limit_values')
    const c = text[i]
    if (c === '{') {
      if (++depth > (limits.depth ?? 64)) throw new Refused('limit_depth')
      i++
      const members = new Map()
      ws()
      if (text[i] === '}') { i++; depth--; return '{}' }
      for (;;) {
        ws()
        if (text[i] !== '"') throw syntax()
        const key = string()
        ws()
        if (text[i++] !== ':') throw syntax()
        ws()
        const v = value()
        if (members.has(key)) throw new Refused('json_duplicate_key')
        members.set(key, v)
        ws()
        const n = text[i++]
        if (n === '}') break
        if (n !== ',') throw syntax()
      }
      depth--
      // JavaScript's default sort is by UTF-16 code units, which is what RFC 8785 asks for.
      const keys = [...members.keys()].sort()
      return '{' + keys.map((k) => JSON.stringify(k) + ':' + members.get(k)).join(',') + '}'
    }
    if (c === '[') {
      if (++depth > (limits.depth ?? 64)) throw new Refused('limit_depth')
      i++
      const items = []
      ws()
      if (text[i] === ']') { i++; depth--; return '[]' }
      for (;;) {
        ws()
        items.push(value())
        ws()
        const n = text[i++]
        if (n === ']') break
        if (n !== ',') throw syntax()
      }
      depth--
      return '[' + items.join(',') + ']'
    }
    if (c === '"') return JSON.stringify(string())
    for (const w of ['true', 'false', 'null']) {
      if (text.startsWith(w, i)) { i += w.length; return w }
    }
    const m = /^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/.exec(text.slice(i, i + 4096))
    if (!m) throw syntax()
    i += m[0].length
    if (/^\d/.test(text.slice(i, i + 1))) throw syntax()
    return String(numberValue(m[0]))
  }
  ws()
  const out = value()
  ws()
  if (i !== text.length) throw syntax()
  return out
}

function run(bytes, limits) {
  let text
  try {
    // ignoreBOM keeps a byte order mark in the text, where the parser refuses it.
    text = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes)
  } catch { throw new Refused('json_utf8') }
  return canonical(text, limits)
}

// Canonical text of a value parsed from canonical text, where numbers are already exact.
function canonValue(v) {
  if (Array.isArray(v)) return '[' + v.map(canonValue).join(',') + ']'
  if (v !== null && typeof v === 'object') {
    return '{' + Object.keys(v).sort().map((k) => JSON.stringify(k) + ':' + canonValue(v[k])).join(',') + '}'
  }
  return JSON.stringify(v)
}
const sha = (text) => createHash('sha256').update(Buffer.from(text, 'utf8')).digest('hex')

function checkRevs(c, input) {
  const board = JSON.parse(run(input, {}))
  const seen = Object.keys(c.revs)
  if (seen.length !== board.objects.length) return `${c.name}: revs doesn't list every object`
  for (const o of board.objects) {
    const got = sha(canonValue(o))
    if (got !== c.revs[o.id]) return `${c.name}: rev of ${o.id} is ${got}, wanted ${c.revs[o.id]}`
  }
  const got = sha(canonValue(board.settings))
  if (got !== c.settings_rev) return `${c.name}: settings_rev is ${got}, wanted ${c.settings_rev}`
  return null
}

// Every id that breaks the id rule, once each, in the order of the canonical form, capped.
const ID = /^[0-9a-z._-]{1,24}$/
function badIds(board) {
  const seen = new Set()
  const note = (v) => { if (typeof v === 'string' && !ID.test(v)) seen.add(v) }
  const sorted = (o) => Object.keys(o).sort()
  const items = (list, each) => { if (Array.isArray(list)) list.forEach((x) => { if (x && typeof x === 'object') each(x) }) }
  if (Array.isArray(board.frames)) board.frames.forEach(note)
  items(board.objects, (o) => {
    for (const k of sorted(o)) {
      const v = o[k]
      if (k === 'id' || k === 'group' || k === 'frame') note(v)
      else if ((k === 'from' || k === 'to') && o.type === 'connector') { if (v) note(v.id) }
      else if (k === 'parent' && o.type === 'mind') note(v)
      else if (k === 'target' && o.type === 'comment') note(v)
      else if (k === 'checks' && (o.type === 'card' || o.type === 'text')) items(v, (x) => note(x.id))
      else if (k === 'columns' && o.type === 'table') items(v, (x) => note(x.id))
      else if (k === 'rows' && o.type === 'table') items(v, (x) => { if (x.cells) sorted(x.cells).forEach(note); note(x.id) })
      else if (k === 'replies' && o.type === 'comment') items(v, (x) => { note(x.by); note(x.id) })
      else if (k === 'reactions' && v) sorted(v).forEach((e) => { if (Array.isArray(v[e])) v[e].forEach(note) })
      else if (k === 'votes' && v) sorted(v).forEach(note)
      else if (k === 'created' && v) note(v.by)
    }
  })
  if (board.people) sorted(board.people).forEach(note)
  const shown = (s) => { const c = [...s]; return c.length > 64 ? c.slice(0, 64).join('') + '\u2026' : s }
  const all = [...seen]
  return { ids: all.slice(0, 1000).map(shown), total: all.length, truncated: all.length > 1000 }
}

const index = JSON.parse(readFileSync(join(dir, 'index.json'), 'utf8'))
let passed = 0
const failed = []
let skipped = 0
for (const c of index.cases) {
  if (c.bad_ids) {
    const got = badIds(JSON.parse(readFileSync(join(dir, c.input), 'utf8')))
    const want = JSON.parse(readFileSync(join(dir, c.bad_ids), 'utf8'))
    if (JSON.stringify(got) === JSON.stringify({ ids: want.ids, total: want.total, truncated: want.truncated })) passed++
    else failed.push(`${c.name}: bad ids differ: ${JSON.stringify(got).slice(0, 300)}`)
    continue
  }
  if (c.layer !== 'canon' && c.layer !== 'rev') { skipped++; continue }
  const input = readFileSync(join(dir, c.input))
  if (c.layer === 'rev') {
    const problem = checkRevs(c, input)
    if (problem) failed.push(problem)
    else passed++
    continue
  }
  try {
    const got = run(input, c.limits || {})
    if (c.refusal) { failed.push(`${c.name}: wanted ${c.refusal}, accepted as ${got}`); continue }
    const want = readFileSync(join(dir, c.canonical))
    const bytes = Buffer.from(got, 'utf8')
    if (!bytes.equals(want)) { failed.push(`${c.name}: canonical bytes differ: ${got}`); continue }
    const hash = createHash('sha256').update(bytes).digest('hex')
    if (hash !== c.sha256) { failed.push(`${c.name}: sha256 ${hash}, wanted ${c.sha256}`); continue }
    passed++
  } catch (e) {
    if (!(e instanceof Refused)) throw e
    if (c.refusal === e.code) passed++
    else failed.push(`${c.name}: refused ${e.code}, wanted ${c.refusal ?? 'acceptance'}`)
  }
}
for (const f of failed) console.log('FAILED ' + f)
console.log(`check.mjs: passed=${passed} failed=${failed.length} other-layers=${skipped}`)
process.exit(failed.length ? 1 : 0)
