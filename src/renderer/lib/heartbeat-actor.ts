// Heartbeat history actor words (prd-app-heartbeats-surface-v1 AH18).
//
// The daemon stamps `heartbeat_fires.actor` on every `changed` row and on
// every row a manual or app fire writes:
//   owner-token | user:<name> | agent:<handle> | app:<username> | app-token:<name>
// Scheduler rows carry `null`. This is the renderer twin of
// `k2_core::heartbeats::actor_phrase` — KEEP THE TWO IN LOCKSTEP.

/** "the owner", "user rosson", "agent sales", "app user bob", "app token
 *  kiosk"; `null` for a scheduler row. Unknown shapes are shown as-is. */
export function heartbeatActorPhrase(actor: string | null | undefined): string | null {
  const a = (actor ?? '').trim()
  if (!a) return null
  if (a === 'owner-token') return 'the owner'
  const prefixes: Array<[string, string]> = [
    ['user:', 'user'],
    ['agent:', 'agent'],
    ['app:', 'app user'],
    ['app-token:', 'app token'],
  ]
  for (const [prefix, words] of prefixes) {
    if (a.startsWith(prefix)) return `${words} ${a.slice(prefix.length)}`
  }
  return a
}

/** One History line naming who did it, or `null` when there is nothing to
 *  add (a scheduler fire keeps its own reason line).
 *  - `changed` rows: "disabled by app user bob", "instructions edited by
 *    user rosson", "renamed from x by agent sales".
 *  - fire rows with an actor: "fired by agent sales". */
export function heartbeatHistoryActorLine(f: {
  decision: string
  reason: string | null
  actor?: string | null
}): string | null {
  const who = heartbeatActorPhrase(f.actor)
  if (f.decision === 'changed') {
    return `${f.reason ?? 'changed'} by ${who ?? 'the scheduler'}`
  }
  if (!who) return null
  const verb =
    f.decision === 'fired' || f.decision === 'fired_catchup'
      ? 'fired'
      : f.decision.startsWith('skipped_')
        ? 'skipped'
        : f.decision
  return `${verb} by ${who}`
}
