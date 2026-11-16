// Go to line (build-editors.md E3): what the small field under ⌃G (or the
// status line's Ln, Col) takes. "12" or "L12", with a column after it if you
// like: "12:4", "L12:C4", "L12 C4", even the status line's own "Ln 12, Col 4".
// Pure.

export type LineTarget = { line: number; col: number | null } | { error: string }

const FORM = /^(?:l(?:n|ine)?\s*)?(\d+)(?:\s*[:, ]\s*(?:c(?:ol(?:umn)?)?\s*)?(\d+))?$/i

/** null for an empty field. */
export function parseLineTarget(input: string, lines: number): LineTarget | null {
  const s = input.trim()
  if (!s) return null
  const m = FORM.exec(s.replace(/\s*,\s*/, ', ').replace(/,\s*/, ':'))
  if (!m) return { error: 'Type a line number, like 12 or L12' }
  const line = Number(m[1])
  if (line < 1 || line > lines) return { error: `This file has ${lines} ${lines === 1 ? 'line' : 'lines'}` }
  return { line, col: m[2] ? Math.max(1, Number(m[2])) : null }
}
