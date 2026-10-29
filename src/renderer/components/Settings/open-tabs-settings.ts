// The "Bring it back in Workspace settings → Tabs" toast's link: open
// Settings on this workspace's Tabs rows and pulse them.

import { useSettingsStore } from '@/stores/settings'
import { TABS_SETTINGS_ID } from '@/lib/workspace-tab-settings'

export function openTabsSettings(projectId: string): void {
  useSettingsStore.getState().openSettings('projects', projectId, 'agent')
  // Two frames: the section mounts and lays out before the row is found
  // (same as the Settings search jump).
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      const el = document.querySelector<HTMLElement>(`[data-settings-id="${TABS_SETTINGS_ID}"]`)
      if (!el) return
      el.scrollIntoView({ behavior: 'smooth', block: 'center' })
      el.classList.add('settings-search-pulse')
      window.setTimeout(() => el.classList.remove('settings-search-pulse'), 1500)
    })
  })
}
