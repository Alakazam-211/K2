import { useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useToastStore } from '@/stores/toast'
import { useUpdateStore } from '@/stores/update'
import { useSettingsStore } from '@/stores/settings'
import { UPDATE_CHECK_INTERVAL } from '@shared/constants'

interface UpdateInfo {
  current_version: string
  latest_version: string
  download_url: string
  has_update: boolean
}

// Track latest known version to avoid repeat toasts for the same update
let lastNotifiedVersion: string | null = null

async function checkForUpdate(showToastIfNone = false): Promise<void> {
  const updateStore = useUpdateStore.getState()

  // Try the Tauri updater plugin first (supports in-app download)
  try {
    const hasUpdate = await updateStore.checkForUpdate()
    if (hasUpdate) {
      const version = useUpdateStore.getState().version
      if (version && version !== lastNotifiedVersion) {
        lastNotifiedVersion = version
        useToastStore.getState().addToast(
          `K2 v${version} is available`,
          'info',
          8000,
          {
            label: 'Update',
            onClick: () => {
              useSettingsStore.getState().openSettings('general')
              useSettingsStore.setState({ pendingUpdateCheck: true })
            },
          }
        )
      }
      return
    }
    if (showToastIfNone) {
      useToastStore.getState().addToast('K2 is up to date', 'success', 3000)
    }
    return
  } catch {
    // Plugin check failed — fall back to legacy GitHub API check
  }

  // Fallback: legacy check via Rust command (browser download)
  try {
    const info = await invoke<UpdateInfo>('check_for_update')
    if (info.has_update && info.latest_version !== lastNotifiedVersion) {
      lastNotifiedVersion = info.latest_version
      useToastStore.getState().addToast(
        `K2 v${info.latest_version} is available`,
        'info',
        8000,
        {
          label: 'Download',
          onClick: () => {
            useSettingsStore.getState().openSettings('general')
            useSettingsStore.setState({ pendingUpdateCheck: true })
          },
        }
      )
    } else if (showToastIfNone && !info.has_update) {
      useToastStore.getState().addToast(
        `K2 v${info.current_version} is up to date`,
        'success',
        3000
      )
    }
  } catch (err) {
    console.warn('[update-checker] Failed to check for updates:', err)
  }
}

/** Exported for Settings to trigger a manual check */
export { checkForUpdate }
export type { UpdateInfo }

/**
 * Hook that checks for updates on mount and every 3 hours.
 * Uses Tauri updater plugin for in-app download when available,
 * falls back to legacy GitHub API check for browser download.
 */
export function useUpdateChecker(): void {
  useEffect(() => {
    const startupTimeout = setTimeout(() => checkForUpdate(), 5000)
    const interval = setInterval(() => checkForUpdate(), UPDATE_CHECK_INTERVAL)
    return () => {
      clearTimeout(startupTimeout)
      clearInterval(interval)
    }
  }, [])
}
