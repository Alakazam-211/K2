import { invoke } from '@tauri-apps/api/core'

/** Cmd+L on a visible browser tab. Hidden panes stay mounted, so skip
 *  `aria-hidden="true"`. Select only after `makeFirstResponder:` on this
 *  UI webview returns YES. `HTMLElement.focus()` is not that resign, and
 *  `setFocus` drops the BOOL. `resign` is the test seam; production calls
 *  `ui_webview_make_first_responder`. */
export function focusVisibleBrowserAddress(
  resign: () => Promise<boolean> = resignUiWebview,
): boolean {
  const inputs = document.querySelectorAll<HTMLInputElement>('[data-browser-address]')
  for (const input of inputs) {
    if (input.closest('[aria-hidden="true"]')) continue
    void resign().then(
      (yes) => {
        if (!yes) return
        input.focus()
        input.select()
      },
      () => {
        // Rejected is not YES. Leave the address alone.
      },
    )
    return true
  }
  return false
}

function resignUiWebview(): Promise<boolean> {
  return invoke<boolean>('ui_webview_make_first_responder').then((yes) => yes === true)
}
