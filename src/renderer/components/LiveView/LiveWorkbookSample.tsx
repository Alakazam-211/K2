// The live view driven by the sample source (loaded on demand: the goldens
// stay out of the main bundle). One source per scenario, made in an effect so
// every one made is closed (StrictMode mounts twice in dev).

import { useEffect, useState } from 'react'
import { SAMPLE_FILE_NAME, SampleWorkbookSource, type SampleScenario } from '@/lib/live-view/sample-source'
import { LiveWorkbookView } from './LiveWorkbookView'

export default function LiveWorkbookSample({ autoplay = true }: { autoplay?: boolean }): React.JSX.Element | null {
  const [scenario, setScenario] = useState<SampleScenario>('turn')
  const [source, setSource] = useState<SampleWorkbookSource | null>(null)
  useEffect(() => {
    const s = new SampleWorkbookSource({ scenario, autoplay })
    setSource(s)
    return () => s.close()
  }, [scenario, autoplay])
  if (!source || source.scenario !== scenario) return null
  return <LiveWorkbookView key={scenario} source={source} fileName={SAMPLE_FILE_NAME} sample={{ source, scenario, onScenario: setScenario }} />
}
