// Window-local menu commands (⌘, Settings, New Tab, Close Tab, terminal and
// app zoom …) reach only the window in front.
//
// menu.rs sends them with `emit_to(EventTarget::AnyLabel { label })` to the
// focused window's label (`emit_to_focused_window_only`). That alone is not
// enough: a JS `listen()` with no target defaults to `{ kind: 'Any' }`, which
// hears every emit whatever its target, so every open K2 window would still
// act. Each window therefore listens with its OWN label as the target.
//
// The label is the window's, and it is also its main webview's: a window that
// holds a Browser or Design child webview still matches (`AnyLabel` matches a
// window or a webview label).
import type { EventCallback, EventName, Options, UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'

type Listen = <T>(event: EventName, handler: EventCallback<T>, options?: Options) => Promise<UnlistenFn>

/** `{ target: AnyLabel <this window> }`, or undefined where there is no
 *  Tauri window (tests, the web client): there the plain listen is all
 *  there is, and only one window exists. */
export function ownWindowListenOptions(): Options | undefined {
  let label: string | null = null
  try {
    label = getCurrentWindow().label
  } catch {
    label = null
  }
  return label ? { target: { kind: 'AnyLabel', label } } : undefined
}

/** `listen(event, handler)` for this window's copy of a menu event only. */
export function listenOwnWindow<T>(
  listen: Listen,
  event: EventName,
  handler: EventCallback<T>,
): Promise<UnlistenFn> {
  return listen(event, handler, ownWindowListenOptions())
}
