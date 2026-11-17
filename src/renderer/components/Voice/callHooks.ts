// The call's shared hooks: whether video is on in it, and its time. Every
// place that shows the call reads these (the stage, the slim bar, full
// screen, the call's own window).

import { useEffect, useMemo, useState } from 'react'
import { useStore } from 'zustand'
import { videoLive } from '@/lib/voice/call-tile-picture'
import { videoTiles, type CallPerson, type VideoTile as Tile } from '@/lib/voice/video-labels'
import type { VideoController } from '@/stores/video-core'
import { callDuration } from './voice-labels'

/** The call's video tiles, as the stage draws them. */
export function useCallTiles(video: VideoController, people: readonly CallPerson[]): Tile[] {
  const tracks = useStore(video.store, (s) => s.tracks)
  const local = useStore(video.store, (s) => s.local)
  return useMemo(() => videoTiles(tracks, local, people), [tracks, local, people])
}

/** Video is on in the call (anyone's camera or a screen share). */
export function useCallVideoLive(video: VideoController, people: readonly CallPerson[]): boolean {
  const ready = useStore(video.store, (s) => s.availability === 'ready')
  const tiles = useCallTiles(video, people)
  return ready && videoLive(tiles)
}

/** The call's time, ticking on the call's own second while mounted. Null
 *  until the call connects. */
export function useCallDuration(startedAt: number | null): string | null {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (startedAt === null) return
    let timer: ReturnType<typeof setTimeout> | null = null
    const tick = (): void => {
      const t = Date.now()
      setNow(t)
      timer = setTimeout(tick, 1000 - ((((t - startedAt) % 1000) + 1000) % 1000))
    }
    tick()
    return () => {
      if (timer) clearTimeout(timer)
    }
  }, [startedAt])
  return startedAt === null ? null : callDuration(startedAt, now)
}
