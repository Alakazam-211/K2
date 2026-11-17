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

/** A say frame's time, in the TimePosition shape; null when absent or unreadable. K2's frame carries `time` as decimal ticks and, when the message said what the playhead was in, `time_in` (docs/say.rs); an `{at, in}` object reads too. */
export function readSayTime(raw: unknown, rawIn?: unknown): SayTime | null {
  if (typeof raw === 'string') return DECIMAL.test(raw) ? (typeof rawIn === 'string' && rawIn ? { at: raw, in: rawIn } : { at: raw }) : null
  if (!raw || typeof raw !== 'object') return null
  const t = raw as { at?: unknown; in?: unknown }
  if (typeof t.at !== 'string' || !DECIMAL.test(t.at)) return null
  if (t.in !== undefined && typeof t.in !== 'string') return null
  return typeof t.in === 'string' ? { at: t.at, in: t.in } : { at: t.at }
}
