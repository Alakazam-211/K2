// Where in time a message points (`docs/say`'s `time`), in the playhead's
// own shape: the kit's TimePosition (participant.rs; the client's
// TimePositionJson), `at` a decimal integer string in the pack's time unit
// (a timeline's ticks) and `in` the address of the sequence it's in. The kit
// has no time address: an address names an element, and a gap isn't one,
// so a message about a gap points at its sequence and carries the time here.

import type { TimePositionJson } from '@k2/mpc-client'

export type SayTime = Pick<TimePositionJson, 'at' | 'in'>

const DECIMAL = /^-?(0|[1-9]\d*)$/

/** A message's time, built as the playhead's time position is: at `at` (ticks), in the sequence `within`. */
export function sayTime(at: bigint | string, within: string): SayTime {
  return { at: String(at), in: within }
}

/** A say frame's `time`, in the TimePosition shape; null when absent or unreadable. */
export function readSayTime(raw: unknown): SayTime | null {
  // TODO(0.46.15): delete the bare-ticks form; senders before 0.46.14 sent `time` as decimal ticks.
  if (typeof raw === 'string') return DECIMAL.test(raw) ? { at: raw } : null
  if (!raw || typeof raw !== 'object') return null
  const t = raw as { at?: unknown; in?: unknown }
  if (typeof t.at !== 'string' || !DECIMAL.test(t.at)) return null
  if (t.in !== undefined && typeof t.in !== 'string') return null
  return typeof t.in === 'string' ? { at: t.at, in: t.in } : { at: t.at }
}
