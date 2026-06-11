import { useEffect } from 'react'
import { listen } from '@tauri-apps/api/event'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore } from '@/stores/presets'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { usePanelsStore } from '@/stores/panels'
import { useSidebarStore } from '@/stores/sidebar'
import { useTimerStore } from '@/stores/timer'

/**
 * Listens for cross-window sync events emitted by the Rust backend.
 * When any window mutates persisted state (projects, settings, presets,
 * focus groups, timer), the backend emits a sync event. All windows
 * re-fetch the relevant data to stay in sync.
 *
 * **0.38.0 Commit 4 — tab sync was retired here.** Tabs are now driven
 * by the daemon's `/cli/sessions/events` WS push (subscribed from
 * `tabs.ts::loadLayoutForWorkspace`). Cross-window tab adoption +
 * drop happens at the daemon layer, so no `sync:tabs` /
 * `sync:tabs-request` traffic flows through Tauri events anymore.
 * Every other channel below still uses Tauri-event broadcast because
 * the underlying state (projects, presets, …) lives in SQLite +
 * Tauri commands rather than the daemon.
 *
 * Call once in App.tsx — runs in every window instance.
 */
export function useWindowSync(): void {
  useEffect(() => {
    const unlisteners: Array<() => void> = []

    const setup = async (): Promise<void> => {
      unlisteners.push(
        await listen('sync:projects', () => {
          useProjectsStore.getState().fetchProjects()
        })
      )

      unlisteners.push(
        await listen('sync:settings', () => {
          useSettingsStore.getState().fetchSettings()
          usePanelsStore.getState().initFromSettings()
          useSidebarStore.getState().initFromSettings()
        })
      )

      unlisteners.push(
        await listen('sync:presets', () => {
          usePresetsStore.getState().fetchPresets()
        })
      )

      unlisteners.push(
        await listen('sync:focus-groups', () => {
          useFocusGroupsStore.getState().fetchFocusGroups()
        })
      )

      unlisteners.push(
        await listen<any>('sync:timer', (event) => {
          useTimerStore.getState().syncFromEvent(event.payload)
        })
      )

      unlisteners.push(
        await listen('sync:timer-entries', () => {
          // Re-fetch entries if the timer settings section is open
          useTimerStore.getState().fetchEntries()
        })
      )
    }

    setup()

    return () => {
      unlisteners.forEach((fn) => fn())
    }
  }, [])
}
