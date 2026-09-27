// Per-window mark for a single-pane sidecar refresh.
//
// `sessions/v2/refresh` unregisters (SessionRemoved) then spawns
// (SessionAdded) for the same `tab-<paneGroupId>`. The HTTP response is
// not a fence: the broadcast is queued, and the renderer may apply
// SessionRemoved after `daemonCliPost` resolves. Clearing this mark in
// `finally` drops the strip tab with nothing left to put it back.
//
// The mark stays until that SessionRemoved has been applied and skipped.
// A spawn failure after unregister (`v2 spawn failed…`) is the opposite:
// SessionAdded will not arrive, so a remove that was already skipped must
// drop now, and one that has not arrived must still drop.

export type SidecarRefreshOutcome =
  | { ok: true }
  | { ok: false; message: string }

export type SidecarRefreshVerdict = 'keep' | 'drop'

export type SessionRemovedTake = 'passthrough' | 'skip'

type Guard =
  | { phase: 'inflight'; removedSeen: boolean }
  | { phase: 'await-skip' }
  | { phase: 'await-drop' }

const guards = new Map<string, Guard>()

export type SidecarRefreshMark = 'none' | 'inflight' | 'await-skip' | 'await-drop'

/** `tab-<paneGroupId>` → pane group id. Anything else is not this path. */
export function paneGroupIdFromTabAgent(agentName: string): string | null {
  if (!agentName.startsWith('tab-')) return null
  const id = agentName.slice(4)
  return id.length > 0 ? id : null
}

export function sidecarRefreshMark(paneGroupId: string): SidecarRefreshMark {
  const guard = guards.get(paneGroupId)
  if (!guard) return 'none'
  if (guard.phase === 'inflight') return 'inflight'
  return guard.phase
}

export function resetSidecarRefreshGuards(): void {
  guards.clear()
}

/** Arm before the refresh POST. Replaces any previous mark for this pane. */
export function beginSidecarRefresh(paneGroupId: string): void {
  if (!paneGroupId) return
  guards.set(paneGroupId, { phase: 'inflight', removedSeen: false })
}

/**
 * Apply the POST result.
 * - success: keep. If the remove was already skipped, clear. If not, leave
 *   the mark so the late SessionRemoved is skipped once.
 * - `v2 spawn failed…`: drop now if the remove was skipped; otherwise the
 *   next SessionRemoved passes through (and is not skipped).
 * - 409/400 (`sidecar refresh has no resumable session`, `sidecar refresh
 *   has no provider session id`, `sidecar refresh would re-exec
 *   --session-id`, `agent_name required`, `cwd required`) and any other
 *   rejection: clear a waiting mark and keep. Do not leave a mark that
 *   would swallow a later real exit.
 */
export function settleSidecarRefresh(
  paneGroupId: string,
  outcome: SidecarRefreshOutcome,
): SidecarRefreshVerdict {
  const guard = guards.get(paneGroupId)
  if (!guard || guard.phase !== 'inflight') return 'keep'

  if (outcome.ok) {
    if (guard.removedSeen) guards.delete(paneGroupId)
    else guards.set(paneGroupId, { phase: 'await-skip' })
    return 'keep'
  }

  if (isSpawnFailure(outcome.message)) {
    if (guard.removedSeen) {
      guards.delete(paneGroupId)
      return 'drop'
    }
    guards.set(paneGroupId, { phase: 'await-drop' })
    return 'keep'
  }

  guards.delete(paneGroupId)
  return 'keep'
}

/**
 * `skip` — this refresh's SessionRemoved; caller must not change the strip.
 * `passthrough` — no skip owed (including a spawn failure whose remove has
 * not been applied yet). Caller runs the existing drop filter.
 */
export function takeSessionRemoved(paneGroupId: string): SessionRemovedTake {
  const guard = guards.get(paneGroupId)
  if (!guard) return 'passthrough'
  if (guard.phase === 'inflight') {
    guard.removedSeen = true
    return 'skip'
  }
  if (guard.phase === 'await-skip') {
    guards.delete(paneGroupId)
    return 'skip'
  }
  guards.delete(paneGroupId)
  return 'passthrough'
}

function isSpawnFailure(message: string): boolean {
  return message.startsWith('v2 spawn failed')
}
