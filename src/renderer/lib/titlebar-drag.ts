/**
 * Title-bar drag helper for frameless / custom chrome.
 *
 * On Windows WebView2, putting `data-tauri-drag-region` / `-webkit-app-region: drag`
 * on a whole top bar often breaks CSS `:hover` for child controls even when
 * they set `no-drag`. Prefer JS `startDragging` only when the event target
 * is not an interactive control.
 */
import type { MouseEvent as ReactMouseEvent } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'

const INTERACTIVE =
  'button, input, select, textarea, a, [role="button"], [role="menuitem"], [role="menu"], .no-drag'

export function titleBarDragOnMouseDown(e: ReactMouseEvent): void {
  if (e.button !== 0) return
  const el = e.target as HTMLElement | null
  if (!el) return
  if (el.closest(INTERACTIVE)) return
  void getCurrentWindow().startDragging().catch(() => {})
}
