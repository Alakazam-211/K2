// The installed editors for an Open in … menu, with the editor picked in
// Settings → Editors / Fonts → Defaults first (stored by its label in the
// global project settings as `defaultEditor`).

import { invoke } from '@tauri-apps/api/core'
import { useSettingsStore } from '@/stores/settings'

export interface InstalledEditor {
  id: string
  label: string
}

/** `editors` with the one labelled `preferred` moved to the front; the rest
 *  keep their order. */
export function defaultEditorFirst<T extends InstalledEditor>(editors: T[], preferred: string | undefined): T[] {
  if (!preferred) return editors
  const i = editors.findIndex((e) => e.label === preferred)
  if (i <= 0) return editors
  return [editors[i], ...editors.slice(0, i), ...editors.slice(i + 1)]
}

/** The installed editors, default first; an empty list when the server
 *  can't say (the menu then just leaves the Open in items out). */
export async function installedEditorsDefaultFirst(): Promise<InstalledEditor[]> {
  let editors: InstalledEditor[] = []
  try {
    const raw = await invoke<InstalledEditor[]>('projects_get_editors')
    editors = Array.isArray(raw) ? raw : []
  } catch {
    return []
  }
  const global = useSettingsStore.getState().projectSettings['__global__'] as { defaultEditor?: string } | undefined
  return defaultEditorFirst(editors, global?.defaultEditor)
}
