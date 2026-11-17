// The scene's hero on the shared shell: the stage for a scene snapshot,
// with its picture source: the docs preview route's stills when the daemon
// draws this file, else the sample's stand-in (useScenePictures).

import { isSceneSnapshot } from '@/lib/live-view/scene/sample-source'
import type { ShellHeroProps } from '../shell/shell-config'
import { StagePlaceholder } from '../shell/StagePlaceholder'
import { SceneStage } from './SceneStage'
import { useScenePictures } from './useScenePictures'

export function SceneHero(p: ShellHeroProps): React.JSX.Element {
  const scene = isSceneSnapshot(p.snap) ? p.snap.scene : null
  const source = useScenePictures(scene?.S ?? null)
  if (!scene) {
    const line = p.state === 'loading' ? 'Opening the scene. The list of objects comes first, then the picture.' : 'The scene’s picture shows here: who’s looking where, and each change as it lands.'
    return <StagePlaceholder kind="scene" line={line} p={p} />
  }
  return <SceneStage S={scene.S} control={scene.control} meshEpoch={scene.meshEpoch ?? 0} source={source} narrow={p.narrow} offscreen={p.offscreen} onJump={p.onJump} follow={p.follow} setSayAt={p.setSayAt} people={p.snap.people} refusal={p.refusal} ask={p.ask} />
}
