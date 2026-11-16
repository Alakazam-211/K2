// The scene's hero on the shared shell: the stage for a scene snapshot,
// with its picture source. The sample's source is the stand-in (loaded on
// demand, sample only) drawing the doc at each rev the sample has seen;
// S7 swaps in the docs preview client behind the same PictureSource.

import { useEffect, useRef, useState } from 'react'
import { isSceneSnapshot } from '@/lib/live-view/scene/sample-source'
import type { PictureSource } from '@/lib/live-view/scene/picture-source'
import type { SimState } from '@/lib/live-view/scene/sim'
import type { ShellHeroProps } from '../shell/shell-config'
import { StagePlaceholder } from '../shell/StagePlaceholder'
import { SceneStage } from './SceneStage'

/** The sample's picture source, made once per tab; it reads the newest step's docs. */
function useSamplePictures(S: SimState | null): PictureSource | null {
  const latest = useRef(S)
  latest.current = S
  const [source, setSource] = useState<PictureSource | null>(null)
  useEffect(() => {
    let made: PictureSource | null = null
    let live = true
    void import('@/lib/live-view/scene/stand-in/stand-in-stills').then(({ StandInStills }) => {
      if (!live) return
      made = new StandInStills((rev) => {
        const cur = latest.current
        return cur ? (cur.revDocs[rev] ?? cur.D ?? null) : null
      })
      setSource(made)
    })
    return () => {
      live = false
      made?.close()
    }
  }, [])
  return source
}

export function SceneHero(p: ShellHeroProps): React.JSX.Element {
  const scene = isSceneSnapshot(p.snap) ? p.snap.scene : null
  const source = useSamplePictures(scene?.S ?? null)
  if (!scene) {
    const line = p.state === 'loading' ? 'Opening the scene. The list of objects comes first, then the picture.' : 'The scene’s picture shows here: who’s looking where, and each change as it lands.'
    return <StagePlaceholder kind="scene" line={line} p={p} />
  }
  return <SceneStage S={scene.S} control={scene.control} source={source} narrow={p.narrow} offscreen={p.offscreen} onJump={p.onJump} />
}
