// The outline of a markdown file (build-editors.md E3): its headings, ATX
// (`## Setup`) and setext (a paragraph over `===` or `---`), outside fenced
// and indented code, as plain words (no `**`, backticks or link targets), with
// the slug of its address (`@h:<slug>`, lib/editor/live/heading-slug: the
// markdown pack's rules, from the heading as written, repeats numbered the
// same way) and the line each starts on.
// Read from the text, so it's the same in Write, Read, Source and a live doc.
// Pure.

import type { Text } from '@codemirror/state'
import { slugNumberer, slugify } from '@/lib/editor/live/heading-slug'

export interface OutlineItem {
  level: number
  text: string
  slug: string
  /** 1-based. */
  line: number
}

const ATX = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$/
const FENCE = /^ {0,3}(`{3,}|~{3,})/
const SETEXT = /^ {0,3}(=+|-+)[ \t]*$/

/** A heading's words without their inline marks. */
export function plainHeading(raw: string): string {
  return raw
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]*)\]\[[^\]]*\]/g, '$1')
    .replace(/(`+)(.*?)\1/g, '$2')
    .replace(/(\*\*|__)(.*?)\1/g, '$2')
    .replace(/(\*|_)(.*?)\1/g, '$2')
    .replace(/~~(.*?)~~/g, '$1')
    .replace(/<[^>]+>/g, '')
    .replace(/\s+/g, ' ')
    .trim()
}

/** A line that can be (part of) a paragraph, so a setext rule under it makes a heading. */
const paragraphLine = (s: string): boolean =>
  !/^\s*$/.test(s) && !/^( {4}|\t)/.test(s) && !/^ {0,3}([-+*]|\d{1,9}[.)])([ \t]|$)/.test(s) && !/^ {0,3}>/.test(s) && !/^ {0,3}\|/.test(s) && !/^ {0,3}([-*_])( *\1){2,} *$/.test(s)

export function outlineOf(doc: Text): OutlineItem[] {
  const out: OutlineItem[] = []
  // Repeats in document order are numbered as headingSlugs numbers them (never a slug already used;
  // a heading with nothing kept has the empty slug, and numbers too, as the pack does).
  const unique = slugNumberer()
  const slugOf = (raw: string): string => unique(slugify(raw))
  let fence: { ch: string; len: number } | null = null
  // The paragraph so far: its first line and its words.
  let para: { line: number; text: string } | null = null
  const add = (level: number, raw: string, line: number): void => {
    out.push({ level, text: plainHeading(raw), slug: slugOf(raw.trim()), line })
  }
  let n = 0
  for (const iter = doc.iterLines(); !iter.next().done; ) {
    const text = iter.value
    n++
    if (fence) {
      const close = new RegExp(`^ {0,3}\\${fence.ch}{${fence.len},}\\s*$`)
      if (close.test(text)) fence = null
      continue
    }
    const f = FENCE.exec(text)
    if (f) {
      fence = { ch: f[1][0], len: f[1].length }
      para = null
      continue
    }
    const atx = ATX.exec(text)
    if (atx) {
      add(atx[1].length, atx[2] ?? '', n)
      para = null
      continue
    }
    const se = SETEXT.exec(text)
    if (se && para) {
      add(se[1][0] === '=' ? 1 : 2, para.text, para.line)
      para = null
      continue
    }
    if (paragraphLine(text)) para = para ? { line: para.line, text: `${para.text} ${text.trim()}` } : { line: n, text: text.trim() }
    else para = null
  }
  return out
}

/** The heading the 1-based `line` is in (an index into `items`), -1 above the first. */
export function headingAt(items: readonly OutlineItem[], line: number): number {
  let at = -1
  for (let i = 0; i < items.length && items[i].line <= line; i++) at = i
  return at
}
