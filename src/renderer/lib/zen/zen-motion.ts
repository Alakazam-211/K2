// prd-zen-mode-v1 Z22, Z57 — Zen motion, Hyprland style.
//
// `[bezier] name = [x1, y1, x2, y2]` and `[animation] name = [on, speed_ds,
// curve, style?]` over the tree
//   global → zen (zenIn, zenOut) → rows (rowIn, rowMove)
//          → messages (messageIn) → conversation (conversationSwitch)
//          → working (workingPulse)
// The daemon resolves the tree (a child inherits until it is set) and sends
// each node as `{on, speed, bezier, style, …}`. Here every node becomes four
// `--zen-*` properties on the Zen root:
//   --zen-anim-<name>-duration    `<ms>ms` (speed is tenths of a second)
//   --zen-anim-<name>-ease        `cubic-bezier(x1, y1, x2, y2)` from the numbers
//   --zen-anim-<name>-keyframes   one of the fixed presets below, or `none`
//   --zen-anim-<name>-scale       the `popin <n>%` start scale (1 otherwise)
// Styles are fixed presets (`fade`, `slide`, `slidefade`, `popin <n>%`):
// keyframes are K2's, never the user's. The `ease` string the daemon also
// sends is not used: the curve is rebuilt from validated numbers.
//
// `prefers-reduced-motion: reduce` wins: every duration is `0ms` and every
// keyframe `none` (there is no reduced-motion handling anywhere else in the
// app; this is Zen-only, Z57). The keyframes sheet repeats that as a media
// query, so a widget that hard-codes a duration is still instant.

/** The animation tree `(name, parent)` (daemon `ANIMATION_TREE`). */
export const ZEN_ANIMATION_TREE: ReadonlyArray<readonly [string, string | null]> = [
  ['global', null],
  ['zen', 'global'],
  ['zenIn', 'zen'],
  ['zenOut', 'zen'],
  ['rows', 'global'],
  ['rowIn', 'rows'],
  ['rowMove', 'rows'],
  ['messages', 'global'],
  ['messageIn', 'messages'],
  ['conversation', 'global'],
  ['conversationSwitch', 'conversation'],
  ['working', 'global'],
  ['workingPulse', 'working'],
]
export const ZEN_ANIMATION_NAMES: readonly string[] = ZEN_ANIMATION_TREE.map(([n]) => n)

export const ZEN_ANIMATION_STYLES = ['fade', 'slide', 'slidefade', 'popin'] as const
export type ZenAnimationStyle = (typeof ZEN_ANIMATION_STYLES)[number]

/** `speed` is tenths of a second, 0..this (daemon `SPEED_MAX_DS`). */
export const ZEN_SPEED_MAX_DS = 100
export const ZEN_BEZIER_Y_MIN = -2
export const ZEN_BEZIER_Y_MAX = 3

/** Curves every file can name (daemon `BUILTIN_BEZIERS`). */
export const ZEN_BUILTIN_BEZIERS: Record<string, [number, number, number, number]> = {
  linear: [0, 0, 1, 1],
  ease: [0.25, 0.1, 0.25, 1],
  glide: [0.22, 1, 0.36, 1],
  soft: [0.45, 0, 0.55, 1],
  overshot: [0.34, 1.56, 0.64, 1],
}

/** One animation line: `[on, speed, curve, style?]` (the default theme). */
export interface ZenAnimationLine {
  on: boolean
  speed: number
  curve: string
  style?: string
}

/** The default template's motion: gentle `glide` curves (Z44). Mirrors the
 *  daemon's `default-zen.toml` `[animation]` table. */
export const ZEN_DEFAULT_ANIMATIONS: Record<string, ZenAnimationLine> = {
  global: { on: true, speed: 3, curve: 'glide' },
  zenIn: { on: true, speed: 4, curve: 'glide', style: 'fade' },
  zenOut: { on: true, speed: 3, curve: 'glide', style: 'fade' },
  rowIn: { on: true, speed: 3, curve: 'glide', style: 'slidefade' },
  rowMove: { on: true, speed: 3, curve: 'glide' },
  messageIn: { on: true, speed: 3, curve: 'glide', style: 'popin 92%' },
  conversationSwitch: { on: true, speed: 2, curve: 'glide', style: 'fade' },
  workingPulse: { on: true, speed: 12, curve: 'soft' },
}

