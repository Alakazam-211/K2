import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isAirgap } from '@/lib/airgap'
// relaunch handled by Rust-side relaunch_via_open (helper script)

type UpdateStatus = 'idle' | 'checking' | 'available' | 'downloading' | 'ready' | 'error'

/** `app_update_check` result (Rust, src-tauri/src/commands/updater.rs). */
interface AppUpdateAvailable {
  version: string
  notes: string | null
}

/** `app-update-progress` event payload while `app_update_download` runs. */
interface AppUpdateProgress {
  downloaded: number
  total: number | null
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

/** True after `app_update_check` found an update (the Rust side holds it).
 *  App updates are found, downloaded, verified with K2's own update keys
 *  and installed in Rust (prd-security-sweep-0464-v1 SS4); the webview
 *  never calls the Tauri updater plugin. Download, then Install & Relaunch:
 *  never one step (on Windows that would run NSIS over a live
 *  `k2-daemon.exe`). */
let pendingUpdate = false

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
        pendingUpdate = false
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
      pendingUpdate = false
      set({
        status: 'error',
        error: updateErrorText(err),
        archDownloadUrl: null,
      })
      return false
    }
    try {
      const update = await invoke<AppUpdateAvailable | null>('app_update_check')
      if (update) {
        pendingUpdate = true
        set({
          status: 'available',
          version: update.version,
          notes: update.notes ?? null,
          error: null,
          archDownloadUrl: null,
        })
        return true
      }
      pendingUpdate = false
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
    let unlisten: (() => void) | null = null
    try {
      unlisten = await listen<AppUpdateProgress>('app-update-progress', (e) => {
        const { downloaded, total } = e.payload
        if (total && total > 0) set({ progress: Math.min(99, Math.round((downloaded / total) * 100)) })
      })
      // Rust downloads AND verifies (k2_signatures); a refusal is an error.
      await invoke<string>('app_update_download')
      set({ status: 'ready', progress: 100 })
    } catch (err) {
      console.error('[updater] Download failed:', err)
      set({ status: 'error', error: updateErrorText(err) })
    } finally {
      unlisten?.()
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
      try {
        await invoke('app_update_install')
      } catch (err) {
        // Not installed (nothing verified to install, or the install failed).
        console.error('[updater] Install failed:', err)
        set({ status: 'error', error: updateErrorText(err) })
        return
      }
      pendingUpdate = false
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
