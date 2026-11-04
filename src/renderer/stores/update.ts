import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isAirgap } from '@/lib/airgap'
import { askBeforeBoardFileLoss } from '@/lib/board-file/board-file-tabs'
import {
  rememberUpdateCheckResult,
  rememberUpdateCheckStarted,
  type UpdateOffer,
} from '@/lib/auto-update-checks'
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
  /** Asks GitHub, always. The automatic timer's interval guard is in
   *  `useUpdateChecker`. Records the time and what it found per install. */
  checkForUpdate: () => Promise<boolean>
  /** Shows what an earlier check found, with no fetch, after a reload or
   *  from another window. Leaves a running check, download or error alone.
   *  Returns true when `offer` is now on show. */
  restoreOffer: (offer: UpdateOffer | null) => boolean
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

/** Asked before an install restarts the app; false stops it. Set from outside so
 *  this store doesn't depend on what it's protecting (a voice call, V36). */
let installGuard: (() => Promise<boolean>) | null = null
export function setUpdateInstallGuard(guard: (() => Promise<boolean>) | null): void {
  installGuard = guard
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
 *  `k2-daemon.exe`). False on an offer restored from an earlier check: this
 *  page didn't find it and Rust may hold nothing (a relaunch empties it). */
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
    rememberUpdateCheckStarted()
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
          rememberUpdateCheckResult({
            version: probe.version,
            notes: null,
            downloadUrl: probe.download_url,
            canInstall: probe.can_install === true,
          })
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
        rememberUpdateCheckResult(null)
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
        rememberUpdateCheckResult({
          version: update.version,
          notes: update.notes ?? null,
          downloadUrl: null,
          canInstall: false,
        })
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
      rememberUpdateCheckResult(null)
      set({ status: 'idle', archDownloadUrl: null })
      return false
    } catch (err) {
      console.error('[updater] Check failed:', err)
      set({ status: 'error', error: String(err), archDownloadUrl: null })
      return false
    }
  },

  restoreOffer: (offer) => {
    const status = get().status
    if (status !== 'idle' && status !== 'available') return false
    if (!offer) {
      if (status === 'available') {
        pendingUpdate = false
        set({ status: 'idle', version: null, notes: null, archDownloadUrl: null, linuxCanInstall: false })
      }
      return false
    }
    set({
      status: 'available',
      version: offer.version,
      notes: offer.notes,
      error: null,
      archDownloadUrl: offer.downloadUrl,
      linuxCanInstall: offer.canInstall,
    })
    return true
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
    if (installGuard && !(await installGuard())) return
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
    if (!(await askBeforeBoardFileLoss('update'))) {
      set({ status: 'error', error: 'The update is waiting: a board file has edits that aren’t saved.' })
      return
    }
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
    // A restored offer: have Rust find the update before downloading it.
    if (!pendingUpdate && get().status === 'available' && !get().archDownloadUrl) {
      await get().checkForUpdate()
    }
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
    if (installGuard && !(await installGuard())) return
    // A board file's edit that isn't in the file yet stays only in this window: an update never discards one silently.
    if (!(await askBeforeBoardFileLoss('update'))) {
      set({ status: 'error', error: 'The update is waiting: a board file has edits that aren’t saved.' })
      return
    }
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
