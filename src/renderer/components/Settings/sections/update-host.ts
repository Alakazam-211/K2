// Pure decision logic + phase handling + copy for the "Update host"
// control (P4 of the remote-update system), split out of GeneralSection.tsx
// so the gating rules, update-phase state machine, and host-named copy are
// unit-testable WITHOUT importing the React component (which transitively
// pulls in Tauri/window-touching modules that can't load in the repo's
// `node` vitest env). GeneralSection's UpdateHostRow renders THROUGH these
// helpers so the UI and the tests can never drift.
//
// This MIRRORS restart-host.ts exactly — same remote-only gating, same
// owner/admin role rules, same host-named copy discipline. The control acts
// on the REMOTE machine you're connected to over K2 Connect, NEVER this Mac
// (the local Tauri auto-updater owns the local update path in GeneralSection).

export type UpdateRole = 'owner' | 'admin' | 'member'

/**
 * The phases the daemon update job moves through, per the P3 status route:
 *   GET /cli/daemon/update/status?job_id= → { phase, progress?, bytes?, error? }
 * phase ∈ downloading | verifying | staged | applying | restarting | done | failed | rolled-back
 */
export type UpdatePhase =
  | 'downloading'
  | 'verifying'
  | 'staged'
  | 'applying'
  | 'restarting'
  | 'done'
  | 'failed'
  | 'rolled-back'

/** The check route's response shape (P3): POST /cli/daemon/update/check. */
export interface UpdateCheckResult {
  current: string
  latest: string
  available: boolean
  notes?: string
  url?: string
  /** 0.39.35: how the remote host is installed. "bundled-app" hosts update
   *  via their co-located Tauri app (Shape A — auto-installs + relaunches,
   *  no separate manual "Install & restart" step); "standalone" hosts via
   *  the in-daemon binary swap (Shape B). Used only to vary copy; the daemon
   *  routes on it server-side. May be absent on older hosts. */
  installKind?: 'standalone' | 'bundled-app' | 'unknown'
}

/** The status route's response shape (P3). */
export interface UpdateStatusResult {
  phase: UpdatePhase
  progress?: number
  bytes?: number
  error?: string
}

/**
 * Visibility/enablement decision for the Update-host control. Mirrors
 * EXACTLY the render guards in UpdateHostRow — the unmistakable
 * remote-targeting requirement, made a pure function (identical rules to
 * restartHostVisibility):
 *   - LOCAL active host → never shown (the local-Mac update is the Tauri
 *     auto-updater "App Version" area).
 *   - remote without the route (serverSupports=false) → hidden.
 *   - Member viewer → hidden (the owner-gated route would 403).
 *   - Owner/Admin → shown + enabled. Unknown role (whoami pending/failed)
 *     → shown but DISABLED (button inert until a role resolves).
 */
export function updateHostVisibility(args: {
  isRemote: boolean
  supportsUpdate: boolean
  role: UpdateRole | null
}): { show: boolean; canUpdate: boolean } {
  const { isRemote, supportsUpdate, role } = args
  if (!isRemote) return { show: false, canUpdate: false }
  if (!supportsUpdate) return { show: false, canUpdate: false }
  if (role === 'member') return { show: false, canUpdate: false }
  return { show: true, canUpdate: role === 'owner' || role === 'admin' }
}

/** True once the staged build is ready to install & restart the host. */
export function isStaged(phase: UpdatePhase | null): boolean {
  return phase === 'staged'
}

/** True for terminal phases that should stop the status poll loop. The host
 *  goes away on `restarting`/`done` (the ConnectionGate's soft-reconnect
 *  takes over), and `failed`/`rolled-back` are dead ends — none warrant
 *  further polling. */
export function isTerminalPhase(phase: UpdatePhase | null): boolean {
  return (
    phase === 'done' ||
    phase === 'restarting' ||
    phase === 'failed' ||
    phase === 'rolled-back'
  )
}

/** True for the two failure phases that must be surfaced as a host-named
 *  error (the host stayed/rolled back on its old version). */
export function isFailurePhase(phase: UpdatePhase | null): boolean {
  return phase === 'failed' || phase === 'rolled-back'
}

/**
 * The progress/phase line shown on the row while an update job runs. NAMES
 * the host (the unmistakable-remote requirement) and renders the daemon's
 * phase verbatim into plain language, threading `progress` for the
 * download. `failed`/`rolled-back` are framed as the host staying on its
 * current version.
 */
export function updatePhaseCopy(
  phase: UpdatePhase,
  hostLabel: string,
  opts?: { progress?: number; current?: string },
): string {
  switch (phase) {
    case 'downloading': {
      const pct =
        typeof opts?.progress === 'number' ? ` (${Math.round(opts.progress)}%)` : ''
      return `Downloading update for ${hostLabel}…${pct}`
    }
    case 'verifying':
      return `Verifying update for ${hostLabel}…`
    case 'staged':
      return `Update staged for ${hostLabel} — ready to install & restart.`
    case 'applying':
      return `Installing update on ${hostLabel}…`
    case 'restarting':
      return `Installing & restarting ${hostLabel}… it'll reconnect automatically.`
    case 'done':
      return `${hostLabel} updated and is reconnecting.`
    case 'failed':
      return `Update failed — ${hostLabel} is still on ${opts?.current ?? 'its current version'}.`
    case 'rolled-back':
      return `Update rolled back — ${hostLabel} is still on ${opts?.current ?? 'its current version'}.`
  }
}

/** Copy for the "Update available — <current> → <latest>" banner once a
 *  check reports `available`. NAMES the host. */
export function updateAvailableCopy(
  hostLabel: string,
  current: string,
  latest: string,
): string {
  return `Update available for ${hostLabel} — ${current} → ${latest}`
}

/** The confirm-dialog copy for the install-&-restart step — it NAMES the
 *  active host (the unmistakable-remote requirement), framed as the REMOTE
 *  machine, explicitly NOT this Mac. */
export function updateHostConfirmCopy(
  hostLabel: string,
  hostname: string,
  latest: string,
): {
  title: string
  message: string
  confirmLabel: string
} {
  return {
    title: `Update ${hostLabel} to ${latest}?`,
    message:
      `This will install ${latest} on ${hostLabel} (${hostname}) — the REMOTE ` +
      `machine you're connected to, not this Mac — and restart it. Active ` +
      `sessions on it will briefly disconnect, then reconnect automatically ` +
      `when it's back on the new version. Continue?`,
    confirmLabel: `Install & restart ${hostLabel}`,
  }
}

/** A clear, host-named message for a 403 from any of the owner-gated update
 *  routes (mirrors the restart-host 403 copy). */
export function updateForbiddenCopy(hostLabel: string): string {
  return `You don't have permission to update ${hostLabel}. Only the host owner or an admin can update it.`
}

/** True when an error message looks like a 403/forbidden from the
 *  owner-token-gated update routes (same matcher the restart row uses). */
export function isForbiddenError(message: string): boolean {
  return /403|forbidden|invalid or missing token/i.test(message)
}
