import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { check, type Update } from '@tauri-apps/plugin-updater'
// relaunch handled by Rust-side relaunch_via_open (helper script)

type UpdateStatus = 'idle' | 'checking' | 'available' | 'downloading' | 'ready' | 'error'

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

let pendingUpdate: Update | null = null

export const useUpdateStore = create<UpdateState>((set, get) => ({
  status: 'idle',
  version: null,
  notes: null,
  progress: 0,
  error: null,

  checkForUpdate: async () => {
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
    try {
      let contentLength = 0
      let downloaded = 0
      await pendingUpdate.downloadAndInstall((event) => {
        if (event.event === 'Started') {
          contentLength = (event.data as any).contentLength ?? 0
        } else if (event.event === 'Progress') {
          downloaded += (event.data as any).chunkLength ?? 0
          const pct = contentLength > 0 ? Math.round((downloaded / contentLength) * 100) : 0
          set({ progress: pct })
        } else if (event.event === 'Finished') {
          set({ status: 'ready', progress: 100 })
        }
      })
      set({ status: 'ready', progress: 100 })
    } catch (err) {
      console.error('[updater] Download failed:', err)
      set({ status: 'error', error: String(err) })
    }
  },

  installAndRelaunch: async () => {
    try {
      // Use macOS `open -n -a` to relaunch the .app bundle via launchd.
      // This bypasses Tauri's built-in relaunch which spawns a bare binary
      // that macOS doesn't register as a GUI app, and survives _exit(0).
      await invoke('relaunch_via_open')
    } catch (err) {
      // If relaunch fails, the update was still installed — tell the user
      console.error('[updater] Relaunch failed:', err)
      set({ status: 'error', error: 'Update installed successfully. Please reopen K2 to use the new version.' })
    }
  },
}))
