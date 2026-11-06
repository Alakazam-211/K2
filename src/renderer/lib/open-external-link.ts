// Opens a link from chat markdown (Thread messages, the Zen Conversation
// widget, Project chat, Feedback comments) in the person's default browser
// or mail app. Desktop: the Tauri opener plugin, which hands the URL to the
// OS (LaunchServices on macOS, ShellExecute on Windows, xdg-open on Linux).
// Hosted web: the opener shim.
//
// Only absolute http, https and mailto URLs open. Anything else (`file:`,
// `javascript:`, app schemes, relative paths, `#anchors`) is refused: an
// agent writes these messages, so a link in one must not reach the local
// file system or another app.

import { openUrl } from '@tauri-apps/plugin-opener'
import { useToastStore } from '@/stores/toast'

const EXTERNAL_LINK_PROTOCOLS = new Set(['http:', 'https:', 'mailto:'])

/** The URL a chat link may open outside K2 (normalized), or null when it
 *  may not: not absolute, or not http, https or mailto. */
export function externalLinkUrl(href: string): string | null {
  const raw = href.trim()
  if (!raw) return null
  let url: URL
  try {
    // No base: a relative href throws, so it can't resolve to K2's own origin.
    url = new URL(raw)
  } catch {
    return null
  }
  if (!EXTERNAL_LINK_PROTOCOLS.has(url.protocol)) return null
  if (url.protocol !== 'mailto:' && !url.hostname) return null
  return url.href
}

/** Open `href` in the default browser (or mail app for mailto). Returns
 *  false, with a toast saying why, when the link may not open. A failure
 *  from the OS shows a toast too: a click must never do nothing silently. */
export function openExternalLink(href: string): boolean {
  const url = externalLinkUrl(href)
  if (!url) {
    useToastStore.getState().addToast('K2 opens only http, https and mailto links.', 'error', 4000)
    return false
  }
  openUrl(url).catch((err: unknown) => {
    console.warn('[open-external-link] failed to open', url, err)
    const why = err instanceof Error ? err.message : String(err)
    useToastStore.getState().addToast(`Couldn't open the link: ${why}`, 'error')
  })
  return true
}
