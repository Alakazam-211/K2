// The call's video full screen (Rosson 2026-11-10). Per-window view state,
// never the daemon's: the call is the voice and video stores'.
//
// The full-screen view (CallFullscreen) is always mounted while the call has
// video, as an empty host. Going full screen draws the view into it at once
// (inside the click or key, so the request is still the person's gesture)
// and asks for element full screen on it: WKWebView (macOS 12.3+, wry turns
// it on), WebKitGTK and WebView2 all have it. Where it's missing or refused,
// the view stays as it is: a maximised overlay over this window. Esc leaves
// either (the browser's own Esc for element full screen, ours for the
// overlay).

import { flushSync } from 'react-dom'
import { create } from 'zustand'

/** off; element: the OS full screen; overlay: over this window only. */
export type CallFullscreenMode = 'off' | 'element' | 'overlay'

export const useCallFullscreen = create<{ mode: CallFullscreenMode }>(() => ({ mode: 'off' }))

interface FsDocument {
  fullscreenEnabled?: boolean
  webkitFullscreenEnabled?: boolean
  fullscreenElement?: Element | null
  webkitFullscreenElement?: Element | null
  exitFullscreen?: () => Promise<void> | void
  webkitExitFullscreen?: () => void
}

interface FsElement {
  requestFullscreen?: () => Promise<void> | void
  webkitRequestFullscreen?: () => void
}

let host: HTMLElement | null = null

/** The view's host registers itself while mounted. */
export function setCallFullscreenHost(el: HTMLElement | null): void {
  host = el
  if (!el && useCallFullscreen.getState().mode !== 'off') useCallFullscreen.setState({ mode: 'off' })
}

export function hasCallFullscreenHost(): boolean {
  return host !== null
}

function doc(): FsDocument {
  return document as unknown as FsDocument
}

/** The element in OS full screen now, if any. */
export function fullscreenElement(): Element | null {
  const d = doc()
  return d.fullscreenElement ?? d.webkitFullscreenElement ?? null
}

/** This webview can put an element in full screen. */
export function elementFullscreenSupported(): boolean {
  const d = doc()
  if (d.fullscreenEnabled === false && d.webkitFullscreenEnabled !== true) return false
  if (d.fullscreenEnabled !== true && d.webkitFullscreenEnabled !== true) return false
  const el = (host ?? document.documentElement) as unknown as FsElement
  return typeof el.requestFullscreen === 'function' || typeof el.webkitRequestFullscreen === 'function'
}

/** Go full screen. Call it from the click, double-click or key itself. */
export function enterCallFullscreen(): void {
  if (!host || useCallFullscreen.getState().mode !== 'off') return
  flushSync(() => useCallFullscreen.setState({ mode: 'overlay' }))
  const el = host as unknown as FsElement
  if (!elementFullscreenSupported()) return
  const refused = (err: unknown): void => {
    // Stays the in-window view.
    console.info('[call] element full screen refused; showing it over the window', err)
  }
  try {
    const p = el.requestFullscreen ? el.requestFullscreen.call(host) : el.webkitRequestFullscreen?.call(host)
    if (p && typeof (p as Promise<void>).then === 'function') (p as Promise<void>).catch(refused)
  } catch (err) {
    refused(err)
  }
}

/** Leave full screen (the OS's too, if it's ours). */
export function exitCallFullscreen(): void {
  if (useCallFullscreen.getState().mode === 'off') return
  useCallFullscreen.setState({ mode: 'off' })
  const fs = fullscreenElement()
  if (!fs || !host || fs !== host) return
  const d = doc()
  try {
    const p = d.exitFullscreen ? d.exitFullscreen.call(document) : d.webkitExitFullscreen?.call(document)
    if (p && typeof (p as Promise<void>).then === 'function') (p as Promise<void>).catch(() => {})
  } catch {
    // Already out.
  }
}

export function toggleCallFullscreen(): void {
  if (useCallFullscreen.getState().mode === 'off') enterCallFullscreen()
  else exitCallFullscreen()
}

/** `fullscreenchange`: the host went into OS full screen, or came out of it
 *  (Esc, the OS's own controls): follow. */
export function onFullscreenChange(): void {
  const fs = fullscreenElement()
  const mode = useCallFullscreen.getState().mode
  if (host && fs === host) {
    if (mode !== 'element') useCallFullscreen.setState({ mode: 'element' })
  } else if (mode === 'element') {
    useCallFullscreen.setState({ mode: 'off' })
  }
}

export function __resetCallFullscreenForTests(): void {
  host = null
  useCallFullscreen.setState({ mode: 'off' })
}
