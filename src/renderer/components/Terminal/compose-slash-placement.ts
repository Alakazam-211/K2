// Where the compose bar's slash menu opens: above its button, inside the
// window. The menu's fixed `left`/`bottom` are CSS px; the button's rect
// and the window size are window px, which the app zoom makes larger, so
// both are converted first (lib/app-zoom).
import { clientRectToCss, cssViewport, type CssRect } from '@/lib/app-zoom'

/** The menu's width (CSS px). */
export const COMPOSE_SLASH_MENU_WIDTH = 200

/** Fixed `left`/`bottom` for the menu over a button measured at `measured`
 *  (a getBoundingClientRect result, window px). */
export function composeSlashMenuSpot(measured: CssRect): { left: number; bottom: number } {
  const rect = clientRectToCss(measured)
  const view = cssViewport()
  return {
    left: Math.max(4, Math.min(rect.left, view.width - COMPOSE_SLASH_MENU_WIDTH - 4)),
    bottom: view.height - rect.top + 4,
  }
}
