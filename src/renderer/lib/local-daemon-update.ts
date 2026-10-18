// Local daemon update state for the ConnectionGate overlay.
//
// On Linux the app restarts the LOCAL k2-daemon user service itself when
// its version differs from the app's (src-tauri/src/linux_daemon.rs, the
// launchctl-kickstart twin): pacman/apt/dnf run as root and cannot restart
// a user unit, so after a package upgrade the old daemon kept answering
// and the gate sat on a dead "Connecting…" (z13flow, 0.46.0). The Rust
// side publishes its phase through `local_daemon_update_state`; the gate
// polls it while it waits on the local host and says "Updating K2
// daemon…", or shows why the restart could not fix it (e.g. a systemd
// drop-in pointing ExecStart at another build).
//
// Remote hosts never consult this: a remote daemon is never restarted.

import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'

export interface LocalDaemonUpdateState {
  phase: 'idle' | 'restarting' | 'done' | 'failed'
  from_version: string | null
  to_version: string | null
  message: string | null
}

export const IDLE_DAEMON_UPDATE: LocalDaemonUpdateState = {
  phase: 'idle',
  from_version: null,
  to_version: null,
  message: null,
}

/** The gate's local wait reason for a daemon on another version
 *  (`localPairedPolicy`: `version <daemon> != app <app>`). */
export function isVersionMismatchReason(reason: string): boolean {
  return reason.startsWith('version ')
}

export interface GateCopy {
  heading: string
  subline: string | null
  /** Multi-line explanation (pre-wrapped), only for a failed restart. */
  detail: string | null
  /** Show the Reload escape hatch now, whatever the attempt count. */
  forceReload: boolean
}

/** Overlay copy for a local wait. `null` = not a daemon-update situation;
 *  the overlay keeps its own "Connecting…" / migrating copy. */
export function daemonUpdateCopy(
  waitReason: string | null,
  state: LocalDaemonUpdateState,
): GateCopy | null {
  if (state.phase === 'failed') {
    return {
      heading: 'K2 daemon needs attention',
      subline:
        state.from_version && state.to_version
          ? `The daemon is v${state.from_version}; this app is v${state.to_version}.`
          : null,
      detail: state.message,
      forceReload: true,
    }
  }
  if (state.phase === 'restarting') {
    return {
      heading: 'Updating K2 daemon…',
      subline:
        state.from_version && state.to_version
          ? `Restarting the daemon (v${state.from_version} → v${state.to_version}).`
          : 'Restarting the daemon.',
      detail: null,
      forceReload: false,
    }
  }
  if (waitReason !== null && isVersionMismatchReason(waitReason)) {
    return {
      heading: 'Updating K2 daemon…',
      subline: 'Waiting for the daemon that matches this app.',
      detail: null,
      forceReload: false,
    }
  }
  return null
}

/** Poll the Rust state (1 s) while `active`. Outside Tauri (web client)
 *  the invoke fails and the state stays idle. */
export function useLocalDaemonUpdateState(active: boolean): LocalDaemonUpdateState {
  const [state, setState] = useState<LocalDaemonUpdateState>(IDLE_DAEMON_UPDATE)
  useEffect(() => {
    if (!active) {
      setState(IDLE_DAEMON_UPDATE)
      return
    }
    let cancelled = false
    let timer: ReturnType<typeof setTimeout> | null = null
    const tick = async (): Promise<void> => {
      try {
        const next = await invoke<LocalDaemonUpdateState>('local_daemon_update_state')
        if (!cancelled && next && typeof next.phase === 'string') setState(next)
      } catch {
        // Not running under Tauri (hosted web client): nothing to show.
        return
      }
      if (!cancelled) timer = setTimeout(() => void tick(), 1000)
    }
    void tick()
    return () => {
      cancelled = true
      if (timer) clearTimeout(timer)
    }
  }, [active])
  return state
}
