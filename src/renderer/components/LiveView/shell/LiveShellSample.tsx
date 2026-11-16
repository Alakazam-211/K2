// A live tab (scene or timeline) driven by its sample (loaded on demand, so
// the samples stay out of the main bundle). One source per mount, made in an
// effect so every one made is closed (StrictMode mounts twice in dev). The
// scene plays the mock's 24 scenarios (lib/live-view/scene/sample-source.ts);
// the timeline its shell sample until its own phase. When the scene's
// picture is the file's own, the banner says so (picture-real.ts).

import { useEffect, useMemo, useState } from 'react'
import { SampleShellSource } from '@/lib/live-view/shared/sample-shell'
import { shellSample, type ShellKind } from '@/lib/live-view/shared/samples'
import { SampleSceneSource } from '@/lib/live-view/scene/sample-source'
import { PictureRealContext, REAL_PICTURE_BANNER } from '../scene/picture-real'
import { SCENE_SHELL } from '../scene/scene-shell'
import { TIMELINE_SHELL } from '../timeline/timeline-shell'
import { LiveShellView } from './LiveShellView'

export default function LiveShellSample({ kind }: { kind: ShellKind }): React.JSX.Element | null {
  const [source, setSource] = useState<SampleShellSource | null>(null)
  // The scene's picture may be the file's own (the daemon draws it): the banner then says so.
  const [realPicture, setRealPicture] = useState(false)
  const config = useMemo(() => (kind !== 'scene' ? TIMELINE_SHELL : realPicture ? { ...SCENE_SHELL, bannerTitle: REAL_PICTURE_BANNER.title, bannerLine: REAL_PICTURE_BANNER.line } : SCENE_SHELL), [kind, realPicture])
  useEffect(() => {
    const s = kind === 'scene' ? new SampleSceneSource({ autoplay: false }) : new SampleShellSource(shellSample(kind), { autoplay: false })
    setSource(s)
    return () => s.close()
  }, [kind])
  if (!source || source.sample.kind !== kind) return null
  return (
    <PictureRealContext.Provider value={setRealPicture}>
      <LiveShellView key={kind} source={source} config={config} />
    </PictureRealContext.Provider>
  )
}
