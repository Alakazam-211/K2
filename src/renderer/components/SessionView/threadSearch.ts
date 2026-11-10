// Searching a Thread's messages (Thread view's search object, beside the bell).
//
// The words of every message the client holds are searched (the pane loads
// the rest of the history when a search starts: `useOverlayThread.loadAll`).
// A message is searched as it reads, not as it was written: Markdown's marks,
// link targets and fences don't count (`markdownSearchText`). Matches are
// case-insensitive, never overlap, and are numbered from the newest: "1 of
// 12" is the newest match, and the next one (Enter, the up button) is older.
//
// Highlights never touch the message's DOM: they are CSS Custom Highlights
// (`::highlight(k2-thread-search)` in globals.css) over ranges of the drawn
// text, so Markdown, links, code and React's own nodes are left exactly as
// they are. Where the API is missing nothing is painted; the count and the
// scroll still work.
//
// The search object (header) and the Thread (pane) are siblings in one
// pane: they share state through the pane's floats host element.

import { useSyncExternalStore } from 'react'
import { THREAD_FLOATS_HOST_ATTR } from './threadFloats'

// ── Text ────────────────────────────────────────────────────────────────

const ENTITIES: Record<string, string> = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ', '#39': "'" }

function decodeEntities(s: string): string {
  return s.replace(/&(amp|lt|gt|quot|apos|nbsp|#39);/g, (_, name: string) => ENTITIES[name] ?? '')
}

/** Inline Markdown as it reads: code spans literal, marks and targets gone. */
function inlineSearchText(line: string): string {
  let out = ''
  // Code spans first: what's inside is literal.
  const parts = line.split(/(`+)([^`]|[^`][\s\S]*?[^`])\1(?!`)/)
  for (let i = 0; i < parts.length; i++) {
    const kind = i % 3
    if (kind === 1) continue
    if (kind === 2) {
      out += parts[i].length > 2 && parts[i].startsWith(' ') && parts[i].endsWith(' ') ? parts[i].slice(1, -1) : parts[i]
      continue
    }
    let s = parts[i]
    s = s.replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    s = s.replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    s = s.replace(/\[([^\]]+)\]\[[^\]]*\]/g, '$1')
    s = s.replace(/<((?:https?|mailto|ftp):[^>\s]+)>/g, '$1')
    s = s.replace(/(\*\*\*|___)(?=\S)([\s\S]*?\S)\1/g, '$2')
    s = s.replace(/(\*\*|__)(?=\S)([\s\S]*?\S)\1/g, '$2')
    s = s.replace(/~~(?=\S)([\s\S]*?\S)~~/g, '$1')
    s = s.replace(/(^|[^\w*])\*(?=[^\s*])([^*]*?[^\s*])?\*(?![\w*])/g, (_, pre: string, body?: string) => pre + (body ?? ''))
    s = s.replace(/(^|[^\w_])_(?=[^\s_])([^_]*?[^\s_])?_(?![\w_])/g, (_, pre: string, body?: string) => pre + (body ?? ''))
    s = s.replace(/\\([!-/:-@[-`{-~])/g, '$1')
    out += decodeEntities(s)
  }
  return out
}

/** One line outside a code block, without its block marks. */
function blockSearchText(line: string): string {
  let s = line
  // Quotes, then the list item or heading inside them.
  while (/^ {0,3}>/.test(s)) s = s.replace(/^ {0,3}> ?/, '')
  if (/^ {0,3}([-*_])( *\1){2,} *$/.test(s)) return ''
  const heading = /^ {0,3}#{1,6}(?: +|$)/.exec(s)
  if (heading) s = s.slice(heading[0].length).replace(/(^| +)#+ *$/, '')
  s = s.replace(/^\s*(?:[-*+]|\d{1,9}[.)])\s+/, '')
  s = s.replace(/^\[[ xX]\]\s+/, '')
  // A table: the divider row reads as nothing, cells as separate blocks.
  if (/^\s*\|/.test(s)) {
    if (/^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/.test(s)) return ''
    return s
      .replace(/^\s*\|/, '')
      .replace(/\|\s*$/, '')
      .split(/(?<!\\)\|/)
      .map((cell) => inlineSearchText(cell.trim()))
      .join('\n')
  }
  return inlineSearchText(s)
}

