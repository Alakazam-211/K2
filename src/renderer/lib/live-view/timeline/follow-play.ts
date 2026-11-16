// Following a playhead on the timeline (T5; the video player design
// section's "Follow and Back to the person"), pure. The shell keeps whom you
// follow (lib/live-view/shared/follow.ts); this keeps what only the
// timeline needs:
//   - the clock a followed playhead runs on: the snapshot's own time plus
//     the wall time since it came, so a playhead that plays extrapolates
//     from its time position's at_ms and rate (playheads.ts headOf);
//   - when the one you follow stops (an agent lets go of its hold or its
//     turn ends) or leaves, following ends as the editors end it: "Atlas
//     stopped. Stopped following." with Back to my place and Follow again;
//   - the sample's scripted follows and loops (the mock's `local`);
//   - what the say row points at: the clip under the monitor's playhead.

import type { TimelineModel } from './fold'
import type { MonitorView } from './monitor-view'
import { loopRange } from './picture'
import type { SimState } from './sim'

/** The time a followed playhead reads against: the snapshot's own, run on by the wall time since it came. */
export const followNow = (stateNow: number, since: number, wall: number): number => stateNow + Math.max(0, wall - since)

/** Following ended because they stopped or left; `home` is where your own playhead was when you started following. */
export interface Finished {
  who: string
  name: string
  why: 'stopped' | 'left'
  home: number
}

/** The finished line and its two ways on (no Follow again once they've left). */
export function finishedWords(f: Finished): { text: string; back: string; again: string | null } {
  return f.why === 'left'
    ? { text: `${f.name} left the file. Stopped following.`, back: 'Back to my place', again: null }
    : { text: `${f.name} stopped. Stopped following.`, back: 'Back to my place', again: 'Follow again' }
}

/** The one you follow stopped at this step: they let go of their hold, or their turn ended. */
export function stoppedAt(S: SimState, who: string): boolean {
  return S.log.some((e) => e.i === S.i && e.actor === who && (e.kind === 'turn_end' || (e.kind === 'claim_end' && e.how === 'released')))
}

export type ScriptedFollow = { type: 'follow'; who: string } | { type: 'away'; who: string } | { type: 'stop' }

/** What the sample's step scripts for Follow: press Follow (the agent, or someone by name), look away from someone (Back to them shows), or stop. */
export function scriptedFollow(S: SimState, agent: string | null): ScriptedFollow | null {
  const l = S.local
  if (!l || l.at !== S.i) return null
  if (l.left) return null // their leaving ends following (the shell sees them go)
  if (l.back) return { type: 'away', who: l.back }
  if (!('follow' in l) || l.follow === undefined) return null
  if (l.follow === false) return { type: 'stop' }
  const who = l.follow === true ? agent : l.follow
  return who ? { type: 'follow', who } : null
}

/** A loop the sample's step starts (you clicked a change's bracket): 1 s before the change to 1 s after. */
export function scriptedLoop(S: SimState, M: TimelineModel, whoName: string): { f0: number; f1: number; who: string; verb: string | null } | null {
  const l = S.local
  if (!l || l.at !== S.i || !l.loop) return null
  const r = loopRange(M, { f0: l.loop.f0, f1: l.loop.f1 })
  return { f0: r.f0, f1: r.f1, who: whoName, verb: l.loop.verb }
}

/** What the say row points at: the clip under the monitor's playhead by its unit key, else the time; both carry the time in ticks. */
export function sayAtOf(M: TimelineModel, v: MonitorView): { label: string; title: string; address: string; time: string } {
  const f = Math.max(0, Math.round(v.head.f))
  const time = String(BigInt(f) * M.FR)
  const it = v.picture?.it
  return it ? { label: it.name, title: `${it.name} at ${M.tcF(f)}`, address: it.key, time } : { label: 'a gap', title: `a gap at ${M.tcF(f)}`, address: `@time:${time}`, time }
}
