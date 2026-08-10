/**
 * App menu action IDs and handlers for the Win/Linux "Menu" button.
 * Emits the same events App.tsx listens for (see menu.rs handle_menu_event).
 * No CmdOrCtrl accelerators — webview owns those keybindings.
 */

import { emit } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { invoke } from '@tauri-apps/api/core'

/** Action IDs that map 1:1 to Tauri menu item ids / event stems. */
export const APP_MENU_ACTION_IDS = [
  'settings',
  'check-for-updates',
  'new-document',
  'new-tab',
  'launch-agent',
  'split-pane',
  'open-workspace',
  'close-tab',
  'command-palette',
  'running-agents',
  'toggle-sidebar',
  'toggle-assistant',
  'focus-window',
  'new-window',
  'minimize',
  'maximize',
  'close-window',
  'quit',
] as const

export type AppMenuActionId = (typeof APP_MENU_ACTION_IDS)[number]

const EVENT_ACTIONS: ReadonlySet<AppMenuActionId> = new Set([
  'settings',
  'check-for-updates',
  'new-document',
  'new-tab',
  'launch-agent',
  'split-pane',
  'open-workspace',
  'close-tab',
  'command-palette',
  'running-agents',
  'toggle-sidebar',
  'toggle-assistant',
  'focus-window',
])

function eventName(id: AppMenuActionId): string {
  return `menu:${id === 'settings' ? 'open-settings' : id === 'check-for-updates' ? 'check-for-updates' : id}`
}

export function isAppMenuActionId(id: string): id is AppMenuActionId {
  return (APP_MENU_ACTION_IDS as readonly string[]).includes(id)
}

/** Every declared action ID has a handler path (event emit, window API, or quit). */
export function appMenuActionHasHandler(id: AppMenuActionId): boolean {
  if (EVENT_ACTIONS.has(id)) return true
  return (
    id === 'new-window' ||
    id === 'minimize' ||
    id === 'maximize' ||
    id === 'close-window' ||
    id === 'quit'
  )
}

export async function handleAppMenuAction(id: AppMenuActionId): Promise<void> {
  if (EVENT_ACTIONS.has(id)) {
    await emit(eventName(id))
    return
  }

  const win = getCurrentWindow()

  switch (id) {
    case 'new-window':
      await invoke('window_new')
      return
    case 'minimize':
      await win.minimize()
      return
    case 'maximize': {
      const maximized = await win.isMaximized()
      if (maximized) await win.unmaximize()
      else await win.maximize()
      return
    }
    case 'close-window':
      await win.close()
      return
    case 'quit': {
      const { exit } = await import('@tauri-apps/plugin-process')
      await exit(0)
      return
    }
    default:
      return
  }
}
