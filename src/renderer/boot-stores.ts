/**
 * The work the stores do once at app start: each store's first load and
 * its retry when the daemon (re)connects.
 *
 * These calls used to run when a store module was first imported, so every
 * test that imported a store fired them into whatever mock was installed.
 * Now a store only defines things at import, and the app's entry
 * (index.tsx) calls `bootStores()` once.
 *
 * Keep the order: it is the order the entry's imports ran these in.
 */
import { bootFocusGroups } from './stores/focus-groups'
import { bootProjects } from './stores/projects'
import { bootSettings } from './stores/settings'

export function bootStores(): void {
  bootSettings()
  // The custom editor themes, in their own chunk as before (settings.ts
  // used to start this import when it was first imported).
  void import('./stores/custom-themes').then((m) => m.bootCustomThemes()).catch(() => {})
  bootFocusGroups()
  bootProjects()
}
