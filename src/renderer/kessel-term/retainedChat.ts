// Pinned-chat background retention — the MRU/cap policy, pure.
// Design: .k2/notes/pinned-chat-background-render-design.md §2.1/§2.5.
//
// The retained set is DERIVED, never stored: it is the top-`cap`
// most-recently-VISITED workspaces (visit = workspace foregrounded this
// session) whose exemption predicate still holds — membership in the
// canonical Active section is the only clause evaluated here (the
// capability/kill-switch clauses gate at the component layer, and
// "session exists" is observed by the pane itself). Deriving instead of
// storing makes every transition — visit, Active-leave, cap shrink/grow,
// pin/unpin — a recompute with no bookkeeping to desync.
//
// Cap: `max(BASE_RETAINED_CAP, pinnedToTopCount)` — owner decision. The
// base cap is 5; when the user has MORE than 5 workspaces pinned to the
// top of the Active section (`manually_active`), the cap grows to the
// pinned count so pinning is never silently defeated by the cap.
// Interpretation note (flagged for owner veto): "over 6, let it grow"
// is read as max(5, pinnedCount) — fewer-than-5 pins never SHRINKS the
// cap below 5.
//
// Kept as standalone pure functions (no React, no zustand, no DOM) so
// the policy truth table is exhaustively unit-testable — same pattern
// as activeViewer.ts.

/** Base retained-set size (foreground + background pinned chats). */
export const BASE_RETAINED_CAP = 5

/**
 * Effective retained-set cap: the base cap, grown to the number of
 * workspaces pinned to the top of the Active section when that number
 * exceeds it. Never below the base cap.
 */
export function retainedCap(
  pinnedToTopCount: number,
  baseCap: number = BASE_RETAINED_CAP,
): number {
  return Math.max(baseCap, pinnedToTopCount)
}

/**
 * MRU order after a visit: `projectId` moves to the front; everything
 * else keeps its relative order. Idempotent for the current front
 * element. Always returns a fresh array (callers store it immutably).
 */
export function recordVisitOrder(
  mruOrder: readonly string[],
  projectId: string,
): string[] {
  return [projectId, ...mruOrder.filter((id) => id !== projectId)]
}

export interface RetainedSetInputs {
  /** Visit order, most-recently-foregrounded first. */
  mruOrder: readonly string[]
  /** The canonical daemon-owned Active set (useActiveStore mirror). */
  activeProjectIds: ReadonlySet<string>
  /** Workspaces pinned to the top of the Active section (manually_active ∩ Active). */
  pinnedToTopCount: number
  baseCap?: number
}

/**
 * The retained set, in MRU order: the first `cap` entries of `mruOrder`
 * that are still in the canonical Active set. Everything past the cap
 * is evicted (detached + unmounted by the retainer); a non-Active entry
 * is skipped, NOT counted against the cap. Re-visiting re-enters at the
 * front.
 */
export function computeRetainedSet(inputs: RetainedSetInputs): string[] {
  const cap = retainedCap(
    inputs.pinnedToTopCount,
    inputs.baseCap ?? BASE_RETAINED_CAP,
  )
  const retained: string[] = []
  for (const id of inputs.mruOrder) {
    if (!inputs.activeProjectIds.has(id)) continue
    retained.push(id)
    if (retained.length >= cap) break
  }
  return retained
}

/**
 * Boot seeding (eager attach): append `bootOrder` (the Active-section
 * list order — pinned-to-top first) behind any visits already recorded,
 * bounding the TOTAL order to `cap` so boot never pre-attaches more
 * than the cap allows. Real visits always outrank seeds — a seed never
 * displaces or reorders an existing entry.
 */
export function seedBootOrder(
  existing: readonly string[],
  bootOrder: readonly string[],
  cap: number,
): string[] {
  const seeded = [...existing]
  for (const id of bootOrder) {
    if (seeded.length >= cap) break
    if (!seeded.includes(id)) seeded.push(id)
  }
  return seeded
}

/**
 * Active-section membership tracking: a workspace that LEAVES the
 * Active section is dropped from the visit order entirely, so a later
 * re-JOIN does not auto-attach it (owner decision: joining only
 * attaches on boot seeding or a fresh visit). Relative order of the
 * survivors is preserved.
 */
export function pruneOrderToActive(
  mruOrder: readonly string[],
  activeProjectIds: ReadonlySet<string>,
): string[] {
  return mruOrder.filter((id) => activeProjectIds.has(id))
}
