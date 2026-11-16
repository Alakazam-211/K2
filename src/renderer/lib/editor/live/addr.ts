// Addresses on a live text doc, in the kit's grammar (K2 Cores: kit addr.rs
// is the one grammar): a heading is `@h:<slug>` (lib/editor/live/heading-slug,
// GitHub-style, repeats -1, -2), a line is `L12` (from 1). Used for your view
// address, what a "Say something" message points at, and reading others'.
// The chip keeps the heading's own words; only the wire carries the address.
// Pure.

import { headingSlugs } from './heading-slug'

export type Addr = { kind: 'heading'; slug: string } | { kind: 'line'; line: number }

export const headingAddr = (slug: string): string => `@h:${slug}`
export const lineAddr = (line: number): string => `L${line}`

export function parseAddr(address: string | null | undefined): Addr | null {
  if (!address) return null
  if (address.startsWith('@h:')) return address.length > 3 ? { kind: 'heading', slug: address.slice(3) } : null
  const l = /^L([1-9]\d*)$/.exec(address)
  if (l) return { kind: 'line', line: Number(l[1]) }
  return parseOldAddr(address)
}

// DELETE in 0.46.14: the old forms K2 sent in 0.46.13 builds before the kit's
// grammar (`heading:<percent-encoded name>`, `line:<n>`), read for one release.
function parseOldAddr(address: string): Addr | null {
  const h = /^heading:(.+)$/.exec(address)
  if (h) {
    let name = h[1]
    try {
      name = decodeURIComponent(name)
    } catch {
      // keep it as it came
    }
    const slug = name
      .toLowerCase()
      .replace(/[^\p{L}\p{N}\p{M}\s_-]/gu, '')
      .replace(/\s/g, '-')
    return slug ? { kind: 'heading', slug } : null
  }
  const l = /^line:([1-9]\d*)$/.exec(address)
  return l ? { kind: 'line', line: Number(l[1]) } : null
}

/** An address's line in this text and its words for a chip ("Setup", "line 12"), or null. */
export function resolveAddr(text: string, address: string | null | undefined): { line: number; label: string } | null {
  const a = parseAddr(address)
  if (!a) return null
  if (a.kind === 'line') return a.line <= text.split('\n').length ? { line: a.line, label: `line ${a.line}` } : null
  const h = headingSlugs(text).find((x) => x.slug === a.slug)
  return h ? { line: h.line, label: h.text } : null
}

/** Where `line` is, as an address: its section's heading (markdown), else the line itself. */
export function placeAddr(text: string, line: number, markdown: boolean): string {
  if (markdown) {
    let found: string | null = null
    let first = true
    for (const h of headingSlugs(text)) {
      if (h.line > line) break
      // The document's opening H1 is its title: what sits under it is the intro (as sections.ts).
      const isTitle = first && /^ {0,3}#[ \t]/.test(text.split('\n')[h.line - 1] ?? '')
      first = false
      found = isTitle ? null : h.slug
    }
    if (found) return headingAddr(found)
  }
  return lineAddr(line)
}
