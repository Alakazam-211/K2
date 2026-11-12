// jsdom has no layout, so a zoomed page is described by hand, the way a real
// engine reports it under the app zoom (CSS `zoom` on <html>):
// getBoundingClientRect and innerWidth/innerHeight are window px, and a
// position:fixed box's `left`/`top`/`width` are CSS px that show `zoom`
// times larger. `onScreen` turns a placed box back into window px.
import { act } from '@testing-library/react'
import { applyAppZoomToDocument } from '@/lib/app-zoom'

/** Zoom the page the way App.tsx does (Cmd+= / Settings): the zoom the
 *  placement code reads, plus <html> zoom and `--k2-app-zoom`. */
export function applyAppZoomForTest(z: number): void {
  window.__k2soZoom = z
  applyAppZoomToDocument(z)
}

/** Back to 100 %. */
export function resetAppZoomForTest(): void {
  delete window.__k2soZoom
  applyAppZoomToDocument(1)
}

export interface ScreenBox {
  left: number
  top: number
  right: number
  bottom: number
}

/** The window's size in window px. */
export function setWindowSize(width: number, height: number): void {
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: width })
  Object.defineProperty(window, 'innerHeight', { configurable: true, value: height })
}

/** Give `el` a rect in window px (what getBoundingClientRect reports). */
export function setScreenRect(el: Element, r: { left: number; top: number; width: number; height: number }): void {
  const rect = { ...r, right: r.left + r.width, bottom: r.top + r.height, x: r.left, y: r.top }
  ;(el as HTMLElement).getBoundingClientRect = () => ({ ...rect, toJSON: () => rect }) as DOMRect
}

/** A box of CSS size `css` (width × height) drawn at zoom `z`: its rect. */
export function setCssSize(el: Element, css: { width: number; height: number }, z: number): void {
  setScreenRect(el, { left: 0, top: 0, width: css.width * z, height: css.height * z })
}

/** Re-run placement after a size stub (the hook listens for resize). */
export function relayout(): void {
  act(() => {
    window.dispatchEvent(new Event('resize'))
  })
}

function px(v: string, what: string): number {
  if (!v.endsWith('px')) throw new Error(`${what} is not in px: "${v}"`)
  const n = Number(v.slice(0, -2))
  if (!Number.isFinite(n)) throw new Error(`${what} is not a number: "${v}"`)
  return n
}

/** Where a position:fixed box with CSS size `css` shows on screen at zoom
 *  `z`, from its inline `left` and `top` or `bottom`. */
export function onScreen(el: HTMLElement, css: { width: number; height: number }, z: number): ScreenBox {
  const left = px(el.style.left, 'left') * z
  const winH = window.innerHeight
  const top = el.style.top !== '' ? px(el.style.top, 'top') * z : winH - px(el.style.bottom, 'bottom') * z - css.height * z
  return { left, top, right: left + css.width * z, bottom: top + css.height * z }
}
