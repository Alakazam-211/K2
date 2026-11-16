// The scene's view addresses: native addresses in the kit's grammar, keyed
// by the live store's keys (K2 Render "scene previews: every address is a
// native address", f2bb30c00; k2-mpc-blend WIRE.md, Addresses):
// - `@scene:main`, the scene's auto view;
// - `@obj:N`, an object (its live key `oN`): its kit address, framed;
// - `@cam:N`, a camera, N its camera object's key (the same N as that
//   object's `@obj:N`);
// - `@turntable:<0-7>`.
// N is decimal, with no sign and no leading zero. A key never changes while
// the doc lives, so a renamed object keeps its view, its cached still and
// whoever follows it. These are the route's and the listing's own
// addresses and a person's viewAddress: the view picker, the stills and
// Follow key by exactly these strings, with no table between. (The older
// proposal named views by `enc(id)`; K2 Render and K2 Cores moved to keys.)

export const SCENE_VIEW = '@scene:main'

export type ViewKindName = 'scene' | 'camera' | 'object' | 'turntable'
export type ParsedView = { kind: 'scene' } | { kind: 'camera' | 'object'; key: number } | { kind: 'turntable'; frame: number }

export const camView = (key: number): string => `@cam:${key}`
export const objView = (key: number): string => `@obj:${key}`
export const turntableView = (n: number): string => `@turntable:${n}`

const KEY = /^(0|[1-9]\d*)$/

/** An object's key from its kit address (`@obj:15` → 15), or null. */
export function keyOf(addr: string): number | null {
  const m = /^@obj:(.*)$/.exec(addr)
  return m && KEY.test(m[1]) ? +m[1] : null
}

/** A scene view address, or null when it isn't one. */
export function parseView(a: string): ParsedView | null {
  if (a === SCENE_VIEW) return { kind: 'scene' }
  const m = /^@(cam|obj):(.*)$/.exec(a)
  if (m) return KEY.test(m[2]) ? { kind: m[1] === 'cam' ? 'camera' : 'object', key: +m[2] } : null
  const t = /^@turntable:([0-7])$/.exec(a)
  return t ? { kind: 'turntable', frame: +t[1] } : null
}

export function formatView(p: ParsedView): string {
  if (p.kind === 'scene') return SCENE_VIEW
  if (p.kind === 'turntable') return turntableView(p.frame)
  return p.kind === 'camera' ? camView(p.key) : objView(p.key)
}
