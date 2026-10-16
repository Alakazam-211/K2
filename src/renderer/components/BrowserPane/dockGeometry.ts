/**
 * Geometry and look of the Browser pane's dock, as the native child view
 * needs them (src-tauri browser_webviews.rs).
 *
 * - `snapRect`: the DOM rect on the device-pixel grid. The native view is
 *   placed in logical points; a rect between device pixels left a seam.
 * - `dockAppearance`: the pane background (the native view shows it before
 *   the page paints and around an overscroll) and which of the dock's
 *   corners sit on a rounded pane corner (macOS clips the page to it).
 */

export interface DockRect {
  x: number
  y: number
  width: number
  height: number
}

/** Edges to whole device pixels (not origin and size separately). */
export function snapRect(r: DockRect, dpr: number): DockRect {
  const scale = Number.isFinite(dpr) && dpr > 0 ? dpr : 1
  const left = Math.round(r.x * scale) / scale
  const top = Math.round(r.y * scale) / scale
  const right = Math.round((r.x + r.width) * scale) / scale
  const bottom = Math.round((r.y + r.height) * scale) / scale
  return { x: left, y: top, width: Math.max(0, right - left), height: Math.max(0, bottom - top) }
}

/** sRGB 0–255 with alpha 0–255, or null for a colour we cannot read. */
export function parseCssColor(value: string): [number, number, number, number] | null {
  const v = value.trim().toLowerCase()
  const clamp = (n: number): number => Math.min(255, Math.max(0, Math.round(n)))
  const alpha = (a: string | undefined): number | null => {
    if (a === undefined) return 255
    const t = a.trim()
    const n = t.endsWith('%') ? parseFloat(t) / 100 : parseFloat(t)
    return Number.isFinite(n) ? clamp(n * 255) : null
  }
  let m = /^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)(?:\s*[,/]\s*([\d.]+%?))?\s*\)$/.exec(v)
  if (m) {
    const a = alpha(m[4])
    const rgb = [m[1], m[2], m[3]].map(Number)
    if (a === null || rgb.some((n) => !Number.isFinite(n))) return null
    return [clamp(rgb[0]), clamp(rgb[1]), clamp(rgb[2]), a]
  }
  m = /^color\(\s*srgb\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+%?))?\s*\)$/.exec(v)
  if (m) {
    const a = alpha(m[4])
    const rgb = [m[1], m[2], m[3]].map(Number)
    if (a === null || rgb.some((n) => !Number.isFinite(n))) return null
    return [clamp(rgb[0] * 255), clamp(rgb[1] * 255), clamp(rgb[2] * 255), a]
  }
  return null
}

export interface DockAppearance {
  /** [r, g, b, a] 0–255; null leaves the engine default. */
  background: [number, number, number, number] | null
  /** Radius (logical px) of the rounded pane corners the dock touches. */
  cornerRadius: number
  /** Top-left, top-right, bottom-right, bottom-left. */
  corners: [boolean, boolean, boolean, boolean]
}

const NO_CORNERS: DockAppearance['corners'] = [false, false, false, false]

/** Within this many px the dock's corner is the pane's corner. */
const CORNER_SLOP = 1.5

/**
 * The nearest rounded ancestor decides the corners: a dock corner that sits
 * on that ancestor's inner corner is clipped to its inner radius (outer
 * radius minus the border). No rounded ancestor: square.
 */
export function dockAppearance(dock: Element): DockAppearance {
  const background = paneBackground(dock)
  const d = dock.getBoundingClientRect()
  let node: Element | null = dock.parentElement
  while (node && node !== document.documentElement) {
    const cs = getComputedStyle(node)
    const radii = [
      cs.borderTopLeftRadius,
      cs.borderTopRightRadius,
      cs.borderBottomRightRadius,
      cs.borderBottomLeftRadius,
    ].map((r) => parseFloat(r) || 0)
    if (radii.some((r) => r > 0)) {
      const b = node.getBoundingClientRect()
      const bl = parseFloat(cs.borderLeftWidth) || 0
      const br = parseFloat(cs.borderRightWidth) || 0
      const bt = parseFloat(cs.borderTopWidth) || 0
      const bb = parseFloat(cs.borderBottomWidth) || 0
      const inner = { left: b.left + bl, right: b.right - br, top: b.top + bt, bottom: b.bottom - bb }
      const near = (a: number, c: number): boolean => Math.abs(a - c) <= CORNER_SLOP
      const corners: DockAppearance['corners'] = [
        near(d.left, inner.left) && near(d.top, inner.top),
        near(d.right, inner.right) && near(d.top, inner.top),
        near(d.right, inner.right) && near(d.bottom, inner.bottom),
        near(d.left, inner.left) && near(d.bottom, inner.bottom),
      ]
      const border = Math.max(bl, br, bt, bb)
      const touched = radii.filter((_, i) => corners[i])
      const radius = touched.length > 0 ? Math.max(0, Math.max(...touched) - border) : 0
      return radius > 0 ? { background, cornerRadius: radius, corners } : { background, cornerRadius: 0, corners: NO_CORNERS }
    }
    node = node.parentElement
  }
  return { background, cornerRadius: 0, corners: NO_CORNERS }
}

/** The colour painted behind the dock: its own background, else the
 *  nearest ancestor's that is not transparent. Null when none is (leave
 *  the engine default). */
export function paneBackground(dock: Element): DockAppearance['background'] {
  let node: Element | null = dock
  while (node) {
    const parsed = parseCssColor(getComputedStyle(node).backgroundColor || '')
    if (parsed && parsed[3] > 0) return parsed
    node = node.parentElement
  }
  return null
}

export function sameAppearance(a: DockAppearance | null, b: DockAppearance): boolean {
  return a !== null && JSON.stringify(a) === JSON.stringify(b)
}