/** A resolved node, ready to become CSS. */
export interface ZenAnimationNode {
  on: boolean
  /** Tenths of a second. */
  speed: number
  bezier: [number, number, number, number]
  style: string | null
}

function parentOf(name: string): string | null {
  const row = ZEN_ANIMATION_TREE.find(([n]) => n === name)
  return row ? row[1] : null
}

function chainOf(name: string): string[] {
  const out = [name]
  let cur = parentOf(name)
  while (cur) {
    out.push(cur)
    cur = parentOf(cur)
  }
  return out
}

/** Resolve the default lines over the tree (what the daemon does). */
export function resolveDefaultAnimations(): Record<string, ZenAnimationNode> {
  const out: Record<string, ZenAnimationNode> = {}
  for (const name of ZEN_ANIMATION_NAMES) {
    const chain = chainOf(name)
    const lineAt = chain.find((n) => ZEN_DEFAULT_ANIMATIONS[n])
    const line = lineAt ? ZEN_DEFAULT_ANIMATIONS[lineAt] : { on: true, speed: 3, curve: 'glide' }
    const styleAt = chain.find((n) => ZEN_DEFAULT_ANIMATIONS[n]?.style)
    out[name] = {
      on: line.on,
      speed: line.on ? line.speed : 0,
      bezier: [...(ZEN_BUILTIN_BEZIERS[line.curve] ?? ZEN_BUILTIN_BEZIERS.linear)] as [number, number, number, number],
      style: line.style ?? (styleAt ? (ZEN_DEFAULT_ANIMATIONS[styleAt].style ?? null) : null),
    }
  }
  return out
}

/** A bezier the daemon would accept, else null. y may leave 0–1. */
export function validBezier(raw: unknown): [number, number, number, number] | null {
  if (!Array.isArray(raw) || raw.length !== 4) return null
  if (!raw.every((n) => typeof n === 'number' && Number.isFinite(n))) return null
  const [x1, y1, x2, y2] = raw as number[]
  if (x1 < 0 || x1 > 1 || x2 < 0 || x2 > 1) return null
  if (y1 < ZEN_BEZIER_Y_MIN || y1 > ZEN_BEZIER_Y_MAX || y2 < ZEN_BEZIER_Y_MIN || y2 > ZEN_BEZIER_Y_MAX) return null
  return [x1, y1, x2, y2]
}

function fmt(v: number): string {
  const s = v.toFixed(3).replace(/0+$/, '').replace(/\.$/, '')
  return s === '-0' ? '0' : s
}

/** `[0.34, 1.56, 0.64, 1]` → `cubic-bezier(0.34, 1.56, 0.64, 1)`. */
export function cubicBezierCss(b: readonly [number, number, number, number]): string {
  return `cubic-bezier(${fmt(b[0])}, ${fmt(b[1])}, ${fmt(b[2])}, ${fmt(b[3])})`
}

/** A preset style the daemon would accept: `fade`, `slide`, `slidefade`,
 *  `popin` or `popin <0–100>%`. Null for no style; undefined when invalid. */
export function parseAnimationStyle(raw: unknown): { preset: ZenAnimationStyle; scale: number } | null | undefined {
  if (raw === null || raw === undefined) return null
  if (typeof raw !== 'string') return undefined
  const s = raw.trim()
  if (s === 'fade' || s === 'slide' || s === 'slidefade') return { preset: s, scale: 1 }
  const m = /^popin(?:\s+(\d{1,3}(?:\.\d+)?)%)?$/.exec(s)
  if (!m) return undefined
  const pct = m[1] === undefined ? 80 : Number.parseFloat(m[1])
  if (!(pct >= 0 && pct <= 100)) return undefined
  return { preset: 'popin', scale: pct / 100 }
}

