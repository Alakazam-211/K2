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
  /** Rust says this deb/rpm/Arch install can take the in-app download +
   *  SHA256SUMS verify + pkexec install (commands::linux_pkg_update).
   *  Missing/false keeps the open-the-download offer. */
  can_install?: boolean
}

/** `install_linux_package_update` result. `installed` → relaunch next;
 *  `manual` → the verified file is staged, show message + command. */
export interface LinuxInstallOutcome {
  status: 'installed' | 'manual'
  message: string
  manual_command: string | null
  daemon: { outcome: string; message: string | null } | null
}

interface UpdateState {
  status: UpdateStatus
  version: string | null
  notes: string | null
  progress: number
  error: string | null
  /** Linux package offer (Arch: the pacman asset; deb/rpm: the release
   *  page). Set only when plugin-updater is skipped. */
  archDownloadUrl: string | null
  /** Linux package install can run in-app (pkexec). */
  linuxCanInstall: boolean
  /** The in-app Linux install is running (download, verify, pkexec). */
  linuxInstalling: boolean
  /** pkexec could not run (missing, cancelled, no agent, tool failed):
   *  the exact command for a terminal. */
  linuxManualCommand: string | null
  checkForUpdate: () => Promise<boolean>
  /** Available-row button: Linux packages install in-app when they can
   *  (else open the package / release page); anything else runs the
   *  Tauri updater download. */
  openAvailableDownload: () => Promise<void>
  /** Download, verify against SHA256SUMS, pkexec install, restart the
   *  local daemon, relaunch. */
  installLinuxPackage: () => Promise<void>
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
  linuxCanInstall: false,
  linuxInstalling: false,
  linuxManualCommand: null,

  checkForUpdate: async () => {
    if (isAirgap()) {
      set({ status: 'idle', error: null, archDownloadUrl: null, linuxCanInstall: false })
      return false
    }
    set({
      status: 'checking',
      error: null,
      archDownloadUrl: null,
      linuxCanInstall: false,
      linuxManualCommand: null,
    })
    try {
      const probe = await invoke<InstallUpdateProbe>('probe_install_update')
      // Linux packages (Arch, deb, rpm) update from their package, not the
      // Tauri updater: latest.json has no linux-x86_64 entry.
      if (probe.kind === 'arch' || probe.kind === 'linux-package') {
        pendingUpdate = null
        if (probe.has_update && probe.version && probe.download_url) {
          set({
            status: 'available',
            version: probe.version,
            notes: null,
            error: null,
            archDownloadUrl: probe.download_url,
            linuxCanInstall: probe.can_install === true,
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
      console.error('[updater] Linux package manifest check failed:', err)
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
    if (url && get().linuxCanInstall) {
      await get().installLinuxPackage()
      return
    }
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

  installLinuxPackage: async () => {
    const version = get().version
    if (!version || !get().linuxCanInstall || get().linuxInstalling) return
    set({ status: 'downloading', progress: 0, error: null, linuxInstalling: true, linuxManualCommand: null })
    let outcome: LinuxInstallOutcome
    try {
      outcome = await invoke<LinuxInstallOutcome>('install_linux_package_update', { version })
    } catch (err) {
      // Refused: no SHA256SUMS line, hash mismatch, download failure.
      console.error('[updater] Linux package install refused:', err)
      set({ status: 'error', error: updateErrorText(err), linuxInstalling: false })
      return
    }
    if (outcome.status !== 'installed') {
      set({
        status: 'error',
        error: outcome.message,
        linuxInstalling: false,
        linuxManualCommand: outcome.manual_command,
      })
      return
    }
    set({ status: 'ready', progress: 100 })
    try {
      // Exits this process; the helper starts the packaged launcher.
      await invoke('relaunch_after_linux_package_update')
    } catch (err) {
      console.error('[updater] Relaunch after Linux install failed:', err)
      set({
        status: 'error',
        error: `K2 v${version} is installed. Please reopen K2 to use it. (${updateErrorText(err)})`,
        linuxInstalling: false,
      })
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
