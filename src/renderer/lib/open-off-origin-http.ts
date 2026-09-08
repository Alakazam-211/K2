// Desktop: in-app Browser tab. Hosted web: opener shim (`window.open`).

import { openUrl } from '@tauri-apps/plugin-opener'
import { useTabsStore } from '@/stores/tabs'
import { webFeatures } from '@/web/features'

export function openOffOriginHttp(href: string): void {
  if (webFeatures.browserPane) {
    useTabsStore.getState().openUrlInNewTab(href)
    return
  }
  void openUrl(href).catch((err) => {
    console.warn('[open-off-origin-http] failed to open', href, err)
  })
}
