/**
 * Global click interceptor that routes non-HTTP URL schemes through
 * macOS LaunchServices instead of letting Tauri's WKWebView swallow
 * the click.
 *
 * The webview only knows how to navigate http(s) URLs; clicking a
 * `message://...` (Apple Mail), `tel:`, `facetime:`, `slack://...`,
 * `vscode://...`, `cursor://...`, etc. link in markdown content
 * (released chat history, notes pasted into the workspace, AI
 * assistant output) just silently fails. macOS knows how to route
 * those — we just need to forward them via the opener plugin so the
 * OS resolves the registered handler app.
 *
 * Installed once at app boot from `index.tsx`. Capture-phase listener
 * so it runs before component-level click handlers.
 */

import { openUrl } from '@tauri-apps/plugin-opener'

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

export function installExternalLinkHandler(): void {
  document.addEventListener(
    'click',
    (e) => {
      // Bail on modifier-clicks; webview has no concept of new tab/window
      // anyway, but be defensive.
      if (e.defaultPrevented) return

      const link = findLinkAncestor(e.target)
      if (!link) return

      const href = link.getAttribute('href') ?? ''
      if (!href || href.startsWith('#')) return

      const colonIdx = href.indexOf(':')
      if (colonIdx === -1) return
      const scheme = href.slice(0, colonIdx).toLowerCase()

      // Plain http(s): leave alone for now — many in-app surfaces use
      // these for SPA navigation (Markdown TOC, anchors, etc.) or rely
      // on Tauri's existing webview behavior. Future enhancement could
      // route external https → user's default browser, but that's a
      // larger UX change.
      if (scheme === 'http' || scheme === 'https' || scheme === 'javascript') {
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
