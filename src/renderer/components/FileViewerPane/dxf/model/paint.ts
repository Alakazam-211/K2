// How a resolved style paints. The model's rgb is used as it is (the viewer
// has no colour table of its own on this path); `fg: true` is the theme's
// foreground. A block definition is drawn once and placed many times, so a
// value that is "insert" (the placement's) reads a CSS variable that each
// placement sets on its <use>; a nested placement whose own value is
// "insert" sets nothing, so the outer placement's value flows through.

import type { ModelStyle, RGB } from './types'

export const FG = 'currentColor'
const C = '--k2i-c'
const A = '--k2i-a'
const V = '--k2i-v'

export const rgbOf = (c: RGB): string => `rgb(${c[0]} ${c[1]} ${c[2]})`

/** A concrete colour, or the placement's. */
export function colourOf(s: ModelStyle | undefined): string {
  if (!s) return FG
  if (s.rgb === 'insert') return `var(${C}, ${FG})`
  return s.fg ? FG : rgbOf(s.rgb)
}

export type Paint = { stroke: string; opacity?: number | string; visibility?: string }

export function paintOf(s: ModelStyle | undefined, visible: boolean | 'insert' = true): Paint {
  const p: Paint = { stroke: colourOf(s) }
  if (s?.alpha === 'insert') p.opacity = `var(${A}, 1)`
  else if (typeof s?.alpha === 'number' && s.alpha < 1) p.opacity = s.alpha
  if (visible === 'insert') p.visibility = `var(${V}, visible)`
  return p
}

/** What a placement hands its block: its own colour and alpha, and whether its layer-0 entities show. */
export function insertVars(s: ModelStyle | undefined, layerOn: boolean | 'insert' = true): Record<string, string> {
  const v: Record<string, string> = {}
  if (s && s.rgb !== 'insert') v[C] = s.fg ? FG : rgbOf(s.rgb)
  if (s && s.alpha !== 'insert') v[A] = String(s.alpha)
  if (layerOn !== 'insert') v[V] = layerOn ? 'visible' : 'hidden'
  return v
}

/** Style props for an element: colour as an attribute where it is concrete, variables through style. */
export function paintProps(p: Paint, attr: 'stroke' | 'fill' = 'stroke'): { [k: string]: unknown; style?: React.CSSProperties } {
  const style: React.CSSProperties = {}
  const props: { [k: string]: unknown; style?: React.CSSProperties } = {}
  if (p.stroke.startsWith('var(')) style[attr] = p.stroke
  else props[attr] = p.stroke
  if (p.opacity !== undefined) style.opacity = p.opacity
  if (p.visibility) style.visibility = p.visibility as React.CSSProperties['visibility']
  if (Object.keys(style).length) props.style = style
  return props
}
