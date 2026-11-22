// The workbook's "Sample data" banner: a thin adapter over the shell's
// (shell/SampleBanner.tsx) for the workbook's sample source and its three
// scenarios.

import { useEffect, useState } from 'react'
import type { SampleScenario, SampleStatus, SampleWorkbookSource } from '@/lib/live-view/sample-source'
import { SampleBannerView, SAMPLE_BANNER_TITLE } from './shell/SampleBanner'
import { PRODUCT_NAME } from '@/lib/brand'

export { SAMPLE_BANNER_TITLE }
export const SAMPLE_BANNER_LINE = `A preview of watching a workbook live: an agent’s turn played from ${PRODUCT_NAME}’s sample workbook. Nothing here comes from the file you opened.`

const SCENARIOS: readonly { value: SampleScenario; label: string }[] = [
  { value: 'turn', label: 'Live turn' },
  { value: 'blob', label: 'Too large' },
  { value: 'none', label: 'Not opened' },
]

export function SampleBanner({ sample, scenario, onScenario }: { sample: SampleWorkbookSource; scenario: SampleScenario; onScenario: (s: SampleScenario) => void }): React.JSX.Element {
  const [st, setSt] = useState<SampleStatus>(() => sample.status())
  useEffect(() => {
    setSt(sample.status())
    return sample.onStatus(setSt)
  }, [sample])
  return (
    <SampleBannerView<SampleScenario>
      line={SAMPLE_BANNER_LINE}
      scenarios={SCENARIOS}
      scenario={scenario}
      onScenario={onScenario}
      status={st}
      onPlay={() => sample.play()}
      onPause={() => sample.pause()}
      onStep={() => sample.step()}
      onRestart={() => sample.restart()}
    />
  )
}
