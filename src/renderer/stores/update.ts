import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { check, type Update } from '@tauri-apps/plugin-updater'
import { isAirgap } from '@/lib/airgap'
// relaunch handled by Rust-side relaunch_via_open (helper script)

type UpdateStatus = 'idle' | 'checking' | 'available' | 'downloading' | 'ready' | 'error'

type UpdateProgressEvent = {
  event: 'Started' | 'Progress' | 'Finished'
  data?: unknown
}

/** Tauri 2 updater: `download()` then `install()` — do not use
 *  `downloadAndInstall` for the Settings "Download" button (that runs
 *  NSIS immediately, skips Install & Relaunch, and on Windows fails to
 *  overwrite a live `k2-daemon.exe`). */
type SplitUpdate = Update & {
  download?: (onEvent?: (event: UpdateProgressEvent) => void) => Promise<void>
  install?: () => Promise<void>
}

interface UpdateState {
  status: UpdateStatus
  version: string | null
  notes: string | null
  progress: number
  error: string | null
  checkForUpdate: () => Promise<boolean>
  startDownload: () => Promise<void>
  installAndRelaunch: () => Promise<void>
}

let pendingUpdate: SplitUpdate | null = null

function applyDownloadProgress(
  event: UpdateProgressEvent,
  acc: { contentLength: number; downloaded: number },
  set: (p: Partial<UpdateState>) => void,
): void {
  if (event.event === 'Started') {
    acc.contentLength = (event.data as { contentLength?: number } | undefined)?.contentLength ?? 0
  } else if (event.event === 'Progress') {
    acc.downloaded += (event.data as { chunkLength?: number } | undefined)?.chunkLength ?? 0
    const pct =
      acc.contentLength > 0 ? Math.round((acc.downloaded / acc.contentLength) * 100) : 0
    set({ progress: pct })
  } else if (event.event === 'Finished') {
    set({ status: 'ready', progress: 100 })
  }
}

export const useUpdateStore = create<UpdateState>((set) => ({
  status: 'idle',
  version: null,
  notes: null,
  progress: 0,
  error: null,

  checkForUpdate: async () => {
    if (isAirgap()) {
      set({ status: 'idle', error: null })
      return false
    }
    set({ status: 'checking', error: null })
    try {
      const update = await check()
      if (update) {
        pendingUpdate = update
        set({
          status: 'available',
          version: update.version,
          notes: update.body ?? null,
        })
        return true
      }
      set({ status: 'idle' })
      return false
    } catch (err) {
      console.error('[updater] Check failed:', err)
      set({ status: 'error', error: String(err) })
      return false
    }
  },

  startDownload: async () => {
    if (!pendingUpdate) return
    set({ status: 'downloading', progress: 0 })
    const acc = { contentLength: 0, downloaded: 0 }
    const onEvent = (event: UpdateProgressEvent) => applyDownloadProgress(event, acc, set)
    try {
      if (typeof pendingUpdate.download === 'function') {
        await pendingUpdate.download(onEvent)
      } else {
        // Older plugin: no split API — last resort, still installs immediately.
        await pendingUpdate.downloadAndInstall(onEvent)
      }
      set({ status: 'ready', progress: 100 })
    } catch (err) {
      console.error('[updater] Download failed:', err)
      set({ status: 'error', error: String(err) })
    }
  },

  installAndRelaunch: async () => {
    try {
      // Unlock k2-daemon.exe so NSIS can replace it (Windows file lock).
      await invoke('stop_bundled_daemon_for_update').catch((e) => {
        console.warn('[updater] stop daemon before install:', e)
      })
      if (pendingUpdate && typeof pendingUpdate.install === 'function') {
        await pendingUpdate.install()
      }
      // macOS: open -a helper. Windows: start after this PID dies.
      // (On Windows, relaunch_via_open used to only process::exit — no relaunch.)
      await invoke('relaunch_via_open')
    } catch (err) {
      console.error('[updater] Relaunch failed:', err)
      set({
        status: 'error',
        error: 'Update installed. Please reopen K2 to use the new version.',
      })
    }
  },
}))
