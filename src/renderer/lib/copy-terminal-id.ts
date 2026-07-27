/**
 * Resolve the id agents / CLI should use for `k2 terminal read|write`.
 *
 * Two id spaces exist until full unification (see post-landing-cleanup PRD):
 * - Renderer `terminalId` — local tab bookkeeping key
 * - Daemon `session_id` — what `/cli/terminal/{write,read}` and the
 *   v2 session map actually address
 *
 * TerminalPane registers renderer→daemon in the pinned-size store at
 * spawn resolve. Prefer that daemon UUID whenever live; fall back to
 * `attachAgentName` (v2_session_map key for surfaced/heartbeat tabs),
 * then the renderer terminalId (legacy/alacritty path).
 *
 * **Not gated on `command`.** Kessel/v2 layouts deliberately drop
 * `command` on serialize (daemon owns the PTY), so requiring it hid
 * "Copy Terminal ID" in GA while fresh dev sessions still showed it.
 */

export interface CopyableTerminalItem {
  terminalId?: string
  attachAgentName?: string
}

/** Pure: first terminal item → best copyable id, or null if none. */
export function resolveCopyableTerminalId(
  items: ReadonlyArray<{ type: string; data: unknown }>,
  sessions: Readonly<Record<string, string>>,
): string | null {
  for (const item of items) {
    if (item.type !== 'terminal') continue
    const d = item.data as CopyableTerminalItem
    const tid = d.terminalId
    if (!tid) continue
    // Daemon session UUID (preferred for `k2 terminal write <id>`).
    const daemonSid = sessions[tid] ?? sessions[`${tid}-shell`]
    if (daemonSid) return daemonSid
    // Surfaced / heartbeat tabs already know their v2_session_map key.
    if (d.attachAgentName) return d.attachAgentName
    return tid
  }
  return null
}