/** A message's Markdown as it reads in the bubble: the text the search
 *  counts. Blocks (lines, cells) are separated by a newline, which a query
 *  (one line) never matches across. */
export function markdownSearchText(md: string): string {
  const out: string[] = []
  let fence: { char: string; len: number } | null = null
  for (const line of md.replace(/\r\n?/g, '\n').split('\n')) {
    const m = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line)
    if (fence) {
      if (m && m[1][0] === fence.char && m[1].length >= fence.len && m[2].trim() === '') fence = null
      else out.push(line)
      continue
    }
    if (m && !(m[1][0] === '`' && m[2].includes('`'))) {
      fence = { char: m[1][0], len: m[1].length }
      continue
    }
    out.push(blockSearchText(line))
  }
  return out.join('\n')
}

/** Case folded, one UTF-16 unit for one, so offsets map back. */
export function foldSearchText(s: string): string {
  let out = ''
  for (const ch of s) {
    if (ch === ' ') {
      out += ' '
      continue
    }
    const low = ch.toLowerCase()
    out += low.length === ch.length ? low : ch
  }
  return out
}

/** A query worth searching: anything but blanks. */
export function searchableQuery(query: string): boolean {
  return query.trim().length > 0
}

/** Where `query` starts in `text`: case-insensitive, never overlapping. */
export function findOccurrences(text: string, query: string): number[] {
  if (!searchableQuery(query) || !text) return []
  const hay = foldSearchText(text)
  const needle = foldSearchText(query)
  const at: number[] = []
  let i = hay.indexOf(needle)
  while (i !== -1) {
    at.push(i)
    i = hay.indexOf(needle, i + needle.length)
  }
  return at
}

// ── Matches ─────────────────────────────────────────────────────────────

export interface ThreadMatch {
  /** The message's id (its row key). */
  id: string
  /** Which occurrence in the message, in reading order. */
  n: number
  key: string
}

export function threadMatchKey(id: string, n: number): string {
  return `${id}\u0000${n}`
}

/** Every match in `rows` (oldest first), newest first: the newest message's
 *  last occurrence is match 1. */
export function findThreadMatches(
  rows: readonly { id: string; text: string }[],
  query: string,
): ThreadMatch[] {
  const out: ThreadMatch[] = []
  if (!searchableQuery(query)) return out
  for (let i = rows.length - 1; i >= 0; i--) {
    const { id, text } = rows[i]
    const count = findOccurrences(text, query).length
    for (let n = count - 1; n >= 0; n--) out.push({ id, n, key: threadMatchKey(id, n) })
  }
  return out
}

/** The current match's index: the one named by `key`, else the newest
 *  (0). -1 when there are none. */
export function currentMatchIndex(matches: readonly ThreadMatch[], key: string | null): number {
  if (matches.length === 0) return -1
  if (key == null) return 0
  const i = matches.findIndex((m) => m.key === key)
  return i < 0 ? 0 : i
}

/** `dir` 1: the next match, older; -1: newer. Wraps at either end. */
export function stepMatchIndex(index: number, total: number, dir: 1 | -1): number {
  if (total <= 0) return -1
  if (index < 0) return 0
  return (index + dir + total) % total
}

/** What the count says, and what a screen reader hears. */
export function threadSearchStatus(o: {
  query: string
  index: number
  total: number
  searching: boolean
}): { text: string; spoken: string } {
  if (!searchableQuery(o.query)) return { text: '', spoken: '' }
  if (o.total === 0) {
    return o.searching
      ? { text: 'Searching…', spoken: 'Searching older messages' }
      : { text: 'No matches', spoken: 'No matches in this Thread' }
  }
  const more = o.searching ? '+' : ''
  return {
    text: `${o.index + 1} of ${o.total}${more}`,
    spoken: `Match ${o.index + 1} of ${o.total}${o.searching ? ', still searching older messages' : ''}`,
  }
}

