/**
 * Web shim for `@tauri-apps/plugin-opener`.
 * http(s) → window.open; paths/other schemes are no-ops with a warn.
 */

export async function openUrl(url: string): Promise<void> {
  if (/^https?:\/\//i.test(url)) {
    window.open(url, '_blank', 'noopener,noreferrer')
    return
  }
  console.warn(`[web-shim] openUrl refused non-http(s) url: ${url}`)
}

export async function openPath(path: string): Promise<void> {
  console.warn(`[web-shim] openPath not available in browser: ${path}`)
}

export async function revealItemInDir(path: string): Promise<void> {
  console.warn(`[web-shim] revealItemInDir not available in browser: ${path}`)
}
