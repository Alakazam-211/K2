// ⌘, must open Settings with a Browser or Design tab in the window
// (0.46.0 smoke, Rosson 2026-10-17: "Cmd+, isn't taking me to the Settings
// page anymore").
//
// On macOS the Settings menu item's accelerator consumes ⌘, so the native
// menu event is the only keyboard path. menu.rs used to look the window up
// with `get_webview_window("main")` / `webview_windows()`. Tauri counts a
// window as a "webview window" only while it holds ONE webview, so once a
// Browser or Design tab added its child webview to the main window both
// returned nothing and the event was dropped: no Settings, and every other
// menu action (⌘J, ⌘L, New Tab …) silently did nothing too.
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { HOTKEYS } from '@shared/hotkeys'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..')
const menuRs = readFileSync(resolve(root, 'src-tauri/src/menu.rs'), 'utf8')
const appTsx = readFileSync(resolve(root, 'src/renderer/App.tsx'), 'utf8')

/** The body of `fn <name>(` up to its closing brace at column 0. */
function fnBody(src: string, name: string): string {
  const start = src.indexOf(`fn ${name}(`)
  if (start < 0) throw new Error(`menu.rs has no fn ${name}`)
  const end = src.indexOf('\n}\n', start)
  if (end < 0) throw new Error(`fn ${name} has no closing brace at column 0`)
  return src.slice(start, end)
}

/** `fn <name>(` bodies for the given cfg attribute line. */
function cfgFnBody(src: string, cfg: string, name: string): string {
  const at = src.indexOf(`${cfg}\nfn ${name}(`)
  if (at < 0) throw new Error(`menu.rs has no ${cfg} fn ${name}`)
  return fnBody(src.slice(at), name)
}

const WINDOW_ONLY_LOOKUPS = /get_webview_window\(|webview_windows\(\)/

describe('⌘, opens Settings from the native menu', () => {
  it('the Settings item carries the ⌘, accelerator and its event is menu:open-settings', () => {
    expect(menuRs).toContain('MenuItem::with_id(handle, "settings", "Settings...", true, Some("CmdOrCtrl+,"))')
    expect(HOTKEYS.find((h) => h.id === 'openSettings')?.defaultKey).toBe('Meta+,')
    const handler = fnBody(menuRs, 'handle_menu_event')
    const arm = handler.slice(handler.indexOf('"settings" =>'), handler.indexOf('"check-for-updates" =>'))
    expect(arm).toContain('app.emit("menu:open-settings", ())')
  })

  it('no menu path looks windows up in a way that skips a window with a Browser or Design child webview', () => {
    for (const name of ['handle_menu_event', 'emit_to_focused', 'emit_to_focused_window_only']) {
      expect(fnBody(menuRs, name), `fn ${name}`).not.toMatch(WINDOW_ONLY_LOOKUPS)
    }
    // With browser-pane (the default build) the helpers count every window.
    const mainPane = cfgFnBody(menuRs, '#[cfg(all(target_os = "macos", feature = "browser-pane"))]', 'main_webview')
    expect(mainPane).toContain('app.get_webview("main")')
    expect(mainPane).not.toMatch(WINDOW_ONLY_LOOKUPS)
    const focused = fnBody(menuRs, 'focused_window_label')
    expect(focused).toContain('#[cfg(feature = "browser-pane")]\n    let windows = app.windows();')
  })

  it('the renderer opens Settings on menu:open-settings and on a ⌘, keydown', () => {
    expect(appTsx).toContain("listen('menu:open-settings', () => openSettings())")
    expect(appTsx).toMatch(/if \(e\.metaKey && e\.key === ','\) \{\s*e\.preventDefault\(\)\s*openSettings\(\)/)
  })
})