// ── Shared state, per pane ──────────────────────────────────────────────

export interface ThreadSearchState {
  open: boolean
  query: string
  /** The current match's key; null = the newest. */
  current: string | null
  /** Published by the pane, newest first. */
  matches: readonly ThreadMatch[]
  /** The pane is still loading older messages to search. */
  searching: boolean
  /** Bumped by the shortcut while open: focus the field again. */
  focusTick: number
}

export const THREAD_SEARCH_CLOSED: ThreadSearchState = Object.freeze({
  open: false,
  query: '',
  current: null,
  matches: Object.freeze([]) as readonly ThreadMatch[],
  searching: false,
  focusTick: 0,
})

const states = new WeakMap<HTMLElement, ThreadSearchState>()
const listeners = new Set<() => void>()

export function threadSearchState(host: HTMLElement | null | undefined): ThreadSearchState {
  return (host && states.get(host)) || THREAD_SEARCH_CLOSED
}

export function updateThreadSearch(
  host: HTMLElement | null | undefined,
  patch: Partial<ThreadSearchState> | ((s: ThreadSearchState) => Partial<ThreadSearchState> | null),
): void {
  if (!host) return
  const prev = threadSearchState(host)
  const p = typeof patch === 'function' ? patch(prev) : patch
  if (!p) return
  const keys = Object.keys(p) as (keyof ThreadSearchState)[]
  if (keys.every((k) => Object.is(prev[k], p[k]))) return
  states.set(host, { ...prev, ...p })
  for (const l of [...listeners]) l()
}

export function closeThreadSearch(host: HTMLElement | null | undefined): void {
  if (!host || threadSearchState(host) === THREAD_SEARCH_CLOSED) return
  states.delete(host)
  for (const l of [...listeners]) l()
}

export function subscribeThreadSearch(cb: () => void): () => void {
  listeners.add(cb)
  return () => {
    listeners.delete(cb)
  }
}

/** The pane's floats host, from the header or from inside the Thread. */
export function threadSearchHost(el: Element | null | undefined): HTMLElement | null {
  if (!el) return null
  const header = el.closest('[data-thread-floats]')
  if (header) return header.parentElement
  return el.closest<HTMLElement>(`[${THREAD_FLOATS_HOST_ATTR}]`)
}

/** This pane's search state; `resolveHost` is read on every change. */
export function useThreadSearchState(resolveHost: () => HTMLElement | null): ThreadSearchState {
  return useSyncExternalStore(
    subscribeThreadSearch,
    () => threadSearchState(resolveHost()),
    () => THREAD_SEARCH_CLOSED,
  )
}

// ── The drawn text ──────────────────────────────────────────────────────

/** Never searched in the drawn text: controls (a code block's Copy) and
 *  decoration. */
const SKIP_SELECTOR = 'button, svg, input, textarea, select, [aria-hidden="true"], [data-thread-search-skip]'
const BLOCK_TAGS = new Set([
  'ADDRESS', 'ARTICLE', 'ASIDE', 'BLOCKQUOTE', 'DD', 'DETAILS', 'DIV', 'DL', 'DT', 'FIGCAPTION', 'FIGURE',
  'FOOTER', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'HEADER', 'HR', 'LI', 'OL', 'P', 'PRE', 'SECTION', 'SUMMARY',
  'TABLE', 'TBODY', 'TD', 'TFOOT', 'TH', 'THEAD', 'TR', 'UL',
])

interface TextSegment {
  node: Text
  at: number
}

/** The text of `roots` as it reads, blocks split by a newline, with where
 *  each text node starts in it. */
