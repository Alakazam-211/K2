/**
 * Web shim for `@tauri-apps/api/webview`.
 */

export interface WebWebview {
  label: string
  setZoom: (factor: number) => Promise<void>
  listen: (
    event: string,
    handler: (event: { event: string; payload: unknown }) => void,
  ) => Promise<() => void>
}

const current: WebWebview = {
  label: 'main',
  async setZoom(factor: number): Promise<void> {
    if (typeof document !== 'undefined') {
      document.documentElement.style.zoom = String(factor)
    }
  },
  async listen(
    _event: string,
    _handler: (event: { event: string; payload: unknown }) => void,
  ): Promise<() => void> {
    return () => {}
  },
}

export function getCurrentWebview(): WebWebview {
  return current
}

export function getCurrent(): WebWebview {
  return current
}
