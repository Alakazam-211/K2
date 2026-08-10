/**
 * Web shim for `@tauri-apps/plugin-process`.
 * Hosted web has no process exit control.
 */

export async function exit(_code?: number): Promise<void> {
  // no-op in browser
}

export async function relaunch(): Promise<void> {
  if (typeof window !== 'undefined') window.location.reload()
}
