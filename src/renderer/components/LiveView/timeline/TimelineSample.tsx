// The live timeline tab driven by its sample (loaded on demand, so the
// sample world stays out of the main bundle): the shared shell with the
// timeline's own sample source, which plays the mock's sample turn and
// scenarios. One source per mount, made in an effect so every one made is
// closed (StrictMode mounts twice in dev).

import { useEffect, useState } from 'react'
import { TimelineSampleSource } from '@/lib/live-view/timeline/sample-source'
import { LiveShellView } from '../shell/LiveShellView'
import { TIMELINE_SHELL } from './timeline-shell'

export default function TimelineSample({ width }: { width?: number }): React.JSX.Element | null {
  const [source, setSource] = useState<TimelineSampleSource | null>(null)
  useEffect(() => {
    const s = new TimelineSampleSource()
    setSource(s)
    return () => s.close()
  }, [])
  if (!source) return null
  return <LiveShellView source={source} config={TIMELINE_SHELL} width={width} />
}
