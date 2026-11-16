// Your own playback once you take over the monitor (Play, a watching key,
// the scrub bar, a card's Skip): the frame and the rate, advanced on
// animation frames while it plays (at most once a frame of the edit, so the
// lanes redraw at the edit's rate, not the screen's), looping a change
// when asked, stopping at the end. A new sample step or file starts over
// on the scenario's own playhead, as the mock does.

import { useEffect, useRef, useState } from 'react'
import { advance } from '@/lib/live-view/timeline/keys'
import type { LocalPlay } from '@/lib/live-view/timeline/monitor-view'

export function usePlayback(key: string, endF: number, fps: number): [LocalPlay | null, (l: LocalPlay | null) => void] {
  const [local, setLocal] = useState<LocalPlay | null>(null)
  const [seen, setSeen] = useState(key)
  if (seen !== key) {
    setSeen(key)
    setLocal(null)
  }
  const cur = useRef(local)
  cur.current = local
  const running = !!local && local.rate !== 0 && !local.ended
  useEffect(() => {
    if (!running || typeof requestAnimationFrame === 'undefined') return
    let raf = 0
    let last = performance.now()
    let acc = 0
    const tick = (now: number): void => {
      const dt = Math.min(0.1, (now - last) / 1000)
      last = now
      acc += dt
      const c = cur.current
      if (c && acc * fps * Math.abs(c.rate) >= 1) {
        const next = advance(c, acc, fps, endF)
        acc = 0
        cur.current = next
        setLocal(next)
        if (!next.rate || next.ended) return
      }
      raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  }, [running, endF, fps])
  return [local, setLocal]
}
