/**
 * Global click interceptor that routes non-HTTP URL schemes through
 * macOS LaunchServices instead of letting Tauri's WKWebView swallow
 * the click, and keeps off-origin http(s) out of the K2 chrome webview.
 *
 * Installed once at app boot from `index.tsx`. Capture-phase listener
 * so it runs before component-level click handlers.
 */

import { openUrl } from '@tauri-apps/plugin-opener'
import { openOffOriginHttp } from '@/lib/open-off-origin-http'

/**
 * Walk up the DOM looking for the nearest `<a>` ancestor with an
 * `href`. Returns null for clicks outside any link.
 */
function findLinkAncestor(target: EventTarget | null): HTMLAnchorElement | null {
  let node = target as HTMLElement | null
  while (node && node !== document.body) {
    if (node instanceof HTMLAnchorElement && node.getAttribute('href')) return node
    node = node.parentElement
  }
  return null
}

function isSameOriginHttp(href: string): boolean {
  try {
    const resolved = new URL(href, window.location.href)
    if (resolved.protocol !== 'http:' && resolved.protocol !== 'https:') return false
    return resolved.origin === window.location.origin
  } catch {
    return false
  }
}

export function installExternalLinkHandler(): void {
  document.addEventListener(
    'click',
    (e) => {
      if (e.defaultPrevented) return

      const link = findLinkAncestor(e.target)
      if (!link) return

      const href = link.getAttribute('href') ?? ''
      if (!href || href.startsWith('#')) return

      const colonIdx = href.indexOf(':')
      if (colonIdx === -1) return
      const scheme = href.slice(0, colonIdx).toLowerCase()

      // javascript: must not run in the K2 chrome (do not `return` and allow it).
      if (scheme === 'javascript') {
        e.preventDefault()
        return
      }

      if (scheme === 'http' || scheme === 'https') {
        // Same-origin path/hash/SPA stays in the React webview.
        if (isSameOriginHttp(href)) return
        e.preventDefault()
        openOffOriginHttp(href)
        return
      }

      // Everything else (message:, tel:, facetime:, slack:, vscode:,
      // cursor:, file:, mailto:, custom app schemes…) → hand to
      // LaunchServices via the opener plugin. The capability allowlist
      // in `src-tauri/capabilities/default.json` enumerates the
      // schemes that are actually permitted; schemes not on the list
      // surface a Tauri permission error in the catch handler.
      e.preventDefault()
      openUrl(href).catch((err) => {
        console.warn('[external-link-handler] failed to open', href, err)
      })
    },
    true,
  )
}
