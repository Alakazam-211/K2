// K2 Render's scene view addresses (its proposal, pending K2 Cores,
// 2026-11-15; .k2/prds/wire-shapes-k2-2026-11-15.md): `@scene:main`,
// `@cam:<enc(id)>`, `@obj:<enc(id)>`, `@turntable:<n>` (0–7), and
// `@cam-n:<i>` / `@obj-n:<i>` only when ids repeat. They are the listing's
// own addresses, so the view picker, the stills and a person's viewAddress
// (S5's Follow) all key by exactly these strings, with no table between.
// enc() percent-encodes `%`, `/`, whitespace and control characters.

export const SCENE_VIEW = '@scene:main'

/** Percent-encode `%`, `/`, whitespace and control characters (UTF-8 bytes, upper-case hex). */
export const encId = (id: string): string => id.replace(/[%/\s\p{Cc}]/gu, (c) => encodeURIComponent(c))

/** The id back; a malformed escape stays as it is. */
export function decId(s: string): string {
  try {
    return decodeURIComponent(s)
  } catch {
    return s
  }
}

export type ViewKindName = 'scene' | 'camera' | 'object' | 'turntable'
export type ParsedView = { kind: 'scene' } | { kind: 'camera' | 'object'; id: string } | { kind: 'camera' | 'object'; index: number } | { kind: 'turntable'; frame: number }

export const camView = (id: string): string => `@cam:${encId(id)}`
export const objView = (id: string): string => `@obj:${encId(id)}`
export const turntableView = (n: number): string => `@turntable:${n}`
export const dupView = (kind: 'camera' | 'object', i: number): string => `@${kind === 'camera' ? 'cam' : 'obj'}-n:${i}`

/** An encoded id: no raw %, /, whitespace or control character; every % starts an escape. */
const ENC = /^(?:[^%/\s\p{Cc}]|%[0-9A-Fa-f]{2})+$/u

/** A scene view address, or null when it isn't one. */
export function parseView(a: string): ParsedView | null {
  if (a === SCENE_VIEW) return { kind: 'scene' }
  const m = /^@(cam|obj)(-n)?:(.*)$/.exec(a)
  if (m) {
    const kind = m[1] === 'cam' ? 'camera' : 'object'
    if (m[2]) return /^(0|[1-9]\d*)$/.test(m[3]) ? { kind, index: +m[3] } : null
    return ENC.test(m[3]) ? { kind, id: decId(m[3]) } : null
  }
  const t = /^@turntable:([0-7])$/.exec(a)
  return t ? { kind: 'turntable', frame: +t[1] } : null
}

export function formatView(p: ParsedView): string {
  if (p.kind === 'scene') return SCENE_VIEW
  if (p.kind === 'turntable') return turntableView(p.frame)
  if ('index' in p) return dupView(p.kind, p.index)
  return p.kind === 'camera' ? camView(p.id) : objView(p.id)
}
