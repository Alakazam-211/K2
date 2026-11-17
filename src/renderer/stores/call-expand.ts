// Full screen: the call over K2, inside this window (CallExpanded), over a
// dim scrim. Per-window view state, never the daemon's: the call is the
// voice and video stores'.
//
// Exiting (the Exit full screen button, a click on the scrim, Esc) puts
// focus back where it was when it opened: the Full screen button that
// opened it, or the same button drawn again (the stage gives way while it's
// open, so its buttons are new elements by then).

import { create } from 'zustand'

export const useCallExpand = create<{ expanded: boolean }>(() => ({ expanded: false }))

/** Where focus goes back to on shrink. */
let opener: { el: HTMLElement | null; testId: string | null } | null = null

function noteOpener(from: string | null): void {
  if (typeof document === 'undefined') return
  const at = document.activeElement instanceof HTMLElement && document.activeElement !== document.body ? document.activeElement : null
  // A click doesn't focus a button in WebKit (the app's webview), so the
  // button says which it is; a key (the chord) leaves focus where it was.
  const el = from ? (at?.dataset.testid === from ? at : null) : at
  opener = { el, testId: from ?? at?.dataset.testid ?? null }
}

/** Open full screen. `from`: the test id of the button that asked
 *  (focus goes back to it, or to the one drawn in its place, on shrink). */
export function expandCall(from: string | null = null): void {
  if (useCallExpand.getState().expanded) return
  noteOpener(from)
  useCallExpand.setState({ expanded: true })
}

/** Exit full screen: back to how it was (the stage, or the slim bar). */
export function shrinkCall(): void {
  if (!useCallExpand.getState().expanded) return
  useCallExpand.setState({ expanded: false })
}

export function toggleCallExpand(): void {
  if (useCallExpand.getState().expanded) shrinkCall()
  else expandCall()
}

/** After the shrink has drawn: focus the opener, or the button drawn in its
 *  place. Called once; nothing to do when nothing had focus. */
export function returnFocusAfterShrink(): void {
  const o = opener
  opener = null
  if (!o || typeof document === 'undefined') return
  const target = o.el && o.el.isConnected ? o.el : o.testId ? document.querySelector<HTMLElement>(`[data-testid="${o.testId.replace(/["\\]/g, '\\$&')}"]`) : null
  target?.focus()
}

export function __resetCallExpandForTests(): void {
  opener = null
  useCallExpand.setState({ expanded: false })
}
