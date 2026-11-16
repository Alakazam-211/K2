// Follow on the timeline (T5), the tab's side of it. The shell keeps whom
// you follow; this:
//   - runs the clock a followed playhead reads against while they play, so
//     the monitor extrapolates from their time position (follow-play.ts);
//   - looks away when you take the monitor over (a watching key, a scrub, a
//     scroll of the lanes, a loop): the shell then offers Back to them in
//     the transport, and your playhead starts where theirs was;
//   - follows again on Back (your own playback ends there);
//   - remembers where your playhead was when you started following, and
//     ends following with the editors' finished words when they stop or
//     leave (Back to my place, Follow again);
//   - plays the sample's scripted follows (the mock's `local`).

import { useEffect, useRef, useState } from 'react'
import { personById } from '@/lib/live-view/shared/people'
import { followNow, scriptedFollow, stoppedAt, type Finished } from '@/lib/live-view/timeline/follow-play'
import type { LocalPlay } from '@/lib/live-view/timeline/monitor-view'
import type { SimState } from '@/lib/live-view/timeline/sim'
import { nowOf } from '@/lib/live-view/timeline/snapshot'
import type { ShellHeroProps } from '../shell/shell-config'

export interface TimelineFollow {
  /** Whose playhead the monitor shows (null: yours). */
  whose: string | null
  /** The time a followed playhead reads against. */
  now: number
  finished: Finished | null
  /** You took the monitor over: stop following (Back to them shows). */
  lookAway: () => void
  /** The monitor's playhead each render: yours (kept for Back to my place) or theirs (where you stay when following ends). */
  noteHead: (f: number, self: boolean) => void
  backHome: () => number | null
  followAgain: () => void
}

/** How often a followed playhead that plays redraws (about a frame of the edit). */
const TICK_MS = 1000 / 24

export function useTimelineFollow(p: ShellHeroProps, S: SimState, stepKey: string, setLocal: (l: LocalPlay | null) => void): TimelineFollow {
  const f = p.follow
  const whose = f?.id ?? null
  const dispatch = f?.dispatch

  // The clock: the snapshot's own time, run on while the one you follow plays.
  const followed = whose ? S.peers[whose] : undefined
  const playing = followed?.timePosition?.state === 'playing'
  const since = useRef({ S, at: performance.now() })
  if (since.current.S !== S) since.current = { S, at: performance.now() }
  const [wall, setWall] = useState(() => performance.now())
  useEffect(() => {
    if (!whose || !playing || typeof requestAnimationFrame === 'undefined') return
    let raf = 0
    let last = 0
    const tick = (t: number): void => {
      if (t - last >= TICK_MS) {
        last = t
        setWall(performance.now())
      }
      raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  }, [whose, playing])
  const stateNow = nowOf(S)
  const now = whose && playing ? followNow(stateNow, since.current.at, wall) : stateNow

  // Where your own playhead was, for Back to my place.
  const own = useRef(0)
  const theirs = useRef<number | null>(null)
  /** Following ended: you stay where they were. */
  const stay = (): void => {
    if (theirs.current != null) setLocal({ f: theirs.current, rate: 0 })
  }
  const home = useRef<number | null>(null)
  const [finished, setFinished] = useState<Finished | null>(null)

  // Following someone (Follow, Back, Follow again): your own playback ends there. A fresh follow keeps
  // your place; coming back (Back to them, Follow again) keeps the place you had when you first followed.
  const prev = useRef<{ who: string | null; kind: string }>({ who: null, kind: 'idle' })
  const kind = f?.state.kind ?? 'idle'
  useEffect(() => {
    const was = prev.current
    if (whose && !was.who) {
      if (was.kind !== 'away' && !finished) home.current = own.current
      setLocal(null)
      setFinished(null)
    }
    prev.current = { who: whose, kind }
  }, [whose, kind, setLocal]) // eslint-disable-line react-hooks/exhaustive-deps

  // They stopped at this step, or they left: following ends with the finished words.
  const stopSeen = useRef<SimState | null>(null)
  useEffect(() => {
    // Once per step: Follow again on the same step keeps following.
    if (!whose || !dispatch || stopSeen.current === S || !stoppedAt(S, whose)) return
    stopSeen.current = S
    const name = personById(p.snap.people, whose)?.name ?? 'They'
    setFinished({ who: whose, name, why: 'stopped', home: home.current ?? own.current })
    stay()
    dispatch({ type: 'stop' })
  }, [S, whose, dispatch, p.snap.people])
  const left = f?.state.kind === 'left' ? f.state : null
  useEffect(() => {
    if (!left) return
    setFinished({ who: left.who, name: left.name, why: 'left', home: home.current ?? own.current })
    stay()
  }, [left?.who, left?.name]) // eslint-disable-line react-hooks/exhaustive-deps

  // The sample's scripted follows, once per step.
  const agent = p.snap.agent
  useEffect(() => {
    if (!dispatch) return
    const s = scriptedFollow(S, agent)
    if (!s) return
    if (s.type === 'stop') dispatch({ type: 'stop' })
    else {
      dispatch({ type: 'follow', who: s.who })
      if (s.type === 'away') dispatch({ type: 'look-away' })
    }
  }, [stepKey]) // eslint-disable-line react-hooks/exhaustive-deps

  return {
    whose,
    now,
    finished,
    lookAway: () => {
      if (whose) dispatch?.({ type: 'look-away' })
      setFinished(null)
    },
    noteHead: (fr, self) => {
      if (self) own.current = fr
      else theirs.current = fr
    },
    backHome: () => {
      const h = finished?.home ?? null
      setFinished(null)
      home.current = null
      return h
    },
    followAgain: () => {
      if (!finished) return
      home.current = finished.home
      prev.current = { ...prev.current, kind: 'away' } // coming back: keep the place you had
      dispatch?.({ type: 'follow', who: finished.who })
      setFinished(null)
    },
  }
}
