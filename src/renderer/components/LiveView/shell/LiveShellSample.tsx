// A live tab (scene or timeline) driven by its sample (loaded on demand, so
// the samples stay out of the main bundle). One source per mount, made in an
// effect so every one made is closed (StrictMode mounts twice in dev).

import { useEffect, useState } from 'react'
import { SampleShellSource } from '@/lib/live-view/shared/sample-shell'
import { shellSample, type ShellKind } from '@/lib/live-view/shared/samples'
import { SCENE_SHELL } from '../scene/scene-shell'
import { TIMELINE_SHELL } from '../timeline/timeline-shell'
import { LiveShellView } from './LiveShellView'

export default function LiveShellSample({ kind }: { kind: ShellKind }): React.JSX.Element | null {
  const [source, setSource] = useState<SampleShellSource | null>(null)
  useEffect(() => {
    const s = new SampleShellSource(shellSample(kind), { autoplay: false })
    setSource(s)
    return () => s.close()
  }, [kind])
  if (!source || source.sample.kind !== kind) return null
  return <LiveShellView key={kind} source={source} config={kind === 'scene' ? SCENE_SHELL : TIMELINE_SHELL} />
}
