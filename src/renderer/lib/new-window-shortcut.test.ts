import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { HOTKEYS, formatKeyCombo } from '@shared/hotkeys'
import { decideNewWindowShortcut, type NewWindowChord } from './new-window-shortcut'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..')

function read(rel: string): string {
  return readFileSync(resolve(root, rel), 'utf8')
}

function chord(partial: Partial<NewWindowChord> & Pick<NewWindowChord, 'isMac'>): NewWindowChord {
  return {
    key: 'n',
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...partial,
  }
}

describe('new window shortcut', () => {
  it('treats Ctrl+Shift+N with Meta up as window_new, and Shift+N alone as not', () => {
    expect(
      decideNewWindowShortcut(chord({ isMac: false, ctrlKey: true, shiftKey: true })),
    ).toBe('window_new')
    expect(
      decideNewWindowShortcut(chord({ isMac: true, ctrlKey: true, shiftKey: true })),
    ).toBe('window_new')
    expect(decideNewWindowShortcut(chord({ isMac: false, shiftKey: true }))).toBeNull()
    expect(decideNewWindowShortcut(chord({ isMac: true, shiftKey: true }))).toBeNull()
    expect(decideNewWindowShortcut(chord({ isMac: false, ctrlKey: true }))).toBeNull()
    expect(decideNewWindowShortcut(chord({ isMac: false, key: 'N', ctrlKey: true, shiftKey: true }))).toBe(
      'window_new',
    )
  })

  it('treats Cmd+Shift+N on mac as window_new and does not open an untitled document', () => {
    expect(
      decideNewWindowShortcut(chord({ isMac: true, metaKey: true, shiftKey: true })),
    ).toBe('window_new')
    expect(
      decideNewWindowShortcut(chord({ isMac: false, metaKey: true, shiftKey: true })),
    ).toBeNull()
    // Cmd+N stays out of this owner. Untitled document is the other hook.
    expect(decideNewWindowShortcut(chord({ isMac: true, metaKey: true, shiftKey: false }))).toBeNull()
    expect(
      decideNewWindowShortcut(
        chord({ isMac: true, metaKey: true, ctrlKey: true, shiftKey: true }),
      ),
    ).toBeNull()
    expect(
      decideNewWindowShortcut(chord({ isMac: false, ctrlKey: true, altKey: true, shiftKey: true })),
    ).toBeNull()

    const shortcuts = read('src/renderer/hooks/useTerminalShortcuts.ts')
    const nCase = shortcuts.slice(shortcuts.indexOf("case 'n':"), shortcuts.indexOf("case 'o':"))
    expect(nCase).toContain('if (e.shiftKey || e.altKey) return')
    expect(nCase.indexOf('if (e.shiftKey || e.altKey) return')).toBeLessThan(
      nCase.indexOf('state.openUntitledDocument'),
    )
    expect(nCase).not.toContain('window_new')
    expect(nCase).not.toContain('decideNewWindowShortcut')
    expect(shortcuts).not.toContain('window_new')
    expect(shortcuts).not.toContain("invoke('window_new')")
  })

  it('owns the chord on the window via window_new, not the macOS menu accelerator', () => {
    const hook = read('src/renderer/hooks/useNewWindowShortcut.ts')
    const app = read('src/renderer/App.tsx')
    const menu = read('src-tauri/src/menu.rs')
    expect(hook).toContain('decideNewWindowShortcut')
    expect(hook).toContain("invoke('window_new')")
    expect(hook).toContain("addEventListener('keydown', handler, true)")
    expect(hook.indexOf('e.repeat')).toBeLessThan(hook.indexOf("invoke('window_new')"))
    expect(hook).toContain('isWebClient()')
    expect(app).toContain('useNewWindowShortcut()')
    expect(menu).toContain(
      '&MenuItem::with_id(handle, "new-window", "New Window", true, None::<&str>)?',
    )
    expect(menu).not.toContain('CmdOrCtrl+Shift+N')
    expect(menu).toContain('"new-window" =>')
  })

  it('keeps the stored Meta+Shift+N default and formats Ctrl on non-mac', () => {
    const def = HOTKEYS.find((h) => h.id === 'newWindow')
    expect(def?.defaultKey).toBe('Meta+Shift+N')
    expect(formatKeyCombo('Meta+Shift+N', true)).toBe('\u2318\u21E7N')
    expect(formatKeyCombo('Meta+Shift+N', false)).toBe('Ctrl+Shift+N')
    expect(formatKeyCombo('Meta+Shift+N')).toBe('\u2318\u21E7N')
    const settings = read('src/renderer/components/Settings/sections/KeybindingsSection.tsx')
    expect(settings).toContain('formatKeyCombo(combo, isMacPlatform())')
    expect(settings).not.toContain('formatKeyCombo(combo)}')
    const hotkeys = read('src/shared/hotkeys.ts')
    expect(hotkeys).toContain("defaultKey: 'Meta+Shift+N'")
    expect(hotkeys).not.toContain("defaultKey: 'Ctrl+Shift+N'")
  })
})
