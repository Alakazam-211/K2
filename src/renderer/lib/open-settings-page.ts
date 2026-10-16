// Open one Settings page from chrome that lives outside Settings (the
// top-bar usage menu, the Usage page's limits strip). The settings store is
// loaded on first use, not at import, so a small top-bar module does not
// pull the whole store (and its boot reads) in with it.

import type { SettingsSection } from '@/stores/settings'

export function openSettingsPage(section: SettingsSection): Promise<void> {
  return import('@/stores/settings').then(({ useSettingsStore }) => {
    useSettingsStore.getState().openSettings(section)
  })
}
