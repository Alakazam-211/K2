/**
 * Web shim for `@crabnebula/tauri-plugin-drag`.
 * Native OS drag is unavailable in the browser.
 */

export async function startDrag(_options: {
  item: string[] | string
  icon?: string
}): Promise<void> {
  console.warn('[web-shim] startDrag not available in browser')
}
