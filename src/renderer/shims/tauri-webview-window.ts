/**
 * Web shim for `@tauri-apps/api/webviewWindow`.
 *
 * The browser has one window, so the current webview window is the window
 * shim's current window (its listen/once/emit go through the event bus) plus
 * the webview shim's setZoom. OS file drops never arrive as tauri://drag-*
 * events on the web, so drop listeners simply never fire.
 */

import { getCurrentWindow, type WebWindow } from './tauri-window'
import { getCurrentWebview } from './tauri-webview'

export type WebWebviewWindow = WebWindow & {
  setZoom: (factor: number) => Promise<void>
}

let current: WebWebviewWindow | null = null

export function getCurrentWebviewWindow(): WebWebviewWindow {
  if (!current) {
    const win = getCurrentWindow()
    current = { ...win, setZoom: getCurrentWebview().setZoom }
  }
  return current
}

export async function getAllWebviewWindows(): Promise<WebWebviewWindow[]> {
  return [getCurrentWebviewWindow()]
}

/** No other native windows on the web: a lookup by label finds none. */
export class WebviewWindow {
  static async getByLabel(_label: string): Promise<WebWebviewWindow | null> {
    return null
  }
  static async getAll(): Promise<WebWebviewWindow[]> {
    return []
  }
}
