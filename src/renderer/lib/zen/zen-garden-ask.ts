// Rosson 2026-10-04 — "+ New Garden → Start empty and ask my agent".
//
// The new Garden is made empty (`k2.blank@1`) and this window switches to
// it; its empty-Garden widget then opens Ask my agent by itself, once. The
// request is per window (module state), for one Garden id, and expires so
// a Garden opened much later never pops the chooser.

/** How long a request waits for the empty Garden to show (ms). */
export const ZEN_GARDEN_ASK_TTL_MS = 30_000

let pending: { gardenId: string; at: number } | null = null

/** Ask my agent should open when Garden `gardenId` shows its empty page. */
export function requestZenGardenAsk(gardenId: string, now: number = Date.now()): void {
  pending = { gardenId, at: now }
}

/** True once for the Garden that asked, within the TTL; clears the request. */
export function takeZenGardenAsk(gardenId: string | null | undefined, now: number = Date.now()): boolean {
  if (!pending || !gardenId || pending.gardenId !== gardenId) return false
  const fresh = now - pending.at <= ZEN_GARDEN_ASK_TTL_MS
  pending = null
  return fresh
}

/** Tests only. */
export function __resetZenGardenAskForTests(): void {
  pending = null
}
