/**
 * Web shim for `@tauri-apps/plugin-clipboard-manager`.
 * Prefers navigator.clipboard when available.
 */

export async function writeText(text: string): Promise<void> {
  if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text)
    return
  }
  console.warn('[web-shim] clipboard writeText unavailable')
}

export async function readText(): Promise<string> {
  if (typeof navigator !== 'undefined' && navigator.clipboard?.readText) {
    return await navigator.clipboard.readText()
  }
  console.warn('[web-shim] clipboard readText unavailable')
  return ''
}

export async function clear(): Promise<void> {
  // No standard browser clear; best-effort empty write.
  if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText('')
  }
}
