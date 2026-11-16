// Where something happened in a live text doc, in words (build-editors.md E4a):
// the markdown section a line sits in ("Setup", or "the intro" above the first
// heading), or the line itself in code. Changes, Live edits, the header's
// sentence and the people list all name places this one way.

const HEADING = /^ {0,3}#{1,6}[ \t]+(.+?)[ \t#]*$/

/** The heading above `line` (1-based), "the intro" above the first one; null for code. */
export function sectionAt(text: string, line: number, markdown: boolean): string | null {
  if (!markdown) return null
  const lines = text.split('\n')
  let fence = false
  let found: string | null = null
  let first = true
  for (let i = 0; i < Math.min(line, lines.length); i++) {
    const l = lines[i]
    if (/^ {0,3}(```|~~~)/.test(l)) fence = !fence
    else if (!fence) {
      const m = HEADING.exec(l)
      // The document's opening H1 is its title: what sits under it is the intro.
      if (m) found = first && /^ {0,3}#[ \t]/.test(l) ? null : m[1].trim()
      if (m) first = false
    }
  }
  return found ?? 'the intro'
}

/** The line (1-based) at UTF-8 byte `byte` of `text`. */
export function lineAtByte(text: string, byte: number): number {
  const bytes = new TextEncoder().encode(text)
  const pre = new TextDecoder().decode(bytes.subarray(0, Math.max(0, Math.min(byte, bytes.length))))
  let n = 1
  for (let i = pre.indexOf('\n'); i !== -1; i = pre.indexOf('\n', i + 1)) n++
  return n
}

/** A caret's line: the kit's `head_lc` ("line:col") when it sent one, else from its bytes. */
export function caretLine(text: string, caret: { head: number; head_lc?: string } | null | undefined): number | null {
  if (!caret) return null
  const lc = caret.head_lc ? Number.parseInt(caret.head_lc.split(':')[0], 10) : NaN
  return Number.isFinite(lc) && lc > 0 ? lc : lineAtByte(text, caret.head)
}

/** A place in words: the section, else the line ("line 12"). */
export function placeAt(text: string, line: number | null, markdown: boolean): string | null {
  if (line === null) return null
  return sectionAt(text, line, markdown) ?? `line ${line}`
}

/** What a finished edit did, after the name: "edited Setup", "edited lines 10–12". */
export function editWords(text: string, lines: { first: number; last: number }, markdown: boolean): string {
  const section = sectionAt(text, lines.first, markdown)
  if (section) return `edited ${section}`
  return lines.first === lines.last ? `edited line ${lines.first}` : `edited lines ${lines.first}–${lines.last}`
}

