// Follow on the scene (S5), the picture's side of it. The shell keeps whom
// you follow (useLiveShell: Follow, Stop, Back, "left the file"); this:
//   - shows their view: the picture is the view they look through;
//   - looks away when you choose a view yourself (the picker, a key, a chip
//     on the edge): the shell then offers Back to them;
//   - remembers the view you had when you started following, and ends
//     following with the editors' and the timeline's words when they stop
//     or leave (Back to my place, Follow again); you keep their view;
//   - plays the sample's scripted follows (the mock's `watch` and `view`).

import { useEffect, useRef, useState } from 'react'
import { followEndWords, scriptedFollow, stoppedAt, type FollowEnd } from '@/lib/live-view/scene/follow-view'
import { peerView } from '@/lib/live-view/scene/panel'
import type { SimState } from '@/lib/live-view/scene/sim'
import { SCENE_VIEW } from '@/lib/live-view/scene/view-address'
import { personById } from '@/lib/live-view/shared/people'
import type { ShellHeroProps } from '../shell/shell-config'

export interface SceneFollow {
  /** Whom you follow (the shell's), else null. */
  whose: string | null
  /** Their view while you follow them. */
  theirView: string | null
  /** You chose a view yourself: following looks away (Back to them shows). */
  lookAway: () => void
  /** Follow someone (a chip by the picker, the second pane's "Atlas's view"). */
  follow: (who: string) => void
  /** Following ended: the line and its two ways on. */
  ended: { text: string; back: string; again: string | null } | null
  /** The view you had before you followed (Back to my place), and clears the line. */
  backHome: () => string | null
  followAgain: () => void
}

export function useSceneFollow(p: Pick<ShellHeroProps, 'follow' | 'snap'>, S: SimState, ownView: string, keep: (view: string) => void): SceneFollow {
  const f = p.follow
  const dispatch = f?.dispatch
  const whose = f && f.state.kind === 'following' ? f.id : null
  // Their view while they're here (once they've gone, you keep the last one).
  const theirView = whose && S.peers[whose] ? (peerView(S, whose) ?? SCENE_VIEW) : null

  // The view you had before following, and theirs (where you stay when following ends).
  const own = useRef(ownView)
  if (!whose) own.current = ownView
  const home = useRef<string | null>(null)
  const last = useRef<string | null>(null)
  if (theirView) last.current = theirView
  const prev = useRef<string | null>(null)
  const [end, setEnd] = useState<FollowEnd | null>(null)
  useEffect(() => {
    if (whose && !prev.current) {
      if (home.current == null) home.current = own.current
      setEnd(null)
    }
    prev.current = whose
  }, [whose])

  /** Following ended: say so, and stay on their view. */
  const finish = (e: FollowEnd): void => {
    setEnd(e)
    if (last.current) keep(last.current)
  }

  // They stopped at this step (their turn ended, or they let go): following ends.
  const seen = useRef<SimState | null>(null)
  useEffect(() => {
    if (!whose || !dispatch || seen.current === S || !stoppedAt(S, whose)) return
    seen.current = S
    finish({ who: whose, name: personById(p.snap.people, whose)?.name ?? 'They', why: 'stopped' })
    dispatch({ type: 'stop' })
  }, [S, whose, dispatch]) // eslint-disable-line react-hooks/exhaustive-deps
  // They left the file (the shell saw them go).
  const left = f?.state.kind === 'left' ? f.state : null
  useEffect(() => {
    if (left) finish({ who: left.who, name: left.name, why: 'left' })
  }, [left?.who]) // eslint-disable-line react-hooks/exhaustive-deps

  // The sample's scripted follows, once per step.
  useEffect(() => {
    if (!dispatch) return
    const s = scriptedFollow(S)
    if (!s) return
    if (s.type === 'stop') dispatch({ type: 'stop' })
    else {
      dispatch({ type: 'follow', who: s.who })
      if (s.type === 'away') dispatch({ type: 'look-away' })
    }
  }, [S]) // eslint-disable-line react-hooks/exhaustive-deps

  return {
    whose,
    theirView,
    lookAway: () => {
      if (whose) dispatch?.({ type: 'look-away' })
      setEnd(null)
    },
    follow: (who) => dispatch?.({ type: 'follow', who }),
    ended: end ? followEndWords(end) : null,
    backHome: () => {
      const h = home.current
      home.current = null
      setEnd(null)
      return h
    },
    followAgain: () => {
      if (!end) return
      dispatch?.({ type: 'follow', who: end.who })
      setEnd(null)
    },
  }
}
