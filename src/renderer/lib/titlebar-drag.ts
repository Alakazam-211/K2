/**
 * Title-bar drag + double-click maximize for frameless / overlay chrome.
 *
 * On Windows WebView2, putting `data-tauri-drag-region` / `-webkit-app-region: drag`
 * on a whole top bar often breaks CSS `:hover` for child controls even when
 * they set `no-drag`. Prefer JS `startDragging` only when the event target
 * is not an interactive control.
 *
 * `startDragging()` on the first mousedown of a double-click swallows the
 * second click, so drag starts only after a short delay or after the
 * pointer actually moves. Double-click uses maximize/unmaximize (allowed
 * in capabilities) rather than `toggleMaximize` (not allow-listed).
 */
import type { MouseEvent as ReactMouseEvent } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'

const INTERACTIVE =
  'button, input, select, textarea, a, [role="button"], [role="menuitem"], [role="menu"], .no-drag'

const DBLCLICK_MS = 500
const DRAG_SLOP_PX = 5

let dragTimer: ReturnType<typeof setTimeout> | null = null
let dragStart: { x: number; y: number } | null = null

function isInteractiveTarget(e: ReactMouseEvent | MouseEvent): boolean {
  const el = e.target as HTMLElement | null
  return !!el?.closest(INTERACTIVE)
}

function cancelPendingDrag(): void {
  if (dragTimer !== null) {
    clearTimeout(dragTimer)
    dragTimer = null
  }
  dragStart = null
  window.removeEventListener('mousemove', onPendingMove)
  window.removeEventListener('mouseup', onPendingUp)
}

function startDrag(): void {
  cancelPendingDrag()
  void getCurrentWindow().startDragging().catch(() => {})
}

function onPendingMove(e: MouseEvent): void {
  if (!dragStart) return
  const dx = e.clientX - dragStart.x
  const dy = e.clientY - dragStart.y
  if (dx * dx + dy * dy >= DRAG_SLOP_PX * DRAG_SLOP_PX) {
    startDrag()
  }
}

function onPendingUp(): void {
  cancelPendingDrag()
}

async function toggleWindowMaximize(): Promise<void> {
  const win = getCurrentWindow()
  try {
    const maximized = await win.isMaximized()
    if (maximized) await win.unmaximize()
    else await win.maximize()
  } catch {
    /* permission / web shim */
  }
}

export function titleBarDragOnMouseDown(e: ReactMouseEvent): void {
  if (e.button !== 0) return
  if (isInteractiveTarget(e)) return
  // Second click of a double-click: do not drag; zoom/restore instead.
  if (e.detail > 1) {
    cancelPendingDrag()
    e.preventDefault()
    void toggleWindowMaximize()
    return
  }
  cancelPendingDrag()
  dragStart = { x: e.clientX, y: e.clientY }
  window.addEventListener('mousemove', onPendingMove)
  window.addEventListener('mouseup', onPendingUp)
  dragTimer = setTimeout(() => {
    startDrag()
  }, DBLCLICK_MS)
}

/** Cancel a pending drag; maximize already ran on the second mousedown. */
export function titleBarOnDoubleClick(e: ReactMouseEvent): void {
  if (isInteractiveTarget(e)) return
  cancelPendingDrag()
  e.preventDefault()
}
