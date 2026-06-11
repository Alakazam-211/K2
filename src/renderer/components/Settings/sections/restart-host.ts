// Pure decision logic + copy for the "Restart connected host" control
// (#661), split out of GeneralSection.tsx so the gating rules + dialog
// copy are unit-testable WITHOUT importing the React component (which
// transitively pulls in Tauri/window-touching modules that can't load in
// the repo's `node` vitest env). GeneralSection's RestartHostRow renders
// THROUGH these so the UI and the tests can never drift.

export type RestartRole = 'owner' | 'admin' | 'member'

/**
 * Visibility/enablement decision for the Restart-host control. Mirrors
 * EXACTLY the render guards in RestartHostRow — the unmistakable
 * remote-targeting requirement, made a pure function:
 *   - LOCAL active host → never shown (the local-Mac restart is DaemonRow).
 *   - remote without the route (serverSupports=false) → hidden.
 *   - Member viewer → hidden (the owner-gated route would 403).
 *   - Owner/Admin → shown + enabled. Unknown role (whoami pending/failed)
 *     → shown but DISABLED (button inert until a role resolves).
 */
export function restartHostVisibility(args: {
  isRemote: boolean
  supportsRestart: boolean
  role: RestartRole | null
}): { show: boolean; canRestart: boolean } {
  const { isRemote, supportsRestart, role } = args
  if (!isRemote) return { show: false, canRestart: false }
  if (!supportsRestart) return { show: false, canRestart: false }
  if (role === 'member') return { show: false, canRestart: false }
  return { show: true, canRestart: role === 'owner' || role === 'admin' }
}

/** The confirm-dialog copy for the Restart-host control — it NAMES the
 *  active host (the unmistakable-remote requirement). */
export function restartHostConfirmCopy(hostLabel: string, hostname: string): {
  title: string
  message: string
  confirmLabel: string
} {
  return {
    title: `Restart ${hostLabel}?`,
    message:
      `This will restart ${hostLabel} (${hostname}) — the REMOTE machine you're ` +
      `connected to, not this Mac. Active sessions on it will briefly disconnect, ` +
      `then reconnect automatically when it's back. Continue?`,
    confirmLabel: `Restart ${hostLabel}`,
  }
}
