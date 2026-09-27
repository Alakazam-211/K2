/** Shell CSP `img-src` is `'self' asset: data: blob:` — an https favicon never paints. */
export const BROWSER_ICON_MAX_CHARS = 120_000

/**
 * Favicon URL the tab strip may put on an `<img>`. Anything else (https,
 * empty, oversized) is missing: the caller keeps the globe.
 */
export function paintableBrowserIcon(icon: string | null | undefined): string | null {
  if (typeof icon !== 'string') return null
  const value = icon.trim()
  if (!value || value.length > BROWSER_ICON_MAX_CHARS) return null
  if (value.startsWith('data:image/')) return value
  if (value.startsWith('blob:')) return value
  return null
}
