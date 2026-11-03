import { useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useToastStore } from '@/stores/toast'
import { useUpdateStore } from '@/stores/update'
import { useSettingsStore } from '@/stores/settings'
import { UPDATE_CHECK_INTERVAL } from '@shared/constants'
import { webFeatures } from '@/web/features'
import { isAirgap } from '@/lib/airgap'
import {
  AUTO_UPDATE_CHECKS_EVENT,
  LS_LAST_UPDATE_CHECK,
  autoUpdateChecksEnabled,
  msUntilUpdateCheckDue,
  readLastUpdateCheck,
} from '@/lib/auto-update-checks'
import { compareVersions } from '@/lib/server-capabilities'

interface UpdateInfo {
  current_version: string
  latest_version: string
  download_url: string
  has_update: boolean
}

// Track latest known version to avoid repeat toasts for the same update
let lastNotifiedVersion: string | null = null

function notifyUpdateAvailable(version: string, label: string): void {
  if (version === lastNotifiedVersion) return
  lastNotifiedVersion = version
  useToastStore.getState().addToast(
    `K2 v${version} is available`,
    'info',
    8000,
    {
      label,
      onClick: () => {
        useSettingsStore.getState().openSettings('general')
        useSettingsStore.setState({ pendingUpdateCheck: true })
      },
    }
  )
}

async function checkForUpdate(showToastIfNone = false): Promise<void> {
  if (isAirgap()) {
    if (showToastIfNone) {
      useToastStore.getState().addToast(
        'Air-gap is on (K2_AIRGAP=1). Update checks are disabled.',
        'info',
        4000,
      )
    }
    return
  }
  // Hosted web: no Tauri app updater — versioned SPA is edge-delivered.
  if (!webFeatures.appUpdater) {
    if (showToastIfNone) {
      useToastStore.getState().addToast(
        'Updates are managed by the edge-delivered web client',
        'info',
        4000,
      )
    }
    return
  }

  const updateStore = useUpdateStore.getState()

  // Try the Tauri updater plugin first (supports in-app download)
  try {
    const hasUpdate = await updateStore.checkForUpdate()
    if (hasUpdate) {
      const version = useUpdateStore.getState().version
      if (version) notifyUpdateAvailable(version, 'Update')
      return
    }
    // A failed Arch manifest fetch returns false with status `error`.
    // Don't toast "up to date" on top of that row.
    if (showToastIfNone && useUpdateStore.getState().status !== 'error') {
      useToastStore.getState().addToast('K2 is up to date', 'success', 3000)
    }
    return
  } catch {
    // Plugin check failed — fall back to legacy GitHub API check
  }

  // Fallback: legacy check via Rust command (browser download)
  try {
    const info = await invoke<UpdateInfo>('check_for_update')
    if (info.has_update) {
      notifyUpdateAvailable(info.latest_version, 'Download')
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

/** Shows the update an earlier check found (this page's, or another
 *  window's) with no fetch. Skipped when it isn't newer than what's running,
 *  as after installing it. */
async function offerRememberedUpdate(): Promise<void> {
  let current: string
  try {
    current = await invoke<string>('get_current_version')
  } catch (err) {
    console.warn('[update-checker] Failed to read the running version:', err)
    return
  }
  if (isAirgap() || !autoUpdateChecksEnabled()) return
  const remembered = readLastUpdateCheck()
  if (!remembered) return
  const offer =
    remembered.offer && compareVersions(current, remembered.offer.version) < 0 ? remembered.offer : null
  if (useUpdateStore.getState().restoreOffer(offer) && offer) {
    notifyUpdateAvailable(offer.version, 'Update')
  }
}

/** Exported for Settings to trigger a manual check */
export { checkForUpdate }
export type { UpdateInfo }

const STARTUP_DELAY_MS = 5000
/** Windows waiting on the same stored time wake spread out over this, so one
 *  has written the new time before the next rechecks it. localStorage reaches
 *  other windows a moment after the write. */
const WAKE_SPREAD_MS = 60_000

/**
 * Hook that checks for updates automatically: 5 seconds after a load if the
 * last check, by any window of this install, is at least one interval old,
 * and otherwise when the interval is up. Manual checks and remote update
 * triggers skip this guard and always fetch.
 */
export function useUpdateChecker(): void {
  useEffect(() => {
    if (!webFeatures.appUpdater) return
    let timer: ReturnType<typeof setTimeout> | undefined
    const stop = (): void => {
      if (timer !== undefined) clearTimeout(timer)
      timer = undefined
    }
    const wake = (delayMs: number): void => {
      timer = setTimeout(fire, delayMs)
    }
    const fire = (): void => {
      const spread = Math.random() * WAKE_SPREAD_MS
      const wait = msUntilUpdateCheckDue(UPDATE_CHECK_INTERVAL)
      if (wait > 0) {
        void offerRememberedUpdate()
        wake(wait + spread)
        return
      }
      void checkForUpdate()
      wake(UPDATE_CHECK_INTERVAL + spread)
    }
    const start = (): void => {
      stop()
      if (isAirgap()) return
      if (!autoUpdateChecksEnabled()) return
      wake(STARTUP_DELAY_MS)
    }
    // Another window's check landed: show its answer without waiting.
    const onStorage = (e: StorageEvent): void => {
      if (e.key === LS_LAST_UPDATE_CHECK) void offerRememberedUpdate()
    }
    start()
    window.addEventListener(AUTO_UPDATE_CHECKS_EVENT, start)
    window.addEventListener('storage', onStorage)
    return () => {
      stop()
      window.removeEventListener(AUTO_UPDATE_CHECKS_EVENT, start)
      window.removeEventListener('storage', onStorage)
    }
  }, [])
}