/** Read one node of the daemon's `motion.animations`. Null when invalid. */
export function parseAnimationNode(raw: unknown): ZenAnimationNode | null {
  if (raw === null || typeof raw !== 'object' || Array.isArray(raw)) return null
  const o = raw as Record<string, unknown>
  if (typeof o.on !== 'boolean') return null
  const speed = o.speed
  if (typeof speed !== 'number' || !Number.isFinite(speed) || speed < 0 || speed > ZEN_SPEED_MAX_DS) return null
  const bezier = validBezier(o.bezier)
  if (!bezier) return null
  const style = parseAnimationStyle(o.style)
  if (style === undefined) return null
  return { on: o.on, speed: o.on ? speed : 0, bezier, style: typeof o.style === 'string' ? o.style.trim() : null }
}

/** The `--zen-anim-*` properties for one node. */
export function animationVars(name: string, node: ZenAnimationNode, reducedMotion: boolean): Record<string, string> {
  const style = parseAnimationStyle(node.style) ?? null
  const still = reducedMotion || !node.on
  const ms = still ? 0 : Math.round(node.speed * 100)
  let keyframes = 'none'
  if (!still) {
    if (style) keyframes = `zen-kf-${style.preset}`
    else if (name === 'workingPulse' || name === 'working') keyframes = 'zen-kf-pulse'
  }
  return {
    [`--zen-anim-${name}-duration`]: `${ms}ms`,
    [`--zen-anim-${name}-ease`]: cubicBezierCss(node.bezier),
    [`--zen-anim-${name}-keyframes`]: keyframes,
    [`--zen-anim-${name}-scale`]: fmt(style ? style.scale : 1),
  }
}

/** Every motion property name the engine may write. */
export const ZEN_MOTION_VARS: readonly string[] = ZEN_ANIMATION_NAMES.flatMap((n) => [
  `--zen-anim-${n}-duration`,
  `--zen-anim-${n}-ease`,
  `--zen-anim-${n}-keyframes`,
  `--zen-anim-${n}-scale`,
])

/**
 * K2's fixed keyframe presets, and the reduced-motion belt. Mounted once in
 * the Zen root. Fixed text: no user value is ever interpolated into it.
 */
export const ZEN_KEYFRAMES_CSS = `
@keyframes zen-kf-fade { from { opacity: 0 } to { opacity: 1 } }
@keyframes zen-kf-slide { from { transform: translateY(8px) } to { transform: none } }
@keyframes zen-kf-slidefade { from { opacity: 0; transform: translateY(8px) } to { opacity: 1; transform: none } }
@keyframes zen-kf-popin { from { opacity: 0; transform: scale(var(--zen-popin-from, 0.92)) } to { opacity: 1; transform: none } }
@keyframes zen-kf-pulse { 0%, 100% { opacity: 1 } 50% { opacity: 0.35 } }
@media (prefers-reduced-motion: reduce) {
  [data-zen-root] *, [data-zen-root] *::before, [data-zen-root] *::after {
    animation-duration: 0ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0ms !important;
  }
}
`

/**
 * Inline style for an element that plays animation `name` (entry presets
 * play once; `workingPulse` loops). For widgets: `style={zenAnimation('rowIn')}`.
 */
export function zenAnimation(name: string, opts: { loop?: boolean } = {}): Record<string, string> {
  const loop = opts.loop ?? name === 'workingPulse'
  return {
    animationName: `var(--zen-anim-${name}-keyframes)`,
    animationDuration: `var(--zen-anim-${name}-duration)`,
    animationTimingFunction: `var(--zen-anim-${name}-ease)`,
    animationFillMode: 'both',
    animationIterationCount: loop ? 'infinite' : '1',
    '--zen-popin-from': `var(--zen-anim-${name}-scale)`,
  }
}

/** A `transition` value for `props` on animation `name`'s timing. */
export function zenTransition(name: string, props: readonly string[]): string {
  return props.map((p) => `${p} var(--zen-anim-${name}-duration) var(--zen-anim-${name}-ease)`).join(', ')
}
