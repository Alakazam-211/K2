// Desktop: in-app Browser tab. Hosted web: opener shim (`window.open`).

import { openUrl } from '@tauri-apps/plugin-opener'
import { focusedRoom } from '@/stores/window-room'
import { webFeatures } from '@/web/features'

export function openOffOriginHttp(href: string): void {
  if (webFeatures.browserPane) {
    // A link click is window-level input: it opens in the focused room
    // (MS17/MS18). With no focused room it does nothing.
    focusedRoom()?.tabs.getState().openUrlInNewTab(href)
    return
  }
  void openUrl(href).catch((err) => {
    console.warn('[open-off-origin-http] failed to open', href, err)
  })
}
