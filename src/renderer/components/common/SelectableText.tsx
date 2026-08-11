// Selectable message/body text for Tickets + Project chat.
//
// Why this is more than `user-select: text`:
//   1. Resize handlers set `document.body.style.userSelect = 'none'` (and
//      often leave it stuck if mouseup is lost). Form controls (textarea)
//      stay selectable under that; plain DOM text does not in WKWebView
//      even when a descendant re-enables user-select.
//   2. Parent gesture handlers (list cards, pane chrome) can swallow the
//      mousedown that starts a drag-select.
//
// Fix: clear stuck body styles on pointer-down, force user-select with
// !important via setProperty, stopPropagation so chrome doesn't treat the
// drag as a UI gesture, and keep CSS class + inline fallbacks.

import React, { type CSSProperties, type ReactNode, useCallback, useRef } from 'react'
import { LinkifiedText } from '@/lib/linkified-text'

export const SELECTABLE_TEXT_STYLE: CSSProperties = {
  userSelect: 'text',
  WebkitUserSelect: 'text',
  cursor: 'text',
}

/** Clear stuck body/html user-select left by resize/reorder handlers. */
export function clearStuckBodyUserSelect(): void {
  const clear = (el: HTMLElement | null): void => {
    if (!el) return
    el.style.removeProperty('user-select')
    el.style.removeProperty('-webkit-user-select')
  }
  clear(document.body)
  clear(document.documentElement)
}

function forceSelectable(node: HTMLElement | null): void {
  if (!node) return
  node.style.setProperty('user-select', 'text', 'important')
  node.style.setProperty('-webkit-user-select', 'text', 'important')
  node.style.setProperty('-webkit-app-region', 'no-drag')
  node.style.cursor = 'text'
}

/** Stop parent list/panel handlers from cancelling a text-selection drag,
 *  and unstick body user-select before the browser builds the range. */
function onSelectablePointerDown(e: React.PointerEvent | React.MouseEvent): void {
  // Only primary button — don't interfere with context menu / middle-click.
  if ('button' in e && e.button !== 0) return
  clearStuckBodyUserSelect()
  e.stopPropagation()
}

export function SelectableText({
  text,
  className = '',
  style,
}: {
  text: string
  className?: string
  /** Merged after the selectable defaults (e.g. editor fontSize). */
  style?: CSSProperties
}): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  const setRef = useCallback((node: HTMLDivElement | null) => {
    ref.current = node
    forceSelectable(node)
  }, [])

  return (
    <div
      ref={setRef}
      className={`selectable-copy chat-thread-selectable ${className}`.trim()}
      style={{ ...SELECTABLE_TEXT_STYLE, ...style }}
      onPointerDown={onSelectablePointerDown}
      onMouseDown={onSelectablePointerDown}
    >
      <LinkifiedText text={text} className="selectable-copy whitespace-pre-wrap break-words" />
    </div>
  )
}

/** Wrapper for mixed children that must remain selectable. */
export function SelectableRegion({
  children,
  className = '',
}: {
  children: ReactNode
  className?: string
}): React.JSX.Element {
  const setRef = useCallback((node: HTMLDivElement | null) => {
    forceSelectable(node)
    // Unstick any leftover body style as soon as a thread pane mounts.
    if (node) clearStuckBodyUserSelect()
  }, [])

  return (
    <div
      ref={setRef}
      className={`selectable-copy chat-thread-selectable ${className}`.trim()}
      style={SELECTABLE_TEXT_STYLE}
      // Only clear stuck body styles on the region itself — do NOT
      // stopPropagation here (that would block AssigneePicker / option
      // buttons). Message bodies use SelectableText which stops chrome
      // handlers on the text node.
      onPointerDown={() => clearStuckBodyUserSelect()}
      onMouseDown={() => clearStuckBodyUserSelect()}
    >
      {children}
    </div>
  )
}
