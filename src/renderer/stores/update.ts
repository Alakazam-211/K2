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

interface InstallUpdateProbe {
  kind: string
  version: string | null
  has_update: boolean
  download_url: string | null
}

interface UpdateState {
  status: UpdateStatus
  version: string | null
  notes: string | null
  progress: number
  error: string | null
  /** Pacman asset for an Arch offer. Set only when plugin-updater is skipped. */
  archDownloadUrl: string | null
  checkForUpdate: () => Promise<boolean>
  /** Available-row Download: Arch opens the pkg, anything else runs the installer. */
  openAvailableDownload: () => Promise<void>
  startDownload: () => Promise<void>
  installAndRelaunch: () => Promise<void>
}

function updateErrorText(err: unknown): string {
  if (typeof err === 'string') return err
  if (err instanceof Error && err.message) return err.message
  return String(err)
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

export const useUpdateStore = create<UpdateState>((set, get) => ({
  status: 'idle',
  version: null,
  notes: null,
  progress: 0,
  error: null,
  archDownloadUrl: null,

  checkForUpdate: async () => {
    if (isAirgap()) {
      set({ status: 'idle', error: null, archDownloadUrl: null })
      return false
    }
    set({ status: 'checking', error: null, archDownloadUrl: null })
    try {
      const probe = await invoke<InstallUpdateProbe>('probe_install_update')
      if (probe.kind === 'arch') {
        pendingUpdate = null
        if (probe.has_update && probe.version && probe.download_url) {
          set({
            status: 'available',
            version: probe.version,
            notes: null,
            error: null,
            archDownloadUrl: probe.download_url,
          })
          return true
        }
        set({
          status: 'idle',
          version: null,
          notes: null,
          error: null,
          archDownloadUrl: null,
        })
        return false
      }
    } catch (err) {
      // Fetch failed. Do not throw: the hook's catch only looks for a .dmg.
      console.error('[updater] Arch manifest check failed:', err)
      pendingUpdate = null
      set({
        status: 'error',
        error: updateErrorText(err),
        archDownloadUrl: null,
      })
      return false
    }
    try {
      const update = await check()
      if (update) {
        pendingUpdate = update
        set({
          status: 'available',
          version: update.version,
          notes: update.body ?? null,
          error: null,
          archDownloadUrl: null,
        })
        return true
      }
      set({ status: 'idle', archDownloadUrl: null })
      return false
    } catch (err) {
      console.error('[updater] Check failed:', err)
      set({ status: 'error', error: String(err), archDownloadUrl: null })
      return false
    }
  },

  openAvailableDownload: async () => {
    const url = get().archDownloadUrl
    if (url) {
      try {
        await invoke('plugin:opener|open_url', { url })
      } catch {
        window.open(url)
      }
      return
    }
    await get().startDownload()
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
    // Arch offers a pkg URL and never sets pendingUpdate. Do not relaunch
    // or the unknown bundle type falls through to an AppImage install.
    if (!pendingUpdate) return
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
