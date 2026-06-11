import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { parseCustomThemeJson, type ThemeColors } from '@/lib/editor-themes'
import type { HighlightStyle } from '@codemirror/language'
// Phase 2.5 fix (finding #547) — daemon-reconnect retry bus.
import { onDaemonConnected } from '@/lib/daemon-reconnect'
// #625 — reload custom themes against the NEW host on a host switch.
import { onActiveHostChange } from '@/stores/connect-host'

/** Phase 2.5 fix (finding #547) — flips to true once `loadCustomThemes`
 *  successfully fetches a theme list from the daemon. Used purely for
 *  the reconnect retry guard below (this store has no destructive
 *  write-on-default code paths, so no persist gate is needed). */
let hasLoadedFromDaemon = false

interface CustomTheme {
  id: string       // e.g. "custom:my-custom-theme"
  name: string     // Display name from JSON
  path: string     // Absolute file path
  colors: ThemeColors
  highlight: HighlightStyle
  isLight: boolean
}

interface CustomThemesStore {
  customThemes: CustomTheme[]
  creatorOpen: boolean
  activeThemePath: string | null

  loadCustomThemes: () => Promise<void>
  openCreator: (baseThemeJson: string) => Promise<string | null>
  closeCreator: () => void
  deleteCustomTheme: (id: string) => Promise<void>
  getTheme: (id: string) => CustomTheme | undefined
}

export const useCustomThemesStore = create<CustomThemesStore>((set, get) => ({
  customThemes: [],
  creatorOpen: false,
  activeThemePath: null,

  loadCustomThemes: async () => {
    try {
      const entries = await daemonCliGet<{ path: string; name: string; valid: boolean }[]>('themes/list')
      const themes: CustomTheme[] = []

      for (const entry of entries) {
        if (!entry.valid) continue
        try {
          const result = await daemonCliGet<{ content: string }>('fs/read-file', { path: entry.path })
          const parsed = parseCustomThemeJson(result.content)
          if (!parsed) continue

          const id = `custom:${entry.path.split('/').pop()?.replace('.json', '') || entry.name}`
          themes.push({
            id,
            name: parsed.name,
            path: entry.path,
            colors: parsed.colors,
            highlight: parsed.highlight,
            isLight: parsed.type === 'light',
          })
        } catch {
          // Skip unreadable files
        }
      }

      set({ customThemes: themes })
      // Phase 2.5 fix (finding #547): only flip after a successful
      // top-level fetch. Per-entry parse failures stay tolerated
      // because they aren't a daemon-availability signal.
      hasLoadedFromDaemon = true
    } catch (err) {
      console.warn('[custom-themes] Failed to load:', err)
    }
  },

  openCreator: async (baseThemeJson: string) => {
    try {
      const result = await daemonCliPost<{ path: string }>('themes/create-template', {
        base_theme_json: baseThemeJson,
      })
      const path = result.path
      set({ creatorOpen: true, activeThemePath: path })
      return path
    } catch (err) {
      console.error('[custom-themes] Failed to create template:', err)
      return null
    }
  },

  closeCreator: () => {
    set({ creatorOpen: false, activeThemePath: null })
    // Reload themes to pick up the new/edited one
    get().loadCustomThemes()
  },

  deleteCustomTheme: async (id: string) => {
    const theme = get().customThemes.find((t) => t.id === id)
    if (!theme) return
    try {
      await daemonCliPost('themes/delete', { path: theme.path })
      set({ customThemes: get().customThemes.filter((t) => t.id !== id) })
    } catch (err) {
      console.error('[custom-themes] Failed to delete:', err)
    }
  },

  getTheme: (id: string) => {
    return get().customThemes.find((t) => t.id === id)
  },
}))

// Phase 2.5 fix (finding #547): retry the load when the daemon
// (re)connects, if the initial fetch (kicked off from settings.ts'
// dynamic import) failed because the daemon's HTTP listener
// wasn't bound yet. The store doesn't write-on-default so the
// only risk pre-fix was "no custom themes appear in the picker
// until the user reloads the app."
onDaemonConnected(() => {
  if (hasLoadedFromDaemon) return
  useCustomThemesStore.getState().loadCustomThemes()
})

// #625 — on a real active-host CHANGE, drop the load gate and reload the
// theme list from the NEW host's `~/.k2so/themes/`. `loadCustomThemes()`
// uses host-aware `daemonCliGet()` (reads `activeHost` at call time), and
// `onActiveHostChange` fires AFTER the flip, so this targets the new host.
onActiveHostChange(() => {
  hasLoadedFromDaemon = false
  void useCustomThemesStore.getState().loadCustomThemes()
})
