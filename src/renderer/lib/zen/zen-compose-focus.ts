// Rosson 2026-10-04 — after the person PICKS an agent in Zen (a click on a
// row in the Agents list, or adding one through Add agent), keyboard focus
// goes to that conversation's message box, so they can type straight away.
//
// A pick files a request for its address. The message box takes it when it
// shows that address and can be typed in (a remote conversation may take a
// moment to open). Nothing else files one, so first render, a remote
// update, ⌘1–9, a Garden switch or a row changing under the list never move
// focus. A request is dropped when it is taken, replaced by a newer pick,
// older than `ZEN_COMPOSE_FOCUS_TTL_MS`, when the page goes away, or when
// the person is typing in another field by the time the box is ready.

import { create } from 'zustand'

/** How long a pick may wait for its conversation to open (ms). */
export const ZEN_COMPOSE_FOCUS_TTL_MS = 15_000

export interface ZenComposeFocusRequest {
  address: string
  at: number
}

export const useZenComposeFocusStore = create<{ request: ZenComposeFocusRequest | null }>(() => ({ request: null }))

/** The person picked `address`: focus its message box when it is ready. */
export function requestZenComposeFocus(address: string): void {
  useZenComposeFocusStore.setState({ request: { address, at: Date.now() } })
}

/** Drop any pending request (the page went away). */
export function cancelZenComposeFocus(): void {
  if (useZenComposeFocusStore.getState().request !== null) useZenComposeFocusStore.setState({ request: null })
}

function editable(el: Element | null): boolean {
  if (!el) return false
  if (el instanceof HTMLTextAreaElement) return true
  if (el instanceof HTMLInputElement) return !['button', 'checkbox', 'radio', 'submit', 'reset'].includes(el.type)
  return el instanceof HTMLElement && el.isContentEditable
}

/**
 * The box for `address` is on screen and enabled: take a pending request
 * for it. True when the box should focus now. A stale request, or one
 * where the person has since started typing in another field, is dropped.
 */
export function takeZenComposeFocus(address: string, box: HTMLElement): boolean {
  const r = useZenComposeFocusStore.getState().request
  if (!r || r.address !== address) return false
  useZenComposeFocusStore.setState({ request: null })
  if (Date.now() - r.at > ZEN_COMPOSE_FOCUS_TTL_MS) return false
  const active = typeof document === 'undefined' ? null : document.activeElement
  if (active && active !== box && editable(active)) return false
  return true
}
