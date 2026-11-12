/** The web build's `@tauri-apps/api/webviewWindow`: no Tauri internals, no throw. */
import { describe, expect, it } from 'vitest'
import { getCurrentWebviewWindow, getAllWebviewWindows } from './tauri-webview-window'

describe('web shim @tauri-apps/api/webviewWindow', () => {
  it('returns the one browser window without touching __TAURI_INTERNALS__', async () => {
    expect('__TAURI_INTERNALS__' in globalThis).toBe(false)
    const view = getCurrentWebviewWindow()
    expect(view.label).toBe('main')
    const off = await view.listen('tauri://drag-drop', () => {})
    expect(typeof off).toBe('function')
    off()
    expect((await getAllWebviewWindows()).map((w) => w.label)).toEqual(['main'])
  })
})
