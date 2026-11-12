// The app zoom (Cmd+= / Cmd+-, Settings → General) is CSS `zoom` on <html>,
// applied by `applyAppZoomToDocument`. It puts two coordinate spaces in the
// page, measured the same in WebKit (macOS, Linux) and Chromium (Windows):
//
//   - window px: getBoundingClientRect, clientX/Y, window.innerWidth and
//     innerHeight. What the person sees.
//   - CSS px: what `left`/`top`/`width` on an element mean. Under the zoom,
//     the page draws each CSS px `zoom` window px wide, so a position:fixed
//     `left: 600px` shows at 900 at 150 %. `100vw` is the window's width in
//     CSS px too, so it also draws `zoom` times wider than the window.
//
// Anything that places a floating box from a measured rect converts the
// rect AND the window size to CSS px first, then places and clamps in CSS
// px. Sizes that must fit the window use `var(--k2-vw)` / `var(--k2-vh)`
// (globals.css): the window's width and height in CSS px at any zoom.

/** The app zoom (1 = 100 %), 1 when unset or not a positive number. */
export function appZoom(): number {
  const z = typeof window !== 'undefined' ? window.__k2soZoom : undefined
  return typeof z === 'number' && Number.isFinite(z) && z > 0 ? z : 1
}

/** A window-px length or coordinate in CSS px. */
export function clientToCssPx(clientPx: number): number {
  return clientPx / appZoom()
}

export interface CssRect {
  top: number
  bottom: number
  left: number
  right: number
  width: number
  height: number
}

/** A getBoundingClientRect result (window px) in CSS px. */
export function clientRectToCss(rect: CssRect): CssRect {
  const z = appZoom()
  return {
    top: rect.top / z,
    bottom: rect.bottom / z,
    left: rect.left / z,
    right: rect.right / z,
    width: rect.width / z,
    height: rect.height / z,
  }
}

/** The visible window in CSS px: what a position:fixed box can use. */
export function cssViewport(): { width: number; height: number } {
  const z = appZoom()
  return { width: window.innerWidth / z, height: window.innerHeight / z }
}

/** Puts zoom `z` on the page: <html> zoom, `--k2-app-zoom` (which also
 *  drives `--k2-vw` / `--k2-vh` and the terminals' inverse zoom) and the
 *  `k2-app-zoomed` class. `window.__k2soZoom` is the caller's. */
export function applyAppZoomToDocument(z: number): void {
  const root = document.documentElement
  root.style.setProperty('--k2-app-zoom', String(z))
  root.classList.toggle('k2-app-zoomed', z !== 1)
  root.style.zoom = z === 1 ? '' : String(z)
}
