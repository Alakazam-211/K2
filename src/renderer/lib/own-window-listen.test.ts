// Menu commands act on the window in front only (owner report 2026-11-25:
// with several K2 windows open, ⌘, sent EVERY window to Settings).
//
// menu.rs sends each window-local command to the focused window's label
// (`emit_to_focused_window_only`), and each window hears only its own copy:
// a JS listen() with no target defaults to `Any` and hears every emit,
// whatever its target.
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const win = vi.hoisted(() => ({ label: 'window-abc' as string | null }))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => {
    if (win.label === null) throw new Error('no Tauri window')
    return { label: win.label }
  },
}))

import { listenOwnWindow, ownWindowListenOptions } from './own-window-listen'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..')
const menuRs = readFileSync(resolve(root, 'src-tauri/src/menu.rs'), 'utf8')
const appTsx = readFileSync(resolve(root, 'src/renderer/App.tsx'), 'utf8')
const terminalSettings = readFileSync(resolve(root, 'src/renderer/stores/terminal-settings.ts'), 'utf8')

/** `handle_menu_event`'s body. */
function handler(): string {
  const start = menuRs.indexOf('pub fn handle_menu_event(')
  const end = menuRs.indexOf('\n}\n', start)
  if (start < 0 || end < 0) throw new Error('menu.rs has no handle_menu_event')
  return menuRs.slice(start, end)
}

/** Every event the native menu sends to the window in front. */
function windowLocalEvents(): string[] {
  return [...handler().matchAll(/emit_to_focused_window_only\(app, "([^"]+)"\)/g)].map((m) => m[1])
}

describe('listenOwnWindow', () => {
  beforeEach(() => {
    win.label = 'window-abc'
  })

  it("listens with this window's own label as the target", async () => {
    const listen = vi.fn(async () => () => {})
    const handlerFn = (): void => {}
    await listenOwnWindow(listen, 'menu:open-settings', handlerFn)
    expect(listen).toHaveBeenCalledTimes(1)
    expect(listen).toHaveBeenCalledWith('menu:open-settings', handlerFn, {
      target: { kind: 'AnyLabel', label: 'window-abc' },
    })
  })

  it('with no Tauri window (tests, the web client) it is a plain listen', () => {
    win.label = null
    expect(ownWindowListenOptions()).toBe(undefined)
  })
})

describe('menu commands reach the window in front only', () => {
  it('the shell sends every window command to one window, never app.emit', () => {
    const events = windowLocalEvents()
    for (const e of [
      'menu:open-settings',
      'menu:check-for-updates',
      'menu:send-feedback',
      'menu:new-document',
      'menu:new-tab',
      'menu:launch-agent',
      'menu:split-pane',
      'menu:open-workspace',
      'menu:close-tab',
      'menu:command-palette',
      'menu:running-agents',
      'menu:projects',
      'menu:toggle-sidebar',
      'menu:server-switcher',
      'menu:focus-window',
      'menu:zen-toggle',
      'menu:zen-shortcuts',
      'app:zoom-in',
      'app:zoom-out',
      'app:zoom-reset',
      'terminal:zoom-in',
      'terminal:zoom-out',
    ]) {
      expect(events, e).toContain(e)
    }
    // The only broadcast left is Quit's board-file question (app-wide).
    const broadcasts = [...handler().matchAll(/app\.emit\("([^"]+)"/g)].map((m) => m[1])
    expect(broadcasts).toEqual(['board-file:quit-requested'])
    expect(menuRs).not.toMatch(/fn emit_to_focused\(/)
  })

  it('every renderer listener for those events hears only its own window', () => {
    const zen = ['menu:zen-toggle', 'menu:zen-shortcuts'] // ZenHost / ZenRoot, own Zen tests
    for (const e of windowLocalEvents().filter((x) => !zen.includes(x))) {
      const src = e.startsWith('terminal:') ? terminalSettings : appTsx
      expect(src, e).toContain(`listenOwnWindow(listen, '${e}',`)
      // An untargeted listen of the same event would still act in every window.
      expect(src, e).not.toContain(`listen('${e}'`)
    }
  })
})
