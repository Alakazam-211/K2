// Presence colours for everyone else on a live view (`--color-presence-1..4`):
// the first seen takes the first free slot and keeps it while here; one who
// leaves frees theirs; past the order's length the slots repeat. The blue (2)
// reads close to your own accent, so it goes last, or never (NO_BLUE) where
// your own blue is on screen too. Used by the timeline and the DXF viewer.

/** A presence slot (`--color-presence-1..4`). */
export type PresenceSlot = 1 | 2 | 3 | 4

/** The order others take slots: the blue (2) last. */
export const OTHERS_FIRST: readonly PresenceSlot[] = [1, 3, 4, 2]

/** The same order without the blue: your own colour is never anyone else's. */
export const NO_BLUE: readonly PresenceSlot[] = [1, 3, 4]

/** Each slot's token, spelled out (the Styles shield reads every variable by name). */
export const PRESENCE_TOKEN: Record<PresenceSlot, string> = { 1: 'var(--color-presence-1)', 2: 'var(--color-presence-2)', 3: 'var(--color-presence-3)', 4: 'var(--color-presence-4)' }

/** A colour that is one of the presence tokens (inked on the background, as the avatars are). */
export const isPresenceToken = (c: string): boolean => (Object.values(PRESENCE_TOKEN) as string[]).includes(c)

/** Everyone's slot now, from the slots before and who is here, in the order first seen. Pure. */
export function presenceSlots(
  prev: ReadonlyMap<string, PresenceSlot>,
  others: readonly string[],
  order: readonly PresenceSlot[] = OTHERS_FIRST,
): Map<string, PresenceSlot> {
  const next = new Map<string, PresenceSlot>()
  for (const id of others) {
    const s = prev.get(id)
    if (s !== undefined && order.includes(s) && ![...next.values()].includes(s)) next.set(id, s)
  }
  others.forEach((id, i) => {
    if (next.has(id)) return
    const used = new Set(next.values())
    next.set(id, order.find((s) => !used.has(s)) ?? order[i % order.length])
  })
  return next
}
