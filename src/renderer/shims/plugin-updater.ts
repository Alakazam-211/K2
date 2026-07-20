/**
 * Web shim for `@tauri-apps/plugin-updater`.
 * No in-browser app updates — always reports no update available.
 */

export interface Update {
  version: string
  body?: string | null
  date?: string | null
  downloadAndInstall: (
    onEvent?: (event: {
      event: 'Started' | 'Progress' | 'Finished'
      data?: unknown
    }) => void,
  ) => Promise<void>
  close: () => Promise<void>
}

export async function check(
  _options?: Record<string, unknown>,
): Promise<Update | null> {
  return null
}
