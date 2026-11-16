// Presence colours on the text (build-editors.md E4b): each other person or
// agent in a live file gets one of the look's four presence tokens
// (`--color-presence-1..4`, 3:1 or more as a caret on the editor in every
// look). The first one seen gets 1, the next 2, and so on; a person keeps
// theirs while they're in the file, and a slot frees when they leave. Past
// four people the slots repeat. The carets, selections, name flags and holds
// all use the slot, so one person reads as one colour everywhere on the text.
// The avatars keep the roster's own colour (the shell's).

export type Slot = 1 | 2 | 3 | 4

/** The slots now: who keeps theirs, who is new, who left. Pure. */
export function assignSlots(prev: ReadonlyMap<string, Slot>, people: readonly { id: string; self?: boolean }[]): Map<string, Slot> {
  const others = people.filter((p) => !p.self)
  const next = new Map<string, Slot>()
  for (const p of others) {
    const s = prev.get(p.id)
    if (s !== undefined) next.set(p.id, s)
  }
  others.forEach((p, i) => {
    if (next.has(p.id)) return
    const used = new Set(next.values())
    const free = ([1, 2, 3, 4] as const).find((s) => !used.has(s))
    next.set(p.id, free ?? (((i % 4) + 1) as Slot))
  })
  return next
}

/** Each slot's token, spelled out (the Styles shield reads every variable by name). */
export const SLOT_COLOURS = ['var(--color-presence-1)', 'var(--color-presence-2)', 'var(--color-presence-3)', 'var(--color-presence-4)'] as const

/** The token for a slot. */
export const slotColour = (s: Slot): string => SLOT_COLOURS[s - 1]

const cssString = (s: string): string => `"${s.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\a ')}"`

/** The kit draws others' carets and selections in the roster's colour, inline
 *  (`--mpc-user`); this puts each person's slot over it, by their caret key
 *  (`data-mpc-caret`: the handle, or `handle#client` per connection). */
export function caretSlotCss(slots: ReadonlyMap<string, Slot>, scope = '.ed-tab'): string {
  let css = ''
  for (const [id, s] of slots) {
    const v = slotColour(s)
    css += `${scope} [data-mpc-caret=${cssString(id)}], ${scope} [data-mpc-caret^=${cssString(`${id}#`)}] { --mpc-user: ${v} !important; --mpc-user-fg: var(--color-bg) !important; }\n`
  }
  return css
}
