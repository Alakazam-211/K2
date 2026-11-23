/**
 * A click on a browser page in the macOS underlay goes to the native page,
 * so the DOM never sees it, and K2's menus and popovers would stay open. K2
 * posts `browser:page-pointer-down` for every mouse-down on a page of this
 * window (src-tauri browser_page_layers.rs); this turns it into the outside
 * click those menus already listen for (a pointerdown and a mousedown on
 * <body>, outside every menu).
 *
 * Whether the click also reaches the page is native (K2 Design's rule): it
 * is swallowed when the frontmost open claim is a `menu`, and passes for a
 * `popover`. Toasts don't listen for outside clicks, so they stay.
 */
import { listen } from '@tauri-apps/api/event'

export const PAGE_POINTER_DOWN_EVENT = 'browser:page-pointer-down'

/** What an outside click looks like to K2's menus and popovers. */
export function outsidePointerDown(target: Element | null = typeof document === 'undefined' ? null : document.body): void {
  if (!target) return
  const init: MouseEventInit = { bubbles: true, cancelable: true, composed: true, button: 0, buttons: 1 }
  const Pointer = typeof PointerEvent === 'function' ? PointerEvent : null
  target.dispatchEvent(Pointer ? new Pointer('pointerdown', init) : new MouseEvent('pointerdown', init))
  target.dispatchEvent(new MouseEvent('mousedown', init))
}

let listening = false

/** Listen once per window. */
export function listenPagePointerDown(): void {
  if (listening) return
  listening = true
  try {
    void listen(PAGE_POINTER_DOWN_EVENT, () => outsidePointerDown()).catch(() => {
      listening = false
    })
  } catch {
    // No Tauri event bridge (tests, the web client).
    listening = false
  }
}