export function drawnSearchText(roots: readonly Element[]): { text: string; segments: TextSegment[] } {
  let text = ''
  const segments: TextSegment[] = []
  const breakLine = (): void => {
    if (text && !text.endsWith('\n')) text += '\n'
  }
  const walk = (el: Element): void => {
    for (const child of Array.from(el.childNodes)) {
      if (child.nodeType === 3) {
        const node = child as Text
        if (!node.data) continue
        segments.push({ node, at: text.length })
        text += node.data
      } else if (child.nodeType === 1) {
        const e = child as Element
        if (e.matches(SKIP_SELECTOR)) continue
        if (e.tagName === 'BR') {
          text += '\n'
          continue
        }
        const block = BLOCK_TAGS.has(e.tagName)
        if (block) breakLine()
        walk(e)
        if (block) breakLine()
      }
    }
  }
  for (const root of roots) {
    breakLine()
    walk(root)
  }
  return { text, segments }
}

function pointAt(segments: readonly TextSegment[], offset: number, end: boolean): { node: Text; offset: number } | null {
  for (const s of segments) {
    const len = s.node.data.length
    if (end ? offset > s.at && offset <= s.at + len : offset >= s.at && offset < s.at + len) {
      return { node: s.node, offset: offset - s.at }
    }
  }
  return null
}

/** The message words a bubble draws (not its quote of another message, its
 *  cards' controls or the author line). */
export function messageBodies(row: Element): Element[] {
  return Array.from(row.querySelectorAll(':scope [data-zen-bubble] > .chat-markdown'))
}

/** Ranges over each occurrence of `query` in `roots`' drawn text. */
export function searchRanges(roots: readonly Element[], query: string): Range[] {
  if (!searchableQuery(query) || roots.length === 0) return []
  const doc = roots[0].ownerDocument
  const { text, segments } = drawnSearchText(roots)
  const len = foldSearchText(query).length
  const out: Range[] = []
  for (const start of findOccurrences(text, query)) {
    const a = pointAt(segments, start, false)
    const b = pointAt(segments, start + len, true)
    if (!a || !b) continue
    const r = doc.createRange()
    r.setStart(a.node, a.offset)
    r.setEnd(b.node, b.offset)
    out.push(r)
  }
  return out
}

// ── Painting ────────────────────────────────────────────────────────────

export const THREAD_SEARCH_HIGHLIGHT = 'k2-thread-search'
export const THREAD_SEARCH_HIGHLIGHT_CURRENT = 'k2-thread-search-current'

interface HighlightRegistryLike {
  set(name: string, value: unknown): unknown
  delete(name: string): unknown
}

function registry(): { reg: HighlightRegistryLike; make: (ranges: Range[]) => unknown } | null {
  const css = (globalThis as { CSS?: { highlights?: HighlightRegistryLike } }).CSS
  const H = (globalThis as { Highlight?: new (...ranges: Range[]) => unknown }).Highlight
  if (!css?.highlights || typeof H !== 'function') return null
  return { reg: css.highlights, make: (ranges) => new H(...ranges) }
}

/** The page can paint highlights (CSS Custom Highlight API). */
export function threadSearchCanPaint(): boolean {
  return registry() !== null
}

// Every pane's ranges: the registry is the page's, so two Threads searching
// at once share the two highlights.
/** Whoever painted a set of matches (a pane). */
type PaintOwner = object
const painted = new Map<PaintOwner, { all: Range[]; current: Range | null }>()

function repaint(): void {
  const r = registry()
  if (!r) return
  const all: Range[] = []
  const current: Range[] = []
  for (const p of painted.values()) {
    for (const range of p.all) if (range !== p.current) all.push(range)
    if (p.current) current.push(p.current)
  }
  if (all.length) r.reg.set(THREAD_SEARCH_HIGHLIGHT, r.make(all))
  else r.reg.delete(THREAD_SEARCH_HIGHLIGHT)
  if (current.length) r.reg.set(THREAD_SEARCH_HIGHLIGHT_CURRENT, r.make(current))
  else r.reg.delete(THREAD_SEARCH_HIGHLIGHT_CURRENT)
}

export function paintThreadSearch(owner: object, all: Range[], current: Range | null): void {
  if (all.length === 0 && !current) {
    clearThreadSearchPaint(owner)
    return
  }
  painted.set(owner, { all, current })
  repaint()
}

export function clearThreadSearchPaint(owner: object): void {
  if (!painted.delete(owner)) return
  repaint()
}
